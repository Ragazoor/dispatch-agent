use anyhow::{Context, Result, bail};
use std::process::Command;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new tmux window with the given name, starting in `working_dir`.
pub fn new_window(name: &str, working_dir: &str) -> Result<()> {
    let args = new_window_args(name, working_dir);
    let status = Command::new("tmux")
        .args(&args)
        .status()
        .context("failed to spawn tmux new-window")?;
    if !status.success() {
        bail!("tmux new-window failed with status {}", status);
    }
    Ok(())
}

/// Send literal text to a tmux window, then press Enter.
///
/// Uses `-l` to prevent tmux from interpreting escape sequences in the text.
/// Enter is sent as a separate `send-keys` call without `-l`.
pub fn send_keys(window: &str, keys: &str) -> Result<()> {
    // Send the text literally (no tmux escape sequence interpretation)
    let status = Command::new("tmux")
        .args(["send-keys", "-t", window, "-l", keys])
        .status()
        .context("failed to spawn tmux send-keys (literal)")?;
    if !status.success() {
        bail!("tmux send-keys -l failed with status {}", status);
    }
    // Press Enter
    let status = Command::new("tmux")
        .args(["send-keys", "-t", window, "Enter"])
        .status()
        .context("failed to spawn tmux send-keys (Enter)")?;
    if !status.success() {
        bail!("tmux send-keys Enter failed with status {}", status);
    }
    Ok(())
}

/// Capture the last `lines` lines of output from a tmux pane, returned trimmed.
pub fn capture_pane(window: &str, lines: usize) -> Result<String> {
    let args = capture_pane_args(window, lines);
    let output = Command::new("tmux")
        .args(&args)
        .output()
        .context("failed to spawn tmux capture-pane")?;
    if !output.status.success() {
        bail!("tmux capture-pane failed with status {}", output.status);
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(text)
}

/// Return true if a tmux window with the given name currently exists.
pub fn has_window(window: &str) -> Result<bool> {
    let output = Command::new("tmux")
        .args(["list-windows", "-F", "#{window_name}"])
        .output()
        .context("failed to spawn tmux list-windows")?;
    // list-windows exits non-zero when there are no windows / no session;
    // treat that as "window not found" rather than a hard error.
    if !output.status.success() {
        return Ok(false);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text.lines().any(|line| line.trim() == window))
}

/// Kill the tmux window with the given name.
pub fn kill_window(window: &str) -> Result<()> {
    let status = Command::new("tmux")
        .args(["kill-window", "-t", window])
        .status()
        .context("failed to spawn tmux kill-window")?;
    if !status.success() {
        bail!("tmux kill-window failed with status {}", status);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Internal helpers (exposed for unit testing without requiring tmux)
// ---------------------------------------------------------------------------

fn new_window_args(name: &str, working_dir: &str) -> Vec<String> {
    vec![
        "new-window".to_string(),
        "-n".to_string(),
        name.to_string(),
        "-c".to_string(),
        working_dir.to_string(),
    ]
}

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
            vec!["new-window", "-n", "task-42", "-c", "/some/path"]
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
        // Simulate what has_window checks against real tmux output lines.
        let fake_output = "main\ntask-42\nother-window\n";
        let target = "task-42";
        let found = fake_output.lines().any(|line| line.trim() == target);
        assert!(found);
    }

    #[test]
    fn has_window_no_match() {
        let fake_output = "main\nother-window\n";
        let target = "task-42";
        let found = fake_output.lines().any(|line| line.trim() == target);
        assert!(!found);
    }

    #[test]
    fn has_window_exact_match_not_prefix() {
        // "task-4" must not match "task-42"
        let fake_output = "task-42\n";
        let target = "task-4";
        let found = fake_output.lines().any(|line| line.trim() == target);
        assert!(!found);
    }
}
