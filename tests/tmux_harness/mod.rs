#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]
//! Shared rig for the real-tmux integration tests
//! (`tests/tmux_split_hook.rs`, `tests/tmux_lifecycle.rs`).
//!
//! # Why a real tmux server
//!
//! `MockProcessRunner` records argv, so a mock test can assert *"we handed tmux
//! this string"* but never *"tmux did what we meant"*. Task #3781 was a pure
//! tmux-targeting defect and its mock test pinned the broken command string
//! while staying green. Anything about **which pane** receives what, **which
//! cwd** a pane resolves, or **how many panes** a window ends up with needs a
//! real server.
//!
//! # Isolation
//!
//! Every test gets its own server on a unique `-L` socket, killed by a drop
//! guard so a panic cannot leak one, and the developer's own tmux is never
//! touched. Per-test servers also keep the tests parallel: they cannot share a
//! session, because they mutate session-global state (the active window, the
//! session-level hook, `pane-base-index`).
//!
//! Every call carries `-f /dev/null` so the developer's `~/.tmux.conf` cannot
//! change what these tests observe. Without it a local `pane-base-index`,
//! `default-command` or custom hook would make behaviour machine-dependent, and
//! CI (which has no config) would be exercising a different tmux than the
//! developer does. See [`SocketRunner::run`] for why it is on every call rather
//! than a single `start-server`.
//!
//! Each server also owns a pair of stub `claude` / `dispatch` binaries, named to
//! production through `ProcessRunner::agent_binaries` rather than shadowed on
//! `PATH` — see the "Stub binaries" section below.
//!
//! # Two observation styles
//!
//! * **Keystroke capture** — panes run `cat > log`, so anything typed into them
//!   lands in a file. Only possible for panes the test creates itself. This is
//!   how pane *routing* is observed (`tests/tmux_split_hook.rs`).
//! * **Execution** — panes run the real shell and launch stub `claude` /
//!   `dispatch` binaries that report their own cwd, pane and argv. This is how
//!   windows *production* creates are observed, since `tmux::new_window` starts
//!   the default shell and takes no command (`tests/tmux_lifecycle.rs`).

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use dispatch_tui::process::{AgentBinaries, ProcessRunner, RealProcessRunner};

/// Deadline for condition polling on asynchronous tmux work (the split hook
/// fires `run-shell -b`; `resync_agent_tree_pane` re-splits in the background),
/// so a result is not visible the instant the triggering call returns.
///
/// This is a *deadline*, never a fixed sleep — see
/// `scripts/check-no-test-sleep.sh` and the "No `tokio::time::sleep` in tests"
/// section of docs/conventions.md. Only the failure path ever pays it in full.
pub const DELIVERY_DEADLINE: Duration = Duration::from_secs(5);
pub const POLL_STEP: Duration = Duration::from_millis(25);

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

/// Owns a private tmux server, plus the stub `claude` / `dispatch` binaries its
/// panes run, and kills the server on drop — including on panic.
pub struct TmuxServer {
    socket: String,
    /// Declared *after* `socket`: `Drop::drop` (which runs `kill-server`, ending
    /// the stub processes that hold the log open) completes before this field's
    /// own destructor unlinks the directory.
    stubs: tempfile::TempDir,
}

impl TmuxServer {
    pub fn start() -> Self {
        // Unique per process AND per thread, so these tests run concurrently
        // (cargo runs them on separate threads by default) without sharing a
        // session. Because the name embeds the pid, it also cannot collide with
        // a server left behind by an earlier `cargo test` run.
        let socket = format!(
            "dispatch-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        );
        // Stubs are per-server rather than per-process so each test gets a
        // private invocation log without deriving one at runtime, and so nothing
        // outlives the test that created it.
        let stubs = tempfile::Builder::new()
            .prefix("dispatch-tmux-stubs-")
            .tempdir()
            .expect("create stub bin dir");
        let server = Self { socket, stubs };
        for name in ["claude", "dispatch"] {
            write_stub(server.stubs.path(), name, &server.stub_log());
        }
        server
    }

    /// The binaries this server's panes will run instead of the real ones — the
    /// value [`SocketRunner`] hands to production via
    /// `ProcessRunner::agent_binaries`.
    fn agent_binaries(&self) -> AgentBinaries {
        let bin = |name: &str| self.stubs.path().join(name).to_string_lossy().into_owned();
        AgentBinaries {
            claude: bin("claude"),
            dispatch: bin("dispatch"),
        }
    }

    /// Where the stubs record their invocations. One tab-separated record per
    /// invocation; see [`StubLine`].
    pub fn stub_log(&self) -> PathBuf {
        self.stubs.path().join("invocations.log")
    }

    /// Run a tmux command against this server, returning its raw output.
    /// Failures are not asserted here — callers that need that use [`Self::tmux_ok`].
    pub fn tmux(&self, args: &[&str]) -> std::process::Output {
        // Routed through the same runner the production calls use, so the
        // `-L <socket>` scoping rule lives in exactly one place.
        self.runner()
            .run("tmux", args)
            .expect("failed to invoke tmux")
    }

    /// Run a tmux command and assert it succeeded.
    pub fn tmux_ok(&self, args: &[&str]) {
        let out = self.tmux(args);
        assert!(
            out.status.success(),
            "tmux {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Run a tmux command and return its trimmed stdout.
    pub fn tmux_stdout(&self, args: &[&str]) -> String {
        let out = self.tmux(args);
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A `ProcessRunner` that routes production tmux calls to this test server,
    /// by prepending `-L <socket>` to every argument list. This keeps the tests
    /// exercising the real `tmux::*` functions rather than hand-copied command
    /// strings, so they cannot drift from what actually ships.
    pub fn runner(&self) -> SocketRunner {
        SocketRunner {
            socket: self.socket.clone(),
            binaries: self.agent_binaries(),
            inner: RealProcessRunner,
        }
    }

    // -- introspection -----------------------------------------------------

    /// Pane ids of `window`, in tmux's own listing order (left to right).
    pub fn pane_ids(&self, window: &str) -> Vec<String> {
        self.list_panes(window, "#{pane_id}")
    }

    pub fn pane_count(&self, window: &str) -> usize {
        self.pane_ids(window).len()
    }

    /// `(pane_id, pane_left)` for each pane, for asserting which side of a split
    /// a pane landed on.
    pub fn pane_lefts(&self, window: &str) -> Vec<(String, u32)> {
        self.list_panes(window, "#{pane_id} #{pane_left}")
            .into_iter()
            .filter_map(|line| {
                let (id, left) = line.split_once(' ')?;
                Some((id.to_string(), left.trim().parse().ok()?))
            })
            .collect()
    }

    /// The pane id of `window`'s leftmost pane.
    pub fn leftmost_pane_id(&self, window: &str) -> Option<String> {
        self.pane_lefts(window)
            .into_iter()
            .min_by_key(|(_, left)| *left)
            .map(|(id, _)| id)
    }

    pub fn active_pane_id(&self, window: &str) -> Option<String> {
        self.list_panes(window, "#{pane_active} #{pane_id}")
            .into_iter()
            .find_map(|line| line.strip_prefix("1 ").map(str::to_string))
    }

    fn list_panes(&self, window: &str, format: &str) -> Vec<String> {
        let out = self.tmux(&["list-panes", "-t", window, "-F", format]);
        if !out.status.success() {
            return Vec::new();
        }
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Every window name on the server. Independent of
    /// `tmux::list_all_window_names` for the same reason as [`Self::pane_exists`]:
    /// these are the oracles for windows production creates and kills.
    pub fn window_names(&self) -> Vec<String> {
        self.tmux_stdout(&["list-windows", "-a", "-F", "#{window_name}"])
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Exact-name membership — deliberately not `-t <name>`, which tmux
    /// prefix-matches (`-t task-4` resolves to `task-42`).
    pub fn has_window(&self, window: &str) -> bool {
        self.window_names().iter().any(|n| n == window)
    }

    /// Whether `pane_id` still exists anywhere on the server.
    ///
    /// Deliberately its own implementation rather than a call to
    /// `tmux::pane_exists`, even though the two now agree: this is the *oracle*
    /// for assertions about panes production was supposed to kill, and an oracle
    /// that calls the function under test cannot catch a regression in it. This
    /// diff is the case in point — production's version was blind (it used
    /// `display-message`, which succeeds for a pane that never existed) and an
    /// independent oracle is what exposed it.
    pub fn pane_exists(&self, pane_id: &str) -> bool {
        self.tmux_stdout(&["list-panes", "-a", "-F", "#{pane_id}"])
            .lines()
            .any(|id| id.trim() == pane_id)
    }

    /// The working directory a pane's process actually resolved — the property
    /// that proves an agent window opened inside its worktree rather than in the
    /// dispatch process's cwd.
    pub fn pane_cwd(&self, pane_id: &str) -> String {
        self.tmux_stdout(&[
            "display-message",
            "-p",
            "-t",
            pane_id,
            "#{pane_current_path}",
        ])
    }

    /// A window-scoped user option (e.g. `@dispatch_dir`). Empty when unset.
    pub fn window_option(&self, window: &str, option: &str) -> String {
        self.tmux_stdout(&["show-options", "-wqv", "-t", window, option])
    }

    /// A pane-scoped user option (e.g. `@dispatch_editor_pane`). Empty when
    /// unset. The oracle for a pane dispatch marked as one it created.
    pub fn pane_option(&self, pane_id: &str, option: &str) -> String {
        self.tmux_stdout(&["show-options", "-pqv", "-t", pane_id, option])
    }

    /// The command a pane was started with — empty for a pane running the default
    /// shell. Lets a test locate the agent-tree companion pane *without* calling
    /// production's own lookup, which would make the oracle circular.
    pub fn pane_start_command(&self, pane_id: &str) -> String {
        self.tmux_stdout(&[
            "display-message",
            "-p",
            "-t",
            pane_id,
            "#{pane_start_command}",
        ])
    }

    /// Write an additional stub binary into this server's stub dir and return its
    /// absolute path. Same shape as the `claude` / `dispatch` stubs: it records
    /// one line to [`Self::stub_log`] and then holds its pane open with
    /// `exec cat`.
    ///
    /// Exists because the editor pane's command is not a dispatch binary — it
    /// comes from `$EDITOR` — so it cannot be one of the two stubs written at
    /// server start. Holding the pane open is the load-bearing part: a stub that
    /// exited would close its pane and make every pane-count assertion racy.
    pub fn extra_stub(&self, name: &str) -> String {
        write_stub(self.stubs.path(), name, &self.stub_log());
        self.stubs.path().join(name).to_string_lossy().into_owned()
    }
}

impl Drop for TmuxServer {
    fn drop(&mut self) {
        self.tmux(&["kill-server"]);
    }
}

/// Routes every production `tmux` call to a private test server, and names that
/// server's stub binaries as the ones the agent launchers should invoke. Non-tmux
/// programs (`git`) pass through untouched.
pub struct SocketRunner {
    socket: String,
    binaries: AgentBinaries,
    inner: RealProcessRunner,
}

impl ProcessRunner for SocketRunner {
    /// The whole substitution seam — see the "Stub binaries" section below.
    fn agent_binaries(&self) -> AgentBinaries {
        self.binaries.clone()
    }

    fn run(&self, program: &str, args: &[&str]) -> anyhow::Result<std::process::Output> {
        if program != "tmux" {
            return self.inner.run(program, args);
        }
        // `-f /dev/null` goes on *every* invocation, not on a one-off
        // `start-server`. tmux reads its config when the server starts, and the
        // server is started implicitly by whichever command happens to be first
        // — so the only way to be sure `-f` is in effect is to pass it always.
        // Verified: `-f` is silently ignored by an explicit `start-server`
        // (the user's `~/.tmux.conf` still loads), honoured on the implicit-start
        // `new-session`, and harmless on every later command.
        let mut full: Vec<&str> = vec!["-L", &self.socket, "-f", "/dev/null"];
        full.extend_from_slice(args);
        self.inner.run(program, &full)
    }
}

// ---------------------------------------------------------------------------
// Availability guard
// ---------------------------------------------------------------------------

/// Whether tmux is usable in this environment.
fn tmux_available() -> bool {
    Command::new("tmux")
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// `true` when the caller may proceed; `false` means "skip this test".
///
/// Under CI a missing tmux is a hard failure instead of a skip. Skipping there
/// would be a silent pass — `eprintln!` is swallowed by the default test
/// harness — which would let a dropped `Install tmux` step or a changed runner
/// image quietly turn these files into a no-op, taking the only real coverage of
/// tmux semantics with it. This is what makes the CI install step an invariant
/// rather than a convention.
pub fn tmux_available_or_skip() -> bool {
    if tmux_available() {
        return true;
    }
    assert!(
        std::env::var_os("CI").is_none(),
        "tmux is required in CI but was not found on PATH — the workflow's \
         `Install tmux` step must run before `cargo test` (see \
         .github/workflows/ci.yml). Refusing to skip and report green."
    );
    eprintln!("skipping: tmux not available on PATH");
    false
}

// ---------------------------------------------------------------------------
// Keystroke capture
// ---------------------------------------------------------------------------

/// Spawn a pane command that records everything typed into the pane to `path`.
/// `cat` reads the pane's tty, so any `send-keys` payload lands in the file once
/// a newline (the hook's `Enter`) flushes the line.
pub fn capture_cmd(path: &Path) -> String {
    format!("cat > {}", path.display())
}

/// Line [`capture_cmd_marked`] writes once per process start.
pub const START_MARKER: &str = "__pane_started__";

/// [`capture_cmd`] plus a start marker, so a test can tell whether a pane's
/// process was *restarted* underneath it.
///
/// `respawn-pane` re-runs a pane's original command, which is invisible to a
/// plain capture: the new `cat` simply reopens the same log. Counting
/// [`START_MARKER`] distinguishes the two — one occurrence means the pane has run
/// once, two means something respawned it. Appends (`>>`) rather than truncates
/// for the same reason.
pub fn capture_cmd_marked(path: &Path) -> String {
    format!(
        "echo {START_MARKER} >> {path}; exec cat >> {path}",
        path = path.display()
    )
}

/// How many times the pane writing to `path` has started — see
/// [`capture_cmd_marked`].
pub fn start_count(path: &Path) -> usize {
    read_now(path)
        .lines()
        .filter(|line| line.trim() == START_MARKER)
        .count()
}

/// Everything a pane received that was not written by [`capture_cmd_marked`]
/// itself — i.e. keystrokes someone synthesised into it.
pub fn typed_input(path: &Path) -> String {
    read_now(path)
        .lines()
        .filter(|line| line.trim() != START_MARKER)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Resolve a path the way tmux reports `#{pane_current_path}`: through the OS,
/// so `/tmp` may come back as `/private/tmp` and a symlinked temp dir differs
/// from the string the test built.
pub fn canonical(p: &str) -> String {
    std::fs::canonicalize(p)
        .map(|c| c.to_string_lossy().into_owned())
        .unwrap_or_else(|_| p.to_string())
}

/// Poll until `path` is non-empty or the deadline expires, returning its
/// contents (empty on timeout).
pub fn read_when_written(path: &Path) -> String {
    poll_for(|| {
        let contents = read_now(path);
        (!contents.trim().is_empty()).then_some(contents)
    })
    .unwrap_or_default()
}

/// Snapshot a file without waiting. Only meaningful once the operation under
/// test has been observed to complete — otherwise it races.
pub fn read_now(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// Poll `f` until it yields a value or [`DELIVERY_DEADLINE`] expires.
///
/// Bounded `std::thread::sleep`, which is the right tool for waiting on
/// asynchronous tmux delivery. `scripts/check-no-test-sleep.sh` bans
/// `std::thread::sleep` in test code too, so the call below carries the
/// script's `allow-test-sleep:` marker — a deadline-bounded poll step is the
/// one shape that check exempts. `f` is evaluated before the first sleep, so an
/// already-satisfied condition costs nothing, and only the failure path pays
/// the deadline in full.
pub fn poll_for<T>(mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let start = Instant::now();
    loop {
        if let Some(value) = f() {
            return Some(value);
        }
        if start.elapsed() >= DELIVERY_DEADLINE {
            return None;
        }
        std::thread::sleep(POLL_STEP); // allow-test-sleep: deadline-bounded poll step
    }
}

/// [`poll_for`] for conditions with no value to carry out.
pub fn poll_until(mut pred: impl FnMut() -> bool) -> bool {
    poll_for(|| pred().then_some(())).is_some()
}

// ---------------------------------------------------------------------------
// Stub binaries
//
// Production takes the binaries it launches from the runner
// (`ProcessRunner::agent_binaries`, src/process.rs), defaulting to the bare names
// `claude` / `dispatch`. [`SocketRunner`] overrides that method with absolute
// paths to this server's stubs, so a test cannot spawn a live agent (which hits
// the network and can hang on its trust prompt) or a real `dispatch` (which opens
// the developer's database).
//
// Do not go back to shadowing them on `PATH` (#3799). That needed four
// cooperating mechanisms and was still unsound: an `env::set_var("PATH", …)` that
// races libtest's parallel `Command::spawn`s, a pinned no-rc `default-command`
// because a pane's login shell re-resolves `PATH`, a `DISPATCH_DB` override to
// bound the damage, and two guards to detect failure — one of which existed
// because it had already failed. An absolute path is immune to `PATH` order.
//
// Still isolating *tmux* itself, unrelated to the binaries: `-f /dev/null` on
// every call and a private `-L` socket per test.
//
// One residual, deliberately left: panes run the developer's login shell, so an
// rc file that `cd`s would move a pane's cwd out from under the `pane_cwd` /
// `StubLine::cwd` assertions. Nothing about *which* binary runs depends on the
// shell any more. If it ever bites, `set-option -g default-command 'bash --norc
// --noprofile'` comes back as pane-determinism config — the same category as
// `-f /dev/null` — not as stub protection.
// ---------------------------------------------------------------------------

/// Write one stub. It records a single line and then holds its pane open with
/// `exec cat`, because a stub that exited would close its pane and make every
/// pane-count assertion racy. Holding the pane with `cat` rather than `sleep`
/// needs no timer, and any stray keystrokes that later reach the pane append to
/// the same log where an assertion can see them.
///
/// `log` is baked in at write time: each server owns its stubs, so the stub does
/// not have to derive a per-test log path at runtime.
fn write_stub(dir: &Path, name: &str, log: &Path) {
    let path = dir.join(name);
    // Tab-separated, because a tab cannot appear in a `%N` pane id, a tempdir
    // path, or the argv these tests produce — so [`StubLine::parse`] needs no
    // escaping and no field-order coupling.
    let script = format!(
        r#"#!/bin/sh
printf '%s\t%s\t%s\t%s\n' "{name}" "${{TMUX_PANE:-none}}" "$PWD" "$*" >> "{log}"
exec cat >> "{log}"
"#,
        log = log.display(),
        name = name,
    );
    let mut f = std::fs::File::create(&path).expect("create stub");
    f.write_all(script.as_bytes()).expect("write stub");
    f.set_permissions(std::fs::Permissions::from_mode(0o755))
        .expect("chmod stub");
}

/// One recorded stub invocation: which binary ran, in which pane, from which
/// directory, with which arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StubLine {
    /// `claude` or `dispatch`.
    pub name: String,
    /// `$TMUX_PANE` as seen by the stub, e.g. `%3`.
    pub pane: String,
    /// `$PWD` as seen by the stub — the cwd tmux actually resolved for the pane.
    pub cwd: String,
    /// The argv the stub was called with, space-joined.
    pub args: String,
}

impl StubLine {
    fn parse(line: &str) -> Option<Self> {
        let mut parts = line.split('\t');
        Some(Self {
            name: parts.next()?.to_string(),
            pane: parts.next()?.to_string(),
            cwd: parts.next()?.to_string(),
            args: parts.next().unwrap_or_default().to_string(),
        })
    }
}

/// All stub invocations recorded for `server`.
pub fn stub_lines(server: &TmuxServer) -> Vec<StubLine> {
    read_now(&server.stub_log())
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(StubLine::parse)
        .collect()
}

/// Wait for a recorded invocation matching `pred`. `None` on timeout.
///
/// Stub delivery is asynchronous — the pane's shell has to start, resolve the
/// stub and run it — so every positive assertion about stub output must poll
/// rather than snapshot.
pub fn await_stub_line(
    server: &TmuxServer,
    mut pred: impl FnMut(&StubLine) -> bool,
) -> Option<StubLine> {
    poll_for(|| stub_lines(server).into_iter().find(|l| pred(l)))
}
