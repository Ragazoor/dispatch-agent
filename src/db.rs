use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::sync::Mutex;

use crate::models::{Task, TaskId, TaskStatus};

// ---------------------------------------------------------------------------
// TaskPatch — builder for selective field updates
// ---------------------------------------------------------------------------

/// Builder for partial task updates. Each field is `None` by default (= don't
/// change). For nullable columns (`plan`, `worktree`, `tmux_window`) we use
/// a double-Option: `None` = don't change, `Some(None)` = set NULL,
/// `Some(Some(x))` = set value.
#[derive(Debug, Default)]
pub struct TaskPatch<'a> {
    pub status: Option<TaskStatus>,
    pub plan: Option<Option<&'a str>>,
    pub title: Option<&'a str>,
    pub description: Option<&'a str>,
    pub repo_path: Option<&'a str>,
    pub worktree: Option<Option<&'a str>>,
    pub tmux_window: Option<Option<&'a str>>,
}

impl<'a> TaskPatch<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn status(mut self, status: TaskStatus) -> Self {
        self.status = Some(status);
        self
    }

    pub fn plan(mut self, plan: Option<&'a str>) -> Self {
        self.plan = Some(plan);
        self
    }

    pub fn title(mut self, title: &'a str) -> Self {
        self.title = Some(title);
        self
    }

    pub fn description(mut self, description: &'a str) -> Self {
        self.description = Some(description);
        self
    }

    pub fn repo_path(mut self, repo_path: &'a str) -> Self {
        self.repo_path = Some(repo_path);
        self
    }

    pub fn worktree(mut self, worktree: Option<&'a str>) -> Self {
        self.worktree = Some(worktree);
        self
    }

    pub fn tmux_window(mut self, tmux_window: Option<&'a str>) -> Self {
        self.tmux_window = Some(tmux_window);
        self
    }

    pub fn has_changes(&self) -> bool {
        self.status.is_some()
            || self.plan.is_some()
            || self.title.is_some()
            || self.description.is_some()
            || self.repo_path.is_some()
            || self.worktree.is_some()
            || self.tmux_window.is_some()
    }
}

// ---------------------------------------------------------------------------
// TaskStore trait
// ---------------------------------------------------------------------------

pub trait TaskStore: Send + Sync {
    fn create_task(&self, title: &str, description: &str, repo_path: &str, plan: Option<&str>, status: TaskStatus) -> Result<TaskId>;
    fn get_task(&self, id: TaskId) -> Result<Option<Task>>;
    fn list_all(&self) -> Result<Vec<Task>>;
    fn list_by_status(&self, status: TaskStatus) -> Result<Vec<Task>>;
    /// Update status only if current status matches `expected`. Returns true if updated.
    fn update_status_if(&self, id: TaskId, new_status: TaskStatus, expected: TaskStatus) -> Result<bool>;
    fn delete_task(&self, id: TaskId) -> Result<()>;
    fn list_repo_paths(&self) -> Result<Vec<String>>;
    fn save_repo_path(&self, path: &str) -> Result<()>;
    fn find_task_by_plan(&self, plan: &str) -> Result<Option<Task>>;
    fn patch_task(&self, id: TaskId, patch: &TaskPatch<'_>) -> Result<()>;
    fn create_task_returning(
        &self,
        title: &str,
        description: &str,
        repo_path: &str,
        plan: Option<&str>,
        status: TaskStatus,
    ) -> Result<Task>;
    fn has_other_tasks_with_worktree(&self, worktree: &str, exclude_id: TaskId) -> Result<bool>;
}

// ---------------------------------------------------------------------------
// Database
// ---------------------------------------------------------------------------

pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        // Ensure the parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create db directory: {}", parent.display()))?;
        }

        let conn = Connection::open(path)
            .with_context(|| format!("Failed to open database at {}", path.display()))?;

        Self::init_schema(&conn)?;

        Ok(Database {
            conn: Mutex::new(conn),
        })
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("Failed to open in-memory database")?;
        Self::init_schema(&conn)?;
        Ok(Database {
            conn: Mutex::new(conn),
        })
    }

    fn init_schema(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA foreign_keys=ON;
             PRAGMA busy_timeout=5000;",
        )
        .context("Failed to set PRAGMAs")?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS tasks (
                id          INTEGER PRIMARY KEY,
                title       TEXT NOT NULL,
                description TEXT NOT NULL,
                repo_path   TEXT NOT NULL,
                status      TEXT NOT NULL DEFAULT 'backlog',
                worktree    TEXT,
                tmux_window TEXT,
                plan        TEXT,
                created_at  TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS repo_paths (
                id        INTEGER PRIMARY KEY,
                path      TEXT NOT NULL UNIQUE,
                last_used TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .context("Failed to create schema")?;

        // Versioned migrations using PRAGMA user_version
        let current_version: i64 =
            conn.pragma_query_value(None, "user_version", |row| row.get(0))?;

        if current_version < 1 {
            // Migration 1: add plan column (idempotent — ignore error if already exists)
            let _ = conn.execute_batch("ALTER TABLE tasks ADD COLUMN plan TEXT");
            conn.pragma_update(None, "user_version", 1i64)
                .context("Failed to update schema version to 1")?;
        }

        if current_version < 2 {
            // Migration 2: drop notes table
            conn.execute_batch("DROP TABLE IF EXISTS notes")
                .context("Failed to drop notes table")?;
            conn.pragma_update(None, "user_version", 2i64)
                .context("Failed to update schema version to 2")?;
        }

        Ok(())
    }

    fn conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| anyhow::anyhow!("db lock poisoned"))
    }
}

impl TaskStore for Database {
    fn create_task(
        &self,
        title: &str,
        description: &str,
        repo_path: &str,
        plan: Option<&str>,
        status: TaskStatus,
    ) -> Result<TaskId> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO tasks (title, description, repo_path, plan, status) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![title, description, repo_path, plan, status.as_str()],
        )
        .context("Failed to insert task")?;
        Ok(TaskId(conn.last_insert_rowid()))
    }

    fn get_task(&self, id: TaskId) -> Result<Option<Task>> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, title, description, repo_path, status, worktree, tmux_window,
                    plan, created_at, updated_at
             FROM tasks WHERE id = ?1",
            params![id.0],
            row_to_task,
        )
        .optional()
        .context("Failed to get task")
    }

    fn list_all(&self) -> Result<Vec<Task>> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, title, description, repo_path, status, worktree, tmux_window,
                        plan, created_at, updated_at
                 FROM tasks ORDER BY id",
            )
            .context("Failed to prepare list_all")?;
        let tasks = stmt
            .query_map([], row_to_task)
            .context("Failed to query tasks")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to collect tasks")?;
        Ok(tasks)
    }

    fn list_by_status(&self, status: TaskStatus) -> Result<Vec<Task>> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, title, description, repo_path, status, worktree, tmux_window,
                        plan, created_at, updated_at
                 FROM tasks WHERE status = ?1 ORDER BY id",
            )
            .context("Failed to prepare list_by_status")?;
        let tasks = stmt
            .query_map(params![status.as_str()], row_to_task)
            .context("Failed to query tasks by status")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to collect tasks by status")?;
        Ok(tasks)
    }

    fn update_status_if(&self, id: TaskId, new_status: TaskStatus, expected: TaskStatus) -> Result<bool> {
        let conn = self.conn()?;
        let rows = conn
            .execute(
                "UPDATE tasks SET status = ?1, updated_at = datetime('now') WHERE id = ?2 AND status = ?3",
                params![new_status.as_str(), id.0, expected.as_str()],
            )
            .context("Failed to conditional-update status")?;
        Ok(rows > 0)
    }

    fn delete_task(&self, id: TaskId) -> Result<()> {
        let conn = self.conn()?;
        let rows = conn
            .execute("DELETE FROM tasks WHERE id = ?1", params![id.0])
            .context("Failed to delete task")?;
        if rows == 0 {
            anyhow::bail!("Task {} not found", id);
        }
        Ok(())
    }

    fn list_repo_paths(&self) -> Result<Vec<String>> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT path FROM repo_paths ORDER BY last_used DESC LIMIT 9")
            .context("Failed to prepare list_repo_paths")?;
        let paths = stmt
            .query_map([], |row| row.get(0))
            .context("Failed to query repo_paths")?
            .collect::<rusqlite::Result<Vec<String>>>()
            .context("Failed to collect repo_paths")?;
        Ok(paths)
    }

    fn save_repo_path(&self, path: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO repo_paths (path) VALUES (?1)
             ON CONFLICT(path) DO UPDATE SET last_used = datetime('now')",
            params![path],
        )
        .context("Failed to save repo_path")?;
        Ok(())
    }

    fn find_task_by_plan(&self, plan: &str) -> Result<Option<Task>> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, title, description, repo_path, status, worktree, tmux_window,
                    plan, created_at, updated_at
             FROM tasks WHERE plan = ?1",
            params![plan],
            row_to_task,
        )
        .optional()
        .context("Failed to find task by plan")
    }

    fn create_task_returning(
        &self,
        title: &str,
        description: &str,
        repo_path: &str,
        plan: Option<&str>,
        status: TaskStatus,
    ) -> Result<Task> {
        let id = self.create_task(title, description, repo_path, plan, status)?;
        self.get_task(id)?
            .ok_or_else(|| anyhow::anyhow!("Task {id} vanished after insert"))
    }

    fn patch_task(&self, id: TaskId, patch: &TaskPatch<'_>) -> Result<()> {
        let conn = self.conn()?;

        // Fetch current values for all patchable fields
        let existing = conn.query_row(
            "SELECT title, description, repo_path, status, plan, worktree, tmux_window
             FROM tasks WHERE id = ?1",
            params![id.0],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            },
        ).optional().context("Failed to fetch task for patch")?;

        let (cur_title, cur_desc, cur_repo, cur_status, cur_plan, cur_worktree, cur_tmux) =
            match existing {
                Some(row) => row,
                None => anyhow::bail!("Task {id} not found"),
            };

        let final_status = patch.status.map(|s| s.as_str().to_string()).unwrap_or(cur_status);
        let final_title = patch.title.unwrap_or(&cur_title);
        let final_desc = patch.description.unwrap_or(&cur_desc);
        let final_repo = patch.repo_path.unwrap_or(&cur_repo);
        let final_plan: Option<&str> = match patch.plan {
            Some(p) => p,
            None => cur_plan.as_deref(),
        };
        let final_worktree: Option<&str> = match patch.worktree {
            Some(w) => w,
            None => cur_worktree.as_deref(),
        };
        let final_tmux: Option<&str> = match patch.tmux_window {
            Some(t) => t,
            None => cur_tmux.as_deref(),
        };

        conn.execute(
            "UPDATE tasks SET title = ?1, description = ?2, repo_path = ?3, status = ?4,
                    plan = ?5, worktree = ?6, tmux_window = ?7, updated_at = datetime('now')
             WHERE id = ?8",
            params![final_title, final_desc, final_repo, final_status, final_plan, final_worktree, final_tmux, id.0],
        ).context("Failed to patch task")?;

        Ok(())
    }

    fn has_other_tasks_with_worktree(&self, worktree: &str, exclude_id: TaskId) -> Result<bool> {
        let conn = self.conn.lock().map_err(|_| anyhow::anyhow!("db lock poisoned"))?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM tasks WHERE worktree = ?1 AND id != ?2 AND status != 'done'",
                params![worktree, exclude_id.0],
                |row| row.get(0),
            )
            .context("Failed to check shared worktree")?;
        Ok(count > 0)
    }
}

// ---------------------------------------------------------------------------
// Row helpers
// ---------------------------------------------------------------------------

fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    let status_str: String = row.get("status")?;
    let status = TaskStatus::parse(&status_str).unwrap_or_else(|| {
        tracing::warn!(raw = %status_str, "unrecognised task status, defaulting to Backlog");
        TaskStatus::Backlog
    });

    let created_str: String = row.get("created_at")?;
    let updated_str: String = row.get("updated_at")?;

    Ok(Task {
        id: TaskId(row.get("id")?),
        title: row.get("title")?,
        description: row.get("description")?,
        repo_path: row.get("repo_path")?,
        status,
        worktree: row.get("worktree")?,
        tmux_window: row.get("tmux_window")?,
        plan: row.get("plan")?,
        created_at: parse_datetime(&created_str),
        updated_at: parse_datetime(&updated_str),
    })
}

/// Parse SQLite `datetime('now')` output: "YYYY-MM-DD HH:MM:SS"
fn parse_datetime(s: &str) -> DateTime<Utc> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|ndt| Utc.from_utc_datetime(&ndt))
        .unwrap_or_else(Utc::now)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn in_memory_db() -> Database {
        Database::open_in_memory().unwrap()
    }

    #[test]
    fn create_and_get() {
        let db = in_memory_db();
        let id = db.create_task("My Task", "A description", "/repo/path", None, TaskStatus::Backlog).unwrap();
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
        db.create_task("Task A", "desc", "/a", None, TaskStatus::Backlog).unwrap();
        db.create_task("Task B", "desc", "/b", None, TaskStatus::Backlog).unwrap();
        db.create_task("Task C", "desc", "/c", None, TaskStatus::Backlog).unwrap();
        let tasks = db.list_all().unwrap();
        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks[0].title, "Task A");
        assert_eq!(tasks[1].title, "Task B");
        assert_eq!(tasks[2].title, "Task C");
    }

    #[test]
    fn list_by_status() {
        let db = in_memory_db();
        let id1 = db.create_task("Task A", "desc", "/a", None, TaskStatus::Backlog).unwrap();
        let id2 = db.create_task("Task B", "desc", "/b", None, TaskStatus::Backlog).unwrap();
        db.create_task("Task C", "desc", "/c", None, TaskStatus::Backlog).unwrap();

        db.patch_task(id1, &TaskPatch::new().status(TaskStatus::Ready)).unwrap();
        db.patch_task(id2, &TaskPatch::new().status(TaskStatus::Ready)).unwrap();

        let ready = db.list_by_status(TaskStatus::Ready).unwrap();
        assert_eq!(ready.len(), 2);

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
        let id = db.create_task("Planned Task", "desc", "/repo", Some("docs/plan.md"), TaskStatus::Backlog).unwrap();
        let task = db.get_task(id).unwrap().unwrap();
        assert_eq!(task.plan.as_deref(), Some("docs/plan.md"));
    }

    #[test]
    fn create_task_without_plan() {
        let db = in_memory_db();
        let id = db.create_task("Simple Task", "desc", "/repo", None, TaskStatus::Backlog).unwrap();
        let task = db.get_task(id).unwrap().unwrap();
        assert!(task.plan.is_none());
    }

    #[test]
    fn find_task_by_plan_returns_match() {
        let db = in_memory_db();
        let id = db.create_task("Planned", "desc", "/repo", Some("/plans/my-plan.md"), TaskStatus::Ready).unwrap();

        let found = db.find_task_by_plan("/plans/my-plan.md").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, id);
    }

    #[test]
    fn find_task_by_plan_returns_none_when_no_match() {
        let db = in_memory_db();
        db.create_task("Other", "desc", "/repo", Some("/plans/other.md"), TaskStatus::Ready).unwrap();

        let found = db.find_task_by_plan("/plans/nonexistent.md").unwrap();
        assert!(found.is_none());
    }

    #[test]
    fn find_task_by_plan_ignores_tasks_without_plan() {
        let db = in_memory_db();
        db.create_task("No Plan", "desc", "/repo", None, TaskStatus::Backlog).unwrap();

        let found = db.find_task_by_plan("/plans/any.md").unwrap();
        assert!(found.is_none());
    }

    #[test]
    fn fresh_db_has_latest_schema_version() {
        let db = in_memory_db();
        let conn = db.conn.lock().unwrap();
        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0)).unwrap();
        assert_eq!(version, 2, "fresh DB should be at schema version 2");
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
        ).unwrap();

        // Insert a note so we can verify the table gets dropped
        conn.execute("INSERT INTO tasks (title, description, repo_path) VALUES ('T', 'D', '/r')", []).unwrap();
        conn.execute("INSERT INTO notes (task_id, content) VALUES (1, 'hello')", []).unwrap();

        // Run init_schema which should migrate
        Database::init_schema(&conn).unwrap();

        // Notes table should be gone
        let table_exists: bool = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name='notes'")
            .unwrap()
            .exists([])
            .unwrap();
        assert!(!table_exists, "notes table should be dropped after migration");

        // Version should be latest
        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0)).unwrap();
        assert_eq!(version, 2);

        // Verify Migration 1 added the plan column
        let has_plan: bool = conn
            .prepare("SELECT plan FROM tasks LIMIT 1")
            .is_ok();
        assert!(has_plan, "Migration 1 should have added the plan column");
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
        let task = db.create_task_returning("Title", "Desc", "/repo", None, TaskStatus::Backlog).unwrap();
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
        let task = db.create_task_returning("T", "D", "/r", Some("plan.md"), TaskStatus::Ready).unwrap();
        assert_eq!(task.plan.as_deref(), Some("plan.md"));
        assert_eq!(task.status, TaskStatus::Ready);
    }

    #[test]
    fn patch_task_applies_all_fields() {
        let db = in_memory_db();
        let id = db
            .create_task("title", "desc", "/repo", None, TaskStatus::Backlog)
            .unwrap();
        let patch = TaskPatch::new()
            .status(TaskStatus::Ready)
            .plan(Some("plan.md"))
            .title("new title");
        db.patch_task(id, &patch).unwrap();
        let task = db.get_task(id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Ready);
        assert_eq!(task.plan.as_deref(), Some("plan.md"));
        assert_eq!(task.title, "new title");
        assert_eq!(task.description, "desc"); // unchanged
    }

    #[test]
    fn patch_task_none_fields_unchanged() {
        let db = in_memory_db();
        let id = db
            .create_task("title", "desc", "/repo", Some("plan.md"), TaskStatus::Ready)
            .unwrap();
        let patch = TaskPatch::new();
        db.patch_task(id, &patch).unwrap();
        let task = db.get_task(id).unwrap().unwrap();
        assert_eq!(task.title, "title");
        assert_eq!(task.plan.as_deref(), Some("plan.md"));
        assert_eq!(task.status, TaskStatus::Ready);
    }

    #[test]
    fn has_other_tasks_with_worktree_returns_false_when_no_others() {
        let db = in_memory_db();
        let id = db.create_task("Task A", "desc", "/repo", None, TaskStatus::Backlog).unwrap();
        db.patch_task(id, &TaskPatch::new()
            .status(TaskStatus::Running)
            .worktree(Some("/repo/.worktrees/1-task-a"))
            .tmux_window(Some("task-1"))).unwrap();

        assert!(!db.has_other_tasks_with_worktree("/repo/.worktrees/1-task-a", id).unwrap());
    }

    #[test]
    fn has_other_tasks_with_worktree_returns_true_when_shared() {
        let db = in_memory_db();
        let id1 = db.create_task("Task A", "desc", "/repo", None, TaskStatus::Backlog).unwrap();
        let id2 = db.create_task("Task B", "desc", "/repo", None, TaskStatus::Backlog).unwrap();
        db.patch_task(id1, &TaskPatch::new()
            .status(TaskStatus::Running)
            .worktree(Some("/repo/.worktrees/1-task-a"))
            .tmux_window(Some("task-1"))).unwrap();
        db.patch_task(id2, &TaskPatch::new()
            .status(TaskStatus::Running)
            .worktree(Some("/repo/.worktrees/1-task-a"))
            .tmux_window(Some("task-1"))).unwrap();

        assert!(db.has_other_tasks_with_worktree("/repo/.worktrees/1-task-a", id1).unwrap());
        assert!(db.has_other_tasks_with_worktree("/repo/.worktrees/1-task-a", id2).unwrap());
    }

    #[test]
    fn has_other_tasks_with_worktree_ignores_done_tasks() {
        let db = in_memory_db();
        let id1 = db.create_task("Task A", "desc", "/repo", None, TaskStatus::Backlog).unwrap();
        let id2 = db.create_task("Task B", "desc", "/repo", None, TaskStatus::Backlog).unwrap();
        db.patch_task(id1, &TaskPatch::new()
            .status(TaskStatus::Running)
            .worktree(Some("/repo/.worktrees/1-task-a"))
            .tmux_window(Some("task-1"))).unwrap();
        db.patch_task(id2, &TaskPatch::new()
            .status(TaskStatus::Done)
            .worktree(Some("/repo/.worktrees/1-task-a"))
            .tmux_window(Some("task-1"))).unwrap();

        assert!(!db.has_other_tasks_with_worktree("/repo/.worktrees/1-task-a", id1).unwrap());
    }

    #[test]
    fn patch_task_clears_plan() {
        let db = in_memory_db();
        let id = db
            .create_task("title", "desc", "/repo", Some("plan.md"), TaskStatus::Ready)
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
        let patch = TaskPatch::new()
            .worktree(None)
            .tmux_window(None);
        db.patch_task(id, &patch).unwrap();
        let task = db.get_task(id).unwrap().unwrap();
        assert!(task.worktree.is_none());
        assert!(task.tmux_window.is_none());
    }

    #[test]
    fn patch_task_status_and_dispatch_together() {
        let db = in_memory_db();
        let id = db
            .create_task("title", "desc", "/repo", None, TaskStatus::Ready)
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
    fn update_status_if_matching() {
        let db = in_memory_db();
        let id = db.create_task("Task", "desc", "/repo", None, TaskStatus::Running).unwrap();

        let updated = db.update_status_if(id, TaskStatus::Review, TaskStatus::Running).unwrap();
        assert!(updated, "should update when current status matches");

        let task = db.get_task(id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Review);
    }

    #[test]
    fn update_status_if_not_matching() {
        let db = in_memory_db();
        let id = db.create_task("Task", "desc", "/repo", None, TaskStatus::Done).unwrap();

        let updated = db.update_status_if(id, TaskStatus::Review, TaskStatus::Running).unwrap();
        assert!(!updated, "should not update when current status doesn't match");

        let task = db.get_task(id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Done, "status should be unchanged");
    }

    #[test]
    fn update_status_if_nonexistent() {
        let db = in_memory_db();
        let updated = db.update_status_if(TaskId(9999), TaskStatus::Review, TaskStatus::Running).unwrap();
        assert!(!updated, "should return false for nonexistent task");
    }
}
