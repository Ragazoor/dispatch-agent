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
//! # Two observation styles
//!
//! * **Keystroke capture** — panes run `cat > log`, so anything typed into them
//!   lands in a file. Only possible for panes the test creates itself. This is
//!   how pane *routing* is observed (`tests/tmux_split_hook.rs`).
//! * **Execution** — panes run the real shell and resolve stub `claude` /
//!   `dispatch` binaries that report their own cwd, pane and argv. This is how
//!   windows *production* creates are observed, since `tmux::new_window` starts
//!   the default shell and takes no command (`tests/tmux_lifecycle.rs`).

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use dispatch_tui::process::{ProcessRunner, RealProcessRunner};

/// Deadline for condition polling on asynchronous tmux work (the split hook
/// fires `run-shell -b`; `resync_agent_tree_pane` re-splits in the background),
/// so a result is not visible the instant the triggering call returns.
///
/// This is a *deadline*, never a fixed sleep — see
/// `scripts/check-no-test-sleep.sh` and the "No `tokio::time::sleep` in tests"
/// section of docs/conventions.md. Only the failure path ever pays it in full.
pub const DELIVERY_DEADLINE: Duration = Duration::from_secs(5);
pub const POLL_STEP: Duration = Duration::from_millis(25);

/// The shell new panes run under test. Explicitly no-rc — see
/// [`TmuxServer::isolate_pane_shell`] for why that is load-bearing.
pub const PANE_SHELL: &str = "bash --norc --noprofile";

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

/// Owns a private tmux server and kills it on drop, including on panic.
pub struct TmuxServer {
    socket: String,
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
        Self { socket }
    }

    pub fn socket(&self) -> &str {
        &self.socket
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
            inner: RealProcessRunner,
        }
    }

    /// Pin the shell new panes run to one that does not source the user's rc
    /// files, and apply it before any pane the test cares about is created.
    ///
    /// With `default-command` empty, tmux runs the pane's shell as a **login**
    /// shell. That sources `~/.bashrc` / `~/.profile`, which rebuild `PATH` and
    /// typically prepend directories ahead of whatever the pane inherited — so
    /// the stub `claude` / `dispatch` lose to the real ones and a test silently
    /// launches production binaries. Verified: without this, a `send-keys`
    /// dispatch runs the real `claude`.
    ///
    /// This is test-environment configuration in the same spirit as
    /// `-f /dev/null`. It does not change any behaviour under test: production
    /// never sets `default-command`, and nothing these tests assert depends on
    /// which shell occupies a pane — only on which pane, which cwd, and what ran.
    pub fn isolate_pane_shell(&self) {
        self.tmux_ok(&["set-option", "-g", "default-command", PANE_SHELL]);
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
}

impl Drop for TmuxServer {
    fn drop(&mut self) {
        self.tmux(&["kill-server"]);
    }
}

/// Routes every production `tmux` call to a private test server. Non-tmux
/// programs (`git`) pass through untouched.
pub struct SocketRunner {
    socket: String,
    inner: RealProcessRunner,
}

impl ProcessRunner for SocketRunner {
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
pub fn tmux_available() -> bool {
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
// Production hardcodes the binary names it launches — `claude` inside a `bash -c`
// string, and `dispatch agent-tree <id>` as argv (src/dispatch/agents.rs) — so
// there is no seam to inject a fake through. These tests substitute them by name
// instead, which takes three cooperating pieces:
//
// 1. `PATH` on THIS PROCESS. A tmux pane inherits the environment of the client
//    that created it — not the server's, and not the session environment. `tmux
//    set-environment` (global or session, before or after session creation) never
//    reaches a later-created pane; verified against tmux 3.5a. `SocketRunner`
//    spawns tmux from this process, so this process is that client.
//
// 2. A no-rc pane shell ([`TmuxServer::isolate_pane_shell`]). PATH suffices for
//    the `--` argv forms, which `execvp` directly. It does NOT suffice for
//    `send-keys`, because tmux runs a pane's shell as a *login* shell, which
//    sources the user's rc files and prepends directories ahead of the inherited
//    PATH. A stub whose name nothing else provides still wins; `claude` and
//    `dispatch`, which do exist there on a dev machine, lose.
//
// 3. Two guards ([`install_stubs`]), because piece 2 was learned the hard way:
//    before it, the stub `dispatch` won while the REAL `claude` launched and sat
//    on its trust prompt. A real `claude` spawns a live agent, hits the network
//    and can hang the test on stdin; a real `dispatch` opens a database. The
//    in-pane guard is the one that would have caught it. `DISPATCH_DB` is
//    overridden as well so that even a defeated guard cannot be destructive.
//
// The deeper fix is for production to accept the binary identities alongside the
// `ProcessRunner` it already threads everywhere, which would retire all three
// pieces. Tracked separately; see the plan doc.
// ---------------------------------------------------------------------------

/// Directory holding the stub `claude` / `dispatch` binaries, created once per
/// test process and prepended to `PATH`. See the section comment above for why
/// PATH is set here and why it is not sufficient on its own.
///
/// Done once behind a `OnceLock` rather than per test: every test wants identical
/// stubs, so there is no per-test env mutation to race on.
fn stub_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        // The stubs must outlive every test in the process, so the TempDir's
        // destructor must not run. `Box::leak` and a `static OnceLock<TempDir>`
        // are equivalent here — neither drops — and libtest offers no
        // process-exit hook, so the directory does survive the run. It is a few
        // KB under the system temp dir; the prefix makes the strays identifiable.
        let dir = Box::leak(Box::new(
            tempfile::Builder::new()
                .prefix("dispatch-tmux-stubs-")
                .tempdir()
                .expect("create stub bin dir"),
        ))
        .path()
        .to_path_buf();
        let logs = dir.join("logs");
        std::fs::create_dir_all(&logs).expect("create stub log dir");

        write_stub(&dir, "claude", &logs);
        write_stub(&dir, "dispatch", &logs);

        let prev = std::env::var("PATH").unwrap_or_default();
        std::env::set_var("PATH", format!("{}:{prev}", dir.display()));

        // Belt and braces: if stub injection is ever defeated again, a real
        // `dispatch` must not reach the developer's database. The argv these
        // tests trigger is built by production code (`dispatch agent-tree <id>`,
        // spawn_agent_tree_pane) so a test cannot inject a `--db` flag — but that
        // flag carries `env = "DISPATCH_DB"` (src/main.rs), and panes inherit
        // this process's environment, so pointing the env var at a throwaway file
        // redirects it anyway. Costs nothing and removes the only destructive
        // failure mode the guard is protecting against.
        std::env::set_var("DISPATCH_DB", dir.join("throwaway-tasks.db"));
        dir
    })
}

/// Write one stub. It records a single line and then holds its pane open with
/// `exec cat`, because a stub that exited would close its pane and make every
/// pane-count assertion racy. Holding the pane with `cat` rather than `sleep`
/// needs no timer, and any stray keystrokes that later reach the pane append to
/// the same log where an assertion can see them.
///
/// The log file is chosen *inside the pane* from `$TMUX`, whose first
/// comma-separated field is the server's socket path. Each test owns a server,
/// so this gives every test a private log without threading a per-test value
/// through a process-global `PATH`.
fn write_stub(dir: &Path, name: &str, logs: &Path) {
    let path = dir.join(name);
    // Tab-separated, because a tab cannot appear in a `%N` pane id, a tempdir
    // path, or the argv these tests produce — so [`StubLine::parse`] needs no
    // escaping and no field-order coupling.
    let script = format!(
        r#"#!/bin/sh
sock=$(printf '%s' "${{TMUX:-unknown}}" | cut -d, -f1)
log="{logs}/$(basename "$sock").log"
printf '%s\t%s\t%s\t%s\n' "{name}" "${{TMUX_PANE:-none}}" "$PWD" "$*" >> "$log"
exec cat >> "$log"
"#,
        logs = logs.display(),
        name = name,
    );
    let mut f = std::fs::File::create(&path).expect("create stub");
    f.write_all(script.as_bytes()).expect("write stub");
    f.set_permissions(std::fs::Permissions::from_mode(0o755))
        .expect("chmod stub");
}

/// Install the stubs and refuse to run unless they really shadow the real
/// binaries — checked in this process, and once per process inside a real pane.
/// See the section comment above for the threat this guards against.
///
/// Every test must call this before touching tmux.
pub fn install_stubs() -> &'static Path {
    let dir = stub_dir();
    for name in ["claude", "dispatch"] {
        let resolved = resolve_on_path(name)
            .unwrap_or_else(|| panic!("stub `{name}` is not resolvable on PATH at all"));
        assert!(
            resolved.starts_with(dir),
            "stub injection failed: `{name}` resolves to {} instead of the stub in {}. \
             Refusing to run — this would execute the real binary.",
            resolved.display(),
            dir.display()
        );
    }
    verify_pane_resolution_once(dir);
    dir
}

/// The guard that actually matters: proves inside a real pane that a
/// `send-keys`-launched command resolves to the stubs. The process-level check
/// passed while the real `claude` was running in a pane, because the pane's login
/// shell re-resolved `PATH` after sourcing the user's rc files.
///
/// Runs once per test process on its own throwaway server, applying exactly the
/// setup every fixture applies, so a pass generalises to every later server.
fn verify_pane_resolution_once(dir: &Path) {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        if !tmux_available() {
            return;
        }
        let server = TmuxServer::start();
        server.tmux_ok(&["new-session", "-d", "-s", "guard", "-n", "w"]);
        server.isolate_pane_shell();

        let out = dir.join("logs").join("guard.txt");
        // A second window, created *after* the isolation option, running the
        // pane shell — the same shape as an agent window.
        server.tmux_ok(&["new-window", "-d", "-n", "probe"]);
        for name in ["claude", "dispatch"] {
            server.tmux_ok(&[
                "send-keys",
                "-t",
                "probe",
                "-l",
                &format!("command -v {name} >> {}", out.display()),
            ]);
            server.tmux_ok(&["send-keys", "-t", "probe", "Enter"]);
        }

        let resolved = poll_until(|| read_now(&out).lines().count() >= 2);
        let got = read_now(&out);
        assert!(
            resolved,
            "could not resolve `claude`/`dispatch` inside a tmux pane at all \
             (got {got:?}) — the stub rig is not working; refusing to run"
        );
        for line in got.lines() {
            assert!(
                Path::new(line.trim()).starts_with(dir),
                "stub injection does not survive into a tmux pane: resolved {line:?} \
                 instead of a stub under {}. Refusing to run — this would execute \
                 the real binary. See stub_dir's `PATH alone is not enough` note.",
                dir.display()
            );
        }
    });
}

/// First executable named `name` on `PATH`.
fn resolve_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|c| c.is_file() && is_executable(c))
}

fn is_executable(p: &Path) -> bool {
    std::fs::metadata(p)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
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

/// The log file the stubs write to for `server`. One tab-separated record per
/// invocation; see [`StubLine`].
pub fn stub_log_path(server: &TmuxServer) -> PathBuf {
    let dir = install_stubs();
    dir.join("logs").join(format!("{}.log", server.socket()))
}

/// All stub invocations recorded for `server`.
pub fn stub_lines(server: &TmuxServer) -> Vec<StubLine> {
    read_now(&stub_log_path(server))
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
