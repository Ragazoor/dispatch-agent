// -- watchers ---------------------------------------------------------------
use super::*;
use crate::models::test_tmux_window;
use crate::service::tasks::watchers::SubscribeOutcome;

/// Shared with `src/notify.rs`'s own tests, so the two can't drift into
/// two different "ready pane" fixtures.
use crate::notify::test_fixtures::READY_PANE as READY_PANE_STDOUT;

#[tokio::test]
async fn subscribe_to_task_creates_watch() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let watcher = svc.create_task(make_task_params("/repo")).await.unwrap();
    let target = svc.create_task(make_task_params("/repo")).await.unwrap();

    let outcome = svc.subscribe_to_task(watcher, target).await.unwrap();
    assert!(matches!(outcome, SubscribeOutcome::Subscribed));

    let watchers = db.list_watchers_of(target).await.unwrap();
    assert_eq!(watchers, vec![watcher]);
}

#[tokio::test]
async fn subscribe_to_task_rejects_self_watch() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let task = svc.create_task(make_task_params("/repo")).await.unwrap();

    let err = svc.subscribe_to_task(task, task).await.unwrap_err();
    assert!(matches!(err, ServiceError::Validation(_)));
}

#[tokio::test]
async fn subscribe_to_task_target_not_found() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let watcher = svc.create_task(make_task_params("/repo")).await.unwrap();

    let err = svc
        .subscribe_to_task(watcher, TaskId(999_999))
        .await
        .unwrap_err();
    assert!(matches!(err, ServiceError::NotFound(_)));
}

#[tokio::test]
async fn subscribe_to_task_already_finished_does_not_create_row() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let watcher = svc.create_task(make_task_params("/repo")).await.unwrap();
    let target = svc.create_task(make_task_params("/repo")).await.unwrap();
    db.patch_task(target, &db::TaskPatch::new().status(TaskStatus::Done))
        .await
        .unwrap();

    let outcome = svc.subscribe_to_task(watcher, target).await.unwrap();
    assert!(matches!(
        outcome,
        SubscribeOutcome::AlreadyFinished(TaskStatus::Done)
    ));
    assert!(db.list_watchers_of(target).await.unwrap().is_empty());
}

#[tokio::test]
async fn subscribe_to_task_is_idempotent_when_still_unfinished() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let watcher = svc.create_task(make_task_params("/repo")).await.unwrap();
    let target = svc.create_task(make_task_params("/repo")).await.unwrap();

    svc.subscribe_to_task(watcher, target).await.unwrap();
    let outcome = svc.subscribe_to_task(watcher, target).await.unwrap();
    assert!(matches!(outcome, SubscribeOutcome::Subscribed));
    assert_eq!(db.list_watchers_of(target).await.unwrap(), vec![watcher]);
}

#[tokio::test]
async fn unsubscribe_from_task_removes_watch() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let watcher = svc.create_task(make_task_params("/repo")).await.unwrap();
    let target = svc.create_task(make_task_params("/repo")).await.unwrap();
    svc.subscribe_to_task(watcher, target).await.unwrap();

    svc.unsubscribe_from_task(watcher, target).await.unwrap();

    assert!(db.list_watchers_of(target).await.unwrap().is_empty());
}

#[tokio::test]
async fn unsubscribe_from_task_is_idempotent_when_absent() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let watcher = svc.create_task(make_task_params("/repo")).await.unwrap();
    let target = svc.create_task(make_task_params("/repo")).await.unwrap();

    svc.unsubscribe_from_task(watcher, target).await.unwrap(); // no prior subscribe — must not error
}

#[tokio::test]
async fn update_task_to_done_notifies_live_watcher() {
    let tmp = tempfile::tempdir().unwrap();
    let worktree = tmp.path().to_str().unwrap().to_string();
    let db = test_db().await;
    let mock = Arc::new(crate::process::MockProcessRunner::new(vec![
        crate::process::MockProcessRunner::ok_with_stdout(READY_PANE_STDOUT),
        crate::process::MockProcessRunner::ok(),
        crate::process::MockProcessRunner::ok(),
    ]));
    let runner: Arc<dyn crate::process::ProcessRunner> = mock.clone();
    let svc = task_svc_with_runner(&db, runner);

    let watcher = svc.create_task(make_task_params("/repo")).await.unwrap();
    db.patch_task(
        watcher,
        &db::TaskPatch::new()
            .worktree(Some(&worktree))
            .tmux_window(Some(&test_tmux_window("task-watcher"))),
    )
    .await
    .unwrap();
    let target = svc.create_task(make_task_params("/repo")).await.unwrap();
    svc.subscribe_to_task(watcher, target).await.unwrap();

    svc.update_task(UpdateTaskParams::for_task(target).status(TaskStatus::Done))
        .await
        .unwrap();

    assert_eq!(
        mock.recorded_calls().len(),
        3,
        "expected a capture-pane check plus one tmux notification (2 send-keys calls)"
    );
    assert!(
        db.list_watchers_of(target).await.unwrap().is_empty(),
        "subscription should be cleared after firing"
    );
}

#[tokio::test]
async fn update_task_to_done_is_noop_when_status_unchanged() {
    let db = test_db().await;
    let runner: Arc<dyn crate::process::ProcessRunner> =
        crate::process::MockProcessRunner::unused();
    let svc = task_svc_with_runner(&db, runner);

    let target = svc.create_task(make_task_params("/repo")).await.unwrap();
    db.patch_task(target, &db::TaskPatch::new().status(TaskStatus::Done))
        .await
        .unwrap();

    // Re-setting to Done (same status) must not attempt any notification —
    // the MockProcessRunner has zero queued responses, so any call would panic.
    svc.update_task(UpdateTaskParams::for_task(target).status(TaskStatus::Done))
        .await
        .unwrap();
}

#[tokio::test]
async fn update_task_to_done_logs_and_drops_dead_watcher() {
    let db = test_db().await;
    let runner: Arc<dyn crate::process::ProcessRunner> =
        crate::process::MockProcessRunner::unused();
    let svc = task_svc_with_runner(&db, runner);

    // Watcher has no worktree/tmux_window — still Backlog.
    let watcher = svc.create_task(make_task_params("/repo")).await.unwrap();
    let target = svc.create_task(make_task_params("/repo")).await.unwrap();
    svc.subscribe_to_task(watcher, target).await.unwrap();

    // Must not panic even though the MockProcessRunner has zero queued
    // responses — the dead watcher is dropped before any process call.
    svc.update_task(UpdateTaskParams::for_task(target).status(TaskStatus::Done))
        .await
        .unwrap();

    assert!(db.list_watchers_of(target).await.unwrap().is_empty());
}

#[tokio::test]
async fn delete_task_notifies_watchers_of_deletion() {
    let tmp = tempfile::tempdir().unwrap();
    let worktree = tmp.path().to_str().unwrap().to_string();
    let db = test_db().await;
    let mock = Arc::new(crate::process::MockProcessRunner::new(vec![
        crate::process::MockProcessRunner::ok_with_stdout(READY_PANE_STDOUT),
        crate::process::MockProcessRunner::ok(),
        crate::process::MockProcessRunner::ok(),
    ]));
    let runner: Arc<dyn crate::process::ProcessRunner> = mock.clone();
    let svc = task_svc_with_runner(&db, runner);

    let watcher = svc.create_task(make_task_params("/repo")).await.unwrap();
    db.patch_task(
        watcher,
        &db::TaskPatch::new()
            .worktree(Some(&worktree))
            .tmux_window(Some(&test_tmux_window("task-watcher"))),
    )
    .await
    .unwrap();
    let target = svc.create_task(make_task_params("/repo")).await.unwrap();
    svc.subscribe_to_task(watcher, target).await.unwrap();

    svc.delete_task(target).await.unwrap();

    assert_eq!(mock.recorded_calls().len(), 3);
    assert!(db.list_watchers_of(target).await.unwrap().is_empty());

    // Assert the delivered message body actually says the task was
    // deleted before it finished (not a generic/finished-style body).
    let messages_dir = tmp.path().join(".claude-messages");
    let entries: Vec<_> = std::fs::read_dir(&messages_dir).unwrap().collect();
    assert_eq!(entries.len(), 1, "should have exactly one message file");
    let content = std::fs::read_to_string(entries[0].as_ref().unwrap().path()).unwrap();
    assert!(
        content.contains("deleted"),
        "message should mention deletion: {content}"
    );
    assert!(
        content.contains("before it finished"),
        "message should mention it was deleted before finishing: {content}"
    );
}

#[tokio::test]
async fn delete_task_does_not_notify_watcher_when_target_already_finished_via_bypassed_write() {
    // Characterization test for the previously-masked gap: FeedRunner
    // writes task status directly via its own DB write handle, bypassing
    // TaskService::update_task entirely — so a
    // feed-synced task that auto-completes never gets its watcher rows
    // cleared by notify_watchers_if_finished. If that already-Done task
    // is later deleted through TaskService::delete_task, the watcher
    // must NOT receive a "deleted before it finished" notification — it
    // actually finished successfully.
    //
    // The watcher is given a live worktree/tmux_window (as in
    // delete_task_notifies_watchers_of_deletion) so the buggy,
    // unconditional code path would actually reach the
    // MockProcessRunner. A zero-response MockProcessRunner means any
    // such call panics *inside* the notification's spawn_blocking task —
    // that panic is caught by tokio and only logged (it does not fail
    // this test on its own), so the deterministic assertion is
    // `recorded_calls()` staying empty, not an uncaught panic.
    let tmp = tempfile::tempdir().unwrap();
    let worktree = tmp.path().to_str().unwrap().to_string();
    let db = test_db().await;
    // Concrete type retained: this test asserts on `recorded_calls()`.
    let mock = Arc::new(crate::process::MockProcessRunner::new(vec![]));
    let runner: Arc<dyn crate::process::ProcessRunner> = mock.clone();
    let svc = task_svc_with_runner(&db, runner);

    let watcher = svc.create_task(make_task_params("/repo")).await.unwrap();
    db.patch_task(
        watcher,
        &db::TaskPatch::new()
            .worktree(Some(&worktree))
            .tmux_window(Some(&test_tmux_window("task-watcher"))),
    )
    .await
    .unwrap();
    let target = svc.create_task(make_task_params("/repo")).await.unwrap();
    svc.subscribe_to_task(watcher, target).await.unwrap();

    // Bypass TaskService entirely (simulating FeedRunner's sanctioned
    // direct DB write) so notify_watchers_if_finished never runs and the
    // watcher row is NOT cleared by the finish hook.
    db.patch_task(target, &db::TaskPatch::new().status(TaskStatus::Done))
        .await
        .unwrap();
    assert_eq!(
        db.list_watchers_of(target).await.unwrap(),
        vec![watcher],
        "direct DB write bypasses the finish hook; watcher row survives"
    );

    svc.delete_task(target).await.unwrap();

    // No tmux call was attempted at all.
    assert!(
        mock.recorded_calls().is_empty(),
        "no notification attempt should be made for an already-finished target: {:?}",
        mock.recorded_calls()
    );
    // And the watcher row is still cleaned up on delete regardless.
    assert!(
        db.list_watchers_of(target).await.unwrap().is_empty(),
        "watcher row should still be cleaned up on delete regardless of notification"
    );
}

#[tokio::test]
async fn delete_task_cleans_up_rows_where_it_was_the_watcher() {
    let db = test_db().await;
    let runner: Arc<dyn crate::process::ProcessRunner> =
        crate::process::MockProcessRunner::unused();
    let svc = task_svc_with_runner(&db, runner);

    let watcher = svc.create_task(make_task_params("/repo")).await.unwrap();
    let target = svc.create_task(make_task_params("/repo")).await.unwrap();
    svc.subscribe_to_task(watcher, target).await.unwrap();

    svc.delete_task(watcher).await.unwrap();

    assert!(db.list_watchers_of(target).await.unwrap().is_empty());
}
