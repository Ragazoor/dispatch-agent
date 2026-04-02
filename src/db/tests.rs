use super::*;

fn in_memory_db() -> Database {
    Database::open_in_memory().unwrap()
}

#[test]
fn create_and_get() {
    let db = in_memory_db();
    let id = db
        .create_task(
            "My Task",
            "A description",
            "/repo/path",
            None,
            TaskStatus::Backlog,
        )
        .unwrap();
    let task = db.get_task(id).unwrap().expect("task should exist");
    assert_eq!(task.id, id);
    assert_eq!(task.title, "My Task");
    assert_eq!(task.description, "A description");
    assert_eq!(task.repo_path, "/repo/path");
    assert_eq!(task.status, TaskStatus::Backlog);
    assert!(task.worktree.is_none());
    assert!(task.tmux_window.is_none());
}

#[test]
fn list_all() {
    let db = in_memory_db();
    db.create_task("Task A", "desc", "/a", None, TaskStatus::Backlog)
        .unwrap();
    db.create_task("Task B", "desc", "/b", None, TaskStatus::Backlog)
        .unwrap();
    db.create_task("Task C", "desc", "/c", None, TaskStatus::Backlog)
        .unwrap();
    let tasks = db.list_all().unwrap();
    assert_eq!(tasks.len(), 3);
    assert_eq!(tasks[0].title, "Task A");
    assert_eq!(tasks[1].title, "Task B");
    assert_eq!(tasks[2].title, "Task C");
}

#[test]
fn list_by_status() {
    let db = in_memory_db();
    let id1 = db
        .create_task("Task A", "desc", "/a", None, TaskStatus::Backlog)
        .unwrap();
    let id2 = db
        .create_task("Task B", "desc", "/b", None, TaskStatus::Backlog)
        .unwrap();
    db.create_task("Task C", "desc", "/c", None, TaskStatus::Backlog)
        .unwrap();

    db.patch_task(id1, &TaskPatch::new().status(TaskStatus::Running))
        .unwrap();
    db.patch_task(id2, &TaskPatch::new().status(TaskStatus::Running))
        .unwrap();

    let running = db.list_by_status(TaskStatus::Running).unwrap();
    assert_eq!(running.len(), 2);

    let backlog = db.list_by_status(TaskStatus::Backlog).unwrap();
    assert_eq!(backlog.len(), 1);
    assert_eq!(backlog[0].title, "Task C");
}

#[test]
fn get_nonexistent() {
    let db = in_memory_db();
    let result = db.get_task(TaskId(9999)).unwrap();
    assert!(result.is_none());
}

#[test]
fn create_task_with_plan() {
    let db = in_memory_db();
    let id = db
        .create_task(
            "Planned Task",
            "desc",
            "/repo",
            Some("docs/plan.md"),
            TaskStatus::Backlog,
        )
        .unwrap();
    let task = db.get_task(id).unwrap().unwrap();
    assert_eq!(task.plan.as_deref(), Some("docs/plan.md"));
}

#[test]
fn create_task_without_plan() {
    let db = in_memory_db();
    let id = db
        .create_task("Simple Task", "desc", "/repo", None, TaskStatus::Backlog)
        .unwrap();
    let task = db.get_task(id).unwrap().unwrap();
    assert!(task.plan.is_none());
}

#[test]
fn find_task_by_plan_returns_match() {
    let db = in_memory_db();
    let id = db
        .create_task(
            "Planned",
            "desc",
            "/repo",
            Some("/plans/my-plan.md"),
            TaskStatus::Backlog,
        )
        .unwrap();

    let found = db.find_task_by_plan("/plans/my-plan.md").unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().id, id);
}

#[test]
fn find_task_by_plan_returns_none_when_no_match() {
    let db = in_memory_db();
    db.create_task(
        "Other",
        "desc",
        "/repo",
        Some("/plans/other.md"),
        TaskStatus::Backlog,
    )
    .unwrap();

    let found = db.find_task_by_plan("/plans/nonexistent.md").unwrap();
    assert!(found.is_none());
}

#[test]
fn find_task_by_plan_ignores_tasks_without_plan() {
    let db = in_memory_db();
    db.create_task("No Plan", "desc", "/repo", None, TaskStatus::Backlog)
        .unwrap();

    let found = db.find_task_by_plan("/plans/any.md").unwrap();
    assert!(found.is_none());
}

#[test]
fn get_setting_bool_returns_none_when_absent() {
    let db = Database::open_in_memory().unwrap();
    assert_eq!(db.get_setting_bool("notifications_enabled").unwrap(), None);
}

#[test]
fn set_and_get_setting_bool_roundtrips() {
    let db = Database::open_in_memory().unwrap();
    db.set_setting_bool("notifications_enabled", true).unwrap();
    assert_eq!(
        db.get_setting_bool("notifications_enabled").unwrap(),
        Some(true)
    );

    db.set_setting_bool("notifications_enabled", false).unwrap();
    assert_eq!(
        db.get_setting_bool("notifications_enabled").unwrap(),
        Some(false)
    );
}

#[test]
fn get_setting_string_returns_none_when_absent() {
    let db = Database::open_in_memory().unwrap();
    assert_eq!(db.get_setting_string("repo_filter").unwrap(), None);
}

#[test]
fn set_and_get_setting_string() {
    let db = Database::open_in_memory().unwrap();
    db.set_setting_string("repo_filter", "/repo1\n/repo2")
        .unwrap();
    assert_eq!(
        db.get_setting_string("repo_filter").unwrap(),
        Some("/repo1\n/repo2".to_string())
    );
}

#[test]
fn set_setting_string_upserts() {
    let db = Database::open_in_memory().unwrap();
    db.set_setting_string("repo_filter", "old").unwrap();
    db.set_setting_string("repo_filter", "new").unwrap();
    assert_eq!(
        db.get_setting_string("repo_filter").unwrap(),
        Some("new".to_string())
    );
}

#[test]
fn fresh_db_has_latest_schema_version() {
    let db = in_memory_db();
    let conn = db.conn.lock().unwrap();
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 23, "fresh DB should be at schema version 23");
}

#[test]
fn legacy_db_migrates_to_latest_version() {
    // Simulate a pre-versioning DB: create tables manually including notes
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys=ON;
         CREATE TABLE tasks (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path TEXT NOT NULL,
             status TEXT NOT NULL DEFAULT 'backlog',
             worktree TEXT,
             tmux_window TEXT,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE notes (
             id INTEGER PRIMARY KEY,
             task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
             content TEXT NOT NULL,
             source TEXT NOT NULL DEFAULT 'user',
             created_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE repo_paths (
             id INTEGER PRIMARY KEY,
             path TEXT NOT NULL UNIQUE,
             last_used TEXT NOT NULL DEFAULT (datetime('now'))
         );",
    )
    .unwrap();

    // Insert a note so we can verify the table gets dropped
    conn.execute(
        "INSERT INTO tasks (title, description, repo_path) VALUES ('T', 'D', '/r')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO notes (task_id, content) VALUES (1, 'hello')",
        [],
    )
    .unwrap();

    // Run init_schema which should migrate
    Database::init_schema(&conn).unwrap();

    // Notes table should be gone
    let table_exists: bool = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name='notes'")
        .unwrap()
        .exists([])
        .unwrap();
    assert!(
        !table_exists,
        "notes table should be dropped after migration"
    );

    // Version should be latest
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 23);

    // Verify Migration 1 added the plan column
    let has_plan: bool = conn.prepare("SELECT plan FROM tasks LIMIT 1").is_ok();
    assert!(has_plan, "Migration 1 should have added the plan column");
}

#[test]
fn migration_6_converts_ready_to_backlog() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys=ON;
         CREATE TABLE tasks (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path TEXT NOT NULL,
             status TEXT NOT NULL DEFAULT 'backlog',
             worktree TEXT,
             tmux_window TEXT,
             plan TEXT,
             epic_id INTEGER,
             needs_input INTEGER NOT NULL DEFAULT 0,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE repo_paths (
             id INTEGER PRIMARY KEY,
             path TEXT NOT NULL UNIQUE,
             last_used TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE epics (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path TEXT NOT NULL,
             done INTEGER NOT NULL DEFAULT 0,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE settings (
             key TEXT PRIMARY KEY,
             value TEXT NOT NULL
         );
         PRAGMA user_version = 5;",
    )
    .unwrap();

    // Insert a ready task
    conn.execute(
        "INSERT INTO tasks (title, description, repo_path, status) VALUES ('T', 'D', '/r', 'ready')",
        [],
    ).unwrap();

    Database::init_schema(&conn).unwrap();

    let status: String = conn
        .query_row("SELECT status FROM tasks WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(status, "backlog");

    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 23);
}

#[test]
fn save_and_list_repo_paths() {
    let db = in_memory_db();
    assert!(db.list_repo_paths().unwrap().is_empty());
    db.save_repo_path("/home/user/project").unwrap();
    db.save_repo_path("/home/user/other").unwrap();
    let paths = db.list_repo_paths().unwrap();
    assert_eq!(paths.len(), 2);
    assert!(paths.contains(&"/home/user/project".to_string()));
    assert!(paths.contains(&"/home/user/other".to_string()));
}

#[test]
fn save_repo_path_deduplicates() {
    let db = in_memory_db();
    db.save_repo_path("/home/user/project").unwrap();
    db.save_repo_path("/home/user/project").unwrap();
    assert_eq!(db.list_repo_paths().unwrap().len(), 1);
}

#[test]
fn list_repo_paths_empty_by_default() {
    let db = in_memory_db();
    assert!(db.list_repo_paths().unwrap().is_empty());
}

#[test]
fn create_task_returning_returns_full_task() {
    let db = in_memory_db();
    let task = db
        .create_task_returning("Title", "Desc", "/repo", None, TaskStatus::Backlog)
        .unwrap();
    assert_eq!(task.title, "Title");
    assert_eq!(task.description, "Desc");
    assert_eq!(task.repo_path, "/repo");
    assert_eq!(task.status, TaskStatus::Backlog);
    assert!(task.worktree.is_none());
    assert!(task.tmux_window.is_none());
    assert!(task.plan.is_none());
}

#[test]
fn create_task_returning_with_plan() {
    let db = in_memory_db();
    let task = db
        .create_task_returning("T", "D", "/r", Some("plan.md"), TaskStatus::Backlog)
        .unwrap();
    assert_eq!(task.plan.as_deref(), Some("plan.md"));
    assert_eq!(task.status, TaskStatus::Backlog);
}

#[test]
fn patch_task_applies_all_fields() {
    let db = in_memory_db();
    let id = db
        .create_task("title", "desc", "/repo", None, TaskStatus::Backlog)
        .unwrap();
    let patch = TaskPatch::new()
        .status(TaskStatus::Running)
        .plan(Some("plan.md"))
        .title("new title");
    db.patch_task(id, &patch).unwrap();
    let task = db.get_task(id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.plan.as_deref(), Some("plan.md"));
    assert_eq!(task.title, "new title");
    assert_eq!(task.description, "desc"); // unchanged
}

#[test]
fn patch_task_none_fields_unchanged() {
    let db = in_memory_db();
    let id = db
        .create_task(
            "title",
            "desc",
            "/repo",
            Some("plan.md"),
            TaskStatus::Running,
        )
        .unwrap();
    let patch = TaskPatch::new();
    db.patch_task(id, &patch).unwrap();
    let task = db.get_task(id).unwrap().unwrap();
    assert_eq!(task.title, "title");
    assert_eq!(task.plan.as_deref(), Some("plan.md"));
    assert_eq!(task.status, TaskStatus::Running);
}

#[test]
fn patch_task_sets_tag() {
    let db = in_memory_db();
    let id = db
        .create_task("title", "desc", "/repo", None, TaskStatus::Backlog)
        .unwrap();
    db.patch_task(id, &TaskPatch::new().tag(Some(TaskTag::Bug)))
        .unwrap();
    let task = db.get_task(id).unwrap().unwrap();
    assert_eq!(task.tag, Some(TaskTag::Bug));
}

#[test]
fn patch_task_clears_tag() {
    let db = in_memory_db();
    let id = db
        .create_task("title", "desc", "/repo", None, TaskStatus::Backlog)
        .unwrap();
    db.patch_task(id, &TaskPatch::new().tag(Some(TaskTag::Feature)))
        .unwrap();
    db.patch_task(id, &TaskPatch::new().tag(None)).unwrap();
    let task = db.get_task(id).unwrap().unwrap();
    assert!(task.tag.is_none());
}

#[test]
fn has_other_tasks_with_worktree_returns_false_when_no_others() {
    let db = in_memory_db();
    let id = db
        .create_task("Task A", "desc", "/repo", None, TaskStatus::Backlog)
        .unwrap();
    db.patch_task(
        id,
        &TaskPatch::new()
            .status(TaskStatus::Running)
            .worktree(Some("/repo/.worktrees/1-task-a"))
            .tmux_window(Some("task-1")),
    )
    .unwrap();

    assert!(!db
        .has_other_tasks_with_worktree("/repo/.worktrees/1-task-a", id)
        .unwrap());
}

#[test]
fn has_other_tasks_with_worktree_returns_true_when_shared() {
    let db = in_memory_db();
    let id1 = db
        .create_task("Task A", "desc", "/repo", None, TaskStatus::Backlog)
        .unwrap();
    let id2 = db
        .create_task("Task B", "desc", "/repo", None, TaskStatus::Backlog)
        .unwrap();
    db.patch_task(
        id1,
        &TaskPatch::new()
            .status(TaskStatus::Running)
            .worktree(Some("/repo/.worktrees/1-task-a"))
            .tmux_window(Some("task-1")),
    )
    .unwrap();
    db.patch_task(
        id2,
        &TaskPatch::new()
            .status(TaskStatus::Running)
            .worktree(Some("/repo/.worktrees/1-task-a"))
            .tmux_window(Some("task-1")),
    )
    .unwrap();

    assert!(db
        .has_other_tasks_with_worktree("/repo/.worktrees/1-task-a", id1)
        .unwrap());
    assert!(db
        .has_other_tasks_with_worktree("/repo/.worktrees/1-task-a", id2)
        .unwrap());
}

#[test]
fn has_other_tasks_with_worktree_ignores_done_tasks() {
    let db = in_memory_db();
    let id1 = db
        .create_task("Task A", "desc", "/repo", None, TaskStatus::Backlog)
        .unwrap();
    let id2 = db
        .create_task("Task B", "desc", "/repo", None, TaskStatus::Backlog)
        .unwrap();
    db.patch_task(
        id1,
        &TaskPatch::new()
            .status(TaskStatus::Running)
            .worktree(Some("/repo/.worktrees/1-task-a"))
            .tmux_window(Some("task-1")),
    )
    .unwrap();
    db.patch_task(
        id2,
        &TaskPatch::new()
            .status(TaskStatus::Done)
            .worktree(Some("/repo/.worktrees/1-task-a"))
            .tmux_window(Some("task-1")),
    )
    .unwrap();

    assert!(!db
        .has_other_tasks_with_worktree("/repo/.worktrees/1-task-a", id1)
        .unwrap());
}

#[test]
fn patch_task_clears_plan() {
    let db = in_memory_db();
    let id = db
        .create_task(
            "title",
            "desc",
            "/repo",
            Some("plan.md"),
            TaskStatus::Backlog,
        )
        .unwrap();
    let patch = TaskPatch::new().plan(None);
    db.patch_task(id, &patch).unwrap();
    let task = db.get_task(id).unwrap().unwrap();
    assert!(task.plan.is_none());
}

#[test]
fn patch_task_sets_dispatch_fields() {
    let db = in_memory_db();
    let id = db
        .create_task("title", "desc", "/repo", None, TaskStatus::Backlog)
        .unwrap();
    let patch = TaskPatch::new()
        .worktree(Some("/repo/.worktrees/1-my-task"))
        .tmux_window(Some("session:1-my-task"));
    db.patch_task(id, &patch).unwrap();
    let task = db.get_task(id).unwrap().unwrap();
    assert_eq!(task.worktree.as_deref(), Some("/repo/.worktrees/1-my-task"));
    assert_eq!(task.tmux_window.as_deref(), Some("session:1-my-task"));
}

#[test]
fn patch_task_clears_dispatch_fields() {
    let db = in_memory_db();
    let id = db
        .create_task("title", "desc", "/repo", None, TaskStatus::Running)
        .unwrap();
    // Set dispatch fields first
    let patch = TaskPatch::new()
        .worktree(Some("/repo/.worktrees/1-my-task"))
        .tmux_window(Some("session:1-my-task"));
    db.patch_task(id, &patch).unwrap();
    let task = db.get_task(id).unwrap().unwrap();
    assert!(task.worktree.is_some());
    assert!(task.tmux_window.is_some());

    // Clear them
    let patch = TaskPatch::new().worktree(None).tmux_window(None);
    db.patch_task(id, &patch).unwrap();
    let task = db.get_task(id).unwrap().unwrap();
    assert!(task.worktree.is_none());
    assert!(task.tmux_window.is_none());
}

#[test]
fn patch_task_status_and_dispatch_together() {
    let db = in_memory_db();
    let id = db
        .create_task("title", "desc", "/repo", None, TaskStatus::Backlog)
        .unwrap();
    let patch = TaskPatch::new()
        .status(TaskStatus::Running)
        .worktree(Some("/repo/.worktrees/1-my-task"))
        .tmux_window(Some("session:1-my-task"));
    db.patch_task(id, &patch).unwrap();
    let task = db.get_task(id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.worktree.as_deref(), Some("/repo/.worktrees/1-my-task"));
    assert_eq!(task.tmux_window.as_deref(), Some("session:1-my-task"));
}

#[test]
fn task_patch_status_does_not_set_sub_status() {
    // status() no longer auto-sets sub_status; patch_task handles the default
    let patch = TaskPatch::new().status(TaskStatus::Review);
    assert_eq!(patch.status, Some(TaskStatus::Review));
    assert_eq!(patch.sub_status, None);
}

#[test]
fn task_patch_status_and_sub_status_independent() {
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

#[test]
fn patch_task_status_change_resets_sub_status_in_db() {
    // End-to-end: after a status-only patch, sub_status in DB reflects the new default
    let db = Database::open_in_memory().unwrap();
    let id = db
        .create_task("T", "d", "/r", None, TaskStatus::Running)
        .unwrap();
    db.patch_task(id, &TaskPatch::default().sub_status(SubStatus::Stale))
        .unwrap();

    db.patch_task(id, &TaskPatch::default().status(TaskStatus::Review))
        .unwrap();

    let task = db.get_task(id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Review);
    assert_eq!(task.sub_status, SubStatus::AwaitingReview);
}

#[test]
fn update_status_if_matching() {
    let db = in_memory_db();
    let id = db
        .create_task("Task", "desc", "/repo", None, TaskStatus::Running)
        .unwrap();

    let updated = db
        .update_status_if(id, TaskStatus::Review, TaskStatus::Running)
        .unwrap();
    assert!(updated, "should update when current status matches");

    let task = db.get_task(id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Review);
}

#[test]
fn update_status_if_not_matching() {
    let db = in_memory_db();
    let id = db
        .create_task("Task", "desc", "/repo", None, TaskStatus::Done)
        .unwrap();

    let updated = db
        .update_status_if(id, TaskStatus::Review, TaskStatus::Running)
        .unwrap();
    assert!(
        !updated,
        "should not update when current status doesn't match"
    );

    let task = db.get_task(id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Done, "status should be unchanged");
}

#[test]
fn update_status_if_nonexistent() {
    let db = in_memory_db();
    let updated = db
        .update_status_if(TaskId(9999), TaskStatus::Review, TaskStatus::Running)
        .unwrap();
    assert!(!updated, "should return false for nonexistent task");
}

// --- Epic CRUD ---

#[test]
fn create_and_get_epic() {
    let db = in_memory_db();
    let epic = db
        .create_epic("Auth Rewrite", "Rewrite auth", "/repo")
        .unwrap();
    assert_eq!(epic.title, "Auth Rewrite");
    assert_eq!(epic.description, "Rewrite auth");
    assert_eq!(epic.repo_path, "/repo");
    assert_eq!(epic.status, TaskStatus::Backlog);

    let fetched = db.get_epic(epic.id).unwrap().unwrap();
    assert_eq!(fetched.id, epic.id);
    assert_eq!(fetched.title, "Auth Rewrite");
}

#[test]
fn list_epics() {
    let db = in_memory_db();
    db.create_epic("Epic A", "desc", "/a").unwrap();
    db.create_epic("Epic B", "desc", "/b").unwrap();
    let epics = db.list_epics().unwrap();
    assert_eq!(epics.len(), 2);
}

#[test]
fn get_epic_nonexistent() {
    let db = in_memory_db();
    assert!(db.get_epic(EpicId(999)).unwrap().is_none());
}

#[test]
fn delete_epic_cascades_subtasks() {
    let db = in_memory_db();
    let epic = db.create_epic("Epic", "desc", "/repo").unwrap();
    db.create_task("Sub 1", "desc", "/repo", None, TaskStatus::Backlog)
        .unwrap();
    let sub_id = db
        .create_task("Sub 2", "desc", "/repo", None, TaskStatus::Backlog)
        .unwrap();

    // Link sub 2 to epic
    db.set_task_epic_id(sub_id, Some(epic.id)).unwrap();

    db.delete_epic(epic.id).unwrap();

    // Epic should be gone
    assert!(db.get_epic(epic.id).unwrap().is_none());
    // Sub 2 (linked to epic) should be deleted
    assert!(db.get_task(sub_id).unwrap().is_none());
    // Sub 1 (not linked) should still exist
    assert_eq!(db.list_all().unwrap().len(), 1);
}

#[test]
fn epic_has_status_field() {
    let db = Database::open_in_memory().unwrap();
    let epic = db.create_epic("Test", "Desc", "/repo").unwrap();
    assert_eq!(epic.status, TaskStatus::Backlog);
}

#[test]
fn patch_epic_status() {
    let db = Database::open_in_memory().unwrap();
    let epic = db.create_epic("Test", "Desc", "/repo").unwrap();
    db.patch_epic(epic.id, &EpicPatch::new().status(TaskStatus::Running))
        .unwrap();
    let epic = db.get_epic(epic.id).unwrap().unwrap();
    assert_eq!(epic.status, TaskStatus::Running);
}

#[test]
fn patch_epic_title() {
    let db = in_memory_db();
    let epic = db.create_epic("Old Title", "desc", "/repo").unwrap();

    db.patch_epic(epic.id, &EpicPatch::new().title("New Title"))
        .unwrap();
    let updated = db.get_epic(epic.id).unwrap().unwrap();
    assert_eq!(updated.title, "New Title");
    assert_eq!(updated.description, "desc"); // unchanged
}

#[test]
fn task_epic_id_roundtrip() {
    let db = in_memory_db();
    let epic = db.create_epic("Epic", "desc", "/repo").unwrap();
    let task_id = db
        .create_task("Task", "desc", "/repo", None, TaskStatus::Backlog)
        .unwrap();

    db.set_task_epic_id(task_id, Some(epic.id)).unwrap();
    let task = db.get_task(task_id).unwrap().unwrap();
    assert_eq!(task.epic_id, Some(epic.id));

    db.set_task_epic_id(task_id, None).unwrap();
    let task = db.get_task(task_id).unwrap().unwrap();
    assert!(task.epic_id.is_none());
}

#[test]
fn list_tasks_for_epic() {
    let db = in_memory_db();
    let epic = db.create_epic("Epic", "desc", "/repo").unwrap();
    let id1 = db
        .create_task("Sub A", "desc", "/repo", None, TaskStatus::Backlog)
        .unwrap();
    let _id2 = db
        .create_task("Standalone", "desc", "/repo", None, TaskStatus::Backlog)
        .unwrap();

    db.set_task_epic_id(id1, Some(epic.id)).unwrap();

    let subtasks = db.list_tasks_for_epic(epic.id).unwrap();
    assert_eq!(subtasks.len(), 1);
    assert_eq!(subtasks[0].title, "Sub A");
}

#[test]
fn task_roundtrip_with_pr_fields() {
    let db = in_memory_db();
    let id = db
        .create_task("PR task", "desc", "/repo", None, TaskStatus::Backlog)
        .unwrap();

    db.patch_task(
        id,
        &TaskPatch::new().pr_url(Some("https://github.com/org/repo/pull/42")),
    )
    .unwrap();

    let task = db.get_task(id).unwrap().unwrap();
    assert_eq!(
        task.pr_url.as_deref(),
        Some("https://github.com/org/repo/pull/42")
    );
}

#[test]
fn task_pr_fields_default_to_none() {
    let db = in_memory_db();
    let id = db
        .create_task("No PR", "desc", "/repo", None, TaskStatus::Backlog)
        .unwrap();
    let task = db.get_task(id).unwrap().unwrap();
    assert!(task.pr_url.is_none());
}

#[test]
fn patch_task_sets_pr_url() {
    let db = in_memory_db();
    let id = db
        .create_task("t", "d", "/r", None, TaskStatus::Backlog)
        .unwrap();

    db.patch_task(
        id,
        &TaskPatch::new().pr_url(Some("https://example.com/pull/1")),
    )
    .unwrap();
    let task = db.get_task(id).unwrap().unwrap();
    assert_eq!(task.pr_url.as_deref(), Some("https://example.com/pull/1"));
}

#[test]
fn patch_epic_plan() {
    let db = in_memory_db();
    let epic = db.create_epic("Epic", "desc", "/repo").unwrap();
    assert!(epic.plan.is_none());

    db.patch_epic(epic.id, &EpicPatch::new().plan(Some("docs/plan.md")))
        .unwrap();
    let updated = db.get_epic(epic.id).unwrap().unwrap();
    assert_eq!(updated.plan.as_deref(), Some("docs/plan.md"));
}

#[test]
fn patch_epic_clear_plan() {
    let db = in_memory_db();
    let epic = db.create_epic("Epic", "desc", "/repo").unwrap();

    db.patch_epic(epic.id, &EpicPatch::new().plan(Some("docs/plan.md")))
        .unwrap();
    db.patch_epic(epic.id, &EpicPatch::new().plan(None))
        .unwrap();
    let updated = db.get_epic(epic.id).unwrap().unwrap();
    assert!(updated.plan.is_none());
}

#[test]
fn patch_epic_repo_path() {
    let db = in_memory_db();
    let epic = db.create_epic("Epic", "desc", "/old").unwrap();

    db.patch_epic(epic.id, &EpicPatch::new().repo_path("/new"))
        .unwrap();
    let updated = db.get_epic(epic.id).unwrap().unwrap();
    assert_eq!(updated.repo_path, "/new");
    assert_eq!(updated.title, "Epic"); // unchanged
}

#[test]
fn patch_task_sets_sort_order() {
    let db = Database::open_in_memory().unwrap();
    let id = db
        .create_task("T", "d", "/r", None, TaskStatus::Backlog)
        .unwrap();
    db.patch_task(id, &TaskPatch::new().sort_order(Some(500)))
        .unwrap();
    let task = db.get_task(id).unwrap().unwrap();
    assert_eq!(task.sort_order, Some(500));
}

#[test]
fn patch_task_clears_sort_order() {
    let db = Database::open_in_memory().unwrap();
    let id = db
        .create_task("T", "d", "/r", None, TaskStatus::Backlog)
        .unwrap();
    db.patch_task(id, &TaskPatch::new().sort_order(Some(100)))
        .unwrap();
    db.patch_task(id, &TaskPatch::new().sort_order(None))
        .unwrap();
    let task = db.get_task(id).unwrap().unwrap();
    assert_eq!(task.sort_order, None);
}

#[test]
fn report_usage_first_insert() {
    let db = Database::open_in_memory().unwrap();
    let id = db
        .create_task("T", "D", "/r", None, TaskStatus::Backlog)
        .unwrap();
    db.report_usage(
        id,
        &UsageReport {
            cost_usd: 0.42,
            input_tokens: 10_000,
            output_tokens: 2_000,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        },
    )
    .unwrap();
    let all = db.get_all_usage().unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].task_id, id);
    assert!((all[0].cost_usd - 0.42).abs() < 1e-9);
    assert_eq!(all[0].input_tokens, 10_000);
    assert_eq!(all[0].output_tokens, 2_000);
    assert_eq!(all[0].cache_read_tokens, 0);
    assert_eq!(all[0].cache_write_tokens, 0);
}

#[test]
fn report_usage_accumulates() {
    let db = Database::open_in_memory().unwrap();
    let id = db
        .create_task("T", "D", "/r", None, TaskStatus::Backlog)
        .unwrap();
    db.report_usage(
        id,
        &UsageReport {
            cost_usd: 0.10,
            input_tokens: 1_000,
            output_tokens: 500,
            cache_read_tokens: 100,
            cache_write_tokens: 50,
        },
    )
    .unwrap();
    db.report_usage(
        id,
        &UsageReport {
            cost_usd: 0.05,
            input_tokens: 500,
            output_tokens: 250,
            cache_read_tokens: 50,
            cache_write_tokens: 25,
        },
    )
    .unwrap();
    let all = db.get_all_usage().unwrap();
    assert_eq!(all.len(), 1);
    let u = &all[0];
    assert!((u.cost_usd - 0.15).abs() < 1e-9);
    assert_eq!(u.input_tokens, 1_500);
    assert_eq!(u.output_tokens, 750);
    assert_eq!(u.cache_read_tokens, 150);
    assert_eq!(u.cache_write_tokens, 75);
}

#[test]
fn get_all_usage_empty() {
    let db = Database::open_in_memory().unwrap();
    assert!(db.get_all_usage().unwrap().is_empty());
}

#[test]
fn filter_presets_save_and_list() {
    let db = Database::open_in_memory().unwrap();
    db.save_filter_preset("frontend", "/repo-a\n/repo-b", "include")
        .unwrap();
    db.save_filter_preset("backend", "/repo-c", "exclude")
        .unwrap();

    let presets = db.list_filter_presets().unwrap();
    assert_eq!(presets.len(), 2);
    assert_eq!(presets[0].0, "backend"); // sorted by name
    assert_eq!(presets[0].2, "exclude");
    assert_eq!(presets[1].0, "frontend");
    assert_eq!(presets[1].1, "/repo-a\n/repo-b");
    assert_eq!(presets[1].2, "include");
}

#[test]
fn filter_presets_overwrite_and_delete() {
    let db = Database::open_in_memory().unwrap();
    db.save_filter_preset("frontend", "/repo-a", "include")
        .unwrap();
    db.save_filter_preset("frontend", "/repo-x\n/repo-y", "exclude")
        .unwrap();

    let presets = db.list_filter_presets().unwrap();
    assert_eq!(presets.len(), 1);
    assert_eq!(presets[0].1, "/repo-x\n/repo-y");
    assert_eq!(presets[0].2, "exclude");

    db.delete_filter_preset("frontend").unwrap();
    let presets = db.list_filter_presets().unwrap();
    assert!(presets.is_empty());
}

#[test]
fn save_and_load_review_prs() {
    use crate::models::{CiStatus, ReviewDecision, ReviewPr};
    use chrono::Utc;

    let db = Database::open_in_memory().unwrap();

    // Initially empty
    let prs = db.load_review_prs().unwrap();
    assert!(prs.is_empty());

    // Save some PRs
    let pr1 = ReviewPr {
        number: 42,
        title: "Fix bug".to_string(),
        author: "alice".to_string(),
        repo: "acme/app".to_string(),
        url: "https://github.com/acme/app/pull/42".to_string(),
        is_draft: false,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        additions: 10,
        deletions: 5,
        review_decision: ReviewDecision::ReviewRequired,
        labels: vec!["bug".to_string()],
        body: String::new(),
        head_ref: String::new(),
        ci_status: CiStatus::None,
        reviewers: vec![],
    };
    let pr2 = ReviewPr {
        number: 99,
        title: "Add feature".to_string(),
        author: "bob".to_string(),
        repo: "acme/app".to_string(),
        url: "https://github.com/acme/app/pull/99".to_string(),
        is_draft: true,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        additions: 200,
        deletions: 80,
        review_decision: ReviewDecision::Approved,
        labels: vec![],
        body: String::new(),
        head_ref: String::new(),
        ci_status: CiStatus::None,
        reviewers: vec![],
    };

    db.save_review_prs(&[pr1, pr2]).unwrap();

    let loaded = db.load_review_prs().unwrap();
    assert_eq!(loaded.len(), 2);

    let p1 = loaded.iter().find(|p| p.number == 42).unwrap();
    assert_eq!(p1.title, "Fix bug");
    assert_eq!(p1.author, "alice");
    assert_eq!(p1.repo, "acme/app");
    assert!(!p1.is_draft);
    assert_eq!(p1.additions, 10);
    assert_eq!(p1.review_decision, ReviewDecision::ReviewRequired);
    assert_eq!(p1.labels, vec!["bug".to_string()]);

    let p2 = loaded.iter().find(|p| p.number == 99).unwrap();
    assert_eq!(p2.review_decision, ReviewDecision::Approved);
    assert!(p2.is_draft);
    assert!(p2.labels.is_empty());
}

#[test]
fn save_review_prs_replaces_all() {
    use crate::models::{CiStatus, ReviewDecision, ReviewPr, Reviewer};
    use chrono::Utc;

    let db = Database::open_in_memory().unwrap();

    let pr1 = ReviewPr {
        number: 1,
        title: "Old PR".to_string(),
        author: "alice".to_string(),
        repo: "acme/app".to_string(),
        url: "https://github.com/acme/app/pull/1".to_string(),
        is_draft: false,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        additions: 0,
        deletions: 0,
        review_decision: ReviewDecision::ReviewRequired,
        labels: vec![],
        body: "Initial body".to_string(),
        head_ref: "feature/old-branch".to_string(),
        ci_status: CiStatus::Pending,
        reviewers: vec![Reviewer {
            login: "carol".to_string(),
            decision: None,
        }],
    };
    db.save_review_prs(&[pr1]).unwrap();
    assert_eq!(db.load_review_prs().unwrap().len(), 1);

    // Verify new fields round-trip on the first save
    let loaded_first = db.load_review_prs().unwrap();
    assert_eq!(loaded_first[0].body, "Initial body");
    assert_eq!(loaded_first[0].head_ref, "feature/old-branch");
    assert_eq!(loaded_first[0].ci_status, CiStatus::Pending);
    assert_eq!(loaded_first[0].reviewers.len(), 1);
    assert_eq!(loaded_first[0].reviewers[0].login, "carol");
    assert_eq!(loaded_first[0].reviewers[0].decision, None);

    // Save new set — old ones should be gone
    let pr2 = ReviewPr {
        number: 2,
        title: "New PR".to_string(),
        author: "bob".to_string(),
        repo: "acme/other".to_string(),
        url: "https://github.com/acme/other/pull/2".to_string(),
        is_draft: false,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        additions: 5,
        deletions: 3,
        review_decision: ReviewDecision::ChangesRequested,
        labels: vec!["urgent".to_string()],
        body: "Needs more work".to_string(),
        head_ref: "fix/new-branch".to_string(),
        ci_status: CiStatus::Failure,
        reviewers: vec![Reviewer {
            login: "dave".to_string(),
            decision: Some(ReviewDecision::ChangesRequested),
        }],
    };
    db.save_review_prs(&[pr2]).unwrap();

    let loaded = db.load_review_prs().unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].number, 2);
    assert_eq!(loaded[0].repo, "acme/other");
    assert_eq!(loaded[0].body, "Needs more work");
    assert_eq!(loaded[0].head_ref, "fix/new-branch");
    assert_eq!(loaded[0].ci_status, CiStatus::Failure);
    assert_eq!(loaded[0].reviewers.len(), 1);
    assert_eq!(loaded[0].reviewers[0].login, "dave");
    assert_eq!(
        loaded[0].reviewers[0].decision,
        Some(ReviewDecision::ChangesRequested)
    );
}

#[test]
fn task_sub_status_persists() {
    let db = Database::open_in_memory().unwrap();
    let id = db
        .create_task("Test", "desc", "/repo", None, TaskStatus::Running)
        .unwrap();
    db.patch_task(id, &TaskPatch::default().sub_status(SubStatus::Stale))
        .unwrap();
    let task = db.get_task(id).unwrap().unwrap();
    assert_eq!(task.sub_status, SubStatus::Stale);
}

#[test]
fn task_sub_status_defaults_to_none() {
    let db = Database::open_in_memory().unwrap();
    let id = db
        .create_task("Test", "desc", "/repo", None, TaskStatus::Backlog)
        .unwrap();
    let task = db.get_task(id).unwrap().unwrap();
    assert_eq!(task.sub_status, SubStatus::None);
}

#[test]
fn migration_13_converts_needs_input() {
    // Simulate a database at version 12 with needs_input column
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys=ON;
         CREATE TABLE tasks (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path TEXT NOT NULL,
             status TEXT NOT NULL DEFAULT 'backlog',
             worktree TEXT,
             tmux_window TEXT,
             plan TEXT,
             epic_id INTEGER,
             needs_input INTEGER NOT NULL DEFAULT 0,
             pr_url TEXT,
             sort_order INTEGER,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE repo_paths (
             id INTEGER PRIMARY KEY,
             path TEXT NOT NULL UNIQUE,
             last_used TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE epics (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path TEXT NOT NULL,
             done INTEGER NOT NULL DEFAULT 0,
             plan TEXT,
             sort_order INTEGER,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE settings (
             key TEXT PRIMARY KEY,
             value TEXT NOT NULL
         );
         CREATE TABLE task_usage (
             task_id            INTEGER PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
             cost_usd           REAL    NOT NULL DEFAULT 0.0,
             input_tokens       INTEGER NOT NULL DEFAULT 0,
             output_tokens      INTEGER NOT NULL DEFAULT 0,
             cache_read_tokens  INTEGER NOT NULL DEFAULT 0,
             cache_write_tokens INTEGER NOT NULL DEFAULT 0,
             updated_at         TEXT    NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE filter_presets (
             name       TEXT PRIMARY KEY,
             repo_paths TEXT NOT NULL
         );
         PRAGMA user_version = 12;",
    )
    .unwrap();

    // Insert tasks with various states
    conn.execute(
        "INSERT INTO tasks (title, description, repo_path, status, needs_input) VALUES ('Blocked', 'desc', '/r', 'running', 1)",
        [],
    ).unwrap();
    conn.execute(
        "INSERT INTO tasks (title, description, repo_path, status, needs_input) VALUES ('Active', 'desc', '/r', 'running', 0)",
        [],
    ).unwrap();
    conn.execute(
        "INSERT INTO tasks (title, description, repo_path, status, needs_input) VALUES ('InReview', 'desc', '/r', 'review', 0)",
        [],
    ).unwrap();

    // Run migration
    Database::init_schema(&conn).unwrap();

    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 23);

    // Verify needs_input=1 became sub_status='needs_input'
    let ss: String = conn
        .query_row("SELECT sub_status FROM tasks WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(ss, "needs_input");

    // Verify running task with needs_input=0 became 'active'
    let ss: String = conn
        .query_row("SELECT sub_status FROM tasks WHERE id = 2", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(ss, "active");

    // Verify review task became 'awaiting_review'
    let ss: String = conn
        .query_row("SELECT sub_status FROM tasks WHERE id = 3", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(ss, "awaiting_review");

    // Verify needs_input column no longer exists
    let has_needs_input = conn
        .prepare("SELECT needs_input FROM tasks LIMIT 1")
        .is_ok();
    assert!(
        !has_needs_input,
        "needs_input column should be removed after migration"
    );
}

#[test]
fn create_task_sets_default_sub_status_for_running() {
    // create_task with status=Running must produce sub_status=active, not 'none'
    let db = in_memory_db();
    let id = db
        .create_task("T", "d", "/r", None, TaskStatus::Running)
        .unwrap();
    let task = db.get_task(id).unwrap().unwrap();
    assert_eq!(task.sub_status, SubStatus::Active);
}

#[test]
fn create_task_sets_default_sub_status_for_backlog() {
    let db = in_memory_db();
    let id = db
        .create_task("T", "d", "/r", None, TaskStatus::Backlog)
        .unwrap();
    let task = db.get_task(id).unwrap().unwrap();
    assert_eq!(task.sub_status, SubStatus::None);
}

#[test]
fn update_status_if_resets_sub_status_to_default() {
    let db = in_memory_db();
    let id = db
        .create_task("T", "d", "/r", None, TaskStatus::Running)
        .unwrap();
    db.patch_task(id, &TaskPatch::default().sub_status(SubStatus::Stale))
        .unwrap();

    let updated = db
        .update_status_if(id, TaskStatus::Review, TaskStatus::Running)
        .unwrap();
    assert!(updated, "should have updated");

    let task = db.get_task(id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Review);
    assert_eq!(task.sub_status, SubStatus::AwaitingReview); // default for review
}

#[test]
fn update_status_if_leaves_sub_status_unchanged_when_condition_fails() {
    let db = in_memory_db();
    let id = db
        .create_task("T", "d", "/r", None, TaskStatus::Running)
        .unwrap();
    db.patch_task(id, &TaskPatch::default().sub_status(SubStatus::Active))
        .unwrap();

    let updated = db
        .update_status_if(id, TaskStatus::Review, TaskStatus::Backlog)
        .unwrap();
    assert!(!updated, "condition was wrong, should not have updated");

    let task = db.get_task(id).unwrap().unwrap();
    assert_eq!(task.sub_status, SubStatus::Active); // unchanged
}

#[test]
fn schema_version_is_21() {
    let db = Database::open_in_memory().unwrap();
    let conn = db.conn.lock().unwrap();
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 23, "fresh DB should be at schema version 23");
}

#[test]
fn check_constraint_rejects_review_with_active_substatus() {
    let db = Database::open_in_memory().unwrap();
    let conn = db.conn.lock().unwrap();
    conn.execute(
        "INSERT INTO tasks (title, description, repo_path, status, sub_status) \
         VALUES ('T', 'D', '/r', 'backlog', 'none')",
        [],
    )
    .unwrap();
    let result = conn.execute(
        "UPDATE tasks SET status = 'review', sub_status = 'active' WHERE id = 1",
        [],
    );
    assert!(
        result.is_err(),
        "CHECK constraint must reject (review, active)"
    );
}

#[test]
fn check_constraint_accepts_review_with_awaiting_review() {
    let db = Database::open_in_memory().unwrap();
    let conn = db.conn.lock().unwrap();
    conn.execute(
        "INSERT INTO tasks (title, description, repo_path, status, sub_status) \
         VALUES ('T', 'D', '/r', 'backlog', 'none')",
        [],
    )
    .unwrap();
    let result = conn.execute(
        "UPDATE tasks SET status = 'review', sub_status = 'awaiting_review' WHERE id = 1",
        [],
    );
    assert!(result.is_ok(), "valid pair should be accepted");
}

#[test]
fn migration_16_cleans_invalid_review_needs_input() {
    // Simulate a v15 DB that has (review, needs_input) rows from old hook behavior
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys=ON;
         CREATE TABLE tasks (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path TEXT NOT NULL,
             status TEXT NOT NULL DEFAULT 'backlog',
             worktree TEXT,
             tmux_window TEXT,
             plan TEXT,
             epic_id INTEGER,
             sub_status TEXT NOT NULL DEFAULT 'none',
             pr_url TEXT,
             tag TEXT,
             sort_order INTEGER,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE repo_paths (
             id INTEGER PRIMARY KEY,
             path TEXT NOT NULL UNIQUE,
             last_used TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE epics (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path TEXT NOT NULL,
             done INTEGER NOT NULL DEFAULT 0,
             plan TEXT,
             sort_order INTEGER,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE task_usage (
             task_id INTEGER PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
             cost_usd REAL NOT NULL DEFAULT 0.0,
             input_tokens INTEGER NOT NULL DEFAULT 0,
             output_tokens INTEGER NOT NULL DEFAULT 0,
             cache_read_tokens INTEGER NOT NULL DEFAULT 0,
             cache_write_tokens INTEGER NOT NULL DEFAULT 0,
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE filter_presets (name TEXT PRIMARY KEY, repo_paths TEXT NOT NULL);
         CREATE TABLE review_prs (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             number INTEGER NOT NULL,
             title TEXT NOT NULL,
             url TEXT NOT NULL,
             repo TEXT NOT NULL,
             author TEXT NOT NULL,
             state TEXT NOT NULL DEFAULT 'open',
             review_decision TEXT,
             created_at TEXT NOT NULL,
             updated_at TEXT NOT NULL
         );
         PRAGMA user_version = 15;",
    )
    .unwrap();

    // Insert invalid rows that migration 16 must clean up
    conn.execute(
        "INSERT INTO tasks (title, description, repo_path, status, sub_status) \
         VALUES ('ReviewBlocked', 'desc', '/r', 'review', 'needs_input')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO tasks (title, description, repo_path, status, sub_status) \
         VALUES ('ValidReview', 'desc', '/r', 'review', 'awaiting_review')",
        [],
    )
    .unwrap();

    // Run migrations
    Database::init_schema(&conn).unwrap();

    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 23);

    // (review, needs_input) must be converted to (review, awaiting_review)
    let ss: String = conn
        .query_row(
            "SELECT sub_status FROM tasks WHERE title = 'ReviewBlocked'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        ss, "awaiting_review",
        "legacy (review, needs_input) must be cleaned up"
    );

    // Valid row must be unchanged
    let ss2: String = conn
        .query_row(
            "SELECT sub_status FROM tasks WHERE title = 'ValidReview'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(ss2, "awaiting_review");
}

#[test]
fn recalculate_epic_status_advances_to_running() {
    let db = in_memory_db();
    let epic = db.create_epic("E", "", "/repo").unwrap();
    assert_eq!(epic.status, TaskStatus::Backlog);

    let task = db
        .create_task_returning("T1", "", "/repo", None, TaskStatus::Backlog)
        .unwrap();
    db.set_task_epic_id(task.id, Some(epic.id)).unwrap();
    db.patch_task(task.id, &TaskPatch::new().status(TaskStatus::Running))
        .unwrap();

    db.recalculate_epic_status(epic.id).unwrap();
    let epic = db.get_epic(epic.id).unwrap().unwrap();
    assert_eq!(epic.status, TaskStatus::Running);
}

#[test]
fn recalculate_epic_status_does_not_move_backward() {
    let db = in_memory_db();
    let epic = db.create_epic("E", "", "/repo").unwrap();
    db.patch_epic(epic.id, &EpicPatch::new().status(TaskStatus::Review))
        .unwrap();

    let task = db
        .create_task_returning("T1", "", "/repo", None, TaskStatus::Backlog)
        .unwrap();
    db.set_task_epic_id(task.id, Some(epic.id)).unwrap();
    db.patch_task(task.id, &TaskPatch::new().status(TaskStatus::Running))
        .unwrap();

    db.recalculate_epic_status(epic.id).unwrap();
    let epic = db.get_epic(epic.id).unwrap().unwrap();
    assert_eq!(epic.status, TaskStatus::Review);
}

#[test]
fn recalculate_epic_status_all_done() {
    let db = in_memory_db();
    let epic = db.create_epic("E", "", "/repo").unwrap();

    let t1 = db
        .create_task_returning("T1", "", "/repo", None, TaskStatus::Backlog)
        .unwrap();
    let t2 = db
        .create_task_returning("T2", "", "/repo", None, TaskStatus::Backlog)
        .unwrap();
    db.set_task_epic_id(t1.id, Some(epic.id)).unwrap();
    db.set_task_epic_id(t2.id, Some(epic.id)).unwrap();
    db.patch_task(t1.id, &TaskPatch::new().status(TaskStatus::Done))
        .unwrap();
    db.patch_task(t2.id, &TaskPatch::new().status(TaskStatus::Done))
        .unwrap();

    db.recalculate_epic_status(epic.id).unwrap();
    let epic = db.get_epic(epic.id).unwrap().unwrap();
    assert_eq!(epic.status, TaskStatus::Done);
}

#[test]
fn recalculate_epic_status_all_review_or_done() {
    let db = in_memory_db();
    let epic = db.create_epic("E", "", "/repo").unwrap();

    let t1 = db
        .create_task_returning("T1", "", "/repo", None, TaskStatus::Backlog)
        .unwrap();
    let t2 = db
        .create_task_returning("T2", "", "/repo", None, TaskStatus::Backlog)
        .unwrap();
    db.set_task_epic_id(t1.id, Some(epic.id)).unwrap();
    db.set_task_epic_id(t2.id, Some(epic.id)).unwrap();
    db.patch_task(t1.id, &TaskPatch::new().status(TaskStatus::Review))
        .unwrap();
    db.patch_task(t2.id, &TaskPatch::new().status(TaskStatus::Done))
        .unwrap();

    db.recalculate_epic_status(epic.id).unwrap();
    let epic = db.get_epic(epic.id).unwrap().unwrap();
    assert_eq!(epic.status, TaskStatus::Review);
}

#[test]
fn recalculate_epic_status_review_beats_running() {
    let db = in_memory_db();
    let epic = db.create_epic("E", "", "/repo").unwrap();

    let t1 = db
        .create_task_returning("T1", "", "/repo", None, TaskStatus::Backlog)
        .unwrap();
    let t2 = db
        .create_task_returning("T2", "", "/repo", None, TaskStatus::Backlog)
        .unwrap();
    let t3 = db
        .create_task_returning("T3", "", "/repo", None, TaskStatus::Backlog)
        .unwrap();
    db.set_task_epic_id(t1.id, Some(epic.id)).unwrap();
    db.set_task_epic_id(t2.id, Some(epic.id)).unwrap();
    db.set_task_epic_id(t3.id, Some(epic.id)).unwrap();
    db.patch_task(t1.id, &TaskPatch::new().status(TaskStatus::Review))
        .unwrap();
    db.patch_task(t2.id, &TaskPatch::new().status(TaskStatus::Review))
        .unwrap();
    db.patch_task(t3.id, &TaskPatch::new().status(TaskStatus::Running))
        .unwrap();

    db.recalculate_epic_status(epic.id).unwrap();
    let epic = db.get_epic(epic.id).unwrap().unwrap();
    assert_eq!(epic.status, TaskStatus::Review);
}
