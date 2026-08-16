#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use super::*;
use crate::db::{CreateTaskRequest, Database, TaskCrud, TaskRead};
use crate::models::TaskStatus;
use crate::process::MockProcessRunner;

/// A scheduled, pinned task sitting idle in `done` — the steady state of a
/// staging pipeline between promotions.
async fn seed_scheduled_task(db: &Database, last_processed_sha: Option<&str>) -> TaskId {
    let id = db
        .create_task(CreateTaskRequest {
            title: "staging pipeline",
            description: "",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Done,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
            schedule_interval_secs: Some(1),
            pinned_branch: Some("staging"),
        })
        .await
        .unwrap();
    if let Some(sha) = last_processed_sha {
        db.patch_task(id, &TaskPatch::new().last_processed_sha(Some(sha)))
            .await
            .unwrap();
    }
    id
}

fn make_runner(
    db: Arc<Database>,
    mock: Arc<dyn ProcessRunner>,
) -> (SchedulerRunner, mpsc::UnboundedReceiver<McpEvent>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (SchedulerRunner::new(db, tx, mock), rx)
}

/// The whole point of the feature: when `origin/<pinned_branch>` still points at
/// `last_processed_sha`, the tick costs one fetch and one rev-parse — no
/// worktree, no tmux window, no agent.
#[tokio::test]
async fn tick_skips_dispatch_when_pinned_branch_unchanged() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let task_id = seed_scheduled_task(&db, Some("abc123")).await;

    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok(),                        // git fetch origin staging
        MockProcessRunner::ok_with_stdout(b"abc123\n"), // git rev-parse origin/staging
    ]));
    let (mut scheduler, _rx) = make_runner(Arc::clone(&db), mock.clone());

    scheduler.tick().await;
    scheduler.join_spawned_jobs().await;

    let calls = mock.recorded_calls();
    assert_eq!(
        calls.len(),
        2,
        "an unchanged branch costs exactly the fetch and the rev-parse: {calls:?}"
    );
    assert!(
        !calls
            .iter()
            .any(|(prog, args)| prog == "tmux" || args.contains(&"worktree".to_string())),
        "nothing may be provisioned when the branch has not moved: {calls:?}"
    );

    let task = db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Done,
        "a skipped tick must not claim the task"
    );
    assert!(
        task.last_scheduled_check_at.is_some(),
        "a skipped tick still records that the scheduler looked"
    );
}

/// The branch moved, so the tick dispatches: the task is claimed and an agent
/// is launched.
#[tokio::test]
async fn tick_dispatches_when_pinned_branch_changed() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let task_id = seed_scheduled_task(&db, Some("abc123")).await;

    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // git fetch origin staging (probe)
        MockProcessRunner::ok_with_stdout(b"def456\n"), // rev-parse — moved
    ]));
    let (mut scheduler, _rx) = make_runner(Arc::clone(&db), mock.clone());

    scheduler.tick().await;
    scheduler.join_spawned_jobs().await;

    let calls = mock.recorded_calls();
    assert!(
        calls
            .iter()
            .any(|(_, args)| args.contains(&"rev-parse".to_string())),
        "the branch must actually be probed: {calls:?}"
    );

    // The observable difference between skipping and dispatching is the claim.
    // `/repo` does not exist, so `pipeline_agent` bails before running any
    // subprocess and the claim is released — which leaves the task in
    // `backlog`, not the `done` it started in. A skipped tick leaves `done`
    // untouched (see `tick_skips_dispatch_when_pinned_branch_unchanged`).
    let task = db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Backlog,
        "a moved branch must be claimed and dispatched, not skipped"
    );
}

/// First run ever: `last_processed_sha` is null, so there is nothing to compare
/// against and the tick dispatches without probing the branch at all.
#[tokio::test]
async fn tick_dispatches_when_never_processed() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let task_id = seed_scheduled_task(&db, None).await;

    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok(),
        MockProcessRunner::ok(),
        MockProcessRunner::ok(),
        MockProcessRunner::ok(),
        MockProcessRunner::ok(),
        MockProcessRunner::ok(),
    ]));
    let (mut scheduler, _rx) = make_runner(Arc::clone(&db), mock.clone());

    scheduler.tick().await;
    scheduler.join_spawned_jobs().await;

    let calls = mock.recorded_calls();
    assert!(
        !calls
            .iter()
            .any(|(_, args)| args.contains(&"rev-parse".to_string())),
        "with no last_processed_sha there is nothing to compare, so no probe: {calls:?}"
    );
    let task = db.get_task(task_id).await.unwrap().unwrap();
    assert_ne!(
        task.status,
        TaskStatus::Done,
        "the first tick must claim the task"
    );
}

/// Not yet due: the tick must not run a single subprocess for this task — not
/// even the lightweight probe. This is what keeps an idle board quiet.
#[tokio::test]
async fn tick_makes_no_subprocess_calls_when_not_yet_due() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let task_id = seed_scheduled_task(&db, Some("abc123")).await;
    // A long interval with the stamp set to now: nowhere near due.
    db.patch_task(
        task_id,
        &TaskPatch::new()
            .schedule_interval_secs(Some(3600))
            .last_scheduled_check_at(Some(Utc::now())),
    )
    .await
    .unwrap();

    let mock = Arc::new(MockProcessRunner::new(vec![]));
    let (mut scheduler, _rx) = make_runner(Arc::clone(&db), mock.clone());

    scheduler.tick().await;
    scheduler.join_spawned_jobs().await;

    assert!(
        mock.recorded_calls().is_empty(),
        "a task that is not due costs nothing: {:?}",
        mock.recorded_calls()
    );
}

/// An unscheduled task is invisible to the scheduler, however it is otherwise
/// shaped. `list_scheduled_tasks` is the gate, and it must not widen.
#[tokio::test]
async fn tick_ignores_unscheduled_tasks() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    db.create_task(CreateTaskRequest {
        title: "ordinary",
        description: "",
        repo_path: "/repo",
        plan: None,
        status: TaskStatus::Backlog,
        base_branch: "main",
        epic_id: None,
        sort_order: None,
        tag: None,
        wrap_up_mode: None,
        auto_run_plan: false,
        schedule_interval_secs: None,
        pinned_branch: None,
    })
    .await
    .unwrap();

    let mock = Arc::new(MockProcessRunner::new(vec![]));
    let (mut scheduler, _rx) = make_runner(Arc::clone(&db), mock.clone());

    scheduler.tick().await;
    scheduler.join_spawned_jobs().await;

    assert!(mock.recorded_calls().is_empty());
}

/// `last_scheduled_check_at` is stamped on *every* look, not only the ones
/// that reach a decision. The persisted stamp is the cold-start gate, so a
/// path that skips it leaves the task reading as overdue after a restart.
///
/// Covers the outcomes reachable without faking a DB error: the skip, and a
/// dispatch that failed to provision.
#[tokio::test]
async fn every_outcome_stamps_last_scheduled_check_at() {
    // Skip: branch unchanged.
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let skipped = seed_scheduled_task(&db, Some("abc123")).await;
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok(),
        MockProcessRunner::ok_with_stdout(b"abc123\n"),
    ]));
    let (mut scheduler, _rx) = make_runner(Arc::clone(&db), mock);
    scheduler.tick().await;
    scheduler.join_spawned_jobs().await;
    assert!(
        db.get_task(skipped)
            .await
            .unwrap()
            .unwrap()
            .last_scheduled_check_at
            .is_some(),
        "the skip path must stamp"
    );

    // Dispatch that failed to provision (`/repo` does not exist).
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let failed = seed_scheduled_task(&db, None).await;
    let mock = Arc::new(MockProcessRunner::new(vec![]));
    let (mut scheduler, _rx) = make_runner(Arc::clone(&db), mock);
    scheduler.tick().await;
    scheduler.join_spawned_jobs().await;
    assert!(
        db.get_task(failed)
            .await
            .unwrap()
            .unwrap()
            .last_scheduled_check_at
            .is_some(),
        "a failed dispatch must stamp too"
    );
}

/// A task with a live agent is not idle, so it is not eligible — otherwise
/// every tick would pile a second agent onto a running one.
#[tokio::test]
async fn list_scheduled_tasks_excludes_a_task_with_a_live_window() {
    let db = Database::open_in_memory().await.unwrap();
    let id = seed_scheduled_task(&db, None).await;
    assert_eq!(db.list_scheduled_tasks().await.unwrap().len(), 1);

    db.patch_task(id, &TaskPatch::new().tmux_window(Some("task-1")))
        .await
        .unwrap();
    assert!(
        db.list_scheduled_tasks().await.unwrap().is_empty(),
        "a task with a live tmux window is not idle"
    );
}

/// The claim is the atomic enforcement point for `DispatchScheduledTask`'s
/// precondition: the second of two racing ticks must lose.
#[tokio::test]
async fn try_claim_scheduled_task_is_exclusive() {
    let db = Database::open_in_memory().await.unwrap();
    let id = seed_scheduled_task(&db, None).await;

    assert!(db.try_claim_scheduled_task(id, Utc::now()).await.unwrap());
    assert!(
        !db.try_claim_scheduled_task(id, Utc::now()).await.unwrap(),
        "a second claim on an already-claimed task must lose"
    );
}

/// Scheduled dispatch is the only caller allowed to start from `done`; the
/// ordinary backlog claim must stay unchanged by this feature.
#[tokio::test]
async fn try_claim_backlog_task_still_refuses_a_done_task() {
    let db = Database::open_in_memory().await.unwrap();
    let id = seed_scheduled_task(&db, None).await;

    assert!(
        !db.try_claim_backlog_task(id, Utc::now()).await.unwrap(),
        "DispatchTask's status = backlog precondition must not have been loosened"
    );
}

/// An unscheduled task cannot be claimed by the scheduled path, even by id.
#[tokio::test]
async fn try_claim_scheduled_task_refuses_an_unscheduled_task() {
    let db = Database::open_in_memory().await.unwrap();
    let id = db
        .create_task(CreateTaskRequest {
            title: "ordinary",
            description: "",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
            schedule_interval_secs: None,
            pinned_branch: None,
        })
        .await
        .unwrap();

    assert!(!db.try_claim_scheduled_task(id, Utc::now()).await.unwrap());
}

/// A failed probe must not be read as "nothing changed" — that would stall the
/// pipeline silently for as long as origin stays unreachable.
#[tokio::test]
async fn an_unreachable_origin_dispatches_rather_than_skipping() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let task_id = seed_scheduled_task(&db, Some("abc123")).await;

    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::fail("network is unreachable"), // git fetch origin staging
        MockProcessRunner::ok(),
        MockProcessRunner::ok(),
        MockProcessRunner::ok(),
        MockProcessRunner::ok(),
        MockProcessRunner::ok(),
    ]));
    let (mut scheduler, _rx) = make_runner(Arc::clone(&db), mock.clone());

    scheduler.tick().await;
    scheduler.join_spawned_jobs().await;

    let task = db.get_task(task_id).await.unwrap().unwrap();
    assert_ne!(
        task.status,
        TaskStatus::Done,
        "an unmeasurable branch must dispatch, not skip"
    );
}
