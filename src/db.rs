use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::sync::Mutex;

use crate::models::{Note, NoteSource, Task, TaskStatus};

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

        // PRAGMAs
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA foreign_keys=ON;
             PRAGMA busy_timeout=5000;",
        )
        .context("Failed to set PRAGMAs")?;

        // Schema
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS tasks (
                id          INTEGER PRIMARY KEY,
                title       TEXT NOT NULL,
                description TEXT NOT NULL,
                repo_path   TEXT NOT NULL,
                status      TEXT NOT NULL DEFAULT 'backlog',
                worktree    TEXT,
                tmux_window TEXT,
                created_at  TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS notes (
                id          INTEGER PRIMARY KEY,
                task_id     INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                content     TEXT NOT NULL,
                source      TEXT NOT NULL DEFAULT 'user',
                created_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .context("Failed to create schema")?;

        Ok(Database {
            conn: Mutex::new(conn),
        })
    }

    // -----------------------------------------------------------------------
    // Task CRUD
    // -----------------------------------------------------------------------

    pub fn create_task(
        &self,
        title: &str,
        description: &str,
        repo_path: &str,
    ) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO tasks (title, description, repo_path) VALUES (?1, ?2, ?3)",
            params![title, description, repo_path],
        )
        .context("Failed to insert task")?;
        Ok(conn.last_insert_rowid())
    }

    pub fn get_task(&self, id: i64) -> Result<Option<Task>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, title, description, repo_path, status, worktree, tmux_window,
                    created_at, updated_at
             FROM tasks WHERE id = ?1",
            params![id],
            row_to_task,
        )
        .optional()
        .context("Failed to get task")
    }

    pub fn list_all(&self) -> Result<Vec<Task>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, title, description, repo_path, status, worktree, tmux_window,
                        created_at, updated_at
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

    pub fn list_by_status(&self, status: TaskStatus) -> Result<Vec<Task>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, title, description, repo_path, status, worktree, tmux_window,
                        created_at, updated_at
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

    pub fn update_status(&self, id: i64, status: TaskStatus) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let rows = conn
            .execute(
                "UPDATE tasks SET status = ?1, updated_at = datetime('now') WHERE id = ?2",
                params![status.as_str(), id],
            )
            .context("Failed to update status")?;
        if rows == 0 {
            anyhow::bail!("Task {} not found", id);
        }
        Ok(())
    }

    pub fn update_dispatch(
        &self,
        id: i64,
        worktree: Option<&str>,
        tmux_window: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let rows = conn
            .execute(
                "UPDATE tasks SET worktree = ?1, tmux_window = ?2, updated_at = datetime('now')
                 WHERE id = ?3",
                params![worktree, tmux_window, id],
            )
            .context("Failed to update dispatch fields")?;
        if rows == 0 {
            anyhow::bail!("Task {} not found", id);
        }
        Ok(())
    }

    pub fn delete_task(&self, id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let rows = conn
            .execute("DELETE FROM tasks WHERE id = ?1", params![id])
            .context("Failed to delete task")?;
        if rows == 0 {
            anyhow::bail!("Task {} not found", id);
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Note CRUD
    // -----------------------------------------------------------------------

    pub fn add_note(&self, task_id: i64, content: &str, source: NoteSource) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO notes (task_id, content, source) VALUES (?1, ?2, ?3)",
            params![task_id, content, source.as_str()],
        )
        .context("Failed to insert note")?;
        Ok(conn.last_insert_rowid())
    }

    pub fn list_notes(&self, task_id: i64) -> Result<Vec<Note>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, task_id, content, source, created_at
                 FROM notes WHERE task_id = ?1 ORDER BY id",
            )
            .context("Failed to prepare list_notes")?;
        let notes = stmt
            .query_map(params![task_id], row_to_note)
            .context("Failed to query notes")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to collect notes")?;
        Ok(notes)
    }
}

// ---------------------------------------------------------------------------
// Row helpers
// ---------------------------------------------------------------------------

fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    let status_str: String = row.get(4)?;
    let status = TaskStatus::from_str(&status_str).unwrap_or(TaskStatus::Backlog);

    let created_str: String = row.get(7)?;
    let updated_str: String = row.get(8)?;

    Ok(Task {
        id: row.get(0)?,
        title: row.get(1)?,
        description: row.get(2)?,
        repo_path: row.get(3)?,
        status,
        worktree: row.get(5)?,
        tmux_window: row.get(6)?,
        created_at: parse_datetime(&created_str),
        updated_at: parse_datetime(&updated_str),
    })
}

fn row_to_note(row: &rusqlite::Row<'_>) -> rusqlite::Result<Note> {
    let source_str: String = row.get(3)?;
    let source = NoteSource::from_str(&source_str).unwrap_or(NoteSource::User);

    let created_str: String = row.get(4)?;

    Ok(Note {
        id: row.get(0)?,
        task_id: row.get(1)?,
        content: row.get(2)?,
        source,
        created_at: parse_datetime(&created_str),
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
        // Use an in-memory SQLite database for tests
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA foreign_keys=ON;
             PRAGMA busy_timeout=5000;",
        )
        .unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS tasks (
                id          INTEGER PRIMARY KEY,
                title       TEXT NOT NULL,
                description TEXT NOT NULL,
                repo_path   TEXT NOT NULL,
                status      TEXT NOT NULL DEFAULT 'backlog',
                worktree    TEXT,
                tmux_window TEXT,
                created_at  TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS notes (
                id          INTEGER PRIMARY KEY,
                task_id     INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                content     TEXT NOT NULL,
                source      TEXT NOT NULL DEFAULT 'user',
                created_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        Database {
            conn: Mutex::new(conn),
        }
    }

    #[test]
    fn create_and_get() {
        let db = in_memory_db();
        let id = db.create_task("My Task", "A description", "/repo/path").unwrap();
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
        db.create_task("Task A", "desc", "/a").unwrap();
        db.create_task("Task B", "desc", "/b").unwrap();
        db.create_task("Task C", "desc", "/c").unwrap();
        let tasks = db.list_all().unwrap();
        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks[0].title, "Task A");
        assert_eq!(tasks[1].title, "Task B");
        assert_eq!(tasks[2].title, "Task C");
    }

    #[test]
    fn list_by_status() {
        let db = in_memory_db();
        let id1 = db.create_task("Task A", "desc", "/a").unwrap();
        let id2 = db.create_task("Task B", "desc", "/b").unwrap();
        db.create_task("Task C", "desc", "/c").unwrap();

        db.update_status(id1, TaskStatus::Ready).unwrap();
        db.update_status(id2, TaskStatus::Ready).unwrap();

        let ready = db.list_by_status(TaskStatus::Ready).unwrap();
        assert_eq!(ready.len(), 2);

        let backlog = db.list_by_status(TaskStatus::Backlog).unwrap();
        assert_eq!(backlog.len(), 1);
        assert_eq!(backlog[0].title, "Task C");
    }

    #[test]
    fn update_status() {
        let db = in_memory_db();
        let id = db.create_task("My Task", "desc", "/repo").unwrap();

        let task = db.get_task(id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Backlog);

        db.update_status(id, TaskStatus::Running).unwrap();

        let task = db.get_task(id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Running);
    }

    #[test]
    fn update_status_nonexistent() {
        let db = in_memory_db();
        let result = db.update_status(9999, TaskStatus::Done);
        assert!(result.is_err(), "Should error for nonexistent task");
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("9999"), "Error should mention the id");
    }

    #[test]
    fn update_dispatch_fields() {
        let db = in_memory_db();
        let id = db.create_task("My Task", "desc", "/repo").unwrap();

        db.update_dispatch(id, Some("/worktrees/my-task"), Some("session:my-task"))
            .unwrap();

        let task = db.get_task(id).unwrap().unwrap();
        assert_eq!(task.worktree.as_deref(), Some("/worktrees/my-task"));
        assert_eq!(task.tmux_window.as_deref(), Some("session:my-task"));

        // Clear them
        db.update_dispatch(id, None, None).unwrap();
        let task = db.get_task(id).unwrap().unwrap();
        assert!(task.worktree.is_none());
        assert!(task.tmux_window.is_none());
    }

    #[test]
    fn get_nonexistent() {
        let db = in_memory_db();
        let result = db.get_task(9999).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn add_and_list_notes() {
        let db = in_memory_db();
        let task_id = db.create_task("My Task", "desc", "/repo").unwrap();

        let n1 = db.add_note(task_id, "User note", NoteSource::User).unwrap();
        let n2 = db.add_note(task_id, "Agent note", NoteSource::Agent).unwrap();
        let n3 = db.add_note(task_id, "System note", NoteSource::System).unwrap();

        let notes = db.list_notes(task_id).unwrap();
        assert_eq!(notes.len(), 3);

        assert_eq!(notes[0].id, n1);
        assert_eq!(notes[0].content, "User note");
        assert_eq!(notes[0].source, NoteSource::User);

        assert_eq!(notes[1].id, n2);
        assert_eq!(notes[1].source, NoteSource::Agent);

        assert_eq!(notes[2].id, n3);
        assert_eq!(notes[2].source, NoteSource::System);
    }

    #[test]
    fn delete_task_cascades_notes() {
        let db = in_memory_db();
        let task_id = db.create_task("My Task", "desc", "/repo").unwrap();
        db.add_note(task_id, "Note 1", NoteSource::User).unwrap();
        db.add_note(task_id, "Note 2", NoteSource::Agent).unwrap();

        // Confirm notes exist
        assert_eq!(db.list_notes(task_id).unwrap().len(), 2);

        db.delete_task(task_id).unwrap();

        // Task is gone
        assert!(db.get_task(task_id).unwrap().is_none());

        // Notes cascade-deleted
        assert_eq!(db.list_notes(task_id).unwrap().len(), 0);
    }
}
