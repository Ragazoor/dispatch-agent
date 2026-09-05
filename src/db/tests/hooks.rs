#![allow(clippy::unwrap_used, clippy::expect_used)]
//! `record_notification` / `record_pre_tool_use` — the two Claude Code hook
//! writes that carry their own guard.
//!
//! Both apply as a single conditional UPDATE rather than a snapshot read
//! followed by a patch. Every hook is its own OS process, so a value read
//! beforehand can already be stale by the time the write lands; these tests
//! exercise exactly that ordering. The sibling hook writes with the same shape
//! are covered in [`super::subagents`] (`try_record_stop`) — they live there
//! because their fixture is a live subagent.
//!
//! See `HookNotification` and `HookPreToolUse` in
//! `docs/specs/agent-health.allium`.
use super::*;
use crate::models::{NotificationKind, NotificationWrite};
use chrono::Utc;

/// A plain Running task with no activity stamps.
///
/// Deliberately not `subagents::set_running`, which also stamps
/// `last_pre_tool_use_at` and `last_notification_at` — every assertion below
/// turns on one of those still being null when the write under test runs.
async fn running_task(db: &Database) -> Task {
    let task = make_task(db, "t").await;
    db.patch_task(
        task.id,
        &crate::db::TaskPatch::new().status(TaskStatus::Running),
    )
    .await
    .unwrap();
    task
}

#[tokio::test]
async fn record_notification_suppresses_a_raise_for_a_shell_started_after_the_kind_was_resolved() {
    let db = in_memory_db().await;
    let task = running_task(&db).await;

    // Resolved first, exactly as the service does — from the kind, with no
    // knowledge of this task's counters.
    let write = NotificationWrite::from_kind(Some(NotificationKind::IdlePrompt));
    // ...then the shell appears, standing in for a concurrent hook process.
    db.shell_start(task.id, "bash_1", "s1", Utc::now())
        .await
        .unwrap();

    db.record_notification(task.id, write, Utc::now())
        .await
        .unwrap();

    let reread = db.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(
        reread.sub_status,
        SubStatus::Active,
        "the shell was live at write time, so the idle notification must not raise"
    );
    assert!(
        reread.last_notification_at.is_none(),
        "declining to stamp is what stops ClassifyAgentActivity re-pinning needs_input each tick"
    );
}

#[tokio::test]
async fn record_notification_raises_for_a_shell_drained_after_the_kind_was_resolved() {
    let db = in_memory_db().await;
    let task = running_task(&db).await;
    db.shell_start(task.id, "bash_1", "s1", Utc::now())
        .await
        .unwrap();

    let write = NotificationWrite::from_kind(Some(NotificationKind::IdlePrompt));
    // The mirror case: the shell is gone by the time the write lands, so the
    // agent really is waiting on a human and the raise must go through.
    db.shell_stop(task.id, "bash_1", "s1").await.unwrap();

    db.record_notification(task.id, write, Utc::now())
        .await
        .unwrap();

    let reread = db.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(reread.sub_status, SubStatus::NeedsInput);
    assert!(reread.last_notification_at.is_some());
}

#[tokio::test]
async fn record_notification_raises_a_blocking_kind_through_a_live_shell() {
    let db = in_memory_db().await;
    let task = running_task(&db).await;
    db.shell_start(task.id, "bash_1", "s1", Utc::now())
        .await
        .unwrap();

    // Only the idle_prompt raise carries the extra predicate: a permission
    // decision needs a human whatever else the agent has running.
    let write = NotificationWrite::from_kind(Some(NotificationKind::PermissionPrompt));
    db.record_notification(task.id, write, Utc::now())
        .await
        .unwrap();

    let reread = db.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(reread.sub_status, SubStatus::NeedsInput);
}

#[tokio::test]
async fn record_notification_is_a_no_op_on_a_task_that_left_running() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    // Never running: the status predicate must reject the write silently
    // rather than error, because the hook observed a state that has moved on.
    let write = NotificationWrite::from_kind(None);

    db.record_notification(task.id, write, Utc::now())
        .await
        .unwrap();

    let reread = db.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(reread.status, TaskStatus::Backlog);
    assert_eq!(reread.sub_status, SubStatus::None);
    assert!(reread.last_notification_at.is_none());
}

#[tokio::test]
async fn record_pre_tool_use_is_a_no_op_on_a_task_that_left_running() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    // The status guard has to ride in the write for the same reason
    // record_notification's does: a concurrent Stop can flip the row to review
    // between the service's read and this write, and an unconditional patch
    // would then write (review, active) and trip the tasks CHECK constraint.
    db.record_pre_tool_use(task.id, SubStatus::Active, Utc::now())
        .await
        .unwrap();

    let reread = db.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(reread.status, TaskStatus::Backlog);
    assert_eq!(reread.sub_status, SubStatus::None);
    assert!(reread.last_pre_tool_use_at.is_none());
}

#[tokio::test]
async fn record_pre_tool_use_stamps_and_sets_sub_status_on_a_running_task() {
    let db = in_memory_db().await;
    let task = running_task(&db).await;

    db.record_pre_tool_use(task.id, SubStatus::StaleShell, Utc::now())
        .await
        .unwrap();

    let reread = db.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(reread.sub_status, SubStatus::StaleShell);
    assert!(reread.last_pre_tool_use_at.is_some());
}
