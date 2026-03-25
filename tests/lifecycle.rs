//! Integration test: full task lifecycle through App::update() with a real (in-memory) DB.

use task_orchestrator::db::Database;
use task_orchestrator::models::TaskStatus;
use task_orchestrator::tui::{App, Command, Message, MoveDirection};

fn make_app() -> (App, Database) {
    let db = Database::open_in_memory().unwrap();
    let app = App::new(vec![]);
    (app, db)
}

/// Helper: execute PersistTask commands against the DB and sync IDs back to the app.
fn persist(app: &mut App, db: &Database, cmds: &[Command]) {
    for cmd in cmds {
        if let Command::PersistTask(task) = cmd {
            if task.id == 0 {
                let new_id = db
                    .create_task(&task.title, &task.description, &task.repo_path)
                    .unwrap();
                if let Some(t) = app.tasks.iter_mut().find(|t| t.id == 0) {
                    t.id = new_id;
                }
            } else {
                let _ = db.update_status(task.id, task.status);
                let _ = db.update_dispatch(
                    task.id,
                    task.worktree.as_deref(),
                    task.tmux_window.as_deref(),
                );
            }
        }
        if let Command::DeleteTask(id) = cmd {
            let _ = db.delete_task(*id);
        }
    }
}

#[test]
fn full_lifecycle() {
    let (mut app, db) = make_app();

    // 1. Create task → appears in Backlog
    let cmds = app.update(Message::CreateTask {
        title: "Fix auth bug".to_string(),
        description: "Users can't log in".to_string(),
        repo_path: "/repo".to_string(),
    });
    persist(&mut app, &db, &cmds);
    assert_eq!(app.tasks.len(), 1);
    assert_eq!(app.tasks[0].status, TaskStatus::Backlog);
    assert_ne!(app.tasks[0].id, 0, "ID should be assigned by DB");

    let task_id = app.tasks[0].id;

    // Verify DB has the task
    let db_task = db.get_task(task_id).unwrap().unwrap();
    assert_eq!(db_task.title, "Fix auth bug");

    // 2. Move to Ready → PersistTask command issued
    let cmds = app.update(Message::MoveTask {
        id: task_id,
        direction: MoveDirection::Forward,
    });
    assert!(matches!(cmds[0], Command::PersistTask(_)));
    persist(&mut app, &db, &cmds);
    assert_eq!(app.tasks[0].status, TaskStatus::Ready);

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
    persist(&mut app, &db, &cmds);
    assert_eq!(app.tasks[0].status, TaskStatus::Running);
    assert_eq!(
        app.tasks[0].tmux_window.as_deref(),
        Some("task-1")
    );

    // 4. WindowGone → auto-advances to Review
    let cmds = app.update(Message::WindowGone(task_id));
    persist(&mut app, &db, &cmds);
    assert_eq!(app.tasks[0].status, TaskStatus::Review);

    // 5. Move to Done → PersistTask
    let cmds = app.update(Message::MoveTask {
        id: task_id,
        direction: MoveDirection::Forward,
    });
    persist(&mut app, &db, &cmds);
    assert_eq!(app.tasks[0].status, TaskStatus::Done);

    let db_task = db.get_task(task_id).unwrap().unwrap();
    assert_eq!(db_task.status, TaskStatus::Done);

    // 6. Delete → removed from state and DB
    let cmds = app.update(Message::DeleteTask(task_id));
    persist(&mut app, &db, &cmds);
    assert!(app.tasks.is_empty());

    let db_task = db.get_task(task_id).unwrap();
    assert!(db_task.is_none());
}

#[test]
fn dispatch_only_from_ready() {
    let (mut app, _db) = make_app();

    // Create task in Backlog
    app.update(Message::CreateTask {
        title: "Task".to_string(),
        description: "desc".to_string(),
        repo_path: "/repo".to_string(),
    });
    let task_id = app.tasks[0].id;

    // Try dispatch from Backlog — should be no-op
    let cmds = app.update(Message::DispatchTask(task_id));
    assert!(cmds.is_empty());
}

#[test]
fn window_gone_only_advances_running() {
    let (mut app, _db) = make_app();

    // Create a task in Review (simulate: create, advance to Review)
    app.update(Message::CreateTask {
        title: "Task".to_string(),
        description: "desc".to_string(),
        repo_path: "/repo".to_string(),
    });
    let task_id = app.tasks[0].id;

    // Manually set to Review
    app.update(Message::MoveTask {
        id: task_id,
        direction: MoveDirection::Forward,
    }); // Ready
    app.update(Message::MoveTask {
        id: task_id,
        direction: MoveDirection::Forward,
    }); // Running
    app.update(Message::MoveTask {
        id: task_id,
        direction: MoveDirection::Forward,
    }); // Review

    assert_eq!(app.tasks[0].status, TaskStatus::Review);

    // WindowGone should NOT advance from Review
    let cmds = app.update(Message::WindowGone(task_id));
    assert!(cmds.is_empty());
    assert_eq!(app.tasks[0].status, TaskStatus::Review);
}
