# Task Orchestrator TUI — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Rust TUI kanban board that manages development tasks and dispatches Claude Code agents into tmux windows, with MCP-based status reporting and CLI fallback.

**Architecture:** Single binary with three modes (tui, update, list). Elm architecture for TUI (message/update/command pattern). Axum MCP server runs alongside the TUI in the same Tokio runtime. SQLite for persistence via rusqlite (sync, spawn_blocking). Agents get dispatched into tmux windows in the current session with worktree isolation.

**Tech Stack:** Rust 2021 edition, ratatui + crossterm, tokio, rusqlite (bundled), axum, serde + serde_json, clap

**Spec:** `docs/specs/2026-03-25-task-orchestrator-design.md`

---

## File Structure

```
task-orchestrator-tui/
├── Cargo.toml
├── README.md
├── src/
│   ├── main.rs           # CLI parsing (clap), entrypoint routing to tui/update/list
│   ├── models.rs         # Task, TaskStatus, Note, DispatchResult, slugify()
│   ├── db.rs             # SQLite schema init, CRUD for tasks and notes
│   ├── tui/
│   │   ├── mod.rs        # App state, Message/Command enums, update() fn (Elm arch)
│   │   ├── ui.rs         # Ratatui rendering: kanban columns, detail panel, status bar
│   │   └── input.rs      # Key event → Message mapping, input mode handling
│   ├── tmux.rs           # tmux subprocess wrappers: new-window, capture-pane, has-window, kill-window
│   ├── dispatch.rs       # Orchestrates dispatch: worktree + .mcp.json + tmux + claude launch
│   └── mcp/
│       ├── mod.rs        # Axum server setup, route registration, start on localhost
│       └── handlers.rs   # MCP tool handlers: update_task, add_note, get_task
└── tests/
    ├── db_test.rs         # Integration tests for SQLite CRUD
    └── models_test.rs     # Unit tests for Task, TaskStatus, slugify
```

---

## Phase 1: Foundation

### Task 1: Project scaffolding

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`

- [ ] **Step 1: Initialize Cargo project**

```toml
# Cargo.toml
[package]
name = "task-orchestrator"
version = "0.1.0"
edition = "2021"

[dependencies]
ratatui = "0.29"
crossterm = "0.28"
tokio = { version = "1", features = ["full"] }
rusqlite = { version = "0.32", features = ["bundled"] }
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
axum = "0.8"
chrono = { version = "0.4", features = ["serde"] }
anyhow = "1"
```

- [ ] **Step 2: Write minimal main.rs with clap subcommands**

```rust
// src/main.rs
mod models;
mod db;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "task-orchestrator", about = "Terminal kanban for AI agent dispatch")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Launch the TUI kanban board
    Tui {
        /// SQLite database path
        #[arg(long, env = "TASK_ORCHESTRATOR_DB")]
        db: Option<String>,
        /// MCP server port
        #[arg(long, env = "TASK_ORCHESTRATOR_PORT", default_value = "3142")]
        port: u16,
    },
    /// Update a task's status (agent CLI fallback)
    Update {
        /// Task ID
        id: i64,
        /// New status: backlog, ready, running, review, done
        status: String,
        /// SQLite database path
        #[arg(long, env = "TASK_ORCHESTRATOR_DB")]
        db: Option<String>,
    },
    /// List all tasks
    List {
        /// SQLite database path
        #[arg(long, env = "TASK_ORCHESTRATOR_DB")]
        db: Option<String>,
        /// Filter by status
        #[arg(long)]
        status: Option<String>,
    },
}

fn default_db_path() -> String {
    let base = std::env::var("XDG_DATA_HOME")
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            format!("{home}/.local/share")
        });
    let dir = format!("{base}/task-orchestrator");
    std::fs::create_dir_all(&dir).ok();
    format!("{dir}/tasks.db")
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Tui { db, port } => {
            let db_path = db.unwrap_or_else(default_db_path);
            println!("TUI mode not yet implemented. DB: {db_path}, Port: {port}");
        }
        Commands::Update { id, status, db } => {
            let db_path = db.unwrap_or_else(default_db_path);
            let db = db::Database::open(&db_path)?;
            let status = models::TaskStatus::from_str(&status)
                .ok_or_else(|| anyhow::anyhow!("invalid status: {status}"))?;
            db.update_status(id, status)?;
            println!("Task {id} updated to {}", status.as_str());
        }
        Commands::List { db, status } => {
            let db_path = db.unwrap_or_else(default_db_path);
            let db = db::Database::open(&db_path)?;
            let tasks = if let Some(s) = status {
                let status = models::TaskStatus::from_str(&s)
                    .ok_or_else(|| anyhow::anyhow!("invalid status: {s}"))?;
                db.list_by_status(status)?
            } else {
                db.list_all()?
            };
            for task in &tasks {
                println!("[{}] #{} {}", task.status.as_str(), task.id, task.title);
            }
            if tasks.is_empty() {
                println!("No tasks found.");
            }
        }
    }

    Ok(())
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check`
Expected: compiles (with unresolved module errors — that's fine, we add models/db next)

- [ ] **Step 4: Commit**

```
git add Cargo.toml src/main.rs
git commit -m "feat: project scaffolding with clap CLI"
```

---

### Task 2: Models — Task, TaskStatus, Note, slugify

**Files:**
- Create: `src/models.rs`

- [ ] **Step 1: Write tests for TaskStatus**

```rust
// src/models.rs — tests section at bottom
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_from_str_roundtrip() {
        for status in TaskStatus::ALL {
            assert_eq!(TaskStatus::from_str(status.as_str()), Some(*status));
        }
    }

    #[test]
    fn status_from_str_invalid() {
        assert_eq!(TaskStatus::from_str("garbage"), None);
    }

    #[test]
    fn status_next() {
        assert_eq!(TaskStatus::Backlog.next(), TaskStatus::Ready);
        assert_eq!(TaskStatus::Ready.next(), TaskStatus::Running);
        assert_eq!(TaskStatus::Running.next(), TaskStatus::Review);
        assert_eq!(TaskStatus::Review.next(), TaskStatus::Done);
        assert_eq!(TaskStatus::Done.next(), TaskStatus::Done);
    }

    #[test]
    fn status_prev() {
        assert_eq!(TaskStatus::Backlog.prev(), TaskStatus::Backlog);
        assert_eq!(TaskStatus::Ready.prev(), TaskStatus::Backlog);
        assert_eq!(TaskStatus::Running.prev(), TaskStatus::Ready);
        assert_eq!(TaskStatus::Review.prev(), TaskStatus::Running);
        assert_eq!(TaskStatus::Done.prev(), TaskStatus::Review);
    }

    #[test]
    fn status_column_index_roundtrip() {
        for (i, status) in TaskStatus::ALL.iter().enumerate() {
            assert_eq!(status.column_index(), i);
            assert_eq!(TaskStatus::from_column_index(i), Some(*status));
        }
    }

    #[test]
    fn slugify_normal() {
        assert_eq!(slugify("refactor config"), "refactor-config");
    }

    #[test]
    fn slugify_special_chars() {
        assert_eq!(slugify("Fix: auth & SSO!!"), "fix-auth-sso");
    }

    #[test]
    fn slugify_empty() {
        assert_eq!(slugify(""), "task");
    }

    #[test]
    fn slugify_only_special() {
        assert_eq!(slugify("!!!"), "task");
    }

    #[test]
    fn slugify_collapses_dashes() {
        assert_eq!(slugify("a   b   c"), "a-b-c");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib models`
Expected: FAIL — types don't exist yet

- [ ] **Step 3: Implement models**

```rust
// src/models.rs
use chrono::{DateTime, Utc};

#[derive(Debug, Clone)]
pub struct Task {
    pub id: i64,
    pub title: String,
    pub description: String,
    pub repo_path: String,
    pub status: TaskStatus,
    pub worktree: Option<String>,
    pub tmux_window: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Backlog,
    Ready,
    Running,
    Review,
    Done,
}

impl TaskStatus {
    pub const ALL: &[TaskStatus] = &[
        Self::Backlog,
        Self::Ready,
        Self::Running,
        Self::Review,
        Self::Done,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Backlog => "backlog",
            Self::Ready => "ready",
            Self::Running => "running",
            Self::Review => "review",
            Self::Done => "done",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "backlog" => Some(Self::Backlog),
            "ready" => Some(Self::Ready),
            "running" => Some(Self::Running),
            "review" => Some(Self::Review),
            "done" => Some(Self::Done),
            _ => None,
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Backlog => Self::Ready,
            Self::Ready => Self::Running,
            Self::Running => Self::Review,
            Self::Review => Self::Done,
            Self::Done => Self::Done,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Backlog => Self::Backlog,
            Self::Ready => Self::Backlog,
            Self::Running => Self::Ready,
            Self::Review => Self::Running,
            Self::Done => Self::Review,
        }
    }

    pub fn column_index(&self) -> usize {
        match self {
            Self::Backlog => 0,
            Self::Ready => 1,
            Self::Running => 2,
            Self::Review => 3,
            Self::Done => 4,
        }
    }

    pub fn from_column_index(i: usize) -> Option<Self> {
        Self::ALL.get(i).copied()
    }
}

#[derive(Debug, Clone)]
pub struct Note {
    pub id: i64,
    pub task_id: i64,
    pub content: String,
    pub source: NoteSource,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteSource {
    User,
    Agent,
    System,
}

impl NoteSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Agent => "agent",
            Self::System => "system",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "user" => Some(Self::User),
            "agent" => Some(Self::Agent),
            "system" => Some(Self::System),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DispatchResult {
    pub worktree_path: String,
    pub tmux_window: String,
}

pub fn slugify(title: &str) -> String {
    let slug: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();

    let mut result = String::new();
    let mut prev_dash = false;
    for c in slug.chars() {
        if c == '-' {
            if !prev_dash && !result.is_empty() {
                result.push('-');
            }
            prev_dash = true;
        } else {
            result.push(c);
            prev_dash = false;
        }
    }
    if result.ends_with('-') {
        result.pop();
    }
    if result.is_empty() {
        "task".to_string()
    } else {
        result
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib models`
Expected: all pass

- [ ] **Step 5: Commit**

```
git add src/models.rs
git commit -m "feat: Task, TaskStatus, Note models with slugify"
```

---

### Task 3: Database — SQLite schema and CRUD

**Files:**
- Create: `src/db.rs`

- [ ] **Step 1: Write tests for database operations**

```rust
// src/db.rs — tests section
#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Database {
        Database::open(":memory:").unwrap()
    }

    #[test]
    fn create_and_get() {
        let db = test_db();
        let id = db.create_task("test task", "description", "/tmp/repo").unwrap();
        let task = db.get_task(id).unwrap().unwrap();
        assert_eq!(task.title, "test task");
        assert_eq!(task.description, "description");
        assert_eq!(task.repo_path, "/tmp/repo");
        assert_eq!(task.status, TaskStatus::Backlog);
    }

    #[test]
    fn list_all() {
        let db = test_db();
        db.create_task("a", "", "/tmp").unwrap();
        db.create_task("b", "", "/tmp").unwrap();
        assert_eq!(db.list_all().unwrap().len(), 2);
    }

    #[test]
    fn list_by_status() {
        let db = test_db();
        let id = db.create_task("a", "", "/tmp").unwrap();
        db.update_status(id, TaskStatus::Ready).unwrap();
        db.create_task("b", "", "/tmp").unwrap();
        assert_eq!(db.list_by_status(TaskStatus::Ready).unwrap().len(), 1);
        assert_eq!(db.list_by_status(TaskStatus::Backlog).unwrap().len(), 1);
    }

    #[test]
    fn update_status() {
        let db = test_db();
        let id = db.create_task("test", "", "/tmp").unwrap();
        db.update_status(id, TaskStatus::Running).unwrap();
        let task = db.get_task(id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Running);
    }

    #[test]
    fn update_status_nonexistent() {
        let db = test_db();
        assert!(db.update_status(999, TaskStatus::Done).is_err());
    }

    #[test]
    fn update_dispatch_fields() {
        let db = test_db();
        let id = db.create_task("test", "", "/tmp").unwrap();
        db.update_dispatch(id, "/tmp/.worktrees/test", "task-1").unwrap();
        let task = db.get_task(id).unwrap().unwrap();
        assert_eq!(task.worktree.as_deref(), Some("/tmp/.worktrees/test"));
        assert_eq!(task.tmux_window.as_deref(), Some("task-1"));
        assert_eq!(task.status, TaskStatus::Running);
    }

    #[test]
    fn get_nonexistent() {
        let db = test_db();
        assert!(db.get_task(999).unwrap().is_none());
    }

    #[test]
    fn add_and_list_notes() {
        let db = test_db();
        let id = db.create_task("test", "", "/tmp").unwrap();
        db.add_note(id, "first note", NoteSource::User).unwrap();
        db.add_note(id, "second note", NoteSource::Agent).unwrap();
        let notes = db.list_notes(id).unwrap();
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].content, "first note");
        assert_eq!(notes[1].source, NoteSource::Agent);
    }

    #[test]
    fn delete_task_cascades_notes() {
        let db = test_db();
        let id = db.create_task("test", "", "/tmp").unwrap();
        db.add_note(id, "note", NoteSource::User).unwrap();
        db.delete_task(id).unwrap();
        assert!(db.get_task(id).unwrap().is_none());
        assert!(db.list_notes(id).unwrap().is_empty());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib db`
Expected: FAIL — Database struct doesn't exist

- [ ] **Step 3: Implement Database**

```rust
// src/db.rs
use anyhow::{bail, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use crate::models::{Note, NoteSource, Task, TaskStatus};

pub struct Database {
    conn: std::sync::Mutex<Connection>,
}

impl Database {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA foreign_keys=ON;
             PRAGMA busy_timeout=5000;

             CREATE TABLE IF NOT EXISTS tasks (
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
                 id         INTEGER PRIMARY KEY,
                 task_id    INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                 content    TEXT NOT NULL,
                 source     TEXT NOT NULL DEFAULT 'user',
                 created_at TEXT NOT NULL DEFAULT (datetime('now'))
             );"
        )?;
        Ok(Self { conn: std::sync::Mutex::new(conn) })
    }

    pub fn create_task(&self, title: &str, description: &str, repo_path: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO tasks (title, description, repo_path) VALUES (?1, ?2, ?3)",
            params![title, description, repo_path],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn get_task(&self, id: i64) -> Result<Option<Task>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
                "SELECT id, title, description, repo_path, status, worktree, tmux_window, created_at, updated_at
                 FROM tasks WHERE id = ?1",
                params![id],
                |row| Ok(row_to_task(row)),
            )
            .optional()?
            .transpose()
    }

    pub fn list_all(&self) -> Result<Vec<Task>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, title, description, repo_path, status, worktree, tmux_window, created_at, updated_at
             FROM tasks ORDER BY created_at ASC"
        )?;
        let rows = stmt.query_map([], |row| Ok(row_to_task(row)))?;
        rows.map(|r| r?.map_err(Into::into)).collect()
    }

    pub fn list_by_status(&self, status: TaskStatus) -> Result<Vec<Task>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, title, description, repo_path, status, worktree, tmux_window, created_at, updated_at
             FROM tasks WHERE status = ?1 ORDER BY created_at ASC"
        )?;
        let rows = stmt.query_map(params![status.as_str()], |row| Ok(row_to_task(row)))?;
        rows.map(|r| r?.map_err(Into::into)).collect()
    }

    pub fn update_status(&self, id: i64, status: TaskStatus) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE tasks SET status = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![status.as_str(), id],
        )?;
        if changed == 0 {
            bail!("task {id} not found");
        }
        Ok(())
    }

    pub fn update_dispatch(&self, id: i64, worktree: &str, tmux_window: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE tasks SET worktree = ?1, tmux_window = ?2, status = 'running', updated_at = datetime('now') WHERE id = ?3",
            params![worktree, tmux_window, id],
        )?;
        Ok(())
    }

    pub fn delete_task(&self, id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM tasks WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn add_note(&self, task_id: i64, content: &str, source: NoteSource) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO notes (task_id, content, source) VALUES (?1, ?2, ?3)",
            params![task_id, content, source.as_str()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn list_notes(&self, task_id: i64) -> Result<Vec<Note>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, task_id, content, source, created_at FROM notes WHERE task_id = ?1 ORDER BY created_at ASC"
        )?;
        let rows = stmt.query_map(params![task_id], |row| Ok(row_to_note(row)))?;
        rows.map(|r| r?.map_err(Into::into)).collect()
    }
}

fn row_to_task(row: &rusqlite::Row) -> Result<Task> {
    let status_str: String = row.get(4)?;
    let created_str: String = row.get(7)?;
    let updated_str: String = row.get(8)?;
    Ok(Task {
        id: row.get(0)?,
        title: row.get(1)?,
        description: row.get(2)?,
        repo_path: row.get(3)?,
        status: TaskStatus::from_str(&status_str)
            .ok_or_else(|| anyhow::anyhow!("invalid status: {status_str}"))?,
        worktree: row.get(5)?,
        tmux_window: row.get(6)?,
        created_at: parse_datetime(&created_str)?,
        updated_at: parse_datetime(&updated_str)?,
    })
}

fn row_to_note(row: &rusqlite::Row) -> Result<Note> {
    let source_str: String = row.get(3)?;
    let created_str: String = row.get(4)?;
    Ok(Note {
        id: row.get(0)?,
        task_id: row.get(1)?,
        content: row.get(2)?,
        source: NoteSource::from_str(&source_str)
            .ok_or_else(|| anyhow::anyhow!("invalid source: {source_str}"))?,
        created_at: parse_datetime(&created_str)?,
    })
}

fn parse_datetime(s: &str) -> Result<DateTime<Utc>> {
    // SQLite datetime('now') produces "YYYY-MM-DD HH:MM:SS"
    Ok(chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")?
        .and_utc())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib db`
Expected: all pass

- [ ] **Step 5: Commit**

```
git add src/db.rs
git commit -m "feat: SQLite database with tasks and notes CRUD"
```

---

### Task 4: Wire CLI subcommands to database

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Update main.rs to use real db module**

Wire the `update` and `list` subcommands to `db::Database` as shown in the scaffolding (Task 1 Step 2 already has the code structure). Ensure `mod models; mod db;` are declared.

- [ ] **Step 2: Verify CLI works end-to-end**

Run: `cargo run -- list --db /tmp/test-orch.db`
Expected: "No tasks found."

Run: `cargo run -- update 1 ready --db /tmp/test-orch.db`
Expected: error "task 1 not found" (no tasks yet — proves the pipeline works)

- [ ] **Step 3: Commit**

```
git add src/main.rs
git commit -m "feat: wire list and update CLI subcommands"
```

---

## Phase 2: TUI

### Task 5: TUI App state and Elm architecture

**Files:**
- Create: `src/tui/mod.rs`

- [ ] **Step 1: Write tests for app state and update logic**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::TaskStatus;

    fn sample_tasks() -> Vec<Task> {
        // Create tasks via db would give real IDs; for unit tests, construct directly
        vec![
            Task { id: 1, title: "backlog-task".into(), description: "".into(),
                   repo_path: "/tmp".into(), status: TaskStatus::Backlog,
                   worktree: None, tmux_window: None,
                   created_at: Utc::now(), updated_at: Utc::now() },
            Task { id: 2, title: "ready-task".into(), description: "".into(),
                   repo_path: "/tmp".into(), status: TaskStatus::Ready,
                   worktree: None, tmux_window: None,
                   created_at: Utc::now(), updated_at: Utc::now() },
            Task { id: 3, title: "running-task".into(), description: "".into(),
                   repo_path: "/tmp".into(), status: TaskStatus::Running,
                   worktree: Some("/tmp/.worktrees/test".into()),
                   tmux_window: Some("task-3".into()),
                   created_at: Utc::now(), updated_at: Utc::now() },
        ]
    }

    #[test]
    fn tasks_by_status_filters() {
        let app = App::new(sample_tasks());
        assert_eq!(app.tasks_by_status(TaskStatus::Backlog).len(), 1);
        assert_eq!(app.tasks_by_status(TaskStatus::Ready).len(), 1);
        assert_eq!(app.tasks_by_status(TaskStatus::Running).len(), 1);
        assert_eq!(app.tasks_by_status(TaskStatus::Review).len(), 0);
    }

    #[test]
    fn move_task_forward() {
        let mut app = App::new(sample_tasks());
        let cmds = app.update(Message::MoveTask { id: 1, direction: MoveDirection::Forward });
        assert_eq!(app.tasks[0].status, TaskStatus::Ready);
        assert!(matches!(cmds[0], Command::PersistTask { .. }));
    }

    #[test]
    fn move_task_backward_at_start_is_noop() {
        let mut app = App::new(sample_tasks());
        let cmds = app.update(Message::MoveTask { id: 1, direction: MoveDirection::Backward });
        assert_eq!(app.tasks[0].status, TaskStatus::Backlog);
    }

    #[test]
    fn dispatch_only_ready_tasks() {
        let mut app = App::new(sample_tasks());
        // Task 1 is Backlog — should not dispatch
        let cmds = app.update(Message::DispatchTask(1));
        assert!(cmds.iter().all(|c| matches!(c, Command::None)));
        // Task 2 is Ready — should dispatch
        let cmds = app.update(Message::DispatchTask(2));
        assert!(cmds.iter().any(|c| matches!(c, Command::Dispatch { .. })));
    }

    #[test]
    fn quit_sets_flag() {
        let mut app = App::new(vec![]);
        app.update(Message::Quit);
        assert!(app.should_quit);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib tui`
Expected: FAIL

- [ ] **Step 3: Implement App state and update**

```rust
// src/tui/mod.rs
pub mod ui;
pub mod input;

use chrono::Utc;
use crate::models::{Task, TaskStatus};

// ── Messages ──────────────────────────────────────────────────────

#[derive(Debug)]
pub enum Message {
    Tick,
    Quit,
    NavigateColumn(isize),   // -1 left, +1 right
    NavigateRow(isize),      // -1 up, +1 down
    MoveTask { id: i64, direction: MoveDirection },
    DispatchTask(i64),
    Dispatched { id: i64, worktree: String, tmux_window: String },
    CreateTask { title: String, description: String, repo_path: String },
    DeleteTask(i64),
    ToggleDetail,
    TmuxOutput { id: i64, output: String },
    Error(String),
}

#[derive(Debug, Clone, Copy)]
pub enum MoveDirection {
    Forward,
    Backward,
}

// ── Commands ──────────────────────────────────────────────────────

#[derive(Debug)]
pub enum Command {
    PersistTask(Task),
    DeleteTask(i64),
    Dispatch { task: Task },
    CaptureTmux { id: i64, window: String },
    None,
}

// ── Input Mode ────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum InputMode {
    Normal,
    InputTitle,
    InputDescription { title: String },
    InputRepoPath { title: String, description: String },
    ConfirmDelete,
}

// ── App State ─────────────────────────────────────────────────────

pub struct App {
    pub tasks: Vec<Task>,
    pub selected_column: usize,    // 0..5
    pub selected_row: [usize; 5],  // per-column cursor
    pub mode: InputMode,
    pub input_buffer: String,
    pub detail_visible: bool,
    pub detail_text: Option<String>,
    pub tmux_outputs: std::collections::HashMap<i64, String>,
    pub status_message: Option<String>,
    pub should_quit: bool,
}

impl App {
    pub fn new(tasks: Vec<Task>) -> Self {
        Self {
            tasks,
            selected_column: 0,
            selected_row: [0; 5],
            mode: InputMode::Normal,
            input_buffer: String::new(),
            detail_visible: false,
            detail_text: None,
            tmux_outputs: std::collections::HashMap::new(),
            status_message: None,
            should_quit: false,
        }
    }

    pub fn tasks_by_status(&self, status: TaskStatus) -> Vec<&Task> {
        self.tasks.iter().filter(|t| t.status == status).collect()
    }

    pub fn selected_task(&self) -> Option<&Task> {
        let status = TaskStatus::from_column_index(self.selected_column)?;
        let tasks = self.tasks_by_status(status);
        tasks.get(self.selected_row[self.selected_column]).copied()
    }

    pub fn update(&mut self, msg: Message) -> Vec<Command> {
        match msg {
            Message::Quit => {
                self.should_quit = true;
                vec![Command::None]
            }
            Message::NavigateColumn(delta) => {
                let new = self.selected_column as isize + delta;
                self.selected_column = new.clamp(0, 4) as usize;
                self.clamp_selection();
                self.detail_text = None;
                vec![Command::None]
            }
            Message::NavigateRow(delta) => {
                let status = TaskStatus::from_column_index(self.selected_column);
                if let Some(status) = status {
                    let count = self.tasks_by_status(status).len();
                    if count > 0 {
                        let row = self.selected_row[self.selected_column] as isize + delta;
                        self.selected_row[self.selected_column] = row.clamp(0, count as isize - 1) as usize;
                    }
                }
                self.detail_text = None;
                vec![Command::None]
            }
            Message::MoveTask { id, direction } => {
                if let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) {
                    let new_status = match direction {
                        MoveDirection::Forward => task.status.next(),
                        MoveDirection::Backward => task.status.prev(),
                    };
                    if new_status != task.status {
                        task.status = new_status;
                        task.updated_at = Utc::now();
                        return vec![Command::PersistTask(task.clone())];
                    }
                }
                vec![Command::None]
            }
            Message::DispatchTask(id) => {
                if let Some(task) = self.tasks.iter().find(|t| t.id == id) {
                    if task.status == TaskStatus::Ready {
                        return vec![Command::Dispatch { task: task.clone() }];
                    }
                    self.status_message = Some("Can only dispatch Ready tasks".into());
                }
                vec![Command::None]
            }
            Message::Dispatched { id, worktree, tmux_window } => {
                if let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) {
                    task.status = TaskStatus::Running;
                    task.worktree = Some(worktree);
                    task.tmux_window = Some(tmux_window);
                    task.updated_at = Utc::now();
                    return vec![Command::PersistTask(task.clone())];
                }
                vec![Command::None]
            }
            Message::CreateTask { title, description, repo_path } => {
                // ID will be set by database; use 0 as placeholder
                let task = Task {
                    id: 0, title, description, repo_path,
                    status: TaskStatus::Backlog,
                    worktree: None, tmux_window: None,
                    created_at: Utc::now(), updated_at: Utc::now(),
                };
                let cmd = Command::PersistTask(task.clone());
                self.tasks.push(task);
                self.mode = InputMode::Normal;
                self.input_buffer.clear();
                vec![cmd]
            }
            Message::DeleteTask(id) => {
                self.tasks.retain(|t| t.id != id);
                self.mode = InputMode::Normal;
                self.clamp_selection();
                vec![Command::DeleteTask(id)]
            }
            Message::Tick => {
                // Capture tmux output for all Running tasks
                let cmds: Vec<Command> = self.tasks.iter()
                    .filter(|t| t.status == TaskStatus::Running)
                    .filter_map(|t| t.tmux_window.as_ref().map(|w| Command::CaptureTmux {
                        id: t.id, window: w.clone()
                    }))
                    .collect();
                if cmds.is_empty() { vec![Command::None] } else { cmds }
            }
            Message::TmuxOutput { id, output } => {
                self.tmux_outputs.insert(id, output);
                vec![Command::None]
            }
            Message::ToggleDetail => {
                self.detail_visible = !self.detail_visible;
                vec![Command::None]
            }
            Message::Error(msg) => {
                self.status_message = Some(msg);
                vec![Command::None]
            }
        }
    }

    fn clamp_selection(&mut self) {
        for col in 0..5 {
            if let Some(status) = TaskStatus::from_column_index(col) {
                let count = self.tasks_by_status(status).len();
                if count == 0 {
                    self.selected_row[col] = 0;
                } else if self.selected_row[col] >= count {
                    self.selected_row[col] = count - 1;
                }
            }
        }
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test --lib tui`
Expected: all pass

- [ ] **Step 5: Commit**

```
git add src/tui/mod.rs
git commit -m "feat: TUI app state with Elm architecture"
```

---

### Task 6: TUI key input handling

**Files:**
- Create: `src/tui/input.rs`

- [ ] **Step 1: Implement key event to message mapping**

```rust
// src/tui/input.rs
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use super::{App, InputMode, Message, MoveDirection};

impl App {
    pub fn handle_key(&mut self, key: KeyEvent) -> Vec<super::Command> {
        match &self.mode {
            InputMode::Normal => self.handle_normal_key(key),
            InputMode::InputTitle | InputMode::InputDescription { .. } | InputMode::InputRepoPath { .. } => {
                self.handle_text_input(key)
            }
            InputMode::ConfirmDelete => self.handle_confirm_delete(key),
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) -> Vec<super::Command> {
        match (key.code, key.modifiers) {
            (KeyCode::Char('q'), _) => self.update(Message::Quit),
            (KeyCode::Char('h') | KeyCode::Left, _) => self.update(Message::NavigateColumn(-1)),
            (KeyCode::Char('l') | KeyCode::Right, _) => self.update(Message::NavigateColumn(1)),
            (KeyCode::Char('j') | KeyCode::Down, _) => self.update(Message::NavigateRow(1)),
            (KeyCode::Char('k') | KeyCode::Up, _) => self.update(Message::NavigateRow(-1)),
            (KeyCode::Char('n'), _) => {
                self.mode = InputMode::InputTitle;
                self.input_buffer.clear();
                self.status_message = Some("New task — enter title:".into());
                vec![super::Command::None]
            }
            (KeyCode::Char('d'), _) => {
                if let Some(task) = self.selected_task() {
                    let id = task.id;
                    self.update(Message::DispatchTask(id))
                } else {
                    vec![super::Command::None]
                }
            }
            (KeyCode::Char('m'), _) => {
                if let Some(task) = self.selected_task() {
                    let id = task.id;
                    self.update(Message::MoveTask { id, direction: MoveDirection::Forward })
                } else {
                    vec![super::Command::None]
                }
            }
            (KeyCode::Char('M'), KeyModifiers::SHIFT) => {
                if let Some(task) = self.selected_task() {
                    let id = task.id;
                    self.update(Message::MoveTask { id, direction: MoveDirection::Backward })
                } else {
                    vec![super::Command::None]
                }
            }
            (KeyCode::Enter, _) => self.update(Message::ToggleDetail),
            _ => vec![super::Command::None],
        }
    }

    fn handle_text_input(&mut self, key: KeyEvent) -> Vec<super::Command> {
        match key.code {
            KeyCode::Esc => {
                self.mode = InputMode::Normal;
                self.input_buffer.clear();
                self.status_message = None;
                vec![super::Command::None]
            }
            KeyCode::Enter => {
                let text = self.input_buffer.trim().to_string();
                if text.is_empty() {
                    return vec![super::Command::None];
                }
                match self.mode.clone() {
                    InputMode::InputTitle => {
                        self.mode = InputMode::InputDescription { title: text };
                        self.input_buffer.clear();
                        self.status_message = Some("Enter description:".into());
                        vec![super::Command::None]
                    }
                    InputMode::InputDescription { title } => {
                        self.mode = InputMode::InputRepoPath { title, description: text };
                        self.input_buffer.clear();
                        self.status_message = Some("Enter repo path:".into());
                        vec![super::Command::None]
                    }
                    InputMode::InputRepoPath { title, description } => {
                        self.update(Message::CreateTask { title, description, repo_path: text })
                    }
                    _ => vec![super::Command::None],
                }
            }
            KeyCode::Backspace => {
                self.input_buffer.pop();
                vec![super::Command::None]
            }
            KeyCode::Char(c) => {
                self.input_buffer.push(c);
                vec![super::Command::None]
            }
            _ => vec![super::Command::None],
        }
    }

    fn handle_confirm_delete(&mut self, key: KeyEvent) -> Vec<super::Command> {
        match key.code {
            KeyCode::Char('y') => {
                if let Some(task) = self.selected_task() {
                    let id = task.id;
                    self.status_message = None;
                    self.update(Message::DeleteTask(id))
                } else {
                    self.mode = InputMode::Normal;
                    vec![super::Command::None]
                }
            }
            _ => {
                self.mode = InputMode::Normal;
                self.status_message = None;
                vec![super::Command::None]
            }
        }
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check`
Expected: compiles

- [ ] **Step 3: Commit**

```
git add src/tui/input.rs
git commit -m "feat: key event handling for TUI"
```

---

### Task 7: TUI rendering — kanban board

**Files:**
- Create: `src/tui/ui.rs`

- [ ] **Step 1: Implement kanban board rendering**

```rust
// src/tui/ui.rs
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    Frame,
};
use crate::models::TaskStatus;
use super::App;

const COLUMN_NAMES: [&str; 5] = ["Backlog", "Ready", "Running", "Review", "Done"];
const COLUMN_COLORS: [Color; 5] = [
    Color::DarkGray,  // Backlog
    Color::Blue,      // Ready
    Color::Yellow,    // Running
    Color::Magenta,   // Review
    Color::Green,     // Done
];

pub fn render(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(6),    // kanban columns
            Constraint::Length(8), // detail panel
            Constraint::Length(1), // status bar
        ])
        .split(frame.area());

    render_columns(frame, app, chunks[0]);
    render_detail(frame, app, chunks[1]);
    render_status_bar(frame, app, chunks[2]);
}

fn render_columns(frame: &mut Frame, app: &App, area: Rect) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(20); 5])
        .split(area);

    for (col_idx, status) in TaskStatus::ALL.iter().enumerate() {
        let tasks = app.tasks_by_status(*status);
        let is_focused = app.selected_column == col_idx;
        let selected_row = app.selected_row[col_idx];

        let items: Vec<ListItem> = tasks.iter().enumerate().map(|(i, task)| {
            let mut lines = vec![Span::raw(&task.title)];

            // Running tasks show last line of tmux output
            if *status == TaskStatus::Running {
                if let Some(output) = app.tmux_outputs.get(&task.id) {
                    if let Some(last_line) = output.lines().last() {
                        lines.push(Span::styled(
                            format!(" > {}", truncate(last_line, 30)),
                            Style::default().fg(Color::DarkGray),
                        ));
                    }
                }
            }

            let style = if is_focused && i == selected_row {
                Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            ListItem::new(Line::from(lines)).style(style)
        }).collect();

        let count = tasks.len();
        let border_color = if is_focused { COLUMN_COLORS[col_idx] } else { Color::DarkGray };
        let title_style = if is_focused {
            Style::default().fg(COLUMN_COLORS[col_idx]).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };

        let block = Block::default()
            .title(format!("{} ({})", COLUMN_NAMES[col_idx], count))
            .title_style(title_style)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));

        frame.render_widget(List::new(items).block(block), columns[col_idx]);
    }
}

fn render_detail(frame: &mut Frame, app: &App, area: Rect) {
    let text = if app.detail_visible {
        match app.selected_task() {
            Some(task) => {
                let mut lines = vec![
                    format!("Task #{}: {}", task.id, task.title),
                    format!("Repo: {}", task.repo_path),
                    format!("Status: {}", task.status.as_str()),
                ];
                if !task.description.is_empty() {
                    lines.push(format!("Description: {}", task.description));
                }
                if let Some(wt) = &task.worktree {
                    lines.push(format!("Worktree: {}", wt));
                }
                if let Some(win) = &task.tmux_window {
                    lines.push(format!("tmux window: {}", win));
                }
                lines.join("\n")
            }
            None => "No task selected".into(),
        }
    } else {
        "Press Enter to show detail".into()
    };

    let block = Block::default()
        .title("Detail")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    frame.render_widget(Paragraph::new(text).block(block).wrap(Wrap { trim: false }), area);
}

fn render_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let text = match &app.mode {
        super::InputMode::InputTitle => format!("Title: {}_", app.input_buffer),
        super::InputMode::InputDescription { title } => format!("[{}] Description: {}_", title, app.input_buffer),
        super::InputMode::InputRepoPath { title, .. } => format!("[{}] Repo path: {}_", title, app.input_buffer),
        super::InputMode::ConfirmDelete => "Delete this task? [y/n]".into(),
        super::InputMode::Normal => app.status_message.clone()
            .unwrap_or_else(|| "[n]ew [d]ispatch [m/M]ove [Enter]detail [q]uit".into()),
    };

    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(Color::White).bg(Color::DarkGray)),
        area,
    );
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max { s.to_string() } else { format!("{}...", &s[..max]) }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check`
Expected: compiles

- [ ] **Step 3: Commit**

```
git add src/tui/ui.rs
git commit -m "feat: kanban board rendering with 5 columns and detail panel"
```

---

### Task 8: TUI main loop — wire everything together

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Implement the TUI main loop**

Add the `tui` module declaration and implement the `run_tui` function in `main.rs`. This wires together: terminal setup, event loop with `tokio::select!`, crossterm key events, tick timer, command execution (persist to DB, spawn tmux capture, dispatch agents).

Key structure:
```rust
async fn run_tui(db_path: &str, port: u16) -> Result<()> {
    let db = db::Database::open(db_path)?;
    let tasks = db.list_all()?;
    let mut app = tui::App::new(tasks);

    // Terminal setup (enable_raw_mode, EnterAlternateScreen)
    // ...

    let (tx, mut rx) = tokio::sync::mpsc::channel(100);
    let mut tick = tokio::time::interval(Duration::from_secs(2));

    loop {
        terminal.draw(|f| tui::ui::render(f, &app))?;

        tokio::select! {
            // crossterm key events
            // channel messages (tmux output, dispatch results)
            // tick timer
        }

        // execute commands from app.update() / app.handle_key()

        if app.should_quit { break; }
    }

    // Terminal cleanup
}
```

- [ ] **Step 2: Test manually**

Run: `cargo run -- tui --db /tmp/test-orch.db`
Expected: TUI launches with empty kanban board. Press `n` to create a task, `q` to quit.

- [ ] **Step 3: Commit**

```
git add src/main.rs
git commit -m "feat: TUI main loop with event handling and DB persistence"
```

---

## Phase 3: Agent Dispatch

### Task 9: tmux subprocess wrappers

**Files:**
- Create: `src/tmux.rs`

- [ ] **Step 1: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_window_command_is_correct() {
        // Test that we build the right command args
        let args = new_window_args("task-1", "/tmp/worktree");
        assert_eq!(args, vec!["new-window", "-n", "task-1", "-c", "/tmp/worktree"]);
    }

    #[test]
    fn capture_pane_args_correct() {
        let args = capture_pane_args("task-1", 5);
        let expected: Vec<String> = vec!["capture-pane", "-t", "task-1", "-p", "-S", "-5"]
            .into_iter().map(String::from).collect();
        assert_eq!(args, expected);
    }

    #[test]
    fn has_window_args_correct() {
        let args = has_window_args("task-1");
        let expected: Vec<String> = vec!["list-windows", "-F", "#{window_name}"]
            .into_iter().map(String::from).collect();
        assert_eq!(args, expected);
    }
}
```

- [ ] **Step 2: Implement tmux wrappers**

```rust
// src/tmux.rs
use anyhow::Result;
use std::process::Command;

pub fn new_window(name: &str, working_dir: &str) -> Result<()> {
    let args = new_window_args(name, working_dir);
    let status = Command::new("tmux").args(&args).status()?;
    if !status.success() {
        anyhow::bail!("tmux new-window failed");
    }
    Ok(())
}

pub fn send_keys(window: &str, keys: &str) -> Result<()> {
    let status = Command::new("tmux")
        .args(["send-keys", "-t", window, keys, "Enter"])
        .status()?;
    if !status.success() {
        anyhow::bail!("tmux send-keys failed");
    }
    Ok(())
}

pub fn capture_pane(window: &str, lines: usize) -> Result<String> {
    let args = capture_pane_args(window, lines);
    let output = Command::new("tmux").args(&args).output()?;
    Ok(String::from_utf8_lossy(&output.stdout).trim_end().to_string())
}

pub fn has_window(window: &str) -> Result<bool> {
    let output = Command::new("tmux")
        .args(["list-windows", "-F", "#{window_name}"])
        .output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.lines().any(|line| line.trim() == window))
}

pub fn kill_window(window: &str) -> Result<()> {
    Command::new("tmux")
        .args(["kill-window", "-t", window])
        .status()?;
    Ok(())
}

// Testable arg builders
fn new_window_args(name: &str, working_dir: &str) -> Vec<&str> {
    vec!["new-window", "-n", name, "-c", working_dir]
}

fn capture_pane_args(window: &str, lines: usize) -> Vec<String> {
    vec![
        "capture-pane".into(), "-t".into(), window.into(),
        "-p".into(), "-S".into(), format!("-{lines}"),
    ]
}

fn has_window_args(window: &str) -> Vec<String> {
    vec!["list-windows".into(), "-F".into(), "#{window_name}".into()]
}
```


- [ ] **Step 3: Run tests**

Run: `cargo test --lib tmux`
Expected: pass (arg builder tests only — no tmux required)

- [ ] **Step 4: Commit**

```
git add src/tmux.rs
git commit -m "feat: tmux subprocess wrappers"
```

---

### Task 10: Dispatch orchestration

**Files:**
- Create: `src/dispatch.rs`

- [ ] **Step 1: Implement dispatch flow**

```rust
// src/dispatch.rs
use anyhow::Result;
use std::fs;
use std::path::Path;
use std::process::Command;

use crate::models::{slugify, DispatchResult};
use crate::tmux;

pub fn dispatch_agent(
    task_id: i64,
    title: &str,
    description: &str,
    repo_path: &str,
    mcp_port: u16,
) -> Result<DispatchResult> {
    let slug = slugify(title);
    let worktree_name = format!("{task_id}-{slug}");
    let worktree_path = format!("{repo_path}/.worktrees/{worktree_name}");
    let tmux_window = format!("task-{task_id}");

    // 1. Create worktree
    fs::create_dir_all(format!("{repo_path}/.worktrees"))?;
    let status = Command::new("git")
        .args(["-C", repo_path, "worktree", "add", &worktree_path, "-b", &worktree_name])
        .status()?;
    if !status.success() {
        anyhow::bail!("git worktree add failed");
    }

    // 2. Write .mcp.json
    let mcp_json = serde_json::json!({
        "mcpServers": {
            "task-orchestrator": {
                "url": format!("http://localhost:{mcp_port}/mcp")
            }
        }
    });
    fs::write(
        format!("{worktree_path}/.mcp.json"),
        serde_json::to_string_pretty(&mcp_json)?,
    )?;

    // 3. Create tmux window
    tmux::new_window(&tmux_window, &worktree_path)?;

    // 4. Send claude command
    let prompt = format!(
        "You are working on task #{task_id}: {title}\n\n\
         {description}\n\n\
         When you have completed the task, call the update_task MCP tool \
         with task_id={task_id} and status=\"review\".\n\n\
         If the MCP server is unavailable, run:\n\
         task-orchestrator update {task_id} review"
    );
    tmux::send_keys(&tmux_window, &format!("claude --prompt '{}'", prompt.replace('\'', "'\\''")))?;

    Ok(DispatchResult {
        worktree_path,
        tmux_window,
    })
}

pub fn cleanup_task(repo_path: &str, worktree_path: &str, tmux_window: &str) -> Result<()> {
    // Kill tmux window if it exists
    if tmux::has_window(tmux_window)? {
        tmux::kill_window(tmux_window)?;
    }

    // Remove worktree
    if Path::new(worktree_path).exists() {
        Command::new("git")
            .args(["-C", repo_path, "worktree", "remove", worktree_path, "--force"])
            .status()?;
    }

    Ok(())
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check`
Expected: compiles

- [ ] **Step 3: Commit**

```
git add src/dispatch.rs
git commit -m "feat: agent dispatch with worktree, mcp.json, and tmux"
```

---

### Task 11: Wire dispatch and tmux capture into TUI main loop

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Handle Command::Dispatch and Command::CaptureTmux in execute_commands**

In the main loop's command executor, add:
- `Command::Dispatch { task }` → spawn `dispatch::dispatch_agent()` on a blocking thread, send `Message::Dispatched` or `Message::Error` back via channel
- `Command::CaptureTmux { id, window }` → spawn `tmux::capture_pane()` on blocking thread, send `Message::TmuxOutput` back via channel

- [ ] **Step 2: Add process exit detection in tick handler**

On each Tick, for Running tasks with a tmux_window, check `tmux::has_window()`. If the window is gone, send `Message::MoveTask { id, direction: Forward }` to move to Review, and add a system note.

- [ ] **Step 3: Test manually**

Create a task via TUI (`n`), move to Ready (`m`), dispatch (`d`). Verify:
- Worktree created in repo's `.worktrees/`
- tmux window opens with Claude running
- Running card shows live output
- When Claude finishes, task moves to Review

- [ ] **Step 4: Commit**

```
git add src/main.rs
git commit -m "feat: wire dispatch and tmux capture into TUI loop"
```

---

## Phase 4: MCP Server

### Task 12: MCP server with Axum

**Files:**
- Create: `src/mcp/mod.rs`
- Create: `src/mcp/handlers.rs`

- [ ] **Step 1: Implement MCP server**

```rust
// src/mcp/mod.rs
pub mod handlers;

use std::sync::Arc;
use axum::{Router, routing::post};
use crate::db::Database;

pub struct McpState {
    pub db: Arc<Database>,
}

pub fn router(db: Arc<Database>) -> Router {
    let state = Arc::new(McpState { db });
    Router::new()
        .route("/mcp", post(handlers::handle_mcp))
        .with_state(state)
}

pub async fn serve(db: Arc<Database>, port: u16) -> anyhow::Result<()> {
    let app = router(db);
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
```

- [ ] **Step 2: Implement MCP tool handlers**

```rust
// src/mcp/handlers.rs
// Implement Streamable HTTP MCP transport:
// - POST /mcp receives JSON-RPC requests
// - Handle "tools/list" → return tool definitions
// - Handle "tools/call" → route to update_task, add_note, get_task
// - Return JSON-RPC responses

use std::sync::Arc;
use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::models::{TaskStatus, NoteSource};
use super::McpState;

#[derive(Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    params: Option<Value>,
}

#[derive(Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: Option<Value>,
    result: Option<Value>,
    error: Option<Value>,
}

pub async fn handle_mcp(
    State(state): State<Arc<McpState>>,
    Json(req): Json<JsonRpcRequest>,
) -> Json<JsonRpcResponse> {
    let result = match req.method.as_str() {
        "tools/list" => Ok(tools_list()),
        "tools/call" => handle_tool_call(&state, req.params).await,
        _ => Err(json!({"code": -32601, "message": "method not found"})),
    };

    Json(match result {
        Ok(val) => JsonRpcResponse { jsonrpc: "2.0".into(), id: req.id, result: Some(val), error: None },
        Err(err) => JsonRpcResponse { jsonrpc: "2.0".into(), id: req.id, result: None, error: Some(err) },
    })
}

fn tools_list() -> Value {
    json!({
        "tools": [
            {
                "name": "update_task",
                "description": "Update a task's status",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "integer", "description": "Task ID" },
                        "status": { "type": "string", "enum": ["backlog", "ready", "running", "review", "done"] }
                    },
                    "required": ["task_id", "status"]
                }
            },
            {
                "name": "add_note",
                "description": "Add a note to a task",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "integer", "description": "Task ID" },
                        "note": { "type": "string", "description": "Note content" }
                    },
                    "required": ["task_id", "note"]
                }
            },
            {
                "name": "get_task",
                "description": "Get task details",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "integer", "description": "Task ID" }
                    },
                    "required": ["task_id"]
                }
            }
        ]
    })
}

async fn handle_tool_call(state: &McpState, params: Option<Value>) -> Result<Value, Value> {
    let params = params.ok_or_else(|| json!({"code": -32602, "message": "missing params"}))?;
    let tool_name = params.get("name").and_then(|v| v.as_str())
        .ok_or_else(|| json!({"code": -32602, "message": "missing tool name"}))?;
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

    match tool_name {
        "update_task" => {
            let task_id = arguments.get("task_id").and_then(|v| v.as_i64())
                .ok_or_else(|| json!({"code": -32602, "message": "missing task_id"}))?;
            let status_str = arguments.get("status").and_then(|v| v.as_str())
                .ok_or_else(|| json!({"code": -32602, "message": "missing status"}))?;
            let status = TaskStatus::from_str(status_str)
                .ok_or_else(|| json!({"code": -32602, "message": "invalid status"}))?;

            state.db.update_status(task_id, status)
                .map_err(|e| json!({"code": -32000, "message": e.to_string()}))?;

            Ok(json!({"content": [{"type": "text", "text": format!("Task {task_id} updated to {status_str}")}]}))
        }
        "add_note" => {
            let task_id = arguments.get("task_id").and_then(|v| v.as_i64())
                .ok_or_else(|| json!({"code": -32602, "message": "missing task_id"}))?;
            let note = arguments.get("note").and_then(|v| v.as_str())
                .ok_or_else(|| json!({"code": -32602, "message": "missing note"}))?;

            state.db.add_note(task_id, note, NoteSource::Agent)
                .map_err(|e| json!({"code": -32000, "message": e.to_string()}))?;

            Ok(json!({"content": [{"type": "text", "text": "Note added"}]}))
        }
        "get_task" => {
            let task_id = arguments.get("task_id").and_then(|v| v.as_i64())
                .ok_or_else(|| json!({"code": -32602, "message": "missing task_id"}))?;

            let task = state.db.get_task(task_id)
                .map_err(|e| json!({"code": -32000, "message": e.to_string()}))?
                .ok_or_else(|| json!({"code": -32000, "message": "task not found"}))?;

            Ok(json!({"content": [{"type": "text", "text": format!(
                "Task #{}: {}\nStatus: {}\nRepo: {}\nDescription: {}",
                task.id, task.title, task.status.as_str(), task.repo_path, task.description
            )}]}))
        }
        _ => Err(json!({"code": -32602, "message": format!("unknown tool: {tool_name}")})),
    }
}
```

- [ ] **Step 3: Wire MCP server into TUI startup**

In `run_tui()`, before the main loop:
```rust
let db_arc = Arc::new(db);
let mcp_db = db_arc.clone();
tokio::spawn(async move {
    if let Err(e) = mcp::serve(mcp_db, port).await {
        eprintln!("MCP server error: {e}");
    }
});
```

Note: `Database` uses `Mutex<Connection>` (added in Task 3), so it's already `Send + Sync`.

- [ ] **Step 4: Test MCP server manually**

Run the TUI, then in another terminal:
```bash
curl -X POST http://localhost:3142/mcp \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
```
Expected: JSON response with 3 tool definitions

- [ ] **Step 5: Commit**

```
git add src/mcp/
git commit -m "feat: MCP server with update_task, add_note, get_task tools"
```

---

### Task 13: README and final verification

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Update README**

Document: what the tool does, how to build (`cargo build --release`), how to use (tui/update/list subcommands), key bindings, MCP server, configuration (--db, --port, env vars). Note that rusqlite is sync with spawn_blocking — may migrate to async later.

- [ ] **Step 2: Full test suite**

Run: `cargo test`
Expected: all pass

- [ ] **Step 3: Manual end-to-end test**

1. `cargo run -- tui`
2. Create a task (`n`), fill in title/description/repo
3. Move to Ready (`m`)
4. Dispatch (`d`) — verify worktree + tmux window + claude starts
5. Check Running card shows live output
6. Agent completes → calls MCP `update_task` → card moves to Review
7. Kill TUI, verify `task-orchestrator list` shows task in Review
8. Move to Done, verify cleanup prompt

- [ ] **Step 4: Commit**

```
git add README.md
git commit -m "docs: README with usage, keybindings, and configuration"
```
