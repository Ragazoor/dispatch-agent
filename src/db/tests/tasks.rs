#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use crate::models::test_tmux_window;

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
            phoenix: false,
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
        phoenix: false,
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
        phoenix: false,
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
        phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
        phoenix: false,
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
        phoenix: false,
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
async fn respawn_phoenix_successor_creates_task_with_labels_in_one_insert() {
    let db = in_memory_db().await;
    let predecessor = db
        .create_task(CreateTaskRequest {
            title: "Weekly audit",
            description: "d",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Done,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
            phoenix: true,
        })
        .await
        .unwrap();

    let labels = vec!["scala-common".to_string(), "security".to_string()];
    let successor_id = db
        .respawn_phoenix_successor(
            predecessor,
            CreateTaskRequest {
                title: "Weekly audit",
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
                phoenix: true,
            },
            &labels,
        )
        .await
        .unwrap();

    let successor = db.get_task(successor_id).await.unwrap().unwrap();
    assert_eq!(
        successor.labels, labels,
        "labels land as part of the single insert, not a follow-up patch"
    );

    let predecessor_task = db.get_task(predecessor).await.unwrap().unwrap();
    assert!(
        !predecessor_task.phoenix,
        "the flag clears in the same transaction as the successor's creation"
    );
}

#[tokio::test]
async fn respawn_phoenix_successor_rolls_back_if_predecessor_is_gone() {
    let db = in_memory_db().await;
    // Far from any id sqlite would ever assign to the successor's own insert,
    // so the UPDATE below cannot accidentally hit the just-inserted successor
    // and must genuinely match zero rows.
    let predecessor = TaskId(999_999);

    let result = db
        .respawn_phoenix_successor(
            predecessor,
            CreateTaskRequest {
                title: "Weekly audit",
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
                phoenix: true,
            },
            &[],
        )
        .await;
    assert!(
        result.is_err(),
        "a predecessor that vanished mid-flight must fail the whole call"
    );

    let all = db.list_all().await.unwrap();
    assert!(
        all.is_empty(),
        "no orphaned successor left behind when the transaction rolls back"
    );
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
async fn patch_task_round_trips_peer_message_timestamps() {
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
            phoenix: false,
        })
        .await
        .unwrap();

    let task = db.get_task(id).await.unwrap().unwrap();
    assert!(task.last_peer_message_sent_at.is_none());
    assert!(task.last_peer_message_received_at.is_none());

    let sent = chrono::Utc::now();
    let received = sent - chrono::Duration::seconds(5);
    db.patch_task(
        id,
        &TaskPatch::new()
            .last_peer_message_sent_at(Some(sent))
            .last_peer_message_received_at(Some(received)),
    )
    .await
    .unwrap();

    let task = db.get_task(id).await.unwrap().unwrap();
    let stored_sent = task
        .last_peer_message_sent_at
        .expect("peer message sent timestamp written");
    let stored_received = task
        .last_peer_message_received_at
        .expect("peer message received timestamp written");
    assert!(
        (stored_sent - sent).num_seconds().abs() <= 1,
        "stored sent {stored_sent} too far from {sent}"
    );
    assert!(
        (stored_received - received).num_seconds().abs() <= 1,
        "stored received {stored_received} too far from {received}"
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
            phoenix: false,
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
            phoenix: false,
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
    // Bulk read: skip-and-warn. The single-entity read stays fail-loud.
    let tasks = db.list_all().await.expect("bulk read must not fail");
    assert!(
        tasks.is_empty(),
        "the row with corrupt labels JSON must be skipped, got {tasks:?}"
    );
    let result = db.get_task(id).await;
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
        })
        .await
        .unwrap();
    let window = test_tmux_window("session:1-my-task");
    let patch = TaskPatch::new()
        .worktree(Some("/repo/.worktrees/1-my-task"))
        .tmux_window(Some(&window));
    db.patch_task(id, &patch).await.unwrap();
    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.worktree.as_deref(), Some("/repo/.worktrees/1-my-task"));
    assert_eq!(
        task.tmux_window.as_ref().map(|w| w.as_str()),
        Some("session:1-my-task")
    );
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
            phoenix: false,
        })
        .await
        .unwrap();
    // Set dispatch fields first
    let window = test_tmux_window("session:1-my-task");
    let patch = TaskPatch::new()
        .worktree(Some("/repo/.worktrees/1-my-task"))
        .tmux_window(Some(&window));
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
            phoenix: false,
        })
        .await
        .unwrap();
    let window = test_tmux_window("session:1-my-task");
    let patch = TaskPatch::new()
        .status(TaskStatus::Running)
        .worktree(Some("/repo/.worktrees/1-my-task"))
        .tmux_window(Some(&window));
    db.patch_task(id, &patch).await.unwrap();
    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.worktree.as_deref(), Some("/repo/.worktrees/1-my-task"));
    assert_eq!(
        task.tmux_window.as_ref().map(|w| w.as_str()),
        Some("session:1-my-task")
    );
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
async fn task_sub_status_pr_closed_persists_for_review() {
    let db = Database::open_in_memory().await.unwrap();
    let id = db
        .create_task(CreateTaskRequest {
            title: "Test",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Review,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
            phoenix: false,
        })
        .await
        .unwrap();
    db.patch_task(id, &TaskPatch::default().sub_status(SubStatus::PrClosed))
        .await
        .unwrap();
    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.sub_status, SubStatus::PrClosed);
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
        })
        .await
        .unwrap();
    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.epic_id, Some(epic.id));
    assert_eq!(task.sort_order, Some(7));
    assert_eq!(task.tag, Some(TaskTag::Bug));
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
            phoenix: false,
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
// Query coverage: task_exists
// ---------------------------------------------------------------------------

#[tokio::test]
async fn task_exists_tracks_creation_and_deletion() {
    let db = in_memory_db().await;
    assert!(
        !db.task_exists(TaskId(9999)).await.unwrap(),
        "an id that was never created must not exist"
    );

    let task = create_task_returning(&db, "t", "desc", "/repo", None, TaskStatus::Backlog)
        .await
        .unwrap();
    assert!(db.task_exists(task.id).await.unwrap());

    db.delete_task(task.id).await.unwrap();
    assert!(
        !db.task_exists(task.id).await.unwrap(),
        "existence must follow the row, not a cache"
    );
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

/// The claim the whole "archive, never delete" instruction for an append-only
/// feed rests on (feeds.allium: AppendOnlyFeed).
///
/// Archiving is the ONLY permanent suppression such a feed offers: dispatch
/// keeps no separate record of an external_id it has retired, so the archived
/// TASK is the record. If archiving dropped the row, moved the epic, or cleared
/// the external_id, the next poll would insert the card again and the
/// suppression would be a lie. Deleting the task really does bring it back —
/// that asymmetry is asserted below too, because it is the reason the
/// instruction says archive rather than delete.
#[tokio::test]
async fn upsert_feed_tasks_leaves_an_archived_task_archived_and_inserts_no_second_row() {
    let db = in_memory_db().await;
    let epic = db.create_epic("Log warnings", "", None).await.unwrap();
    let items = vec![make_feed_item("log:WARN:mod:a warning", "warn A")];

    db.upsert_feed_tasks(epic.id, &items, &["/repo".to_string()], &main_branches(1))
        .await
        .unwrap();

    let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    assert_eq!(tasks.len(), 1);
    let archived_id = tasks[0].id;
    db.patch_task(archived_id, &TaskPatch::new().status(TaskStatus::Archived))
        .await
        .unwrap();

    // The record is still in the log, so the script emits it again — forever.
    for _ in 0..3 {
        db.upsert_feed_tasks(epic.id, &items, &["/repo".to_string()], &main_branches(1))
            .await
            .unwrap();
    }

    let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    assert_eq!(
        tasks.len(),
        1,
        "the archived row is the conflict target, so re-emission updates it rather than inserting beside it"
    );
    assert_eq!(
        tasks[0].id, archived_id,
        "it is the same row, not a replacement"
    );
    assert_eq!(
        tasks[0].status,
        TaskStatus::Archived,
        "status preservation applies to archived exactly as it does to running: the card stays suppressed"
    );

    // The other half: deleting instead of archiving removes the conflict
    // target, so the very next poll brings the card back.
    db.delete_task(archived_id).await.unwrap();
    db.upsert_feed_tasks(epic.id, &items, &["/repo".to_string()], &main_branches(1))
        .await
        .unwrap();

    let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    assert_eq!(
        tasks.len(),
        1,
        "a deleted feed task is re-inserted by the next emission"
    );
    // Not asserted by id: SQLite reuses the rowid once the table is empty, so
    // the fresh row can land on the id the deleted one had. What makes it a
    // fresh row is that it carries the FEED's status again rather than the
    // triage the user applied.
    assert_ne!(
        tasks[0].status,
        TaskStatus::Archived,
        "deleting is not suppression: the re-inserted row takes the feed's status, undoing the triage"
    );
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
async fn upsert_feed_tasks_preserves_sort_order_when_task_is_done() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let items = vec![make_feed_item("ext-1", "Original Title")];

    db.upsert_feed_tasks(epic.id, &items, &["/repo".to_string()], &main_branches(1))
        .await
        .unwrap();

    // Simulate the task completing and getting a completion-order
    // sort_order, then the feed re-polling with its own severity-rank
    // sort_order — the completion value must survive.
    let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    db.patch_task(
        tasks[0].id,
        &TaskPatch::new()
            .status(TaskStatus::Done)
            .sort_order(Some(-1_700_000_000_000)),
    )
    .await
    .unwrap();

    let mut updated_item = make_feed_item("ext-1", "Original Title");
    updated_item.sort_order = Some(1); // feed severity rank
    db.upsert_feed_tasks(
        epic.id,
        &[updated_item],
        &["/repo".to_string()],
        &main_branches(1),
    )
    .await
    .unwrap();

    let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(
        tasks[0].sort_order,
        Some(-1_700_000_000_000),
        "re-poll must not clobber a Done task's completion-order sort_order"
    );
}

#[tokio::test]
async fn upsert_feed_tasks_still_updates_sort_order_when_task_is_not_done() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let items = vec![make_feed_item("ext-1", "Original Title")];

    db.upsert_feed_tasks(epic.id, &items, &["/repo".to_string()], &main_branches(1))
        .await
        .unwrap();

    let mut updated_item = make_feed_item("ext-1", "Original Title");
    updated_item.sort_order = Some(7);
    db.upsert_feed_tasks(
        epic.id,
        &[updated_item],
        &["/repo".to_string()],
        &main_branches(1),
    )
    .await
    .unwrap();

    let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    assert_eq!(tasks[0].sort_order, Some(7));
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

/// The subtree delete must hand back the rows it removed so the caller can tear
/// down their worktrees (`feeds.allium`: `RoleRoutedFeedSync`). Only rows
/// carrying a worktree or tmux window are returned — a plain card has nothing to
/// clean up. Crucially, the DELETE predicate is unchanged: every stale feed task
/// is still removed from the DB, reported or not.
///
/// Two of the deleted rows carry state on purpose: reporting must be *complete*,
/// not merely non-empty. Under-reporting is invisible in the DB (the row is gone
/// either way) and surfaces only as a silently leaked worktree, so a single
/// state-carrying row would let a truncated `RETURNING` drain pass.
#[tokio::test]
async fn delete_stale_subtree_feed_tasks_returns_removed_rows_with_state() {
    let db = in_memory_db().await;
    let parent = db.create_epic("Reviews", "", None).await.unwrap();
    let sub = db
        .create_epic("My Reviews", "", Some(parent.id))
        .await
        .unwrap();

    db.upsert_feed_tasks(
        sub.id,
        &[
            make_feed_item("stale-1", "Stale"),
            make_feed_item("stale-2", "Also stale"),
            make_feed_item("plain-3", "Plain"),
            make_feed_item("keep-4", "Kept"),
        ],
        &vec!["/repo/a".to_string(); 4],
        &main_branches(4),
    )
    .await
    .unwrap();

    let tasks = db.list_tasks_for_epic(sub.id).await.unwrap();
    let by_ext = |ext: &str| {
        tasks
            .iter()
            .find(|t| t.external_id.as_deref() == Some(ext))
            .unwrap()
            .id
    };

    // `stale-1` carries both kinds of state; `stale-2` only a worktree, so the
    // two reported rows are not interchangeable. `plain-3` carries neither.
    let stale_1 = by_ext("stale-1");
    let stale_2 = by_ext("stale-2");
    db.patch_task(
        stale_1,
        &TaskPatch::new()
            .worktree(Some("/repo/a/.worktrees/stale-1"))
            .tmux_window(Some(&test_tmux_window("dispatch:stale-1"))),
    )
    .await
    .unwrap();
    db.patch_task(
        stale_2,
        &TaskPatch::new().worktree(Some("/repo/a/.worktrees/stale-2")),
    )
    .await
    .unwrap();

    let removed = db
        .delete_stale_subtree_feed_tasks(parent.id, &["keep-4".to_string()])
        .await
        .unwrap();

    // stale-1, stale-2 and plain-3 are all really gone from the DB...
    let left = db.list_tasks_for_epic(sub.id).await.unwrap();
    assert_eq!(left.len(), 1, "only the kept item survives, got {left:?}");
    assert_eq!(left[0].external_id.as_deref(), Some("keep-4"));

    // ...but only the two rows with state need teardown, and *both* of them do:
    // dropping either would leak a worktree with nothing left to name it.
    assert_eq!(
        removed.len(),
        2,
        "every removed row with state must be reported, got {removed:?}"
    );
    let reported = |id| removed.iter().find(|r| r.id == id).unwrap();
    let one = reported(stale_1);
    assert_eq!(one.repo_path, "/repo/a");
    assert_eq!(one.worktree.as_deref(), Some("/repo/a/.worktrees/stale-1"));
    assert_eq!(
        one.tmux_window.as_ref().map(|w| w.as_str()),
        Some("dispatch:stale-1")
    );
    let two = reported(stale_2);
    assert_eq!(two.repo_path, "/repo/a");
    assert_eq!(two.worktree.as_deref(), Some("/repo/a/.worktrees/stale-2"));
    assert_eq!(two.tmux_window, None);
}

/// The same contract for the flat/grouped path's stale-delete, which runs inside
/// `upsert_feed_tasks` (`feeds.allium`: `UpsertFeedTasks`).
#[tokio::test]
async fn upsert_feed_tasks_returns_removed_rows_with_state() {
    let db = in_memory_db().await;
    let epic = db.create_epic("CVE Feed", "", None).await.unwrap();

    db.upsert_feed_tasks(
        epic.id,
        &[
            make_feed_item("gone-1", "Gone"),
            make_feed_item("bare-2", "Bare"),
        ],
        &vec!["/repo/b".to_string(); 2],
        &main_branches(2),
    )
    .await
    .unwrap();

    let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    let gone = tasks
        .iter()
        .find(|t| t.external_id.as_deref() == Some("gone-1"))
        .unwrap();
    db.patch_task(
        gone.id,
        &TaskPatch::new().worktree(Some("/repo/b/.worktrees/gone-1")),
    )
    .await
    .unwrap();

    // An empty emission clears the epic and reports what it removed.
    let removed = db.upsert_feed_tasks(epic.id, &[], &[], &[]).await.unwrap();

    // The delete really ran — draining RETURNING is what executes it.
    assert!(
        db.list_tasks_for_epic(epic.id).await.unwrap().is_empty(),
        "every stale feed task is deleted, reported or not"
    );

    assert_eq!(removed.len(), 1, "only the row with a worktree is reported");
    assert_eq!(removed[0].id, gone.id);
    assert_eq!(removed[0].repo_path, "/repo/b");
    assert_eq!(
        removed[0].worktree.as_deref(),
        Some("/repo/b/.worktrees/gone-1")
    );
    assert_eq!(removed[0].tmux_window, None, "no tmux window was set");
}

/// The additive variant writes everything `upsert_feed_tasks` writes and deletes
/// nothing (`feeds.allium`: `DegradedNonEmptyEmission`). This is the DB half of
/// the partial-degradation guard: a tainted emission's omissions must not reach
/// a `DELETE`, so there is no row to report and nothing for the teardown fan-out
/// to destroy.
#[tokio::test]
async fn upsert_feed_tasks_additive_upserts_without_deleting_absent_tasks() {
    let db = in_memory_db().await;
    let epic = db.create_epic("Reviews", "", None).await.unwrap();

    db.upsert_feed_tasks(
        epic.id,
        &[
            make_feed_item("live-1", "Under review"),
            make_feed_item("keep-2", "Still open"),
        ],
        &vec!["/repo/a".to_string(); 2],
        &main_branches(2),
    )
    .await
    .unwrap();

    // `live-1` carries an agent's worktree — the row a stale delete would
    // report and the fan-out would then force-remove.
    let live = db
        .list_tasks_for_epic(epic.id)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.external_id.as_deref() == Some("live-1"))
        .unwrap();
    db.patch_task(
        live.id,
        &TaskPatch::new().worktree(Some("/repo/a/.worktrees/live-1")),
    )
    .await
    .unwrap();

    // A partially degraded emission: `live-1` dropped out, one item is new.
    let removed = db
        .upsert_feed_tasks_additive(
            epic.id,
            &[
                make_feed_item("keep-2", "Still open, retitled"),
                make_feed_item("new-3", "Newly seen"),
            ],
            &vec!["/repo/a".to_string(); 2],
            &main_branches(2),
        )
        .await
        .unwrap();
    assert!(
        removed.is_empty(),
        "the additive variant reports nothing for teardown because it removes \
         nothing, got {removed:?}"
    );

    let left = db.list_tasks_for_epic(epic.id).await.unwrap();
    let ext = |t: &crate::models::Task| t.external_id.clone().unwrap_or_default();
    let mut ids: Vec<String> = left.iter().map(ext).collect();
    ids.sort();
    assert_eq!(
        ids,
        vec![
            "keep-2".to_string(),
            "live-1".to_string(),
            "new-3".to_string()
        ],
        "the omitted task must survive and the new one must be inserted"
    );

    // The omitted task keeps its state, so the agent in it is untouched.
    let survivor = left
        .iter()
        .find(|t| t.external_id.as_deref() == Some("live-1"))
        .unwrap();
    assert_eq!(
        survivor.worktree.as_deref(),
        Some("/repo/a/.worktrees/live-1")
    );

    // The present items are still refreshed — additive means "no removals",
    // not "no writes".
    let kept = left
        .iter()
        .find(|t| t.external_id.as_deref() == Some("keep-2"))
        .unwrap();
    assert_eq!(kept.title, "Still open, retitled");
}

/// An empty additive emission is a no-op, not a clear. The reconciling variant
/// treats `[]` as "delete everything"; the additive one must treat it as "learn
/// nothing", or the guard would leak the exact wipe it exists to prevent.
#[tokio::test]
async fn upsert_feed_tasks_additive_with_no_items_deletes_nothing() {
    let db = in_memory_db().await;
    let epic = db.create_epic("Reviews", "", None).await.unwrap();
    db.upsert_feed_tasks(
        epic.id,
        &[make_feed_item("ext-1", "One")],
        &["/repo/a".to_string()],
        &main_branches(1),
    )
    .await
    .unwrap();

    db.upsert_feed_tasks_additive(epic.id, &[], &[], &[])
        .await
        .unwrap();

    assert_eq!(
        db.list_tasks_for_epic(epic.id).await.unwrap().len(),
        1,
        "an empty additive emission must not clear the epic"
    );
}

/// A manual task (`external_id IS NULL`) is never deleted and never reported,
/// even when it carries a worktree.
#[tokio::test]
async fn delete_stale_subtree_feed_tasks_never_reports_manual_tasks() {
    let db = in_memory_db().await;
    let parent = db.create_epic("Reviews", "", None).await.unwrap();
    let sub = db
        .create_epic("My Reviews", "", Some(parent.id))
        .await
        .unwrap();

    let manual = db
        .create_task(CreateTaskRequest {
            title: "Manual",
            description: "",
            repo_path: "/repo/a",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: Some(sub.id),
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
            phoenix: false,
        })
        .await
        .unwrap();
    db.patch_task(
        manual,
        &TaskPatch::new().worktree(Some("/repo/a/.worktrees/manual")),
    )
    .await
    .unwrap();

    let removed = db
        .delete_stale_subtree_feed_tasks(parent.id, &[])
        .await
        .unwrap();

    assert!(
        removed.is_empty(),
        "manual tasks are neither deleted nor reported, got {removed:?}"
    );
    assert_eq!(db.list_tasks_for_epic(sub.id).await.unwrap().len(), 1);
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
            phoenix: false,
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
            phoenix: false,
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
    assert!(TaskPatch::new()
        .tmux_window(Some(&test_tmux_window("tw")))
        .has_changes());
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

    /// A `'static` window for the `'static`-lifetime patch below — `TaskPatch`
    /// borrows its window, so a temporary would not outlive the returned patch.
    static PATCH_WINDOW: crate::models::TmuxWindow = crate::models::TmuxWindow::from_static("w");

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
            p = p.tmux_window(Some(&PATCH_WINDOW));
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
                        phoenix: false,
                    })
                    .await
                    .unwrap();
                let baseline = db.get_task(id).await.unwrap().unwrap();

                // Generated as strings, then lifted to the typed window field:
                // `[a-z]{1,16}` is always a valid window name (non-empty, not a
                // pane id), so no generated value is dropped by the lift.
                let tmux_window: Option<Option<crate::models::TmuxWindow>> = tmux_window
                    .map(|inner| inner.map(|s| test_tmux_window(&s)));

                let mut p = TaskPatch::new();
                if let Some(t)  = title.as_deref()       { p = p.title(t); }
                if let Some(d)  = description.as_deref() { p = p.description(d); }
                if let Some(r)  = repo_path.as_deref()   { p = p.repo_path(r); }
                if let Some(bb) = base_branch.as_deref() { p = p.base_branch(bb); }
                if let Some(ref pp) = plan_path   { p = p.plan_path(pp.as_deref()); }
                if let Some(ref w)  = worktree    { p = p.worktree(w.as_deref()); }
                if let Some(ref tw) = tmux_window { p = p.tmux_window(tw.as_ref()); }
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
                        phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
async fn get_task_errors_on_unknown_wrap_up_mode_while_list_all_skips_it() {
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
            phoenix: false,
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
    let tasks = db.list_all().await.expect("bulk read must not fail");
    assert!(
        tasks.is_empty(),
        "the row with an unknown wrap_up_mode must be skipped, got {tasks:?}"
    );
    let result = db.get_task(id).await;
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
            .tmux_window(Some(&test_tmux_window("session:1394")))
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
        task.tmux_window.as_ref().map(|w| w.as_str()),
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
            phoenix: false,
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
            phoenix: false,
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

// -- try_claim_next_backlog_task --------------------------------------------

/// Helper: a subtask of `epic_id` in `status`, with an explicit `sort_order`.
async fn subtask(
    db: &Database,
    epic_id: EpicId,
    title: &str,
    status: TaskStatus,
    sort_order: Option<i64>,
) -> TaskId {
    db.create_task(CreateTaskRequest {
        title,
        description: "",
        repo_path: "/tmp/r",
        plan: None,
        status,
        base_branch: "main",
        epic_id: Some(epic_id),
        sort_order,
        tag: None,
        wrap_up_mode: None,
        auto_run_plan: false,
        phoenix: false,
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn try_claim_next_backlog_task_claims_the_lowest_sort_order_subtask() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let third = subtask(&db, epic.id, "c", TaskStatus::Backlog, Some(30)).await;
    let first = subtask(&db, epic.id, "a", TaskStatus::Backlog, Some(10)).await;
    let second = subtask(&db, epic.id, "b", TaskStatus::Backlog, Some(20)).await;

    let claimed = db
        .try_claim_next_backlog_task(epic.id, chrono::Utc::now())
        .await
        .unwrap();

    assert_eq!(claimed, Some(first));
    for untouched in [second, third] {
        assert_eq!(
            db.get_task(untouched).await.unwrap().unwrap().status,
            TaskStatus::Backlog,
            "only the selected row may be claimed"
        );
    }
}

/// The ordering key is `COALESCE(sort_order, id)` then `id` — the SQL
/// equivalent of the `(sort_order.unwrap_or(id), id)` sort this statement
/// replaced. A null-sort_order subtask sorts by its own id, so it loses to an
/// explicitly lower sort_order and wins against a higher one, regardless of
/// insertion order.
#[tokio::test]
async fn try_claim_next_backlog_task_falls_back_to_id_when_sort_order_is_null() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let unordered = subtask(&db, epic.id, "no sort_order", TaskStatus::Backlog, None).await;
    let above = subtask(&db, epic.id, "sorts after", TaskStatus::Backlog, Some(500)).await;
    let below = subtask(&db, epic.id, "sorts before", TaskStatus::Backlog, Some(0)).await;

    let now = chrono::Utc::now();
    assert_eq!(
        db.try_claim_next_backlog_task(epic.id, now).await.unwrap(),
        Some(below),
        "sort_order 0 must beat a null whose fallback key is its own id"
    );
    assert_eq!(
        db.try_claim_next_backlog_task(epic.id, now).await.unwrap(),
        Some(unordered),
        "the null-sort_order subtask beats sort_order 500 via its id fallback"
    );
    assert_eq!(
        db.try_claim_next_backlog_task(epic.id, now).await.unwrap(),
        Some(above)
    );
}

#[tokio::test]
async fn try_claim_next_backlog_task_skips_non_backlog_subtasks() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    subtask(&db, epic.id, "running", TaskStatus::Running, Some(1)).await;
    subtask(&db, epic.id, "review", TaskStatus::Review, Some(2)).await;
    subtask(&db, epic.id, "done", TaskStatus::Done, Some(3)).await;
    let backlog = subtask(&db, epic.id, "backlog", TaskStatus::Backlog, Some(4)).await;

    assert_eq!(
        db.try_claim_next_backlog_task(epic.id, chrono::Utc::now())
            .await
            .unwrap(),
        Some(backlog)
    );
}

/// `PhoenixIsNeverChained` (docs/specs/epics.allium): the chain passes OVER a
/// phoenix subtask and takes the next ordinary one behind it. Without the skip,
/// a phoenix subtask would respawn on completion and be dispatched again
/// immediately — an epic that never runs out of work.
#[tokio::test]
async fn try_claim_next_backlog_task_skips_phoenix_subtasks() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let recurring = phoenix_subtask(&db, epic.id, "recurring", Some(1)).await;
    let ordinary = subtask(&db, epic.id, "ordinary", TaskStatus::Backlog, Some(2)).await;

    assert_eq!(
        db.try_claim_next_backlog_task(epic.id, chrono::Utc::now())
            .await
            .unwrap(),
        Some(ordinary),
        "the phoenix subtask sorts first but is not a candidate"
    );
    assert_eq!(
        db.get_task(recurring).await.unwrap().unwrap().status,
        TaskStatus::Backlog,
        "and it is left in backlog, unclaimed"
    );
}

/// The fourth normal stopping condition: every backlog subtask left is a
/// phoenix one, so the chain stops rather than looping.
#[tokio::test]
async fn try_claim_next_backlog_task_is_none_when_only_phoenix_subtasks_remain() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    phoenix_subtask(&db, epic.id, "recurring", Some(1)).await;
    phoenix_subtask(&db, epic.id, "also recurring", Some(2)).await;

    assert!(db
        .try_claim_next_backlog_task(epic.id, chrono::Utc::now())
        .await
        .unwrap()
        .is_none());
}

/// Helper: a backlog subtask of `epic_id` carrying the phoenix flag.
async fn phoenix_subtask(
    db: &Database,
    epic_id: EpicId,
    title: &str,
    sort_order: Option<i64>,
) -> TaskId {
    db.create_task(CreateTaskRequest {
        title,
        description: "",
        repo_path: "/tmp/r",
        plan: None,
        status: TaskStatus::Backlog,
        base_branch: "main",
        epic_id: Some(epic_id),
        sort_order,
        tag: None,
        wrap_up_mode: None,
        auto_run_plan: false,
        phoenix: true,
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn try_claim_next_backlog_task_is_none_when_no_backlog_subtask_remains() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    subtask(&db, epic.id, "running", TaskStatus::Running, Some(1)).await;

    assert!(db
        .try_claim_next_backlog_task(epic.id, chrono::Utc::now())
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn try_claim_next_backlog_task_ignores_other_epics_subtasks() {
    let db = in_memory_db().await;
    let mine = db.create_epic("mine", "", None).await.unwrap();
    let other = db.create_epic("other", "", None).await.unwrap();
    let theirs = subtask(&db, other.id, "theirs", TaskStatus::Backlog, Some(1)).await;

    assert!(db
        .try_claim_next_backlog_task(mine.id, chrono::Utc::now())
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        db.get_task(theirs).await.unwrap().unwrap().status,
        TaskStatus::Backlog
    );
}

#[tokio::test]
async fn try_claim_next_backlog_task_applies_running_and_the_activity_stamp() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let id = subtask(&db, epic.id, "t", TaskStatus::Backlog, Some(1)).await;
    let before = db.get_task(id).await.unwrap().unwrap().updated_at;

    let claimed = db
        .try_claim_next_backlog_task(epic.id, chrono::Utc::now())
        .await
        .unwrap();

    assert_eq!(claimed, Some(id));
    let task = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.sub_status, SubStatus::default_for(TaskStatus::Running));
    assert!(
        task.last_pre_tool_use_at.is_some(),
        "the claim seeds the activity stamp so the tick classifier does not flicker the task to Stale"
    );
    assert!(task.updated_at >= before);
}

/// Selection and claim are one statement, so repeated calls walk the epic's
/// backlog and can never hand the same subtask out twice — the exclusivity
/// `AutoDispatchNextSubtask` depends on, at the layer that provides it.
#[tokio::test]
async fn try_claim_next_backlog_task_claims_each_subtask_at_most_once() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let first = subtask(&db, epic.id, "a", TaskStatus::Backlog, Some(10)).await;
    let second = subtask(&db, epic.id, "b", TaskStatus::Backlog, Some(20)).await;

    let now = chrono::Utc::now();
    assert_eq!(
        db.try_claim_next_backlog_task(epic.id, now).await.unwrap(),
        Some(first)
    );
    assert_eq!(
        db.try_claim_next_backlog_task(epic.id, now).await.unwrap(),
        Some(second)
    );
    assert!(db
        .try_claim_next_backlog_task(epic.id, now)
        .await
        .unwrap()
        .is_none());
}

// -- try_claim_backlog_task (by id) ----------------------------------------
//
// The by-id twin of the claim above, backing every dispatch entry point that is
// handed a specific task (DispatchClaimExclusive in docs/specs/dispatch.allium).

/// The phoenix skip belongs to the CHAIN, not to dispatch. `PhoenixIsNeverChained`
/// (docs/specs/epics.allium) stops an epic launching agents at a recurring task
/// on its own; it does not stop a human doing it, which is the entire point of
/// the flag. Pressing Space on a phoenix backlog card must dispatch it.
#[tokio::test]
async fn try_claim_backlog_task_claims_a_phoenix_task_the_chain_would_skip() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let id = phoenix_subtask(&db, epic.id, "recurring", Some(1)).await;

    assert!(db
        .try_claim_backlog_task(id, chrono::Utc::now())
        .await
        .unwrap());
    assert_eq!(
        db.get_task(id).await.unwrap().unwrap().status,
        TaskStatus::Running
    );
    assert!(
        db.get_task(id).await.unwrap().unwrap().phoenix,
        "dispatching does not consume the flag; only entering Done does"
    );
}

#[tokio::test]
async fn try_claim_backlog_task_applies_the_full_claim() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let id = subtask(&db, epic.id, "target", TaskStatus::Backlog, Some(1)).await;

    assert!(db
        .try_claim_backlog_task(id, chrono::Utc::now())
        .await
        .unwrap());

    // Same SET list as the by-epic claim — asserted here so the two cannot drift.
    let claimed = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(claimed.status, TaskStatus::Running);
    assert_eq!(
        claimed.sub_status,
        SubStatus::default_for(TaskStatus::Running)
    );
    assert!(claimed.last_pre_tool_use_at.is_some());
    assert!(
        claimed.worktree.is_none(),
        "the claim runs ahead of provisioning"
    );
}

#[tokio::test]
async fn try_claim_backlog_task_is_false_for_a_task_out_of_backlog() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let id = subtask(&db, epic.id, "running", TaskStatus::Running, Some(1)).await;

    assert!(!db
        .try_claim_backlog_task(id, chrono::Utc::now())
        .await
        .unwrap());
    assert!(
        db.get_task(id)
            .await
            .unwrap()
            .unwrap()
            .last_pre_tool_use_at
            .is_none(),
        "a lost claim writes nothing at all — one statement, so it cannot half-apply"
    );
}

#[tokio::test]
async fn try_claim_backlog_task_is_false_for_a_missing_task() {
    let db = in_memory_db().await;
    assert!(!db
        .try_claim_backlog_task(TaskId(999_999), chrono::Utc::now())
        .await
        .unwrap());
}

#[tokio::test]
async fn try_claim_backlog_task_claims_at_most_once() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let id = subtask(&db, epic.id, "target", TaskStatus::Backlog, Some(1)).await;
    let now = chrono::Utc::now();

    assert!(db.try_claim_backlog_task(id, now).await.unwrap());
    assert!(
        !db.try_claim_backlog_task(id, now).await.unwrap(),
        "the row has left Backlog, so a second claim on it must lose"
    );
}

// -- try_release_backlog_claim ----------------------------------------------

/// Helper: a backlog subtask, claimed, ready to have its claim released.
async fn claimed_task(db: &Database) -> TaskId {
    let epic = db.create_epic("E", "", None).await.unwrap();
    let id = subtask(db, epic.id, "t", TaskStatus::Backlog, None).await;
    assert_eq!(
        db.try_claim_next_backlog_task(epic.id, chrono::Utc::now())
            .await
            .unwrap(),
        Some(id)
    );
    id
}

#[tokio::test]
async fn try_release_backlog_claim_undoes_an_unprovisioned_claim() {
    let db = in_memory_db().await;
    let id = claimed_task(&db).await;

    assert!(db.try_release_backlog_claim(id).await.unwrap());
    let released = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(released.status, TaskStatus::Backlog);
    assert_eq!(
        released.sub_status,
        SubStatus::default_for(TaskStatus::Backlog)
    );
    // The stamp the claim seeded is cleared, or the task is not quite "as it
    // was before the chain fired".
    assert!(released.last_pre_tool_use_at.is_none());
}

#[tokio::test]
async fn try_release_backlog_claim_spares_a_provisioned_task() {
    let db = in_memory_db().await;
    let id = claimed_task(&db).await;
    // Provisioning landed: the dispatch succeeded and recorded a worktree.
    db.patch_task(id, &TaskPatch::new().worktree(Some("/tmp/wt")))
        .await
        .unwrap();

    // The release must not stomp a task that is genuinely running.
    assert!(!db.try_release_backlog_claim(id).await.unwrap());
    let kept = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(kept.status, TaskStatus::Running);
    assert_eq!(kept.worktree.as_deref(), Some("/tmp/wt"));
}

#[tokio::test]
async fn try_release_backlog_claim_spares_a_task_moved_out_of_running() {
    let db = in_memory_db().await;
    let id = claimed_task(&db).await;
    // A human moved it on while provisioning was still in flight.
    db.patch_task(id, &TaskPatch::new().status(TaskStatus::Review))
        .await
        .unwrap();

    assert!(!db.try_release_backlog_claim(id).await.unwrap());
    assert_eq!(
        db.get_task(id).await.unwrap().unwrap().status,
        TaskStatus::Review
    );
}

#[tokio::test]
async fn try_release_backlog_claim_is_false_for_missing_task() {
    let db = in_memory_db().await;
    assert!(!db.try_release_backlog_claim(TaskId(999_999)).await.unwrap());
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
            phoenix: false,
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
            phoenix: false,
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
        phoenix: false,
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

// ---------------------------------------------------------------------------
// Decode-failure policy: skip-and-warn for bulk reads, fail-loud for
// single-entity reads. See the decode-failure-policy section of
// docs/conventions.md.
// ---------------------------------------------------------------------------

/// Plant an undecodable task row (unrecognised `status`) alongside a healthy
/// one and return the healthy task's id.
async fn db_with_undecodable_status_row() -> (Database, TaskId) {
    let db = in_memory_db().await;
    let good = create_task_returning(&db, "healthy", "", "/repo", None, TaskStatus::Backlog)
        .await
        .unwrap();
    write_corrupt_row(
        &db,
        "INSERT INTO tasks (id, title, description, repo_path, status, sub_status,
                            base_branch, created_at, updated_at)
         VALUES (9001, 'corrupt', '', '/repo', 'not_a_status', 'none', 'main',
                 '2026-01-01 00:00:00', '2026-01-01 00:00:00');",
    )
    .await;
    (db, good.id)
}

#[tokio::test]
async fn list_all_skips_row_with_unrecognised_status() {
    // Covers the skip-and-warn behaviour for every bulk task read: they all
    // funnel their `query_map` iterator through the same `collect_decodable`
    // row decoder (see "Bulk reads skip and warn" in docs/conventions.md).
    let (db, good_id) = db_with_undecodable_status_row().await;
    let before = crate::db::decode_fallback_count();

    let tasks = db
        .list_all()
        .await
        .expect("one undecodable row must not fail the whole board load");

    assert_eq!(
        tasks.iter().map(|t| t.id).collect::<Vec<_>>(),
        vec![good_id],
        "the healthy row must still load; the corrupt row must be skipped"
    );
    assert!(
        crate::db::decode_fallback_count() > before,
        "skipping a row must bump the decode-fallback counter"
    );
}

/// A `tmux_window` value that is not a window name — a pane id, or the empty
/// string — soft-fails to `None` rather than failing the row. The task then
/// reads back as owning no window, which is exactly what a task whose agent is
/// gone looks like; failing the row would drop the card from every bulk read.
#[tokio::test]
async fn malformed_stored_tmux_window_decodes_to_none() {
    for stored in ["%3", ""] {
        let db = in_memory_db().await;
        let task = create_task_returning(&db, "windowed", "", "/repo", None, TaskStatus::Running)
            .await
            .unwrap();
        db.db_call(move |conn| {
            conn.execute(
                "UPDATE tasks SET tmux_window = ?1 WHERE id = ?2",
                rusqlite::params![stored, task.id.0],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let before = crate::db::decode_fallback_count();

        let read = db.get_task(task.id).await.unwrap().unwrap();

        assert_eq!(read.tmux_window, None, "stored {stored:?}");
        assert_eq!(read.title, "windowed", "the rest of the row must survive");
        // The empty string reads back as SQL NULL-equivalent "no window" too,
        // but only a non-empty malformed value is a decode fallback worth
        // counting — an empty column is indistinguishable from an unset one.
        if !stored.is_empty() {
            assert!(
                crate::db::decode_fallback_count() > before,
                "a malformed stored window must bump the decode-fallback counter"
            );
        }
    }
}

/// A window name written by an older binary that this one has no opinion about
/// still round-trips: `parse` only rejects the two strings that are not window
/// names, so an unfamiliar-but-valid name is preserved verbatim.
#[tokio::test]
async fn unfamiliar_stored_tmux_window_round_trips() {
    let db = in_memory_db().await;
    let task = create_task_returning(&db, "windowed", "", "/repo", None, TaskStatus::Running)
        .await
        .unwrap();
    db.db_call(move |conn| {
        conn.execute(
            "UPDATE tasks SET tmux_window = 'session:1-legacy' WHERE id = ?1",
            rusqlite::params![task.id.0],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    let read = db.get_task(task.id).await.unwrap().unwrap();

    assert_eq!(
        read.tmux_window.as_ref().map(|w| w.as_str()),
        Some("session:1-legacy")
    );
}

#[tokio::test]
async fn get_task_errors_on_unrecognised_status() {
    let (db, _) = db_with_undecodable_status_row().await;
    let result = db.get_task(TaskId(9001)).await;
    assert!(
        result.is_err(),
        "a single-entity read must fail loudly for the row the caller asked for, got {result:?}"
    );
    let msg = format!("{:#}", result.unwrap_err());
    assert!(
        msg.contains("not_a_status"),
        "error must name the offending value, got: {msg}"
    );
}

/// Plant a task row whose `url`/`url_type` pair is inconsistent — a state the
/// application can never write, but which a partially-applied migration could
/// leave behind.
async fn db_with_inconsistent_url_row() -> (Database, TaskId, TaskId) {
    let db = in_memory_db().await;
    let good = create_task_returning(&db, "healthy", "", "/repo", None, TaskStatus::Backlog)
        .await
        .unwrap();
    let bad = create_task_returning(&db, "corrupt", "", "/repo", None, TaskStatus::Backlog)
        .await
        .unwrap();
    let bad_id = bad.id.0;
    db.db_call(move |conn| {
        conn.execute(
            "UPDATE tasks SET url = 'https://example.com/pull/1', url_type = NULL WHERE id = ?1",
            rusqlite::params![bad_id],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    (db, good.id, bad.id)
}

#[tokio::test]
async fn list_all_skips_row_with_inconsistent_url_pair() {
    let (db, good_id, _) = db_with_inconsistent_url_row().await;
    let tasks = db
        .list_all()
        .await
        .expect("an inconsistent url/url_type pair must not fail the whole board load");
    assert_eq!(
        tasks.iter().map(|t| t.id).collect::<Vec<_>>(),
        vec![good_id]
    );
}

#[tokio::test]
async fn get_task_errors_on_inconsistent_url_pair() {
    let (db, _, bad_id) = db_with_inconsistent_url_row().await;
    let result = db.get_task(bad_id).await;
    assert!(
        result.is_err(),
        "a single-entity read must fail loudly on a corrupt url/url_type pair, got {result:?}"
    );
}

#[tokio::test]
async fn find_task_by_plan_errors_on_undecodable_row() {
    let db = in_memory_db().await;
    write_corrupt_row(
        &db,
        "INSERT INTO tasks (id, title, description, repo_path, status, sub_status,
                            base_branch, plan_path, created_at, updated_at)
         VALUES (9002, 'corrupt', '', '/repo', 'not_a_status', 'none', 'main', '/p/plan.md',
                 '2026-01-01 00:00:00', '2026-01-01 00:00:00');",
    )
    .await;
    let result = db.find_task_by_plan("/p/plan.md").await;
    assert!(
        result.is_err(),
        "find_task_by_plan targets one row, so it must fail loudly, got {result:?}"
    );
}
