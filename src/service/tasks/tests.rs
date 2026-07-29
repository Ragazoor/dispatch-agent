#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::sync::Arc;

use super::{ClaimTaskParams, CreateTaskParams, ListTasksFilter, TaskService, UpdateTaskParams};
use crate::db::{self, Database, EpicCrud, EpicRead, TaskRead};
use crate::models::{
    EpicId, HookEventKind, NotificationKind, SubStatus, TaskId, TaskStatus, TaskTag,
};
use crate::service::epics::{CreateEpicParams, EpicService, UpdateEpicParams};
use crate::service::{FieldUpdate, ServiceError};

async fn test_db() -> Arc<dyn db::TaskStore> {
    Arc::new(Database::open_in_memory().await.unwrap())
}

fn task_svc(db: &Arc<dyn db::TaskStore>) -> TaskService {
    task_svc_with_runner(db, crate::process::MockProcessRunner::unused())
}

fn epic_svc(db: &Arc<dyn db::TaskStore>) -> EpicService {
    let d: Arc<dyn db::TaskAndEpicStore> = db.clone();
    EpicService::new(d)
}

/// Construct a `TaskService` with a caller-supplied `ProcessRunner` (e.g. a
/// `MockProcessRunner`). Used by watch/finish notification tests to assert
/// on tmux/file-system side effects deterministically.
fn task_svc_with_runner(
    db: &Arc<dyn db::TaskStore>,
    runner: Arc<dyn crate::process::ProcessRunner>,
) -> TaskService {
    let d: Arc<dyn db::TaskAndEpicStore> = db.clone();
    TaskService::new(d, runner)
}

fn make_task_params(repo_path: &str) -> CreateTaskParams {
    CreateTaskParams {
        title: "T".into(),
        description: "".into(),
        repo_path: repo_path.to_string(),
        plan_path: None,
        epic_id: None,
        sort_order: None,
        tag: None,
        base_branch: None,
        wrap_up_mode: None,
        auto_run_plan: false,
    }
}

// -- TaskService ----------------------------------------------------------

#[tokio::test]
async fn create_and_get_task() {
    let db = test_db().await;
    let svc = task_svc(&db);

    let id = svc
        .create_task(CreateTaskParams {
            title: "Test".into(),
            description: "desc".into(),
            repo_path: "/repo".to_string(),
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

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.title, "Test");
    assert_eq!(task.status, TaskStatus::Backlog);
}

#[tokio::test]
async fn create_task_with_tag() {
    let db = test_db().await;
    let svc = task_svc(&db);

    let id = svc
        .create_task(CreateTaskParams {
            title: "Bug fix".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
            plan_path: None,
            epic_id: None,
            sort_order: Some(5),
            tag: Some(TaskTag::Bug),
            base_branch: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.tag, Some(TaskTag::Bug));
    assert_eq!(task.sort_order, Some(5));
}

#[tokio::test]
async fn create_task_with_sort_order() {
    let db = test_db().await;
    let svc = task_svc(&db);

    let id = svc
        .create_task(CreateTaskParams {
            title: "Sorted".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
            plan_path: None,
            epic_id: None,
            sort_order: Some(42),
            tag: None,
            base_branch: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.sort_order, Some(42));
}

#[tokio::test]
async fn update_task_status() {
    let db = test_db().await;
    let svc = task_svc(&db);

    let id = svc
        .create_task(CreateTaskParams {
            title: "T".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
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

    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Running))
        .await
        .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Running);
}

// Note: Done/Archived restriction moved to MCP handler layer.
// The service now allows any status transition (TUI needs it).

#[tokio::test]
async fn update_task_no_fields_returns_error() {
    let db = test_db().await;
    let svc = task_svc(&db);

    let id = svc
        .create_task(CreateTaskParams {
            title: "T".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
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

    let err = svc
        .update_task(UpdateTaskParams::for_task(id))
        .await
        .unwrap_err();
    assert!(matches!(err, ServiceError::Validation(_)));
}

#[tokio::test]
async fn update_task_params_builder_compiles() {
    let db = test_db().await;
    let svc = task_svc(&db);

    let id = svc
        .create_task(CreateTaskParams {
            title: "T".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
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

    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Running))
        .await
        .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Running);
}

#[tokio::test]
async fn update_task_invalid_substatus_for_status() {
    let db = test_db().await;
    let svc = task_svc(&db);

    let id = svc
        .create_task(CreateTaskParams {
            title: "T".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
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

    // active is not valid for backlog
    let err = svc
        .update_task(UpdateTaskParams::for_task(id).sub_status(SubStatus::Active))
        .await
        .unwrap_err();
    assert!(matches!(err, ServiceError::Validation(_)));
}

#[tokio::test]
async fn update_task_entering_done_sets_sort_order() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Review))
        .await
        .unwrap();
    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Done))
        .await
        .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Done);
    assert!(
        task.sort_order.is_some_and(|so| so < 0),
        "expected a negative sort_order on entering Done, got {:?}",
        task.sort_order
    );
}

#[tokio::test]
async fn update_task_leaving_done_clears_sort_order() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Done))
        .await
        .unwrap();
    assert!(svc.get_task(id).await.unwrap().sort_order.is_some());

    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Review))
        .await
        .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Review);
    assert_eq!(task.sort_order, None);
}

#[tokio::test]
async fn update_task_leaving_done_clears_sort_order_even_with_stale_caller_sort_order() {
    // Reproduces the exec_persist_task shape: a caller sends both a status
    // change AND a stale sort_order left over from when the task entered
    // Done, exactly as exec_persist_task (src/runtime/tasks.rs) forwards
    // whatever sort_order is sitting on the in-memory Task struct. The
    // "leaving Done" clear must win over this caller-supplied value.
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Done))
        .await
        .unwrap();
    let stale_sort_order = svc.get_task(id).await.unwrap().sort_order.unwrap();

    svc.update_task(
        UpdateTaskParams::for_task(id)
            .status(TaskStatus::Review)
            .sort_order(stale_sort_order),
    )
    .await
    .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(
        task.sort_order, None,
        "the leaving-Done clear must win over a caller-supplied stale sort_order"
    );
}

#[tokio::test]
async fn update_task_status_change_within_done_leaves_sort_order_untouched() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Done))
        .await
        .unwrap();
    let sort_order_after_entry = svc.get_task(id).await.unwrap().sort_order;

    // An unrelated field edit while already Done (no status change at all).
    svc.update_task(UpdateTaskParams::for_task(id).title("Renamed".to_string()))
        .await
        .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.sort_order, sort_order_after_entry);
}

#[tokio::test]
async fn update_task_non_done_status_change_preserves_sort_order() {
    // The complement of the two leaving-Done tests above: when neither the
    // prior nor the new status is Done, sort_order_for_status_transition
    // returns None and an explicitly-set sort_order must survive the status
    // change untouched. Distinct from
    // update_task_status_change_within_done_leaves_sort_order_untouched,
    // which covers a no-status-change edit on an already-Done task.
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    svc.update_task(UpdateTaskParams::for_task(id).sort_order(7))
        .await
        .unwrap();

    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Running))
        .await
        .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.sort_order, Some(7));
}

#[tokio::test]
async fn update_task_archived_to_backlog_is_unaffected_by_done_rule() {
    // The task editor's freeform STATUS field can retype an Archived task's
    // status back to any value (no transition-legality validation), which
    // routes through this same update_task — a reachable "un-archive" path.
    // sort_order is already None by the time a task reaches Archived (it
    // was cleared on the Done -> Archived leg), so Archived -> Backlog must
    // be a no-op for sort_order and must not error.
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Done))
        .await
        .unwrap();
    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Archived))
        .await
        .unwrap();
    assert_eq!(svc.get_task(id).await.unwrap().sort_order, None);

    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Backlog))
        .await
        .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Backlog);
    assert_eq!(task.sort_order, None);
}

#[tokio::test]
async fn claim_task_success() {
    let db = test_db().await;
    let svc = task_svc(&db);

    let id = svc
        .create_task(CreateTaskParams {
            title: "T".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
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

    let task = svc
        .claim_task(ClaimTaskParams {
            task_id: id,
            worktree: "/repo/.worktrees/feature".into(),
            tmux_window: "win1".into(),
        })
        .await
        .unwrap();
    assert_eq!(task.title, "T");

    // Verify it was actually updated
    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.worktree.as_deref(), Some("/repo/.worktrees/feature"));
}

#[tokio::test]
async fn claim_task_seeds_last_pre_tool_use_at() {
    // Without seeding the timestamp, ClassifyAgentActivity would flip the
    // freshly running task to Stale on the next TUI tick.
    let db = test_db().await;
    let svc = task_svc(&db);

    let id = svc
        .create_task(CreateTaskParams {
            title: "T".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
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

    let before = chrono::Utc::now();
    svc.claim_task(ClaimTaskParams {
        task_id: id,
        worktree: "/repo/.worktrees/feature".into(),
        tmux_window: "win1".into(),
    })
    .await
    .unwrap();

    let task = svc.get_task(id).await.unwrap();
    let stamp = task
        .last_pre_tool_use_at
        .expect("claim_task should seed last_pre_tool_use_at");
    assert!(stamp >= before - chrono::Duration::seconds(1));
    assert!(stamp <= chrono::Utc::now() + chrono::Duration::seconds(1));
}

#[tokio::test]
async fn claim_task_returns_post_patch_task() {
    // The returned Task must reflect the write, not the pre-patch snapshot.
    let db = test_db().await;
    let svc = task_svc(&db);

    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    let claimed = svc
        .claim_task(ClaimTaskParams {
            task_id: id,
            worktree: "/repo/.worktrees/feature".into(),
            tmux_window: "win1".into(),
        })
        .await
        .unwrap();

    assert_eq!(claimed.status, TaskStatus::Running);
    assert_eq!(
        claimed.worktree.as_deref(),
        Some("/repo/.worktrees/feature")
    );
    assert_eq!(claimed.tmux_window.as_deref(), Some("win1"));
    assert!(claimed.last_pre_tool_use_at.is_some());
}

#[tokio::test]
async fn claim_task_recalculates_parent_epic_status() {
    // Backlog → Running on an epic subtask must recalculate the parent epic
    // per the EpicStatusRecalculation invariant (docs/specs/epics.allium).
    // Observable via the done → backlog regression path: an epic marked done
    // that still has an active non-done child regresses to backlog.
    let db = test_db().await;
    let svc = task_svc(&db);
    let epics = epic_svc(&db);

    let epic = epics
        .create_epic(CreateEpicParams {
            title: "E".into(),
            description: String::new(),
            sort_order: None,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
        })
        .await
        .unwrap();

    let id = svc
        .create_task(CreateTaskParams {
            epic_id: Some(epic.id),
            ..make_task_params("/repo")
        })
        .await
        .unwrap();

    // Force a stale rollup: the epic claims done while its only child is backlog.
    db.patch_epic(epic.id, &db::EpicPatch::new().status(TaskStatus::Done))
        .await
        .unwrap();
    assert_eq!(
        db.get_epic(epic.id).await.unwrap().unwrap().status,
        TaskStatus::Done
    );

    svc.claim_task(ClaimTaskParams {
        task_id: id,
        worktree: "/repo/.worktrees/feature".into(),
        tmux_window: "win1".into(),
    })
    .await
    .unwrap();

    assert_eq!(
        db.get_epic(epic.id).await.unwrap().unwrap().status,
        TaskStatus::Backlog,
        "claim_task must recalculate the parent epic's rolled-up status"
    );
}

#[tokio::test]
async fn claim_task_wrong_repo() {
    let db = test_db().await;
    let svc = task_svc(&db);

    let id = svc
        .create_task(CreateTaskParams {
            title: "T".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
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

    let err = svc
        .claim_task(ClaimTaskParams {
            task_id: id,
            worktree: "/repo-b/.worktrees/feature".into(),
            tmux_window: "win1".into(),
        })
        .await
        .unwrap_err();
    assert!(matches!(err, ServiceError::Validation(_)));
}

#[tokio::test]
async fn claim_task_not_backlog() {
    let db = test_db().await;
    let svc = task_svc(&db);

    let id = svc
        .create_task(CreateTaskParams {
            title: "T".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
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

    // Move to running first
    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Running))
        .await
        .unwrap();

    let err = svc
        .claim_task(ClaimTaskParams {
            task_id: id,
            worktree: "/repo/.worktrees/feature".into(),
            tmux_window: "win1".into(),
        })
        .await
        .unwrap_err();
    assert!(matches!(err, ServiceError::Validation(_)));
}

#[tokio::test]
async fn list_tasks_with_filter() {
    let db = test_db().await;
    let svc = task_svc(&db);

    svc.create_task(CreateTaskParams {
        title: "T1".into(),
        description: "".into(),
        repo_path: "/repo".to_string(),
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

    let tasks = svc
        .list_tasks(ListTasksFilter {
            statuses: Some(vec![TaskStatus::Backlog]),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(tasks.len(), 1);

    let tasks = svc
        .list_tasks(ListTasksFilter {
            statuses: Some(vec![TaskStatus::Running]),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(tasks.is_empty());
}

#[tokio::test]
async fn get_task_not_found() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let err = svc.get_task(TaskId(999)).await.unwrap_err();
    assert!(matches!(err, ServiceError::NotFound(_)));
}

#[tokio::test]
async fn update_task_with_epic_linkage() {
    let db = test_db().await;
    let task_svc = task_svc(&db);
    let epic_svc = epic_svc(&db);

    let epic = epic_svc
        .create_epic(CreateEpicParams {
            title: "Epic".into(),
            description: "".into(),
            sort_order: None,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
        })
        .await
        .unwrap();

    let id = task_svc
        .create_task(CreateTaskParams {
            title: "T".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
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

    task_svc
        .update_task(UpdateTaskParams::for_task(id).epic_id(epic.id))
        .await
        .unwrap();

    let task = task_svc.get_task(id).await.unwrap();
    assert_eq!(task.epic_id, Some(epic.id));
}

#[tokio::test]
async fn update_task_status_recalculates_parent_epic() {
    // recalculate_epic_for_task: epic with running task stays in backlog
    // (running and review tasks do not auto-advance epic status)
    let db = test_db().await;
    let task_svc = task_svc(&db);
    let epic_svc = epic_svc(&db);

    let epic = epic_svc
        .create_epic(CreateEpicParams {
            title: "E".into(),
            description: "".into(),
            sort_order: None,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
        })
        .await
        .unwrap();

    let id = task_svc
        .create_task(CreateTaskParams {
            title: "T".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
            plan_path: None,
            epic_id: Some(epic.id),
            sort_order: None,
            tag: None,
            base_branch: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    task_svc
        .update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Running))
        .await
        .unwrap();

    let refreshed = epic_svc.get_epic(epic.id).await.unwrap();
    assert_eq!(refreshed.status, TaskStatus::Backlog); // running task → epic stays backlog
}

#[tokio::test]
async fn update_task_relink_recalculates_old_and_new_epic() {
    // Linkage-change branch of recalculate_epic_for_task: moving a Running
    // task between two epics. Both epics stay in Backlog because running
    // tasks do not auto-advance epic status.
    let db = test_db().await;
    let task_svc = task_svc(&db);
    let epic_svc = epic_svc(&db);

    let epic_a = epic_svc
        .create_epic(CreateEpicParams {
            title: "A".into(),
            description: "".into(),
            sort_order: None,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
        })
        .await
        .unwrap();
    let epic_b = epic_svc
        .create_epic(CreateEpicParams {
            title: "B".into(),
            description: "".into(),
            sort_order: None,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
        })
        .await
        .unwrap();

    let id = task_svc
        .create_task(CreateTaskParams {
            title: "T".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
            plan_path: None,
            epic_id: Some(epic_a.id),
            sort_order: None,
            tag: None,
            base_branch: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    task_svc
        .update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Running))
        .await
        .unwrap();

    // Sanity: epic A stays in Backlog (running task doesn't auto-advance).
    assert_eq!(
        epic_svc.get_epic(epic_a.id).await.unwrap().status,
        TaskStatus::Backlog
    );

    task_svc
        .update_task(UpdateTaskParams::for_task(id).epic_id(epic_b.id))
        .await
        .unwrap();

    // After relinking, both epics stay in Backlog (running task doesn't auto-advance)
    assert_eq!(
        epic_svc.get_epic(epic_a.id).await.unwrap().status,
        TaskStatus::Backlog
    );
    assert_eq!(
        epic_svc.get_epic(epic_b.id).await.unwrap().status,
        TaskStatus::Backlog
    );
}

// -- move_task_to_epic ----------------------------------------------------

/// Helper: create a root epic with the given title.
async fn make_epic(svc: &EpicService, title: &str) -> crate::models::Epic {
    svc.create_epic(CreateEpicParams {
        title: title.into(),
        description: "".into(),
        sort_order: None,
        parent_epic_id: None,
        feed_command: None,
        feed_interval_secs: None,
    })
    .await
    .unwrap()
}

/// Helper: create a backlog task in the given (optional) epic.
async fn make_task(svc: &TaskService, epic_id: Option<EpicId>) -> TaskId {
    svc.create_task(CreateTaskParams {
        title: "T".into(),
        description: "".into(),
        repo_path: "/repo".to_string(),
        plan_path: None,
        epic_id,
        sort_order: None,
        tag: None,
        base_branch: None,
        wrap_up_mode: None,
        auto_run_plan: false,
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn move_task_to_epic_links_standalone_task() {
    let db = test_db().await;
    let task_svc = task_svc(&db);
    let epic_svc = epic_svc(&db);

    let epic = make_epic(&epic_svc, "E").await;
    let id = make_task(&task_svc, None).await;

    task_svc.move_task_to_epic(id, Some(epic.id)).await.unwrap();

    assert_eq!(task_svc.get_task(id).await.unwrap().epic_id, Some(epic.id));
}

#[tokio::test]
async fn move_task_to_epic_detaches_and_recalculates_old_epic() {
    // Epic A holds a Done task plus a Backlog task → A stays Backlog (not all
    // active children done). Detaching the Backlog task leaves only the Done
    // task, so A recalculates to Done.
    let db = test_db().await;
    let task_svc = task_svc(&db);
    let epic_svc = epic_svc(&db);

    let epic_a = make_epic(&epic_svc, "A").await;
    let done_task = make_task(&task_svc, Some(epic_a.id)).await;
    let backlog_task = make_task(&task_svc, Some(epic_a.id)).await;

    task_svc
        .update_task(UpdateTaskParams::for_task(done_task).status(TaskStatus::Done))
        .await
        .unwrap();
    assert_eq!(
        epic_svc.get_epic(epic_a.id).await.unwrap().status,
        TaskStatus::Backlog,
        "epic with a non-done active child stays Backlog"
    );

    // Detach the Backlog task → A's only active child is now Done → A is Done.
    task_svc
        .move_task_to_epic(backlog_task, None)
        .await
        .unwrap();

    assert_eq!(task_svc.get_task(backlog_task).await.unwrap().epic_id, None);
    assert_eq!(
        epic_svc.get_epic(epic_a.id).await.unwrap().status,
        TaskStatus::Done,
        "old epic recalculates to Done after the non-done child leaves"
    );
}

#[tokio::test]
async fn move_task_to_epic_between_epics_recalculates_new_epic() {
    // Epic B holds a single Done task → B is Done. Moving a Backlog task into B
    // regresses B back to Backlog (it now has a non-done active child).
    let db = test_db().await;
    let task_svc = task_svc(&db);
    let epic_svc = epic_svc(&db);

    let epic_a = make_epic(&epic_svc, "A").await;
    let epic_b = make_epic(&epic_svc, "B").await;

    let b_task = make_task(&task_svc, Some(epic_b.id)).await;
    task_svc
        .update_task(UpdateTaskParams::for_task(b_task).status(TaskStatus::Done))
        .await
        .unwrap();
    assert_eq!(
        epic_svc.get_epic(epic_b.id).await.unwrap().status,
        TaskStatus::Done,
        "epic with all active children done is Done"
    );

    let a_task = make_task(&task_svc, Some(epic_a.id)).await;
    task_svc
        .move_task_to_epic(a_task, Some(epic_b.id))
        .await
        .unwrap();

    assert_eq!(
        task_svc.get_task(a_task).await.unwrap().epic_id,
        Some(epic_b.id)
    );
    assert_eq!(
        epic_svc.get_epic(epic_b.id).await.unwrap().status,
        TaskStatus::Backlog,
        "new epic regresses to Backlog after a non-done task joins"
    );
}

#[tokio::test]
async fn move_task_to_epic_unknown_epic_errors() {
    let db = test_db().await;
    let task_svc = task_svc(&db);

    let id = make_task(&task_svc, None).await;

    let result = task_svc.move_task_to_epic(id, Some(EpicId(9999))).await;

    assert!(
        matches!(result, Err(ServiceError::NotFound(_))),
        "moving to a non-existent epic should be NotFound, got: {result:?}"
    );
    // The task is left untouched.
    assert_eq!(task_svc.get_task(id).await.unwrap().epic_id, None);
}

#[tokio::test]
async fn move_task_to_epic_unknown_task_errors() {
    let db = test_db().await;
    let task_svc = task_svc(&db);

    let result = task_svc.move_task_to_epic(TaskId(9999), None).await;

    assert!(
        result.is_err(),
        "moving a non-existent task should error, got: {result:?}"
    );
}

// -- EpicService ----------------------------------------------------------

#[tokio::test]
async fn create_and_get_epic() {
    let db = test_db().await;
    let svc = epic_svc(&db);

    let epic = svc
        .create_epic(CreateEpicParams {
            title: "Epic 1".into(),
            description: "desc".into(),
            sort_order: None,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
        })
        .await
        .unwrap();

    let fetched = svc.get_epic(epic.id).await.unwrap();
    assert_eq!(fetched.title, "Epic 1");
}

#[tokio::test]
async fn get_epic_not_found() {
    let db = test_db().await;
    let svc = epic_svc(&db);
    let err = svc.get_epic(EpicId(999)).await.unwrap_err();
    assert!(matches!(err, ServiceError::NotFound(_)));
}

#[tokio::test]
async fn update_epic_status() {
    let db = test_db().await;
    let svc = epic_svc(&db);

    let epic = svc
        .create_epic(CreateEpicParams {
            title: "E".into(),
            description: "".into(),
            sort_order: None,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
        })
        .await
        .unwrap();

    svc.update_epic(UpdateEpicParams {
        epic_id: epic.id,
        title: None,
        description: None,
        status: Some(TaskStatus::Running),
        plan_path: None,
        sort_order: None,
        auto_dispatch: None,
        feed_command: None,
        feed_interval_secs: None,
        group_by_repo: None,
        parent_epic_id: None,
    })
    .await
    .unwrap();

    let updated = svc.get_epic(epic.id).await.unwrap();
    assert_eq!(updated.status, TaskStatus::Running);
}

#[tokio::test]
async fn update_epic_no_fields_returns_error() {
    let db = test_db().await;
    let svc = epic_svc(&db);

    let epic = svc
        .create_epic(CreateEpicParams {
            title: "E".into(),
            description: "".into(),
            sort_order: None,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
        })
        .await
        .unwrap();

    let err = svc
        .update_epic(UpdateEpicParams {
            epic_id: epic.id,
            title: None,
            description: None,
            status: None,
            plan_path: None,
            sort_order: None,
            auto_dispatch: None,
            feed_command: None,
            feed_interval_secs: None,
            group_by_repo: None,
            parent_epic_id: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(err, ServiceError::Validation(_)));
}

#[tokio::test]
async fn update_epic_auto_dispatch_persists() {
    let db = test_db().await;
    let svc = epic_svc(&db);

    let epic = svc
        .create_epic(CreateEpicParams {
            title: "E".into(),
            description: "".into(),
            sort_order: None,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
        })
        .await
        .unwrap();

    // Default is false.
    assert!(!db.get_epic(epic.id).await.unwrap().unwrap().auto_dispatch);

    svc.update_epic(UpdateEpicParams {
        epic_id: epic.id,
        title: None,
        description: None,
        status: None,
        plan_path: None,
        sort_order: None,
        auto_dispatch: Some(true),
        feed_command: None,
        feed_interval_secs: None,
        group_by_repo: None,
        parent_epic_id: None,
    })
    .await
    .unwrap();

    assert!(db.get_epic(epic.id).await.unwrap().unwrap().auto_dispatch);
}

#[tokio::test]
async fn list_epics_with_progress() {
    let db = test_db().await;
    let task_svc = task_svc(&db);
    let epic_svc = epic_svc(&db);

    let epic = epic_svc
        .create_epic(CreateEpicParams {
            title: "E".into(),
            description: "".into(),
            sort_order: None,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
        })
        .await
        .unwrap();

    task_svc
        .create_task(CreateTaskParams {
            title: "Sub1".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
            plan_path: None,
            epic_id: Some(epic.id),
            sort_order: None,
            tag: None,
            base_branch: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let list = epic_svc.list_epics_with_progress().await.unwrap();
    assert_eq!(list.len(), 1);
    let (_, done, total) = &list[0];
    assert_eq!(*done, 0);
    assert_eq!(*total, 1);
}

#[tokio::test]
async fn list_epics_with_progress_multiple_epics() {
    let db = test_db().await;
    let task_svc = task_svc(&db);
    let epic_svc = epic_svc(&db);

    let e1 = epic_svc
        .create_epic(CreateEpicParams {
            title: "E1".into(),
            description: "".into(),
            sort_order: None,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
        })
        .await
        .unwrap();
    let e2 = epic_svc
        .create_epic(CreateEpicParams {
            title: "E2".into(),
            description: "".into(),
            sort_order: None,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
        })
        .await
        .unwrap();

    // 2 tasks in E1
    let t1 = task_svc
        .create_task(CreateTaskParams {
            title: "T1".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
            plan_path: None,
            epic_id: Some(e1.id),
            sort_order: None,
            tag: None,
            base_branch: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    task_svc
        .create_task(CreateTaskParams {
            title: "T2".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
            plan_path: None,
            epic_id: Some(e1.id),
            sort_order: None,
            tag: None,
            base_branch: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    // 1 task in E2
    task_svc
        .create_task(CreateTaskParams {
            title: "T3".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
            plan_path: None,
            epic_id: Some(e2.id),
            sort_order: None,
            tag: None,
            base_branch: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    // Mark T1 as done
    task_svc
        .update_task(UpdateTaskParams::for_task(t1).status(TaskStatus::Done))
        .await
        .unwrap();

    let list = epic_svc.list_epics_with_progress().await.unwrap();
    assert_eq!(list.len(), 2);
    let e1_progress = list.iter().find(|(e, _, _)| e.id == e1.id).unwrap();
    assert_eq!(e1_progress.1, 1); // 1 done
    assert_eq!(e1_progress.2, 2); // 2 total
    let e2_progress = list.iter().find(|(e, _, _)| e.id == e2.id).unwrap();
    assert_eq!(e2_progress.1, 0);
    assert_eq!(e2_progress.2, 1);
}

#[tokio::test]
async fn update_task_status_recalculates_epic() {
    let db = test_db().await;
    let task_svc = task_svc(&db);
    let epic_svc = epic_svc(&db);

    let epic = epic_svc
        .create_epic(CreateEpicParams {
            title: "E".into(),
            description: "".into(),
            sort_order: None,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
        })
        .await
        .unwrap();

    let task_id = task_svc
        .create_task(CreateTaskParams {
            title: "Sub".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
            plan_path: None,
            epic_id: Some(epic.id),
            sort_order: None,
            tag: None,
            base_branch: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    task_svc
        .update_task(UpdateTaskParams::for_task(task_id).status(TaskStatus::Done))
        .await
        .unwrap();

    let updated_epic = epic_svc.get_epic(epic.id).await.unwrap();
    assert_eq!(updated_epic.status, TaskStatus::Done);
}

#[tokio::test]
async fn get_epic_with_subtasks() {
    let db = test_db().await;
    let task_svc = task_svc(&db);
    let epic_svc = epic_svc(&db);

    let epic = epic_svc
        .create_epic(CreateEpicParams {
            title: "E".into(),
            description: "".into(),
            sort_order: None,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
        })
        .await
        .unwrap();

    task_svc
        .create_task(CreateTaskParams {
            title: "Sub".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
            plan_path: None,
            epic_id: Some(epic.id),
            sort_order: None,
            tag: None,
            base_branch: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let (e, subtasks) = epic_svc.get_epic_with_subtasks(epic.id).await.unwrap();
    assert_eq!(e.title, "E");
    assert_eq!(subtasks.len(), 1);
}

// -- next_backlog_task -----------------------------------------------------

#[tokio::test]
async fn next_backlog_task_returns_first_by_sort_order() {
    let db = test_db().await;
    let task_svc = task_svc(&db);
    let epic_svc = epic_svc(&db);

    let epic = epic_svc
        .create_epic(CreateEpicParams {
            title: "E".into(),
            description: "".into(),
            sort_order: None,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
        })
        .await
        .unwrap();

    task_svc
        .create_task(CreateTaskParams {
            title: "Second".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
            plan_path: None,
            epic_id: Some(epic.id),
            sort_order: Some(20),
            tag: None,
            base_branch: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    task_svc
        .create_task(CreateTaskParams {
            title: "First".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
            plan_path: None,
            epic_id: Some(epic.id),
            sort_order: Some(10),
            tag: None,
            base_branch: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let next = task_svc.next_backlog_task(epic.id).await.unwrap();
    assert_eq!(next.unwrap().title, "First");
}

#[tokio::test]
async fn next_backlog_task_skips_non_backlog() {
    let db = test_db().await;
    let task_svc = task_svc(&db);
    let epic_svc = epic_svc(&db);

    let epic = epic_svc
        .create_epic(CreateEpicParams {
            title: "E".into(),
            description: "".into(),
            sort_order: None,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
        })
        .await
        .unwrap();

    let id = task_svc
        .create_task(CreateTaskParams {
            title: "Running".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
            plan_path: None,
            epic_id: Some(epic.id),
            sort_order: Some(1),
            tag: None,
            base_branch: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    // Move to running
    task_svc
        .update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Running))
        .await
        .unwrap();

    let next = task_svc.next_backlog_task(epic.id).await.unwrap();
    assert!(next.is_none());
}

#[tokio::test]
async fn next_backlog_task_epic_not_found() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let err = svc.next_backlog_task(EpicId(999)).await.unwrap_err();
    assert!(matches!(err, ServiceError::NotFound(_)));
}

// -- claim_next_backlog_task -----------------------------------------------
//
// The atomic claim is what makes AutoDispatchNextSubtask's "at most one agent
// per closed session" guarantee hold under concurrent closes
// (docs/specs/epics.allium).

/// Create an epic with `count` backlog subtasks, sort_order 1..=count.
async fn epic_with_backlog_subtasks(
    db: &Arc<dyn db::TaskStore>,
    count: i64,
) -> (EpicId, Vec<TaskId>) {
    let epic_svc = epic_svc(db);
    let task_svc = task_svc(db);
    let epic = epic_svc
        .create_epic(CreateEpicParams {
            title: "E".into(),
            description: "".into(),
            sort_order: None,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
        })
        .await
        .unwrap();
    let mut ids = Vec::new();
    for i in 1..=count {
        let id = task_svc
            .create_task(CreateTaskParams {
                title: format!("Sub {i}"),
                description: "".into(),
                repo_path: "/repo".to_string(),
                plan_path: None,
                epic_id: Some(epic.id),
                sort_order: Some(i),
                tag: None,
                base_branch: None,
                wrap_up_mode: None,
                auto_run_plan: false,
            })
            .await
            .unwrap();
        ids.push(id);
    }
    (epic.id, ids)
}

#[tokio::test]
async fn claim_next_backlog_task_marks_the_claimed_task_running() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let (epic_id, ids) = epic_with_backlog_subtasks(&db, 2).await;

    let claimed = svc.claim_next_backlog_task(epic_id).await.unwrap().unwrap();
    assert_eq!(claimed.id, ids[0], "claims the first subtask by sort_order");
    assert_eq!(claimed.status, TaskStatus::Running);
    assert_eq!(
        claimed.sub_status,
        SubStatus::default_for(TaskStatus::Running)
    );
    assert!(
        claimed.last_pre_tool_use_at.is_some(),
        "the claim seeds last_pre_tool_use_at so the tick classifier keeps the task Active"
    );

    let persisted = db.get_task(ids[0]).await.unwrap().unwrap();
    assert_eq!(persisted.status, TaskStatus::Running);
    assert!(persisted.worktree.is_none(), "the claim does not provision");
}

#[tokio::test]
async fn claim_next_backlog_task_returns_none_when_no_backlog_remains() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let (epic_id, _) = epic_with_backlog_subtasks(&db, 1).await;

    assert!(svc
        .claim_next_backlog_task(epic_id)
        .await
        .unwrap()
        .is_some());
    assert!(
        svc.claim_next_backlog_task(epic_id)
            .await
            .unwrap()
            .is_none(),
        "a claimed task is out of contention"
    );
}

#[tokio::test]
async fn claim_next_backlog_task_is_exclusive_under_concurrency() {
    let db = test_db().await;
    let svc = Arc::new(task_svc(&db));
    let (epic_id, _) = epic_with_backlog_subtasks(&db, 2).await;

    let a = {
        let svc = svc.clone();
        tokio::spawn(async move { svc.claim_next_backlog_task(epic_id).await })
    };
    let b = {
        let svc = svc.clone();
        tokio::spawn(async move { svc.claim_next_backlog_task(epic_id).await })
    };
    let first = a.await.unwrap().unwrap().expect("first claim");
    let second = b.await.unwrap().unwrap().expect("second claim");

    assert_ne!(
        first.id, second.id,
        "two concurrent claims must never win the same subtask"
    );
    assert!(
        svc.claim_next_backlog_task(epic_id)
            .await
            .unwrap()
            .is_none(),
        "both subtasks are claimed, so a third caller gets None"
    );
}

#[tokio::test]
async fn claim_next_backlog_task_epic_not_found() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let err = svc.claim_next_backlog_task(EpicId(999)).await.unwrap_err();
    assert!(matches!(err, ServiceError::NotFound(_)));
}

// -- create_task_returning ---------------------------------------------------

#[tokio::test]
async fn create_task_returning_gives_full_task() {
    let db = test_db().await;
    let svc = task_svc(&db);

    let task = svc
        .create_task_returning(CreateTaskParams {
            title: "Full task".into(),
            description: "desc".into(),
            repo_path: "/repo".to_string(),
            plan_path: None,
            epic_id: None,
            sort_order: None,
            tag: Some(TaskTag::Feature),
            base_branch: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    assert_eq!(task.title, "Full task");
    assert_eq!(task.description, "desc");
    assert_eq!(task.tag, Some(TaskTag::Feature));
    assert_eq!(task.status, TaskStatus::Backlog);
}

#[tokio::test]
async fn create_task_with_auto_run_plan_true_persists() {
    let db = test_db().await;
    let svc = task_svc(&db);

    let task = svc
        .create_task_returning(CreateTaskParams {
            title: "T".to_string(),
            description: "d".to_string(),
            repo_path: "/r".to_string(),
            plan_path: None,
            epic_id: None,
            sort_order: None,
            tag: None,
            base_branch: None,
            wrap_up_mode: None,
            auto_run_plan: true,
        })
        .await
        .unwrap();
    assert!(task.auto_run_plan);
}

#[tokio::test]
async fn create_task_returning_with_epic() {
    let db = test_db().await;
    let tsvc = task_svc(&db);
    let esvc = epic_svc(&db);

    let epic = esvc
        .create_epic(CreateEpicParams {
            title: "E".into(),
            description: "".into(),
            sort_order: None,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
        })
        .await
        .unwrap();

    let task = tsvc
        .create_task_returning(CreateTaskParams {
            title: "Sub".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
            plan_path: None,
            epic_id: Some(epic.id),
            sort_order: None,
            tag: None,
            base_branch: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    assert_eq!(task.epic_id, Some(epic.id));
}

#[tokio::test]
async fn create_task_returning_sets_all_optional_fields_atomically() {
    let db = test_db().await;
    let tsvc = task_svc(&db);
    let esvc = epic_svc(&db);

    let epic = esvc
        .create_epic(CreateEpicParams {
            title: "E".into(),
            description: "".into(),
            sort_order: None,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
        })
        .await
        .unwrap();

    let task = tsvc
        .create_task_returning(CreateTaskParams {
            title: "Atomic".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
            plan_path: None,
            epic_id: Some(epic.id),
            sort_order: Some(3),
            tag: Some(TaskTag::Feature),
            base_branch: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    assert_eq!(task.epic_id, Some(epic.id));
    assert_eq!(task.sort_order, Some(3));
    assert_eq!(task.tag, Some(TaskTag::Feature));
}

// -- delete_task -------------------------------------------------------------

#[tokio::test]
async fn delete_task_removes_it() {
    let db = test_db().await;
    let svc = task_svc(&db);

    let id = svc
        .create_task(CreateTaskParams {
            title: "T".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
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

    svc.delete_task(id).await.unwrap();

    let err = svc.get_task(id).await.unwrap_err();
    assert!(matches!(err, ServiceError::NotFound(_)));
}

#[tokio::test]
async fn delete_task_not_found() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let err = svc.delete_task(TaskId(999)).await.unwrap_err();
    assert!(matches!(err, ServiceError::NotFound(_)));
}

// -- update_task with worktree/tmux_window -----------------------------------

#[tokio::test]
async fn update_task_sets_worktree_and_tmux_window() {
    let db = test_db().await;
    let svc = task_svc(&db);

    let id = svc
        .create_task(CreateTaskParams {
            title: "T".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
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

    svc.update_task(
        UpdateTaskParams::for_task(id)
            .status(TaskStatus::Running)
            .worktree(FieldUpdate::Set("/repo/.worktrees/feat".into()))
            .tmux_window(FieldUpdate::Set("task-1".into())),
    )
    .await
    .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.worktree.as_deref(), Some("/repo/.worktrees/feat"));
    assert_eq!(task.tmux_window.as_deref(), Some("task-1"));
}

#[tokio::test]
async fn update_task_clears_worktree() {
    let db = test_db().await;
    let svc = task_svc(&db);

    let id = svc
        .create_task(CreateTaskParams {
            title: "T".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
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

    // Set worktree
    svc.update_task(
        UpdateTaskParams::for_task(id)
            .status(TaskStatus::Running)
            .worktree(FieldUpdate::Set("/repo/.worktrees/feat".into()))
            .tmux_window(FieldUpdate::Set("task-1".into())),
    )
    .await
    .unwrap();

    // Clear worktree via FieldUpdate::Clear
    svc.update_task(
        UpdateTaskParams::for_task(id)
            .status(TaskStatus::Done)
            .worktree(FieldUpdate::Clear)
            .tmux_window(FieldUpdate::Clear),
    )
    .await
    .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert!(task.worktree.is_none());
    assert!(task.tmux_window.is_none());
}

// -- update_task allows done/archived (MCP restriction moved to handler) -----

#[tokio::test]
async fn update_task_allows_done_status() {
    let db = test_db().await;
    let svc = task_svc(&db);

    let id = svc
        .create_task(CreateTaskParams {
            title: "T".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
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

    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Done))
        .await
        .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Done);
}

// -- delete_epic -------------------------------------------------------------

#[tokio::test]
async fn delete_epic_removes_it() {
    let db = test_db().await;
    let svc = epic_svc(&db);

    let epic = svc
        .create_epic(CreateEpicParams {
            title: "E".into(),
            description: "".into(),
            sort_order: None,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
        })
        .await
        .unwrap();

    svc.delete_epic(epic.id).await.unwrap();

    let err = svc.get_epic(epic.id).await.unwrap_err();
    assert!(matches!(err, ServiceError::NotFound(_)));
}

#[tokio::test]
async fn delete_epic_not_found() {
    let db = test_db().await;
    let svc = epic_svc(&db);
    let err = svc.delete_epic(EpicId(999)).await.unwrap_err();
    assert!(matches!(err, ServiceError::NotFound(_)));
}

// --- FieldUpdate ---

#[tokio::test]
async fn field_update_set_has_value() {
    let fu: FieldUpdate = FieldUpdate::Set("hello".to_string());
    assert!(matches!(fu, FieldUpdate::Set(ref s) if s == "hello"));
}

#[tokio::test]
async fn field_update_clear_is_clear() {
    let fu: FieldUpdate = FieldUpdate::Clear;
    assert!(matches!(fu, FieldUpdate::Clear));
}

#[tokio::test]
async fn update_task_worktree_set_persists() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc
        .create_task(CreateTaskParams {
            title: "t".into(),
            description: "d".into(),
            repo_path: "/repo".to_string(),
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
    svc.update_task(
        UpdateTaskParams::for_task(id)
            .status(TaskStatus::Running)
            .worktree(FieldUpdate::Set("/wt".to_string()))
            .tmux_window(FieldUpdate::Set("win".to_string())),
    )
    .await
    .unwrap();
    let task = db.get_task(TaskId(id.0)).await.unwrap().unwrap();
    assert_eq!(task.worktree.as_deref(), Some("/wt"));
    assert_eq!(task.tmux_window.as_deref(), Some("win"));
}

#[tokio::test]
async fn update_task_worktree_clear_sets_null() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc
        .create_task(CreateTaskParams {
            title: "t".into(),
            description: "d".into(),
            repo_path: "/repo".to_string(),
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
    // First set a value
    svc.update_task(
        UpdateTaskParams::for_task(id)
            .status(TaskStatus::Running)
            .worktree(FieldUpdate::Set("/wt".to_string()))
            .tmux_window(FieldUpdate::Set("win".to_string())),
    )
    .await
    .unwrap();
    // Then clear it
    svc.update_task(
        UpdateTaskParams::for_task(id)
            .worktree(FieldUpdate::Clear)
            .tmux_window(FieldUpdate::Clear),
    )
    .await
    .unwrap();
    let task = db.get_task(TaskId(id.0)).await.unwrap().unwrap();
    assert_eq!(task.worktree, None);
    assert_eq!(task.tmux_window, None);
}

#[tokio::test]
async fn update_task_pr_url_set_and_clear() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc
        .create_task(CreateTaskParams {
            title: "t".into(),
            description: "d".into(),
            repo_path: "/repo".to_string(),
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
    // Set PR URL
    svc.update_task(
        UpdateTaskParams::for_task(id).url(crate::service::UrlUpdate::Set(
            crate::models::TaskUrl::new(
                "https://github.com/org/repo/pull/1",
                crate::models::UrlType::Pr,
            ),
        )),
    )
    .await
    .unwrap();
    let task = db.get_task(TaskId(id.0)).await.unwrap().unwrap();
    assert_eq!(
        task.url.as_ref().map(|u| u.url.as_str()),
        Some("https://github.com/org/repo/pull/1")
    );
    // Clear PR URL
    svc.update_task(UpdateTaskParams::for_task(id).url(crate::service::UrlUpdate::Clear))
        .await
        .unwrap();
    let task = db.get_task(TaskId(id.0)).await.unwrap().unwrap();
    assert_eq!(task.url, None);
}

#[tokio::test]
async fn list_tasks_filters_by_epic_id() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let esvc = epic_svc(&db);

    let epic = esvc
        .create_epic(CreateEpicParams {
            title: "E".into(),
            description: "".into(),
            sort_order: None,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
        })
        .await
        .unwrap();

    let id1 = svc
        .create_task(CreateTaskParams {
            title: "In epic".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
            plan_path: None,
            epic_id: Some(epic.id),
            sort_order: None,
            tag: None,
            base_branch: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let _id2 = svc
        .create_task(CreateTaskParams {
            title: "No epic".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
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

    let tasks = svc
        .list_tasks(ListTasksFilter {
            epic_id: Some(epic.id),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, id1);
}

#[tokio::test]
async fn list_tasks_excludes_archived_by_default() {
    let db = test_db().await;
    let svc = task_svc(&db);

    let id = svc
        .create_task(CreateTaskParams {
            title: "T".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
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

    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Archived))
        .await
        .unwrap();

    let tasks = svc
        .list_tasks(ListTasksFilter {
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(tasks.is_empty());
}

#[tokio::test]
async fn list_tasks_filters_by_repo_paths() {
    let db = test_db().await;
    let svc = task_svc(&db);

    svc.create_task(CreateTaskParams {
        title: "Repo A".into(),
        description: "".into(),
        repo_path: "/repo/a".to_string(),
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

    svc.create_task(CreateTaskParams {
        title: "Repo B".into(),
        description: "".into(),
        repo_path: "/repo/b".to_string(),
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

    let tasks = svc
        .list_tasks(ListTasksFilter {
            repo_paths: Some(vec!["/repo/a".to_string()]),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].title, "Repo A");
}

#[tokio::test]
async fn list_tasks_excludes_caller_task() {
    let db = test_db().await;
    let svc = task_svc(&db);

    let id1 = svc
        .create_task(CreateTaskParams {
            title: "T1".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
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

    svc.create_task(CreateTaskParams {
        title: "T2".into(),
        description: "".into(),
        repo_path: "/repo".to_string(),
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

    let tasks = svc
        .list_tasks(ListTasksFilter {
            exclude_task_id: Some(id1),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].title, "T2");
}

#[tokio::test]
async fn validate_send_message_missing_worktree() {
    let db = test_db().await;
    let svc = task_svc(&db);

    let from_id = svc
        .create_task(CreateTaskParams {
            title: "Sender".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
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

    // Target task has no worktree (still backlog)
    let to_id = svc
        .create_task(CreateTaskParams {
            title: "Receiver".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
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

    let err = svc.validate_send_message(from_id, to_id).await.unwrap_err();
    assert!(matches!(err, ServiceError::Validation(_)));
    assert!(err.to_string().contains("no worktree"));
}

#[tokio::test]
async fn validate_send_message_missing_tmux_window() {
    let db = test_db().await;
    let svc = task_svc(&db);

    let from_id = svc
        .create_task(CreateTaskParams {
            title: "Sender".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
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

    let to_id = svc
        .create_task(CreateTaskParams {
            title: "Receiver".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
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

    // Set worktree but not tmux_window
    svc.update_task(
        UpdateTaskParams::for_task(to_id)
            .status(TaskStatus::Running)
            .worktree(FieldUpdate::Set("/repo/.worktrees/feat".into())),
    )
    .await
    .unwrap();

    let err = svc.validate_send_message(from_id, to_id).await.unwrap_err();
    assert!(matches!(err, ServiceError::Validation(_)));
    assert!(err.to_string().contains("no tmux window"));
}

#[tokio::test]
async fn validate_send_message_target_not_found() {
    let db = test_db().await;
    let svc = task_svc(&db);

    let from_id = svc
        .create_task(CreateTaskParams {
            title: "Sender".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
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

    let err = svc
        .validate_send_message(from_id, TaskId(999))
        .await
        .unwrap_err();
    assert!(matches!(err, ServiceError::NotFound(_)));
}

// -------------------------------------------------------------------------
// Epic-in-epic service tests
// -------------------------------------------------------------------------

#[tokio::test]
async fn create_sub_epic_links_parent() {
    let db = test_db().await;
    let svc = epic_svc(&db);

    let parent = svc
        .create_epic(CreateEpicParams {
            title: "Parent".into(),
            description: "".into(),
            sort_order: None,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
        })
        .await
        .unwrap();

    let child = svc
        .create_epic(CreateEpicParams {
            title: "Child".into(),
            description: "".into(),
            sort_order: None,
            parent_epic_id: Some(parent.id),
            feed_command: None,
            feed_interval_secs: None,
        })
        .await
        .unwrap();

    assert_eq!(child.parent_epic_id, Some(parent.id));

    let fetched = svc.get_epic(child.id).await.unwrap();
    assert_eq!(fetched.parent_epic_id, Some(parent.id));
}

#[tokio::test]
async fn list_root_epics_service() {
    let db = test_db().await;
    let svc = epic_svc(&db);

    let parent = svc
        .create_epic(CreateEpicParams {
            title: "Root".into(),
            description: "".into(),
            sort_order: None,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
        })
        .await
        .unwrap();
    svc.create_epic(CreateEpicParams {
        title: "Sub".into(),
        description: "".into(),
        sort_order: None,
        parent_epic_id: Some(parent.id),
        feed_command: None,
        feed_interval_secs: None,
    })
    .await
    .unwrap();

    let roots = svc.list_root_epics().await.unwrap();
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].id, parent.id);
}

#[tokio::test]
async fn list_sub_epics_service() {
    let db = test_db().await;
    let svc = epic_svc(&db);

    let parent = svc
        .create_epic(CreateEpicParams {
            title: "Parent".into(),
            description: "".into(),
            sort_order: None,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
        })
        .await
        .unwrap();
    let child = svc
        .create_epic(CreateEpicParams {
            title: "Child".into(),
            description: "".into(),
            sort_order: None,
            parent_epic_id: Some(parent.id),
            feed_command: None,
            feed_interval_secs: None,
        })
        .await
        .unwrap();

    let subs = svc.list_sub_epics(parent.id).await.unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].id, child.id);
}

// -- TOCTOU regression -----------------------------------------------------
//
// `validate_sub_status` in `crud.rs` reads the current task status before
// writing the patch. A second writer can land between the read and the
// write. Per the docs/conventions.md "Sub-status validation TOCTOU" note,
// this is accepted: simultaneous status changes from two agents on the
// same task are user error, and the result is last-write-wins. These
// tests pin that behaviour so the policy can't drift silently.

#[tokio::test]
async fn update_task_toctou_last_write_wins() {
    let db = test_db().await;
    let svc_a = task_svc(&db);
    let svc_b = task_svc(&db);

    let id = svc_a
        .create_task(CreateTaskParams {
            title: "T".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
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

    // svc_a moves the task to Running/Active.
    svc_a
        .update_task(
            UpdateTaskParams::for_task(id)
                .status(TaskStatus::Running)
                .sub_status(SubStatus::Active),
        )
        .await
        .unwrap();

    // svc_b moves it on to Review/AwaitingReview. The sub_status is valid
    // for the requested status, so validation passes despite the write
    // landing on top of svc_a's state. Last write wins.
    svc_b
        .update_task(
            UpdateTaskParams::for_task(id)
                .status(TaskStatus::Review)
                .sub_status(SubStatus::AwaitingReview),
        )
        .await
        .unwrap();

    let task = svc_a.get_task(id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Review);
    assert_eq!(task.sub_status, SubStatus::AwaitingReview);
}

#[tokio::test]
async fn update_task_sub_status_validated_against_persisted_status() {
    // A sub-status update without a status change is validated against the
    // currently-persisted status. If a previous writer changed status, the
    // later sub_status-only update sees the new status — this is the
    // TOCTOU-accepting behaviour: validation uses *current* state, not the
    // state the caller may have observed earlier.
    let db = test_db().await;
    let svc_a = task_svc(&db);
    let svc_b = task_svc(&db);

    let id = svc_a
        .create_task(CreateTaskParams {
            title: "T".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
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

    svc_a
        .update_task(
            UpdateTaskParams::for_task(id)
                .status(TaskStatus::Running)
                .sub_status(SubStatus::Active),
        )
        .await
        .unwrap();

    // svc_b sees Running (sub_status Stale is valid for Running).
    svc_b
        .update_task(UpdateTaskParams::for_task(id).sub_status(SubStatus::Stale))
        .await
        .unwrap();
    assert_eq!(
        svc_a.get_task(id).await.unwrap().sub_status,
        SubStatus::Stale
    );

    // Now svc_a moves status to Review without specifying sub_status.
    svc_a
        .update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Review))
        .await
        .unwrap();

    // svc_b attempts a sub_status-only update with `Active`, which is
    // valid for Running but NOT for Review. Validation reads the current
    // status (Review) and rejects the update — no panic, just a
    // Validation error.
    let err = svc_b
        .update_task(UpdateTaskParams::for_task(id).sub_status(SubStatus::Active))
        .await
        .unwrap_err();
    assert!(matches!(err, ServiceError::Validation(_)), "got {err:?}");
}

// -- record_hook_event ---------------------------------------------------

/// Move a freshly created backlog task into the Running state with a custom
/// sub_status. Used by the hook-event tests to set up scenarios where a
/// hook arrives at a Running task already in NeedsInput / Active.
async fn create_running_task(svc: &TaskService, sub_status: SubStatus) -> TaskId {
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();
    svc.update_task(
        UpdateTaskParams::for_task(id)
            .status(TaskStatus::Running)
            .sub_status(sub_status),
    )
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn record_hook_event_pre_tool_use_stamps_and_clears_needs_input() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = create_running_task(&svc, SubStatus::NeedsInput).await;
    let earlier = chrono::Utc::now() - chrono::Duration::seconds(30);
    db.patch_task(
        id,
        &crate::db::TaskPatch::new().last_notification_at(Some(earlier)),
    )
    .await
    .unwrap();

    svc.record_hook_event(id, HookEventKind::PreToolUse)
        .await
        .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.sub_status, SubStatus::Active);
    assert!(task.last_pre_tool_use_at.is_some());
}

/// Compat/raise path: a Notification with no `notification_type` (older Claude
/// Code, or the field absent) preserves the historical "always needs_input".
#[tokio::test]
async fn record_hook_event_notification_absent_kind_sets_needs_input_and_stamps() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = create_running_task(&svc, SubStatus::Active).await;

    svc.record_hook_event(id, HookEventKind::Notification(None))
        .await
        .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.sub_status, SubStatus::NeedsInput);
    assert!(task.last_notification_at.is_some());
}

/// Raise bucket: permission_prompt / idle_prompt / elicitation_dialog each set
/// needs_input and stamp last_notification_at (a genuine block).
#[tokio::test]
async fn record_hook_event_notification_raise_kinds_set_needs_input() {
    for kind in [
        NotificationKind::PermissionPrompt,
        NotificationKind::IdlePrompt,
        NotificationKind::ElicitationDialog,
    ] {
        let db = test_db().await;
        let svc = task_svc(&db);
        let id = create_running_task(&svc, SubStatus::Active).await;

        svc.record_hook_event(id, HookEventKind::Notification(Some(kind)))
            .await
            .unwrap();

        let task = svc.get_task(id).await.unwrap();
        assert_eq!(task.sub_status, SubStatus::NeedsInput, "kind {kind:?}");
        assert!(task.last_notification_at.is_some(), "kind {kind:?}");
    }
}

/// Clear bucket: elicitation_complete / elicitation_response return the task to
/// active and drop last_notification_at the instant the user answers.
#[tokio::test]
async fn record_hook_event_notification_resolve_kinds_clear_needs_input() {
    for kind in [
        NotificationKind::ElicitationComplete,
        NotificationKind::ElicitationResponse,
    ] {
        let db = test_db().await;
        let svc = task_svc(&db);
        let id = create_running_task(&svc, SubStatus::NeedsInput).await;
        db.patch_task(
            id,
            &crate::db::TaskPatch::new().last_notification_at(Some(chrono::Utc::now())),
        )
        .await
        .unwrap();

        svc.record_hook_event(id, HookEventKind::Notification(Some(kind)))
            .await
            .unwrap();

        let task = svc.get_task(id).await.unwrap();
        assert_eq!(task.status, TaskStatus::Running, "kind {kind:?}");
        assert_eq!(task.sub_status, SubStatus::Active, "kind {kind:?}");
        assert!(task.last_notification_at.is_none(), "kind {kind:?}");
    }
}

/// Ignore bucket: auth_success is informational — it must not change an active
/// task's sub_status and must not stamp last_notification_at.
#[tokio::test]
async fn record_hook_event_notification_auth_success_is_noop_on_active() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = create_running_task(&svc, SubStatus::Active).await;

    svc.record_hook_event(
        id,
        HookEventKind::Notification(Some(NotificationKind::AuthSuccess)),
    )
    .await
    .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.sub_status, SubStatus::Active);
    assert!(task.last_notification_at.is_none());
}

/// Ignore bucket must be a *pure* no-op: an auth_success arriving while the task
/// is already needs_input must not clobber that state back to active.
#[tokio::test]
async fn record_hook_event_notification_auth_success_does_not_clobber_needs_input() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = create_running_task(&svc, SubStatus::NeedsInput).await;
    let stamped = chrono::Utc::now();
    db.patch_task(
        id,
        &crate::db::TaskPatch::new().last_notification_at(Some(stamped)),
    )
    .await
    .unwrap();

    svc.record_hook_event(
        id,
        HookEventKind::Notification(Some(NotificationKind::AuthSuccess)),
    )
    .await
    .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.sub_status, SubStatus::NeedsInput);
    assert!(task.last_notification_at.is_some());
}

#[tokio::test]
async fn record_hook_event_stop_transitions_to_review_and_clears_stamps() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = create_running_task(&svc, SubStatus::Active).await;
    let now = chrono::Utc::now();
    db.patch_task(
        id,
        &crate::db::TaskPatch::new()
            .last_pre_tool_use_at(Some(now))
            .last_notification_at(Some(now)),
    )
    .await
    .unwrap();

    svc.record_hook_event(id, HookEventKind::Stop)
        .await
        .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Review);
    assert_eq!(task.sub_status, SubStatus::AwaitingReview);
    assert!(task.last_pre_tool_use_at.is_none());
    assert!(task.last_notification_at.is_none());
}

#[tokio::test]
async fn record_hook_event_noop_for_non_running_task() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc
        .create_task(CreateTaskParams {
            title: "T".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
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

    svc.record_hook_event(id, HookEventKind::PreToolUse)
        .await
        .unwrap();
    svc.record_hook_event(id, HookEventKind::Notification(None))
        .await
        .unwrap();
    svc.record_hook_event(id, HookEventKind::Stop)
        .await
        .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Backlog);
    assert!(task.last_pre_tool_use_at.is_none());
    assert!(task.last_notification_at.is_none());
}

#[tokio::test]
async fn record_hook_event_user_prompt_submit_resumes_review_task_to_running() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = create_running_task(&svc, SubStatus::Active).await;
    svc.record_hook_event(id, HookEventKind::Stop)
        .await
        .unwrap();
    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Review);

    svc.record_hook_event(id, HookEventKind::UserPromptSubmit)
        .await
        .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.sub_status, SubStatus::Active);
    assert!(task.last_pre_tool_use_at.is_some());
}

#[tokio::test]
async fn record_hook_event_user_prompt_submit_refreshes_running_task() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = create_running_task(&svc, SubStatus::NeedsInput).await;

    svc.record_hook_event(id, HookEventKind::UserPromptSubmit)
        .await
        .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.sub_status, SubStatus::Active);
    assert!(task.last_pre_tool_use_at.is_some());
}

#[tokio::test]
async fn record_hook_event_user_prompt_submit_noop_for_backlog_task() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    svc.record_hook_event(id, HookEventKind::UserPromptSubmit)
        .await
        .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Backlog);
    assert!(task.last_pre_tool_use_at.is_none());
}

#[tokio::test]
async fn record_hook_event_unknown_task_returns_not_found() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let err = svc
        .record_hook_event(TaskId(99_999), HookEventKind::PreToolUse)
        .await
        .unwrap_err();
    assert!(matches!(err, ServiceError::NotFound(_)), "got {err:?}");
}

// -- attach_plan ----------------------------------------------------------

#[tokio::test]
async fn attach_plan_sets_plan_path() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();
    assert_eq!(svc.get_task(id).await.unwrap().plan_path, None);

    svc.attach_plan(id, "/abs/plan.md").await.unwrap();

    assert_eq!(
        svc.get_task(id).await.unwrap().plan_path.as_deref(),
        Some("/abs/plan.md")
    );
}

#[tokio::test]
async fn attach_plan_on_missing_task_is_not_found() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let err = svc
        .attach_plan(TaskId(9999), "/abs/plan.md")
        .await
        .unwrap_err();
    assert!(matches!(err, ServiceError::NotFound(_)), "got {err:?}");
}

// -- mark_pr_learnings_gate_shown -----------------------------------------

#[tokio::test]
async fn mark_pr_learnings_gate_shown_first_then_idempotent() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    assert!(svc.mark_pr_learnings_gate_shown(id).await.unwrap());
    assert!(!svc.mark_pr_learnings_gate_shown(id).await.unwrap());
}

mod property_tests {
    use super::*;
    use crate::db::{CreateTaskRequest, EpicPatch};
    use proptest::prelude::*;

    /// The non-archived task statuses an epic rolls up over, plus `Archived`,
    /// which the recalc query filters out (so it must never influence the roll-up).
    const RECALC_STATUSES: &[TaskStatus] = &[
        TaskStatus::Backlog,
        TaskStatus::Running,
        TaskStatus::Review,
        TaskStatus::Done,
        TaskStatus::Archived,
    ];

    /// Epic baseline statuses — `recalculate_epic_status` reads the epic's
    /// current status as the fallback/regression pivot. Archived epics are
    /// excluded (they are not recalculated as live epics here).
    const EPIC_BASELINES: &[TaskStatus] = &[
        TaskStatus::Backlog,
        TaskStatus::Running,
        TaskStatus::Review,
        TaskStatus::Done,
    ];

    /// Re-derive the expected rolled-up epic status from the same rule the DB
    /// recalc applies: archived subtasks are ignored; an all-`Done` active set
    /// rolls the epic to `Done`; a `Done` epic with any active non-`Done` child
    /// regresses to `Backlog`; otherwise the baseline status is preserved.
    fn expected_rollup(baseline: TaskStatus, task_statuses: &[TaskStatus]) -> TaskStatus {
        let active: Vec<TaskStatus> = task_statuses
            .iter()
            .copied()
            .filter(|s| *s != TaskStatus::Archived)
            .collect();
        if active.is_empty() {
            baseline
        } else if active.iter().all(|s| *s == TaskStatus::Done) {
            TaskStatus::Done
        } else if baseline == TaskStatus::Done {
            TaskStatus::Backlog
        } else {
            baseline
        }
    }

    /// Mirror of the `FieldUpdate ↔ Option<Option<T>>` mapping documented in
    /// docs/conventions.md and applied by `validators::build_task_patch`:
    ///   `Some(Set(v))` → `Some(Some(v))`
    ///   `Some(Clear)`  → `Some(None)`
    ///   `None`         → `None`
    fn map_field_update(fu: Option<FieldUpdate>) -> Option<Option<String>> {
        match fu {
            Some(FieldUpdate::Set(v)) => Some(Some(v)),
            Some(FieldUpdate::Clear) => Some(None),
            None => None,
        }
    }

    fn field_update_strategy() -> impl Strategy<Value = Option<FieldUpdate>> {
        prop_oneof![
            Just(None),
            Just(Some(FieldUpdate::Clear)),
            "[a-zA-Z0-9/]{0,32}".prop_map(|s| Some(FieldUpdate::Set(s))),
        ]
    }

    proptest! {
        /// `FieldUpdate` round-trips through the canonical mapping cleanly.
        #[test]
        fn field_update_roundtrip(fu in field_update_strategy()) {
            let mapped = map_field_update(fu.clone());
            let back: Option<FieldUpdate> = match mapped {
                None              => None,
                Some(None)        => Some(FieldUpdate::Clear),
                Some(Some(v))     => Some(FieldUpdate::Set(v)),
            };
            prop_assert_eq!(back, fu);
        }

        /// `build_task_patch` applies the mapping to `worktree` and
        /// `tmux_window`. For all input combinations, the resulting `TaskPatch`
        /// must carry the canonical `Option<Option<&str>>` shape.
        #[test]
        fn build_task_patch_maps_field_updates(
            worktree in field_update_strategy(),
            tmux_window in field_update_strategy(),
        ) {
            let mut params = UpdateTaskParams::for_task(TaskId(1));
            if let Some(ref w) = worktree    { params = params.worktree(w.clone()); }
            if let Some(ref t) = tmux_window { params = params.tmux_window(t.clone()); }

            let patch = super::super::validators::build_task_patch(&params, None, None);

            let expect = |fu: &Option<FieldUpdate>| -> Option<Option<String>> {
                fu.as_ref().map(|x| match x {
                    FieldUpdate::Set(v) => Some(v.clone()),
                    FieldUpdate::Clear  => None,
                })
            };
            prop_assert_eq!(
                patch.worktree.map(|o| o.map(|s| s.to_string())),
                expect(&worktree)
            );
            prop_assert_eq!(
                patch.tmux_window.map(|o| o.map(|s| s.to_string())),
                expect(&tmux_window)
            );
        }

        /// Epic sub-status recalculation over random subtask-status combinations:
        /// for any baseline epic status and any multiset of subtask statuses,
        /// `recalculate_epic_status` must produce the rolled-up status given by
        /// `expected_rollup` (archived subtasks ignored; all-Done → Done;
        /// Done-epic regression → Backlog; otherwise baseline preserved).
        #[test]
        fn epic_recalc_rolls_up_subtask_statuses(
            baseline_idx in 0..EPIC_BASELINES.len(),
            status_idxs in proptest::collection::vec(0..RECALC_STATUSES.len(), 0..6),
        ) {
            let baseline = EPIC_BASELINES[baseline_idx];
            let task_statuses: Vec<TaskStatus> =
                status_idxs.iter().map(|&i| RECALC_STATUSES[i]).collect();

            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let actual = rt.block_on(async {
                let db = test_db().await;
                let epic = db.create_epic("E", "", None).await.unwrap();
                // Seed the baseline status the recalc pivots on.
                db.patch_epic(epic.id, &EpicPatch::new().status(baseline))
                    .await
                    .unwrap();
                for status in &task_statuses {
                    db.create_task(CreateTaskRequest {
                        title: "t",
                        description: "",
                        repo_path: "/r",
                        plan: None,
                        status: *status,
                        base_branch: "main",
                        epic_id: Some(epic.id),
                        sort_order: None,
                        tag: None,
                        wrap_up_mode: None,
                        auto_run_plan: false,
                    })
                    .await
                    .unwrap();
                }
                db.recalculate_epic_status(epic.id).await.unwrap();
                db.get_epic(epic.id).await.unwrap().unwrap().status
            });

            prop_assert_eq!(
                actual,
                expected_rollup(baseline, &task_statuses),
                "baseline={:?} tasks={:?}",
                baseline,
                task_statuses
            );
        }
    }
}

// -- cli_update_task -------------------------------------------------------

#[tokio::test]
async fn cli_update_task_updates_status_unconditionally() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    let updated = svc
        .cli_update_task(id, TaskStatus::Running, None, None)
        .await
        .unwrap();

    assert!(updated);
    assert_eq!(svc.get_task(id).await.unwrap().status, TaskStatus::Running);
}

#[tokio::test]
async fn cli_update_task_entering_done_sets_sort_order() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    svc.cli_update_task(id, TaskStatus::Done, None, None)
        .await
        .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert!(
        task.sort_order.is_some_and(|so| so < 0),
        "expected a negative sort_order on entering Done, got {:?}",
        task.sort_order
    );
}

#[tokio::test]
async fn cli_update_task_leaving_done_clears_sort_order() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    svc.cli_update_task(id, TaskStatus::Done, None, None)
        .await
        .unwrap();
    assert!(svc.get_task(id).await.unwrap().sort_order.is_some());

    svc.cli_update_task(id, TaskStatus::Backlog, None, None)
        .await
        .unwrap();

    assert_eq!(svc.get_task(id).await.unwrap().sort_order, None);
}

#[tokio::test]
async fn cli_update_task_only_if_not_matching_does_not_touch_sort_order() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    // Precondition doesn't match (task is Backlog, not Running) — the
    // conditional write must be a full no-op, including sort_order.
    let updated = svc
        .cli_update_task(id, TaskStatus::Done, Some(TaskStatus::Running), None)
        .await
        .unwrap();

    assert!(!updated);
    assert_eq!(svc.get_task(id).await.unwrap().sort_order, None);
}

#[tokio::test]
async fn cli_update_task_only_if_matching_entering_done_sets_sort_order() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    let updated = svc
        .cli_update_task(id, TaskStatus::Done, Some(TaskStatus::Backlog), None)
        .await
        .unwrap();

    assert!(updated);
    assert!(svc
        .get_task(id)
        .await
        .unwrap()
        .sort_order
        .is_some_and(|so| so < 0));
}

#[tokio::test]
async fn cli_update_task_with_only_if_matching_returns_true_and_updates() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    let updated = svc
        .cli_update_task(id, TaskStatus::Running, Some(TaskStatus::Backlog), None)
        .await
        .unwrap();

    assert!(updated);
    assert_eq!(svc.get_task(id).await.unwrap().status, TaskStatus::Running);
}

#[tokio::test]
async fn cli_update_task_with_only_if_not_matching_returns_false_and_preserves_status() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    let updated = svc
        .cli_update_task(id, TaskStatus::Done, Some(TaskStatus::Running), None)
        .await
        .unwrap();

    assert!(!updated);
    assert_eq!(svc.get_task(id).await.unwrap().status, TaskStatus::Backlog);
}

#[tokio::test]
async fn cli_update_task_unconditional_sets_sub_status() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    svc.cli_update_task(id, TaskStatus::Running, None, Some(SubStatus::Active))
        .await
        .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.sub_status, SubStatus::Active);
}

#[tokio::test]
async fn cli_update_task_conditional_sets_sub_status_when_matching() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    svc.cli_update_task(
        id,
        TaskStatus::Running,
        Some(TaskStatus::Backlog),
        Some(SubStatus::Active),
    )
    .await
    .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.sub_status, SubStatus::Active);
}

#[tokio::test]
async fn cli_update_task_conditional_does_not_apply_sub_status_when_not_matching() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    svc.cli_update_task(
        id,
        TaskStatus::Done,
        Some(TaskStatus::Running),
        Some(SubStatus::Active),
    )
    .await
    .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Backlog);
    assert_eq!(task.sub_status, SubStatus::None);
}

#[tokio::test]
async fn cli_update_task_recalculates_parent_epic() {
    let db = test_db().await;
    let tsvc = task_svc(&db);
    let esvc = epic_svc(&db);

    let epic = esvc
        .create_epic(CreateEpicParams {
            title: "E".into(),
            description: "".into(),
            sort_order: None,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
        })
        .await
        .unwrap();

    let id = tsvc
        .create_task(CreateTaskParams {
            epic_id: Some(epic.id),
            ..make_task_params("/repo")
        })
        .await
        .unwrap();

    tsvc.cli_update_task(id, TaskStatus::Done, None, None)
        .await
        .unwrap();

    assert_eq!(
        esvc.get_epic(epic.id).await.unwrap().status,
        TaskStatus::Done
    );
}

// -- validate_wrap_up ------------------------------------------------------

#[tokio::test]
async fn validate_wrap_up_running_with_worktree_succeeds() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    svc.update_task(
        UpdateTaskParams::for_task(id)
            .status(TaskStatus::Running)
            .worktree(FieldUpdate::Set("/repo/.worktrees/feat".into())),
    )
    .await
    .unwrap();

    let task = svc.validate_wrap_up(id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Running);
}

#[tokio::test]
async fn validate_wrap_up_review_with_worktree_succeeds() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    svc.update_task(
        UpdateTaskParams::for_task(id)
            .status(TaskStatus::Review)
            .worktree(FieldUpdate::Set("/repo/.worktrees/feat".into())),
    )
    .await
    .unwrap();

    let task = svc.validate_wrap_up(id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Review);
}

#[tokio::test]
async fn validate_wrap_up_backlog_task_fails() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    let err = svc.validate_wrap_up(id).await.unwrap_err();
    assert!(matches!(err, ServiceError::Validation(_)));
}

#[tokio::test]
async fn validate_wrap_up_running_without_worktree_fails() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Running))
        .await
        .unwrap();

    let err = svc.validate_wrap_up(id).await.unwrap_err();
    assert!(matches!(err, ServiceError::Validation(_)));
}

// -- was_pr_finalisation ---------------------------------------------------

#[tokio::test]
async fn update_task_pr_finalisation_true_when_first_pr_and_review_status() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    let result = svc
        .update_task(
            UpdateTaskParams::for_task(id)
                .status(TaskStatus::Review)
                .url(crate::service::UrlUpdate::Set(crate::models::TaskUrl::new(
                    "https://github.com/org/repo/pull/1",
                    crate::models::UrlType::Pr,
                ))),
        )
        .await
        .unwrap();

    assert!(result.was_pr_finalisation);
}

#[tokio::test]
async fn update_task_pr_finalisation_false_when_pr_already_existed() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    svc.update_task(
        UpdateTaskParams::for_task(id).url(crate::service::UrlUpdate::Set(
            crate::models::TaskUrl::new(
                "https://github.com/org/repo/pull/1",
                crate::models::UrlType::Pr,
            ),
        )),
    )
    .await
    .unwrap();

    let result = svc
        .update_task(
            UpdateTaskParams::for_task(id)
                .status(TaskStatus::Review)
                .url(crate::service::UrlUpdate::Set(crate::models::TaskUrl::new(
                    "https://github.com/org/repo/pull/1",
                    crate::models::UrlType::Pr,
                ))),
        )
        .await
        .unwrap();

    assert!(!result.was_pr_finalisation);
}

#[tokio::test]
async fn update_task_pr_finalisation_false_when_not_moving_to_review() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    let result = svc
        .update_task(
            UpdateTaskParams::for_task(id).url(crate::service::UrlUpdate::Set(
                crate::models::TaskUrl::new(
                    "https://github.com/org/repo/pull/1",
                    crate::models::UrlType::Pr,
                ),
            )),
        )
        .await
        .unwrap();

    assert!(!result.was_pr_finalisation);
}

#[tokio::test]
async fn update_task_pr_finalisation_false_with_non_pr_url() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    // A non-PR-typed url moving to Review is not a PR finalisation.
    let result = svc
        .update_task(
            UpdateTaskParams::for_task(id)
                .status(TaskStatus::Review)
                .url(crate::service::UrlUpdate::Set(crate::models::TaskUrl::new(
                    "https://github.com/org/repo/issues/1",
                    crate::models::UrlType::Issue,
                ))),
        )
        .await
        .unwrap();

    assert!(!result.was_pr_finalisation);
}

#[tokio::test]
async fn update_task_propagates_db_error_on_prior_task_read() {
    // When update_task needs to read the prior task state (epic_id is set, so
    // needs_prior=true) and the DB returns an error when reading the task back,
    // the error should propagate rather than being silently swallowed as None.
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let svc = TaskService::new(
        db.clone() as Arc<dyn db::TaskAndEpicStore>,
        crate::process::MockProcessRunner::unused(),
    );

    // Create a task that we'll corrupt so get_task fails
    let id = svc
        .create_task(CreateTaskParams {
            title: "T".into(),
            description: "".into(),
            repo_path: "/repo".to_string(),
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

    // Corrupt the task's tag to an unknown value so that get_task returns an error.
    // tag has no CHECK constraint so the UPDATE succeeds, but parse_tag() will fail
    // when the row is read back.
    let raw_id = id.0;
    db.db_call(move |conn| {
        conn.execute(
            "UPDATE tasks SET tag = 'invalid_unknown_tag' WHERE id = ?1",
            rusqlite::params![raw_id],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    // Create an epic so we can link to it (epic_id triggers needs_prior=true)
    let epic = db.create_epic("E", "D", None).await.unwrap();

    // update_task with epic_id → needs_prior=true → get_task fails → should propagate
    let result = svc
        .update_task(UpdateTaskParams::for_task(id).epic_id(epic.id))
        .await;

    assert!(
        result.is_err(),
        "DB error on prior-task read should propagate, not be silently ignored"
    );
    let err = result.unwrap_err();
    assert!(
        matches!(err, ServiceError::Internal(_)),
        "error should be ServiceError::Internal, got: {err:?}"
    );
}

// -- Repo-grouping routing -------------------------------------------------

#[tokio::test]
async fn create_task_on_grouped_epic_routes_into_sub_epic() {
    use crate::db::EpicCrud;
    let db = std::sync::Arc::new(crate::db::Database::open_in_memory().await.unwrap());
    let svc =
        crate::service::TaskService::new(db.clone(), crate::process::MockProcessRunner::unused());
    let root = db.create_epic("root", "", None).await.unwrap();
    db.patch_epic(root.id, &crate::db::EpicPatch::new().group_by_repo(true))
        .await
        .unwrap();

    let task = svc
        .create_task_returning(crate::service::CreateTaskParams {
            title: "t".into(),
            description: String::new(),
            repo_path: "/x/dispatch".into(),
            plan_path: None,
            epic_id: Some(root.id),
            sort_order: None,
            tag: None,
            base_branch: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let placed = db.get_epic(task.epic_id.unwrap()).await.unwrap().unwrap();
    assert_eq!(placed.title, "dispatch");
    assert_eq!(placed.origin, crate::models::EpicOrigin::RepoGroup);
    assert_ne!(task.epic_id, Some(root.id));
}

#[tokio::test]
async fn update_repo_path_reroutes_within_grouped_epic() {
    use crate::db::EpicCrud;
    let db = std::sync::Arc::new(crate::db::Database::open_in_memory().await.unwrap());
    let svc =
        crate::service::TaskService::new(db.clone(), crate::process::MockProcessRunner::unused());
    let root = db.create_epic("root", "", None).await.unwrap();
    db.patch_epic(root.id, &crate::db::EpicPatch::new().group_by_repo(true))
        .await
        .unwrap();
    let task = svc
        .create_task_returning(crate::service::CreateTaskParams {
            title: "t".into(),
            description: String::new(),
            repo_path: "/x/alpha".into(),
            plan_path: None,
            epic_id: Some(root.id),
            sort_order: None,
            tag: None,
            base_branch: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    svc.update_task(
        crate::service::UpdateTaskParams::for_task(task.id).repo_path("/x/beta".into()),
    )
    .await
    .unwrap();

    let reloaded = db.get_task(task.id).await.unwrap().unwrap();
    let placed = db
        .get_epic(reloaded.epic_id.unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(placed.title, "beta");
}

#[tokio::test]
async fn move_task_to_grouped_epic_routes_into_sub_epic() {
    // TDD — blocker fix: move_task_to_epic must route through route_target.
    // Create a group_by_repo non-feed root, a standalone task, call
    // move_task_to_epic(task, Some(root)) and assert the task lands in a
    // per-repo RepoGroup sub-epic, NOT directly on the root.
    use crate::db::EpicCrud;
    let db = std::sync::Arc::new(crate::db::Database::open_in_memory().await.unwrap());
    let svc =
        crate::service::TaskService::new(db.clone(), crate::process::MockProcessRunner::unused());

    // Create a grouped (non-feed) root.
    let root = db.create_epic("root", "", None).await.unwrap();
    db.patch_epic(root.id, &crate::db::EpicPatch::new().group_by_repo(true))
        .await
        .unwrap();

    // Create a standalone task with a known repo path.
    let task_id = svc
        .create_task(CreateTaskParams {
            title: "T".into(),
            description: "".into(),
            repo_path: "/x/dispatch".into(),
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

    // Move the task onto the grouped root.
    svc.move_task_to_epic(task_id, Some(root.id)).await.unwrap();

    // The task must land in a sub-epic, not directly on the root.
    let task = svc.get_task(task_id).await.unwrap();
    assert_ne!(
        task.epic_id,
        Some(root.id),
        "task must NOT be placed directly on the grouped root"
    );
    let placed_id = task.epic_id.expect("task must have an epic");
    let placed = db.get_epic(placed_id).await.unwrap().unwrap();
    assert_eq!(
        placed.origin,
        crate::models::EpicOrigin::RepoGroup,
        "placed epic must be a RepoGroup sub-epic"
    );
    assert_eq!(placed.title, "dispatch", "sub-epic title = repo basename");
}

#[tokio::test]
async fn move_task_to_non_grouped_epic_lands_directly() {
    // Regression guard: moving to a plain (non-grouped) epic must NOT route.
    use crate::db::EpicCrud;
    let db = std::sync::Arc::new(crate::db::Database::open_in_memory().await.unwrap());
    let svc =
        crate::service::TaskService::new(db.clone(), crate::process::MockProcessRunner::unused());

    let plain = db.create_epic("plain", "", None).await.unwrap();

    let task_id = svc
        .create_task(CreateTaskParams {
            title: "T".into(),
            description: "".into(),
            repo_path: "/x/dispatch".into(),
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

    svc.move_task_to_epic(task_id, Some(plain.id))
        .await
        .unwrap();

    let task = svc.get_task(task_id).await.unwrap();
    assert_eq!(
        task.epic_id,
        Some(plain.id),
        "plain epic: task must land directly on it"
    );
}

// -- watchers ---------------------------------------------------------------

mod watchers {
    use super::*;
    use crate::service::tasks::watchers::SubscribeOutcome;

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
                .tmux_window(Some("task-watcher")),
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
            2,
            "expected one tmux notification (2 send-keys calls)"
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
    async fn cli_update_task_to_done_notifies_watcher() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path().to_str().unwrap().to_string();
        let db = test_db().await;
        let runner: Arc<dyn crate::process::ProcessRunner> =
            Arc::new(crate::process::MockProcessRunner::new(vec![
                crate::process::MockProcessRunner::ok(),
                crate::process::MockProcessRunner::ok(),
            ]));
        let svc = task_svc_with_runner(&db, runner);

        let watcher = svc.create_task(make_task_params("/repo")).await.unwrap();
        db.patch_task(
            watcher,
            &db::TaskPatch::new()
                .worktree(Some(&worktree))
                .tmux_window(Some("task-watcher")),
        )
        .await
        .unwrap();
        let target = svc.create_task(make_task_params("/repo")).await.unwrap();
        svc.subscribe_to_task(watcher, target).await.unwrap();

        svc.cli_update_task(target, TaskStatus::Done, None, None)
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
                .tmux_window(Some("task-watcher")),
        )
        .await
        .unwrap();
        let target = svc.create_task(make_task_params("/repo")).await.unwrap();
        svc.subscribe_to_task(watcher, target).await.unwrap();

        svc.delete_task(target).await.unwrap();

        assert_eq!(mock.recorded_calls().len(), 2);
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
        // TaskService::update_task/cli_update_task entirely — so a
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
                .tmux_window(Some("task-watcher")),
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
}
