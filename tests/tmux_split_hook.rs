#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Real-tmux integration test for the `after-split-window` hook that keeps new
//! panes in their agent's worktree (`ensure_split_hook` / `set_window_dispatch_dir`).
//!
//! # Why this needs a real tmux server
//!
//! The mock layer structurally cannot catch the bug these tests guard against.
//! `MockProcessRunner` records argv, so a mock test can assert *"we handed tmux
//! this string"* but never *"tmux did what we meant"* — the defect was purely a
//! matter of tmux's own targeting semantics, and the pre-existing mock test
//! asserted the broken hook string verbatim and stayed green throughout. So the
//! assertions below are about **pane routing**, observed through a real server:
//! the `cd` must reach the newly created pane, and must not reach the board TUI
//! pane or the agent's own pane, both of which consume keystrokes as user input.
//!
//! See `ensure_split_hook` in src/tmux.rs for why the hook needs an explicit
//! `-t #{pane_id}` target, and why the hook exists at all.
//!
//! # Isolation
//!
//! Every test gets its own tmux server on a unique `-L` socket, killed via a drop
//! guard, so a failing assertion or panic cannot leak a server and the
//! developer's own tmux session is never touched. Per-test servers also keep the
//! tests parallel: they cannot share one session, because they mutate
//! session-global state (the active window, and the session-level hook itself).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use dispatch_tui::process::{ProcessRunner, RealProcessRunner};
use dispatch_tui::tmux;

/// How long to wait for the hook's keystrokes to land. The hook fires
/// `run-shell -b`, which is asynchronous, so the write is not visible the
/// instant `split-window` returns. This is a *deadline* for condition polling,
/// never a fixed sleep — see `scripts/check-no-test-sleep.sh` and the
/// "No `tokio::time::sleep` in tests" section of docs/conventions.md. Only the
/// failure path ever pays it in full.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(5);
const POLL_STEP: Duration = Duration::from_millis(25);

/// The agent window under test, named the way dispatch names them (`task-<id>`).
const AGENT_WINDOW: &str = "task-42";
/// The board TUI window — the pane that must never receive hook keystrokes.
const BOARD_WINDOW: &str = "board";
/// Companion-pane width. Production's `AGENT_TREE_PANE_PERCENT` is private to
/// `src/dispatch/agents.rs`; the exact percentage is irrelevant to pane routing,
/// which is what these tests assert.
const PANE_PERCENT: u8 = 30;

/// Owns a private tmux server and kills it on drop, including on panic.
struct TmuxServer {
    socket: String,
}

impl TmuxServer {
    fn start() -> Self {
        // Unique per process AND per thread, so these tests run concurrently
        // (cargo runs them on separate threads by default) without sharing a
        // session. Because the name embeds the pid, it is also guaranteed not to
        // collide with a server left behind by an earlier `cargo test` run.
        let socket = format!(
            "dispatch-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        );
        Self { socket }
    }

    /// Run a tmux command against this server, returning its raw output.
    /// Failures are not asserted here — callers that need that use [`Self::tmux_ok`].
    fn tmux(&self, args: &[&str]) -> std::process::Output {
        // Routed through the same runner the production calls use, so the
        // `-L <socket>` scoping rule lives in exactly one place.
        self.runner()
            .run("tmux", args)
            .expect("failed to invoke tmux")
    }

    /// Run a tmux command and assert it succeeded.
    fn tmux_ok(&self, args: &[&str]) {
        let out = self.tmux(args);
        assert!(
            out.status.success(),
            "tmux {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A `ProcessRunner` that routes production tmux calls to this test server,
    /// by prepending `-L <socket>` to every argument list. This keeps the tests
    /// exercising the real `tmux::*` functions rather than hand-copied command
    /// strings, so they cannot drift from what actually ships.
    fn runner(&self) -> SocketRunner {
        SocketRunner {
            socket: self.socket.clone(),
            inner: RealProcessRunner,
        }
    }
}

impl Drop for TmuxServer {
    fn drop(&mut self) {
        self.tmux(&["kill-server"]);
    }
}

struct SocketRunner {
    socket: String,
    inner: RealProcessRunner,
}

impl ProcessRunner for SocketRunner {
    fn run(&self, program: &str, args: &[&str]) -> anyhow::Result<std::process::Output> {
        if program != "tmux" {
            return self.inner.run(program, args);
        }
        let mut full: Vec<&str> = vec!["-L", &self.socket];
        full.extend_from_slice(args);
        self.inner.run(program, &full)
    }
}

/// Whether tmux is usable in this environment.
fn tmux_available() -> bool {
    Command::new("tmux")
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Spawn a pane command that records everything typed into the pane to `path`.
/// `cat` reads the pane's tty, so any `send-keys` payload lands in the file once
/// a newline (the hook's `Enter`) flushes the line.
fn capture_cmd(path: &Path) -> String {
    format!("cat > {}", path.display())
}

/// Poll until `path` is non-empty or the deadline expires, returning its
/// contents (empty on timeout).
fn read_when_written(path: &Path) -> String {
    let start = Instant::now();
    loop {
        let contents = std::fs::read_to_string(path).unwrap_or_default();
        if !contents.trim().is_empty() || start.elapsed() >= DELIVERY_DEADLINE {
            return contents;
        }
        std::thread::sleep(POLL_STEP);
    }
}

/// Snapshot a capture file without waiting. Only meaningful once the hook has
/// been observed to fire — see [`Fixture::split_agent_window_and_await_hook`].
fn read_now(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

struct Fixture {
    // Declared before `dir` on purpose: fields drop in declaration order, so the
    // server (and its `cat` processes) dies before the temp dir holding their
    // capture files is unlinked.
    server: TmuxServer,
    /// Root for the worktree and capture files. Held for the fixture's lifetime
    /// because the pane processes write into it.
    dir: tempfile::TempDir,
    board_log: PathBuf,
    agent_log: PathBuf,
    new_pane_log: PathBuf,
    worktree: PathBuf,
}

/// Build the fixture, or skip when tmux is unavailable. Folding the guard into
/// construction means a later test cannot forget it and hard-fail on a developer
/// machine without tmux.
///
/// Under CI the missing binary is a hard failure instead. Skipping there would be
/// a silent pass — `eprintln!` is swallowed by the default test harness — which
/// would let a dropped `Install tmux` step or a changed runner image quietly turn
/// this whole file into a no-op, taking the only real coverage of the hook's pane
/// routing with it. This is what makes the CI install step an invariant rather
/// than a convention.
fn setup_or_skip() -> Option<Fixture> {
    if !tmux_available() {
        assert!(
            std::env::var_os("CI").is_none(),
            "tmux is required in CI but was not found on PATH — the workflow's \
             `Install tmux` step must run before `cargo test` (see \
             .github/workflows/ci.yml). Refusing to skip and report green."
        );
        eprintln!("skipping: tmux not available on PATH");
        return None;
    }
    Some(setup())
}

/// Build the topology dispatch actually produces: a focused board window plus a
/// background agent window carrying `@dispatch_dir`, with the production hook
/// installed. Mirrors `resume_agent` / `dispatch_with_prompt` in
/// src/dispatch/agents.rs, which call `set_window_dispatch_dir` then
/// `ensure_split_hook` before splitting the agent window for the companion pane.
fn setup() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let worktree = dir.path().join("worktrees").join("42-some-task");
    std::fs::create_dir_all(&worktree).unwrap();

    let board_log = dir.path().join("board.log");
    let agent_log = dir.path().join("agent.log");
    let new_pane_log = dir.path().join("new_pane.log");

    let server = TmuxServer::start();

    // Window 1: the board TUI. Created first, so it is the active window —
    // exactly the state during a dispatch or resume triggered from the board.
    server.tmux_ok(&[
        "new-session",
        "-d",
        "-s",
        "t",
        "-n",
        BOARD_WINDOW,
        "--",
        "sh",
        "-c",
        &capture_cmd(&board_log),
    ]);

    // Window 2: the agent window, in the background.
    server.tmux_ok(&[
        "new-window",
        "-d",
        "-n",
        AGENT_WINDOW,
        "--",
        "sh",
        "-c",
        &capture_cmd(&agent_log),
    ]);

    let runner = server.runner();
    tmux::set_window_dispatch_dir(AGENT_WINDOW, worktree.to_str().unwrap(), &runner).unwrap();
    tmux::ensure_split_hook(&runner).unwrap();

    Fixture {
        server,
        dir,
        board_log,
        agent_log,
        new_pane_log,
        worktree,
    }
}

impl Fixture {
    /// Split the agent window through the same production call
    /// `spawn_agent_tree_pane` uses (`-d`, so focus stays on the board), then
    /// block until the hook's keystrokes reach the new pane, returning them.
    ///
    /// Returning only after delivery gives every negative assertion a
    /// happens-before anchor: once the hook has demonstrably fired, anything it
    /// misrouted has already been written too. That is what lets those tests
    /// assert an absence without sleeping out the full deadline.
    fn split_agent_window_and_await_hook(&self) -> String {
        self.split_window_capturing(AGENT_WINDOW, &self.new_pane_log);
        read_when_written(&self.new_pane_log)
    }

    /// Split `window` via the production helper, with the new pane capturing
    /// anything typed into it.
    fn split_window_capturing(&self, window: &str, log: &Path) {
        tmux::split_window_horizontal_running(
            window,
            PANE_PERCENT,
            &["sh", "-c", &capture_cmd(log)],
            &self.server.runner(),
        )
        .expect("split-window");
    }
}

#[test]
fn split_hook_cds_the_new_pane_into_the_worktree() {
    let Some(fx) = setup_or_skip() else { return };

    let got = fx.split_agent_window_and_await_hook();

    assert!(
        got.contains(&format!("cd {}", fx.worktree.display())),
        "new pane should be sent `cd <worktree>`, got: {got:?}"
    );
}

/// The regression test for task #3781. Before the fix this failed with the
/// board's log containing the `cd` line.
#[test]
fn split_hook_never_types_into_the_board_window() {
    let Some(fx) = setup_or_skip() else { return };

    fx.split_agent_window_and_await_hook();

    let board = read_now(&fx.board_log);
    assert!(
        board.trim().is_empty(),
        "the board TUI must never receive hook keystrokes — it interprets them \
         as keybindings (`c` opens Copy Task). got: {board:?}"
    );
}

/// The other half of correct routing: the agent's own pane runs Claude, so a
/// stray `cd` there would be typed into the agent's prompt.
#[test]
fn split_hook_never_types_into_the_agents_own_pane() {
    let Some(fx) = setup_or_skip() else { return };

    fx.split_agent_window_and_await_hook();

    let agent = read_now(&fx.agent_log);
    assert!(
        agent.trim().is_empty(),
        "the agent's own pane must never receive hook keystrokes — they would \
         be typed into the running Claude session. got: {agent:?}"
    );
}

/// Same routing requirement with the agent window *focused* — the case a user
/// hits by splitting a pane while working in an agent window. Worth asserting
/// separately: with the pre-fix hook the keystrokes followed the active pane, so
/// this scenario typed `cd <worktree>` straight into the running Claude session,
/// while `split_hook_never_types_into_the_agents_own_pane` passed only because
/// the board happened to be active and absorbed them instead.
#[test]
fn split_hook_targets_the_new_pane_even_when_the_agent_window_is_focused() {
    let Some(fx) = setup_or_skip() else { return };
    tmux::select_window(AGENT_WINDOW, &fx.server.runner()).unwrap();

    let got = fx.split_agent_window_and_await_hook();

    assert!(
        got.contains(&format!("cd {}", fx.worktree.display())),
        "new pane should be sent `cd <worktree>` regardless of focus, got: {got:?}"
    );
    let agent = read_now(&fx.agent_log);
    assert!(
        agent.trim().is_empty(),
        "focusing the agent window must not redirect keystrokes into its own \
         Claude pane. got: {agent:?}"
    );
}

/// A window without `@dispatch_dir` (the board, the main session) must not
/// trigger the hook at all — the `if-shell -F` guard covers this, and it is the
/// property that keeps non-agent windows unaffected.
#[test]
fn split_hook_is_inert_for_windows_without_a_dispatch_dir() {
    let Some(fx) = setup_or_skip() else { return };

    // Split the *board* window, which carries no @dispatch_dir.
    let plain_log = fx.dir.path().join("plain.log");
    fx.split_window_capturing(BOARD_WINDOW, &plain_log);

    // Anchor on a split that *does* fire the hook, so this is not merely a race
    // that happens to observe "nothing yet".
    fx.split_agent_window_and_await_hook();

    let plain = read_now(&plain_log);
    assert!(
        plain.trim().is_empty(),
        "a split in a window without @dispatch_dir must receive nothing, got: {plain:?}"
    );
}
