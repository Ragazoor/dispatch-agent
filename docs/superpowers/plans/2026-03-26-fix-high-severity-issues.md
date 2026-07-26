# Fix High-Severity Code Review Issues — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix five high-severity issues found in code review: eliminate the id=0 placeholder pattern, fix JSON injection in hooks config, remove TaskStore forwarding boilerplate, fix dual-write partial-update window, and add RAII terminal guard.

**Architecture:** Each fix is independent. The id=0 removal restructures the create-task flow so the DB insert happens in the command handler (not optimistically in `update()`), returning a `TaskCreated` message with the real ID. The other four are surgical fixes to existing code.

**Tech Stack:** Rust, rusqlite, serde_json, crossterm, ratatui

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `src/tui/types.rs` | Modify | Replace `CreateTask`/`TaskIdAssigned` messages with `InsertTask`/`TaskCreated` |
| `src/tui/mod.rs` | Modify | Replace `handle_create_task`/`handle_task_id_assigned` with `handle_task_created` |
| `src/tui/input.rs` | Modify | Update repo-path Enter handler to emit new `InsertTask` command |
| `src/tui/tests.rs` | Modify | Update tests for new create-task flow |
| `src/runtime.rs` | Modify | Add `exec_insert_task`, replace dual-write in `exec_persist_task`, add RAII terminal guard |
| `src/dispatch.rs` | Modify | Use `serde_json::json!()` in `build_hooks_config` |
| `src/db.rs` | Modify | Remove inherent methods, implement `TaskStore` directly |

---

### Task 1: Fix `build_hooks_config` JSON injection

**Files:**
- Modify: `src/dispatch.rs:172-176` (`build_hooks_config`)
- Modify: `src/dispatch.rs:272-284` (test)

- [ ] **Step 1: Add a test for a db_path containing special characters**

Add to the `tests` module in `src/dispatch.rs`:

```rust
#[test]
fn build_hooks_config_escapes_special_chars_in_db_path() {
    let config = build_hooks_config(1, "/path/with spaces/and\"quotes/tasks.db");
    let parsed: serde_json::Value = serde_json::from_str(&config)
        .expect("hooks config with special chars should be valid JSON");
    let stop_cmd = parsed["hooks"]["Stop"][0]["command"].as_str().unwrap();
    assert!(stop_cmd.contains("update 1 review"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test build_hooks_config_escapes`
Expected: FAIL — the current `format!` produces invalid JSON when `db_path` contains `"`.

- [ ] **Step 3: Rewrite `build_hooks_config` using `serde_json`**

Replace the existing `build_hooks_config` function in `src/dispatch.rs`:

```rust
fn build_hooks_config(task_id: i64, db_path: &str) -> String {
    let stop_cmd = format!("task-orchestrator --db '{}' update {} review", db_path, task_id);
    let submit_cmd = format!("task-orchestrator --db '{}' update {} running", db_path, task_id);
    serde_json::json!({
        "hooks": {
            "Stop": [{"type": "command", "command": stop_cmd}],
            "UserPromptSubmit": [{"type": "command", "command": submit_cmd}]
        }
    })
    .to_string()
}
```

This uses `serde_json::json!()` which handles JSON escaping automatically. The `db_path` is single-quoted in the shell command to handle spaces.

- [ ] **Step 4: Run all tests to verify they pass**

Run: `cargo test`
Expected: All tests pass, including both the existing `build_hooks_config_contains_task_id_and_db_path` and the new special-chars test.

- [ ] **Step 5: Commit**

```bash
git add src/dispatch.rs
git commit -m "fix: use serde_json for hooks config to prevent JSON injection"
```

---

### Task 2: Remove TaskStore forwarding boilerplate

**Files:**
- Modify: `src/db.rs`

- [ ] **Step 1: Move the implementation from inherent methods into `impl TaskStore for Database`**

In `src/db.rs`, remove all the `pub fn` inherent methods from `impl Database` (lines 108-295, everything between the `open`/`open_in_memory`/`init_schema` methods and the closing `}` of `impl Database`). Then replace the forwarding `impl TaskStore for Database` block (lines 298-335) with the actual implementations.

The resulting `impl Database` block should contain only:
- `pub fn open(path: &Path) -> Result<Self>`
- `pub fn open_in_memory() -> Result<Self>`
- `fn init_schema(conn: &Connection) -> Result<()>`

The `impl TaskStore for Database` block should contain the actual method bodies (previously in the inherent methods):

```rust
impl TaskStore for Database {
    fn create_task(
        &self,
        title: &str,
        description: &str,
        repo_path: &str,
        plan: Option<&str>,
        status: TaskStatus,
    ) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO tasks (title, description, repo_path, plan, status) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![title, description, repo_path, plan, status.as_str()],
        )
        .context("Failed to insert task")?;
        Ok(conn.last_insert_rowid())
    }

    fn get_task(&self, id: i64) -> Result<Option<Task>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, title, description, repo_path, status, worktree, tmux_window,
                    plan, created_at, updated_at
             FROM tasks WHERE id = ?1",
            params![id],
            row_to_task,
        )
        .optional()
        .context("Failed to get task")
    }

    fn list_all(&self) -> Result<Vec<Task>> {
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
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

    fn update_status(&self, id: i64, status: TaskStatus) -> Result<()> {
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

    fn update_dispatch(
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

    fn delete_task(&self, id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let rows = conn
            .execute("DELETE FROM tasks WHERE id = ?1", params![id])
            .context("Failed to delete task")?;
        if rows == 0 {
            anyhow::bail!("Task {} not found", id);
        }
        Ok(())
    }

    fn update_task(
        &self,
        id: i64,
        title: &str,
        description: &str,
        repo_path: &str,
        status: TaskStatus,
        plan: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let changed = conn
            .execute(
                "UPDATE tasks SET title = ?1, description = ?2, repo_path = ?3, status = ?4, plan = ?5, updated_at = datetime('now') WHERE id = ?6",
                params![title, description, repo_path, status.as_str(), plan, id],
            )
            .context("Failed to update task")?;
        if changed == 0 {
            anyhow::bail!("Task {id} not found");
        }
        Ok(())
    }

    fn add_note(&self, task_id: i64, content: &str, source: NoteSource) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO notes (task_id, content, source) VALUES (?1, ?2, ?3)",
            params![task_id, content, source.as_str()],
        )
        .context("Failed to insert note")?;
        Ok(conn.last_insert_rowid())
    }

    fn list_notes(&self, task_id: i64) -> Result<Vec<Note>> {
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

    fn list_repo_paths(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO repo_paths (path) VALUES (?1)
             ON CONFLICT(path) DO UPDATE SET last_used = datetime('now')",
            params![path],
        )
        .context("Failed to save repo_path")?;
        Ok(())
    }
}
```

- [ ] **Step 2: Fix call sites that use `Database::method` directly**

The existing tests in `src/db.rs` call `db.create_task(...)` etc. These calls still work because Rust resolves trait methods when the trait is in scope (and `use super::*` brings `TaskStore` into scope in the test module). No changes needed to test code.

Check that `src/main.rs` has `use crate::db;` — its `db.update_status(...)` call needs `TaskStore` in scope. Add at the top of `src/main.rs` if not present:

```rust
use task_orchestrator::db::TaskStore;
```

- [ ] **Step 3: Verify tests pass**

Run: `cargo test`
Expected: All tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/db.rs src/main.rs
git commit -m "refactor: implement TaskStore directly on Database, remove forwarding"
```

---

### Task 3: Fix `exec_persist_task` dual-write

**Files:**
- Modify: `src/db.rs` (add `persist_task` to `TaskStore` trait)
- Modify: `src/runtime.rs:119-145` (`exec_persist_task`)

- [ ] **Step 1: Add `persist_task` method to `TaskStore` trait**

In `src/db.rs`, add to the `TaskStore` trait:

```rust
fn persist_task(&self, id: i64, status: TaskStatus, worktree: Option<&str>, tmux_window: Option<&str>) -> Result<()>;
```

- [ ] **Step 2: Implement `persist_task` on Database**

Add to `impl TaskStore for Database`:

```rust
fn persist_task(&self, id: i64, status: TaskStatus, worktree: Option<&str>, tmux_window: Option<&str>) -> Result<()> {
    let conn = self.conn.lock().unwrap();
    let rows = conn
        .execute(
            "UPDATE tasks SET status = ?1, worktree = ?2, tmux_window = ?3, updated_at = datetime('now') WHERE id = ?4",
            params![status.as_str(), worktree, tmux_window, id],
        )
        .context("Failed to persist task")?;
    if rows == 0 {
        anyhow::bail!("Task {} not found", id);
    }
    Ok(())
}
```

- [ ] **Step 3: Write a test for `persist_task`**

Add to the tests in `src/db.rs`:

```rust
#[test]
fn persist_task_updates_status_and_dispatch_atomically() {
    let db = in_memory_db();
    let id = db.create_task("Task", "desc", "/repo", None, TaskStatus::Backlog).unwrap();

    db.persist_task(id, TaskStatus::Running, Some("/wt/task"), Some("task-1")).unwrap();

    let task = db.get_task(id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.worktree.as_deref(), Some("/wt/task"));
    assert_eq!(task.tmux_window.as_deref(), Some("task-1"));
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test persist_task_updates`
Expected: PASS

- [ ] **Step 5: Replace dual-write in `exec_persist_task`**

In `src/runtime.rs`, change the `else` branch of `exec_persist_task` (lines 133-145) from:

```rust
        } else {
            // Existing task — update its status and dispatch fields
            if let Err(e) = self.database.update_status(task.id, task.status) {
                app.update(Message::Error(format!("DB error updating status: {e}")));
            }
            if let Err(e) = self.database.update_dispatch(
                task.id,
                task.worktree.as_deref(),
                task.tmux_window.as_deref(),
            ) {
                app.update(Message::Error(format!("DB error updating dispatch: {e}")));
            }
        }
```

to:

```rust
        } else {
            // Existing task — update status and dispatch fields atomically
            if let Err(e) = self.database.persist_task(
                task.id,
                task.status,
                task.worktree.as_deref(),
                task.tmux_window.as_deref(),
            ) {
                app.update(Message::Error(format!("DB error persisting task: {e}")));
            }
        }
```

- [ ] **Step 6: Verify tests pass**

Run: `cargo test`
Expected: All tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/db.rs src/runtime.rs
git commit -m "fix: atomic persist_task replaces dual-write update_status + update_dispatch"
```

---

### Task 4: Remove id=0 placeholder pattern

**Files:**
- Modify: `src/tui/types.rs`
- Modify: `src/tui/mod.rs`
- Modify: `src/tui/input.rs`
- Modify: `src/tui/tests.rs`
- Modify: `src/runtime.rs`

This is the largest change. The new flow:

1. User finishes entering repo path → `handle_key` returns `Command::InsertTask { title, description, repo_path }` + `Command::SaveRepoPath`
2. `exec_insert_task` does the DB insert, gets real ID, sends `Message::TaskCreated { task }` back
3. `handle_task_created` adds the task (with real ID) to `app.tasks`

No more `id: 0`, no more `TaskIdAssigned`, no more `handle_create_task`.

- [ ] **Step 1: Update Message and Command enums in `types.rs`**

In `src/tui/types.rs`:

Remove `CreateTask` and `TaskIdAssigned` from `Message`:
```rust
// Remove these two lines:
//     CreateTask { title: String, description: String, repo_path: String },
//     TaskIdAssigned { placeholder_id: i64, real_id: i64 },
```

Add `TaskCreated` to `Message`:
```rust
    TaskCreated { task: Task },
```

Add `InsertTask` to `Command`:
```rust
    InsertTask { title: String, description: String, repo_path: String },
```

Remove `PersistTask` handling for `id == 0` — it will never happen after this change. (The `PersistTask` variant stays for existing-task updates.)

- [ ] **Step 2: Update `App::update()` routing in `mod.rs`**

In `src/tui/mod.rs`, in the `update()` method:

Remove:
```rust
            Message::CreateTask { title, description, repo_path } =>
                self.handle_create_task(title, description, repo_path),
```
```rust
            Message::TaskIdAssigned { placeholder_id, real_id } =>
                self.handle_task_id_assigned(placeholder_id, real_id),
```

Add:
```rust
            Message::TaskCreated { task } => self.handle_task_created(task),
```

- [ ] **Step 3: Replace `handle_create_task` and `handle_task_id_assigned` with `handle_task_created`**

Remove `handle_create_task` (lines 228-247) and `handle_task_id_assigned` (lines 338-342).

Add:

```rust
fn handle_task_created(&mut self, task: Task) -> Vec<Command> {
    self.tasks.push(task);
    self.clamp_selection();
    vec![]
}
```

No `PersistTask` command — the task is already in the DB when this message arrives.

- [ ] **Step 4: Update input.rs to return `InsertTask` command directly**

In `src/tui/input.rs`, in the `InputMode::InputRepoPath` branch of `handle_key_text_input` (around line 170-190), change the flow so that instead of calling `self.update(Message::CreateTask { ... })`, it returns the commands directly:

Change from:
```rust
                    InputMode::InputRepoPath => {
                        let draft = self.task_draft.take().unwrap_or_default();
                        let repo_path = if value.is_empty() {
                            if let Some(first) = self.repo_paths.first() {
                                first.clone()
                            } else {
                                self.task_draft = Some(draft);
                                self.status_message =
                                    Some("Repo path required (no saved paths available)".to_string());
                                return vec![];
                            }
                        } else {
                            value
                        };
                        self.mode = InputMode::Normal;
                        self.status_message = None;
                        self.update(Message::CreateTask {
                            title: draft.title,
                            description: draft.description,
                            repo_path,
                        })
                    }
```

to:

```rust
                    InputMode::InputRepoPath => {
                        let draft = self.task_draft.take().unwrap_or_default();
                        let repo_path = if value.is_empty() {
                            if let Some(first) = self.repo_paths.first() {
                                first.clone()
                            } else {
                                self.task_draft = Some(draft);
                                self.status_message =
                                    Some("Repo path required (no saved paths available)".to_string());
                                return vec![];
                            }
                        } else {
                            value
                        };
                        self.mode = InputMode::Normal;
                        self.status_message = None;
                        vec![
                            Command::InsertTask {
                                title: draft.title,
                                description: draft.description,
                                repo_path: repo_path.clone(),
                            },
                            Command::SaveRepoPath(repo_path),
                        ]
                    }
```

Also update the number-key shortcut path (around line 206-216) similarly — change the `self.update(Message::CreateTask { ... })` call to return the commands directly:

```rust
                    if idx < self.repo_paths.len() {
                        let draft = self.task_draft.take().unwrap_or_default();
                        let repo_path = self.repo_paths[idx].clone();
                        self.mode = InputMode::Normal;
                        self.status_message = None;
                        return vec![
                            Command::InsertTask {
                                title: draft.title,
                                description: draft.description,
                                repo_path: repo_path.clone(),
                            },
                            Command::SaveRepoPath(repo_path),
                        ];
                    }
```

- [ ] **Step 5: Add `exec_insert_task` to `TuiRuntime`**

In `src/runtime.rs`, add a new method:

```rust
fn exec_insert_task(&self, app: &mut App, title: String, description: String, repo_path: String) {
    match self.database.create_task(&title, &description, &repo_path, None, models::TaskStatus::Backlog) {
        Ok(new_id) => {
            let now = chrono::Utc::now();
            let task = models::Task {
                id: new_id,
                title,
                description,
                repo_path,
                status: models::TaskStatus::Backlog,
                worktree: None,
                tmux_window: None,
                plan: None,
                created_at: now,
                updated_at: now,
            };
            app.update(Message::TaskCreated { task });
        }
        Err(e) => {
            app.update(Message::Error(format!("DB error creating task: {e}")));
        }
    }
}
```

- [ ] **Step 6: Add routing in `execute_commands`**

In `src/runtime.rs`, in `execute_commands`, add:

```rust
            Command::InsertTask { title, description, repo_path } =>
                rt.exec_insert_task(app, title, description, repo_path),
```

- [ ] **Step 7: Clean up `exec_persist_task` — remove id==0 branch**

In `src/runtime.rs`, simplify `exec_persist_task` to remove the `if task.id == 0` branch entirely:

```rust
fn exec_persist_task(&self, _app: &mut App, task: models::Task) {
    if let Err(e) = self.database.persist_task(
        task.id,
        task.status,
        task.worktree.as_deref(),
        task.tmux_window.as_deref(),
    ) {
        _app.update(Message::Error(format!("DB error persisting task: {e}")));
    }
}
```

Also clean up `exec_delete_task` — remove the `if id != 0` guard:

```rust
fn exec_delete_task(&self, app: &mut App, id: i64) {
    if let Err(e) = self.database.delete_task(id) {
        app.update(Message::Error(format!("DB error deleting task: {e}")));
    }
}
```

- [ ] **Step 8: Update tests in `tests.rs`**

Remove the test `task_id_assigned_updates_placeholder`.

Update `create_task_adds_to_backlog_and_persists` — this test used `Message::CreateTask`. Replace it with a test for `TaskCreated`:

```rust
#[test]
fn task_created_adds_to_list() {
    let now = chrono::Utc::now();
    let task = Task {
        id: 42,
        title: "New Task".to_string(),
        description: "desc".to_string(),
        repo_path: "/repo".to_string(),
        status: TaskStatus::Backlog,
        worktree: None,
        tmux_window: None,
        plan: None,
        created_at: now,
        updated_at: now,
    };
    let mut app = App::new(vec![]);
    let cmds = app.update(Message::TaskCreated { task });
    assert_eq!(app.tasks.len(), 1);
    assert_eq!(app.tasks[0].id, 42);
    assert_eq!(app.tasks[0].status, TaskStatus::Backlog);
    assert!(cmds.is_empty());
}
```

Update `repo_path_empty_uses_saved_path`, `repo_path_empty_no_saved_stays_in_mode`, and `repo_path_nonempty_used_as_is` — these test the input flow. The key difference: they should now check that the returned commands contain `Command::InsertTask` instead of checking `app.tasks`. Example for `repo_path_nonempty_used_as_is`:

```rust
#[test]
fn repo_path_nonempty_used_as_is() {
    let mut app = App::new(vec![]);
    app.repo_paths = vec!["/saved/repo".to_string()];

    app.mode = InputMode::InputRepoPath;
    app.task_draft = Some(TaskDraft { title: "Test".to_string(), description: "desc".to_string() });
    app.input_buffer = "/custom/path".to_string();

    let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    let cmds = app.handle_key(key);

    assert_eq!(app.mode, InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(c, Command::InsertTask { repo_path, .. } if repo_path == "/custom/path")));
    assert_eq!(app.tasks.len(), 0); // task not added until TaskCreated
}
```

Update `repo_path_empty_uses_saved_path`:

```rust
#[test]
fn repo_path_empty_uses_saved_path() {
    let mut app = App::new(vec![]);
    app.repo_paths = vec!["/saved/repo".to_string()];

    app.mode = InputMode::InputRepoPath;
    app.task_draft = Some(TaskDraft { title: "Test".to_string(), description: "desc".to_string() });
    app.input_buffer.clear();

    let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    let cmds = app.handle_key(key);

    assert_eq!(app.mode, InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(c, Command::InsertTask { repo_path, .. } if repo_path == "/saved/repo")));
}
```

Update `number_key_in_repo_path_selects_saved_path`:

```rust
#[test]
fn number_key_in_repo_path_selects_saved_path() {
    let mut app = App::new(vec![]);
    app.mode = InputMode::InputRepoPath;
    app.task_draft = Some(TaskDraft { title: "T".to_string(), description: "d".to_string() });
    app.input_buffer.clear();
    app.repo_paths = vec!["/repo1".to_string(), "/repo2".to_string()];
    let cmds = app.handle_key(make_key(KeyCode::Char('2')));
    assert_eq!(app.mode, InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(c, Command::InsertTask { repo_path, .. } if repo_path == "/repo2")));
}
```

- [ ] **Step 9: Verify all tests pass**

Run: `cargo test`
Expected: All tests pass.

- [ ] **Step 10: Run clippy**

Run: `cargo clippy`
Expected: No warnings.

- [ ] **Step 11: Commit**

```bash
git add src/tui/types.rs src/tui/mod.rs src/tui/input.rs src/tui/tests.rs src/runtime.rs
git commit -m "fix: remove id=0 placeholder — DB insert before adding task to app"
```

---

### Task 5: Add RAII terminal guard in `exec_edit_in_editor`

**Files:**
- Modify: `src/runtime.rs:203-279` (`exec_edit_in_editor`)

- [ ] **Step 1: Create a `TerminalSuspend` guard struct**

Add above the `TuiRuntime` struct in `src/runtime.rs`:

```rust
struct TerminalSuspend<'a> {
    terminal: &'a mut Terminal<CrosstermBackend<io::Stdout>>,
}

impl<'a> TerminalSuspend<'a> {
    fn new(terminal: &'a mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<Self> {
        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        terminal.show_cursor()?;
        Ok(TerminalSuspend { terminal })
    }
}

impl Drop for TerminalSuspend<'_> {
    fn drop(&mut self) {
        let _ = enable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), EnterAlternateScreen);
        let _ = self.terminal.hide_cursor();
        let _ = self.terminal.clear();
    }
}
```

- [ ] **Step 2: Use the guard in `exec_edit_in_editor`**

Replace the manual suspend/resume in `exec_edit_in_editor` (lines 218-233) with the guard:

Change from:
```rust
        // Suspend TUI
        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        terminal.show_cursor()?;

        // Open editor
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vim".to_string());
        let status = std::process::Command::new(&editor)
            .arg(&tmp)
            .status();

        // Resume TUI
        enable_raw_mode()?;
        execute!(terminal.backend_mut(), EnterAlternateScreen)?;
        terminal.hide_cursor()?;
        terminal.clear()?;
```

to:

```rust
        // Suspend TUI (RAII guard restores on drop, even if editor panics)
        let _guard = TerminalSuspend::new(terminal)?;

        // Open editor
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vim".to_string());
        let status = std::process::Command::new(&editor)
            .arg(&tmp)
            .status();

        // Guard will restore terminal on drop at end of scope
        drop(_guard);
```

- [ ] **Step 3: Verify it compiles and tests pass**

Run: `cargo test`
Expected: All tests pass.

- [ ] **Step 4: Run clippy**

Run: `cargo clippy`
Expected: No warnings.

- [ ] **Step 5: Commit**

```bash
git add src/runtime.rs
git commit -m "fix: RAII terminal guard prevents raw-mode leak in editor flow"
```

---

### Task 6: Final verification

- [ ] **Step 1: Run the full test suite**

Run: `cargo test`
Expected: All tests pass.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy`
Expected: No warnings.

- [ ] **Step 3: Verify the build**

Run: `cargo build`
Expected: Compiles cleanly.
