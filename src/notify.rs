//! Shared notification delivery: writes a message file into a task's
//! worktree and injects a tmux nudge pointing at it. Used by both the
//! `send_message` MCP tool and task-watcher completion/deletion notices.

use crate::process::ProcessRunner;

/// Writes `body` to a uniquely-named markdown file under
/// `<worktree>/.claude-messages/` and returns the filename (not the full
/// path) so callers can reference it in a tmux notification.
pub fn write_message_file(worktree: &str, file_prefix: &str, body: &str) -> Result<String, String> {
    let messages_dir = format!("{worktree}/.claude-messages");
    std::fs::create_dir_all(&messages_dir)
        .map_err(|e| format!("failed to create messages dir: {e}"))?;
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let filename = format!("{file_prefix}-{timestamp}.md");
    let path = format!("{messages_dir}/{filename}");
    std::fs::write(&path, body).map_err(|e| format!("failed to write message file: {e}"))?;
    Ok(filename)
}

/// Injects `text` into the given tmux window. On failure, best-effort
/// removes the message file at `<worktree>/.claude-messages/<filename>`
/// before returning the error, so a failed delivery doesn't leave an
/// orphaned file behind.
pub fn notify_tmux(
    runner: &dyn ProcessRunner,
    worktree: &str,
    tmux_window: &str,
    filename: &str,
    text: &str,
) -> Result<(), String> {
    if let Err(e) = crate::tmux::send_keys(tmux_window, text, runner) {
        let _ = std::fs::remove_file(format!("{worktree}/.claude-messages/{filename}"));
        return Err(format!("failed to send notification to target agent: {e}"));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::process::MockProcessRunner;

    #[test]
    fn write_message_file_creates_dir_and_file_with_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path().to_str().unwrap();

        let filename = write_message_file(worktree, "watch-finished-42", "hello world").unwrap();

        assert!(filename.starts_with("watch-finished-42-"));
        assert!(filename.ends_with(".md"));
        let content =
            std::fs::read_to_string(format!("{worktree}/.claude-messages/{filename}")).unwrap();
        assert_eq!(content, "hello world");
    }

    #[test]
    fn notify_tmux_sends_two_keys_and_succeeds() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path().to_str().unwrap();
        let filename = write_message_file(worktree, "prefix", "body").unwrap();

        let runner = MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // send-keys -l
            MockProcessRunner::ok(), // send-keys Enter
        ]);

        notify_tmux(&runner, worktree, "task-1", &filename, "notify text").unwrap();

        let calls = runner.recorded_calls();
        assert_eq!(calls.len(), 2);
    }

    #[test]
    fn notify_tmux_removes_file_on_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path().to_str().unwrap();
        let filename = write_message_file(worktree, "prefix", "body").unwrap();
        let path = format!("{worktree}/.claude-messages/{filename}");
        assert!(std::path::Path::new(&path).exists());

        let runner = MockProcessRunner::new(vec![Err(anyhow::anyhow!("tmux not found"))]);

        let err = notify_tmux(&runner, worktree, "task-1", &filename, "notify text").unwrap_err();
        assert!(err.contains("failed to send notification"));
        assert!(
            !std::path::Path::new(&path).exists(),
            "message file should be cleaned up on delivery failure"
        );
    }
}
