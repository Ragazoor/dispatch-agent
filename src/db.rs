use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::sync::Mutex;

use crate::models::{Epic, EpicId, ReviewDecision, ReviewPr, RepoPath, Task, TaskId, TaskStatus, TaskUsage, TmuxWindow, WorktreePath, UsageReport};

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
    pub needs_input: Option<bool>,
    pub pr_url: Option<Option<&'a str>>,
    pub tag: Option<Option<&'a str>>,
    pub sort_order: Option<Option<i64>>,
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

    pub fn needs_input(mut self, needs_input: bool) -> Self {
        self.needs_input = Some(needs_input);
        self
    }

    pub fn pr_url(mut self, pr_url: Option<&'a str>) -> Self {
        self.pr_url = Some(pr_url);
        self
    }

    pub fn tag(mut self, tag: Option<&'a str>) -> Self {
        self.tag = Some(tag);
        self
    }

    pub fn sort_order(mut self, sort_order: Option<i64>) -> Self {
        self.sort_order = Some(sort_order);
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
            || self.needs_input.is_some()
            || self.pr_url.is_some()
            || self.tag.is_some()
            || self.sort_order.is_some()
    }
}

// ---------------------------------------------------------------------------
// EpicPatch — builder for selective epic field updates
// ---------------------------------------------------------------------------

/// Builder for partial epic updates, mirroring `TaskPatch`. Each field is
/// `None` by default (= don't change). For nullable columns (`plan`) we use
/// a double-Option: `None` = don't change, `Some(None)` = set NULL,
/// `Some(Some(x))` = set value.
#[derive(Debug, Default)]
pub struct EpicPatch<'a> {
    pub title: Option<&'a str>,
    pub description: Option<&'a str>,
    pub done: Option<bool>,
    pub plan: Option<Option<&'a str>>,
    pub sort_order: Option<Option<i64>>,
}

impl<'a> EpicPatch<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn title(mut self, title: &'a str) -> Self {
        self.title = Some(title);
        self
    }

    pub fn description(mut self, description: &'a str) -> Self {
        self.description = Some(description);
        self
    }

    pub fn done(mut self, done: bool) -> Self {
        self.done = Some(done);
        self
    }

    pub fn plan(mut self, plan: Option<&'a str>) -> Self {
        self.plan = Some(plan);
        self
    }

    pub fn sort_order(mut self, sort_order: Option<i64>) -> Self {
        self.sort_order = Some(sort_order);
        self
    }

    pub fn has_changes(&self) -> bool {
        self.title.is_some()
            || self.description.is_some()
            || self.done.is_some()
            || self.plan.is_some()
            || self.sort_order.is_some()
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

    // Epic operations
    fn create_epic(&self, title: &str, description: &str, repo_path: &str) -> Result<Epic>;
    fn get_epic(&self, id: EpicId) -> Result<Option<Epic>>;
    fn list_epics(&self) -> Result<Vec<Epic>>;
    fn patch_epic(&self, id: EpicId, patch: &EpicPatch<'_>) -> Result<()>;
    fn delete_epic(&self, id: EpicId) -> Result<()>;
    fn set_task_epic_id(&self, task_id: TaskId, epic_id: Option<EpicId>) -> Result<()>;
    fn list_tasks_for_epic(&self, epic_id: EpicId) -> Result<Vec<Task>>;

    // Settings
    fn get_setting_bool(&self, key: &str) -> Result<Option<bool>>;
    fn set_setting_bool(&self, key: &str, value: bool) -> Result<()>;
    fn get_setting_string(&self, key: &str) -> Result<Option<String>>;
    fn set_setting_string(&self, key: &str, value: &str) -> Result<()>;

    // Usage tracking
    fn report_usage(&self, task_id: TaskId, usage: &UsageReport) -> Result<()>;

    fn get_all_usage(&self) -> Result<Vec<TaskUsage>>;

    // Filter presets
    fn save_filter_preset(&self, name: &str, repo_paths: &str) -> Result<()>;
    fn delete_filter_preset(&self, name: &str) -> Result<()>;
    fn list_filter_presets(&self) -> Result<Vec<(String, String)>>;

    // Review PRs
    fn save_review_prs(&self, prs: &[crate::models::ReviewPr]) -> Result<()>;
    fn load_review_prs(&self) -> Result<Vec<crate::models::ReviewPr>>;
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
                tag         TEXT,
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

        if current_version < 3 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS epics (
                    id          INTEGER PRIMARY KEY,
                    title       TEXT NOT NULL,
                    description TEXT NOT NULL,
                    plan        TEXT NOT NULL DEFAULT '',
                    repo_path   TEXT NOT NULL,
                    done        INTEGER NOT NULL DEFAULT 0,
                    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
                    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
                )",
            )
            .context("Failed to create epics table")?;

            let _ = conn.execute_batch(
                "ALTER TABLE tasks ADD COLUMN epic_id INTEGER REFERENCES epics(id)"
            );

            conn.pragma_update(None, "user_version", 3i64)
                .context("Failed to update schema version to 3")?;
        }

        if current_version < 4 {
            // Migration 4: add needs_input column + drop plan column from epics.
            let _ = conn.execute_batch(
                "ALTER TABLE tasks ADD COLUMN needs_input INTEGER NOT NULL DEFAULT 0"
            );

            // SQLite doesn't support DROP COLUMN before 3.35.0; recreate the table.
            // Disable FK checks so DROP TABLE succeeds when tasks reference epics,
            // and wrap in a transaction for atomicity.
            conn.execute_batch(
                "PRAGMA foreign_keys = OFF;
                BEGIN;
                CREATE TABLE epics_new (
                    id          INTEGER PRIMARY KEY,
                    title       TEXT NOT NULL,
                    description TEXT NOT NULL,
                    repo_path   TEXT NOT NULL,
                    done        INTEGER NOT NULL DEFAULT 0,
                    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
                    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
                );
                INSERT INTO epics_new (id, title, description, repo_path, done, created_at, updated_at)
                    SELECT id, title, description, repo_path, done, created_at, updated_at FROM epics;
                DROP TABLE epics;
                ALTER TABLE epics_new RENAME TO epics;
                COMMIT;
                PRAGMA foreign_keys = ON;",
            )
            .context("Failed to migrate epics (drop plan column)")?;
            conn.pragma_update(None, "user_version", 4i64)
                .context("Failed to update schema version to 4")?;
        }

        if current_version < 5 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS settings (
                    key   TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                )",
            )
            .context("Failed to create settings table")?;
            conn.pragma_update(None, "user_version", 5i64)
                .context("Failed to update schema version to 5")?;
        }

        if current_version < 6 {
            conn.execute_batch(
                "UPDATE tasks SET status = 'backlog' WHERE status = 'ready'"
            )
            .context("Failed to migrate ready tasks to backlog")?;
            conn.pragma_update(None, "user_version", 6i64)
                .context("Failed to update schema version to 6")?;
        }

        if current_version < 7 {
            let _ = conn.execute_batch("ALTER TABLE tasks ADD COLUMN pr_url TEXT");
            let _ = conn.execute_batch("ALTER TABLE tasks ADD COLUMN pr_number INTEGER");
            conn.pragma_update(None, "user_version", 7i64)
                .context("Failed to update schema version to 7")?;
        }

        if current_version < 8 {
            let _ = conn.execute_batch("ALTER TABLE epics ADD COLUMN plan TEXT");
            conn.pragma_update(None, "user_version", 8i64)
                .context("Failed to update schema version to 8")?;
        }

        if current_version < 9 {
            let _ = conn.execute_batch("ALTER TABLE tasks ADD COLUMN sort_order INTEGER");
            let _ = conn.execute_batch("ALTER TABLE epics ADD COLUMN sort_order INTEGER");
            conn.pragma_update(None, "user_version", 9i64)
                .context("Failed to update schema version to 9")?;
        }

        if current_version < 10 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS task_usage (
                    task_id            INTEGER PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
                    cost_usd           REAL    NOT NULL DEFAULT 0.0,
                    input_tokens       INTEGER NOT NULL DEFAULT 0,
                    output_tokens      INTEGER NOT NULL DEFAULT 0,
                    cache_read_tokens  INTEGER NOT NULL DEFAULT 0,
                    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
                    updated_at         TEXT    NOT NULL DEFAULT (datetime('now'))
                )",
            )
            .context("Failed to create task_usage table")?;
            conn.pragma_update(None, "user_version", 10i64)
                .context("Failed to update schema version to 10")?;
        }

        if current_version < 11 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS filter_presets (
                    name       TEXT PRIMARY KEY,
                    repo_paths TEXT NOT NULL
                )",
            )
            .context("Failed to create filter_presets table")?;
            conn.pragma_update(None, "user_version", 11i64)
                .context("Failed to update schema version to 11")?;
        }

        if current_version < 12 {
            // DROP COLUMN requires SQLite 3.35.0+; bundled libsqlite3-sys satisfies this.
            // Ignore error for fresh DBs where the column was never added.
            let _ = conn.execute_batch("ALTER TABLE tasks DROP COLUMN pr_number");
            conn.pragma_update(None, "user_version", 12i64)
                .context("Failed to update schema version to 12")?;
        }

        if current_version < 13 {
            // Add optional tag column for dispatch behavior (bug, feature, chore, epic).
            let _ = conn.execute_batch("ALTER TABLE tasks ADD COLUMN tag TEXT");
            conn.pragma_update(None, "user_version", 13i64)
                .context("Failed to update schema version to 13")?;
        }

        if current_version < 14 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS review_prs (
                    repo            TEXT    NOT NULL,
                    number          INTEGER NOT NULL,
                    title           TEXT    NOT NULL,
                    author          TEXT    NOT NULL,
                    url             TEXT    NOT NULL,
                    is_draft        INTEGER NOT NULL,
                    created_at      TEXT    NOT NULL,
                    updated_at      TEXT    NOT NULL,
                    additions       INTEGER NOT NULL,
                    deletions       INTEGER NOT NULL,
                    review_decision TEXT    NOT NULL,
                    labels          TEXT    NOT NULL,
                    PRIMARY KEY (repo, number)
                )",
            )
            .context("Failed to create review_prs table")?;
            conn.pragma_update(None, "user_version", 14i64)
                .context("Failed to update schema version to 14")?;
        }

        Ok(())
    }

    fn conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| anyhow::anyhow!("db lock poisoned"))
    }
}

/// Column list shared by all task SELECT queries. Pair with `row_to_task`.
const TASK_COLUMNS: &str =
    "id, title, description, repo_path, status, worktree, tmux_window, \
     plan, epic_id, needs_input, pr_url, tag, sort_order, created_at, updated_at";

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
            &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?1"),
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
                &format!("SELECT {TASK_COLUMNS} FROM tasks ORDER BY COALESCE(sort_order, id) ASC, id ASC"),
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
                &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE status = ?1 ORDER BY COALESCE(sort_order, id) ASC, id ASC"),
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
            &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE plan = ?1"),
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
        if !patch.has_changes() {
            return Ok(());
        }
        let conn = self.conn()?;
        let mut sets: Vec<&str> = Vec::new();
        let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(s) = patch.status {
            sets.push("status = ?");
            values.push(Box::new(s.as_str().to_string()));
        }
        if let Some(t) = patch.title {
            sets.push("title = ?");
            values.push(Box::new(t.to_string()));
        }
        if let Some(d) = patch.description {
            sets.push("description = ?");
            values.push(Box::new(d.to_string()));
        }
        if let Some(r) = patch.repo_path {
            sets.push("repo_path = ?");
            values.push(Box::new(r.to_string()));
        }
        if let Some(p) = patch.plan {
            sets.push("plan = ?");
            values.push(Box::new(p.map(|s| s.to_string())));
        }
        if let Some(w) = patch.worktree {
            sets.push("worktree = ?");
            values.push(Box::new(w.map(|s| s.to_string())));
        }
        if let Some(t) = patch.tmux_window {
            sets.push("tmux_window = ?");
            values.push(Box::new(t.map(|s| s.to_string())));
        }
        if let Some(ni) = patch.needs_input {
            sets.push("needs_input = ?");
            values.push(Box::new(ni as i64));
        }
        if let Some(url) = &patch.pr_url {
            sets.push("pr_url = ?");
            values.push(Box::new(url.map(|s| s.to_string())));
        }
        if let Some(tag) = &patch.tag {
            sets.push("tag = ?");
            values.push(Box::new(tag.map(|s| s.to_string())));
        }
        if let Some(so) = patch.sort_order {
            sets.push("sort_order = ?");
            values.push(Box::new(so));
        }

        sets.push("updated_at = datetime('now')");
        values.push(Box::new(id.0));

        let sql = format!(
            "UPDATE tasks SET {} WHERE id = ?",
            sets.join(", ")
        );
        let refs: Vec<&dyn rusqlite::types::ToSql> =
            values.iter().map(|v| v.as_ref()).collect();
        let rows = conn.execute(&sql, refs.as_slice())
            .context("Failed to patch task")?;
        if rows == 0 {
            anyhow::bail!("Task {id} not found");
        }
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

    fn create_epic(&self, title: &str, description: &str, repo_path: &str) -> Result<Epic> {
        let id = {
            let conn = self.conn()?;
            conn.execute(
                "INSERT INTO epics (title, description, repo_path) VALUES (?1, ?2, ?3)",
                params![title, description, repo_path],
            )
            .context("Failed to insert epic")?;
            EpicId(conn.last_insert_rowid())
        }; // MutexGuard dropped here — avoids deadlock when get_epic() re-locks
        self.get_epic(id)?
            .ok_or_else(|| anyhow::anyhow!("Epic {id} vanished after insert"))
    }

    fn get_epic(&self, id: EpicId) -> Result<Option<Epic>> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, title, description, repo_path, done, plan, sort_order, created_at, updated_at
             FROM epics WHERE id = ?1",
            params![id.0],
            row_to_epic,
        )
        .optional()
        .context("Failed to get epic")
    }

    fn list_epics(&self) -> Result<Vec<Epic>> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, title, description, repo_path, done, plan, sort_order, created_at, updated_at
                 FROM epics ORDER BY COALESCE(sort_order, id) ASC, id ASC",
            )
            .context("Failed to prepare list_epics")?;
        let epics = stmt
            .query_map([], row_to_epic)
            .context("Failed to query epics")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to collect epics")?;
        Ok(epics)
    }

    fn patch_epic(&self, id: EpicId, patch: &EpicPatch<'_>) -> Result<()> {
        if !patch.has_changes() {
            return Ok(());
        }
        let conn = self.conn()?;
        let mut sets: Vec<&str> = Vec::new();
        let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(t) = patch.title {
            sets.push("title = ?");
            values.push(Box::new(t.to_string()));
        }
        if let Some(d) = patch.description {
            sets.push("description = ?");
            values.push(Box::new(d.to_string()));
        }
        if let Some(d) = patch.done {
            sets.push("done = ?");
            values.push(Box::new(d as i64));
        }
        if let Some(p) = patch.plan {
            sets.push("plan = ?");
            values.push(Box::new(p.map(|s| s.to_string())));
        }
        if let Some(so) = patch.sort_order {
            sets.push("sort_order = ?");
            values.push(Box::new(so));
        }

        sets.push("updated_at = datetime('now')");
        values.push(Box::new(id.0));

        let sql = format!(
            "UPDATE epics SET {} WHERE id = ?",
            sets.join(", ")
        );
        let refs: Vec<&dyn rusqlite::types::ToSql> =
            values.iter().map(|v| v.as_ref()).collect();
        let rows = conn.execute(&sql, refs.as_slice())
            .context("Failed to patch epic")?;
        if rows == 0 {
            anyhow::bail!("Epic {id} not found");
        }
        Ok(())
    }

    fn delete_epic(&self, id: EpicId) -> Result<()> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM tasks WHERE epic_id = ?1", params![id.0])
            .context("Failed to delete epic subtasks")?;
        let rows = conn
            .execute("DELETE FROM epics WHERE id = ?1", params![id.0])
            .context("Failed to delete epic")?;
        if rows == 0 {
            anyhow::bail!("Epic {} not found", id);
        }
        Ok(())
    }

    fn set_task_epic_id(&self, task_id: TaskId, epic_id: Option<EpicId>) -> Result<()> {
        let conn = self.conn()?;
        let rows = conn
            .execute(
                "UPDATE tasks SET epic_id = ?1, updated_at = datetime('now') WHERE id = ?2",
                params![epic_id.map(|e| e.0), task_id.0],
            )
            .context("Failed to set task epic_id")?;
        if rows == 0 {
            anyhow::bail!("Task {} not found", task_id);
        }
        Ok(())
    }

    fn list_tasks_for_epic(&self, epic_id: EpicId) -> Result<Vec<Task>> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE epic_id = ?1 ORDER BY COALESCE(sort_order, id) ASC, id ASC"),
            )
            .context("Failed to prepare list_tasks_for_epic")?;
        let tasks = stmt
            .query_map(params![epic_id.0], row_to_task)
            .context("Failed to query tasks for epic")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to collect tasks for epic")?;
        Ok(tasks)
    }

    fn get_setting_bool(&self, key: &str) -> Result<Option<bool>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![key],
            |row| {
                let v: String = row.get(0)?;
                Ok(v == "1")
            },
        )
        .optional()
        .context("Failed to get setting")
    }

    fn set_setting_bool(&self, key: &str, value: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = ?2",
            params![key, if value { "1" } else { "0" }],
        )?;
        Ok(())
    }

    fn get_setting_string(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()
        .context("Failed to get setting")
    }

    fn set_setting_string(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = ?2",
            params![key, value],
        )?;
        Ok(())
    }

    fn report_usage(&self, task_id: TaskId, usage: &UsageReport) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO task_usage
                 (task_id, cost_usd, input_tokens, output_tokens,
                  cache_read_tokens, cache_write_tokens, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'))
             ON CONFLICT(task_id) DO UPDATE SET
                 cost_usd           = cost_usd           + excluded.cost_usd,
                 input_tokens       = input_tokens       + excluded.input_tokens,
                 output_tokens      = output_tokens      + excluded.output_tokens,
                 cache_read_tokens  = cache_read_tokens  + excluded.cache_read_tokens,
                 cache_write_tokens = cache_write_tokens + excluded.cache_write_tokens,
                 updated_at         = excluded.updated_at",
            params![task_id.0, usage.cost_usd, usage.input_tokens, usage.output_tokens,
                    usage.cache_read_tokens, usage.cache_write_tokens],
        )
        .context("Failed to upsert task_usage")?;
        Ok(())
    }

    fn get_all_usage(&self) -> Result<Vec<TaskUsage>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT task_id, cost_usd, input_tokens, output_tokens,
                    cache_read_tokens, cache_write_tokens, updated_at
             FROM task_usage",
        )
        .context("Failed to prepare get_all_usage")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, f64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })
            .context("Failed to query task_usage")?;
        let mut out = Vec::new();
        for row in rows {
            let (task_id, cost_usd, input, output, cr, cw, updated_at_str) =
                row.context("Failed to read usage row")?;
            let updated_at = NaiveDateTime::parse_from_str(&updated_at_str, "%Y-%m-%d %H:%M:%S")
                .with_context(|| format!("Invalid updated_at in task_usage: {updated_at_str:?}"))?
                .and_utc();
            out.push(TaskUsage {
                task_id: TaskId(task_id),
                cost_usd,
                input_tokens: input,
                output_tokens: output,
                cache_read_tokens: cr,
                cache_write_tokens: cw,
                updated_at,
            });
        }
        Ok(out)
    }

    fn save_filter_preset(&self, name: &str, repo_paths: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO filter_presets (name, repo_paths) VALUES (?1, ?2)
             ON CONFLICT(name) DO UPDATE SET repo_paths = ?2",
            params![name, repo_paths],
        )?;
        Ok(())
    }

    fn delete_filter_preset(&self, name: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM filter_presets WHERE name = ?1", params![name])?;
        Ok(())
    }

    fn list_filter_presets(&self) -> Result<Vec<(String, String)>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare("SELECT name, repo_paths FROM filter_presets ORDER BY name")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>().context("Failed to list filter presets")
    }

    fn save_review_prs(&self, prs: &[ReviewPr]) -> Result<()> {
        let conn = self.conn()?;
        let tx = conn.unchecked_transaction()?;
        tx.execute("DELETE FROM review_prs", [])?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO review_prs (repo, number, title, author, url, is_draft,
                 created_at, updated_at, additions, deletions, review_decision, labels)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            )?;
            for pr in prs {
                let labels_json =
                    serde_json::to_string(&pr.labels).context("Failed to serialize labels")?;
                stmt.execute(params![
                    pr.repo,
                    pr.number,
                    pr.title,
                    pr.author,
                    pr.url,
                    pr.is_draft,
                    pr.created_at.to_rfc3339(),
                    pr.updated_at.to_rfc3339(),
                    pr.additions,
                    pr.deletions,
                    pr.review_decision.as_db_str(),
                    labels_json,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn load_review_prs(&self) -> Result<Vec<ReviewPr>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT repo, number, title, author, url, is_draft,
                    created_at, updated_at, additions, deletions,
                    review_decision, labels
             FROM review_prs",
        )?;
        let rows = stmt.query_map([], |row| {
            let repo: String = row.get(0)?;
            let number: i64 = row.get(1)?;
            let title: String = row.get(2)?;
            let author: String = row.get(3)?;
            let url: String = row.get(4)?;
            let is_draft: bool = row.get(5)?;
            let created_at_str: String = row.get(6)?;
            let updated_at_str: String = row.get(7)?;
            let additions: i64 = row.get(8)?;
            let deletions: i64 = row.get(9)?;
            let decision_str: String = row.get(10)?;
            let labels_json: String = row.get(11)?;
            Ok((
                repo,
                number,
                title,
                author,
                url,
                is_draft,
                created_at_str,
                updated_at_str,
                additions,
                deletions,
                decision_str,
                labels_json,
            ))
        })?;

        let mut prs = Vec::new();
        for row in rows {
            let (
                repo,
                number,
                title,
                author,
                url,
                is_draft,
                created_at_str,
                updated_at_str,
                additions,
                deletions,
                decision_str,
                labels_json,
            ) = row?;

            let created_at = DateTime::parse_from_rfc3339(&created_at_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
            let updated_at = DateTime::parse_from_rfc3339(&updated_at_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
            let review_decision = ReviewDecision::from_db_str(&decision_str)
                .unwrap_or(ReviewDecision::ReviewRequired);
            let labels: Vec<String> = serde_json::from_str(&labels_json).unwrap_or_default();

            prs.push(ReviewPr {
                number,
                title,
                author,
                repo,
                url,
                is_draft,
                created_at,
                updated_at,
                additions,
                deletions,
                review_decision,
                labels,
            });
        }
        Ok(prs)
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
        repo_path: RepoPath(row.get::<_, String>("repo_path")?),
        status,
        worktree: row.get::<_, Option<String>>("worktree")?.map(WorktreePath),
        tmux_window: row.get::<_, Option<String>>("tmux_window")?.map(TmuxWindow),
        plan: row.get("plan")?,
        epic_id: row.get::<_, Option<i64>>("epic_id")
            .unwrap_or(None)
            .map(EpicId),
        needs_input: row.get::<_, i64>("needs_input").unwrap_or(0) != 0,
        pr_url: row.get::<_, Option<String>>("pr_url").unwrap_or(None),
        tag: row.get::<_, Option<String>>("tag").unwrap_or(None),
        sort_order: row.get::<_, Option<i64>>("sort_order").unwrap_or(None),
        created_at: parse_datetime(&created_str),
        updated_at: parse_datetime(&updated_str),
    })
}

fn row_to_epic(row: &rusqlite::Row<'_>) -> rusqlite::Result<Epic> {
    let created_str: String = row.get("created_at")?;
    let updated_str: String = row.get("updated_at")?;
    let done_int: i64 = row.get("done")?;

    Ok(Epic {
        id: EpicId(row.get("id")?),
        title: row.get("title")?,
        description: row.get("description")?,
        repo_path: RepoPath(row.get::<_, String>("repo_path")?),
        done: done_int != 0,
        plan: row.get("plan")?,
        sort_order: row.get::<_, Option<i64>>("sort_order").unwrap_or(None),
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
        assert_eq!(task.repo_path, RepoPath("/repo/path".into()));
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

        db.patch_task(id1, &TaskPatch::new().status(TaskStatus::Running)).unwrap();
        db.patch_task(id2, &TaskPatch::new().status(TaskStatus::Running)).unwrap();

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
        let id = db.create_task("Planned", "desc", "/repo", Some("/plans/my-plan.md"), TaskStatus::Backlog).unwrap();

        let found = db.find_task_by_plan("/plans/my-plan.md").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, id);
    }

    #[test]
    fn find_task_by_plan_returns_none_when_no_match() {
        let db = in_memory_db();
        db.create_task("Other", "desc", "/repo", Some("/plans/other.md"), TaskStatus::Backlog).unwrap();

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
    fn get_setting_bool_returns_none_when_absent() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(db.get_setting_bool("notifications_enabled").unwrap(), None);
    }

    #[test]
    fn set_and_get_setting_bool_roundtrips() {
        let db = Database::open_in_memory().unwrap();
        db.set_setting_bool("notifications_enabled", true).unwrap();
        assert_eq!(db.get_setting_bool("notifications_enabled").unwrap(), Some(true));

        db.set_setting_bool("notifications_enabled", false).unwrap();
        assert_eq!(db.get_setting_bool("notifications_enabled").unwrap(), Some(false));
    }

    #[test]
    fn get_setting_string_returns_none_when_absent() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(db.get_setting_string("repo_filter").unwrap(), None);
    }

    #[test]
    fn set_and_get_setting_string() {
        let db = Database::open_in_memory().unwrap();
        db.set_setting_string("repo_filter", "/repo1\n/repo2").unwrap();
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
        assert_eq!(db.get_setting_string("repo_filter").unwrap(), Some("new".to_string()));
    }

    #[test]
    fn fresh_db_has_latest_schema_version() {
        let db = in_memory_db();
        let conn = db.conn.lock().unwrap();
        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0)).unwrap();
        assert_eq!(version, 14, "fresh DB should be at schema version 14");
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
        assert_eq!(version, 14);

        // Verify Migration 1 added the plan column
        let has_plan: bool = conn
            .prepare("SELECT plan FROM tasks LIMIT 1")
            .is_ok();
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
        ).unwrap();

        // Insert a ready task
        conn.execute(
            "INSERT INTO tasks (title, description, repo_path, status) VALUES ('T', 'D', '/r', 'ready')",
            [],
        ).unwrap();

        Database::init_schema(&conn).unwrap();

        let status: String = conn
            .query_row("SELECT status FROM tasks WHERE id = 1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(status, "backlog");

        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0)).unwrap();
        assert_eq!(version, 14);
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
        assert_eq!(task.repo_path, RepoPath("/repo".into()));
        assert_eq!(task.status, TaskStatus::Backlog);
        assert!(task.worktree.is_none());
        assert!(task.tmux_window.is_none());
        assert!(task.plan.is_none());
    }

    #[test]
    fn create_task_returning_with_plan() {
        let db = in_memory_db();
        let task = db.create_task_returning("T", "D", "/r", Some("plan.md"), TaskStatus::Backlog).unwrap();
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
            .create_task("title", "desc", "/repo", Some("plan.md"), TaskStatus::Running)
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
        let id = db.create_task("title", "desc", "/repo", None, TaskStatus::Backlog).unwrap();
        db.patch_task(id, &TaskPatch::new().tag(Some("bug"))).unwrap();
        let task = db.get_task(id).unwrap().unwrap();
        assert_eq!(task.tag.as_deref(), Some("bug"));
    }

    #[test]
    fn patch_task_clears_tag() {
        let db = in_memory_db();
        let id = db.create_task("title", "desc", "/repo", None, TaskStatus::Backlog).unwrap();
        db.patch_task(id, &TaskPatch::new().tag(Some("feature"))).unwrap();
        db.patch_task(id, &TaskPatch::new().tag(None)).unwrap();
        let task = db.get_task(id).unwrap().unwrap();
        assert!(task.tag.is_none());
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
            .create_task("title", "desc", "/repo", Some("plan.md"), TaskStatus::Backlog)
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
        assert_eq!(task.worktree.as_ref().map(|w| w.as_ref()), Some("/repo/.worktrees/1-my-task"));
        assert_eq!(task.tmux_window, Some(TmuxWindow("session:1-my-task".into())));
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
            .create_task("title", "desc", "/repo", None, TaskStatus::Backlog)
            .unwrap();
        let patch = TaskPatch::new()
            .status(TaskStatus::Running)
            .worktree(Some("/repo/.worktrees/1-my-task"))
            .tmux_window(Some("session:1-my-task"));
        db.patch_task(id, &patch).unwrap();
        let task = db.get_task(id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Running);
        assert_eq!(task.worktree.as_ref().map(|w| w.as_ref()), Some("/repo/.worktrees/1-my-task"));
        assert_eq!(task.tmux_window, Some(TmuxWindow("session:1-my-task".into())));
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

    // --- Epic CRUD ---

    #[test]
    fn create_and_get_epic() {
        let db = in_memory_db();
        let epic = db.create_epic("Auth Rewrite", "Rewrite auth", "/repo").unwrap();
        assert_eq!(epic.title, "Auth Rewrite");
        assert_eq!(epic.description, "Rewrite auth");
        assert_eq!(epic.repo_path, RepoPath("/repo".into()));
        assert!(!epic.done);

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
        db.create_task("Sub 1", "desc", "/repo", None, TaskStatus::Backlog).unwrap();
        let sub_id = db.create_task("Sub 2", "desc", "/repo", None, TaskStatus::Backlog).unwrap();

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
    fn patch_epic_done_flag() {
        let db = in_memory_db();
        let epic = db.create_epic("Epic", "desc", "/repo").unwrap();
        assert!(!epic.done);

        db.patch_epic(epic.id, &EpicPatch::new().done(true)).unwrap();
        let updated = db.get_epic(epic.id).unwrap().unwrap();
        assert!(updated.done);
    }

    #[test]
    fn patch_epic_title() {
        let db = in_memory_db();
        let epic = db.create_epic("Old Title", "desc", "/repo").unwrap();

        db.patch_epic(epic.id, &EpicPatch::new().title("New Title")).unwrap();
        let updated = db.get_epic(epic.id).unwrap().unwrap();
        assert_eq!(updated.title, "New Title");
        assert_eq!(updated.description, "desc"); // unchanged
    }

    #[test]
    fn task_epic_id_roundtrip() {
        let db = in_memory_db();
        let epic = db.create_epic("Epic", "desc", "/repo").unwrap();
        let task_id = db.create_task("Task", "desc", "/repo", None, TaskStatus::Backlog).unwrap();

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
        let id1 = db.create_task("Sub A", "desc", "/repo", None, TaskStatus::Backlog).unwrap();
        let _id2 = db.create_task("Standalone", "desc", "/repo", None, TaskStatus::Backlog).unwrap();

        db.set_task_epic_id(id1, Some(epic.id)).unwrap();

        let subtasks = db.list_tasks_for_epic(epic.id).unwrap();
        assert_eq!(subtasks.len(), 1);
        assert_eq!(subtasks[0].title, "Sub A");
    }

    #[test]
    fn task_roundtrip_with_pr_fields() {
        let db = in_memory_db();
        let id = db.create_task("PR task", "desc", "/repo", None, TaskStatus::Backlog).unwrap();

        db.patch_task(id, &TaskPatch::new()
            .pr_url(Some("https://github.com/org/repo/pull/42"))
        ).unwrap();

        let task = db.get_task(id).unwrap().unwrap();
        assert_eq!(task.pr_url.as_deref(), Some("https://github.com/org/repo/pull/42"));
    }

    #[test]
    fn task_pr_fields_default_to_none() {
        let db = in_memory_db();
        let id = db.create_task("No PR", "desc", "/repo", None, TaskStatus::Backlog).unwrap();
        let task = db.get_task(id).unwrap().unwrap();
        assert!(task.pr_url.is_none());
    }

    #[test]
    fn patch_task_sets_pr_url() {
        let db = in_memory_db();
        let id = db.create_task("t", "d", "/r", None, TaskStatus::Backlog).unwrap();

        db.patch_task(id, &TaskPatch::new().pr_url(Some("https://example.com/pull/1"))).unwrap();
        let task = db.get_task(id).unwrap().unwrap();
        assert_eq!(task.pr_url.as_deref(), Some("https://example.com/pull/1"));
    }

    #[test]
    fn patch_epic_plan() {
        let db = in_memory_db();
        let epic = db.create_epic("Epic", "desc", "/repo").unwrap();
        assert!(epic.plan.is_none());

        db.patch_epic(epic.id, &EpicPatch::new().plan(Some("docs/plan.md"))).unwrap();
        let updated = db.get_epic(epic.id).unwrap().unwrap();
        assert_eq!(updated.plan.as_deref(), Some("docs/plan.md"));
    }

    #[test]
    fn patch_epic_clear_plan() {
        let db = in_memory_db();
        let epic = db.create_epic("Epic", "desc", "/repo").unwrap();

        db.patch_epic(epic.id, &EpicPatch::new().plan(Some("docs/plan.md"))).unwrap();
        db.patch_epic(epic.id, &EpicPatch::new().plan(None)).unwrap();
        let updated = db.get_epic(epic.id).unwrap().unwrap();
        assert!(updated.plan.is_none());
    }

    #[test]
    fn patch_task_sets_sort_order() {
        let db = Database::open_in_memory().unwrap();
        let id = db.create_task("T", "d", "/r", None, TaskStatus::Backlog).unwrap();
        db.patch_task(id, &TaskPatch::new().sort_order(Some(500))).unwrap();
        let task = db.get_task(id).unwrap().unwrap();
        assert_eq!(task.sort_order, Some(500));
    }

    #[test]
    fn patch_task_clears_sort_order() {
        let db = Database::open_in_memory().unwrap();
        let id = db.create_task("T", "d", "/r", None, TaskStatus::Backlog).unwrap();
        db.patch_task(id, &TaskPatch::new().sort_order(Some(100))).unwrap();
        db.patch_task(id, &TaskPatch::new().sort_order(None)).unwrap();
        let task = db.get_task(id).unwrap().unwrap();
        assert_eq!(task.sort_order, None);
    }

    #[test]
    fn report_usage_first_insert() {
        let db = Database::open_in_memory().unwrap();
        let id = db.create_task("T", "D", "/r", None, TaskStatus::Backlog).unwrap();
        db.report_usage(id, &UsageReport { cost_usd: 0.42, input_tokens: 10_000, output_tokens: 2_000, cache_read_tokens: 0, cache_write_tokens: 0 }).unwrap();
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
        let id = db.create_task("T", "D", "/r", None, TaskStatus::Backlog).unwrap();
        db.report_usage(id, &UsageReport { cost_usd: 0.10, input_tokens: 1_000, output_tokens: 500, cache_read_tokens: 100, cache_write_tokens: 50 }).unwrap();
        db.report_usage(id, &UsageReport { cost_usd: 0.05, input_tokens: 500, output_tokens: 250, cache_read_tokens: 50, cache_write_tokens: 25 }).unwrap();
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
        db.save_filter_preset("frontend", "/repo-a\n/repo-b").unwrap();
        db.save_filter_preset("backend", "/repo-c").unwrap();

        let presets = db.list_filter_presets().unwrap();
        assert_eq!(presets.len(), 2);
        assert_eq!(presets[0].0, "backend"); // sorted by name
        assert_eq!(presets[1].0, "frontend");
        assert_eq!(presets[1].1, "/repo-a\n/repo-b");
    }

    #[test]
    fn filter_presets_overwrite_and_delete() {
        let db = Database::open_in_memory().unwrap();
        db.save_filter_preset("frontend", "/repo-a").unwrap();
        db.save_filter_preset("frontend", "/repo-x\n/repo-y").unwrap();

        let presets = db.list_filter_presets().unwrap();
        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].1, "/repo-x\n/repo-y");

        db.delete_filter_preset("frontend").unwrap();
        let presets = db.list_filter_presets().unwrap();
        assert!(presets.is_empty());
    }

    #[test]
    fn save_and_load_review_prs() {
        use crate::models::{ReviewDecision, ReviewPr};
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
        use crate::models::{ReviewDecision, ReviewPr};
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
        };
        db.save_review_prs(&[pr1]).unwrap();
        assert_eq!(db.load_review_prs().unwrap().len(), 1);

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
        };
        db.save_review_prs(&[pr2]).unwrap();

        let loaded = db.load_review_prs().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].number, 2);
        assert_eq!(loaded[0].repo, "acme/other");
    }
}
