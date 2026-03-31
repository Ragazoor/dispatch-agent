use anyhow::{Context, Result, bail};

use crate::models::TmuxWindow;
use crate::process::ProcessRunner;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new tmux window with the given name, starting in `working_dir`.
pub fn new_window(name: &TmuxWindow, working_dir: &str, runner: &dyn ProcessRunner) -> Result<()> {
    let output = runner.run("tmux", &["new-window", "-d", "-n", name.as_ref(), "-c", working_dir])?;
    if !output.status.success() {
        bail!("tmux new-window failed with status {}", output.status);
    }
    Ok(())
}

/// Send literal text to a tmux window, then press Enter.
///
/// Uses `-l` to prevent tmux from interpreting escape sequences in the text.
/// Enter is sent as a separate `send-keys` call without `-l`.
pub fn send_keys(window: &TmuxWindow, keys: &str, runner: &dyn ProcessRunner) -> Result<()> {
    let output = runner.run("tmux", &["send-keys", "-t", window.as_ref(), "-l", keys])?;
    if !output.status.success() {
        bail!("tmux send-keys -l failed with status {}", output.status);
    }
    let output = runner.run("tmux", &["send-keys", "-t", window.as_ref(), "Enter"])?;
    if !output.status.success() {
        bail!("tmux send-keys Enter failed with status {}", output.status);
    }
    Ok(())
}

/// Capture the last `lines` lines of output from a tmux pane, returned trimmed.
pub fn capture_pane(window: &TmuxWindow, lines: usize, runner: &dyn ProcessRunner) -> Result<String> {
    let lines_arg = format!("-{lines}");
    let output = runner.run(
        "tmux",
        &["capture-pane", "-t", window.as_ref(), "-p", "-S", &lines_arg],
    )?;
    if !output.status.success() {
        bail!("tmux capture-pane failed with status {}", output.status);
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(text)
}

/// Return true if a tmux window with the given name currently exists.
pub fn has_window(window: &TmuxWindow, runner: &dyn ProcessRunner) -> Result<bool> {
    let output = runner
        .run("tmux", &["list-windows", "-F", "#{window_name}"])
        .context("failed to run tmux list-windows")?;
    // list-windows exits non-zero when there are no windows / no session;
    // treat that as "window not found" rather than a hard error.
    if !output.status.success() {
        return Ok(false);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text.lines().any(|line| line.trim() == window.as_ref()))
}

/// Return the Unix timestamp of the last activity in a tmux window.
///
/// Uses `tmux display-message` with the `#{window_activity}` format variable,
/// which reports a per-second resolution timestamp updated on any pane I/O.
pub fn window_activity(window: &TmuxWindow, runner: &dyn ProcessRunner) -> Result<u64> {
    let output = runner.run(
        "tmux",
        &["display-message", "-p", "-t", window.as_ref(), "#{window_activity}"],
    )?;
    if !output.status.success() {
        bail!("tmux display-message failed with status {}", output.status);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.trim()
        .parse::<u64>()
        .with_context(|| format!("failed to parse window_activity timestamp: {text:?}"))
}

/// Kill the tmux window with the given name.
pub fn kill_window(window: &TmuxWindow, runner: &dyn ProcessRunner) -> Result<()> {
    let output = runner.run("tmux", &["kill-window", "-t", window.as_ref()])?;
    if !output.status.success() {
        bail!("tmux kill-window failed with status {}", output.status);
    }
    Ok(())
}

/// Switch the active tmux window to the one with the given name.
pub fn select_window(window: &TmuxWindow, runner: &dyn ProcessRunner) -> Result<()> {
    let output = runner.run("tmux", &["select-window", "-t", window.as_ref()])?;
    if !output.status.success() {
        bail!("tmux select-window failed with status {}", output.status);
    }
    Ok(())
}

/// Set a per-window hook so that splitting a pane automatically `cd`s the new
/// pane into the given working directory.
pub fn set_after_split_hook(window: &TmuxWindow, working_dir: &str, runner: &dyn ProcessRunner) -> Result<()> {
    let hook_cmd = format!("send-keys 'cd {}' Enter", working_dir);
    let output = runner.run("tmux", &[
        "set-hook", "-w", "-t", window.as_ref(),
        "after-split-window", &hook_cmd,
    ])?;
    if !output.status.success() {
        bail!("tmux set-hook failed with status {}", output.status);
    }
    Ok(())
}

/// Return the name of the currently active tmux window.
pub fn current_window_name(runner: &dyn ProcessRunner) -> Result<String> {
    let output = runner.run("tmux", &["display-message", "-p", "#W"])?;
    if !output.status.success() {
        bail!("tmux display-message failed with status {}", output.status);
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(text)
}

/// Rename a tmux window. Pass `""` as `target` to rename the current window.
pub fn rename_window(target: &str, new_name: &str, runner: &dyn ProcessRunner) -> Result<()> {
    let output = runner.run("tmux", &["rename-window", "-t", target, new_name])?;
    if !output.status.success() {
        bail!("tmux rename-window failed with status {}", output.status);
    }
    Ok(())
}

/// Bind a tmux key (with the default prefix) to a command string.
pub fn bind_key(key: &str, command: &str, runner: &dyn ProcessRunner) -> Result<()> {
    let output = runner.run("tmux", &["bind-key", key, command])?;
    if !output.status.success() {
        bail!("tmux bind-key failed with status {}", output.status);
    }
    Ok(())
}

/// Remove a tmux key binding (with the default prefix).
pub fn unbind_key(key: &str, runner: &dyn ProcessRunner) -> Result<()> {
    let output = runner.run("tmux", &["unbind-key", key])?;
    if !output.status.success() {
        bail!("tmux unbind-key failed with status {}", output.status);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Internal helpers (kept for arg-shape unit tests)
// ---------------------------------------------------------------------------

#[cfg(test)]
fn select_window_args(window: &TmuxWindow) -> Vec<String> {
    vec!["select-window".to_string(), "-t".to_string(), window.0.clone()]
}

#[cfg(test)]
fn new_window_args(name: &TmuxWindow, working_dir: &str) -> Vec<String> {
    vec![
        "new-window".to_string(),
        "-d".to_string(),
        "-n".to_string(),
        name.0.clone(),
        "-c".to_string(),
        working_dir.to_string(),
    ]
}

#[cfg(test)]
fn capture_pane_args(window: &TmuxWindow, lines: usize) -> Vec<String> {
    vec![
        "capture-pane".to_string(),
        "-t".to_string(),
        window.0.clone(),
        "-p".to_string(),
        "-S".to_string(),
        format!("-{lines}"),
    ]
}

#[cfg(test)]
fn window_activity_args(window: &TmuxWindow) -> Vec<String> {
    vec![
        "display-message".to_string(),
        "-p".to_string(),
        "-t".to_string(),
        window.0.clone(),
        "#{window_activity}".to_string(),
    ]
}

#[cfg(test)]
fn set_after_split_hook_args(window: &TmuxWindow, working_dir: &str) -> Vec<String> {
    vec![
        "set-hook".to_string(),
        "-w".to_string(),
        "-t".to_string(),
        window.0.clone(),
        "after-split-window".to_string(),
        format!("send-keys 'cd {}' Enter", working_dir),
    ]
}

#[cfg(test)]
fn current_window_name_args() -> Vec<String> {
    vec!["display-message".to_string(), "-p".to_string(), "#W".to_string()]
}

#[cfg(test)]
fn rename_window_args(target: &str, new_name: &str) -> Vec<String> {
    vec![
        "rename-window".to_string(),
        "-t".to_string(),
        target.to_string(),
        new_name.to_string(),
    ]
}

#[cfg(test)]
fn bind_key_args(key: &str, command: &str) -> Vec<String> {
    vec!["bind-key".to_string(), key.to_string(), command.to_string()]
}

#[cfg(test)]
fn unbind_key_args(key: &str) -> Vec<String> {
    vec!["unbind-key".to_string(), key.to_string()]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_window_args_correct() {
        let args = new_window_args(&TmuxWindow("task-42".into()), "/some/path");
        assert_eq!(
            args,
            vec!["new-window", "-d", "-n", "task-42", "-c", "/some/path"]
        );
    }

    #[test]
    fn capture_pane_args_correct() {
        let args = capture_pane_args(&TmuxWindow("task-42".into()), 5);
        assert_eq!(
            args,
            vec!["capture-pane", "-t", "task-42", "-p", "-S", "-5"]
        );
    }

    #[test]
    fn capture_pane_args_different_line_count() {
        let args = capture_pane_args(&TmuxWindow("my-window".into()), 100);
        assert_eq!(args[5], "-100");
    }

    #[test]
    fn has_window_finds_match_in_output() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"main\ntask-42\nother-window\n"),
        ]);
        let result = has_window(&TmuxWindow("task-42".into()), &mock).unwrap();
        assert!(result);
    }

    #[test]
    fn has_window_no_match() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"main\nother-window\n"),
        ]);
        let result = has_window(&TmuxWindow("task-42".into()), &mock).unwrap();
        assert!(!result);
    }

    #[test]
    fn has_window_exact_match_not_prefix() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"task-42\n"),
        ]);
        let result = has_window(&TmuxWindow("task-4".into()), &mock).unwrap();
        assert!(!result);
    }

    #[test]
    fn select_window_args_correct() {
        let args = select_window_args(&TmuxWindow("task-42".into()));
        assert_eq!(args, vec!["select-window", "-t", "task-42"]);
    }

    // --- ProcessRunner-based tests ---

    use crate::process::MockProcessRunner;

    #[test]
    fn new_window_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        new_window(&TmuxWindow("task-42".into()), "/some/path", &mock).unwrap();
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
        let result = capture_pane(&TmuxWindow("task-42".into()), 5, &mock).unwrap();
        assert_eq!(result, "hello from tmux");
    }

    #[test]
    fn has_window_returns_false_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no sessions")]);
        let result = has_window(&TmuxWindow("task-42".into()), &mock).unwrap();
        assert!(!result);
    }

    #[test]
    fn window_activity_args_correct() {
        let args = window_activity_args(&TmuxWindow("task-42".into()));
        assert_eq!(
            args,
            vec!["display-message", "-p", "-t", "task-42", "#{window_activity}"]
        );
    }

    #[test]
    fn window_activity_parses_timestamp() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"1711700000\n"),
        ]);
        let result = window_activity(&TmuxWindow("task-42".into()), &mock).unwrap();
        assert_eq!(result, 1711700000);
    }

    #[test]
    fn window_activity_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no window")]);
        assert!(window_activity(&TmuxWindow("task-42".into()), &mock).is_err());
    }

    #[test]
    fn set_after_split_hook_args_correct() {
        let args = set_after_split_hook_args(&TmuxWindow("task-42".into()), "/some/path");
        assert_eq!(
            args,
            vec![
                "set-hook", "-w", "-t", "task-42",
                "after-split-window", "send-keys 'cd /some/path' Enter",
            ]
        );
    }

    #[test]
    fn set_after_split_hook_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        set_after_split_hook(&TmuxWindow("task-42".into()), "/some/path", &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(
            calls[0].1,
            vec![
                "set-hook", "-w", "-t", "task-42",
                "after-split-window", "send-keys 'cd /some/path' Enter",
            ]
        );
    }

    #[test]
    fn current_window_name_args_correct() {
        let args = current_window_name_args();
        assert_eq!(args, vec!["display-message", "-p", "#W"]);
    }

    #[test]
    fn current_window_name_returns_trimmed_stdout() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"dispatch\n"),
        ]);
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
    fn rename_window_args_correct() {
        let args = rename_window_args("dispatch", "my-old-name");
        assert_eq!(args, vec!["rename-window", "-t", "dispatch", "my-old-name"]);
    }

    #[test]
    fn rename_window_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        rename_window("dispatch", "my-old-name", &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(calls[0].1, vec!["rename-window", "-t", "dispatch", "my-old-name"]);
    }

    #[test]
    fn rename_window_fails_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no window")]);
        assert!(rename_window("dispatch", "other", &mock).is_err());
    }

    #[test]
    fn bind_key_args_correct() {
        let args = bind_key_args("g", "select-window -t dispatch");
        assert_eq!(args, vec!["bind-key", "g", "select-window -t dispatch"]);
    }

    #[test]
    fn bind_key_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        bind_key("g", "select-window -t dispatch", &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(calls[0].1, vec!["bind-key", "g", "select-window -t dispatch"]);
    }

    #[test]
    fn unbind_key_args_correct() {
        let args = unbind_key_args("g");
        assert_eq!(args, vec!["unbind-key", "g"]);
    }

    #[test]
    fn unbind_key_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        unbind_key("g", &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(calls[0].1, vec!["unbind-key", "g"]);
    }
}
