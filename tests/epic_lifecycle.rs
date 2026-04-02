use dispatch_agent::db::{Database, EpicPatch, TaskPatch, TaskStore};
use dispatch_agent::models::*;

#[test]
fn full_epic_lifecycle() {
    let db = Database::open_in_memory().unwrap();

    // 1. Create an epic
    let epic = db
        .create_epic("Auth Rewrite", "Rewrite auth system", "/repo")
        .unwrap();

    // 2. Create subtasks linked to epic
    let sub1 = db
        .create_task(
            "Extract middleware",
            "desc",
            "/repo",
            None,
            TaskStatus::Backlog,
        )
        .unwrap();
    let sub2 = db
        .create_task(
            "Add JWT validation",
            "desc",
            "/repo",
            None,
            TaskStatus::Backlog,
        )
        .unwrap();
    db.set_task_epic_id(sub1, Some(epic.id)).unwrap();
    db.set_task_epic_id(sub2, Some(epic.id)).unwrap();

    // 3. Verify epic status is Backlog (new epics start as Backlog)
    let epic = db.get_epic(epic.id).unwrap().unwrap();
    assert_eq!(epic_status(&epic), TaskStatus::Backlog);

    // 4. Move epic status to Running
    db.patch_epic(epic.id, &EpicPatch::new().status(TaskStatus::Running))
        .unwrap();
    let epic = db.get_epic(epic.id).unwrap().unwrap();
    assert_eq!(epic_status(&epic), TaskStatus::Running);

    // 5. Move all subtasks to Done, advance epic to Review
    db.patch_task(sub1, &TaskPatch::new().status(TaskStatus::Done))
        .unwrap();
    db.patch_task(sub2, &TaskPatch::new().status(TaskStatus::Done))
        .unwrap();
    db.patch_epic(epic.id, &EpicPatch::new().status(TaskStatus::Review))
        .unwrap();
    let epic = db.get_epic(epic.id).unwrap().unwrap();
    assert_eq!(epic_status(&epic), TaskStatus::Review);

    // 6. Mark epic as done
    db.patch_epic(epic.id, &EpicPatch::new().status(TaskStatus::Done))
        .unwrap();
    let epic = db.get_epic(epic.id).unwrap().unwrap();
    assert_eq!(epic_status(&epic), TaskStatus::Done);

    // 7. Delete epic cascades
    db.delete_epic(epic.id).unwrap();
    assert!(db.get_epic(epic.id).unwrap().is_none());
    assert!(db.get_task(sub1).unwrap().is_none());
    assert!(db.get_task(sub2).unwrap().is_none());
}
