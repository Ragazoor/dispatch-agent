#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Integration test: visible dispatching status across the full
//! mark → in-flight → resolved lifecycle. This test exercises the public
//! `App` API (`status_message`, `is_dispatching`) so a regression that
//! breaks the user-facing feedback contract is caught even if the
//! TUI-internal helpers are refactored.

use dispatch_tui::models::{SubStatus, Task, TaskId, TaskStatus};
use dispatch_tui::tui::{App, Message};

fn make_task(id: i64, title: &str) -> Task {
    let now = chrono::Utc::now();
    Task {
        id: TaskId(id),
        title: title.to_string(),
        description: String::new(),
        repo_path: "/repo".to_string(),
        status: TaskStatus::Backlog,
        worktree: None,
        tmux_window: None,
        plan_path: None,
        epic_id: None,
        sub_status: SubStatus::default_for(TaskStatus::Backlog),
        url: None,
        tag: None,
        sort_order: None,
        base_branch: "main".into(),
        external_id: None,
        labels: Vec::new(),
        created_at: now,
        updated_at: now,
        last_pre_tool_use_at: None,
        last_notification_at: None,
        last_peer_message_sent_at: None,
        last_peer_message_received_at: None,
        wrap_up_mode: None,
        auto_run_plan: false,
        phoenix: false,
        live_subagents: 0,
        stop_pending: false,
        live_shells: 0,
        oldest_live_shell_started_at: None,
    }
}

fn make_app(task: Task) -> App {
    App::new(vec![task])
}

#[tokio::test]
async fn dispatching_status_visible_across_lifecycle_success() {
    let mut app = make_app(make_task(7, "Fix login bug"));

    // 1. Pre-mark: no status, not dispatching.
    assert!(app.status_message().is_none());
    assert!(!app.is_dispatching(TaskId(7)));

    // 2. Mark dispatching: status mentions task title.
    app.update(Message::Task(
        dispatch_tui::tui::messages::TaskMessage::MarkDispatching(TaskId(7)),
    ));
    assert!(app.is_dispatching(TaskId(7)));
    let msg = app
        .status_message()
        .expect("status set after MarkDispatching");
    assert!(msg.contains("Fix login bug"), "got: {msg}");
    assert!(msg.contains("Dispatching"), "got: {msg}");

    // 3. Tick mid-flight: status persists (sticky).
    app.update(Message::System(
        dispatch_tui::tui::messages::SystemMessage::Tick,
    ));
    let msg = app
        .status_message()
        .expect("sticky status survives Tick during dispatch");
    assert!(msg.contains("Fix login bug"));

    // 4. Dispatched: status clears, set drains.
    app.update(Message::Task(
        dispatch_tui::tui::messages::TaskMessage::Dispatched {
            id: TaskId(7),
            worktree: "/repo/.worktrees/7-fix-login-bug".to_string(),
            tmux_window: "task-7".to_string(),
            switch_focus: false,
        },
    ));
    assert!(!app.is_dispatching(TaskId(7)));
    assert!(
        app.status_message().is_none(),
        "status should clear when dispatch resolves"
    );
}

#[tokio::test]
async fn dispatching_status_visible_across_lifecycle_failure() {
    let mut app = make_app(make_task(8, "Refactor module"));

    app.update(Message::Task(
        dispatch_tui::tui::messages::TaskMessage::MarkDispatching(TaskId(8)),
    ));
    assert!(app.is_dispatching(TaskId(8)));
    assert!(app.status_message().is_some());

    app.update(Message::Task(
        dispatch_tui::tui::messages::TaskMessage::DispatchFailed(TaskId(8)),
    ));
    assert!(!app.is_dispatching(TaskId(8)));
    assert!(
        app.status_message().is_none(),
        "status should clear when dispatch fails"
    );
}

#[tokio::test]
async fn multiple_dispatches_show_pluralized_status() {
    let mut app = App::new(vec![make_task(1, "Task A"), make_task(2, "Task B")]);

    app.update(Message::Task(
        dispatch_tui::tui::messages::TaskMessage::MarkDispatching(TaskId(1)),
    ));
    app.update(Message::Task(
        dispatch_tui::tui::messages::TaskMessage::MarkDispatching(TaskId(2)),
    ));

    let msg = app.status_message().expect("status set");
    assert!(msg.contains("2 tasks"), "expected plural form, got: {msg}");

    // Resolving one transitions back to the singular form.
    app.update(Message::Task(
        dispatch_tui::tui::messages::TaskMessage::Dispatched {
            id: TaskId(1),
            worktree: "/wt/1".to_string(),
            tmux_window: "task-1".to_string(),
            switch_focus: false,
        },
    ));
    let msg = app.status_message().expect("status still set");
    assert!(
        msg.contains("Task B"),
        "expected singular form referencing remaining task, got: {msg}"
    );
}
