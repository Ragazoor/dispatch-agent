use anyhow::{Context, Result, bail};

use crate::process::ProcessRunner;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new tmux window with the given name, starting in `working_dir`.
pub fn new_window(name: &str, working_dir: &str, runner: &dyn ProcessRunner) -> Result<()> {
    let output = runner.run("tmux", &["new-window", "-d", "-n", name, "-c", working_dir])?;
    if !output.status.success() {
        bail!("tmux new-window failed with status {}", output.status);
    }
    Ok(())
}

/// Send literal text to a tmux window, then press Enter.
///
/// Uses `-l` to prevent tmux from interpreting escape sequences in the text.
/// Enter is sent as a separate `send-keys` call without `-l`.
pub fn send_keys(window: &str, keys: &str, runner: &dyn ProcessRunner) -> Result<()> {
    let output = runner.run("tmux", &["send-keys", "-t", window, "-l", keys])?;
    if !output.status.success() {
        bail!("tmux send-keys -l failed with status {}", output.status);
    }
    let output = runner.run("tmux", &["send-keys", "-t", window, "Enter"])?;
    if !output.status.success() {
        bail!("tmux send-keys Enter failed with status {}", output.status);
    }
    Ok(())
}

/// Capture the last `lines` lines of output from a tmux pane, returned trimmed.
pub fn capture_pane(window: &str, lines: usize, runner: &dyn ProcessRunner) -> Result<String> {
    let lines_arg = format!("-{lines}");
    let output = runner.run(
        "tmux",
        &["capture-pane", "-t", window, "-p", "-S", &lines_arg],
    )?;
    if !output.status.success() {
        bail!("tmux capture-pane failed with status {}", output.status);
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(text)
}

/// Return true if a tmux window with the given name currently exists.
pub fn has_window(window: &str, runner: &dyn ProcessRunner) -> Result<bool> {
    let output = runner
        .run("tmux", &["list-windows", "-F", "#{window_name}"])
        .context("failed to run tmux list-windows")?;
    // list-windows exits non-zero when there are no windows / no session;
    // treat that as "window not found" rather than a hard error.
    if !output.status.success() {
        return Ok(false);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text.lines().any(|line| line.trim() == window))
}

/// Kill the tmux window with the given name.
pub fn kill_window(window: &str, runner: &dyn ProcessRunner) -> Result<()> {
    let output = runner.run("tmux", &["kill-window", "-t", window])?;
    if !output.status.success() {
        bail!("tmux kill-window failed with status {}", output.status);
    }
    Ok(())
}

/// Switch the active tmux window to the one with the given name.
pub fn select_window(window: &str, runner: &dyn ProcessRunner) -> Result<()> {
    let output = runner.run("tmux", &["select-window", "-t", window])?;
    if !output.status.success() {
        bail!("tmux select-window failed with status {}", output.status);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Internal helpers (kept for arg-shape unit tests)
// ---------------------------------------------------------------------------

#[cfg(test)]
fn select_window_args(window: &str) -> Vec<String> {
    vec!["select-window".to_string(), "-t".to_string(), window.to_string()]
}

#[cfg(test)]
fn new_window_args(name: &str, working_dir: &str) -> Vec<String> {
    vec![
        "new-window".to_string(),
        "-d".to_string(),
        "-n".to_string(),
        name.to_string(),
        "-c".to_string(),
        working_dir.to_string(),
    ]
}

#[cfg(test)]
fn capture_pane_args(window: &str, lines: usize) -> Vec<String> {
    vec![
        "capture-pane".to_string(),
        "-t".to_string(),
        window.to_string(),
        "-p".to_string(),
        "-S".to_string(),
        format!("-{lines}"),
    ]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_window_args_correct() {
        let args = new_window_args("task-42", "/some/path");
        assert_eq!(
            args,
            vec!["new-window", "-d", "-n", "task-42", "-c", "/some/path"]
        );
    }

    #[test]
    fn capture_pane_args_correct() {
        let args = capture_pane_args("task-42", 5);
        assert_eq!(
            args,
            vec!["capture-pane", "-t", "task-42", "-p", "-S", "-5"]
        );
    }

    #[test]
    fn capture_pane_args_different_line_count() {
        let args = capture_pane_args("my-window", 100);
        assert_eq!(args[5], "-100");
    }

    #[test]
    fn has_window_finds_match_in_output() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"main\ntask-42\nother-window\n"),
        ]);
        let result = has_window("task-42", &mock).unwrap();
        assert!(result);
    }

    #[test]
    fn has_window_no_match() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"main\nother-window\n"),
        ]);
        let result = has_window("task-42", &mock).unwrap();
        assert!(!result);
    }

    #[test]
    fn has_window_exact_match_not_prefix() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"task-42\n"),
        ]);
        let result = has_window("task-4", &mock).unwrap();
        assert!(!result);
    }

    #[test]
    fn select_window_args_correct() {
        let args = select_window_args("task-42");
        assert_eq!(args, vec!["select-window", "-t", "task-42"]);
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
    fn capture_pane_returns_trimmed_stdout() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(
            b"  hello from tmux  \n",
        )]);
        let result = capture_pane("task-42", 5, &mock).unwrap();
        assert_eq!(result, "hello from tmux");
    }

    #[test]
    fn has_window_returns_false_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no sessions")]);
        let result = has_window("task-42", &mock).unwrap();
        assert!(!result);
    }
}
