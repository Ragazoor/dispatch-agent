//! Integration test: full task lifecycle through App::update() with a real (in-memory) DB.

use task_orchestrator::db::{Database, TaskStore};
use task_orchestrator::models::{Task, TaskStatus};
use task_orchestrator::tui::{App, Command, Message, MoveDirection};

fn make_app() -> (App, Database) {
    let db = Database::open_in_memory().unwrap();
    let app = App::new(vec![]);
    (app, db)
}

/// Helper: execute PersistTask/DeleteTask commands against the DB.
fn execute(db: &Database, cmds: &[Command]) {
    for cmd in cmds {
        match cmd {
            Command::PersistTask(task) => {
                let _ = db.persist_task(
                    task.id,
                    task.status,
                    task.worktree.as_deref(),
                    task.tmux_window.as_deref(),
                );
            }
            Command::DeleteTask(id) => {
                let _ = db.delete_task(*id);
            }
            _ => {}
        }
    }
}

#[test]
fn full_lifecycle() {
    let (mut app, db) = make_app();

    // 1. Create task: simulate what exec_insert_task does (DB insert + TaskCreated message)
    let task_id = db
        .create_task("Fix auth bug", "Users can't log in", "/repo", None, TaskStatus::Backlog)
        .unwrap();
    let now = chrono::Utc::now();
    let cmds = app.update(Message::TaskCreated {
        task: Task {
            id: task_id,
            title: "Fix auth bug".to_string(),
            description: "Users can't log in".to_string(),
            repo_path: "/repo".to_string(),
            status: TaskStatus::Backlog,
            worktree: None,
            tmux_window: None,
            plan: None,
            created_at: now,
            updated_at: now,
        },
    });
    assert!(cmds.is_empty());
    assert_eq!(app.tasks().len(), 1);
    assert_eq!(app.tasks()[0].status, TaskStatus::Backlog);
    assert_ne!(app.tasks()[0].id, 0, "ID should be assigned by DB");

    // Verify DB has the task
    let db_task = db.get_task(task_id).unwrap().unwrap();
    assert_eq!(db_task.title, "Fix auth bug");

    // 2. Move to Ready → PersistTask command issued
    let cmds = app.update(Message::MoveTask {
        id: task_id,
        direction: MoveDirection::Forward,
    });
    assert!(matches!(cmds[0], Command::PersistTask(_)));
    execute(&db, &cmds);
    assert_eq!(app.tasks()[0].status, TaskStatus::Ready);

    let db_task = db.get_task(task_id).unwrap().unwrap();
    assert_eq!(db_task.status, TaskStatus::Ready);

    // 3. Dispatch → Dispatch command issued
    let cmds = app.update(Message::DispatchTask(task_id));
    assert!(matches!(cmds[0], Command::Dispatch { .. }));

    // Simulate dispatch result → moves to Running
    let cmds = app.update(Message::Dispatched {
        id: task_id,
        worktree: "/repo/.worktrees/1-fix-auth-bug".to_string(),
        tmux_window: "task-1".to_string(),
    });
    execute(&db, &cmds);
    assert_eq!(app.tasks()[0].status, TaskStatus::Running);
    assert_eq!(
        app.tasks()[0].tmux_window.as_deref(),
        Some("task-1")
    );

    // 4. WindowGone → clears tmux_window, keeps task Running
    let cmds = app.update(Message::WindowGone(task_id));
    execute(&db, &cmds);
    assert_eq!(app.tasks()[0].status, TaskStatus::Running);
    assert!(app.tasks()[0].tmux_window.is_none());

    // 4b. Agent advances task to Review via MCP (simulated as MoveTask)
    let cmds = app.update(Message::MoveTask {
        id: task_id,
        direction: MoveDirection::Forward,
    });
    execute(&db, &cmds);
    assert_eq!(app.tasks()[0].status, TaskStatus::Review);

    // 5. Move to Done → PersistTask
    let cmds = app.update(Message::MoveTask {
        id: task_id,
        direction: MoveDirection::Forward,
    });
    execute(&db, &cmds);
    assert_eq!(app.tasks()[0].status, TaskStatus::Done);

    let db_task = db.get_task(task_id).unwrap().unwrap();
    assert_eq!(db_task.status, TaskStatus::Done);

    // 6. Delete → removed from state and DB
    let cmds = app.update(Message::DeleteTask(task_id));
    execute(&db, &cmds);
    assert!(app.tasks().is_empty());

    let db_task = db.get_task(task_id).unwrap();
    assert!(db_task.is_none());
}
