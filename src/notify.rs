//! Shared notification delivery: writes a message file into a task's
//! worktree and injects a tmux nudge pointing at it, only once a pane-content
//! probe confirms the target is idle at its own chat input. Used by
//! task-watcher completion/deletion notices — agent-initiated messaging goes
//! through Claude Code's native `SendMessage` tool instead (task #4098; see
//! `docs/specs/mcp-task-tools.allium`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::process::ProcessRunner;

/// Process-wide counter appended to message filenames so two `deliver()`
/// calls landing in the same millisecond (concurrent watcher notifications)
/// don't collide and silently overwrite each other.
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

/// What happened when [`notify_tmux`] tried to deliver a nudge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryOutcome {
    /// The nudge was typed into the target pane.
    Notified,
    /// The message file was written, but the nudge was withheld because the
    /// target pane was not showing its normal ready-for-input view (a
    /// permission prompt, plan-mode/elicitation dialog, pager, or similar) —
    /// injecting keystrokes there would answer that surface instead of being
    /// read as a message. The file is kept; it's queued, not failed.
    QueuedNoNudge,
}

/// Substring Claude Code renders at the end of its status line only when it
/// is idle at its own chat input and not showing any modal — confirmed
/// across auto/plan-mode states. A permission prompt, an elicitation/plan
/// dialog, or a pager replaces that entire footer region instead, so this
/// string's absence is used as a conservative "don't inject keystrokes here"
/// signal. See the reproduction in
/// `docs/superpowers/specs/2026-08-15-send-message-delivery-hardening-design.md`:
/// injecting the exact nudge text this module sends into a real plan-mode
/// dialog silently answered a multiple-choice question instead of being read.
const READY_FOR_INPUT_MARKER: &str = "shift+tab to cycle";

/// Whether `captured` (a `tmux capture-pane -p` snapshot) shows Claude Code
/// idle at its own chat input, based on the last non-blank line.
fn pane_shows_ready_for_input(captured: &str) -> bool {
    captured
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .is_some_and(|line| line.contains(READY_FOR_INPUT_MARKER))
}

/// Injects `text` into the given tmux window, unless the pane isn't showing
/// its normal ready-for-input view (see [`pane_shows_ready_for_input`]), in
/// which case the nudge is withheld. On a hard failure (the window itself
/// can't be resolved, or the capture fails), best-effort removes the message
/// file at `<worktree>/.claude-messages/<filename>` before returning the
/// error, so a failed delivery doesn't leave an orphaned file behind. A
/// withheld nudge is not a failure — the file is kept.
pub fn notify_tmux(
    runner: &dyn ProcessRunner,
    worktree: &str,
    tmux_window: &str,
    filename: &str,
    text: &str,
) -> Result<DeliveryOutcome, String> {
    let captured = match crate::tmux::capture_pane(tmux_window, runner) {
        Ok(captured) => captured,
        Err(e) => {
            let _ = std::fs::remove_file(format!("{worktree}/.claude-messages/{filename}"));
            return Err(format!("failed to send notification to target agent: {e}"));
        }
    };
    if !pane_shows_ready_for_input(&captured) {
        return Ok(DeliveryOutcome::QueuedNoNudge);
    }
    if let Err(e) = crate::tmux::send_keys(tmux_window, text, runner) {
        let _ = std::fs::remove_file(format!("{worktree}/.claude-messages/{filename}"));
        return Err(format!("failed to send notification to target agent: {e}"));
    }
    Ok(DeliveryOutcome::Notified)
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
) -> Result<DeliveryOutcome, String> {
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

    /// A capture-pane snapshot representing Claude Code idle at its own chat
    /// input — the "safe to inject keystrokes" case.
    const READY_PANE: &[u8] =
        b"> \n---\n  [Sonnet 5] /tmp/x (main)\n  auto mode on (shift+tab to cycle) - 1 agent\n";

    /// A capture-pane snapshot representing a plan-mode/elicitation dialog —
    /// the exact shape reproduced in the design doc. No "shift+tab to cycle"
    /// anywhere.
    const DIALOG_PANE: &[u8] =
        b"> 1. MIT\n  2. Apache 2.0\n\nEnter to select - up/down to navigate - Esc to cancel\n";

    #[test]
    fn notify_tmux_sends_two_keys_when_pane_is_ready() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path().to_str().unwrap();
        let filename = write_message_file(worktree, "prefix", "body").unwrap();

        let runner = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(READY_PANE), // capture-pane
            MockProcessRunner::ok(),                       // send-keys -l
            MockProcessRunner::ok(),                       // send-keys Enter
        ]);

        let outcome = notify_tmux(&runner, worktree, "task-1", &filename, "notify text").unwrap();

        assert_eq!(outcome, DeliveryOutcome::Notified);
        let calls = runner.recorded_calls();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[1].1[0], "send-keys");
        assert_eq!(calls[2].1[0], "send-keys");
    }

    #[test]
    fn notify_tmux_withholds_the_nudge_when_pane_shows_a_dialog() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path().to_str().unwrap();
        let filename = write_message_file(worktree, "prefix", "body").unwrap();
        let path = format!("{worktree}/.claude-messages/{filename}");

        let runner = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(DIALOG_PANE), // capture-pane
        ]);

        let outcome = notify_tmux(&runner, worktree, "task-1", &filename, "notify text").unwrap();

        assert_eq!(outcome, DeliveryOutcome::QueuedNoNudge);
        let calls = runner.recorded_calls();
        assert_eq!(
            calls.len(),
            1,
            "no send-keys call should be issued when the pane isn't ready"
        );
        assert!(
            std::path::Path::new(&path).exists(),
            "message file must be kept — it's queued, not failed"
        );
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

    #[test]
    fn pane_shows_ready_for_input_reads_the_last_non_blank_line() {
        assert!(pane_shows_ready_for_input(
            "some old output\n  auto mode on (shift+tab to cycle) · 1 agent\n"
        ));
        assert!(!pane_shows_ready_for_input(
            "Enter to select · ↑/↓ to navigate · Esc to cancel"
        ));
        assert!(!pane_shows_ready_for_input(""));
        assert!(!pane_shows_ready_for_input("\n\n   \n"));
        // The marker appearing earlier in scrollback, but NOT on the last
        // non-blank line, must not count — a modal fully replaces that region.
        assert!(!pane_shows_ready_for_input(
            "shift+tab to cycle\n\nEnter to select · Esc to cancel\n"
        ));
    }

    #[tokio::test]
    async fn deliver_writes_file_and_notifies_using_its_filename() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path().to_str().unwrap().to_string();
        let runner: Arc<dyn crate::process::ProcessRunner> =
            Arc::new(MockProcessRunner::new(vec![
                MockProcessRunner::ok_with_stdout(READY_PANE),
                MockProcessRunner::ok(),
                MockProcessRunner::ok(),
            ]));

        let outcome = deliver(
            runner,
            worktree.clone(),
            "task-1".to_string(),
            "prefix".to_string(),
            "body".to_string(),
            |filename| format!("see {filename}"),
        )
        .await
        .unwrap();

        assert_eq!(outcome, DeliveryOutcome::Notified);
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
