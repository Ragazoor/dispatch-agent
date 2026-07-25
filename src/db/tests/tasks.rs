#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;

#[tokio::test]
async fn create_and_get() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "My Task",
            description: "A description",
            repo_path: "/repo/path",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let task = db.get_task(id).await.unwrap().expect("task should exist");
    assert_eq!(task.id, id);
    assert_eq!(task.title, "My Task");
    assert_eq!(task.description, "A description");
    assert_eq!(task.repo_path, "/repo/path");
    assert_eq!(task.status, TaskStatus::Backlog);
    assert!(task.worktree.is_none());
    assert!(task.tmux_window.is_none());
}

#[tokio::test]
async fn list_all() {
    let db = in_memory_db().await;
    db.create_task(CreateTaskRequest {
        title: "Task A",
        description: "desc",
        repo_path: "/a",
        plan: None,
        status: TaskStatus::Backlog,
        base_branch: "main",
        epic_id: None,
        sort_order: None,
        tag: None,
        wrap_up_mode: None,
        auto_run_plan: false,
    })
    .await
    .unwrap();
    db.create_task(CreateTaskRequest {
        title: "Task B",
        description: "desc",
        repo_path: "/b",
        plan: None,
        status: TaskStatus::Backlog,
        base_branch: "main",
        epic_id: None,
        sort_order: None,
        tag: None,
        wrap_up_mode: None,
        auto_run_plan: false,
    })
    .await
    .unwrap();
    db.create_task(CreateTaskRequest {
        title: "Task C",
        description: "desc",
        repo_path: "/c",
        plan: None,
        status: TaskStatus::Backlog,
        base_branch: "main",
        epic_id: None,
        sort_order: None,
        tag: None,
        wrap_up_mode: None,
        auto_run_plan: false,
    })
    .await
    .unwrap();
    let tasks = db.list_all().await.unwrap();
    assert_eq!(tasks.len(), 3);
    assert_eq!(tasks[0].title, "Task A");
    assert_eq!(tasks[1].title, "Task B");
    assert_eq!(tasks[2].title, "Task C");
}

#[tokio::test]
async fn list_by_status() {
    let db = in_memory_db().await;
    let id1 = db
        .create_task(CreateTaskRequest {
            title: "Task A",
            description: "desc",
            repo_path: "/a",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let id2 = db
        .create_task(CreateTaskRequest {
            title: "Task B",
            description: "desc",
            repo_path: "/b",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    db.create_task(CreateTaskRequest {
        title: "Task C",
        description: "desc",
        repo_path: "/c",
        plan: None,
        status: TaskStatus::Backlog,
        base_branch: "main",
        epic_id: None,
        sort_order: None,
        tag: None,
        wrap_up_mode: None,
        auto_run_plan: false,
    })
    .await
    .unwrap();

    db.patch_task(id1, &TaskPatch::new().status(TaskStatus::Running))
        .await
        .unwrap();
    db.patch_task(id2, &TaskPatch::new().status(TaskStatus::Running))
        .await
        .unwrap();

    let running = db.list_by_status(TaskStatus::Running).await.unwrap();
    assert_eq!(running.len(), 2);

    let backlog = db.list_by_status(TaskStatus::Backlog).await.unwrap();
    assert_eq!(backlog.len(), 1);
    assert_eq!(backlog[0].title, "Task C");
}

#[tokio::test]
async fn get_nonexistent() {
    let db = in_memory_db().await;
    let result = db.get_task(TaskId(9999)).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn create_task_with_plan() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "Planned Task",
            description: "desc",
            repo_path: "/repo",
            plan: Some("docs/plan.md"),
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.plan_path.as_deref(), Some("docs/plan.md"));
}

#[tokio::test]
async fn create_task_without_plan() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "Simple Task",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let task = db.get_task(id).await.unwrap().unwrap();
    assert!(task.plan_path.is_none());
}

#[tokio::test]
async fn find_task_by_plan_returns_match() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "Planned",
            description: "desc",
            repo_path: "/repo",
            plan: Some("/plans/my-plan.md"),
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let found = db.find_task_by_plan("/plans/my-plan.md").await.unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().id, id);
}

#[tokio::test]
async fn find_task_by_plan_returns_none_when_no_match() {
    let db = in_memory_db().await;
    db.create_task(CreateTaskRequest {
        title: "Other",
        description: "desc",
        repo_path: "/repo",
        plan: Some("/plans/other.md"),
        status: TaskStatus::Backlog,
        base_branch: "main",
        epic_id: None,
        sort_order: None,
        tag: None,
        wrap_up_mode: None,
        auto_run_plan: false,
    })
    .await
    .unwrap();

    let found = db.find_task_by_plan("/plans/nonexistent.md").await.unwrap();
    assert!(found.is_none());
}

#[tokio::test]
async fn find_task_by_plan_ignores_tasks_without_plan() {
    let db = in_memory_db().await;
    db.create_task(CreateTaskRequest {
        title: "No Plan",
        description: "desc",
        repo_path: "/repo",
        plan: None,
        status: TaskStatus::Backlog,
        base_branch: "main",
        epic_id: None,
        sort_order: None,
        tag: None,
        wrap_up_mode: None,
        auto_run_plan: false,
    })
    .await
    .unwrap();

    let found = db.find_task_by_plan("/plans/any.md").await.unwrap();
    assert!(found.is_none());
}

#[tokio::test]
async fn create_task_returning_returns_full_task() {
    let db = in_memory_db().await;
    let task = create_task_returning(&db, "Title", "Desc", "/repo", None, TaskStatus::Backlog)
        .await
        .unwrap();
    assert_eq!(task.title, "Title");
    assert_eq!(task.description, "Desc");
    assert_eq!(task.repo_path, "/repo");
    assert_eq!(task.status, TaskStatus::Backlog);
    assert!(task.worktree.is_none());
    assert!(task.tmux_window.is_none());
    assert!(task.plan_path.is_none());
}

#[tokio::test]
async fn create_task_returning_with_plan() {
    let db = in_memory_db().await;
    let task = create_task_returning(&db, "T", "D", "/r", Some("plan.md"), TaskStatus::Backlog)
        .await
        .unwrap();
    assert_eq!(task.plan_path.as_deref(), Some("plan.md"));
    assert_eq!(task.status, TaskStatus::Backlog);
}

#[tokio::test]
async fn patch_task_applies_all_fields() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "title",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let patch = TaskPatch::new()
        .status(TaskStatus::Running)
        .plan_path(Some("plan.md"))
        .title("new title");
    db.patch_task(id, &patch).await.unwrap();
    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.plan_path.as_deref(), Some("plan.md"));
    assert_eq!(task.title, "new title");
    assert_eq!(task.description, "desc"); // unchanged
}

#[tokio::test]
async fn patch_task_none_fields_unchanged() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "title",
            description: "desc",
            repo_path: "/repo",
            plan: Some("plan.md"),
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let patch = TaskPatch::new();
    db.patch_task(id, &patch).await.unwrap();
    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.title, "title");
    assert_eq!(task.plan_path.as_deref(), Some("plan.md"));
    assert_eq!(task.status, TaskStatus::Running);
}

#[tokio::test]
async fn create_task_defaults_labels_to_empty() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "t",
            description: "",
            repo_path: "/r",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.labels, Vec::<String>::new());
}

#[tokio::test]
async fn patch_task_sets_labels() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "t",
            description: "",
            repo_path: "/r",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let labels = vec!["scala-common".to_string(), "security".to_string()];
    db.patch_task(id, &TaskPatch::new().labels(&labels))
        .await
        .unwrap();
    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.labels, labels);
}

#[tokio::test]
async fn patch_task_clears_labels_to_empty() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "t",
            description: "",
            repo_path: "/r",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let initial = vec!["one".to_string()];
    db.patch_task(id, &TaskPatch::new().labels(&initial))
        .await
        .unwrap();
    let empty: Vec<String> = Vec::new();
    db.patch_task(id, &TaskPatch::new().labels(&empty))
        .await
        .unwrap();
    let task = db.get_task(id).await.unwrap().unwrap();
    assert!(task.labels.is_empty());

    // Verify the column actually contains '[]', not NULL.
    let task_id = id.0;
    let raw: String = db
        .db_call(move |conn| {
            conn.query_row(
                "SELECT labels FROM tasks WHERE id = ?1",
                rusqlite::params![task_id],
                |r| r.get(0),
            )
            .map_err(anyhow::Error::from)
        })
        .await
        .unwrap();
    assert_eq!(raw, "[]");
}

#[tokio::test]
async fn patch_task_round_trips_hook_event_timestamps() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "t",
            description: "",
            repo_path: "/r",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let task = db.get_task(id).await.unwrap().unwrap();
    assert!(task.last_pre_tool_use_at.is_none());
    assert!(task.last_notification_at.is_none());

    let pre_tool = chrono::Utc::now();
    let notification = pre_tool - chrono::Duration::seconds(30);
    db.patch_task(
        id,
        &TaskPatch::new()
            .last_pre_tool_use_at(Some(pre_tool))
            .last_notification_at(Some(notification)),
    )
    .await
    .unwrap();

    let task = db.get_task(id).await.unwrap().unwrap();
    let stored_pre = task.last_pre_tool_use_at.expect("pre_tool_use written");
    let stored_notif = task.last_notification_at.expect("notification written");
    assert!(
        (stored_pre - pre_tool).num_seconds().abs() <= 1,
        "stored pre_tool_use {stored_pre} too far from {pre_tool}"
    );
    assert!(
        (stored_notif - notification).num_seconds().abs() <= 1,
        "stored notification {stored_notif} too far from {notification}"
    );
}

#[tokio::test]
async fn patch_task_none_preserves_labels() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "t",
            description: "",
            repo_path: "/r",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let labels = vec!["keep-me".to_string()];
    db.patch_task(id, &TaskPatch::new().labels(&labels))
        .await
        .unwrap();
    // Patching unrelated field must not touch labels.
    db.patch_task(id, &TaskPatch::new().title("new"))
        .await
        .unwrap();
    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.labels, labels);
}

#[tokio::test]
async fn list_all_errors_on_corrupt_labels_json() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "t",
            description: "",
            repo_path: "/r",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let task_id = id.0;
    db.db_call(move |conn| {
        conn.execute(
            "UPDATE tasks SET labels = ?1 WHERE id = ?2",
            rusqlite::params!["{not json", task_id],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let result = db.list_all().await;
    assert!(
        result.is_err(),
        "expected Err on corrupt labels JSON, got {:?}",
        result
    );
}

#[tokio::test]
async fn patch_task_sets_tag() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "title",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    db.patch_task(id, &TaskPatch::new().tag(Some(TaskTag::Bug)))
        .await
        .unwrap();
    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.tag, Some(TaskTag::Bug));
}

#[tokio::test]
async fn patch_task_clears_tag() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "title",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    db.patch_task(id, &TaskPatch::new().tag(Some(TaskTag::Feature)))
        .await
        .unwrap();
    db.patch_task(id, &TaskPatch::new().tag(None))
        .await
        .unwrap();
    let task = db.get_task(id).await.unwrap().unwrap();
    assert!(task.tag.is_none());
}

#[tokio::test]
async fn has_other_tasks_with_worktree_returns_false_when_no_others() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "Task A",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    db.patch_task(
        id,
        &TaskPatch::new()
            .status(TaskStatus::Running)
            .worktree(Some("/repo/.worktrees/1-task-a"))
            .tmux_window(Some("task-1")),
    )
    .await
    .unwrap();

    assert!(!db
        .has_other_tasks_with_worktree("/repo/.worktrees/1-task-a", id)
        .await
        .unwrap());
}

#[tokio::test]
async fn has_other_tasks_with_worktree_returns_true_when_shared() {
    let db = in_memory_db().await;
    let id1 = db
        .create_task(CreateTaskRequest {
            title: "Task A",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let id2 = db
        .create_task(CreateTaskRequest {
            title: "Task B",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    db.patch_task(
        id1,
        &TaskPatch::new()
            .status(TaskStatus::Running)
            .worktree(Some("/repo/.worktrees/1-task-a"))
            .tmux_window(Some("task-1")),
    )
    .await
    .unwrap();
    db.patch_task(
        id2,
        &TaskPatch::new()
            .status(TaskStatus::Running)
            .worktree(Some("/repo/.worktrees/1-task-a"))
            .tmux_window(Some("task-1")),
    )
    .await
    .unwrap();

    assert!(db
        .has_other_tasks_with_worktree("/repo/.worktrees/1-task-a", id1)
        .await
        .unwrap());
    assert!(db
        .has_other_tasks_with_worktree("/repo/.worktrees/1-task-a", id2)
        .await
        .unwrap());
}

#[tokio::test]
async fn has_other_tasks_with_worktree_ignores_done_tasks() {
    let db = in_memory_db().await;
    let id1 = db
        .create_task(CreateTaskRequest {
            title: "Task A",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let id2 = db
        .create_task(CreateTaskRequest {
            title: "Task B",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    db.patch_task(
        id1,
        &TaskPatch::new()
            .status(TaskStatus::Running)
            .worktree(Some("/repo/.worktrees/1-task-a"))
            .tmux_window(Some("task-1")),
    )
    .await
    .unwrap();
    db.patch_task(
        id2,
        &TaskPatch::new()
            .status(TaskStatus::Done)
            .worktree(Some("/repo/.worktrees/1-task-a"))
            .tmux_window(Some("task-1")),
    )
    .await
    .unwrap();

    assert!(!db
        .has_other_tasks_with_worktree("/repo/.worktrees/1-task-a", id1)
        .await
        .unwrap());
}

#[tokio::test]
async fn patch_task_clears_plan() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "title",
            description: "desc",
            repo_path: "/repo",
            plan: Some("plan.md"),
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let patch = TaskPatch::new().plan_path(None);
    db.patch_task(id, &patch).await.unwrap();
    let task = db.get_task(id).await.unwrap().unwrap();
    assert!(task.plan_path.is_none());
}

#[tokio::test]
async fn patch_task_sets_dispatch_fields() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "title",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let patch = TaskPatch::new()
        .worktree(Some("/repo/.worktrees/1-my-task"))
        .tmux_window(Some("session:1-my-task"));
    db.patch_task(id, &patch).await.unwrap();
    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.worktree.as_deref(), Some("/repo/.worktrees/1-my-task"));
    assert_eq!(task.tmux_window.as_deref(), Some("session:1-my-task"));
}

#[tokio::test]
async fn patch_task_clears_dispatch_fields() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "title",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    // Set dispatch fields first
    let patch = TaskPatch::new()
        .worktree(Some("/repo/.worktrees/1-my-task"))
        .tmux_window(Some("session:1-my-task"));
    db.patch_task(id, &patch).await.unwrap();
    let task = db.get_task(id).await.unwrap().unwrap();
    assert!(task.worktree.is_some());
    assert!(task.tmux_window.is_some());

    // Clear them
    let patch = TaskPatch::new().worktree(None).tmux_window(None);
    db.patch_task(id, &patch).await.unwrap();
    let task = db.get_task(id).await.unwrap().unwrap();
    assert!(task.worktree.is_none());
    assert!(task.tmux_window.is_none());
}

#[tokio::test]
async fn patch_task_status_and_dispatch_together() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "title",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let patch = TaskPatch::new()
        .status(TaskStatus::Running)
        .worktree(Some("/repo/.worktrees/1-my-task"))
        .tmux_window(Some("session:1-my-task"));
    db.patch_task(id, &patch).await.unwrap();
    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.worktree.as_deref(), Some("/repo/.worktrees/1-my-task"));
    assert_eq!(task.tmux_window.as_deref(), Some("session:1-my-task"));
}

#[tokio::test]
async fn task_patch_status_does_not_set_sub_status() {
    // status() no longer auto-sets sub_status; patch_task handles the default
    let patch = TaskPatch::new().status(TaskStatus::Review);
    assert_eq!(patch.status, Some(TaskStatus::Review));
    assert_eq!(patch.sub_status, None);
}

#[tokio::test]
async fn task_patch_status_and_sub_status_independent() {
    // Order of builder calls doesn't matter — both fields are set independently
    let patch_a = TaskPatch::new()
        .status(TaskStatus::Running)
        .sub_status(SubStatus::NeedsInput);
    let patch_b = TaskPatch::new()
        .sub_status(SubStatus::NeedsInput)
        .status(TaskStatus::Running);
    assert_eq!(patch_a.status, Some(TaskStatus::Running));
    assert_eq!(patch_a.sub_status, Some(SubStatus::NeedsInput));
    assert_eq!(patch_b.status, Some(TaskStatus::Running));
    assert_eq!(patch_b.sub_status, Some(SubStatus::NeedsInput));
}

#[tokio::test]
async fn patch_task_status_change_resets_sub_status_in_db() {
    // End-to-end: after a status-only patch, sub_status in DB reflects the new default
    let db = Database::open_in_memory().await.unwrap();
    let id = db
        .create_task(CreateTaskRequest {
            title: "T",
            description: "d",
            repo_path: "/r",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    db.patch_task(id, &TaskPatch::default().sub_status(SubStatus::Stale))
        .await
        .unwrap();

    db.patch_task(id, &TaskPatch::default().status(TaskStatus::Review))
        .await
        .unwrap();

    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Review);
    assert_eq!(task.sub_status, SubStatus::AwaitingReview);
}

#[tokio::test]
async fn update_status_if_matching() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "Task",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let updated = db
        .update_status_if(id, TaskStatus::Review, TaskStatus::Running)
        .await
        .unwrap();
    assert!(updated, "should update when current status matches");

    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Review);
}

#[tokio::test]
async fn update_status_if_not_matching() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "Task",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Done,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let updated = db
        .update_status_if(id, TaskStatus::Review, TaskStatus::Running)
        .await
        .unwrap();
    assert!(
        !updated,
        "should not update when current status doesn't match"
    );

    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Done, "status should be unchanged");
}

#[tokio::test]
async fn update_status_if_nonexistent() {
    let db = in_memory_db().await;
    let updated = db
        .update_status_if(TaskId(9999), TaskStatus::Review, TaskStatus::Running)
        .await
        .unwrap();
    assert!(!updated, "should return false for nonexistent task");
}

#[tokio::test]
async fn task_roundtrip_with_pr_fields() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "PR task",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let url = crate::models::TaskUrl::new(
        "https://github.com/org/repo/pull/42",
        crate::models::UrlType::Pr,
    );
    db.patch_task(id, &TaskPatch::new().url(Some(&url)))
        .await
        .unwrap();

    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.url, Some(url));
}

#[tokio::test]
async fn task_pr_fields_default_to_none() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "No PR",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let task = db.get_task(id).await.unwrap().unwrap();
    assert!(task.url.is_none());
}

#[tokio::test]
async fn patch_sets_and_clears_typed_url_together() {
    use crate::models::{TaskUrl, UrlType};
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "t",
            description: "d",
            repo_path: "/r",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    // Set
    let url = TaskUrl::new("https://github.com/o/r/pull/9", UrlType::Pr);
    db.patch_task(id, &TaskPatch::new().url(Some(&url)))
        .await
        .unwrap();
    let t = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(
        t.url,
        Some(TaskUrl::new("https://github.com/o/r/pull/9", UrlType::Pr))
    );

    // Clear (both columns null)
    db.patch_task(id, &TaskPatch::new().url(None))
        .await
        .unwrap();
    let t = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(t.url, None);
}

#[tokio::test]
async fn patch_task_sets_sort_order() {
    let db = Database::open_in_memory().await.unwrap();
    let id = db
        .create_task(CreateTaskRequest {
            title: "T",
            description: "d",
            repo_path: "/r",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    db.patch_task(id, &TaskPatch::new().sort_order(Some(500)))
        .await
        .unwrap();
    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.sort_order, Some(500));
}

#[tokio::test]
async fn patch_task_clears_sort_order() {
    let db = Database::open_in_memory().await.unwrap();
    let id = db
        .create_task(CreateTaskRequest {
            title: "T",
            description: "d",
            repo_path: "/r",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    db.patch_task(id, &TaskPatch::new().sort_order(Some(100)))
        .await
        .unwrap();
    db.patch_task(id, &TaskPatch::new().sort_order(None))
        .await
        .unwrap();
    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.sort_order, None);
}

#[tokio::test]
async fn task_sub_status_persists() {
    let db = Database::open_in_memory().await.unwrap();
    let id = db
        .create_task(CreateTaskRequest {
            title: "Test",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    db.patch_task(id, &TaskPatch::default().sub_status(SubStatus::Stale))
        .await
        .unwrap();
    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.sub_status, SubStatus::Stale);
}

#[tokio::test]
async fn task_sub_status_defaults_to_none() {
    let db = Database::open_in_memory().await.unwrap();
    let id = db
        .create_task(CreateTaskRequest {
            title: "Test",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.sub_status, SubStatus::None);
}

#[tokio::test]
async fn create_task_sets_default_sub_status_for_running() {
    // create_task with status=Running must produce sub_status=active, not 'none'
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "T",
            description: "d",
            repo_path: "/r",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.sub_status, SubStatus::Active);
}

#[tokio::test]
async fn create_task_sets_default_sub_status_for_backlog() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "T",
            description: "d",
            repo_path: "/r",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.sub_status, SubStatus::None);
}

#[tokio::test]
async fn create_task_with_epic_sort_tag_single_insert() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let id = db
        .create_task(CreateTaskRequest {
            title: "T",
            description: "d",
            repo_path: "/r",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: Some(epic.id),
            sort_order: Some(7),
            tag: Some(TaskTag::Bug),
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.epic_id, Some(epic.id));
    assert_eq!(task.sort_order, Some(7));
    assert_eq!(task.tag, Some(TaskTag::Bug));
}

#[tokio::test]
async fn update_status_if_resets_sub_status_to_default() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "T",
            description: "d",
            repo_path: "/r",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    db.patch_task(id, &TaskPatch::default().sub_status(SubStatus::Stale))
        .await
        .unwrap();

    let updated = db
        .update_status_if(id, TaskStatus::Review, TaskStatus::Running)
        .await
        .unwrap();
    assert!(updated, "should have updated");

    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Review);
    assert_eq!(task.sub_status, SubStatus::AwaitingReview); // default for review
}

#[tokio::test]
async fn update_status_if_leaves_sub_status_unchanged_when_condition_fails() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "T",
            description: "d",
            repo_path: "/r",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    db.patch_task(id, &TaskPatch::default().sub_status(SubStatus::Active))
        .await
        .unwrap();

    let updated = db
        .update_status_if(id, TaskStatus::Review, TaskStatus::Backlog)
        .await
        .unwrap();
    assert!(!updated, "condition was wrong, should not have updated");

    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.sub_status, SubStatus::Active); // unchanged
}

#[tokio::test]
async fn check_constraint_rejects_review_with_active_substatus() {
    let db = Database::open_in_memory().await.unwrap();
    let rejected = db
        .db_call(|conn| {
            conn.execute(
                "INSERT INTO tasks (title, description, repo_path, status, sub_status) \
                 VALUES ('T', 'D', '/r', 'backlog', 'none')",
                [],
            )?;
            let result = conn.execute(
                "UPDATE tasks SET status = 'review', sub_status = 'active' WHERE id = 1",
                [],
            );
            Ok(result.is_err())
        })
        .await
        .unwrap();
    assert!(rejected, "CHECK constraint must reject (review, active)");
}

#[tokio::test]
async fn check_constraint_accepts_review_with_awaiting_review() {
    let db = Database::open_in_memory().await.unwrap();
    let accepted = db
        .db_call(|conn| {
            conn.execute(
                "INSERT INTO tasks (title, description, repo_path, status, sub_status) \
                 VALUES ('T', 'D', '/r', 'backlog', 'none')",
                [],
            )?;
            let result = conn.execute(
                "UPDATE tasks SET status = 'review', sub_status = 'awaiting_review' WHERE id = 1",
                [],
            );
            Ok(result.is_ok())
        })
        .await
        .unwrap();
    assert!(accepted, "valid pair should be accepted");
}

// ---------------------------------------------------------------------------
// Query coverage: delete_task
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_task_removes_task() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "Doomed",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    assert!(db.get_task(id).await.unwrap().is_some());

    db.delete_task(id).await.unwrap();
    assert!(db.get_task(id).await.unwrap().is_none());
}

#[tokio::test]
async fn delete_task_nonexistent_errors() {
    let db = in_memory_db().await;
    let result = db.delete_task(TaskId(9999));
    assert!(result.await.is_err());
}

// ---------------------------------------------------------------------------
// upsert_feed_tasks
// ---------------------------------------------------------------------------

fn make_feed_item(external_id: &str, title: &str) -> crate::models::FeedItem {
    crate::models::FeedItem {
        external_id: external_id.to_string(),
        title: title.to_string(),
        description: "desc".to_string(),
        url: String::new(),
        url_type: None,
        status: TaskStatus::Backlog,
        tag: crate::models::TaskTag::Bug,
        labels: Vec::new(),
        sort_order: None,
        signals: vec![],
        wrap_up_mode: None,
    }
}

/// Build a parallel vec of "main" base branches for tests that don't
/// exercise the per-task base_branch path.
fn main_branches(n: usize) -> Vec<String> {
    vec!["main".to_string(); n]
}

#[tokio::test]
async fn upsert_feed_tasks_creates_tasks() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let items = vec![
        make_feed_item("ext-1", "Task One"),
        make_feed_item("ext-2", "Task Two"),
    ];
    let repo_paths = vec!["/repo".to_string(), "/repo".to_string()];
    let branches = main_branches(items.len());

    db.upsert_feed_tasks(epic.id, &items, &repo_paths, &branches)
        .await
        .unwrap();

    let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    assert_eq!(tasks.len(), 2);
    let mut titles: Vec<&str> = tasks.iter().map(|t| t.title.as_str()).collect();
    titles.sort();
    assert_eq!(titles, vec!["Task One", "Task Two"]);
    assert!(tasks.iter().all(|t| t.status == TaskStatus::Backlog));
    assert!(tasks
        .iter()
        .all(|t| t.external_id.as_deref() == Some("ext-1")
            || t.external_id.as_deref() == Some("ext-2")));
}

#[tokio::test]
async fn upsert_feed_tasks_rejects_mismatched_slice_lengths() {
    // The three slices are parallel-to-items by contract. A length mismatch
    // would silently truncate via zip and drop feed items, so it must error
    // explicitly instead.
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let items = vec![
        make_feed_item("ext-1", "Task One"),
        make_feed_item("ext-2", "Task Two"),
    ];

    // repo_paths shorter than items
    let err = db
        .upsert_feed_tasks(epic.id, &items, &["/repo".to_string()], &main_branches(2))
        .await
        .expect_err("mismatched repo_paths length must error");
    assert!(
        err.to_string().contains("length"),
        "error should mention length mismatch, got: {err}"
    );

    // base_branches shorter than items
    let err = db
        .upsert_feed_tasks(
            epic.id,
            &items,
            &["/repo".to_string(), "/repo".to_string()],
            &main_branches(1),
        )
        .await
        .expect_err("mismatched base_branches length must error");
    assert!(
        err.to_string().contains("length"),
        "error should mention length mismatch, got: {err}"
    );

    // No tasks should have been written on either failed call.
    let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    assert!(tasks.is_empty(), "no tasks should be written on mismatch");
}

#[tokio::test]
async fn upsert_feed_tasks_idempotent() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let items = vec![make_feed_item("ext-1", "Task One")];
    let repo_paths = vec!["/repo".to_string()];
    let branches = main_branches(items.len());

    db.upsert_feed_tasks(epic.id, &items, &repo_paths, &branches)
        .await
        .unwrap();
    db.upsert_feed_tasks(epic.id, &items, &repo_paths, &branches)
        .await
        .unwrap();

    let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    assert_eq!(tasks.len(), 1, "second call should not create duplicate");
    assert_eq!(tasks[0].title, "Task One");
}

#[tokio::test]
async fn upsert_feed_tasks_preserves_status() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let items = vec![make_feed_item("ext-1", "Original Title")];

    db.upsert_feed_tasks(epic.id, &items, &["/repo".to_string()], &main_branches(1))
        .await
        .unwrap();

    // Simulate user moving task to Running
    let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    db.patch_task(tasks[0].id, &TaskPatch::new().status(TaskStatus::Running))
        .await
        .unwrap();

    // Re-run upsert with updated title and different status
    let updated = vec![crate::models::FeedItem {
        external_id: "ext-1".to_string(),
        title: "Updated Title".to_string(),
        description: "new desc".to_string(),
        url: String::new(),
        url_type: None,
        status: TaskStatus::Done, // feed says done; user status should be preserved
        tag: crate::models::TaskTag::Bug,
        labels: Vec::new(),
        sort_order: None,
        signals: vec![],
        wrap_up_mode: None,
    }];
    db.upsert_feed_tasks(epic.id, &updated, &["/repo".to_string()], &main_branches(1))
        .await
        .unwrap();

    let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].title, "Updated Title", "title should be updated");
    assert_eq!(
        tasks[0].description, "new desc",
        "description should be updated"
    );
    assert_eq!(
        tasks[0].status,
        TaskStatus::Running,
        "user-managed status must be preserved"
    );
}

#[tokio::test]
async fn upsert_feed_tasks_adds_new_items() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();

    db.upsert_feed_tasks(
        epic.id,
        &[make_feed_item("ext-1", "First")],
        &["/repo".to_string()],
        &main_branches(1),
    )
    .await
    .unwrap();

    db.upsert_feed_tasks(
        epic.id,
        &[
            make_feed_item("ext-1", "First"),
            make_feed_item("ext-2", "Second"),
        ],
        &["/repo".to_string(), "/repo".to_string()],
        &main_branches(2),
    )
    .await
    .unwrap();

    let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    assert_eq!(tasks.len(), 2, "new item should be created on second call");
}

/// Insert a `reviews-parent` epic and a `my-reviews` role sub-epic beneath
/// it, returning the sub-epic's id. Plain `create_epic` always defaults to
/// `feed_role = 'none'`, which the v72/v76 subtree-uniqueness triggers
/// ignore entirely — so tests that must exercise those triggers (rather than
/// accidentally bypass them) set up the epic tree via raw SQL instead.
async fn create_role_sub_epic(db: &Database) -> EpicId {
    db.db_call(|conn| {
        conn.execute_batch(
            "INSERT INTO epics (id, title, description, status, feed_role, origin)
             VALUES (1, 'PR Reviews', '', 'backlog', 'reviews-parent', 'manual');
             INSERT INTO epics (id, title, description, status, feed_role, origin, parent_epic_id)
             VALUES (2, 'My Reviews', '', 'backlog', 'my-reviews', 'manual', 1);",
        )
        .map_err(anyhow::Error::from)
    })
    .await
    .unwrap();
    EpicId(2)
}

/// Regression test for the v72 trigger false-positiving on the ON CONFLICT
/// DO UPDATE path: re-upserting an already-tracked task into the SAME role
/// sub-epic must not error, since it resolves via the existing (epic_id,
/// external_id) row, not a genuine cross-epic duplicate.
#[tokio::test]
async fn upsert_feed_tasks_reupsert_into_role_sub_epic_does_not_error() {
    let db = in_memory_db().await;
    let epic_id = create_role_sub_epic(&db).await;
    let items = vec![make_feed_item("ext-1", "Task One")];
    let repo_paths = vec!["/repo".to_string()];
    let branches = main_branches(1);

    db.upsert_feed_tasks(epic_id, &items, &repo_paths, &branches)
        .await
        .unwrap();
    db.upsert_feed_tasks(epic_id, &items, &repo_paths, &branches)
        .await
        .expect("re-upserting an already-tracked task in the same role sub-epic must not error");

    let tasks = db.list_tasks_for_epic(epic_id).await.unwrap();
    assert_eq!(tasks.len(), 1, "second call should not create a duplicate");
}

/// A batch containing one already-tracked item and one brand-new item, both
/// targeting the same role sub-epic, must fully succeed: the pre-existing
/// item's self-conflict must not abort the whole transaction and silently
/// drop the new item alongside it.
#[tokio::test]
async fn upsert_feed_tasks_mixed_batch_existing_and_new_item_in_role_sub_epic_succeeds() {
    let db = in_memory_db().await;
    let epic_id = create_role_sub_epic(&db).await;

    db.upsert_feed_tasks(
        epic_id,
        &[make_feed_item("ext-1", "First")],
        &["/repo".to_string()],
        &main_branches(1),
    )
    .await
    .unwrap();

    db.upsert_feed_tasks(
        epic_id,
        &[
            make_feed_item("ext-1", "First"),
            make_feed_item("ext-2", "Second"),
        ],
        &["/repo".to_string(), "/repo".to_string()],
        &main_branches(2),
    )
    .await
    .expect("a batch mixing an already-tracked item with a brand-new item must fully succeed");

    let tasks = db.list_tasks_for_epic(epic_id).await.unwrap();
    assert_eq!(
        tasks.len(),
        2,
        "both the existing and the new item must be present"
    );
}

#[tokio::test]
async fn upsert_feed_tasks_removes_stale_items() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();

    // First fetch: two items
    db.upsert_feed_tasks(
        epic.id,
        &[
            make_feed_item("ext-1", "First"),
            make_feed_item("ext-2", "Second"),
        ],
        &["/repo".to_string(), "/repo".to_string()],
        &main_branches(2),
    )
    .await
    .unwrap();
    assert_eq!(db.list_tasks_for_epic(epic.id).await.unwrap().len(), 2);

    // Second fetch: only ext-1 remains in the feed
    db.upsert_feed_tasks(
        epic.id,
        &[make_feed_item("ext-1", "First")],
        &["/repo".to_string()],
        &main_branches(1),
    )
    .await
    .unwrap();

    let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    assert_eq!(tasks.len(), 1, "stale feed task should be removed");
    assert_eq!(tasks[0].external_id.as_deref(), Some("ext-1"));
}

/// `delete_stale_subtree_feed_tasks` deletes feed tasks (external_id set) across
/// the WHOLE subtree of a parent epic, except those in the keep-set. It must:
/// - keep a feed task whose external_id is in the keep-set (even in another child);
/// - delete a feed task absent from the keep-set;
/// - preserve manual tasks (external_id IS NULL) regardless of the keep-set.
#[tokio::test]
async fn delete_stale_subtree_feed_tasks_scopes_to_subtree_and_keeps_set() {
    let db = in_memory_db().await;
    let parent = db.create_epic("Reviews", "", None).await.unwrap();
    let child_a = db.create_epic("A", "", Some(parent.id)).await.unwrap();
    let child_b = db.create_epic("B", "", Some(parent.id)).await.unwrap();

    // Feed tasks in both children.
    db.upsert_feed_tasks(
        child_a.id,
        &[make_feed_item("keep-1", "Kept")],
        &["/repo".to_string()],
        &main_branches(1),
    )
    .await
    .unwrap();
    db.upsert_feed_tasks(
        child_b.id,
        &[make_feed_item("stale-1", "Stale")],
        &["/repo".to_string()],
        &main_branches(1),
    )
    .await
    .unwrap();

    // A manual task (no external_id) in child_a must survive.
    let manual_id = db
        .create_task(CreateTaskRequest {
            title: "Manual",
            description: "",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: Some(child_a.id),
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    db.delete_stale_subtree_feed_tasks(parent.id, &["keep-1".to_string()])
        .await
        .unwrap();

    let a_tasks = db.list_tasks_for_epic(child_a.id).await.unwrap();
    assert_eq!(a_tasks.len(), 2, "kept feed task + manual task survive");
    assert!(a_tasks
        .iter()
        .any(|t| t.external_id.as_deref() == Some("keep-1")));
    assert!(a_tasks.iter().any(|t| t.id == manual_id));

    let b_tasks = db.list_tasks_for_epic(child_b.id).await.unwrap();
    assert!(
        b_tasks.is_empty(),
        "stale feed task absent from keep-set is deleted, got {b_tasks:?}"
    );
}

#[tokio::test]
async fn upsert_feed_tasks_uses_resolved_repo_path() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let items = vec![make_feed_item("ext-1", "Task One")];
    let repo_paths = vec!["/resolved/local/repo".to_string()];
    let branches = main_branches(items.len());

    db.upsert_feed_tasks(epic.id, &items, &repo_paths, &branches)
        .await
        .unwrap();

    let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    assert_eq!(tasks[0].repo_path, "/resolved/local/repo");
}

#[tokio::test]
async fn upsert_feed_tasks_stores_empty_sentinel_when_unresolved() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let items = vec![make_feed_item("ext-1", "Task One")];
    let repo_paths = vec!["".to_string()];
    let branches = main_branches(items.len());

    db.upsert_feed_tasks(epic.id, &items, &repo_paths, &branches)
        .await
        .unwrap();

    let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    assert_eq!(tasks[0].repo_path, "");
}

#[tokio::test]
async fn upsert_feed_tasks_on_conflict_does_not_update_repo_path() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let items = vec![make_feed_item("ext-1", "Original")];

    // First upsert: resolved path stored
    db.upsert_feed_tasks(
        epic.id,
        &items,
        &["/first/path".to_string()],
        &main_branches(1),
    )
    .await
    .unwrap();
    let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    assert_eq!(tasks[0].repo_path, "/first/path");

    // Second upsert: different path provided — ON CONFLICT should NOT update repo_path
    let updated = vec![crate::models::FeedItem {
        external_id: "ext-1".to_string(),
        title: "Updated Title".to_string(),
        description: "new desc".to_string(),
        url: String::new(),
        url_type: None,
        status: TaskStatus::Backlog,
        tag: crate::models::TaskTag::Bug,
        labels: Vec::new(),
        sort_order: None,
        signals: vec![],
        wrap_up_mode: None,
    }];
    db.upsert_feed_tasks(
        epic.id,
        &updated,
        &["/second/path".to_string()],
        &main_branches(1),
    )
    .await
    .unwrap();

    let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    assert_eq!(tasks[0].title, "Updated Title");
    assert_eq!(
        tasks[0].repo_path, "/first/path",
        "repo_path must not be updated on conflict"
    );
}

#[tokio::test]
async fn upsert_feed_tasks_mixed_batch_resolved_and_unresolved() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let items = vec![
        make_feed_item("ext-1", "Resolved Task"),
        make_feed_item("ext-2", "Unresolved Task"),
    ];
    let repo_paths = vec!["/matched/local/path".to_string(), "".to_string()];
    let branches = main_branches(items.len());

    db.upsert_feed_tasks(epic.id, &items, &repo_paths, &branches)
        .await
        .unwrap();

    let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    let resolved = tasks
        .iter()
        .find(|t| t.external_id.as_deref() == Some("ext-1"))
        .unwrap();
    let unresolved = tasks
        .iter()
        .find(|t| t.external_id.as_deref() == Some("ext-2"))
        .unwrap();
    assert_eq!(resolved.repo_path, "/matched/local/path");
    assert_eq!(unresolved.repo_path, "");
}

#[tokio::test]
async fn upsert_feed_tasks_stores_per_task_base_branch() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let items = vec![
        make_feed_item("ext-1", "Master Task"),
        make_feed_item("ext-2", "Develop Task"),
        make_feed_item("ext-3", "Main Task"),
    ];
    let repo_paths = vec![
        "/repo-a".to_string(),
        "/repo-b".to_string(),
        "/repo-c".to_string(),
    ];
    let base_branches = vec![
        "master".to_string(),
        "develop".to_string(),
        "main".to_string(),
    ];

    db.upsert_feed_tasks(epic.id, &items, &repo_paths, &base_branches)
        .await
        .unwrap();

    let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    let by_ext = |ext: &str| {
        tasks
            .iter()
            .find(|t| t.external_id.as_deref() == Some(ext))
            .unwrap()
    };
    assert_eq!(by_ext("ext-1").base_branch, "master");
    assert_eq!(by_ext("ext-2").base_branch, "develop");
    assert_eq!(by_ext("ext-3").base_branch, "main");
}

#[tokio::test]
async fn upsert_feed_tasks_does_not_remove_manual_tasks() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();

    // Manually created task linked to the epic (no external_id)
    let manual_task_id = db
        .create_task(CreateTaskRequest {
            title: "Manual",
            description: "",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: Some(epic.id),
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    // Feed fetch with one item
    db.upsert_feed_tasks(
        epic.id,
        &[make_feed_item("ext-1", "Feed Task")],
        &["/repo".to_string()],
        &main_branches(1),
    )
    .await
    .unwrap();

    // Feed fetch returns nothing — only manual task should survive
    db.upsert_feed_tasks(epic.id, &[], &[], &[]).await.unwrap();

    let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    assert_eq!(
        tasks.len(),
        1,
        "manual task should survive empty feed fetch"
    );
    assert_eq!(tasks[0].id, manual_task_id);
}

#[tokio::test]
async fn upsert_feed_tasks_persists_tag() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let items = vec![crate::models::FeedItem {
        external_id: "ext-1".to_string(),
        title: "Tagged".to_string(),
        description: "".to_string(),
        url: String::new(),
        url_type: None,
        status: TaskStatus::Backlog,
        tag: crate::models::TaskTag::PrReview,
        labels: Vec::new(),
        sort_order: None,
        signals: vec![],
        wrap_up_mode: None,
    }];

    db.upsert_feed_tasks(epic.id, &items, &["/repo".to_string()], &main_branches(1))
        .await
        .unwrap();

    let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].tag, Some(crate::models::TaskTag::PrReview));
}

#[tokio::test]
async fn upsert_feed_tasks_updates_tag_on_conflict() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let initial = vec![crate::models::FeedItem {
        external_id: "ext-1".to_string(),
        title: "T".to_string(),
        description: "".to_string(),
        url: String::new(),
        url_type: None,
        status: TaskStatus::Backlog,
        tag: crate::models::TaskTag::PrReview,
        labels: Vec::new(),
        sort_order: None,
        signals: vec![],
        wrap_up_mode: None,
    }];
    db.upsert_feed_tasks(epic.id, &initial, &["/repo".to_string()], &main_branches(1))
        .await
        .unwrap();

    // Re-emit the same item with a different tag — feed is the source of truth.
    let updated = vec![crate::models::FeedItem {
        external_id: "ext-1".to_string(),
        title: "T".to_string(),
        description: "".to_string(),
        url: String::new(),
        url_type: None,
        status: TaskStatus::Backlog,
        tag: crate::models::TaskTag::Fix,
        labels: Vec::new(),
        sort_order: None,
        signals: vec![],
        wrap_up_mode: None,
    }];
    db.upsert_feed_tasks(epic.id, &updated, &["/repo".to_string()], &main_branches(1))
        .await
        .unwrap();

    let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].tag, Some(crate::models::TaskTag::Fix));
}

#[tokio::test]
async fn feed_item_legacy_json_deserializes_with_default_labels_and_sort_order() {
    // Wire-compat: scripts written before labels/sort_order existed must still
    // parse. Both fields are #[serde(default)].
    let legacy_json = r#"{
        "external_id": "ext-1",
        "title": "Legacy",
        "description": "",
        "url": "",
        "status": "backlog",
        "tag": "bug"
    }"#;
    let item: crate::models::FeedItem = serde_json::from_str(legacy_json).unwrap();
    assert!(item.labels.is_empty());
    assert_eq!(item.sort_order, None);
    // wrap_up_mode is #[serde(default)]: absent -> None.
    assert_eq!(item.wrap_up_mode, None);
}

#[tokio::test]
async fn feed_item_deserializes_wrap_up_mode() {
    // A feed script may declare wrap_up_mode; "pr" parses to WrapUpMode::Pr
    // (WrapUpMode derives Deserialize with rename_all = "lowercase").
    let json = r#"{
        "external_id": "cve:org/repo#1",
        "title": "[CRITICAL] repo: CVE-1",
        "description": "",
        "status": "backlog",
        "tag": "fix",
        "wrap_up_mode": "pr"
    }"#;
    let item: crate::models::FeedItem = serde_json::from_str(json).unwrap();
    assert_eq!(item.wrap_up_mode, Some(crate::models::WrapUpMode::Pr));
}

#[tokio::test]
async fn upsert_feed_tasks_writes_labels_and_sort_order_on_insert() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let items = vec![crate::models::FeedItem {
        external_id: "ext-1".to_string(),
        title: "CRITICAL CVE-1234".to_string(),
        description: "".to_string(),
        url: String::new(),
        url_type: None,
        status: TaskStatus::Backlog,
        tag: crate::models::TaskTag::Fix,
        labels: vec!["scala-common".to_string()],
        sort_order: Some(1),
        signals: vec![],
        wrap_up_mode: None,
    }];
    db.upsert_feed_tasks(epic.id, &items, &["/repo".to_string()], &main_branches(1))
        .await
        .unwrap();

    let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].labels, vec!["scala-common".to_string()]);
    assert_eq!(tasks[0].sort_order, Some(1));
}

#[tokio::test]
async fn upsert_feed_tasks_replaces_labels_and_sort_order_on_conflict() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let initial = vec![crate::models::FeedItem {
        external_id: "ext-1".to_string(),
        title: "T".to_string(),
        description: "".to_string(),
        url: String::new(),
        url_type: None,
        status: TaskStatus::Backlog,
        tag: crate::models::TaskTag::Fix,
        labels: vec!["repo-a".to_string()],
        sort_order: Some(3),
        signals: vec![],
        wrap_up_mode: None,
    }];
    db.upsert_feed_tasks(epic.id, &initial, &["/repo".to_string()], &main_branches(1))
        .await
        .unwrap();
    // Simulate user moving the task — status & repo_path must be preserved.
    let task_id = db.list_tasks_for_epic(epic.id).await.unwrap()[0].id;
    db.patch_task(
        task_id,
        &TaskPatch::new()
            .status(TaskStatus::Running)
            .repo_path("/manually-fixed"),
    )
    .await
    .unwrap();

    let updated = vec![crate::models::FeedItem {
        external_id: "ext-1".to_string(),
        title: "T".to_string(),
        description: "".to_string(),
        url: String::new(),
        url_type: None,
        status: TaskStatus::Backlog,
        tag: crate::models::TaskTag::Fix,
        labels: vec!["repo-a".to_string(), "security".to_string()],
        sort_order: Some(1),
        signals: vec![],
        wrap_up_mode: None,
    }];
    db.upsert_feed_tasks(epic.id, &updated, &["/repo".to_string()], &main_branches(1))
        .await
        .unwrap();

    let task = db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(
        task.labels,
        vec!["repo-a".to_string(), "security".to_string()],
        "labels are feed-controlled and replaced on conflict"
    );
    assert_eq!(
        task.sort_order,
        Some(1),
        "sort_order is replaced on conflict"
    );
    // User-owned fields preserved.
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.repo_path, "/manually-fixed");
}

#[tokio::test]
async fn upsert_feed_tasks_sets_wrap_up_mode_on_insert() {
    let db = in_memory_db().await;
    let epic = db.create_epic("CVE", "", None).await.unwrap();
    let items = vec![
        crate::models::FeedItem {
            sort_order: Some(1),
            wrap_up_mode: Some(crate::models::WrapUpMode::Pr),
            ..make_feed_item("cve:org/repo#1", "[CRITICAL] repo: CVE-1")
        },
        crate::models::FeedItem {
            sort_order: Some(2),
            // Omitted by the script -> stays NULL on the task.
            wrap_up_mode: None,
            ..make_feed_item("cve:org/repo#2", "[LOW] repo: CVE-2")
        },
    ];
    db.upsert_feed_tasks(
        epic.id,
        &items,
        &["/repo".to_string(), "/repo".to_string()],
        &main_branches(2),
    )
    .await
    .unwrap();

    let mut tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    tasks.sort_by_key(|t| t.sort_order);
    assert_eq!(tasks.len(), 2);
    assert_eq!(
        tasks[0].wrap_up_mode,
        Some(crate::models::WrapUpMode::Pr),
        "declared wrap_up_mode is applied on insert"
    );
    assert_eq!(
        tasks[1].wrap_up_mode, None,
        "omitted wrap_up_mode leaves the task's value NULL"
    );
}

#[tokio::test]
async fn upsert_feed_tasks_preserves_wrap_up_mode_on_conflict() {
    let db = in_memory_db().await;
    let epic = db.create_epic("CVE", "", None).await.unwrap();
    let initial = vec![crate::models::FeedItem {
        wrap_up_mode: Some(crate::models::WrapUpMode::Pr),
        ..make_feed_item("cve:org/repo#1", "T")
    }];
    db.upsert_feed_tasks(epic.id, &initial, &["/repo".to_string()], &main_branches(1))
        .await
        .unwrap();

    // User changes the wrap-up choice manually.
    let task_id = db.list_tasks_for_epic(epic.id).await.unwrap()[0].id;
    db.patch_task(
        task_id,
        &TaskPatch::new().wrap_up_mode(Some(crate::models::WrapUpMode::Rebase)),
    )
    .await
    .unwrap();

    // Feed re-polls the same alert, still declaring "pr".
    db.upsert_feed_tasks(epic.id, &initial, &["/repo".to_string()], &main_branches(1))
        .await
        .unwrap();

    let task = db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(
        task.wrap_up_mode,
        Some(crate::models::WrapUpMode::Rebase),
        "wrap_up_mode is insert-only; a user's manual change survives feed refreshes"
    );
}

#[tokio::test]
async fn upsert_feed_tasks_sets_pr_url_from_item_url_on_insert() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let items = vec![
        crate::models::FeedItem {
            external_id: "dep:org/repo#42".to_string(),
            title: "#42 Bump foo".to_string(),
            description: "".to_string(),
            url: "https://github.com/org/repo/pull/42".to_string(),
            url_type: None,
            status: TaskStatus::Backlog,
            tag: crate::models::TaskTag::PrReview,
            labels: vec![],
            sort_order: None,
            signals: vec![],
            wrap_up_mode: None,
        },
        crate::models::FeedItem {
            external_id: "dep:org/repo#43".to_string(),
            title: "#43 Bump bar".to_string(),
            description: "".to_string(),
            url: "https://github.com/org/repo/pull/43".to_string(),
            url_type: None,
            status: TaskStatus::Backlog,
            tag: crate::models::TaskTag::Dependabot,
            labels: vec![],
            sort_order: None,
            signals: vec![],
            wrap_up_mode: None,
        },
        crate::models::FeedItem {
            external_id: "cve:GHSA-xxxx".to_string(),
            title: "CRITICAL CVE-1234".to_string(),
            description: "".to_string(),
            url: "https://github.com/org/repo/security/advisories/GHSA-xxxx".to_string(),
            url_type: None,
            status: TaskStatus::Backlog,
            tag: crate::models::TaskTag::Fix,
            labels: vec![],
            sort_order: None,
            signals: vec![],
            wrap_up_mode: None,
        },
    ];
    db.upsert_feed_tasks(
        epic.id,
        &items,
        &vec!["/repo".to_string(); 3],
        &main_branches(3),
    )
    .await
    .unwrap();

    let mut tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    tasks.sort_by(|a, b| a.external_id.cmp(&b.external_id));
    assert_eq!(tasks.len(), 3);
    assert_eq!(
        tasks[0].url.as_ref().map(|u| u.url.as_str()),
        Some("https://github.com/org/repo/security/advisories/GHSA-xxxx"),
        "non-empty url copied to url regardless of tag (Fix)"
    );
    assert_eq!(
        tasks[0].url.as_ref().map(|u| u.url_type),
        Some(crate::models::UrlType::Other),
        "non-PR/issue url inferred as other"
    );
    assert_eq!(
        tasks[1].url.as_ref().map(|u| u.url.as_str()),
        Some("https://github.com/org/repo/pull/42"),
        "PrReview items keep url-on-insert"
    );
    assert_eq!(
        tasks[1].url.as_ref().map(|u| u.url_type),
        Some(crate::models::UrlType::Pr),
        "pull url inferred as pr"
    );
    assert_eq!(
        tasks[2].url.as_ref().map(|u| u.url.as_str()),
        Some("https://github.com/org/repo/pull/43"),
        "Dependabot items get url-on-insert"
    );
}

#[tokio::test]
async fn upsert_feed_tasks_leaves_pr_url_null_when_item_url_empty() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let items = vec![crate::models::FeedItem {
        external_id: "ext-no-url".to_string(),
        title: "no url".to_string(),
        description: "".to_string(),
        url: "".to_string(),
        url_type: None,
        status: TaskStatus::Backlog,
        tag: crate::models::TaskTag::Dependabot,
        labels: vec![],
        sort_order: None,
        signals: vec![],
        wrap_up_mode: None,
    }];
    db.upsert_feed_tasks(epic.id, &items, &["/repo".to_string()], &main_branches(1))
        .await
        .unwrap();

    let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert!(tasks[0].url.is_none());
}

#[tokio::test]
async fn upsert_feed_tasks_backfills_null_pr_url_on_conflict() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    // First emission: no URL — task created with url = NULL.
    let initial = vec![crate::models::FeedItem {
        external_id: "dep:org/repo#42".to_string(),
        title: "#42 Bump foo".to_string(),
        description: "".to_string(),
        url: "".to_string(),
        url_type: None,
        status: TaskStatus::Backlog,
        tag: crate::models::TaskTag::Dependabot,
        labels: vec![],
        sort_order: None,
        signals: vec![],
        wrap_up_mode: None,
    }];
    db.upsert_feed_tasks(epic.id, &initial, &["/repo".to_string()], &main_branches(1))
        .await
        .unwrap();
    let task_id = db.list_tasks_for_epic(epic.id).await.unwrap()[0].id;
    assert!(
        db.get_task(task_id).await.unwrap().unwrap().url.is_none(),
        "precondition: url is null after first upsert"
    );

    // Second emission: same external_id but now with a URL.
    let refreshed = vec![crate::models::FeedItem {
        url: "https://github.com/org/repo/pull/42".to_string(),
        ..initial[0].clone()
    }];
    db.upsert_feed_tasks(
        epic.id,
        &refreshed,
        &["/repo".to_string()],
        &main_branches(1),
    )
    .await
    .unwrap();

    let task = db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(
        task.url.as_ref().map(|u| u.url.as_str()),
        Some("https://github.com/org/repo/pull/42"),
        "null url is backfilled from item.url on conflict"
    );
    assert_eq!(
        task.url.as_ref().map(|u| u.url_type),
        Some(crate::models::UrlType::Pr),
        "backfilled url_type is inferred"
    );
}

#[tokio::test]
async fn upsert_feed_tasks_preserves_pr_url_on_conflict() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let initial = vec![crate::models::FeedItem {
        external_id: "dep:org/repo#42".to_string(),
        title: "#42 Bump foo".to_string(),
        description: "".to_string(),
        url: "https://github.com/org/repo/pull/42".to_string(),
        url_type: None,
        status: TaskStatus::Backlog,
        tag: crate::models::TaskTag::PrReview,
        labels: vec![],
        sort_order: None,
        signals: vec![],
        wrap_up_mode: None,
    }];
    db.upsert_feed_tasks(epic.id, &initial, &["/repo".to_string()], &main_branches(1))
        .await
        .unwrap();
    let task_id = db.list_tasks_for_epic(epic.id).await.unwrap()[0].id;
    let manual = crate::models::TaskUrl::new(
        "https://github.com/org/repo/pull/999",
        crate::models::UrlType::Pr,
    );
    db.patch_task(task_id, &TaskPatch::new().url(Some(&manual)))
        .await
        .unwrap();

    // Re-run upsert; url on the existing task must not be overwritten.
    db.upsert_feed_tasks(epic.id, &initial, &["/repo".to_string()], &main_branches(1))
        .await
        .unwrap();

    let task = db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(
        task.url.as_ref().map(|u| u.url.as_str()),
        Some("https://github.com/org/repo/pull/999")
    );
}

#[tokio::test]
async fn feed_upsert_infers_url_type_and_backfills_atomically() {
    use crate::models::{TaskUrl, UrlType};
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();

    let feed_item = |external_id: &str, url: &str| crate::models::FeedItem {
        external_id: external_id.to_string(),
        title: "t".to_string(),
        description: "".to_string(),
        url: url.to_string(),
        url_type: None,
        status: TaskStatus::Backlog,
        tag: crate::models::TaskTag::Dependabot,
        labels: vec![],
        sort_order: None,
        signals: vec![],
        wrap_up_mode: None,
    };
    // First emit: a PR URL is inferred as pr.
    let items = vec![feed_item("ext-1", "https://github.com/o/r/pull/5")];
    db.upsert_feed_tasks(epic.id, &items, &["/r".into()], &main_branches(1))
        .await
        .unwrap();
    let t = db.list_tasks_for_epic(epic.id).await.unwrap().remove(0);
    assert_eq!(
        t.url,
        Some(TaskUrl::new("https://github.com/o/r/pull/5", UrlType::Pr))
    );

    // Conflict re-emit with a DIFFERENT url must NOT clobber the existing pair.
    let items = vec![feed_item("ext-1", "https://github.com/o/r/pull/999")];
    db.upsert_feed_tasks(epic.id, &items, &["/r".into()], &main_branches(1))
        .await
        .unwrap();
    let t = db.list_tasks_for_epic(epic.id).await.unwrap().remove(0);
    assert_eq!(
        t.url,
        Some(TaskUrl::new("https://github.com/o/r/pull/5", UrlType::Pr)),
        "existing url/url_type must be preserved on conflict"
    );
}

#[tokio::test]
async fn upsert_feed_tasks_explicit_url_type_wins_over_inference() {
    use crate::models::{TaskUrl, UrlType};
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();

    // A Dependabot alert URL has no /pull/ or /issues/ segment, so inference
    // would classify it as Other. The declared security_alert must win.
    let alert_url = "https://github.com/org/repo/security/dependabot/7";
    let items = vec![
        crate::models::FeedItem {
            url: alert_url.to_string(),
            url_type: Some(UrlType::SecurityAlert),
            ..make_feed_item("ext-declared", "declared")
        },
        crate::models::FeedItem {
            url: alert_url.to_string(),
            url_type: None,
            ..make_feed_item("ext-inferred", "inferred")
        },
    ];
    db.upsert_feed_tasks(
        epic.id,
        &items,
        &["/repo".to_string(), "/repo".to_string()],
        &main_branches(2),
    )
    .await
    .unwrap();

    let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    let by_ext = |ext: &str| {
        tasks
            .iter()
            .find(|t| t.external_id.as_deref() == Some(ext))
            .unwrap()
    };
    assert_eq!(
        by_ext("ext-declared").url,
        Some(TaskUrl::new(alert_url, UrlType::SecurityAlert)),
        "explicit url_type is stored verbatim"
    );
    assert_eq!(
        by_ext("ext-inferred").url,
        Some(TaskUrl::new(alert_url, UrlType::Other)),
        "absent url_type falls back to inference"
    );
}

#[tokio::test]
async fn upsert_feed_tasks_backfill_uses_declared_url_type() {
    use crate::models::{TaskUrl, UrlType};
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();

    // First emission: no URL — task created with url = NULL.
    let initial = vec![make_feed_item("ext-1", "alert")];
    db.upsert_feed_tasks(epic.id, &initial, &["/repo".to_string()], &main_branches(1))
        .await
        .unwrap();
    let task_id = db.list_tasks_for_epic(epic.id).await.unwrap()[0].id;
    assert!(
        db.get_task(task_id).await.unwrap().unwrap().url.is_none(),
        "precondition: url is null after first upsert"
    );

    // Refresh with a URL and a declared type that inference cannot reach.
    let alert_url = "https://github.com/org/repo/security/dependabot/7";
    let refreshed = vec![crate::models::FeedItem {
        url: alert_url.to_string(),
        url_type: Some(UrlType::SecurityAlert),
        ..initial[0].clone()
    }];
    db.upsert_feed_tasks(
        epic.id,
        &refreshed,
        &["/repo".to_string()],
        &main_branches(1),
    )
    .await
    .unwrap();

    let task = db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(
        task.url,
        Some(TaskUrl::new(alert_url, UrlType::SecurityAlert)),
        "backfilled url_type uses the declared type, not inference"
    );
}

#[tokio::test]
async fn upsert_feed_tasks_can_purge_task_with_associated_learning() {
    use crate::models::{LearningKind, LearningScope};

    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();

    // First feed run: creates a task.
    let initial = vec![make_feed_item("ext-1", "first")];
    db.upsert_feed_tasks(epic.id, &initial, &["/repo".to_string()], &main_branches(1))
        .await
        .unwrap();
    let task_id = db.list_tasks_for_epic(epic.id).await.unwrap()[0].id;

    // The dispatched agent records a learning referencing the task as its source.
    db.create_learning(CreateLearningRow {
        kind: LearningKind::Pitfall,
        summary: "watch out",
        detail: None,
        scope: LearningScope::User,
        scope_ref: None,
        tags: &[],
        source_task_id: Some(task_id),
        embedding: None,
    })
    .await
    .unwrap();

    // Second feed run with a different external_id — the previous task should
    // be purged. Without ON DELETE SET NULL on learnings.source_task_id, this
    // fails with a FK violation.
    let next = vec![make_feed_item("ext-2", "second")];
    db.upsert_feed_tasks(epic.id, &next, &["/repo".to_string()], &main_branches(1))
        .await
        .expect("stale feed task with associated learning should be purgeable");

    let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].external_id.as_deref(), Some("ext-2"));
}

#[tokio::test]
async fn upsert_feed_tasks_can_purge_stale_task() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();

    let initial = vec![make_feed_item("ext-1", "first")];
    db.upsert_feed_tasks(epic.id, &initial, &["/repo".to_string()], &main_branches(1))
        .await
        .unwrap();

    let next = vec![make_feed_item("ext-2", "second")];
    db.upsert_feed_tasks(epic.id, &next, &["/repo".to_string()], &main_branches(1))
        .await
        .expect("stale feed task should be purgeable");
}

// ---------------------------------------------------------------------------
// patch_struct! macro correctness — has_changes() and setter coverage
// ---------------------------------------------------------------------------

#[tokio::test]
async fn task_patch_default_has_no_changes() {
    assert!(!TaskPatch::default().has_changes());
}

#[tokio::test]
async fn task_patch_each_setter_marks_has_changes() {
    assert!(TaskPatch::new().status(TaskStatus::Running).has_changes());
    assert!(TaskPatch::new().plan_path(Some("p")).has_changes());
    assert!(TaskPatch::new().plan_path(None).has_changes());
    assert!(TaskPatch::new().title("t").has_changes());
    assert!(TaskPatch::new().description("d").has_changes());
    assert!(TaskPatch::new().repo_path("/r").has_changes());
    assert!(TaskPatch::new().worktree(Some("w")).has_changes());
    assert!(TaskPatch::new().worktree(None).has_changes());
    assert!(TaskPatch::new().tmux_window(Some("tw")).has_changes());
    assert!(TaskPatch::new().tmux_window(None).has_changes());
    assert!(TaskPatch::new().sub_status(SubStatus::Active).has_changes());
    let url = crate::models::TaskUrl::new("u", crate::models::UrlType::Other);
    assert!(TaskPatch::new().url(Some(&url)).has_changes());
    assert!(TaskPatch::new().url(None).has_changes());
    assert!(TaskPatch::new().tag(Some(TaskTag::Bug)).has_changes());
    assert!(TaskPatch::new().tag(None).has_changes());
    assert!(TaskPatch::new().sort_order(Some(1)).has_changes());
    assert!(TaskPatch::new().sort_order(None).has_changes());
    assert!(TaskPatch::new().base_branch("main").has_changes());
    assert!(TaskPatch::new().external_id(Some("x")).has_changes());
    assert!(TaskPatch::new().external_id(None).has_changes());
    let labels: Vec<String> = vec!["x".into()];
    assert!(TaskPatch::new().labels(&labels).has_changes());
}

// ---------------------------------------------------------------------------
// Property tests
// ---------------------------------------------------------------------------

mod property_tests {
    use super::*;
    use proptest::prelude::*;

    /// Build a `TaskPatch` with the subset of fields indicated by `bits`.
    /// Each bit (0-12) maps to one field in `has_changes()` order.
    fn taskpatch_from_bits(bits: u16) -> TaskPatch<'static> {
        let mut p = TaskPatch::new();
        if bits & (1 << 0) != 0 {
            p = p.status(crate::models::TaskStatus::Backlog);
        }
        if bits & (1 << 1) != 0 {
            p = p.plan_path(Some("plan.md"));
        }
        if bits & (1 << 2) != 0 {
            p = p.title("t");
        }
        if bits & (1 << 3) != 0 {
            p = p.description("d");
        }
        if bits & (1 << 4) != 0 {
            p = p.repo_path("/repo");
        }
        if bits & (1 << 5) != 0 {
            p = p.worktree(Some(".wt"));
        }
        if bits & (1 << 6) != 0 {
            p = p.tmux_window(Some("w"));
        }
        if bits & (1 << 7) != 0 {
            p = p.sub_status(crate::models::SubStatus::Active);
        }
        if bits & (1 << 8) != 0 {
            static URL: std::sync::LazyLock<crate::models::TaskUrl> =
                std::sync::LazyLock::new(|| {
                    crate::models::TaskUrl::new(
                        "https://github.com/pr/1",
                        crate::models::UrlType::Pr,
                    )
                });
            p = p.url(Some(&URL));
        }
        if bits & (1 << 9) != 0 {
            p = p.tag(Some(crate::models::TaskTag::Bug));
        }
        if bits & (1 << 10) != 0 {
            p = p.sort_order(Some(1));
        }
        if bits & (1 << 11) != 0 {
            p = p.base_branch("main");
        }
        if bits & (1 << 12) != 0 {
            p = p.external_id(Some("ext-1"));
        }
        p
    }

    /// Build an `EpicPatch` with the subset of fields indicated by `bits`.
    /// Each bit (0-8) maps to one field in `has_changes()` order.
    fn epicpatch_from_bits(bits: u16) -> EpicPatch<'static> {
        let mut p = EpicPatch::new();
        if bits & (1 << 0) != 0 {
            p = p.title("epic title");
        }
        if bits & (1 << 1) != 0 {
            p = p.description("desc");
        }
        if bits & (1 << 2) != 0 {
            p = p.status(crate::models::TaskStatus::Running);
        }
        if bits & (1 << 3) != 0 {
            p = p.plan_path(Some("plan.md"));
        }
        if bits & (1 << 4) != 0 {
            p = p.sort_order(Some(1));
        }
        if bits & (1 << 5) != 0 {
            p = p.auto_dispatch(true);
        }
        if bits & (1 << 6) != 0 {
            p = p.feed_command(Some("cmd"));
        }
        if bits & (1 << 7) != 0 {
            p = p.feed_interval_secs(Some(60));
        }
        p
    }

    proptest! {
        #[test]
        fn taskpatch_has_changes_iff_any_field_set(bits in 0u16..8192) {
            let patch = taskpatch_from_bits(bits);
            prop_assert_eq!(patch.has_changes(), bits != 0);
        }

        #[test]
        fn epicpatch_has_changes_iff_any_field_set(bits in 0u16..256) {
            let patch = epicpatch_from_bits(bits);
            prop_assert_eq!(patch.has_changes(), bits != 0);
        }

        /// Applying a `TaskPatch` to a baseline task and re-reading should yield:
        /// - `Some(_)` patch fields → applied to the row
        /// - `None` patch fields   → preserved from baseline
        ///
        /// For nullable fields, `Some(Some(v))` writes `v` and `Some(None)` writes NULL.
        ///
        /// `status` and `sort_order` are exercised in dedicated property tests below
        /// because they have additional invariants (sub_status coupling, signed integer).
        #[test]
        fn taskpatch_roundtrip(
            title       in proptest::option::of("[a-zA-Z0-9 ]{1,32}"),
            description in proptest::option::of("[a-zA-Z0-9 ]{0,32}"),
            repo_path   in proptest::option::of("/[a-z]{1,16}"),
            base_branch in proptest::option::of("[a-z]{1,16}"),
            plan_path   in proptest::option::of(proptest::option::of("[a-z]{1,16}\\.md")),
            worktree    in proptest::option::of(proptest::option::of("/[a-z]{1,16}")),
            tmux_window in proptest::option::of(proptest::option::of("[a-z]{1,16}")),
            url         in proptest::option::of(proptest::option::of("https://x/[0-9]{1,4}")),
            external_id in proptest::option::of(proptest::option::of("[a-z]{1,16}")),
        ) {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async {
                let db = in_memory_db().await;
                let id = db
                    .create_task(CreateTaskRequest {
                        title: "Baseline",
                        description: "baseline desc",
                        repo_path: "/baseline",
                        plan: None,
                        status: TaskStatus::Backlog,
                        base_branch: "main",
                        epic_id: None,
                        sort_order: None,
                        tag: None,
                        wrap_up_mode: None,
                        auto_run_plan: false,
                    })
                    .await
                    .unwrap();
                let baseline = db.get_task(id).await.unwrap().unwrap();

                let mut p = TaskPatch::new();
                if let Some(t)  = title.as_deref()       { p = p.title(t); }
                if let Some(d)  = description.as_deref() { p = p.description(d); }
                if let Some(r)  = repo_path.as_deref()   { p = p.repo_path(r); }
                if let Some(bb) = base_branch.as_deref() { p = p.base_branch(bb); }
                if let Some(ref pp) = plan_path   { p = p.plan_path(pp.as_deref()); }
                if let Some(ref w)  = worktree    { p = p.worktree(w.as_deref()); }
                if let Some(ref tw) = tmux_window { p = p.tmux_window(tw.as_deref()); }
                // Map the generated string into a typed url (inferred type).
                let url_typed: Option<Option<crate::models::TaskUrl>> = url.as_ref().map(|inner| {
                    inner
                        .as_ref()
                        .map(|s| crate::models::TaskUrl::new(s.clone(), crate::models::UrlType::infer(s)))
                });
                if let Some(ref u)  = url_typed   { p = p.url(u.as_ref()); }
                if let Some(ref e)  = external_id { p = p.external_id(e.as_deref()); }

                db.patch_task(id, &p).await.unwrap();
                let after = db.get_task(id).await.unwrap().unwrap();

                prop_assert_eq!(&after.title,       &title.unwrap_or(baseline.title));
                prop_assert_eq!(&after.description, &description.unwrap_or(baseline.description));
                prop_assert_eq!(&after.repo_path,   &repo_path.unwrap_or(baseline.repo_path));
                prop_assert_eq!(&after.base_branch, &base_branch.unwrap_or(baseline.base_branch));
                prop_assert_eq!(&after.plan_path,   &plan_path.unwrap_or(baseline.plan_path));
                prop_assert_eq!(&after.worktree,    &worktree.unwrap_or(baseline.worktree));
                prop_assert_eq!(&after.tmux_window, &tmux_window.unwrap_or(baseline.tmux_window));
                prop_assert_eq!(&after.url,         &url_typed.unwrap_or(baseline.url));
                prop_assert_eq!(&after.external_id, &external_id.unwrap_or(baseline.external_id));
                prop_assert_eq!(after.status,     baseline.status);
                prop_assert_eq!(after.sub_status, baseline.sub_status);
                Ok::<(), proptest::test_runner::TestCaseError>(())
            })?;
        }

        /// `sort_order` is `nullable i64` — round-trip both Some(v) and None separately.
        #[test]
        fn taskpatch_roundtrip_sort_order(
            sort_order in proptest::option::of(proptest::option::of(any::<i64>())),
        ) {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async {
                let db = in_memory_db().await;
                let id = db
                    .create_task(CreateTaskRequest {
                        title: "T", description: "d", repo_path: "/r",
                        plan: None, status: TaskStatus::Backlog, base_branch: "main",
                        epic_id: None, sort_order: Some(42), tag: None,
                        wrap_up_mode: None,
                        auto_run_plan: false,
                    })
                    .await
                    .unwrap();
                let baseline = db.get_task(id).await.unwrap().unwrap();
                let mut p = TaskPatch::new();
                if let Some(so) = sort_order { p = p.sort_order(so); }
                db.patch_task(id, &p).await.unwrap();
                let after = db.get_task(id).await.unwrap().unwrap();
                prop_assert_eq!(after.sort_order, sort_order.unwrap_or(baseline.sort_order));
                Ok::<(), proptest::test_runner::TestCaseError>(())
            })?;
        }

        /// Applying an `EpicPatch` to a baseline epic and re-reading should yield
        /// the same Some(_) ↔ field, None ↔ baseline contract as `TaskPatch`.
        #[test]
        fn epicpatch_roundtrip(
            title       in proptest::option::of("[a-zA-Z0-9 ]{1,32}"),
            description in proptest::option::of("[a-zA-Z0-9 ]{0,32}"),
            plan_path   in proptest::option::of(proptest::option::of("[a-z]{1,16}\\.md")),
            sort_order  in proptest::option::of(proptest::option::of(any::<i64>())),
            auto_dispatch in proptest::option::of(any::<bool>()),
            feed_command  in proptest::option::of(proptest::option::of("[a-z]{1,16}")),
            feed_interval in proptest::option::of(proptest::option::of(1i64..86_400)),
        ) {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async {
                let db = in_memory_db().await;
                let epic = db
                    .create_epic("Baseline epic", "baseline", None).await
                    .unwrap();
                let baseline = db.get_epic(epic.id).await.unwrap().unwrap();

                let mut p = EpicPatch::new();
                if let Some(t)  = title.as_deref()       { p = p.title(t); }
                if let Some(d)  = description.as_deref() { p = p.description(d); }
                if let Some(ref pp) = plan_path { p = p.plan_path(pp.as_deref()); }
                if let Some(so) = sort_order    { p = p.sort_order(so); }
                if let Some(ad) = auto_dispatch { p = p.auto_dispatch(ad); }
                if let Some(ref fc) = feed_command  { p = p.feed_command(fc.as_deref()); }
                if let Some(fi) = feed_interval     { p = p.feed_interval_secs(fi); }

                db.patch_epic(epic.id, &p).await.unwrap();
                let after = db.get_epic(epic.id).await.unwrap().unwrap();

                prop_assert_eq!(&after.title,         &title.unwrap_or(baseline.title));
                prop_assert_eq!(&after.description,   &description.unwrap_or(baseline.description));
                prop_assert_eq!(&after.plan_path,     &plan_path.unwrap_or(baseline.plan_path));
                prop_assert_eq!(after.sort_order,     sort_order.unwrap_or(baseline.sort_order));
                prop_assert_eq!(after.auto_dispatch,  auto_dispatch.unwrap_or(baseline.auto_dispatch));
                prop_assert_eq!(&after.feed_command,  &feed_command.unwrap_or(baseline.feed_command));
                prop_assert_eq!(after.feed_interval_secs, feed_interval.unwrap_or(baseline.feed_interval_secs));
                Ok::<(), proptest::test_runner::TestCaseError>(())
            })?;
        }
    }
}

#[tokio::test]
async fn create_task_wrap_up_mode_defaults_to_none() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "T",
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
        })
        .await
        .unwrap();
    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.wrap_up_mode, None);
}

#[tokio::test]
async fn create_task_with_wrap_up_mode_rebase() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "T",
            description: "",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: Some(WrapUpMode::Rebase),
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.wrap_up_mode, Some(WrapUpMode::Rebase));
}

#[tokio::test]
async fn patch_task_wrap_up_mode() {
    let db = in_memory_db().await;
    let task = create_task_returning(&db, "T", "", "/repo", None, TaskStatus::Backlog)
        .await
        .unwrap();
    assert_eq!(task.wrap_up_mode, None);

    // Set to Pr
    db.patch_task(
        task.id,
        &TaskPatch::new().wrap_up_mode(Some(WrapUpMode::Pr)),
    )
    .await
    .unwrap();
    let task = db.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(task.wrap_up_mode, Some(WrapUpMode::Pr));

    // Clear it
    db.patch_task(task.id, &TaskPatch::new().wrap_up_mode(None))
        .await
        .unwrap();
    let task = db.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(task.wrap_up_mode, None);
}

#[tokio::test]
async fn patch_auto_run_plan_true() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "T",
            description: "d",
            repo_path: "/r",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    db.patch_task(id, &TaskPatch::new().auto_run_plan(true))
        .await
        .unwrap();
    let task = db.get_task(id).await.unwrap().expect("task should exist");
    assert!(task.auto_run_plan);
}

#[tokio::test]
async fn get_task_errors_on_unknown_tag() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "t",
            description: "",
            repo_path: "/r",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let task_id = id.0;
    db.db_call(move |conn| {
        conn.execute(
            "UPDATE tasks SET tag = 'xyzzy_unknown' WHERE id = ?1",
            rusqlite::params![task_id],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let result = db.get_task(id).await;
    assert!(
        result.is_err(),
        "expected Err on unknown tag, got {:?}",
        result
    );
}

#[tokio::test]
async fn list_all_errors_on_unknown_wrap_up_mode() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "t",
            description: "",
            repo_path: "/r",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let task_id = id.0;
    db.db_call(move |conn| {
        conn.execute(
            "UPDATE tasks SET wrap_up_mode = 'unknown_mode' WHERE id = ?1",
            rusqlite::params![task_id],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let result = db.list_all().await;
    assert!(result.is_err(), "expected Err on unknown wrap_up_mode");
}

#[tokio::test]
async fn row_to_task_sub_status_none_string_maps_to_none_variant() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "t",
            description: "d",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.sub_status, SubStatus::None);
}

#[tokio::test]
async fn row_to_task_base_branch_defaults_to_main() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "t",
            description: "d",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.base_branch, "main");
}

#[tokio::test]
async fn get_task_errors_on_corrupt_sort_order_type() {
    // Regression: row.get::<_, Option<i64>>("sort_order").unwrap_or(None) silently
    // returned None when the column held a non-integer value. Now uses `?` so
    // schema drift surfaces immediately.
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "t",
            description: "d",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let task_id = id.0;
    db.db_call(move |conn| {
        conn.execute(
            "UPDATE tasks SET sort_order = 'not-an-int' WHERE id = ?1",
            rusqlite::params![task_id],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let result = db.get_task(id).await;
    assert!(
        result.is_err(),
        "expected Err when sort_order holds a non-integer value, got {:?}",
        result
    );
}

// ---------------------------------------------------------------------------
// OwnedTaskPatch / OwnedCreateTaskRequest mirror parity
// ---------------------------------------------------------------------------

/// Every field in TaskPatch must survive the round-trip through OwnedTaskPatch
/// into the database.  This test catches any field that the From impl silently
/// drops from the DB write.
#[tokio::test]
async fn patch_task_all_fields_round_trip() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "original",
            description: "orig desc",
            repo_path: "/orig",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let labels = vec!["lbl-a".to_string(), "lbl-b".to_string()];
    let ts_pre = chrono::Utc::now() - chrono::Duration::seconds(120);
    let ts_notif = chrono::Utc::now() - chrono::Duration::seconds(60);
    let patch_url = crate::models::TaskUrl::new(
        "https://github.com/org/repo/pull/99",
        crate::models::UrlType::Pr,
    );

    db.patch_task(
        id,
        &TaskPatch::new()
            .status(TaskStatus::Running)
            .sub_status(SubStatus::Active)
            .plan_path(Some("docs/my-plan.md"))
            .title("patched title")
            .description("patched desc")
            .repo_path("/patched/repo")
            .worktree(Some(".worktrees/1394"))
            .tmux_window(Some("session:1394"))
            .url(Some(&patch_url))
            .tag(Some(TaskTag::Feature))
            .sort_order(Some(42))
            .base_branch("feature-branch")
            .external_id(Some("ext-xyz"))
            .labels(&labels)
            .last_pre_tool_use_at(Some(ts_pre))
            .last_notification_at(Some(ts_notif))
            .wrap_up_mode(Some(WrapUpMode::Pr)),
    )
    .await
    .unwrap();

    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Running, "status");
    assert_eq!(task.sub_status, SubStatus::Active, "sub_status");
    assert_eq!(
        task.plan_path.as_deref(),
        Some("docs/my-plan.md"),
        "plan_path"
    );
    assert_eq!(task.title, "patched title", "title");
    assert_eq!(task.description, "patched desc", "description");
    assert_eq!(task.repo_path, "/patched/repo", "repo_path");
    assert_eq!(
        task.worktree.as_deref(),
        Some(".worktrees/1394"),
        "worktree"
    );
    assert_eq!(
        task.tmux_window.as_deref(),
        Some("session:1394"),
        "tmux_window"
    );
    assert_eq!(task.url, Some(patch_url), "url");
    assert_eq!(task.tag, Some(TaskTag::Feature), "tag");
    assert_eq!(task.sort_order, Some(42), "sort_order");
    assert_eq!(task.base_branch, "feature-branch", "base_branch");
    assert_eq!(task.external_id.as_deref(), Some("ext-xyz"), "external_id");
    assert_eq!(task.labels, labels, "labels");
    let stored_pre = task
        .last_pre_tool_use_at
        .expect("last_pre_tool_use_at written");
    assert!(
        (stored_pre - ts_pre).num_seconds().abs() <= 1,
        "last_pre_tool_use_at"
    );
    let stored_notif = task
        .last_notification_at
        .expect("last_notification_at written");
    assert!(
        (stored_notif - ts_notif).num_seconds().abs() <= 1,
        "last_notification_at"
    );
    assert_eq!(task.wrap_up_mode, Some(WrapUpMode::Pr), "wrap_up_mode");
}

/// wrap_up_mode in CreateTaskRequest must be persisted (not silently dropped by
/// OwnedCreateTaskRequest).
#[tokio::test]
async fn create_task_persists_wrap_up_mode() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "T",
            description: "d",
            repo_path: "/r",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: Some(WrapUpMode::Rebase),
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.wrap_up_mode, Some(WrapUpMode::Rebase));
}

#[tokio::test]
async fn mark_pr_learnings_gate_shown_sets_once() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "t",
            description: "",
            repo_path: "/tmp/r",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    // First call sets the flag -> true (block).
    assert!(db.mark_pr_learnings_gate_shown(id).await.unwrap());
    // Second call: already set -> false (allow).
    assert!(!db.mark_pr_learnings_gate_shown(id).await.unwrap());
}

#[tokio::test]
async fn mark_pr_learnings_gate_shown_missing_task_is_false() {
    let db = in_memory_db().await;
    assert!(!db
        .mark_pr_learnings_gate_shown(TaskId(999_999))
        .await
        .unwrap());
}

#[tokio::test]
async fn batch_patch_sub_status_updates_all_tasks() {
    let db = in_memory_db().await;
    let t1 = db
        .create_task(CreateTaskRequest {
            title: "A",
            description: "",
            repo_path: "/r",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let t2 = db
        .create_task(CreateTaskRequest {
            title: "B",
            description: "",
            repo_path: "/r",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    db.batch_patch_sub_status(&[(t1, SubStatus::Stale), (t2, SubStatus::NeedsInput)])
        .await
        .unwrap();

    assert_eq!(
        db.get_task(t1).await.unwrap().unwrap().sub_status,
        SubStatus::Stale
    );
    assert_eq!(
        db.get_task(t2).await.unwrap().unwrap().sub_status,
        SubStatus::NeedsInput
    );
}

#[tokio::test]
async fn batch_patch_sub_status_empty_is_no_op() {
    let db = in_memory_db().await;
    // Should not error on empty input.
    db.batch_patch_sub_status(&[]).await.unwrap();
}

#[tokio::test]
async fn get_total_changes_increases_after_write() {
    let db = in_memory_db().await;
    let v1 = db.get_total_changes().await.unwrap();
    db.create_task(CreateTaskRequest {
        title: "T",
        description: "",
        repo_path: "/r",
        plan: None,
        status: TaskStatus::Backlog,
        base_branch: "main",
        epic_id: None,
        sort_order: None,
        tag: None,
        wrap_up_mode: None,
        auto_run_plan: false,
    })
    .await
    .unwrap();
    let v2 = db.get_total_changes().await.unwrap();
    assert!(
        v2 > v1,
        "total_changes must increase after a write ({v1} → {v2})"
    );
}

#[tokio::test]
async fn get_total_changes_stable_when_no_writes() {
    let db = in_memory_db().await;
    // Two consecutive reads with only a SELECT between them must return the same value.
    let v1 = db.get_total_changes().await.unwrap();
    let _ = db.list_all().await.unwrap();
    let v2 = db.get_total_changes().await.unwrap();
    assert_eq!(
        v1, v2,
        "total_changes must not change across read-only queries"
    );
}

#[tokio::test]
async fn create_task_watcher_is_idempotent() {
    let db = in_memory_db().await;
    let a = create_task_returning(&db, "Watcher", "", "/repo", None, TaskStatus::Running)
        .await
        .unwrap();
    let b = create_task_returning(&db, "Target", "", "/repo", None, TaskStatus::Running)
        .await
        .unwrap();

    db.create_task_watcher(a.id, b.id).await.unwrap();
    db.create_task_watcher(a.id, b.id).await.unwrap(); // no-op, must not error

    let watchers = db.list_watchers_of(b.id).await.unwrap();
    assert_eq!(watchers, vec![a.id]);
}

#[tokio::test]
async fn delete_task_watcher_is_idempotent() {
    let db = in_memory_db().await;
    let a = create_task_returning(&db, "Watcher", "", "/repo", None, TaskStatus::Running)
        .await
        .unwrap();
    let b = create_task_returning(&db, "Target", "", "/repo", None, TaskStatus::Running)
        .await
        .unwrap();

    db.create_task_watcher(a.id, b.id).await.unwrap();
    db.delete_task_watcher(a.id, b.id).await.unwrap();
    db.delete_task_watcher(a.id, b.id).await.unwrap(); // no-op, must not error

    assert!(db.list_watchers_of(b.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn list_watchers_of_returns_all_watchers() {
    let db = in_memory_db().await;
    let a = create_task_returning(&db, "Watcher A", "", "/repo", None, TaskStatus::Running)
        .await
        .unwrap();
    let b = create_task_returning(&db, "Watcher B", "", "/repo", None, TaskStatus::Running)
        .await
        .unwrap();
    let target = create_task_returning(&db, "Target", "", "/repo", None, TaskStatus::Running)
        .await
        .unwrap();

    db.create_task_watcher(a.id, target.id).await.unwrap();
    db.create_task_watcher(b.id, target.id).await.unwrap();

    let mut watchers = db.list_watchers_of(target.id).await.unwrap();
    watchers.sort_by_key(|t| t.0);
    let mut expected = vec![a.id, b.id];
    expected.sort_by_key(|t| t.0);
    assert_eq!(watchers, expected);
}

#[tokio::test]
async fn delete_watches_of_target_removes_all_watchers() {
    let db = in_memory_db().await;
    let a = create_task_returning(&db, "Watcher A", "", "/repo", None, TaskStatus::Running)
        .await
        .unwrap();
    let b = create_task_returning(&db, "Watcher B", "", "/repo", None, TaskStatus::Running)
        .await
        .unwrap();
    let target = create_task_returning(&db, "Target", "", "/repo", None, TaskStatus::Running)
        .await
        .unwrap();

    db.create_task_watcher(a.id, target.id).await.unwrap();
    db.create_task_watcher(b.id, target.id).await.unwrap();

    db.delete_watches_of_target(target.id).await.unwrap();

    assert!(db.list_watchers_of(target.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn delete_watches_by_watcher_removes_only_that_watchers_rows() {
    let db = in_memory_db().await;
    let a = create_task_returning(&db, "Watcher A", "", "/repo", None, TaskStatus::Running)
        .await
        .unwrap();
    let b = create_task_returning(&db, "Watcher B", "", "/repo", None, TaskStatus::Running)
        .await
        .unwrap();
    let target1 = create_task_returning(&db, "Target 1", "", "/repo", None, TaskStatus::Running)
        .await
        .unwrap();
    let target2 = create_task_returning(&db, "Target 2", "", "/repo", None, TaskStatus::Running)
        .await
        .unwrap();

    db.create_task_watcher(a.id, target1.id).await.unwrap();
    db.create_task_watcher(b.id, target2.id).await.unwrap();

    db.delete_watches_by_watcher(a.id).await.unwrap();

    assert!(db.list_watchers_of(target1.id).await.unwrap().is_empty());
    assert_eq!(db.list_watchers_of(target2.id).await.unwrap(), vec![b.id]);
}
