#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Integration test: end-to-end hook-event flow through `TaskService`.

use std::sync::Arc;

use dispatch_tui::db::Database;
use dispatch_tui::models::{HookEventKind, SubStatus, TaskStatus};
use dispatch_tui::service::{CreateTaskParams, FixedClock, TaskService, UpdateTaskParams};

#[tokio::test]
async fn hook_event_flow_drives_sub_status_and_lifecycle() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    // Inject a manually-advanced clock so hook-event timestamps land in distinct
    // seconds deterministically — no wall-clock sleeps. Timestamps persist at
    // one-second resolution, so each step below advances the clock by ≥1s.
    let clock = FixedClock::new(
        "2026-01-01T00:00:00Z"
            .parse::<chrono::DateTime<chrono::Utc>>()
            .unwrap(),
    );
    let svc = TaskService::new(db, dispatch_tui::process::MockProcessRunner::unused())
        .with_clock(Arc::new(clock.clone()));

    let id = svc
        .create_task(CreateTaskParams {
            title: "active health".into(),
            description: "".into(),
            repo_path: "/repo".into(),
            plan_path: None,
            epic_id: None,
            sort_order: None,
            tag: None,
            base_branch: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    // Running is the whole precondition: `record_hook_event` branches on
    // `status` and the timestamp fields only, never on worktree/tmux_window.
    // How the task reached Running is not what this test covers.
    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Running))
        .await
        .unwrap();

    let t = svc.get_task(id).await.unwrap();
    assert_eq!(t.status, TaskStatus::Running);

    svc.record_hook_event(id, HookEventKind::PreToolUse)
        .await
        .unwrap();
    let t = svc.get_task(id).await.unwrap();
    assert_eq!(t.sub_status, SubStatus::Active);
    assert!(t.last_pre_tool_use_at.is_some());

    // Advance ≥1s so the next event records a strictly later timestamp.
    clock.advance(chrono::Duration::seconds(2));

    svc.record_hook_event(id, HookEventKind::Notification(None))
        .await
        .unwrap();
    let t = svc.get_task(id).await.unwrap();
    assert_eq!(t.sub_status, SubStatus::NeedsInput);
    assert!(t.last_notification_at.is_some());

    clock.advance(chrono::Duration::seconds(2));

    svc.record_hook_event(id, HookEventKind::PreToolUse)
        .await
        .unwrap();
    let t = svc.get_task(id).await.unwrap();
    assert_eq!(t.sub_status, SubStatus::Active);
    let pre = t.last_pre_tool_use_at.unwrap();
    let notif = t.last_notification_at.unwrap();
    assert!(
        pre > notif,
        "PreToolUse {pre} should be newer than Notification {notif}"
    );

    svc.record_hook_event(id, HookEventKind::Stop)
        .await
        .unwrap();
    let t = svc.get_task(id).await.unwrap();
    assert_eq!(t.status, TaskStatus::Review);
    assert_eq!(t.sub_status, SubStatus::default_for(TaskStatus::Review));
    assert!(t.last_pre_tool_use_at.is_none());
    assert!(t.last_notification_at.is_none());
}
