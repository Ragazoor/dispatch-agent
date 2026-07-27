use anyhow::{bail, Context, Result};
use std::process::Output;

use crate::process::{stderr_str, stdout_str, ProcessRunner};

// ---------------------------------------------------------------------------
// Shared checked-run helper
// ---------------------------------------------------------------------------

/// Build the consistent `"tmux {context} failed with status {status}[: {stderr}]"`
/// error for a failed [`Output`].
fn checked_error(context: &str, output: &Output) -> anyhow::Error {
    let stderr = stderr_str(output);
    if stderr.is_empty() {
        anyhow::anyhow!("tmux {context} failed with status {}", output.status)
    } else {
        anyhow::anyhow!(
            "tmux {context} failed with status {}: {}",
            output.status,
            stderr
        )
    }
}

/// Run `tmux` with `args`, returning the raw [`Output`] on success and a
/// consistent checked-run error (see [`checked_error`]) otherwise.
fn run_checked(runner: &dyn ProcessRunner, args: &[&str], context: &str) -> Result<Output> {
    let output = runner.run("tmux", args)?;
    if !output.status.success() {
        return Err(checked_error(context, &output));
    }
    Ok(output)
}

/// Like [`run_checked`], but returns trimmed stdout as a `String` instead of
/// the raw `Output` — for calls whose stdout is the actual result.
fn run_checked_stdout(runner: &dyn ProcessRunner, args: &[&str], context: &str) -> Result<String> {
    let output = run_checked(runner, args, context)?;
    Ok(stdout_str(&output))
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new tmux window with the given name, starting in `working_dir`.
pub fn new_window(name: &str, working_dir: &str, runner: &dyn ProcessRunner) -> Result<()> {
    run_checked(
        runner,
        &["new-window", "-d", "-n", name, "-c", working_dir],
        "new-window",
    )?;
    Ok(())
}

/// Create a new tmux window running the given command as separate argv
/// elements (no shell wrapping). When the command exits, the window closes.
///
/// `-d` keeps current focus; callers use [`select_window`] afterwards to
/// switch to the new window if desired.
pub fn new_window_running(
    name: &str,
    working_dir: &str,
    command: &[&str],
    runner: &dyn ProcessRunner,
) -> Result<()> {
    if command.is_empty() {
        bail!("new_window_running: command must not be empty");
    }
    let mut args: Vec<&str> = vec!["new-window", "-d", "-n", name, "-c", working_dir, "--"];
    args.extend(command.iter().copied());
    run_checked(runner, &args, "new-window")?;
    Ok(())
}

/// Send literal text to a tmux window, then press Enter.
///
/// Uses `-l` to prevent tmux from interpreting escape sequences in the text.
/// Enter is sent as a separate `send-keys` call without `-l`.
pub fn send_keys(window: &str, keys: &str, runner: &dyn ProcessRunner) -> Result<()> {
    run_checked(
        runner,
        &["send-keys", "-t", window, "-l", keys],
        "send-keys -l",
    )?;
    run_checked(
        runner,
        &["send-keys", "-t", window, "Enter"],
        "send-keys Enter",
    )?;
    Ok(())
}

/// Return true if a tmux window with the given name currently exists,
/// searching across all sessions. Built on [`list_all_window_names`], which
/// issues the identical `list-windows -a` query — same `-a` rationale: works
/// whether the caller is inside or outside tmux, and finds windows living in
/// a session other than the current/attached one.
pub fn has_window(window: &str, runner: &dyn ProcessRunner) -> Result<bool> {
    Ok(list_all_window_names(runner)?.iter().any(|n| n == window))
}

/// Whether `window` should be treated as alive: `true` when [`has_window`]
/// finds it, but also `true` when the query itself fails.
///
/// A query failure (tmux not reachable, transient error) is deliberately
/// mapped to "present" rather than "absent" — the callers of this helper use
/// the result to decide whether to treat a task's agent as crashed or the
/// main session as gone, and a false "absent" would trigger a spurious
/// re-dispatch or crash notification from a hiccup that has nothing to do
/// with the window's actual state. See `has_window`'s other callers
/// (`kill_window_if_present`) for the opposite default, which applies where
/// the gated action is itself destructive.
pub fn has_window_or_assume_present(window: &str, runner: &dyn ProcessRunner) -> bool {
    has_window(window, runner).unwrap_or(true)
}

/// Kill `window` if a live check finds it present.
///
/// Unlike [`has_window_or_assume_present`], a query failure here is logged
/// and treated as "nothing to kill" rather than propagated or assumed
/// present — attempting a `kill-window` against a query we couldn't
/// validate risks a hard failure that would abort the rest of the
/// caller's cleanup (e.g. removing the git worktree). Skipping the kill is
/// the safe choice when we can't tell whether the window still exists.
pub fn kill_window_if_present(window: &str, runner: &dyn ProcessRunner) -> Result<()> {
    match has_window(window, runner) {
        Ok(true) => kill_window(window, runner),
        Ok(false) => Ok(()),
        Err(e) => {
            tracing::warn!("could not check tmux window '{window}' before kill: {e}");
            Ok(())
        }
    }
}

/// List the names of all tmux windows across all sessions.
///
/// Uses `-a` so the query works whether the caller is inside or outside tmux.
/// Returns an empty vec (not an error) when no tmux server is running.
pub fn list_all_window_names(runner: &dyn ProcessRunner) -> Result<Vec<String>> {
    let output = runner.run("tmux", &["list-windows", "-a", "-F", "#{window_name}"])?;
    if !output.status.success() {
        return Ok(vec![]);
    }
    let names = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    Ok(names)
}

/// Kill the tmux window with the given name.
pub fn kill_window(window: &str, runner: &dyn ProcessRunner) -> Result<()> {
    run_checked(runner, &["kill-window", "-t", window], "kill-window")?;
    Ok(())
}

/// Switch the active tmux window to the one with the given name.
pub fn select_window(window: &str, runner: &dyn ProcessRunner) -> Result<()> {
    run_checked(runner, &["select-window", "-t", window], "select-window")?;
    Ok(())
}

/// Store the worktree path as a per-window user option so the session-level
/// `after-split-window` hook (installed by [`ensure_split_hook`]) can look it
/// up when a split happens in this window.
pub fn set_window_dispatch_dir(
    window: &str,
    working_dir: &str,
    runner: &dyn ProcessRunner,
) -> Result<()> {
    let args = [
        "set-option",
        "-w",
        "-t",
        window,
        "@dispatch_dir",
        working_dir,
    ];
    let output = runner.run("tmux", &args)?;
    if !output.status.success() {
        if String::from_utf8_lossy(&output.stderr).contains("ambiguous") {
            bail!(
                "multiple tmux windows named '{}' exist — close the duplicate windows before dispatching",
                window
            );
        }
        return Err(checked_error("set-option", &output));
    }
    Ok(())
}

/// Install a single session-level `after-split-window` hook that reads the
/// `@dispatch_dir` window option.  If the option is set on the window being
/// split, the new pane `cd`s into that directory; otherwise nothing happens.
///
/// This is idempotent — calling it multiple times replaces the same hook.
pub fn ensure_split_hook(runner: &dyn ProcessRunner) -> Result<()> {
    // if-shell -F only format-expands its test argument, NOT the branch
    // command.  send-keys doesn't expand formats either, so we wrap it in
    // run-shell -C which does expand #{…} before executing the tmux command.
    let hook_cmd = "if-shell -F '#{@dispatch_dir}' 'run-shell -bC \"send-keys \\\"cd #{@dispatch_dir}\\\" Enter\"'";
    run_checked(
        runner,
        &["set-hook", "after-split-window", hook_cmd],
        "set-hook",
    )?;
    Ok(())
}

/// Check whether tmux has `focus-events` enabled globally.
///
/// Returns `false` if the option is off or if the query fails (e.g. not
/// running inside tmux).
pub fn focus_events_enabled(runner: &dyn ProcessRunner) -> bool {
    let Ok(output) = runner.run("tmux", &["show-options", "-gv", "focus-events"]) else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.trim() == "on"
}

/// Enable tmux `focus-events` globally.
///
/// This is idempotent — calling it when already enabled is a no-op.
pub fn set_focus_events(runner: &dyn ProcessRunner) -> Result<()> {
    run_checked(
        runner,
        &["set-option", "-g", "focus-events", "on"],
        "set-option focus-events",
    )?;
    Ok(())
}

/// Path to the user's `~/.tmux.conf`. Owned here so callers don't re-derive
/// the `$HOME`-relative location.
pub(crate) fn tmux_conf_path() -> Result<std::path::PathBuf> {
    let home = std::env::var("HOME").context("HOME not set")?;
    Ok(std::path::PathBuf::from(home).join(".tmux.conf"))
}

pub(crate) fn write_focus_events_to_tmux_conf_at(path: &std::path::Path) -> Result<()> {
    let existing = if path.exists() {
        std::fs::read_to_string(path).context("failed to read .tmux.conf")?
    } else {
        String::new()
    };
    if existing.contains("focus-events on") {
        return Ok(());
    }
    let addition = if existing.ends_with('\n') || existing.is_empty() {
        "set -g focus-events on\n".to_string()
    } else {
        "\nset -g focus-events on\n".to_string()
    };
    std::fs::write(path, existing + &addition).context("failed to write .tmux.conf")?;
    Ok(())
}

/// Return the name of the currently active tmux window.
pub fn current_window_name(runner: &dyn ProcessRunner) -> Result<String> {
    run_checked_stdout(runner, &["display-message", "-p", "#W"], "display-message")
}

/// Rename a tmux window. Pass `""` as `target` to rename the current window.
pub fn rename_window(target: &str, new_name: &str, runner: &dyn ProcessRunner) -> Result<()> {
    run_checked(
        runner,
        &["rename-window", "-t", target, new_name],
        "rename-window",
    )?;
    Ok(())
}

/// Bind a tmux key (requires the tmux prefix first) to a command string.
pub fn bind_key(key: &str, command: &str, runner: &dyn ProcessRunner) -> Result<()> {
    run_checked(runner, &["bind-key", key, command], "bind-key")?;
    Ok(())
}

/// Remove a tmux key binding (previously registered with `bind-key`).
pub fn unbind_key(key: &str, runner: &dyn ProcessRunner) -> Result<()> {
    run_checked(runner, &["unbind-key", key], "unbind-key")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Split mode operations
// ---------------------------------------------------------------------------

/// Return the tmux pane ID of the current pane (e.g. "%42").
pub fn current_pane_id(runner: &dyn ProcessRunner) -> Result<String> {
    run_checked_stdout(
        runner,
        &["display-message", "-p", "#{pane_id}"],
        "display-message",
    )
}

/// Create a horizontal split (right pane) at 40% width, keeping focus on the
/// left pane. Returns the new pane's ID.
pub fn split_window_horizontal(target_pane: &str, runner: &dyn ProcessRunner) -> Result<String> {
    run_checked_stdout(
        runner,
        &[
            "split-window",
            "-h",
            "-d",
            "-l",
            "40%",
            "-t",
            target_pane,
            "-P",
            "-F",
            "#{pane_id}",
        ],
        "split-window",
    )
}

/// Create a horizontal split (right pane) at `size_pct`% width, running the
/// given command as separate argv elements (no shell wrapping) in the new
/// pane, following the same "create + immediately run a command" shape
/// [`new_window_running`] establishes for window creation. Keeps focus on
/// the target pane. Returns the new pane's ID.
///
/// Sibling of [`split_window_horizontal`] (hardcoded 40%, no command, used
/// by the board's own split-pane feature) — this one is for spawning a
/// companion process (e.g. `dispatch agent-tree <task_id>`) narrower than
/// that split, since the target pane's own output still needs the room.
pub fn split_window_horizontal_running(
    target_pane: &str,
    size_pct: u8,
    command: &[&str],
    runner: &dyn ProcessRunner,
) -> Result<String> {
    if command.is_empty() {
        bail!("split_window_horizontal_running: command must not be empty");
    }
    let size_arg = format!("{size_pct}%");
    let mut args: Vec<&str> = vec![
        "split-window",
        "-h",
        "-d",
        "-l",
        &size_arg,
        "-t",
        target_pane,
        "-P",
        "-F",
        "#{pane_id}",
        "--",
    ];
    args.extend(command.iter().copied());
    run_checked_stdout(runner, &args, "split-window")
}

/// Move a tmux window into the current window as a right pane (40% width).
/// Returns the new pane's ID.
pub fn join_pane(
    source_window: &str,
    target_pane: &str,
    runner: &dyn ProcessRunner,
) -> Result<String> {
    // Get the source pane ID first — pane IDs are preserved across moves,
    // and join-pane does not support -P/-F for printing the result.
    let pane_id = run_checked_stdout(
        runner,
        &["display-message", "-p", "-t", source_window, "#{pane_id}"],
        "display-message",
    )?;

    run_checked(
        runner,
        &[
            "join-pane",
            "-h",
            "-d",
            "-s",
            source_window,
            "-t",
            target_pane,
            "-l",
            "40%",
        ],
        "join-pane",
    )?;
    Ok(pane_id)
}

/// Break a pane out into its own tmux window with the given name.
pub fn break_pane_to_window(
    pane_id: &str,
    window_name: &str,
    runner: &dyn ProcessRunner,
) -> Result<()> {
    run_checked(
        runner,
        &["break-pane", "-d", "-s", pane_id, "-n", window_name],
        "break-pane",
    )?;
    Ok(())
}

/// Kill a specific tmux pane by ID.
pub fn kill_pane(pane_id: &str, runner: &dyn ProcessRunner) -> Result<()> {
    run_checked(runner, &["kill-pane", "-t", pane_id], "kill-pane")?;
    Ok(())
}

/// Replace the content of a pane with a fresh shell, preserving the pane itself.
pub fn respawn_pane(pane_id: &str, runner: &dyn ProcessRunner) -> Result<()> {
    run_checked(
        runner,
        &["respawn-pane", "-k", "-t", pane_id],
        "respawn-pane",
    )?;
    Ok(())
}

/// Get the pane ID for a window's first pane.
pub fn pane_id_for_window(window: &str, runner: &dyn ProcessRunner) -> Result<String> {
    run_checked_stdout(
        runner,
        &["display-message", "-p", "-t", window, "#{pane_id}"],
        "display-message",
    )
}

/// Atomically swap the contents of two panes without changing the layout.
/// `source` can be a pane ID or `<window>.0` to reference a window's first pane.
/// `-d` keeps focus on the current pane.
pub fn swap_pane(source: &str, target: &str, runner: &dyn ProcessRunner) -> Result<()> {
    run_checked(
        runner,
        &["swap-pane", "-d", "-s", source, "-t", target],
        "swap-pane",
    )?;
    Ok(())
}

/// Move tmux focus to the specified pane.
pub fn select_pane(pane_id: &str, runner: &dyn ProcessRunner) -> Result<()> {
    run_checked(runner, &["select-pane", "-t", pane_id], "select-pane")?;
    Ok(())
}

/// Check whether a tmux pane with the given ID still exists.
pub fn pane_exists(pane_id: &str, runner: &dyn ProcessRunner) -> bool {
    runner
        .run("tmux", &["display-message", "-t", pane_id, "-p", ""])
        .map(|output| output.status.success())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn has_window_finds_match_in_output() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(
            b"main\ntask-42\nother-window\n",
        )]);
        let result = has_window("task-42", &mock).unwrap();
        assert!(result);
    }

    #[test]
    fn has_window_no_match() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(
            b"main\nother-window\n",
        )]);
        let result = has_window("task-42", &mock).unwrap();
        assert!(!result);
    }

    #[test]
    fn has_window_exact_match_not_prefix() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"task-42\n")]);
        let result = has_window("task-4", &mock).unwrap();
        assert!(!result);
    }

    // --- ProcessRunner-based tests ---

    use crate::process::MockProcessRunner;

    #[test]
    fn new_window_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        new_window("task-42", "/some/path", &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(
            calls[0].1,
            vec!["new-window", "-d", "-n", "task-42", "-c", "/some/path"]
        );
    }

    #[test]
    fn new_window_running_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        new_window_running("dispatch-edit-1", "/home/u", &["vim", "/tmp/foo.md"], &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(
            calls[0].1,
            vec![
                "new-window",
                "-d",
                "-n",
                "dispatch-edit-1",
                "-c",
                "/home/u",
                "--",
                "vim",
                "/tmp/foo.md"
            ]
        );
    }

    #[test]
    fn new_window_running_keeps_argv_elements_separate() {
        // A path with spaces must be passed as its own argv element, not
        // joined into a single shell string. This is why we use the `--`
        // exec form rather than `send-keys` with a concatenated command.
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        new_window_running(
            "edit-1",
            "/tmp",
            &["vim", "/tmp/dir with spaces/file.md"],
            &mock,
        )
        .unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls[0].1.last().unwrap(), "/tmp/dir with spaces/file.md");
        // and the preceding element is the exec separator + program
        assert_eq!(calls[0].1[calls[0].1.len() - 3], "--");
        assert_eq!(calls[0].1[calls[0].1.len() - 2], "vim");
    }

    #[test]
    fn new_window_running_rejects_empty_command() {
        let mock = MockProcessRunner::new(vec![]);
        let err = new_window_running("n", "/tmp", &[], &mock).unwrap_err();
        assert!(err.to_string().contains("command must not be empty"));
    }

    #[test]
    fn new_window_running_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("bad")]);
        let err = new_window_running("n", "/tmp", &["vim", "f"], &mock).unwrap_err();
        assert!(err.to_string().contains("new-window failed"));
    }

    #[test]
    fn has_window_returns_false_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no sessions")]);
        let result = has_window("task-42", &mock).unwrap();
        assert!(!result);
    }

    // --- has_window_or_assume_present ---

    #[test]
    fn has_window_or_assume_present_true_when_present() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"task-42\n")]);
        assert!(has_window_or_assume_present("task-42", &mock));
    }

    #[test]
    fn has_window_or_assume_present_false_when_absent() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"other\n")]);
        assert!(!has_window_or_assume_present("task-42", &mock));
    }

    #[test]
    fn has_window_or_assume_present_true_when_query_fails() {
        let mock = MockProcessRunner::new(vec![Err(anyhow::anyhow!("tmux: command not found"))]);
        assert!(has_window_or_assume_present("task-42", &mock));
    }

    // --- kill_window_if_present ---

    #[test]
    fn kill_window_if_present_kills_when_present() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"task-42\n"), // has_window
            MockProcessRunner::ok(),                         // kill-window
        ]);
        kill_window_if_present("task-42", &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1].1[0], "kill-window");
    }

    #[test]
    fn kill_window_if_present_skips_when_absent() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"other\n")]);
        kill_window_if_present("task-42", &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1, "kill-window should not be called");
    }

    #[test]
    fn kill_window_if_present_skips_and_succeeds_when_query_fails() {
        let mock = MockProcessRunner::new(vec![Err(anyhow::anyhow!("tmux: command not found"))]);
        kill_window_if_present("task-42", &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1, "kill-window should not be attempted");
    }

    #[test]
    fn kill_window_if_present_propagates_kill_failure() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"task-42\n"), // has_window
            MockProcessRunner::fail("no such window"),       // kill-window fails
        ]);
        let err = kill_window_if_present("task-42", &mock).unwrap_err();
        assert!(err.to_string().contains("kill-window failed"), "got: {err}");
    }

    #[test]
    fn has_window_queries_across_all_sessions() {
        // has_window is used for cross-session liveness checks (main session,
        // cleanup, finish, editor, staleness) — without -a, list-windows scopes
        // to the current/attached session only and misses windows living in
        // another session, producing a false "not found".
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"task-42\n")]);
        let _ = has_window("task-42", &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(
            calls[0].1,
            vec!["list-windows", "-a", "-F", "#{window_name}"]
        );
    }

    #[test]
    fn set_window_dispatch_dir_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        set_window_dispatch_dir("task-42", "/some/path", &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(
            calls[0].1,
            vec![
                "set-option",
                "-w",
                "-t",
                "task-42",
                "@dispatch_dir",
                "/some/path",
            ]
        );
    }

    #[test]
    fn set_window_dispatch_dir_detects_ambiguous_windows() {
        let mock =
            MockProcessRunner::new(vec![MockProcessRunner::fail("ambiguous window: task-42")]);
        let err = set_window_dispatch_dir("task-42", "/some/path", &mock).unwrap_err();
        assert!(err.to_string().contains("multiple tmux windows"));
    }

    #[test]
    fn ensure_split_hook_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        ensure_split_hook(&mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(
            calls[0].1,
            vec![
                "set-hook",
                "after-split-window",
                "if-shell -F '#{@dispatch_dir}' 'run-shell -bC \"send-keys \\\"cd #{@dispatch_dir}\\\" Enter\"'",
            ]
        );
    }

    #[test]
    fn current_window_name_returns_trimmed_stdout() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"dispatch\n")]);
        let result = current_window_name(&mock).unwrap();
        assert_eq!(result, "dispatch");
    }

    #[test]
    fn current_window_name_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"dispatch\n")]);
        current_window_name(&mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(calls[0].1, vec!["display-message", "-p", "#W"]);
    }

    #[test]
    fn current_window_name_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no session")]);
        assert!(current_window_name(&mock).is_err());
    }

    #[test]
    fn rename_window_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        rename_window("dispatch", "my-old-name", &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(
            calls[0].1,
            vec!["rename-window", "-t", "dispatch", "my-old-name"]
        );
    }

    #[test]
    fn rename_window_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no window")]);
        assert!(rename_window("dispatch", "other", &mock).is_err());
    }

    #[test]
    fn bind_key_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        bind_key("space", "select-window -t dispatch", &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(
            calls[0].1,
            vec!["bind-key", "space", "select-window -t dispatch"]
        );
    }

    #[test]
    fn unbind_key_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        unbind_key("space", &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(calls[0].1, vec!["unbind-key", "space"]);
    }

    #[test]
    fn join_pane_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"%5\n"), // display-message to get source pane ID
            MockProcessRunner::ok(),                    // join-pane (no -P/-F)
        ]);
        let pane_id = join_pane("task-42", "%1", &mock).unwrap();
        assert_eq!(pane_id, "%5");
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 2);
        // First call: get the source pane ID
        assert_eq!(
            calls[0].1,
            vec!["display-message", "-p", "-t", "task-42", "#{pane_id}"]
        );
        // Second call: join-pane without -P or -F
        assert_eq!(
            calls[1].1,
            vec![
                "join-pane",
                "-h",
                "-d",
                "-s",
                "task-42",
                "-t",
                "%1",
                "-l",
                "40%"
            ]
        );
    }

    #[test]
    fn join_pane_returns_source_pane_id() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"%99\n"),
            MockProcessRunner::ok(),
        ]);
        let result = join_pane("my-window", "%0", &mock).unwrap();
        assert_eq!(result, "%99");
    }

    #[test]
    fn select_pane_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        select_pane("%42", &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(calls[0].1, vec!["select-pane", "-t", "%42"]);
    }

    #[test]
    fn focus_events_enabled_returns_true_when_on() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"on\n")]);
        assert!(focus_events_enabled(&mock));
    }

    #[test]
    fn focus_events_enabled_returns_false_when_off() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"off\n")]);
        assert!(!focus_events_enabled(&mock));
    }

    #[test]
    fn set_focus_events_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        set_focus_events(&mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(calls[0].1, vec!["set-option", "-g", "focus-events", "on"]);
    }

    #[test]
    fn write_focus_events_creates_file_if_absent() {
        let dir = tempfile::tempdir().unwrap();
        let conf = dir.path().join(".tmux.conf");
        write_focus_events_to_tmux_conf_at(&conf).unwrap();
        let content = std::fs::read_to_string(&conf).unwrap();
        assert!(content.contains("set -g focus-events on"));
    }

    #[test]
    fn write_focus_events_appends_to_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let conf = dir.path().join(".tmux.conf");
        std::fs::write(&conf, "set -g mouse on\n").unwrap();
        write_focus_events_to_tmux_conf_at(&conf).unwrap();
        let content = std::fs::read_to_string(&conf).unwrap();
        assert!(content.contains("set -g mouse on"));
        assert!(content.contains("set -g focus-events on"));
    }

    #[test]
    fn write_focus_events_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let conf = dir.path().join(".tmux.conf");
        std::fs::write(&conf, "set -g focus-events on\n").unwrap();
        write_focus_events_to_tmux_conf_at(&conf).unwrap();
        let content = std::fs::read_to_string(&conf).unwrap();
        assert_eq!(
            content.matches("focus-events on").count(),
            1,
            "should not duplicate the line"
        );
    }

    #[test]
    fn list_all_window_names_parses_output() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(
            b"dispatch\ntask-42\ntask-99\n",
        )]);
        let names = list_all_window_names(&mock).unwrap();
        assert_eq!(names, vec!["dispatch", "task-42", "task-99"]);
    }

    #[test]
    fn list_all_window_names_empty_when_no_sessions() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no server running")]);
        let names = list_all_window_names(&mock).unwrap();
        assert!(
            names.is_empty(),
            "expected empty vec when tmux not running, got: {names:?}"
        );
    }

    #[test]
    fn list_all_window_names_issues_correct_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"dispatch\n")]);
        let _ = list_all_window_names(&mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(
            calls[0].1,
            vec!["list-windows", "-a", "-F", "#{window_name}"]
        );
    }

    // --- new_window failure path ---

    #[test]
    fn new_window_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no server running")]);
        let err = new_window("task-1", "/tmp", &mock).unwrap_err();
        assert!(
            err.to_string().contains("new-window failed"),
            "expected 'new-window failed', got: {err}"
        );
    }

    // --- send_keys ---

    #[test]
    fn send_keys_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // send-keys -l
            MockProcessRunner::ok(), // send-keys Enter
        ]);
        send_keys("task-1", "hello world", &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(
            calls[0].1,
            vec!["send-keys", "-t", "task-1", "-l", "hello world"]
        );
        assert_eq!(calls[1].0, "tmux");
        assert_eq!(calls[1].1, vec!["send-keys", "-t", "task-1", "Enter"]);
    }

    #[test]
    fn send_keys_fails_on_first_send_error() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no pane")]);
        let err = send_keys("task-1", "hello", &mock).unwrap_err();
        assert!(
            err.to_string().contains("send-keys -l failed"),
            "got: {err}"
        );
    }

    #[test]
    fn send_keys_fails_on_enter_send_error() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok(),              // send-keys -l succeeds
            MockProcessRunner::fail("pane gone"), // send-keys Enter fails
        ]);
        let err = send_keys("task-1", "hello", &mock).unwrap_err();
        assert!(
            err.to_string().contains("send-keys Enter failed"),
            "got: {err}"
        );
    }

    // --- kill_window ---

    #[test]
    fn kill_window_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        kill_window("task-42", &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(calls[0].1, vec!["kill-window", "-t", "task-42"]);
    }

    #[test]
    fn kill_window_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no window")]);
        let err = kill_window("task-42", &mock).unwrap_err();
        assert!(err.to_string().contains("kill-window failed"), "got: {err}");
    }

    // --- select_window failure ---

    #[test]
    fn select_window_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no window")]);
        let err = select_window("task-42", &mock).unwrap_err();
        assert!(
            err.to_string().contains("select-window failed"),
            "got: {err}"
        );
    }

    // --- ensure_split_hook failure ---

    #[test]
    fn ensure_split_hook_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no session")]);
        let err = ensure_split_hook(&mock).unwrap_err();
        assert!(err.to_string().contains("set-hook failed"), "got: {err}");
    }

    // --- set_window_dispatch_dir generic failure ---

    #[test]
    fn set_window_dispatch_dir_fails_on_generic_nonzero_exit() {
        // Non-ambiguous error (does not contain "ambiguous")
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no session running")]);
        let err = set_window_dispatch_dir("task-42", "/some/path", &mock).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("set-option failed"),
            "expected 'set-option failed', got: {msg}"
        );
        assert!(
            !msg.contains("multiple tmux windows"),
            "should not be the ambiguous-window error, got: {msg}"
        );
    }

    // --- split_window_horizontal ---

    #[test]
    fn split_window_horizontal_issues_correct_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"%5\n")]);
        let pane_id = split_window_horizontal("%1", &mock).unwrap();
        assert_eq!(pane_id, "%5");
        let calls = mock.recorded_calls();
        assert_eq!(
            calls[0].1,
            vec![
                "split-window",
                "-h",
                "-d",
                "-l",
                "40%",
                "-t",
                "%1",
                "-P",
                "-F",
                "#{pane_id}",
            ]
        );
    }

    #[test]
    fn split_window_horizontal_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no target pane")]);
        let err = split_window_horizontal("%1", &mock).unwrap_err();
        assert!(
            err.to_string().contains("split-window failed"),
            "got: {err}"
        );
    }

    // --- split_window_horizontal_running ---

    #[test]
    fn split_window_horizontal_running_issues_correct_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"%9\n")]);
        let pane_id =
            split_window_horizontal_running("%1", 30, &["dispatch", "agent-tree", "42"], &mock)
                .unwrap();
        assert_eq!(pane_id, "%9");
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(
            calls[0].1,
            vec![
                "split-window",
                "-h",
                "-d",
                "-l",
                "30%",
                "-t",
                "%1",
                "-P",
                "-F",
                "#{pane_id}",
                "--",
                "dispatch",
                "agent-tree",
                "42",
            ]
        );
    }

    #[test]
    fn split_window_horizontal_running_keeps_argv_elements_separate() {
        // A path with spaces must stay one argv element via the `--` exec
        // form, not get joined into a single shell string.
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"%9\n")]);
        split_window_horizontal_running("%1", 30, &["vim", "/tmp/dir with spaces/file.md"], &mock)
            .unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls[0].1.last().unwrap(), "/tmp/dir with spaces/file.md");
        assert_eq!(calls[0].1[calls[0].1.len() - 3], "--");
        assert_eq!(calls[0].1[calls[0].1.len() - 2], "vim");
    }

    #[test]
    fn split_window_horizontal_running_rejects_empty_command() {
        let mock = MockProcessRunner::new(vec![]);
        let err = split_window_horizontal_running("%1", 30, &[], &mock).unwrap_err();
        assert!(err.to_string().contains("command must not be empty"));
    }

    #[test]
    fn split_window_horizontal_running_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no target pane")]);
        let err =
            split_window_horizontal_running("%1", 30, &["dispatch", "agent-tree", "42"], &mock)
                .unwrap_err();
        assert!(
            err.to_string().contains("split-window failed"),
            "got: {err}"
        );
    }

    // --- join_pane failure paths ---

    #[test]
    fn join_pane_fails_when_display_message_fails() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no such window")]);
        let err = join_pane("task-42", "%1", &mock).unwrap_err();
        assert!(
            err.to_string().contains("display-message failed"),
            "got: {err}"
        );
    }

    #[test]
    fn join_pane_fails_when_join_pane_command_fails() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"%5\n"), // display-message ok
            MockProcessRunner::fail("invalid target"),  // join-pane fails
        ]);
        let err = join_pane("task-42", "%1", &mock).unwrap_err();
        assert!(err.to_string().contains("join-pane failed"), "got: {err}");
    }

    // --- break_pane_to_window ---

    #[test]
    fn break_pane_to_window_issues_correct_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        break_pane_to_window("%5", "new-win", &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(
            calls[0].1,
            vec!["break-pane", "-d", "-s", "%5", "-n", "new-win"]
        );
    }

    #[test]
    fn break_pane_to_window_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no such pane")]);
        let err = break_pane_to_window("%5", "new-win", &mock).unwrap_err();
        assert!(err.to_string().contains("break-pane failed"), "got: {err}");
    }

    // --- kill_pane ---

    #[test]
    fn kill_pane_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        kill_pane("%42", &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls[0].1, vec!["kill-pane", "-t", "%42"]);
    }

    #[test]
    fn kill_pane_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no pane")]);
        let err = kill_pane("%42", &mock).unwrap_err();
        assert!(err.to_string().contains("kill-pane failed"), "got: {err}");
    }

    // --- respawn_pane ---

    #[test]
    fn respawn_pane_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        respawn_pane("%42", &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls[0].1, vec!["respawn-pane", "-k", "-t", "%42"]);
    }

    #[test]
    fn respawn_pane_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no such pane")]);
        let err = respawn_pane("%42", &mock).unwrap_err();
        assert!(
            err.to_string().contains("respawn-pane failed"),
            "got: {err}"
        );
    }

    // --- pane_id_for_window ---

    #[test]
    fn pane_id_for_window_issues_correct_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"%3\n")]);
        let result = pane_id_for_window("task-42", &mock).unwrap();
        assert_eq!(result, "%3");
        let calls = mock.recorded_calls();
        assert_eq!(
            calls[0].1,
            vec!["display-message", "-p", "-t", "task-42", "#{pane_id}"]
        );
    }

    #[test]
    fn pane_id_for_window_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no such window")]);
        let err = pane_id_for_window("task-42", &mock).unwrap_err();
        assert!(
            err.to_string().contains("display-message failed"),
            "got: {err}"
        );
    }

    // --- swap_pane ---

    #[test]
    fn swap_pane_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        swap_pane("%1", "%2", &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls[0].1, vec!["swap-pane", "-d", "-s", "%1", "-t", "%2"]);
    }

    #[test]
    fn swap_pane_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no pane")]);
        let err = swap_pane("%1", "%2", &mock).unwrap_err();
        assert!(err.to_string().contains("swap-pane failed"), "got: {err}");
    }

    // --- current_pane_id ---

    #[test]
    fn current_pane_id_issues_correct_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"%42\n")]);
        let result = current_pane_id(&mock).unwrap();
        assert_eq!(result, "%42");
        let calls = mock.recorded_calls();
        assert_eq!(calls[0].1, vec!["display-message", "-p", "#{pane_id}"]);
    }

    #[test]
    fn current_pane_id_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no session")]);
        let err = current_pane_id(&mock).unwrap_err();
        assert!(
            err.to_string().contains("display-message failed"),
            "got: {err}"
        );
    }

    // --- pane_exists ---

    #[test]
    fn pane_exists_returns_true_on_success() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        assert!(pane_exists("%42", &mock));
    }

    #[test]
    fn pane_exists_returns_false_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no such pane")]);
        assert!(!pane_exists("%42", &mock));
    }

    #[test]
    fn pane_exists_returns_false_on_runner_error() {
        let mock = MockProcessRunner::new(vec![Err(anyhow::anyhow!("binary not found"))]);
        assert!(!pane_exists("%42", &mock));
    }

    // --- set_focus_events failure ---

    #[test]
    fn set_focus_events_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no server running")]);
        let err = set_focus_events(&mock).unwrap_err();
        assert!(
            err.to_string().contains("set-option focus-events failed"),
            "got: {err}"
        );
    }

    // --- focus_events_enabled runner error ---

    #[test]
    fn focus_events_enabled_returns_false_on_runner_error() {
        let mock = MockProcessRunner::new(vec![Err(anyhow::anyhow!("tmux not found"))]);
        assert!(!focus_events_enabled(&mock));
    }

    // --- bind_key / unbind_key failure paths ---

    #[test]
    fn bind_key_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("invalid key")]);
        let err = bind_key("space", "select-window -t dispatch", &mock).unwrap_err();
        assert!(err.to_string().contains("bind-key failed"), "got: {err}");
    }

    #[test]
    fn unbind_key_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no key bound")]);
        let err = unbind_key("space", &mock).unwrap_err();
        assert!(err.to_string().contains("unbind-key failed"), "got: {err}");
    }

    // --- select_pane failure ---

    #[test]
    fn select_pane_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no such pane")]);
        let err = select_pane("%42", &mock).unwrap_err();
        assert!(err.to_string().contains("select-pane failed"), "got: {err}");
    }
}
