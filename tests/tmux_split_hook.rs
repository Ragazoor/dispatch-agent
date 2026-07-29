#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Real-tmux integration test for the `after-split-window` hook that keeps new
//! panes in their agent's worktree (`ensure_split_hook` / `set_window_dispatch_dir`).
//!
//! This test exists because the mock layer structurally cannot catch the bug it
//! guards against. `MockProcessRunner` records argv, so a mock test can assert
//! *"we handed tmux this string"* but never *"tmux did what we meant"*. The
//! original defect was purely a matter of tmux semantics: the hook's `send-keys`
//! had no `-t` target, and inside `run-shell -bC` the enclosing target context is
//! lost, so tmux fell back to the session's **active** pane. Because dispatch
//! opens the agent-tree companion pane by splitting the agent window in the
//! background while the board is still focused, the board TUI received
//! `cd <worktree>` as genuine keystrokes — the `c` fired the Copy-Task binding
//! and the rest was typed into the resulting field. A mock test asserted the
//! broken string verbatim and stayed green throughout.
//!
//! So the assertions below are about *pane routing*, observed through a real tmux
//! server: the `cd` line must arrive in the newly created pane, and must not
//! arrive in the active (board stand-in) pane or the agent's own pane.
//!
//! Isolation: every run uses its own tmux server on a unique `-L` socket and
//! kills it via a drop guard, so a failing assertion or panic cannot leak a
//! server and the developer's own tmux session is never touched.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use dispatch_tui::process::RealProcessRunner;
use dispatch_tui::tmux;

/// How long to wait for the hook's keystrokes to land. The hook fires
/// `run-shell -b`, which is asynchronous, so the write is not visible the
/// instant `split-window` returns. This is a *deadline* for condition polling,
/// never a fixed sleep — see `scripts/check-no-test-sleep.sh` and the
/// "No `tokio::time::sleep` in tests" section of docs/conventions.md.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(5);
const POLL_STEP: Duration = Duration::from_millis(25);

/// Owns a private tmux server and kills it on drop, including on panic.
struct TmuxServer {
    socket: String,
}

impl TmuxServer {
    fn start() -> Self {
        // Unique per process AND per test, so tests in this file can run
        // concurrently (cargo runs them on separate threads by default).
        let socket = format!(
            "dispatch-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        );
        let server = Self { socket };
        // Kill any leftover from a previously crashed run before starting.
        server.tmux(&["kill-server"]);
        server
    }

    /// Run a tmux command against this server, ignoring failures. Used for
    /// setup/teardown calls whose failure is either expected (`kill-server` with
    /// nothing running) or will surface as a later assertion failure anyway.
    fn tmux(&self, args: &[&str]) -> std::process::Output {
        let mut full = vec!["-L", &self.socket];
        full.extend_from_slice(args);
        Command::new("tmux")
            .args(&full)
            .output()
            .expect("failed to invoke tmux")
    }

    /// Run a tmux command and assert it succeeded.
    fn tmux_ok(&self, args: &[&str]) -> String {
        let out = self.tmux(args);
        assert!(
            out.status.success(),
            "tmux {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A `ProcessRunner` that routes production tmux calls to this test server,
    /// by prepending `-L <socket>` to every argument list. This keeps the test
    /// exercising the real `tmux::*` functions rather than a hand-copied command
    /// string, so the test cannot drift from what actually ships.
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

impl dispatch_tui::process::ProcessRunner for SocketRunner {
    fn run(&self, program: &str, args: &[&str]) -> anyhow::Result<std::process::Output> {
        if program != "tmux" {
            return self.inner.run(program, args);
        }
        let mut full: Vec<&str> = vec!["-L", &self.socket];
        full.extend_from_slice(args);
        self.inner.run(program, &full)
    }
}

/// Whether tmux is usable in this environment. Absence is reported loudly rather
/// than silently passing, so a CI image that loses tmux does not quietly turn
/// this test into a no-op.
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

/// Poll until `path` is non-empty or the deadline expires. Returns its contents
/// (empty string on timeout, which is a legitimate expected outcome for the
/// panes that must NOT receive keystrokes).
fn read_when_written(path: &Path) -> String {
    let start = Instant::now();
    loop {
        if let Ok(s) = std::fs::read_to_string(path) {
            if !s.trim().is_empty() {
                return s;
            }
        }
        if start.elapsed() >= DELIVERY_DEADLINE {
            return std::fs::read_to_string(path).unwrap_or_default();
        }
        std::thread::sleep(POLL_STEP);
    }
}

/// Wait for a pane that must stay untouched, then confirm it is still empty.
/// Uses the same deadline as the positive case so a late delivery cannot slip
/// through by arriving just after a shorter check.
fn read_expecting_silence(path: &Path, settled: &Path) -> String {
    // Anchor on the pane that *should* receive the keys: once that has landed,
    // the hook has run, so anything destined elsewhere would have been sent too.
    let _ = read_when_written(settled);
    std::fs::read_to_string(path).unwrap_or_default()
}

struct Fixture {
    server: TmuxServer,
    _dir: tempfile::TempDir,
    board_log: PathBuf,
    agent_log: PathBuf,
    new_pane_log: PathBuf,
    worktree: PathBuf,
}

/// Build the topology dispatch actually produces: a focused board window plus a
/// background agent window carrying `@dispatch_dir`, with the production hook
/// installed. Mirrors `resume_agent` / `dispatch_with_prompt` in
/// src/dispatch/agents.rs, which call `set_window_dispatch_dir` then
/// `ensure_split_hook` then split the agent window for the companion pane.
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
        "board",
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
        "task-42",
        "--",
        "sh",
        "-c",
        &capture_cmd(&agent_log),
    ]);

    let runner = server.runner();
    tmux::set_window_dispatch_dir("task-42", worktree.to_str().unwrap(), &runner).unwrap();
    tmux::ensure_split_hook(&runner).unwrap();

    Fixture {
        server,
        _dir: dir,
        board_log,
        agent_log,
        new_pane_log,
        worktree,
    }
}

impl Fixture {
    /// Split the agent window the way `spawn_agent_tree_pane` does (`-d`, so
    /// focus stays on the board), with the new pane capturing its input.
    fn split_agent_window(&self) {
        self.server.tmux_ok(&[
            "split-window",
            "-h",
            "-d",
            "-l",
            "30%",
            "-t",
            "task-42",
            "--",
            "sh",
            "-c",
            &capture_cmd(&self.new_pane_log),
        ]);
    }
}

#[test]
fn split_hook_cds_the_new_pane_into_the_worktree() {
    if !tmux_available() {
        eprintln!("skipping: tmux not available on PATH");
        return;
    }
    let fx = setup();
    fx.split_agent_window();

    let got = read_when_written(&fx.new_pane_log);
    assert!(
        got.contains(&format!("cd {}", fx.worktree.display())),
        "new pane should be sent `cd <worktree>`, got: {got:?}"
    );
}

/// The regression test for task #3781. Before the fix this failed with the
/// board's log containing the `cd` line.
#[test]
fn split_hook_never_types_into_the_board_window() {
    if !tmux_available() {
        eprintln!("skipping: tmux not available on PATH");
        return;
    }
    let fx = setup();
    fx.split_agent_window();

    let board = read_expecting_silence(&fx.board_log, &fx.new_pane_log);
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
    if !tmux_available() {
        eprintln!("skipping: tmux not available on PATH");
        return;
    }
    let fx = setup();
    fx.split_agent_window();

    let agent = read_expecting_silence(&fx.agent_log, &fx.new_pane_log);
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
    if !tmux_available() {
        eprintln!("skipping: tmux not available on PATH");
        return;
    }
    let fx = setup();
    fx.server.tmux_ok(&["select-window", "-t", "task-42"]);
    fx.split_agent_window();

    let got = read_when_written(&fx.new_pane_log);
    assert!(
        got.contains(&format!("cd {}", fx.worktree.display())),
        "new pane should be sent `cd <worktree>` regardless of focus, got: {got:?}"
    );
    let agent = std::fs::read_to_string(&fx.agent_log).unwrap_or_default();
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
    if !tmux_available() {
        eprintln!("skipping: tmux not available on PATH");
        return;
    }
    let fx = setup();

    // Split the *board* window, which carries no @dispatch_dir.
    let plain_log = fx._dir.path().join("plain.log");
    fx.server.tmux_ok(&[
        "split-window",
        "-h",
        "-d",
        "-t",
        "board",
        "--",
        "sh",
        "-c",
        &capture_cmd(&plain_log),
    ]);

    // Anchor the wait on a split that *does* fire the hook, so this is not just
    // a race that happens to observe "nothing yet".
    fx.split_agent_window();
    let _ = read_when_written(&fx.new_pane_log);

    let plain = std::fs::read_to_string(&plain_log).unwrap_or_default();
    assert!(
        plain.trim().is_empty(),
        "a split in a window without @dispatch_dir must receive nothing, got: {plain:?}"
    );
}
