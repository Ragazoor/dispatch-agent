use super::*;
use crate::models::test_tmux_window;

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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
        phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
        feed_append_only: None,
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
            feed_append_only: None,
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
        feed_append_only: None,
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
        })
        .await
        .unwrap();

    let (e, subtasks) = epic_svc.get_epic_with_subtasks(epic.id).await.unwrap();
    assert_eq!(e.title, "E");
    assert_eq!(subtasks.len(), 1);
}

// -- close_session ---------------------------------------------------------
//
// The one purpose-built terminal write. Callers gate the tmux teardown and the
// epic chain on its Result, so `Err` must mean "the write did not land" and
// nothing else — see ExitSession in docs/specs/pr-workflow.allium.

/// A running task with a worktree and a tmux window — what `exit_session`
/// closes.
async fn running_task_with_window(
    db: &Arc<dyn db::TaskStore>,
    epic_id: Option<EpicId>,
) -> (TaskId, crate::models::TmuxWindow) {
    let svc = task_svc(db);
    let mut params = make_task_params("/repo");
    params.epic_id = epic_id;
    let id = svc.create_task(params).await.unwrap();
    let window = crate::models::TmuxWindow::for_task(id);
    svc.update_task(
        UpdateTaskParams::for_task(id)
            .status(TaskStatus::Running)
            .worktree(FieldUpdate::Set("/repo/.worktrees/wt".to_string()))
            .tmux_window(crate::service::TmuxWindowUpdate::Set(window.clone())),
    )
    .await
    .unwrap();
    (id, window)
}

#[tokio::test]
async fn close_session_done_moves_task_to_done_and_clears_the_window() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let (id, window) = running_task_with_window(&db, None).await;

    let closed = svc
        .close_session(id, crate::service::CloseSessionOutcome::Done)
        .await
        .unwrap();

    assert_eq!(
        closed.window,
        Some(window),
        "the caller tears down the window this close cleared"
    );
    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Done);
    assert_eq!(task.sub_status, SubStatus::default_for(TaskStatus::Done));
    assert!(task.tmux_window.is_none());
    assert!(
        task.worktree.is_some(),
        "the worktree survives the close; it is removed on archive"
    );
    assert!(
        task.sort_order.is_some(),
        "the Done transition applies the completion-recency rank"
    );
    assert!(task.url.is_none());
}

#[tokio::test]
async fn close_session_pr_moves_task_to_review_and_records_the_url() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let (id, window) = running_task_with_window(&db, None).await;

    let closed = svc
        .close_session(
            id,
            crate::service::CloseSessionOutcome::Review {
                pr_url: crate::models::TaskUrl::new(
                    "https://github.com/acme/repo/pull/7".to_string(),
                    crate::models::UrlType::Pr,
                ),
            },
        )
        .await
        .unwrap();

    assert_eq!(closed.window, Some(window));
    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Review);
    assert_eq!(task.sub_status, SubStatus::default_for(TaskStatus::Review));
    assert!(task.tmux_window.is_none());
    let url = task.url.expect("pr url recorded");
    assert_eq!(url.url, "https://github.com/acme/repo/pull/7");
    assert!(url.is_pr());
}

#[tokio::test]
async fn close_session_reports_a_missing_window_as_none() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    let closed = svc
        .close_session(id, crate::service::CloseSessionOutcome::Done)
        .await
        .unwrap();

    assert!(closed.window.is_none(), "nothing to tear down");
}

#[tokio::test]
async fn close_session_missing_task_is_not_found() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let err = svc
        .close_session(TaskId(999_999), crate::service::CloseSessionOutcome::Done)
        .await
        .unwrap_err();
    assert!(matches!(err, ServiceError::NotFound(_)));
}

#[tokio::test]
async fn close_session_recalculates_the_parent_epic() {
    let db = test_db().await;
    let svc = task_svc(&db);
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
    let (id, _) = running_task_with_window(&db, Some(epic.id)).await;

    svc.close_session(id, crate::service::CloseSessionOutcome::Done)
        .await
        .unwrap();

    let after = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(
        after.status,
        TaskStatus::Done,
        "an epic whose only subtask closed rolls up to Done"
    );
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
                phoenix: false,
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
async fn claim_next_backlog_task_clears_leftover_subagents_without_flipping() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let (epic_id, ids) = epic_with_backlog_subtasks(&db, 1).await;

    // Leftovers from a previous run of this same task.
    db.subagent_start(ids[0], "stale", "old-session", chrono::Utc::now())
        .await
        .unwrap();
    db.patch_task(ids[0], &crate::db::TaskPatch::new().stop_pending(true))
        .await
        .unwrap();

    let claimed = svc.claim_next_backlog_task(epic_id).await.unwrap().unwrap();
    assert_eq!(claimed.id, ids[0]);

    let task = db.get_task(ids[0]).await.unwrap().unwrap();
    assert_eq!(task.live_subagents, 0, "a fresh dispatch starts from zero");
    assert!(!task.stop_pending);
    assert_eq!(
        task.status,
        TaskStatus::Running,
        "dispatch owns the status; the drain path must not flip it to Review"
    );
}

#[tokio::test]
async fn claim_next_backlog_task_epic_not_found() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let err = svc.claim_next_backlog_task(EpicId(999)).await.unwrap_err();
    assert!(matches!(err, ServiceError::NotFound(_)));
}

// -- claim_backlog_task (by id) ---------------------------------------------
//
// The by-id twin of the claim above. Every dispatch entry point takes this
// before it provisions, which is what makes DispatchClaimExclusive
// (docs/specs/dispatch.allium) hold across entry points and not merely
// between chains.

#[tokio::test]
async fn claim_backlog_task_moves_the_task_running_without_provisioning() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    assert!(svc.claim_backlog_task(id).await.unwrap());

    let claimed = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(claimed.status, TaskStatus::Running);
    assert_eq!(
        claimed.sub_status,
        SubStatus::default_for(TaskStatus::Running)
    );
    assert!(
        claimed.last_pre_tool_use_at.is_some(),
        "the claim seeds last_pre_tool_use_at so the tick classifier keeps the task Active"
    );
    assert!(
        claimed.worktree.is_none(),
        "the claim runs ahead of provisioning"
    );
}

#[tokio::test]
async fn dispatch_claim_clears_leftover_subagents_without_flipping() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    // Leftovers from a previous run of this same task.
    db.subagent_start(id, "stale", "old-session", chrono::Utc::now())
        .await
        .unwrap();
    db.patch_task(id, &crate::db::TaskPatch::new().stop_pending(true))
        .await
        .unwrap();

    assert!(svc.claim_backlog_task(id).await.unwrap());

    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.live_subagents, 0, "a fresh dispatch starts from zero");
    assert!(!task.stop_pending);
    assert_eq!(
        task.status,
        TaskStatus::Running,
        "dispatch owns the status; the drain path must not flip it to Review"
    );
}

#[tokio::test]
async fn claim_backlog_task_lost_claim_writes_nothing() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();
    svc.update_task(
        UpdateTaskParams::for_task(id)
            .status(TaskStatus::Review)
            .sub_status(SubStatus::default_for(TaskStatus::Review)),
    )
    .await
    .unwrap();

    assert!(!svc.claim_backlog_task(id).await.unwrap());

    // The extras patch must be gated on the transition winning, or a lost
    // claim would stamp last_pre_tool_use_at on someone else's task.
    let after = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(after.status, TaskStatus::Review);
    assert_eq!(after.sub_status, SubStatus::default_for(TaskStatus::Review));
    assert!(
        after.last_pre_tool_use_at.is_none(),
        "a lost claim must not seed the activity stamp"
    );
}

#[tokio::test]
async fn claim_backlog_task_is_false_for_a_missing_task() {
    let db = test_db().await;
    let svc = task_svc(&db);
    assert!(!svc.claim_backlog_task(TaskId(999_999)).await.unwrap());
}

#[tokio::test]
async fn claim_backlog_task_is_exclusive_under_concurrency() {
    let db = test_db().await;
    let svc = Arc::new(task_svc(&db));
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    let a = {
        let svc = svc.clone();
        tokio::spawn(async move { svc.claim_backlog_task(id).await })
    };
    let b = {
        let svc = svc.clone();
        tokio::spawn(async move { svc.claim_backlog_task(id).await })
    };
    let first = a.await.unwrap().unwrap();
    let second = b.await.unwrap().unwrap();

    assert!(
        first ^ second,
        "exactly one of two concurrent claims on the same task may win"
    );
}

#[tokio::test]
async fn release_claim_undoes_a_by_id_claim() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();
    assert!(svc.claim_backlog_task(id).await.unwrap());

    assert!(svc.release_claim(id).await.unwrap());

    let released = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(released.status, TaskStatus::Backlog);
    assert_eq!(
        released.sub_status,
        SubStatus::default_for(TaskStatus::Backlog)
    );
    assert!(
        released.last_pre_tool_use_at.is_none(),
        "the release clears the stamp the claim seeded"
    );
    assert!(
        svc.claim_backlog_task(id).await.unwrap(),
        "a released task is dispatchable again"
    );
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
        })
        .await
        .unwrap();

    svc.update_task(
        UpdateTaskParams::for_task(id)
            .status(TaskStatus::Running)
            .worktree(FieldUpdate::Set("/repo/.worktrees/feat".into()))
            .tmux_window(crate::service::TmuxWindowUpdate::Set(test_tmux_window(
                "task-1",
            ))),
    )
    .await
    .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.worktree.as_deref(), Some("/repo/.worktrees/feat"));
    assert_eq!(
        task.tmux_window.as_ref().map(|w| w.as_str()),
        Some("task-1")
    );
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
            phoenix: false,
        })
        .await
        .unwrap();

    // Set worktree
    svc.update_task(
        UpdateTaskParams::for_task(id)
            .status(TaskStatus::Running)
            .worktree(FieldUpdate::Set("/repo/.worktrees/feat".into()))
            .tmux_window(crate::service::TmuxWindowUpdate::Set(test_tmux_window(
                "task-1",
            ))),
    )
    .await
    .unwrap();

    // Clear worktree via FieldUpdate::Clear
    svc.update_task(
        UpdateTaskParams::for_task(id)
            .status(TaskStatus::Done)
            .worktree(FieldUpdate::Clear)
            .tmux_window(crate::service::TmuxWindowUpdate::Clear),
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
        phoenix: false,
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
        phoenix: false,
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
            phoenix: false,
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
        phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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

// -- record_peer_message_sent (task #4098: hook-observed native SendMessage) -

#[tokio::test]
async fn record_peer_message_sent_stamps_sender_and_resolved_target() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let sender = svc.create_task(make_task_params("/repo")).await.unwrap();
    let target = svc.create_task(make_task_params("/repo")).await.unwrap();

    svc.record_peer_message_sent(sender, &format!("task-{}", target.0))
        .await
        .unwrap();

    let sender_task = svc.get_task(sender).await.unwrap();
    let target_task = svc.get_task(target).await.unwrap();
    assert!(
        sender_task.last_peer_message_sent_at.is_some(),
        "sender's own row should be stamped"
    );
    assert!(
        target_task.last_peer_message_received_at.is_some(),
        "resolved target's row should be stamped"
    );
    assert!(sender_task.last_peer_message_received_at.is_none());
    assert!(target_task.last_peer_message_sent_at.is_none());
}

#[tokio::test]
async fn record_peer_message_sent_ignores_a_name_outside_dispatchs_own_naming_convention() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let sender = svc.create_task(make_task_params("/repo")).await.unwrap();

    // Not a "task-<id>" name at all — a message to some other local session
    // dispatch didn't launch. Must not error.
    svc.record_peer_message_sent(sender, "my-other-terminal-3f")
        .await
        .unwrap();

    let sender_task = svc.get_task(sender).await.unwrap();
    assert!(
        sender_task.last_peer_message_sent_at.is_some(),
        "sender's own stamp still lands even when the target can't be resolved"
    );
}

#[tokio::test]
async fn record_peer_message_sent_strips_a_disambiguating_ref_suffix() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let sender = svc.create_task(make_task_params("/repo")).await.unwrap();
    let target = svc.create_task(make_task_params("/repo")).await.unwrap();

    // SendMessage's `to` field can carry a disambiguating " [ref]" suffix
    // (per ListAgents' listing format) even though dispatch's task-<id>
    // names are unique by construction and should rarely need one.
    svc.record_peer_message_sent(sender, &format!("task-{} [3fa9c1]", target.0))
        .await
        .unwrap();

    let target_task = svc.get_task(target).await.unwrap();
    assert!(target_task.last_peer_message_received_at.is_some());
}

#[tokio::test]
async fn record_peer_message_sent_ignores_a_task_id_that_no_longer_exists() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let sender = svc.create_task(make_task_params("/repo")).await.unwrap();

    svc.record_peer_message_sent(sender, "task-999999")
        .await
        .unwrap();

    let sender_task = svc.get_task(sender).await.unwrap();
    assert!(sender_task.last_peer_message_sent_at.is_some());
}

#[tokio::test]
async fn record_peer_message_sent_errors_when_sender_is_missing() {
    let db = test_db().await;
    let svc = task_svc(&db);

    let err = svc
        .record_peer_message_sent(TaskId(999999), "task-1")
        .await
        .unwrap_err();
    assert!(matches!(err, ServiceError::NotFound(_)));
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
            phoenix: false,
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

// -- record_subagent_event -------------------------------------------------

#[tokio::test]
async fn stop_with_live_subagents_defers_the_review_flip() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = create_running_task(&svc, SubStatus::Active).await;

    svc.record_subagent_event(
        id,
        SubagentEvent::Start {
            agent_id: "a1".into(),
            session_id: "s1".into(),
        },
    )
    .await
    .unwrap();

    svc.record_hook_event(id, HookEventKind::Stop)
        .await
        .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Running,
        "a Stop with live subagents must not move the task to Review"
    );
    assert!(task.stop_pending, "the deferred flip must be recorded");
}

#[tokio::test]
async fn last_subagent_stop_performs_the_deferred_flip() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = create_running_task(&svc, SubStatus::Active).await;

    svc.record_subagent_event(
        id,
        SubagentEvent::Start {
            agent_id: "a1".into(),
            session_id: "s1".into(),
        },
    )
    .await
    .unwrap();
    svc.record_hook_event(id, HookEventKind::Stop)
        .await
        .unwrap();

    svc.record_subagent_event(
        id,
        SubagentEvent::Stop {
            agent_id: "a1".into(),
            session_id: "s1".into(),
        },
    )
    .await
    .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Review,
        "draining the last subagent flips to Review"
    );
    assert!(!task.stop_pending, "the pending bit must be consumed");
    assert!(task.last_pre_tool_use_at.is_none());
    assert!(task.last_notification_at.is_none());
}

#[tokio::test]
async fn subagent_stop_with_others_still_live_does_not_flip() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = create_running_task(&svc, SubStatus::Active).await;

    for a in ["a1", "a2"] {
        svc.record_subagent_event(
            id,
            SubagentEvent::Start {
                agent_id: a.into(),
                session_id: "s1".into(),
            },
        )
        .await
        .unwrap();
    }
    svc.record_hook_event(id, HookEventKind::Stop)
        .await
        .unwrap();

    svc.record_subagent_event(
        id,
        SubagentEvent::Stop {
            agent_id: "a1".into(),
            session_id: "s1".into(),
        },
    )
    .await
    .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Running,
        "one of two draining must not flip"
    );
    assert!(task.stop_pending);
}

#[tokio::test]
async fn stop_with_no_subagents_flips_immediately_as_before() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = create_running_task(&svc, SubStatus::Active).await;

    svc.record_hook_event(id, HookEventKind::Stop)
        .await
        .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Review,
        "unchanged behaviour when nothing is in flight"
    );
    assert!(!task.stop_pending);
}

#[tokio::test]
async fn subagent_stop_without_a_pending_stop_leaves_status_alone() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = create_running_task(&svc, SubStatus::Active).await;

    svc.record_subagent_event(
        id,
        SubagentEvent::Start {
            agent_id: "a1".into(),
            session_id: "s1".into(),
        },
    )
    .await
    .unwrap();
    svc.record_subagent_event(
        id,
        SubagentEvent::Stop {
            agent_id: "a1".into(),
            session_id: "s1".into(),
        },
    )
    .await
    .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Running,
        "draining without a pending Stop is not a reason to move to Review"
    );
}

/// The draining `Clear` variant — reached only from detach, which owns no
/// status of its own. `SessionStart` uses `clear_subagents_no_drain` instead.
#[tokio::test]
async fn detach_clear_drains_and_performs_a_pending_flip() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = create_running_task(&svc, SubStatus::Active).await;

    svc.record_subagent_event(
        id,
        SubagentEvent::Start {
            agent_id: "a1".into(),
            session_id: "s1".into(),
        },
    )
    .await
    .unwrap();
    svc.record_hook_event(id, HookEventKind::Stop)
        .await
        .unwrap();

    svc.record_subagent_event(id, SubagentEvent::Clear)
        .await
        .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Review,
        "a detach removes the agent that would have drained the count, so the \
         deferred Stop must resolve rather than strand"
    );
    assert_eq!(task.live_subagents, 0);
    assert!(!task.stop_pending);
}

/// The non-draining twin, used by `SessionStart` and by the crash /
/// dispatch-claim paths: entries and `stop_pending` go, status stays.
#[tokio::test]
async fn clear_no_drain_voids_a_pending_stop_without_flipping_to_review() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = create_running_task(&svc, SubStatus::Active).await;

    svc.record_subagent_event(
        id,
        SubagentEvent::Start {
            agent_id: "a1".into(),
            session_id: "s1".into(),
        },
    )
    .await
    .unwrap();
    svc.record_hook_event(id, HookEventKind::Stop)
        .await
        .unwrap();

    svc.clear_subagents_no_drain(id).await.unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Running,
        "a stale Stop from the previous turn must be voided, not applied"
    );
    assert_eq!(task.live_subagents, 0);
    assert!(!task.stop_pending);
}

#[tokio::test]
async fn record_shell_event_start_increments_live_shells() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = create_running_task(&svc, SubStatus::Active).await;

    svc.record_shell_event(
        id,
        ShellEvent::Start {
            shell_id: "bash_1".into(),
            session_id: "sess_1".into(),
        },
    )
    .await
    .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.live_shells, 1);
}

#[tokio::test]
async fn shell_stop_drains_a_deferred_stop_to_review() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = create_running_task(&svc, SubStatus::Active).await;

    svc.record_shell_event(
        id,
        ShellEvent::Start {
            shell_id: "bash_1".into(),
            session_id: "sess_1".into(),
        },
    )
    .await
    .unwrap();
    svc.record_hook_event(id, HookEventKind::Stop)
        .await
        .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Running,
        "Stop must defer, not flip, while a shell is live -- #4187's core bug"
    );

    svc.record_shell_event(
        id,
        ShellEvent::Stop {
            shell_id: "bash_1".into(),
            session_id: "sess_1".into(),
        },
    )
    .await
    .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Review);
}

#[tokio::test]
async fn clear_shells_no_drain_zeroes_live_shells_without_touching_status() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = create_running_task(&svc, SubStatus::Active).await;

    svc.record_shell_event(
        id,
        ShellEvent::Start {
            shell_id: "bash_1".into(),
            session_id: "sess_1".into(),
        },
    )
    .await
    .unwrap();

    svc.clear_shells_no_drain(id).await.unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.live_shells, 0);
    assert_eq!(task.status, TaskStatus::Running);
}

/// The interleaving that used to strand a task: the last `SubagentStop`
/// commits *before* the `Stop`, so the drain sees no pending bit and the Stop
/// then lands on a row whose count has already reached zero. Under the old
/// read-decide-write it wrote `stop_pending = true` there, leaving Running +
/// `stop_pending` + zero live subagents with nothing left to resolve it. The
/// Stop's write is now conditional on the committed count, so it flips instead.
#[tokio::test]
async fn a_stop_landing_after_the_last_drain_flips_instead_of_stranding() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = create_running_task(&svc, SubStatus::Active).await;

    svc.record_subagent_event(
        id,
        SubagentEvent::Start {
            agent_id: "a1".into(),
            session_id: "s1".into(),
        },
    )
    .await
    .unwrap();
    svc.record_subagent_event(
        id,
        SubagentEvent::Stop {
            agent_id: "a1".into(),
            session_id: "s1".into(),
        },
    )
    .await
    .unwrap();
    svc.record_hook_event(id, HookEventKind::Stop)
        .await
        .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Review,
        "the Stop saw a committed count of zero and applied the flip"
    );
    assert!(!task.stop_pending);
    assert_eq!(task.live_subagents, 0);
}

/// Whichever order the two hook processes commit in, the task must end in
/// Review and must never come to rest in the stranded triple. This is the
/// assertion that replaces the retired tick reconciler and its test.
#[tokio::test]
async fn neither_hook_order_can_strand_a_task() {
    for stop_first in [true, false] {
        let db = test_db().await;
        let svc = task_svc(&db);
        let id = create_running_task(&svc, SubStatus::Active).await;

        svc.record_subagent_event(
            id,
            SubagentEvent::Start {
                agent_id: "a1".into(),
                session_id: "s1".into(),
            },
        )
        .await
        .unwrap();

        let drain = SubagentEvent::Stop {
            agent_id: "a1".into(),
            session_id: "s1".into(),
        };
        if stop_first {
            svc.record_hook_event(id, HookEventKind::Stop)
                .await
                .unwrap();
            svc.record_subagent_event(id, drain).await.unwrap();
        } else {
            svc.record_subagent_event(id, drain).await.unwrap();
            svc.record_hook_event(id, HookEventKind::Stop)
                .await
                .unwrap();
        }

        let task = svc.get_task(id).await.unwrap();
        assert_eq!(
            task.status,
            TaskStatus::Review,
            "stop_first = {stop_first} must still end in Review"
        );
        assert!(
            !(task.status == TaskStatus::Running && task.stop_pending && task.live_subagents == 0),
            "stop_first = {stop_first} left the task stranded"
        );
    }
}

/// Park a task in the shape a live fan-out plus a Stop hook leaves behind:
/// Running, one live subagent, and the deferred flip recorded.
async fn running_task_with_a_deferred_stop(svc: &TaskService) -> TaskId {
    let id = create_running_task(svc, SubStatus::Active).await;
    svc.record_subagent_event(
        id,
        SubagentEvent::Start {
            agent_id: "a1".into(),
            session_id: "s1".into(),
        },
    )
    .await
    .unwrap();
    svc.record_hook_event(id, HookEventKind::Stop)
        .await
        .unwrap();
    assert!(svc.get_task(id).await.unwrap().stop_pending);
    id
}

#[tokio::test]
async fn update_task_moving_out_of_running_clears_stop_pending() {
    let db = test_db().await;
    let svc = task_svc(&db);

    for target in [TaskStatus::Review, TaskStatus::Backlog] {
        let id = running_task_with_a_deferred_stop(&svc).await;
        svc.update_task(UpdateTaskParams::for_task(id).status(target))
            .await
            .unwrap();
        let task = svc.get_task(id).await.unwrap();
        assert_eq!(task.status, target);
        assert!(
            !task.stop_pending,
            "leaving Running must void the deferred Stop (target {target:?})"
        );
    }
}

#[tokio::test]
async fn update_task_staying_in_running_keeps_stop_pending() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = running_task_with_a_deferred_stop(&svc).await;

    // A redundant Running write, and a write that carries no status at all:
    // neither ends the turn the Stop was deferred under.
    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Running))
        .await
        .unwrap();
    assert!(svc.get_task(id).await.unwrap().stop_pending);

    svc.update_task(UpdateTaskParams::for_task(id).title("renamed".to_string()))
        .await
        .unwrap();
    assert!(svc.get_task(id).await.unwrap().stop_pending);
}

#[tokio::test]
async fn update_task_to_done_clears_stop_pending() {
    let db = test_db().await;
    let svc = task_svc(&db);

    // Done is not in update_task_moving_out_of_running_clears_stop_pending's
    // loop (that one covers Review/Backlog), and it is the transition the
    // finishing-status write path takes.
    let id = running_task_with_a_deferred_stop(&svc).await;
    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Done))
        .await
        .unwrap();
    assert!(!svc.get_task(id).await.unwrap().stop_pending);
}

#[tokio::test]
async fn close_session_clears_stop_pending() {
    let db = test_db().await;
    let svc = task_svc(&db);

    let id = running_task_with_a_deferred_stop(&svc).await;
    svc.close_session(id, crate::service::CloseSessionOutcome::Done)
        .await
        .unwrap();
    assert!(!svc.get_task(id).await.unwrap().stop_pending);

    let id = running_task_with_a_deferred_stop(&svc).await;
    svc.close_session(
        id,
        crate::service::CloseSessionOutcome::Review {
            pr_url: crate::models::TaskUrl::new(
                "https://github.com/acme/repo/pull/1".to_string(),
                crate::models::UrlType::Pr,
            ),
        },
    )
    .await
    .unwrap();
    assert!(!svc.get_task(id).await.unwrap().stop_pending);
}

/// The reported sequence, end to end (#3847): a Stop deferred during a live
/// fan-out, the card dragged to Review, the subagents finishing, then the card
/// dragged back to Running to keep working. Before the leaving-Running clear
/// the bit survived step two, and the deferred Stop was applied against the
/// new turn — flipping the card straight back to Review, where it stuck,
/// undraggable without a re-dispatch.
#[tokio::test]
async fn moving_back_into_running_after_a_deferred_stop_does_not_re_flip() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = running_task_with_a_deferred_stop(&svc).await;

    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Review))
        .await
        .unwrap();

    // The subagent finishes after the move. The drain path is skipped (status
    // is no longer Running), so nothing here clears the bit either.
    svc.record_subagent_event(
        id,
        SubagentEvent::Stop {
            agent_id: "a1".into(),
            session_id: "s1".into(),
        },
    )
    .await
    .unwrap();

    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Running))
        .await
        .unwrap();

    // A further drain must find nothing to apply: the bit was voided on the
    // way out of Running, so the new turn does not inherit the old Stop.
    svc.record_subagent_event(
        id,
        SubagentEvent::Stop {
            agent_id: "a1".into(),
            session_id: "s1".into(),
        },
    )
    .await
    .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Running,
        "the card must stay where the human put it"
    );
    assert!(!task.stop_pending);
}

#[tokio::test]
async fn clear_no_drain_on_a_missing_task_is_not_found() {
    let db = test_db().await;
    let svc = task_svc(&db);
    assert!(matches!(
        svc.clear_subagents_no_drain(TaskId(9999)).await,
        Err(crate::service::ServiceError::NotFound(_))
    ));
}

/// The clock is injected and advanced rather than left on the wall clock: the
/// void is conditional on the prompt having fired *after* the Stop it
/// supersedes, so back-to-back calls sharing an instant would tie — and a tie
/// preserves. A human resuming is never that fast; a test is.
#[tokio::test]
async fn user_prompt_submit_voids_a_pending_stop() {
    let db = test_db().await;
    let (svc, clock) = task_svc_with_fixed_clock(&db);
    let id = create_running_task(&svc, SubStatus::Active).await;

    svc.record_subagent_event(
        id,
        SubagentEvent::Start {
            agent_id: "a1".into(),
            session_id: "s1".into(),
        },
    )
    .await
    .unwrap();
    svc.record_hook_event(id, HookEventKind::Stop)
        .await
        .unwrap();
    clock.advance(chrono::Duration::seconds(1));
    svc.record_hook_event(id, HookEventKind::UserPromptSubmit)
        .await
        .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert!(
        !task.stop_pending,
        "a human resuming voids the deferred flip"
    );
    assert_eq!(task.status, TaskStatus::Running);
}

/// The late-write race the conditional clear exists for, end to end. Every
/// Claude Code hook is its own process, so a `UserPromptSubmit` write can land
/// after the `Stop` of the very turn that prompt started. Voiding there would
/// delete a Stop no drain can re-create; the task would sit in Running with no
/// agent left to move it. The clock runs backwards between the two calls to
/// stage exactly that ordering without depending on scheduling.
#[tokio::test]
async fn user_prompt_submit_that_lands_after_the_stop_it_races_keeps_it_pending() {
    let db = test_db().await;
    let (svc, clock) = task_svc_with_fixed_clock(&db);
    let id = create_running_task(&svc, SubStatus::Active).await;
    svc.record_subagent_event(
        id,
        SubagentEvent::Start {
            agent_id: "a1".into(),
            session_id: "s1".into(),
        },
    )
    .await
    .unwrap();

    // The Stop fires at t2, after the prompt fired at t0 …
    svc.record_hook_event(id, HookEventKind::Stop)
        .await
        .unwrap();
    assert!(svc.get_task(id).await.unwrap().stop_pending);

    // … and only now does the prompt's own write land.
    clock.advance(-chrono::Duration::seconds(1));
    svc.record_hook_event(id, HookEventKind::UserPromptSubmit)
        .await
        .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert!(
        task.stop_pending,
        "the Stop belongs to the turn this prompt started — voiding it would \
         strand the task in Running"
    );
    assert_eq!(task.status, TaskStatus::Running);

    // And the drain still owes the flip.
    svc.record_subagent_event(
        id,
        SubagentEvent::Stop {
            agent_id: "a1".into(),
            session_id: "s1".into(),
        },
    )
    .await
    .unwrap();
    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Review);
    assert!(!task.stop_pending);
}

/// Regression test: `live_subagents` is hook-owned, like
/// `last_pre_tool_use_at`. A stale in-memory snapshot riding a generic
/// `update_task` call must not zero it.
#[tokio::test]
async fn generic_persist_does_not_clobber_the_subagent_count() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = create_running_task(&svc, SubStatus::Active).await;

    let snapshot = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(snapshot.live_subagents, 0);

    db.subagent_start(id, "a1", "s1", chrono::Utc::now())
        .await
        .unwrap();

    // Persist the *stale* snapshot, which still says zero.
    svc.update_task(
        UpdateTaskParams::for_task(id)
            .status(snapshot.status)
            .sub_status(snapshot.sub_status),
    )
    .await
    .unwrap();

    let reread = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(
        reread.live_subagents, 1,
        "a generic persist must not overwrite the hook-owned count"
    );
}

/// A `Start` can never drain: it inserts its row before the recount, so the
/// resulting count is never zero. That is why the `Start` arm needs neither the
/// task's status nor its `stop_pending`, and why it must not reach the drain
/// path — asserted here against the hardest input, a task already stranded in
/// Running + `stop_pending` + zero live subagents.
#[tokio::test]
async fn subagent_start_never_drains_a_pending_stop() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = create_running_task(&svc, SubStatus::Active).await;
    // The stranded state directly, rather than reproducing the interleaving via
    // hook events — this test is about the subagent arm, not how it got there.
    db.patch_task(id, &crate::db::TaskPatch::new().stop_pending(true))
        .await
        .unwrap();

    svc.record_subagent_event(
        id,
        SubagentEvent::Start {
            agent_id: "a2".into(),
            session_id: "s1".into(),
        },
    )
    .await
    .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Running,
        "a Start must never apply a deferred Stop"
    );
    assert!(task.stop_pending, "nor consume the pending bit");
    assert_eq!(task.live_subagents, 1);
}

#[tokio::test]
async fn record_subagent_event_unknown_task_returns_not_found() {
    let db = test_db().await;
    let svc = task_svc(&db);
    // All three variants share the contract but no longer share one existence
    // check: Start uses `task_exists`, Stop and Clear read the row they need for
    // the drain decision. A fourth variant must remember to check too.
    for event in [
        SubagentEvent::Start {
            agent_id: "a1".into(),
            session_id: "s1".into(),
        },
        SubagentEvent::Stop {
            agent_id: "a1".into(),
            session_id: "s1".into(),
        },
        SubagentEvent::Clear,
    ] {
        let err = svc
            .record_subagent_event(TaskId(99_999), event.clone())
            .await
            .unwrap_err();
        assert!(
            matches!(err, ServiceError::NotFound(_)),
            "got {err:?} for {event:?}"
        );
    }
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
    let svc = TaskService::new(db.clone(), crate::process::MockProcessRunner::unused());

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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
// -- PhoenixRespawn --------------------------------------------------------
//
// A phoenix task recreates itself on completion, and the flag MOVES to the
// copy. See the `== Phoenix ==` section and `PhoenixRespawn` in
// docs/specs/tasks.allium — each test below names the clause it pins.

/// A backlog phoenix task with every inheritable field set to something
/// distinguishable, so `WhatTheSuccessorInherits` can be asserted field by
/// field rather than on a couple of representatives.
async fn phoenix_task(db: &Arc<dyn db::TaskStore>, epic_id: Option<EpicId>) -> TaskId {
    let svc = task_svc(db);
    let id = svc
        .create_task(CreateTaskParams {
            title: "Weekly dep audit".into(),
            description: "check every direct dependency".into(),
            repo_path: "/repo".to_string(),
            plan_path: Some("/repo/docs/plans/audit.md".to_string()),
            epic_id,
            sort_order: None,
            tag: Some(TaskTag::Chore),
            base_branch: Some("develop".to_string()),
            wrap_up_mode: Some(crate::models::WrapUpMode::Done),
            auto_run_plan: true,
            phoenix: true,
        })
        .await
        .unwrap();
    id
}

/// The successor of `predecessor`, or `None` when nothing was respawned.
async fn successor_of(
    db: &Arc<dyn db::TaskStore>,
    predecessor: TaskId,
) -> Option<crate::models::Task> {
    task_svc(db)
        .list_tasks(ListTasksFilter::default())
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.id != predecessor && t.status == TaskStatus::Backlog)
}

#[tokio::test]
async fn phoenix_task_entering_done_spawns_a_backlog_successor() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = phoenix_task(&db, None).await;

    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Done))
        .await
        .unwrap();

    let successor = successor_of(&db, id)
        .await
        .expect("entering Done must create a fresh backlog copy");
    assert_eq!(successor.status, TaskStatus::Backlog);
    assert!(successor.phoenix, "the flag moves to the successor");
    assert_eq!(
        successor.sub_status,
        SubStatus::default_for(TaskStatus::Backlog)
    );
}

/// `TheFlagIsTheReceipt`: the predecessor's flag is cleared exactly when the
/// successor lands, so the flag's own absence records that it already fired.
#[tokio::test]
async fn a_successful_respawn_clears_the_predecessors_flag() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = phoenix_task(&db, None).await;

    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Done))
        .await
        .unwrap();

    let predecessor = svc.get_task(id).await.unwrap();
    assert_eq!(predecessor.status, TaskStatus::Done);
    assert!(
        !predecessor.phoenix,
        "a task that respawned is no longer the live phoenix"
    );
    assert!(
        !predecessor.respawn_failed(),
        "a cleared flag is what makes respawn_failed false in Done"
    );
}

/// `WhatTheSuccessorInherits`: everything the operator configured, and nothing
/// the finished run produced.
#[tokio::test]
async fn the_successor_inherits_the_operators_settings_and_none_of_the_run() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let epic = epic_svc(&db)
        .create_epic(CreateEpicParams {
            title: "recurring work".into(),
            description: "".into(),
            sort_order: None,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
        })
        .await
        .unwrap();
    let id = phoenix_task(&db, Some(epic.id)).await;
    svc.update_task(
        UpdateTaskParams::for_task(id)
            .status(TaskStatus::Running)
            .worktree(FieldUpdate::Set("/repo/.worktrees/wt".to_string()))
            .tmux_window(crate::service::TmuxWindowUpdate::Set(test_tmux_window(
                "task-x",
            ))),
    )
    .await
    .unwrap();

    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Done))
        .await
        .unwrap();

    let s = successor_of(&db, id).await.unwrap();
    assert_eq!(s.title, "Weekly dep audit", "title is carried verbatim");
    assert_eq!(s.description, "check every direct dependency");
    assert_eq!(s.repo_path, "/repo");
    assert_eq!(s.tag, Some(TaskTag::Chore));
    assert_eq!(s.base_branch, "develop");
    assert_eq!(s.wrap_up_mode, Some(crate::models::WrapUpMode::Done));
    assert_eq!(s.plan_path.as_deref(), Some("/repo/docs/plans/audit.md"));
    assert!(s.auto_run_plan);
    assert_eq!(s.epic_id, Some(epic.id), "EpicMembershipIsInherited");

    assert!(
        s.worktree.is_none(),
        "the worktree belongs to the finished run"
    );
    assert!(s.tmux_window.is_none());
    assert!(s.url.is_none());
    assert!(s.external_id.is_none());
    assert!(
        s.sort_order.is_none(),
        "the copy sorts by its own id, at the bottom of backlog"
    );
}

/// Labels land as part of the successor's own creation, not a follow-up patch
/// that could fail independently and silently drop them.
#[tokio::test]
async fn the_successor_inherits_labels() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = phoenix_task(&db, None).await;
    let labels = vec!["scala-common".to_string(), "security".to_string()];
    db.patch_task(id, &db::TaskPatch::new().labels(&labels))
        .await
        .unwrap();

    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Done))
        .await
        .unwrap();

    let s = successor_of(&db, id).await.unwrap();
    assert_eq!(s.labels, labels);
}

#[tokio::test]
async fn an_ordinary_task_entering_done_spawns_nothing() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Done))
        .await
        .unwrap();

    assert!(successor_of(&db, id).await.is_none());
}

/// `TheFlagIsTheReceipt`, second property: a Done -> Review -> Done round-trip
/// cannot duplicate, because the first pass cleared the flag.
#[tokio::test]
async fn re_entering_done_does_not_spawn_a_second_successor() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = phoenix_task(&db, None).await;

    for status in [TaskStatus::Done, TaskStatus::Review, TaskStatus::Done] {
        svc.update_task(UpdateTaskParams::for_task(id).status(status))
            .await
            .unwrap();
    }

    let backlog = svc
        .list_tasks(ListTasksFilter {
            statuses: Some(vec![TaskStatus::Backlog]),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(backlog.len(), 1, "exactly one successor, got {backlog:?}");
}

/// `transitions_to` fires on an actual change of value only — the same
/// property `DetectTaskDone` relies on.
#[tokio::test]
async fn rewriting_done_over_done_does_not_respawn() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = phoenix_task(&db, None).await;
    // Land in Done without the flag ever being set, so the respawn this test
    // rules out could only come from the no-op rewrite below.
    svc.update_task(UpdateTaskParams::for_task(id).phoenix(false))
        .await
        .unwrap();
    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Done))
        .await
        .unwrap();
    svc.update_task(UpdateTaskParams::for_task(id).phoenix(true))
        .await
        .unwrap();

    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Done))
        .await
        .unwrap();

    assert!(
        successor_of(&db, id).await.is_none(),
        "Done -> Done is not a transition into Done"
    );
}

/// `FeedTasksAreExempt`: a feed epic recreates and reconciles its own rows, so
/// a phoenix copy would either duplicate one or be deleted as stale.
#[tokio::test]
async fn a_feed_owned_task_does_not_respawn() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = phoenix_task(&db, None).await;
    db.patch_task(id, &db::TaskPatch::new().external_id(Some("pr-42")))
        .await
        .unwrap();

    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Done))
        .await
        .unwrap();

    assert!(successor_of(&db, id).await.is_none());
    assert!(
        svc.get_task(id).await.unwrap().phoenix,
        "the flag is ignored, not consumed — nothing respawned to consume it"
    );
}

#[tokio::test]
async fn close_session_done_respawns_a_phoenix_task() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = phoenix_task(&db, None).await;
    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Running))
        .await
        .unwrap();

    svc.close_session(id, crate::service::CloseSessionOutcome::Done)
        .await
        .unwrap();

    let successor = successor_of(&db, id)
        .await
        .expect("the rebase/done branch of ExitSession lands in Done");
    assert!(successor.phoenix);
    assert!(!svc.get_task(id).await.unwrap().phoenix);
}

/// The `pr` branch lands in Review, not Done. The respawn waits for the PR to
/// merge or for a human to complete the task.
#[tokio::test]
async fn close_session_pr_does_not_respawn() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = phoenix_task(&db, None).await;
    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Running))
        .await
        .unwrap();

    svc.close_session(
        id,
        crate::service::CloseSessionOutcome::Review {
            pr_url: crate::models::TaskUrl::new(
                "https://github.com/o/r/pull/1".to_string(),
                crate::models::UrlType::Pr,
            ),
        },
    )
    .await
    .unwrap();

    assert!(successor_of(&db, id).await.is_none());
    assert!(
        svc.get_task(id).await.unwrap().phoenix,
        "the recurrence survives into Review"
    );
}

/// `DoneOutranksTheRespawn`. The half that is enforced by construction:
/// `respawn_phoenix` returns `()`, so no respawn outcome can reach the caller's
/// `Result` — which is what keeps `close_session`'s `Err` meaning exactly "the
/// terminal write did not land", the property its caller gates the tmux
/// teardown on.
///
/// The half this test observes: a close whose respawn is skipped still reports
/// success and still moves the task, so the completion never waits on the
/// recurrence. Feed ownership is the skip used here because it is the one the
/// spec makes reachable without a fault-injecting store — a genuine create
/// failure takes the same `return`, one branch further down.
#[tokio::test]
async fn a_skipped_respawn_does_not_contaminate_the_close_result() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = phoenix_task(&db, None).await;
    db.patch_task(id, &db::TaskPatch::new().external_id(Some("pr-42")))
        .await
        .unwrap();
    svc.update_task(
        UpdateTaskParams::for_task(id)
            .status(TaskStatus::Running)
            .tmux_window(crate::service::TmuxWindowUpdate::Set(test_tmux_window(
                "task-x",
            ))),
    )
    .await
    .unwrap();

    let closed = svc
        .close_session(id, crate::service::CloseSessionOutcome::Done)
        .await
        .expect("the close succeeded; the respawn is a follow-on, not part of it");

    assert_eq!(closed.window.as_ref().map(|w| w.as_str()), Some("task-x"));
    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Done, "the completion sticks");
    assert!(
        task.respawn_failed(),
        "a phoenix flag surviving in Done is the visible 'did not respawn' state"
    );
}
