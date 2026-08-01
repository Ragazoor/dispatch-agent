use anyhow::{Context, Result};
use std::collections::VecDeque;
use std::process::Output;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Canonical timeout for long-running subprocesses (git fetch, worktree add).
/// Matches `DISPATCH_WATCHDOG_TIMEOUT` in `src/tui/mod.rs` — both kept in sync at 60s.
pub(crate) const SUBPROCESS_TIMEOUT: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Extract stderr from a process `Output` as a trimmed `String`.
pub(crate) fn stderr_str(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_string()
}

/// Extract stdout from a process `Output` as a trimmed `String`.
pub(crate) fn stdout_str(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// The `claude` and `dispatch` binaries the agent launchers in
/// `src/dispatch/agents.rs` invoke.
///
/// Production uses the bare names, resolved on `PATH` at launch time; a test can
/// name a stub instead. See [`ProcessRunner::agent_binaries`] for why this rides
/// with the runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentBinaries {
    /// The Claude Code CLI. Interpolated into shell command strings, so read it
    /// through [`Self::claude_quoted`] rather than directly.
    pub claude: String,
    /// This crate's own binary, launched as `dispatch agent-tree <id>` in the
    /// companion pane. Passed as argv, so it needs no quoting.
    pub dispatch: String,
}

impl Default for AgentBinaries {
    fn default() -> Self {
        Self {
            claude: "claude".to_string(),
            dispatch: "dispatch".to_string(),
        }
    }
}

impl AgentBinaries {
    /// [`Self::claude`] as one shell word, ready to interpolate into a command
    /// string.
    ///
    /// Every launcher site is arranged so that exactly one quoting layer applies
    /// — including `dispatch_with_prompt`, which passes the binary as bash's
    /// `$0` *outside* its single-quoted script body rather than inside it. Nested
    /// quoting would need the value escaped twice, and a site that got only one
    /// of the two layers right would look escaped while splitting at the first
    /// space; keeping the layer count at one everywhere removes that class of
    /// mistake instead of encapsulating it.
    pub fn claude_quoted(&self) -> String {
        shell_quote(&self.claude)
    }

    /// The stub identities the test suites substitute. One definition so the
    /// sentinel paths cannot drift between the launcher tests that assert on
    /// them.
    #[cfg(test)]
    pub fn stub() -> Self {
        Self {
            claude: "/stub/bin/claude-stub".to_string(),
            dispatch: "/stub/bin/dispatch-stub".to_string(),
        }
    }
}

/// Quote `s` for use as one word in a shell command, leaving it untouched when it
/// needs no quoting.
///
/// The pass-through case is load-bearing, not an optimisation: the default binary
/// names are plain, so the only change to production's emitted command string is
/// the `$0` indirection itself — no quoting noise on top of it.
fn shell_quote(s: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c);
    if !s.is_empty() && s.chars().all(safe) {
        return s.to_string();
    }
    // POSIX single-quoting: everything inside is literal, and an embedded quote
    // is written by closing, escaping, and reopening.
    format!("'{}'", s.replace('\'', r"'\''"))
}

pub trait ProcessRunner: Send + Sync {
    fn run(&self, program: &str, args: &[&str]) -> Result<Output>;

    /// Run the process but kill it (and return an error) if it hasn't exited
    /// within `timeout`. Stdout and stderr are drained on a pair of background
    /// threads so the pipe buffer never fills up while we poll.
    ///
    /// The default implementation ignores `timeout` and delegates to `run()`.
    /// Override this on real runners that can actually spawn and kill children.
    fn run_with_timeout(&self, program: &str, args: &[&str], _timeout: Duration) -> Result<Output> {
        self.run(program, args)
    }

    /// Which `claude` / `dispatch` binaries the agent launchers should invoke.
    ///
    /// The default is the bare names, resolved on `PATH` at launch — what every
    /// production runner wants. Override it to name stubs instead; that is the
    /// seam `tests/tmux_harness/mod.rs` uses so its real-tmux tests cannot spawn
    /// a live agent or open the developer's database.
    ///
    /// Only `src/dispatch/agents.rs` reads this, which cuts against the
    /// trait-narrowing convention in docs/conventions.md — deliberately. That
    /// convention is scoped to the DB traits, where narrowing is what makes the
    /// mutation boundary compiler-enforced. Here the alternative is threading the
    /// value through six launcher signatures and ~50 call sites that all pass the
    /// default, to substitute it in exactly one. Don't "fix" this by narrowing.
    fn agent_binaries(&self) -> AgentBinaries {
        AgentBinaries::default()
    }
}

// ---------------------------------------------------------------------------
// Real implementation — wraps std::process::Command
// ---------------------------------------------------------------------------

pub struct RealProcessRunner;

impl ProcessRunner for RealProcessRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<Output> {
        std::process::Command::new(program)
            .args(args)
            .output()
            .with_context(|| format!("failed to run {program}"))
    }

    fn run_with_timeout(&self, program: &str, args: &[&str], timeout: Duration) -> Result<Output> {
        use std::io::Read;
        use std::sync::mpsc;

        let mut child = std::process::Command::new(program)
            .args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to spawn {program}"))?;

        // Drain stdout/stderr on background threads to prevent pipe-buffer
        // deadlock if the subprocess writes a large amount of output while
        // we are sleeping between try_wait polls.
        #[allow(clippy::expect_used)] // invariant: we set Stdio::piped() above
        let mut stdout_pipe = child.stdout.take().expect("stdout is piped");
        #[allow(clippy::expect_used)] // invariant: we set Stdio::piped() above
        let mut stderr_pipe = child.stderr.take().expect("stderr is piped");

        let (stdout_tx, stdout_rx) = mpsc::channel();
        let (stderr_tx, stderr_rx) = mpsc::channel();

        std::thread::spawn(move || {
            let mut buf = Vec::new();
            stdout_pipe.read_to_end(&mut buf).ok();
            let _ = stdout_tx.send(buf);
        });
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            stderr_pipe.read_to_end(&mut buf).ok();
            let _ = stderr_tx.send(buf);
        });

        let deadline = std::time::Instant::now() + timeout;
        let poll_interval = Duration::from_millis(50);

        let status = loop {
            if let Some(s) = child
                .try_wait()
                .with_context(|| format!("failed to poll {program}"))?
            {
                break s;
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!("{program} timed out after {timeout:?}");
            }
            std::thread::sleep(poll_interval);
        };

        let stdout = stdout_rx.recv().unwrap_or_default();
        let stderr = stderr_rx.recv().unwrap_or_default();
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    }
}

// ---------------------------------------------------------------------------
// Mock implementation — for tests only
// ---------------------------------------------------------------------------

/// How a [`MockProcessRunner`] answers `tmux::window_target`'s exact-name
/// window lookup.
///
/// # Why this needs a policy at all
///
/// Every `tmux::` helper that takes a window *name* resolves it to a pane ID
/// first, because tmux resolves a bare `-t <name>` by **prefix** and would
/// otherwise act on a different task's window (see `tmux::window_target`). That
/// makes the lookup a precondition of nearly every tmux call in the codebase —
/// so how the mock answers it is a decision worth naming rather than burying.
enum WindowLookup {
    /// Resolve whatever name is asked for, assigning pane IDs `%0`, `%1`, … in
    /// first-seen order. Answered out of band: not taken from the positional
    /// response queue, and not recorded in [`MockProcessRunner::recorded_calls`].
    ///
    /// The default, because for the overwhelming majority of tests the subject
    /// is the *operation* — that dispatch sends the right claude command, that
    /// cleanup kills a window — and resolution is infrastructure. Interleaving a
    /// listing response into every queue and re-numbering every `calls[N]`
    /// assertion around it would obscure those tests without testing anything
    /// new. The trade is deliberate: a mock cannot meaningfully verify target
    /// resolution anyway (it records argv, not tmux's interpretation of it), so
    /// the real coverage lives in tests/tmux_window_targets.rs against a real
    /// server, plus the `window_target` unit tests in src/tmux.rs.
    AnyName(Mutex<Vec<String>>),
    /// Resolve only these names, in this order; anything else fails as absent.
    /// Also answered out of band. Use when a test needs a *specific* topology —
    /// notably a prefix collision (`task-4` alongside `task-42`).
    OnlyNames(Vec<String>),
    /// Do not intercept: answer the lookup from the positional queue and record
    /// it like any other call. Use when the lookup itself is the subject, or
    /// when no call at all is expected (see [`MockProcessRunner::unused`]).
    Queued,
}

pub struct MockProcessRunner {
    calls: Mutex<Vec<(String, Vec<String>)>>,
    responses: Mutex<VecDeque<(Option<Duration>, Result<Output>)>>,
    window_lookup: WindowLookup,
    binaries: AgentBinaries,
}

impl MockProcessRunner {
    /// Construct a runner. tmux window-name lookups resolve permissively — see
    /// [`WindowLookup::AnyName`] for why that is the default, and
    /// [`Self::with_windows`] / [`Self::with_queued_window_lookup`] to change it.
    pub fn new(responses: Vec<Result<Output>>) -> Self {
        Self::new_with_delays(responses.into_iter().map(|r| (None, r)).collect())
    }

    /// Construct a runner whose responses are delivered after a per-response
    /// delay. Use for testing watchdog/timeout logic.
    pub fn new_with_delays(responses: Vec<(Option<Duration>, Result<Output>)>) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            responses: Mutex::new(VecDeque::from(responses)),
            window_lookup: WindowLookup::AnyName(Mutex::new(Vec::new())),
            binaries: AgentBinaries::default(),
        }
    }

    /// Restrict this fake tmux server to exactly these window names, so a name
    /// that is not listed fails to resolve. Each gets one active pane, `%0`,
    /// `%1`, … in the order given; [`Self::pane_id_of`] returns the ID a
    /// resolution will yield.
    ///
    /// Use where the topology matters — above all a prefix collision, e.g.
    /// `with_windows(&["task-42"])` and then an operation on `task-4`, which
    /// must fail rather than hit `task-42`. See [`WindowLookup::OnlyNames`].
    pub fn with_windows(mut self, names: &[&str]) -> Self {
        self.window_lookup =
            WindowLookup::OnlyNames(names.iter().map(|n| (*n).to_string()).collect());
        self
    }

    /// Answer window lookups from the positional response queue and record them,
    /// instead of resolving them out of band. See [`WindowLookup::Queued`].
    pub fn with_queued_window_lookup(mut self) -> Self {
        self.window_lookup = WindowLookup::Queued;
        self
    }

    /// The pane ID this fake server resolves `name` to, i.e. the `-t` target the
    /// helper under test will pass to tmux.
    ///
    /// # Panics
    ///
    /// Under [`Self::with_windows`], if `name` was not declared — asserting
    /// against a window this server does not have is a test bug. Under
    /// [`Self::with_queued_window_lookup`], always: there is no fake server to
    /// ask.
    #[allow(clippy::expect_used, clippy::unwrap_used)] // test helper
    pub fn pane_id_of(&self, name: &str) -> String {
        let index = match &self.window_lookup {
            WindowLookup::AnyName(seen) => Self::index_of_or_insert(seen, name),
            WindowLookup::OnlyNames(names) => names
                .iter()
                .position(|n| n == name)
                .expect("window was not declared via with_windows"),
            WindowLookup::Queued => {
                panic!("pane_id_of has no meaning with a queued window lookup")
            }
        };
        format!("%{index}")
    }

    /// Index of `name` in `seen`, appending it first if absent. Stable within a
    /// test, so repeated resolutions of one name agree and two names differ.
    #[allow(clippy::unwrap_used)] // test helper — panics on poisoned mutex
    fn index_of_or_insert(seen: &Mutex<Vec<String>>, name: &str) -> usize {
        let mut seen = seen.lock().unwrap();
        if let Some(i) = seen.iter().position(|n| n == name) {
            return i;
        }
        seen.push(name.to_string());
        seen.len() - 1
    }

    /// Answer `tmux::window_target`'s lookup out of band, unless this call is not
    /// that lookup or the policy is [`WindowLookup::Queued`].
    ///
    /// The reply is what a real tmux would print for that lookup's `-f` filter:
    /// one row per *matching* pane. Under [`WindowLookup::OnlyNames`] the match
    /// is computed here over the declared windows, so a declared prefix collision
    /// (`task-4` asked for, only `task-42` declared) yields no rows and the
    /// production resolver is the thing that turns that into an error.
    fn answer_window_lookup(&self, program: &str, args: &[&str]) -> Option<Output> {
        if program != "tmux" {
            return None;
        }
        let wanted = crate::tmux::window_name_in_lookup(args)?;
        match &self.window_lookup {
            WindowLookup::Queued => None,
            WindowLookup::AnyName(seen) => {
                let index = Self::index_of_or_insert(seen, wanted);
                Some(Self::listing(&[(index, wanted.to_string())]))
            }
            WindowLookup::OnlyNames(names) => {
                let rows: Vec<(usize, String)> = names
                    .iter()
                    .enumerate()
                    .filter(|(_, name)| name.as_str() == wanted)
                    .map(|(i, name)| (i, name.clone()))
                    .collect();
                Some(Self::listing(&rows))
            }
        }
    }

    /// A `list-panes` listing in `tmux::WINDOW_PANE_FORMAT`: one active pane per
    /// row, each carrying the pane ID its index implies.
    fn listing(rows: &[(usize, String)]) -> Output {
        let stdout: String = rows
            .iter()
            .map(|(i, name)| format!("1 %{i} {name}\n"))
            .collect();
        Output {
            status: exit_ok(),
            stdout: stdout.into_bytes(),
            stderr: vec![],
        }
    }

    /// Name distinctive `claude` / `dispatch` binaries, so a test can assert
    /// which binary an agent launcher actually invoked rather than only that it
    /// invoked *something* called `claude`.
    pub fn with_agent_binaries(mut self, binaries: AgentBinaries) -> Self {
        self.binaries = binaries;
        self
    }

    #[allow(clippy::unwrap_used)] // test helper — panics on poisoned mutex (programming error)
    pub fn recorded_calls(&self) -> Vec<(String, Vec<String>)> {
        self.calls.lock().unwrap().clone()
    }

    /// Record a call and pop the next queued (delay, response) pair.
    /// Panics if no response is queued — same contract as `run` / `run_with_timeout`.
    #[allow(clippy::unwrap_used)] // test helper — panics on poisoned mutex (programming error)
    fn record_and_pop(&self, program: &str, args: &[&str]) -> (Option<Duration>, Result<Output>) {
        // Deliberately before the recording: an out-of-band window lookup is
        // neither queued nor recorded, so `calls[N]` indices stay stable.
        if let Some(listing) = self.answer_window_lookup(program, args) {
            return (None, Ok(listing));
        }
        self.calls.lock().unwrap().push((
            program.to_string(),
            args.iter().map(|s| s.to_string()).collect(),
        ));
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| {
                panic!("MockProcessRunner: no response queued for {program} {args:?}")
            })
    }

    /// A runner with no queued responses, ready to inject as a dependency.
    ///
    /// Use this wherever a test must supply a `ProcessRunner` but expects no
    /// commands to be run — the mock panics on the first call, so an
    /// accidental shell-out fails the test loudly instead of hitting the host.
    /// Queued window lookups on purpose: a window lookup is a shell-out too, and
    /// this runner's whole job is to panic on any of them.
    pub fn unused() -> Arc<dyn ProcessRunner> {
        Arc::new(Self::new(vec![]).with_queued_window_lookup())
    }

    /// Successful Output with empty stdout/stderr.
    pub fn ok() -> Result<Output> {
        Ok(Output {
            status: exit_ok(),
            stdout: vec![],
            stderr: vec![],
        })
    }

    /// Successful Output with specific stdout bytes.
    pub fn ok_with_stdout(stdout: &[u8]) -> Result<Output> {
        Ok(Output {
            status: exit_ok(),
            stdout: stdout.to_vec(),
            stderr: vec![],
        })
    }

    /// Failed Output (non-zero exit) with specific stderr.
    pub fn fail(stderr: &str) -> Result<Output> {
        Ok(Output {
            status: exit_fail(),
            stdout: vec![],
            stderr: stderr.as_bytes().to_vec(),
        })
    }

    /// Failed Output with a specific exit code. `fail` hardcodes 1, which cannot
    /// express the codes `git ls-remote --exit-code` uses to distinguish "no
    /// matching ref" (2) from "could not reach the remote" (128).
    pub fn fail_with_code(code: i32, stderr: &str) -> Result<Output> {
        Ok(Output {
            status: exit_code(code),
            stdout: vec![],
            stderr: stderr.as_bytes().to_vec(),
        })
    }
}

impl ProcessRunner for MockProcessRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<Output> {
        let (delay, response) = self.record_and_pop(program, args);
        if let Some(d) = delay {
            std::thread::sleep(d);
        }
        response
    }

    fn run_with_timeout(&self, program: &str, args: &[&str], timeout: Duration) -> Result<Output> {
        let (delay, response) = self.record_and_pop(program, args);
        if let Some(d) = delay {
            if d >= timeout {
                anyhow::bail!("{program} timed out after {timeout:?}");
            }
            std::thread::sleep(d);
        }
        response
    }

    fn agent_binaries(&self) -> AgentBinaries {
        self.binaries.clone()
    }
}

// ---------------------------------------------------------------------------
// Helpers for constructing ExitStatus in tests (Unix only)
// ---------------------------------------------------------------------------

#[cfg(unix)]
pub fn exit_ok() -> std::process::ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    std::process::ExitStatus::from_raw(0)
}

#[cfg(unix)]
pub fn exit_fail() -> std::process::ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    // Raw status word: exit code 1 = 1 << 8 = 256
    std::process::ExitStatus::from_raw(1 << 8)
}

/// An `ExitStatus` carrying a specific exit code, for callers that classify on
/// the code rather than the message (e.g. `git ls-remote --exit-code`).
#[cfg(unix)]
pub fn exit_code(code: i32) -> std::process::ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    // Raw status word: the exit code lives in the high byte.
    std::process::ExitStatus::from_raw(code << 8)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    // --- AgentBinaries ---

    #[test]
    fn agent_binaries_defaults_to_bare_names() {
        let bins = AgentBinaries::default();
        assert_eq!(bins.claude, "claude");
        assert_eq!(bins.dispatch, "dispatch");
    }

    /// Production must keep resolving on `PATH`: the trait default is what every
    /// real runner inherits, and no production code overrides it.
    #[test]
    fn real_process_runner_uses_default_agent_binaries() {
        assert_eq!(RealProcessRunner.agent_binaries(), AgentBinaries::default());
    }

    #[test]
    fn mock_process_runner_reports_the_binaries_it_was_built_with() {
        let bins = AgentBinaries::stub();
        let mock = MockProcessRunner::new(vec![]).with_agent_binaries(bins.clone());
        assert_eq!(mock.agent_binaries(), bins);
    }

    /// The pass-through case is what keeps the bare default names out of the
    /// emitted command string unquoted, exactly as they were before.
    #[test]
    fn shell_quote_leaves_plain_paths_untouched() {
        for s in ["claude", "/tmp/x/claude", "./claude", "claude-1.2_beta"] {
            assert_eq!(shell_quote(s), s, "{s} should need no quoting");
        }
    }

    #[test]
    fn shell_quote_wraps_paths_needing_it() {
        assert_eq!(shell_quote("/my dir/claude"), "'/my dir/claude'");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("a;rm -rf /"), "'a;rm -rf /'");
    }

    /// An embedded single quote cannot be escaped *inside* single quotes, so it
    /// has to close, escape and reopen — get this wrong and the quoting is a
    /// shell injection rather than a defence against one.
    #[test]
    fn shell_quote_escapes_embedded_single_quotes() {
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn claude_quoted_passes_a_plain_name_through_unchanged() {
        assert_eq!(AgentBinaries::default().claude_quoted(), "claude");
    }

    /// Executed rather than string-compared: the value is interpolated into
    /// exactly the shape `dispatch_with_prompt` emits — as bash's `$0`, *after*
    /// the single-quoted script body — handed to a real shell, and the binary
    /// must run as a single word with its argument intact.
    ///
    /// This is what pins the `$0` arrangement in place. Move the binary back
    /// inside the quoted body and a path with a space needs escaping twice; this
    /// test fails for `"claude bin"` if anyone does.
    #[test]
    fn claude_quoted_survives_the_launcher_command_shape() {
        for name in ["claude bin", "cla'ude", "claude"] {
            let dir = tempfile::tempdir().unwrap();
            let bin = dir.path().join(name);
            std::fs::write(&bin, "#!/bin/sh\nprintf 'ran:%s' \"$1\"\n").unwrap();
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();

            let bins = AgentBinaries {
                claude: bin.to_string_lossy().into_owned(),
                ..AgentBinaries::default()
            };
            let claude = bins.claude_quoted();
            let cmd = format!("bash -c '\"$0\" arg' {claude}");

            let out = std::process::Command::new("sh")
                .args(["-c", &cmd])
                .output()
                .unwrap();
            assert_eq!(
                String::from_utf8_lossy(&out.stdout),
                "ran:arg",
                "quoting failed for {name:?}; command was: {cmd} (stderr: {})",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    #[test]
    fn agent_binaries_stub_names_both_binaries_distinctly() {
        let stub = AgentBinaries::stub();
        assert_ne!(stub, AgentBinaries::default());
        assert_ne!(stub.claude, stub.dispatch);
    }

    // --- RealProcessRunner::run_with_timeout ---

    #[test]
    fn real_run_with_timeout_returns_output_on_success() {
        let runner = RealProcessRunner;
        let result = runner.run_with_timeout("true", &[], Duration::from_secs(5));
        assert!(result.is_ok(), "expected success, got: {result:?}");
        assert!(result.unwrap().status.success());
    }

    #[test]
    fn real_run_with_timeout_kills_stuck_process_and_returns_error() {
        let runner = RealProcessRunner;
        // sleep 10 will be killed after 100ms timeout
        let result = runner.run_with_timeout("sleep", &["10"], Duration::from_millis(100));
        assert!(result.is_err(), "expected timeout error, got success");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("timed out") || msg.contains("killed"),
            "unexpected error message: {msg}"
        );
    }

    #[test]
    fn real_run_with_timeout_captures_stdout() {
        let runner = RealProcessRunner;
        let result = runner.run_with_timeout("echo", &["hello"], Duration::from_secs(5));
        assert!(result.is_ok());
        let output = result.unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("hello"), "stdout: {stdout:?}");
    }

    /// A missing binary (e.g. tmux not installed) must surface as a spawn error
    /// with the program name in the context, not a panic or silent success.
    const MISSING_BINARY: &str = "dispatch-nonexistent-binary-xyzzy";

    #[test]
    fn real_run_missing_binary_returns_error() {
        let runner = RealProcessRunner;
        let result = runner.run(MISSING_BINARY, &[]);
        assert!(result.is_err(), "expected spawn error, got: {result:?}");
        let msg = format!("{:#}", result.unwrap_err());
        assert!(
            msg.contains(MISSING_BINARY),
            "error should name the missing program, got: {msg}"
        );
    }

    #[test]
    fn real_run_with_timeout_missing_binary_returns_error() {
        let runner = RealProcessRunner;
        let result = runner.run_with_timeout(MISSING_BINARY, &[], Duration::from_secs(5));
        assert!(result.is_err(), "expected spawn error, got: {result:?}");
        let msg = format!("{:#}", result.unwrap_err());
        assert!(
            msg.contains(MISSING_BINARY),
            "error should name the missing program, got: {msg}"
        );
    }

    // --- MockProcessRunner::run_with_timeout ---

    #[test]
    fn mock_run_with_timeout_records_call() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        mock.run_with_timeout("git", &["fetch"], Duration::from_secs(5))
            .unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "git");
        assert_eq!(calls[0].1, vec!["fetch"]);
    }

    #[test]
    fn mock_run_with_timeout_succeeds_when_delay_within_timeout() {
        let mock = MockProcessRunner::new_with_delays(vec![(
            Some(Duration::from_millis(10)),
            MockProcessRunner::ok(),
        )]);
        let result = mock.run_with_timeout("git", &["fetch"], Duration::from_millis(500));
        assert!(result.is_ok(), "expected success, got: {result:?}");
    }

    #[test]
    fn mock_run_with_timeout_returns_error_when_delay_exceeds_timeout() {
        let mock = MockProcessRunner::new_with_delays(vec![(
            Some(Duration::from_millis(200)),
            MockProcessRunner::ok(),
        )]);
        let result = mock.run_with_timeout("git", &["fetch"], Duration::from_millis(50));
        assert!(result.is_err(), "expected timeout error, got success");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("timed out") || msg.contains("killed"),
            "unexpected error message: {msg}"
        );
    }

    #[test]
    fn mock_run_with_timeout_no_delay_always_succeeds() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        let result = mock.run_with_timeout("git", &["status"], Duration::from_millis(1));
        assert!(result.is_ok());
    }

    #[test]
    fn fail_with_code_reports_the_requested_exit_code() {
        let out = MockProcessRunner::fail_with_code(2, "no matching ref").unwrap();
        assert_eq!(out.status.code(), Some(2));
        assert!(!out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stderr), "no matching ref");
    }
}
