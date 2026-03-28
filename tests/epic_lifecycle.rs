use task_orchestrator::db::{Database, TaskStore};
use task_orchestrator::models::*;

#[test]
fn full_epic_lifecycle() {
    let db = Database::open_in_memory().unwrap();

    // 1. Create an epic
    let epic = db
        .create_epic(
            "Auth Rewrite",
            "Rewrite auth system",
            "## Plan\n- Extract middleware\n- Add JWT\n- Remove legacy",
            "/repo",
        )
        .unwrap();

    // 2. Create subtasks linked to epic
    let sub1 = db
        .create_task("Extract middleware", "desc", "/repo", None, TaskStatus::Backlog)
        .unwrap();
    let sub2 = db
        .create_task("Add JWT validation", "desc", "/repo", None, TaskStatus::Backlog)
        .unwrap();
    db.set_task_epic_id(sub1, Some(epic.id)).unwrap();
    db.set_task_epic_id(sub2, Some(epic.id)).unwrap();

    // 3. Verify epic status is Backlog (all subtasks in backlog)
    let tasks = db.list_tasks_for_epic(epic.id).unwrap();
    let statuses: Vec<_> = tasks.iter().map(|t| t.status).collect();
    assert_eq!(epic_status(&epic, &statuses), TaskStatus::Backlog);

    // 4. Move a subtask to Running
    db.update_status(sub1, TaskStatus::Running).unwrap();
    let tasks = db.list_tasks_for_epic(epic.id).unwrap();
    let statuses: Vec<_> = tasks.iter().map(|t| t.status).collect();
    assert_eq!(epic_status(&epic, &statuses), TaskStatus::Running);

    // 5. Move all subtasks to Done
    db.update_status(sub1, TaskStatus::Done).unwrap();
    db.update_status(sub2, TaskStatus::Done).unwrap();
    let tasks = db.list_tasks_for_epic(epic.id).unwrap();
    let statuses: Vec<_> = tasks.iter().map(|t| t.status).collect();
    assert_eq!(epic_status(&epic, &statuses), TaskStatus::Review);

    // 6. Mark epic as done
    db.update_epic(epic.id, None, None, None, Some(true))
        .unwrap();
    let epic = db.get_epic(epic.id).unwrap().unwrap();
    assert_eq!(epic_status(&epic, &statuses), TaskStatus::Done);

    // 7. Delete epic cascades
    db.delete_epic(epic.id).unwrap();
    assert!(db.get_epic(epic.id).unwrap().is_none());
    assert!(db.get_task(sub1).unwrap().is_none());
    assert!(db.get_task(sub2).unwrap().is_none());
}
