//! The `task-<id>` window-name convention, as a pure pair of string functions.
//!
//! The same `task-<id>` string names two things: the tmux window a dispatched
//! agent runs in, and that agent's native cross-session-messaging session
//! (`dispatch::agents::session_name_flag`). Both the dispatch adapter and
//! `TaskService::record_peer_message_sent` need to build and parse it, so the
//! convention lives here in the domain model rather than in either consumer —
//! a service reaching into the adapter for a pure predicate inverts the
//! layering, and a second copy of `strip_prefix("task-")` could drift from
//! this one.

use super::TaskId;

/// The tmux window / messaging-session name for a task.
pub fn build_tmux_window_name(task_id: TaskId) -> String {
    format!("task-{task_id}")
}

/// Inverse of [`build_tmux_window_name`]: recover the task id from a tmux
/// window name, or `None` for any window that isn't a task-agent window
/// (the board's own TUI window, the main-session window, anything else).
pub fn parse_tmux_window_task_id(window: &str) -> Option<TaskId> {
    window.strip_prefix("task-")?.parse().ok()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn build_tmux_window_name_uses_task_prefix() {
        assert_eq!(build_tmux_window_name(TaskId(42)), "task-42");
    }

    #[test]
    fn parse_tmux_window_task_id_roundtrips_with_build_tmux_window_name() {
        let name = build_tmux_window_name(TaskId(42));
        assert_eq!(parse_tmux_window_task_id(&name), Some(TaskId(42)));
    }

    #[test]
    fn parse_tmux_window_task_id_rejects_non_task_windows() {
        assert_eq!(parse_tmux_window_task_id("TUI"), None);
        assert_eq!(parse_tmux_window_task_id("dispatch-main"), None);
        assert_eq!(parse_tmux_window_task_id("task-"), None);
        assert_eq!(parse_tmux_window_task_id("task-abc"), None);
    }
}
