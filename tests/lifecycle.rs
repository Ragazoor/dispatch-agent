#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Integration test: full task lifecycle through App::update() with a real (in-memory) DB.

use dispatch_tui::db::{self, CreateTaskRequest, Database, TaskCrud, TaskRead};
use dispatch_tui::models::{DispatchMode, Task, TaskId, TaskStatus};
use dispatch_tui::tui::{App, Command, Message, MoveDirection};

async fn make_app() -> (App, Database) {
    let db = Database::open_in_memory().await.unwrap();
    let app = App::new(vec![]);
    (app, db)
}

/// Helper: execute PersistTask/DeleteTask commands against the DB.
async fn execute(db: &Database, cmds: &[Command]) {
    for cmd in cmds {
        match cmd {
            Command::Task(dispatch_tui::tui::commands::TaskCommand::Persist(task)) => {
                let _ = db
                    .patch_task(
                        task.id,
                        &db::TaskPatch::new()
                            .status(task.status)
                            .worktree(task.worktree.as_deref())
                            .tmux_window(task.tmux_window.as_deref()),
                    )
                    .await;
            }
            Command::Task(dispatch_tui::tui::commands::TaskCommand::Delete(id)) => {
                let _ = db.delete_task(*id).await;
            }
            _ => {}
        }
    }
}

#[tokio::test]
async fn full_lifecycle() {
    let (mut app, db) = make_app().await;

    // 1. Create task with a plan: simulate what exec_insert_task does (DB insert + TaskCreated message)
    let task_id = db
        .create_task(CreateTaskRequest {
            title: "Fix auth bug",
            description: "Users can't log in",
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
    let now = chrono::Utc::now();
    let cmds = app.update(Message::Task(
        dispatch_tui::tui::messages::TaskMessage::Created {
            task: Task {
                id: task_id,
                title: "Fix auth bug".to_string(),
                description: "Users can't log in".to_string(),
                repo_path: "/repo".to_string(),
                status: TaskStatus::Backlog,
                worktree: None,
                tmux_window: None,
                plan_path: Some("plan.md".into()),
                epic_id: None,
                sub_status: dispatch_tui::models::SubStatus::None,
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
                wrap_up_mode: None,
                auto_run_plan: false,
            },
        },
    ));
    assert!(cmds.is_empty());
    assert_eq!(app.tasks().len(), 1);
    assert_eq!(app.tasks()[0].status, TaskStatus::Backlog);
    assert_ne!(app.tasks()[0].id, TaskId(0), "ID should be assigned by DB");

    // Verify DB has the task
    let db_task = db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(db_task.title, "Fix auth bug");

    // 2. Dispatch directly from Backlog (task has a plan) → Dispatch command issued
    let cmds = app.update(Message::Task(
        dispatch_tui::tui::messages::TaskMessage::Dispatch(task_id, DispatchMode::Dispatch),
    ));
    assert!(matches!(
        cmds[0],
        Command::Task(dispatch_tui::tui::commands::TaskCommand::DispatchAgent { .. })
    ));

    // Simulate dispatch result → moves to Running
    let cmds = app.update(Message::Task(
        dispatch_tui::tui::messages::TaskMessage::Dispatched {
            id: task_id,
            worktree: "/repo/.worktrees/1-fix-auth-bug".to_string(),
            tmux_window: "task-1".to_string(),
            switch_focus: false,
        },
    ));
    execute(&db, &cmds).await;
    assert_eq!(app.tasks()[0].status, TaskStatus::Running);
    assert_eq!(app.tasks()[0].tmux_window.as_deref(), Some("task-1"));

    // 4. WindowGone on a Running task → marks as crashed (tmux_window cleared, window is gone)
    let cmds = app.update(Message::Task(
        dispatch_tui::tui::messages::TaskMessage::WindowGone(task_id),
    ));
    execute(&db, &cmds).await;
    assert_eq!(app.tasks()[0].status, TaskStatus::Running);
    // tmux_window is cleared — the window is gone by definition
    assert!(app.tasks()[0].tmux_window.is_none());
    assert!(app.is_crashed(task_id));

    // 4b. Agent advances task to Review via MCP (simulated as MoveTask)
    let cmds = app.update(Message::Task(
        dispatch_tui::tui::messages::TaskMessage::Move {
            id: task_id,
            direction: MoveDirection::Forward,
        },
    ));
    execute(&db, &cmds).await;
    assert_eq!(app.tasks()[0].status, TaskStatus::Review);

    // 5. Move to Done → requires confirmation
    let cmds = app.update(Message::Task(
        dispatch_tui::tui::messages::TaskMessage::Move {
            id: task_id,
            direction: MoveDirection::Forward,
        },
    ));
    assert!(
        cmds.is_empty(),
        "MoveTask should not produce commands when entering ConfirmDone"
    );
    assert_eq!(
        app.tasks()[0].status,
        TaskStatus::Review,
        "Task stays in Review until confirmed"
    );

    // Confirm the Done transition
    let cmds = app.update(Message::Input(
        dispatch_tui::tui::messages::InputMessage::ConfirmDone,
    ));
    execute(&db, &cmds).await;
    assert_eq!(app.tasks()[0].status, TaskStatus::Done);

    let db_task = db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(db_task.status, TaskStatus::Done);

    // 6. Delete → removed from state and DB
    let cmds = app.update(Message::Task(
        dispatch_tui::tui::messages::TaskMessage::Delete(task_id),
    ));
    execute(&db, &cmds).await;
    assert!(app.tasks().is_empty());

    let db_task = db.get_task(task_id).await.unwrap();
    assert!(db_task.is_none());
}
