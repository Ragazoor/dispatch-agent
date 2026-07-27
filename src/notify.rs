//! Shared notification delivery: writes a message file into a task's
//! worktree and injects a tmux nudge pointing at it. Used by both the
//! `send_message` MCP tool and task-watcher completion/deletion notices.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::process::ProcessRunner;

/// Process-wide counter appended to message filenames so two `deliver()`
/// calls landing in the same millisecond (concurrent `send_message` calls,
/// or a watcher completion racing a send) don't collide and silently
/// overwrite each other.
static NEXT_MESSAGE_ID: AtomicU64 = AtomicU64::new(0);

/// Writes `body` to a uniquely-named markdown file under
/// `<worktree>/.claude-messages/` and returns the filename (not the full
/// path) so callers can reference it in a tmux notification.
pub fn write_message_file(worktree: &str, file_prefix: &str, body: &str) -> Result<String, String> {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    write_message_file_at(worktree, file_prefix, body, timestamp)
}

fn write_message_file_at(
    worktree: &str,
    file_prefix: &str,
    body: &str,
    timestamp: u128,
) -> Result<String, String> {
    let messages_dir = format!("{worktree}/.claude-messages");
    std::fs::create_dir_all(&messages_dir)
        .map_err(|e| format!("failed to create messages dir: {e}"))?;
    let counter = NEXT_MESSAGE_ID.fetch_add(1, Ordering::Relaxed);
    let filename = format!("{file_prefix}-{timestamp}-{counter}.md");
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

/// Writes `body` to a message file and injects a tmux nudge pointing at it,
/// wrapping both blocking calls in a single `spawn_blocking`. `notification_text`
/// builds the tmux nudge from the filename `write_message_file` produced, since
/// the text can't be known until the file is written. Folds a `spawn_blocking`
/// panic into the same `Result` as a write/tmux failure, so callers get one
/// error type regardless of which step failed.
pub async fn deliver(
    runner: Arc<dyn ProcessRunner>,
    worktree: String,
    tmux_window: String,
    file_prefix: String,
    body: String,
    notification_text: impl FnOnce(&str) -> String + Send + 'static,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let filename = write_message_file(&worktree, &file_prefix, &body)?;
        let text = notification_text(&filename);
        notify_tmux(&*runner, &worktree, &tmux_window, &filename, &text)
    })
    .await
    .unwrap_or_else(|e| Err(format!("notification task panicked: {e}")))
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
    fn write_message_file_at_produces_unique_filenames_for_the_same_timestamp() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path().to_str().unwrap();

        let first = write_message_file_at(worktree, "prefix", "a", 1_000).unwrap();
        let second = write_message_file_at(worktree, "prefix", "b", 1_000).unwrap();

        assert_ne!(
            first, second,
            "two calls landing in the same millisecond must not collide on filename"
        );
        let content_a =
            std::fs::read_to_string(format!("{worktree}/.claude-messages/{first}")).unwrap();
        let content_b =
            std::fs::read_to_string(format!("{worktree}/.claude-messages/{second}")).unwrap();
        assert_eq!(content_a, "a");
        assert_eq!(content_b, "b");
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

    #[tokio::test]
    async fn deliver_writes_file_and_notifies_using_its_filename() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path().to_str().unwrap().to_string();
        let runner: Arc<dyn crate::process::ProcessRunner> =
            Arc::new(MockProcessRunner::new(vec![
                MockProcessRunner::ok(),
                MockProcessRunner::ok(),
            ]));

        deliver(
            runner,
            worktree.clone(),
            "task-1".to_string(),
            "prefix".to_string(),
            "body".to_string(),
            |filename| format!("see {filename}"),
        )
        .await
        .unwrap();

        let messages_dir = format!("{worktree}/.claude-messages");
        let entries: Vec<_> = std::fs::read_dir(&messages_dir).unwrap().collect();
        assert_eq!(entries.len(), 1);
    }

    #[tokio::test]
    async fn deliver_maps_spawn_blocking_panic_to_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path().to_str().unwrap().to_string();
        let runner: Arc<dyn crate::process::ProcessRunner> =
            Arc::new(MockProcessRunner::new(vec![]));

        let err = deliver(
            runner,
            worktree,
            "task-1".to_string(),
            "prefix".to_string(),
            "body".to_string(),
            |_filename| panic!("boom"),
        )
        .await
        .unwrap_err();

        assert!(err.contains("notification task panicked"));
    }
}
