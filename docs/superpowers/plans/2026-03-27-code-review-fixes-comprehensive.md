# Comprehensive Code Review Fixes

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix all actionable findings from the comprehensive code review: quick fixes, DB layer improvements, dispatch refactoring, input-to-message architecture fix, and TuiRuntime test coverage.

**Architecture:** The codebase follows the Elm Architecture (Message/Command pattern). Changes preserve this: the input refactoring (Task 7) closes a violation where input.rs mutated state directly instead of dispatching Messages. The DB changes add a `patch_task` method for atomic partial updates and a `create_task_returning` method to eliminate duplicate Task construction. TuiRuntime tests use real in-memory DB + MockProcessRunner.

**Tech Stack:** Rust, rusqlite, ratatui, crossterm, tokio, serde_json

---

## File Map

| File | Changes |
|------|---------|
| `src/dispatch.rs` | Remove `_mcp_port` from `build_prompt`, fix `add_note` reference, extract `dispatch_with_prompt` helper |
| `src/models.rs` | Extract staleness threshold constants |
| `src/process.rs` | Make `calls` field private |
| `src/tmux.rs` | Fix `has_window` tests to call real function via MockProcessRunner |
| `src/db.rs` | Replace dynamic SQL in `update_title_description`, add `patch_task` method, add `create_task_returning` method |
| `src/mcp/handlers.rs` | Use `patch_task` instead of three separate DB calls |
| `src/runtime.rs` | Use `create_task_returning` to eliminate duplicate Task construction, add test suite |
| `src/tui/types.rs` | Add new Message variants for input flow |
| `src/tui/mod.rs` | Add handler methods for new input Messages |
| `src/tui/input.rs` | Convert direct mutations to Message dispatch |
| `src/tui/tests.rs` | Add tests for new input Message handlers |

---

### Task 1: Fix stale `add_note` reference and unused `_mcp_port`

**Files:**
- Modify: `src/dispatch.rs:197` (build_prompt signature)
- Modify: `src/dispatch.rs:216-235` (build_quick_dispatch_prompt)
- Modify: `src/dispatch.rs:54-70` (dispatch_agent call site)

This task fixes two related issues in dispatch.rs: the `build_prompt` function accepts an `_mcp_port` parameter it never uses, and `build_quick_dispatch_prompt` references an `add_note` MCP tool that doesn't exist.

- [ ] **Step 1: Remove `_mcp_port` from `build_prompt` signature**

Change `build_prompt` to drop the unused parameter:

```rust
fn build_prompt(task_id: i64, title: &str, description: &str, plan: Option<&str>) -> String {
```

Update the call site in `dispatch_agent` (line 57):

```rust
let prompt = build_prompt(task.id, &task.title, &task.description, task.plan.as_deref());
```

- [ ] **Step 2: Fix `add_note` reference in `build_quick_dispatch_prompt`**

Replace line 234 in `build_quick_dispatch_prompt`. Change:

```
post notes as you work (tool: task-orchestrator, tool name: add_note) and \
rename this task (tool: task-orchestrator, tool name: update_task — set the title field).
```

To:

```
rename this task once you understand the goal \
(tool: task-orchestrator, tool name: update_task — set the title field).
```

- [ ] **Step 3: Run tests**

Run: `cargo test --lib dispatch`
Expected: All existing dispatch tests pass (prompt content tests may need updating if they assert on the removed text).

- [ ] **Step 4: Fix any failing prompt-content tests**

The `quick_dispatch_prompt_*` tests in dispatch.rs may assert on `add_note`. Update those assertions to match the new prompt text.

- [ ] **Step 5: Run tests again**

Run: `cargo test --lib dispatch`
Expected: All pass.

- [ ] **Step 6: Commit**

```
fix: remove stale add_note reference and unused _mcp_port parameter
```

---

### Task 2: Extract staleness threshold constants

**Files:**
- Modify: `src/models.rs:186-255`

The staleness thresholds (3 days, 7 days) and the format_age breakpoints (14 days) are magic numbers. Extract them as named constants.

- [ ] **Step 1: Add constants and update `Staleness::from_age`**

Add above the `Staleness` enum (around line 185):

```rust
/// Tasks updated within this many hours are considered fresh.
const FRESH_THRESHOLD_HOURS: i64 = 3 * 24; // 3 days
/// Tasks updated within this many hours are aging (not yet stale).
const AGING_THRESHOLD_HOURS: i64 = 7 * 24; // 7 days
/// Days threshold above which format_age switches to weeks.
const WEEKS_THRESHOLD_DAYS: i64 = 14;
```

Update `Staleness::from_age`:

```rust
if hours < FRESH_THRESHOLD_HOURS {
    Staleness::Fresh
} else if hours < AGING_THRESHOLD_HOURS {
    Staleness::Aging
} else {
    Staleness::Stale
}
```

Update `format_age` to use `WEEKS_THRESHOLD_DAYS`:

```rust
if days < WEEKS_THRESHOLD_DAYS {
    format!("{days}d")
} else {
    format!("{}w", days / 7)
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test --lib models`
Expected: All staleness/format_age tests pass unchanged.

- [ ] **Step 3: Commit**

```
refactor: extract staleness threshold constants in models.rs
```

---

### Task 3: Make `MockProcessRunner::calls` field private

**Files:**
- Modify: `src/process.rs:34`

The `calls` field is `pub` but a `recorded_calls()` accessor already exists. Make it private for consistency.

- [ ] **Step 1: Change field visibility**

```rust
pub struct MockProcessRunner {
    calls: Mutex<Vec<(String, Vec<String>)>>,
    responses: Mutex<VecDeque<Result<Output>>>,
}
```

- [ ] **Step 2: Run full test suite to check for direct field access**

Run: `cargo test`
Expected: If anything accesses `.calls` directly, it will fail to compile. Fix those call sites to use `recorded_calls()` instead.

- [ ] **Step 3: Commit**

```
refactor: make MockProcessRunner::calls field private
```

---

### Task 4: Fix `has_window` tests to call real function

**Files:**
- Modify: `src/tmux.rs:144-166`

Three tests reproduce `has_window`'s internal logic inline instead of calling the real function through MockProcessRunner. Replace them with proper tests.

- [ ] **Step 1: Replace inline logic tests with MockProcessRunner-based tests**

Replace the three tests at lines 144-166:

```rust
#[test]
fn has_window_finds_match_in_output() {
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"main\ntask-42\nother-window\n"),
    ]);
    let result = has_window("task-42", &mock).unwrap();
    assert!(result);
}

#[test]
fn has_window_no_match() {
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"main\nother-window\n"),
    ]);
    let result = has_window("task-42", &mock).unwrap();
    assert!(!result);
}

#[test]
fn has_window_exact_match_not_prefix() {
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"task-42\n"),
    ]);
    let result = has_window("task-4", &mock).unwrap();
    assert!(!result);
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test --lib tmux`
Expected: All pass.

- [ ] **Step 3: Commit**

```
fix: has_window tests now call real function via MockProcessRunner
```

---

### Task 5: Replace dynamic SQL in `update_title_description`

**Files:**
- Modify: `src/db.rs:277-304`

Replace the dynamic SQL builder with explicit match arms.

- [ ] **Step 1: Replace the implementation**

Replace the body of `update_title_description`:

```rust
fn update_title_description(&self, id: i64, title: Option<&str>, description: Option<&str>) -> Result<()> {
    let conn = self.conn.lock().map_err(|_| anyhow::anyhow!("db lock poisoned"))?;
    let rows = match (title, description) {
        (Some(t), Some(d)) => conn.execute(
            "UPDATE tasks SET title = ?1, description = ?2, updated_at = datetime('now') WHERE id = ?3",
            params![t, d, id],
        ),
        (Some(t), None) => conn.execute(
            "UPDATE tasks SET title = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![t, id],
        ),
        (None, Some(d)) => conn.execute(
            "UPDATE tasks SET description = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![d, id],
        ),
        (None, None) => return Ok(()),
    }
    .context("Failed to update title/description")?;
    if rows == 0 {
        anyhow::bail!("Task {id} not found");
    }
    Ok(())
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test --lib db`
Expected: All `update_title_description_*` tests pass.

- [ ] **Step 3: Commit**

```
refactor: replace dynamic SQL in update_title_description with explicit match arms
```

---

### Task 6: Add `patch_task` to TaskStore

**Files:**
- Modify: `src/db.rs` (trait + impl + tests)

Add a `patch_task` method that atomically applies optional field updates in a single SQL statement. This replaces the three-call pattern in MCP's `handle_update_task`.

- [ ] **Step 1: Write the failing test**

Add to `db::tests`:

```rust
#[test]
fn patch_task_updates_status_only() {
    let db = in_memory_db();
    let id = db.create_task("Title", "Desc", "/repo", None, TaskStatus::Backlog).unwrap();

    db.patch_task(id, Some(TaskStatus::Ready), None, None, None).unwrap();

    let task = db.get_task(id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Ready);
    assert_eq!(task.title, "Title");
    assert_eq!(task.description, "Desc");
    assert!(task.plan.is_none());
}

#[test]
fn patch_task_updates_multiple_fields() {
    let db = in_memory_db();
    let id = db.create_task("Title", "Desc", "/repo", None, TaskStatus::Backlog).unwrap();

    db.patch_task(
        id,
        Some(TaskStatus::Running),
        Some("New Title"),
        Some("New Desc"),
        Some(Some("plan.md")),
    ).unwrap();

    let task = db.get_task(id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.title, "New Title");
    assert_eq!(task.description, "New Desc");
    assert_eq!(task.plan.as_deref(), Some("plan.md"));
}

#[test]
fn patch_task_clears_plan() {
    let db = in_memory_db();
    let id = db.create_task("Title", "Desc", "/repo", Some("plan.md"), TaskStatus::Backlog).unwrap();

    db.patch_task(id, None, None, None, Some(None)).unwrap();

    let task = db.get_task(id).unwrap().unwrap();
    assert!(task.plan.is_none());
}

#[test]
fn patch_task_no_fields_is_noop() {
    let db = in_memory_db();
    let id = db.create_task("Title", "Desc", "/repo", None, TaskStatus::Backlog).unwrap();

    db.patch_task(id, None, None, None, None).unwrap();

    let task = db.get_task(id).unwrap().unwrap();
    assert_eq!(task.title, "Title");
}

#[test]
fn patch_task_nonexistent_errors() {
    let db = in_memory_db();
    let result = db.patch_task(9999, Some(TaskStatus::Done), None, None, None);
    assert!(result.is_err());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib db::tests::patch_task`
Expected: Compile error — `patch_task` doesn't exist yet.

- [ ] **Step 3: Add `patch_task` to the `TaskStore` trait**

Add to the trait in `src/db.rs`:

```rust
fn patch_task(
    &self,
    id: i64,
    status: Option<TaskStatus>,
    title: Option<&str>,
    description: Option<&str>,
    plan: Option<Option<&str>>,
) -> Result<()>;
```

The `plan` field uses `Option<Option<&str>>`: `None` means "don't change", `Some(None)` means "clear it", `Some(Some("x"))` means "set to x".

- [ ] **Step 4: Implement `patch_task` on `Database`**

Use explicit match arms like `update_title_description`. Given 4 optional fields, we use a fetch-then-update approach within a single lock hold (no TOCTOU since the Mutex serializes access):

```rust
fn patch_task(
    &self,
    id: i64,
    status: Option<TaskStatus>,
    title: Option<&str>,
    description: Option<&str>,
    plan: Option<Option<&str>>,
) -> Result<()> {
    let conn = self.conn.lock().map_err(|_| anyhow::anyhow!("db lock poisoned"))?;

    // Fetch current values for fields not being patched
    let existing = conn.query_row(
        "SELECT title, description, status, plan FROM tasks WHERE id = ?1",
        params![id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        },
    ).optional().context("Failed to fetch task for patch")?;

    let (cur_title, cur_desc, cur_status, cur_plan) = match existing {
        Some(row) => row,
        None => anyhow::bail!("Task {id} not found"),
    };

    let final_status = status.map(|s| s.as_str().to_string()).unwrap_or(cur_status);
    let final_title = title.unwrap_or(&cur_title);
    let final_desc = description.unwrap_or(&cur_desc);
    let final_plan: Option<&str> = match plan {
        Some(p) => p,
        None => cur_plan.as_deref(),
    };

    conn.execute(
        "UPDATE tasks SET title = ?1, description = ?2, status = ?3, plan = ?4, updated_at = datetime('now') WHERE id = ?5",
        params![final_title, final_desc, final_status, final_plan, id],
    ).context("Failed to patch task")?;

    Ok(())
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test --lib db::tests::patch_task`
Expected: All 5 new tests pass.

- [ ] **Step 6: Run full db tests**

Run: `cargo test --lib db`
Expected: All pass.

- [ ] **Step 7: Commit**

```
feat: add patch_task method for atomic partial updates
```

---

### Task 7: Use `patch_task` in MCP handler

**Files:**
- Modify: `src/mcp/handlers.rs:270-332`

Replace the three separate DB calls in `handle_update_task` with a single `patch_task` call.

- [ ] **Step 1: Run existing MCP tests to establish baseline**

Run: `cargo test --lib mcp`
Expected: All pass.

- [ ] **Step 2: Replace the three DB calls with `patch_task`**

In `handle_update_task`, replace lines 290-320 (the three separate `if let Some(...)` blocks) with:

```rust
let status = if let Some(ref status_str) = parsed.status {
    match TaskStatus::parse(status_str) {
        Some(s) => Some(s),
        None => {
            return JsonRpcResponse::err(
                id,
                -32602,
                format!("Unknown status: {status_str}. Valid values: backlog, ready, running, review, done"),
            )
        }
    }
} else {
    None
};

let plan = parsed.plan.as_ref().map(|p| Some(p.as_str()));

if let Err(e) = state.db.patch_task(
    parsed.task_id,
    status,
    parsed.title.as_deref(),
    parsed.description.as_deref(),
    plan,
) {
    return JsonRpcResponse::err(id, -32603, format!("Database error: {e}"));
}
```

- [ ] **Step 3: Run MCP tests**

Run: `cargo test --lib mcp`
Expected: All pass.

- [ ] **Step 4: Commit**

```
refactor: use atomic patch_task in MCP handle_update_task
```

---

### Task 8: Add `create_task_returning` to TaskStore

**Files:**
- Modify: `src/db.rs` (trait + impl + tests)

Add a method that creates a task and returns the full `Task` struct, eliminating the need for callers to manually reconstruct it.

- [ ] **Step 1: Write the failing test**

Add to `db::tests`:

```rust
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib db::tests::create_task_returning`
Expected: Compile error.

- [ ] **Step 3: Add to trait and implement**

Add to `TaskStore` trait:

```rust
fn create_task_returning(
    &self,
    title: &str,
    description: &str,
    repo_path: &str,
    plan: Option<&str>,
    status: TaskStatus,
) -> Result<Task>;
```

Implement on `Database`:

```rust
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
```

- [ ] **Step 4: Run tests**

Run: `cargo test --lib db::tests::create_task_returning`
Expected: Both pass.

- [ ] **Step 5: Commit**

```
feat: add create_task_returning to get full Task after insert
```

---

### Task 9: Eliminate duplicate Task construction in runtime

**Files:**
- Modify: `src/runtime.rs:150-223`

Replace the manual `Task` construction in `exec_insert_task` and `exec_quick_dispatch` with `create_task_returning`.

- [ ] **Step 1: Update `exec_insert_task`**

Replace lines 151-173:

```rust
fn exec_insert_task(&self, app: &mut App, title: String, description: String, repo_path: String) {
    match self.database.create_task_returning(&title, &description, &repo_path, None, models::TaskStatus::Backlog) {
        Ok(task) => {
            app.update(Message::TaskCreated { task });
        }
        Err(e) => {
            app.update(Message::Error(format!("DB error creating task: {e}")));
        }
    }
}
```

- [ ] **Step 2: Update `exec_quick_dispatch`**

Replace the task creation + manual construction in lines 175-223. The `task.clone()` before the `spawn_blocking` remains since the task is moved into the closure:

```rust
fn exec_quick_dispatch(&self, app: &mut App, title: String, description: String, repo_path: String) {
    match self.database.create_task_returning(&title, &description, &repo_path, None, models::TaskStatus::Ready) {
        Ok(task) => {
            app.update(Message::TaskCreated { task: task.clone() });
            let _ = self.database.save_repo_path(&repo_path);
            let paths = self.database.list_repo_paths().unwrap_or_default();
            app.update(Message::RepoPathsUpdated(paths));
            let tx = self.msg_tx.clone();
            let port = self.port;
            let runner = self.runner.clone();
            tokio::task::spawn_blocking(move || {
                let id = task.id;
                match dispatch::quick_dispatch_agent(&task, port, &*runner) {
                    Ok(result) => {
                        let _ = tx.send(Message::Dispatched {
                            id,
                            worktree: result.worktree_path,
                            tmux_window: result.tmux_window,
                            switch_focus: true,
                        });
                    }
                    Err(e) => {
                        let _ = tx.send(Message::Error(format!("Quick dispatch failed: {e:#}")));
                    }
                }
            });
        }
        Err(e) => {
            app.update(Message::Error(format!("DB error creating task: {e}")));
        }
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test`
Expected: All pass (no behavioral change).

- [ ] **Step 4: Commit**

```
refactor: use create_task_returning to eliminate duplicate Task construction
```

---

### Task 10: Extract `dispatch_with_prompt` helper

**Files:**
- Modify: `src/dispatch.rs:54-113`

The three dispatch functions (`dispatch_agent`, `brainstorm_agent`, `quick_dispatch_agent`) share identical post-provision logic. Extract a shared helper.

- [ ] **Step 1: Run existing dispatch tests to establish baseline**

Run: `cargo test --lib dispatch`
Expected: All pass.

- [ ] **Step 2: Extract the helper**

Add a private function:

```rust
fn dispatch_with_prompt(
    task: &Task,
    prompt: &str,
    runner: &dyn ProcessRunner,
) -> Result<DispatchResult> {
    let provision = provision_worktree(task, runner)?;

    let prompt_file = format!("{}/.claude-prompt", provision.worktree_path);
    fs::write(&prompt_file, prompt)
        .with_context(|| format!("failed to write {prompt_file}"))?;
    tmux::send_keys(
        &provision.tmux_window,
        "claude \"$(cat .claude-prompt)\"",
        runner,
    )
    .context("failed to send keys to tmux window")?;

    Ok(DispatchResult {
        worktree_path: provision.worktree_path,
        tmux_window: provision.tmux_window,
    })
}
```

- [ ] **Step 3: Rewrite the three public functions**

```rust
pub fn dispatch_agent(task: &Task, mcp_port: u16, runner: &dyn ProcessRunner) -> Result<DispatchResult> {
    let prompt = build_prompt(task.id, &task.title, &task.description, task.plan.as_deref());
    tracing::info!(task_id = task.id, "agent dispatched");
    dispatch_with_prompt(task, &prompt, runner)
}

pub fn brainstorm_agent(task: &Task, mcp_port: u16, runner: &dyn ProcessRunner) -> Result<DispatchResult> {
    let prompt = build_brainstorm_prompt(task.id, &task.title, &task.description, mcp_port);
    tracing::info!(task_id = task.id, "brainstorm dispatched");
    dispatch_with_prompt(task, &prompt, runner)
}

pub fn quick_dispatch_agent(task: &Task, mcp_port: u16, runner: &dyn ProcessRunner) -> Result<DispatchResult> {
    let prompt = build_quick_dispatch_prompt(task.id, &task.title, &task.description, mcp_port);
    tracing::info!(task_id = task.id, "quick dispatch agent launched");
    dispatch_with_prompt(task, &prompt, runner)
}
```

Note: `mcp_port` stays in the public signatures since `brainstorm_agent` and `quick_dispatch_agent` pass it to their prompt builders. The tracing log is moved before `dispatch_with_prompt` so the log message appears even if dispatch fails.

- [ ] **Step 4: Run tests**

Run: `cargo test --lib dispatch`
Expected: All pass.

- [ ] **Step 5: Commit**

```
refactor: extract dispatch_with_prompt to deduplicate dispatch functions
```

---

### Task 11: Route input through Messages

**Files:**
- Modify: `src/tui/types.rs` (new Message variants)
- Modify: `src/tui/mod.rs` (new handler methods + routing)
- Modify: `src/tui/input.rs` (replace direct mutation with Message dispatch)
- Modify: `src/tui/tests.rs` (new tests for Message handlers)

This is the architectural fix: `input.rs` currently mutates App fields directly for form entry, mode transitions, and status messages. After this task, `input.rs` becomes a thin `KeyEvent -> Message` translator, and all state changes flow through `App::update()`.

**Design:**

New Messages:
- `BeginCreateTask` — 'n' key pressed, enter title input mode
- `TitleEntered(String)` — user submitted a title
- `DescriptionEntered(String)` — user submitted a description
- `RepoPathEntered(String)` — user submitted a repo path, triggers InsertTask
- `CancelInput` — Esc during any input mode
- `SetStatusMessage(Option<String>)` — display a transient status message
- `EnterConfirmDelete` — 'x' key, enter confirm-delete mode
- `EnterQuickDispatchSelect` — 'D' with multiple repo paths
- `EnterConfirmRetry(i64)` — 'd' on stale/crashed task
- `ConfirmDelete` — 'y' in confirm-delete mode
- `CancelConfirmDelete` — any non-y key in confirm-delete mode
- `CancelConfirmRetry` — Esc in confirm-retry mode
- `QuickDispatchSelect(String)` — digit key in quick dispatch mode
- `CancelQuickDispatch` — Esc in quick dispatch mode
- `InputChar(char)` — character typed during text input
- `InputBackspace` — backspace during text input

- [ ] **Step 1: Add Message variants to `types.rs`**

Add to the `Message` enum:

```rust
// Input flow messages
BeginCreateTask,
TitleEntered(String),
DescriptionEntered(String),
RepoPathEntered(String),
CancelInput,
SetStatusMessage(Option<String>),
EnterConfirmDelete,
ConfirmDelete,
CancelConfirmDelete,
EnterQuickDispatchSelect,
QuickDispatchSelect(String),
CancelQuickDispatch,
EnterConfirmRetry(i64),
CancelConfirmRetry,
InputChar(char),
InputBackspace,
```

- [ ] **Step 2: Write tests for `BeginCreateTask`**

Add to `tests.rs`:

```rust
#[test]
fn begin_create_task_enters_title_mode() {
    let mut app = make_app();
    let cmds = app.update(Message::BeginCreateTask);
    assert!(cmds.is_empty());
    assert_eq!(app.mode, InputMode::InputTitle);
    assert!(app.input_buffer.is_empty());
    assert!(app.task_draft.is_none());
    assert_eq!(app.status_message.as_deref(), Some("Enter title: "));
}
```

- [ ] **Step 3: Write tests for `TitleEntered`**

```rust
#[test]
fn title_entered_advances_to_description() {
    let mut app = make_app();
    app.mode = InputMode::InputTitle;
    let cmds = app.update(Message::TitleEntered("My Task".to_string()));
    assert!(cmds.is_empty());
    assert_eq!(app.mode, InputMode::InputDescription);
    assert!(app.input_buffer.is_empty());
    assert_eq!(app.task_draft.as_ref().unwrap().title, "My Task");
    assert_eq!(app.status_message.as_deref(), Some("Enter description: "));
}

#[test]
fn title_entered_empty_cancels() {
    let mut app = make_app();
    app.mode = InputMode::InputTitle;
    let cmds = app.update(Message::TitleEntered(String::new()));
    assert!(cmds.is_empty());
    assert_eq!(app.mode, InputMode::Normal);
    assert!(app.task_draft.is_none());
    assert!(app.status_message.is_none());
}
```

- [ ] **Step 4: Write tests for `DescriptionEntered`**

```rust
#[test]
fn description_entered_advances_to_repo_path() {
    let mut app = make_app();
    app.mode = InputMode::InputDescription;
    app.task_draft = Some(TaskDraft { title: "T".to_string(), description: String::new() });
    let cmds = app.update(Message::DescriptionEntered("some desc".to_string()));
    assert!(cmds.is_empty());
    assert_eq!(app.mode, InputMode::InputRepoPath);
    assert_eq!(app.task_draft.as_ref().unwrap().description, "some desc");
    assert_eq!(app.status_message.as_deref(), Some("Enter repo path: "));
}
```

- [ ] **Step 5: Write tests for `RepoPathEntered`**

```rust
#[test]
fn repo_path_entered_emits_insert_command() {
    let mut app = make_app();
    app.mode = InputMode::InputRepoPath;
    app.task_draft = Some(TaskDraft { title: "T".to_string(), description: "D".to_string() });
    let cmds = app.update(Message::RepoPathEntered("/my/repo".to_string()));
    assert_eq!(app.mode, InputMode::Normal);
    assert!(app.status_message.is_none());
    assert_eq!(cmds.len(), 2);
    assert!(matches!(&cmds[0], Command::InsertTask { title, description, repo_path }
        if title == "T" && description == "D" && repo_path == "/my/repo"));
    assert!(matches!(&cmds[1], Command::SaveRepoPath(p) if p == "/my/repo"));
}

#[test]
fn repo_path_entered_empty_uses_first_saved_path() {
    let mut app = make_app();
    app.mode = InputMode::InputRepoPath;
    app.task_draft = Some(TaskDraft { title: "T".to_string(), description: "D".to_string() });
    app.repo_paths = vec!["/saved/path".to_string()];
    let cmds = app.update(Message::RepoPathEntered(String::new()));
    assert_eq!(app.mode, InputMode::Normal);
    assert!(matches!(&cmds[0], Command::InsertTask { repo_path, .. } if repo_path == "/saved/path"));
}

#[test]
fn repo_path_entered_empty_no_saved_paths_shows_error() {
    let mut app = make_app();
    app.mode = InputMode::InputRepoPath;
    app.task_draft = Some(TaskDraft { title: "T".to_string(), description: "D".to_string() });
    app.repo_paths = vec![];
    let cmds = app.update(Message::RepoPathEntered(String::new()));
    assert!(cmds.is_empty());
    assert!(app.status_message.as_deref().unwrap().contains("Repo path required"));
    // Should stay in InputRepoPath mode with draft preserved
    assert_eq!(app.mode, InputMode::InputRepoPath);
    assert!(app.task_draft.is_some());
}
```

- [ ] **Step 6: Write tests for `CancelInput`**

```rust
#[test]
fn cancel_input_returns_to_normal() {
    let mut app = make_app();
    app.mode = InputMode::InputTitle;
    app.input_buffer = "partial".to_string();
    app.task_draft = Some(TaskDraft { title: "T".to_string(), description: String::new() });
    let cmds = app.update(Message::CancelInput);
    assert!(cmds.is_empty());
    assert_eq!(app.mode, InputMode::Normal);
    assert!(app.input_buffer.is_empty());
    assert!(app.task_draft.is_none());
    assert!(app.status_message.is_none());
}
```

- [ ] **Step 7: Write tests for mode-transition Messages**

```rust
#[test]
fn enter_confirm_delete_sets_mode() {
    let mut app = make_app();
    let cmds = app.update(Message::EnterConfirmDelete);
    assert!(cmds.is_empty());
    assert_eq!(app.mode, InputMode::ConfirmDelete);
    assert_eq!(app.status_message.as_deref(), Some("Delete task? (y/n)"));
}

#[test]
fn confirm_delete_emits_command() {
    let mut app = make_app();
    app.mode = InputMode::ConfirmDelete;
    // Task 1 is selected (column 0, row 0)
    let cmds = app.update(Message::ConfirmDelete);
    assert_eq!(app.mode, InputMode::Normal);
    assert!(app.status_message.is_none());
    // Should delete the selected task
    assert!(!cmds.is_empty());
}

#[test]
fn cancel_confirm_delete_returns_to_normal() {
    let mut app = make_app();
    app.mode = InputMode::ConfirmDelete;
    let cmds = app.update(Message::CancelConfirmDelete);
    assert!(cmds.is_empty());
    assert_eq!(app.mode, InputMode::Normal);
    assert!(app.status_message.is_none());
}

#[test]
fn set_status_message_updates_field() {
    let mut app = make_app();
    let cmds = app.update(Message::SetStatusMessage(Some("hello".to_string())));
    assert!(cmds.is_empty());
    assert_eq!(app.status_message.as_deref(), Some("hello"));
}

#[test]
fn input_char_appends_to_buffer() {
    let mut app = make_app();
    app.mode = InputMode::InputTitle;
    app.update(Message::InputChar('H'));
    app.update(Message::InputChar('i'));
    assert_eq!(app.input_buffer, "Hi");
}

#[test]
fn input_backspace_pops_from_buffer() {
    let mut app = make_app();
    app.input_buffer = "abc".to_string();
    app.update(Message::InputBackspace);
    assert_eq!(app.input_buffer, "ab");
}
```

- [ ] **Step 8: Run tests to verify they fail (Message variants exist but handlers don't)**

Run: `cargo test --lib tui::tests`
Expected: Compile error or panic — no routing arms in `update()` yet.

- [ ] **Step 9: Add handler methods to `App` in `mod.rs`**

```rust
fn handle_begin_create_task(&mut self) -> Vec<Command> {
    self.mode = InputMode::InputTitle;
    self.input_buffer.clear();
    self.task_draft = None;
    self.status_message = Some("Enter title: ".to_string());
    vec![]
}

fn handle_title_entered(&mut self, value: String) -> Vec<Command> {
    self.input_buffer.clear();
    if value.is_empty() {
        self.mode = InputMode::Normal;
        self.task_draft = None;
        self.status_message = None;
    } else {
        self.task_draft = Some(TaskDraft {
            title: value,
            description: String::new(),
        });
        self.mode = InputMode::InputDescription;
        self.status_message = Some("Enter description: ".to_string());
    }
    vec![]
}

fn handle_description_entered(&mut self, value: String) -> Vec<Command> {
    self.input_buffer.clear();
    if let Some(ref mut draft) = self.task_draft {
        draft.description = value;
    }
    self.mode = InputMode::InputRepoPath;
    self.status_message = Some("Enter repo path: ".to_string());
    vec![]
}

fn handle_repo_path_entered(&mut self, value: String) -> Vec<Command> {
    self.input_buffer.clear();
    let draft = self.task_draft.take().unwrap_or_default();
    let repo_path = if value.is_empty() {
        if let Some(first) = self.repo_paths.first() {
            first.clone()
        } else {
            self.task_draft = Some(draft);
            self.status_message = Some("Repo path required (no saved paths available)".to_string());
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

fn handle_cancel_input(&mut self) -> Vec<Command> {
    self.mode = InputMode::Normal;
    self.input_buffer.clear();
    self.task_draft = None;
    self.status_message = None;
    vec![]
}

fn handle_set_status_message(&mut self, msg: Option<String>) -> Vec<Command> {
    self.status_message = msg;
    vec![]
}

fn handle_enter_confirm_delete(&mut self) -> Vec<Command> {
    self.mode = InputMode::ConfirmDelete;
    self.status_message = Some("Delete task? (y/n)".to_string());
    vec![]
}

fn handle_confirm_delete(&mut self) -> Vec<Command> {
    self.mode = InputMode::Normal;
    self.status_message = None;
    if let Some(task) = self.selected_task() {
        let id = task.id;
        self.handle_delete_task(id)
    } else {
        vec![]
    }
}

fn handle_cancel_confirm_delete(&mut self) -> Vec<Command> {
    self.mode = InputMode::Normal;
    self.status_message = None;
    vec![]
}

fn handle_enter_quick_dispatch_select(&mut self) -> Vec<Command> {
    self.mode = InputMode::QuickDispatch;
    self.status_message = Some("Select repo path (1-9) or Esc to cancel".to_string());
    vec![]
}

fn handle_quick_dispatch_select(&mut self, repo_path: String) -> Vec<Command> {
    self.mode = InputMode::Normal;
    self.status_message = None;
    self.handle_quick_dispatch(repo_path)
}

fn handle_cancel_quick_dispatch(&mut self) -> Vec<Command> {
    self.mode = InputMode::Normal;
    self.status_message = None;
    vec![]
}

fn handle_enter_confirm_retry(&mut self, id: i64) -> Vec<Command> {
    self.handle_kill_and_retry(id)
}

fn handle_cancel_confirm_retry(&mut self) -> Vec<Command> {
    self.mode = InputMode::Normal;
    self.status_message = None;
    vec![]
}

fn handle_input_char(&mut self, c: char) -> Vec<Command> {
    self.input_buffer.push(c);
    vec![]
}

fn handle_input_backspace(&mut self) -> Vec<Command> {
    self.input_buffer.pop();
    vec![]
}
```

- [ ] **Step 10: Add routing arms in `update()`**

Add to the match in `App::update()`:

```rust
Message::BeginCreateTask => self.handle_begin_create_task(),
Message::TitleEntered(value) => self.handle_title_entered(value),
Message::DescriptionEntered(value) => self.handle_description_entered(value),
Message::RepoPathEntered(value) => self.handle_repo_path_entered(value),
Message::CancelInput => self.handle_cancel_input(),
Message::SetStatusMessage(msg) => self.handle_set_status_message(msg),
Message::EnterConfirmDelete => self.handle_enter_confirm_delete(),
Message::ConfirmDelete => self.handle_confirm_delete(),
Message::CancelConfirmDelete => self.handle_cancel_confirm_delete(),
Message::EnterQuickDispatchSelect => self.handle_enter_quick_dispatch_select(),
Message::QuickDispatchSelect(repo_path) => self.handle_quick_dispatch_select(repo_path),
Message::CancelQuickDispatch => self.handle_cancel_quick_dispatch(),
Message::EnterConfirmRetry(id) => self.handle_enter_confirm_retry(id),
Message::CancelConfirmRetry => self.handle_cancel_confirm_retry(),
Message::InputChar(c) => self.handle_input_char(c),
Message::InputBackspace => self.handle_input_backspace(),
```

- [ ] **Step 11: Run the new tests**

Run: `cargo test --lib tui::tests`
Expected: All new tests pass. Existing tests also pass (the old handlers still exist).

- [ ] **Step 12: Rewrite `input.rs` to dispatch Messages**

Now replace all direct mutations in `input.rs` with `self.update(Message::...)` calls.

**`handle_key_normal`** — Replace direct mutations:

```rust
fn handle_key_normal(&mut self, key: KeyEvent) -> Vec<Command> {
    match key.code {
        KeyCode::Char('q') => self.update(Message::Quit),

        KeyCode::Char('h') | KeyCode::Left => self.update(Message::NavigateColumn(-1)),
        KeyCode::Char('l') | KeyCode::Right => self.update(Message::NavigateColumn(1)),
        KeyCode::Char('j') | KeyCode::Down => self.update(Message::NavigateRow(1)),
        KeyCode::Char('k') | KeyCode::Up => self.update(Message::NavigateRow(-1)),

        KeyCode::Char('n') => self.update(Message::BeginCreateTask),

        KeyCode::Char('d') => {
            if let Some(task) = self.selected_task() {
                let id = task.id;
                let status = task.status;
                let has_window = task.tmux_window.is_some();
                let has_worktree = task.worktree.is_some();
                match status {
                    TaskStatus::Backlog => self.update(Message::BrainstormTask(id)),
                    TaskStatus::Ready => self.update(Message::DispatchTask(id)),
                    TaskStatus::Running | TaskStatus::Review => {
                        if self.stale_tasks.contains(&id) || self.crashed_tasks.contains(&id) {
                            self.update(Message::EnterConfirmRetry(id))
                        } else if has_window {
                            self.update(Message::SetStatusMessage(Some(
                                "Agent already running, press g to jump".to_string(),
                            )))
                        } else if has_worktree {
                            self.update(Message::ResumeTask(id))
                        } else {
                            self.update(Message::SetStatusMessage(Some(
                                "No worktree to resume, move to Ready and re-dispatch".to_string(),
                            )))
                        }
                    }
                    TaskStatus::Done => {
                        self.update(Message::SetStatusMessage(Some("Task is done".to_string())))
                    }
                }
            } else {
                vec![]
            }
        }

        KeyCode::Char('g') => {
            if let Some(task) = self.selected_task() {
                if let Some(window) = &task.tmux_window {
                    vec![Command::JumpToTmux { window: window.clone() }]
                } else {
                    self.update(Message::SetStatusMessage(Some("No active session".to_string())))
                }
            } else {
                vec![]
            }
        }

        KeyCode::Char('m') => {
            if let Some(task) = self.selected_task() {
                let id = task.id;
                self.update(Message::MoveTask { id, direction: MoveDirection::Forward })
            } else {
                vec![]
            }
        }

        KeyCode::Char('M') => {
            if let Some(task) = self.selected_task() {
                let id = task.id;
                self.update(Message::MoveTask { id, direction: MoveDirection::Backward })
            } else {
                vec![]
            }
        }

        KeyCode::Enter => self.update(Message::ToggleDetail),

        KeyCode::Char('e') => {
            if let Some(task) = self.selected_task() {
                vec![Command::EditTaskInEditor(task.clone())]
            } else {
                vec![]
            }
        }

        KeyCode::Char('x') => {
            if self.selected_task().is_some() {
                self.update(Message::EnterConfirmDelete)
            } else {
                vec![]
            }
        }

        KeyCode::Char('D') => {
            match self.repo_paths.len() {
                0 => self.update(Message::SetStatusMessage(
                    Some("No saved repo paths — create a task first".to_string()),
                )),
                1 => {
                    let repo_path = self.repo_paths[0].clone();
                    self.update(Message::QuickDispatch { repo_path })
                }
                _ => self.update(Message::EnterQuickDispatchSelect),
            }
        }

        _ => vec![],
    }
}
```

**`handle_key_text_input`** — Replace direct mutations:

```rust
fn handle_key_text_input(&mut self, key: KeyEvent) -> Vec<Command> {
    match key.code {
        KeyCode::Esc => self.update(Message::CancelInput),

        KeyCode::Enter => {
            let value = self.input_buffer.trim().to_string();

            match self.mode.clone() {
                InputMode::InputTitle => self.update(Message::TitleEntered(value)),
                InputMode::InputDescription => self.update(Message::DescriptionEntered(value)),
                InputMode::InputRepoPath => {
                    // In repo path mode, handle digit shortcut for saved paths
                    self.update(Message::RepoPathEntered(value))
                }
                _ => vec![],
            }
        }

        KeyCode::Backspace => self.update(Message::InputBackspace),

        KeyCode::Char(c) => {
            // In repo path mode with empty buffer, 1-9 selects a saved path
            if self.mode == InputMode::InputRepoPath
                && self.input_buffer.is_empty()
                && c.is_ascii_digit()
                && c != '0'
            {
                let idx = (c as usize) - ('1' as usize);
                if idx < self.repo_paths.len() {
                    let repo_path = self.repo_paths[idx].clone();
                    return self.update(Message::RepoPathEntered(repo_path));
                }
            }
            self.update(Message::InputChar(c))
        }

        _ => vec![],
    }
}
```

**`handle_key_confirm_delete`**:

```rust
fn handle_key_confirm_delete(&mut self, key: KeyEvent) -> Vec<Command> {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => self.update(Message::ConfirmDelete),
        _ => self.update(Message::CancelConfirmDelete),
    }
}
```

**`handle_key_quick_dispatch`**:

```rust
fn handle_key_quick_dispatch(&mut self, key: KeyEvent) -> Vec<Command> {
    match key.code {
        KeyCode::Esc => self.update(Message::CancelQuickDispatch),
        KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
            let idx = (c as usize) - ('1' as usize);
            if idx < self.repo_paths.len() {
                let repo_path = self.repo_paths[idx].clone();
                self.update(Message::QuickDispatchSelect(repo_path))
            } else {
                vec![]
            }
        }
        _ => vec![],
    }
}
```

**`handle_key_confirm_retry`**:

```rust
fn handle_key_confirm_retry(&mut self, key: KeyEvent, id: i64) -> Vec<Command> {
    match key.code {
        KeyCode::Char('r') => self.update(Message::RetryResume(id)),
        KeyCode::Char('f') => self.update(Message::RetryFresh(id)),
        KeyCode::Esc => self.update(Message::CancelConfirmRetry),
        _ => vec![],
    }
}
```

- [ ] **Step 13: Run ALL tests**

Run: `cargo test`
Expected: All pass. The key-event based tests in tests.rs still work because they drive through `handle_key` which now dispatches Messages, which in turn call the same handlers.

- [ ] **Step 14: Remove `KillAndRetry` from the public `Message` enum if unused**

Check if `Message::KillAndRetry` is still dispatched from anywhere other than `handle_enter_confirm_retry`. If `handle_enter_confirm_retry` just calls `self.handle_kill_and_retry(id)` directly (which it does in the implementation above), then the only remaining caller is `input.rs` for stale/crashed tasks. But we changed `input.rs` to dispatch `EnterConfirmRetry` instead. So `KillAndRetry` is now only called from `handle_enter_confirm_retry`. This is fine — keep it as an internal implementation detail. No removal needed.

- [ ] **Step 15: Run clippy**

Run: `cargo clippy`
Expected: No new warnings.

- [ ] **Step 16: Commit**

```
refactor: route all input through Message dispatch

input.rs is now a pure KeyEvent-to-Message translator.
All state mutations flow through App::update(), making the
form entry flow testable via Message dispatch.
```

---

### Task 12: Add TuiRuntime tests

**Files:**
- Modify: `src/runtime.rs` (add `#[cfg(test)]` module)

Add tests for TuiRuntime using real in-memory DB + MockProcessRunner. Focus on the synchronous exec_* methods that can be tested without a terminal.

- [ ] **Step 1: Add test module boilerplate**

Add at the end of `src/runtime.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::models::TaskStatus;
    use crate::process::MockProcessRunner;
    use std::time::Duration;
    use tokio::sync::mpsc;

    fn make_runtime(db: Arc<dyn db::TaskStore>, runner: Arc<dyn ProcessRunner>) -> (TuiRuntime, mpsc::UnboundedReceiver<Message>) {
        let (msg_tx, msg_rx) = mpsc::unbounded_channel();
        let rt = TuiRuntime {
            database: db,
            msg_tx,
            port: 3142,
            input_paused: Arc::new(AtomicBool::new(false)),
            runner,
        };
        (rt, msg_rx)
    }

    fn make_app() -> App {
        App::new(vec![], Duration::from_secs(300))
    }

    fn in_memory_db() -> Arc<Database> {
        Arc::new(Database::open_in_memory().unwrap())
    }
}
```

- [ ] **Step 2: Test `exec_insert_task`**

```rust
#[test]
fn exec_insert_task_creates_task_and_updates_app() {
    let db = in_memory_db();
    let runner = Arc::new(MockProcessRunner::new(vec![]));
    let (rt, _rx) = make_runtime(db.clone(), runner);
    let mut app = make_app();

    rt.exec_insert_task(&mut app, "Title".into(), "Desc".into(), "/repo".into());

    // Task should be in DB
    let tasks = db.list_all().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].title, "Title");

    // Task should be in App
    assert_eq!(app.tasks().len(), 1);
    assert_eq!(app.tasks()[0].title, "Title");
    assert_eq!(app.tasks()[0].status, TaskStatus::Backlog);
}
```

- [ ] **Step 3: Test `exec_persist_task`**

```rust
#[test]
fn exec_persist_task_updates_db() {
    let db = in_memory_db();
    let id = db.create_task("T", "D", "/r", None, TaskStatus::Backlog).unwrap();
    let runner = Arc::new(MockProcessRunner::new(vec![]));
    let (rt, _rx) = make_runtime(db.clone(), runner);
    let mut app = make_app();

    let mut task = db.get_task(id).unwrap().unwrap();
    task.status = TaskStatus::Running;
    task.worktree = Some("/wt".to_string());
    task.tmux_window = Some("win".to_string());

    rt.exec_persist_task(&mut app, task);

    let updated = db.get_task(id).unwrap().unwrap();
    assert_eq!(updated.status, TaskStatus::Running);
    assert_eq!(updated.worktree.as_deref(), Some("/wt"));
    assert_eq!(updated.tmux_window.as_deref(), Some("win"));
    assert!(app.error_popup().is_none());
}
```

- [ ] **Step 4: Test `exec_delete_task`**

```rust
#[test]
fn exec_delete_task_removes_from_db() {
    let db = in_memory_db();
    let id = db.create_task("T", "D", "/r", None, TaskStatus::Backlog).unwrap();
    let runner = Arc::new(MockProcessRunner::new(vec![]));
    let (rt, _rx) = make_runtime(db.clone(), runner);
    let mut app = make_app();

    rt.exec_delete_task(&mut app, id);

    assert!(db.get_task(id).unwrap().is_none());
    assert!(app.error_popup().is_none());
}

#[test]
fn exec_delete_task_nonexistent_shows_error() {
    let db = in_memory_db();
    let runner = Arc::new(MockProcessRunner::new(vec![]));
    let (rt, _rx) = make_runtime(db, runner);
    let mut app = make_app();

    rt.exec_delete_task(&mut app, 9999);

    assert!(app.error_popup().is_some());
}
```

- [ ] **Step 5: Test `exec_save_repo_path`**

```rust
#[test]
fn exec_save_repo_path_persists_and_updates_app() {
    let db = in_memory_db();
    let runner = Arc::new(MockProcessRunner::new(vec![]));
    let (rt, _rx) = make_runtime(db.clone(), runner);
    let mut app = make_app();

    rt.exec_save_repo_path(&mut app, "/my/repo".into());

    let paths = db.list_repo_paths().unwrap();
    assert_eq!(paths, vec!["/my/repo"]);
    assert_eq!(app.repo_paths(), &["/my/repo"]);
}
```

- [ ] **Step 6: Test `exec_refresh_from_db`**

```rust
#[test]
fn exec_refresh_from_db_updates_app_tasks() {
    let db = in_memory_db();
    let id = db.create_task("T", "D", "/r", None, TaskStatus::Backlog).unwrap();
    let runner = Arc::new(MockProcessRunner::new(vec![]));
    let (rt, _rx) = make_runtime(db.clone(), runner);
    let mut app = make_app();

    // App starts empty
    assert!(app.tasks().is_empty());

    rt.exec_refresh_from_db(&mut app);

    // Now app should have the task from DB
    assert_eq!(app.tasks().len(), 1);
    assert_eq!(app.tasks()[0].id, id);
}
```

- [ ] **Step 7: Test `exec_jump_to_tmux` success and failure**

```rust
#[test]
fn exec_jump_to_tmux_calls_select_window() {
    let db = in_memory_db();
    let runner = Arc::new(MockProcessRunner::new(vec![MockProcessRunner::ok()]));
    let (rt, _rx) = make_runtime(db, runner.clone());
    let mut app = make_app();

    rt.exec_jump_to_tmux(&mut app, "task-1".into());

    let calls = runner.recorded_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "tmux");
    assert!(calls[0].1.contains(&"select-window".to_string()));
    assert!(app.error_popup().is_none());
}

#[test]
fn exec_jump_to_tmux_failure_shows_error() {
    let db = in_memory_db();
    let runner = Arc::new(MockProcessRunner::new(vec![MockProcessRunner::fail("no window")]));
    let (rt, _rx) = make_runtime(db, runner);
    let mut app = make_app();

    rt.exec_jump_to_tmux(&mut app, "nonexistent".into());

    assert!(app.error_popup().is_some());
}
```

- [ ] **Step 8: Run tests**

Run: `cargo test --lib runtime`
Expected: All pass.

- [ ] **Step 9: Commit**

```
test: add TuiRuntime unit tests for synchronous exec_* methods
```

---

## Summary

| Task | Scope | Estimated size |
|------|-------|---------------|
| 1. Fix `add_note` + `_mcp_port` | Quick fix | Small |
| 2. Staleness constants | Quick fix | Small |
| 3. MockProcessRunner::calls private | Quick fix | Trivial |
| 4. Fix has_window tests | Quick fix | Small |
| 5. Replace dynamic SQL | Quick fix | Small |
| 6. Add `patch_task` to TaskStore | DB layer | Medium |
| 7. Use `patch_task` in MCP | MCP | Small |
| 8. Add `create_task_returning` | DB layer | Small |
| 9. Eliminate duplicate Task construction | Runtime | Small |
| 10. Extract `dispatch_with_prompt` | Dispatch | Small |
| 11. Route input through Messages | Architecture | Large |
| 12. TuiRuntime tests | Testing | Medium |

Tasks 1-5 are independent quick fixes. Tasks 6-7 form a dependency chain (patch_task then use it). Tasks 8-9 form a chain (create_task_returning then use it). Task 10 is independent. Task 11 is the big refactor, independent of DB changes. Task 12 is independent but benefits from Task 9 being done first (so it tests the cleaner code).
