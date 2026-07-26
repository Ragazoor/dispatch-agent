# Code Review Round 2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Address all verified findings from the second code review: bugs, reliability, code duplication, type safety, and test coverage gaps.

**Architecture:** Elm Architecture preserved throughout. Messages flow through `App::update()`, commands through `execute_commands()`. Side effects stay in runtime. All changes strengthen existing patterns without altering the core contract.

**Tech Stack:** Rust, rusqlite (transactions), ratatui, tokio, tracing

---

### Task 1: Fix stale `add_note` in quick dispatch prompt

**Files:**
- Modify: `src/dispatch.rs:218-236` (prompt text)
- Modify: `src/dispatch.rs:358-363` (test)

- [ ] **Step 1: Update the test to expect correct MCP tools**

In `src/dispatch.rs`, find the test `build_quick_dispatch_prompt_mentions_mcp` (line 358-363). Replace:

```rust
    #[test]
    fn build_quick_dispatch_prompt_mentions_mcp() {
        let prompt = build_quick_dispatch_prompt(1, "Quick task", "", 3142);
        assert!(prompt.contains("3142"));
        assert!(prompt.contains("update_task"));
        assert!(!prompt.contains("add_note"));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test build_quick_dispatch_prompt_mentions_mcp`
Expected: FAIL — prompt still contains `add_note`

- [ ] **Step 3: Fix the prompt text**

In `src/dispatch.rs`, replace lines 232-236 of `build_quick_dispatch_prompt`:

```rust
"An MCP server is available at http://localhost:{mcp_port}/mcp — use it to \
query and update tasks (tool: task-orchestrator). Use update_task to rename \
this task with a descriptive title, and get_task to check current state."
```

(Replaces the old text that referenced `add_note`.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test build_quick_dispatch_prompt`
Expected: All 3 quick dispatch prompt tests PASS

- [ ] **Step 5: Commit**

```bash
git add src/dispatch.rs
git commit -m "fix: remove stale add_note reference from quick dispatch prompt"
```

---

### Task 2: Remove dead `_mcp_port` parameter from `build_prompt`

**Files:**
- Modify: `src/dispatch.rs:197` (function signature)
- Modify: `src/dispatch.rs:57` (call site in `dispatch_agent`)
- Modify: `src/dispatch.rs:297-311` (tests)

- [ ] **Step 1: Update tests to not pass `mcp_port` to `build_prompt`**

In the test module, update these tests:

```rust
    #[test]
    fn build_prompt_contains_task_info() {
        let prompt = build_prompt(42, "Fix bug", "A nasty crash", None);
        assert!(prompt.contains("42"));
        assert!(prompt.contains("Fix bug"));
        assert!(prompt.contains("A nasty crash"));
        assert!(prompt.contains("automatically via hooks"));
    }

    #[test]
    fn build_prompt_mentions_automatic_hooks() {
        let prompt = build_prompt(7, "Title", "Desc", None);
        assert!(prompt.contains("automatically via hooks"));
        assert!(!prompt.contains("update the task status to 'review'"));
    }

    #[test]
    fn build_prompt_includes_plan_path() {
        let prompt = build_prompt(1, "Task", "Desc", Some("docs/plans/my-plan.md"));
        assert!(prompt.contains("Plan: docs/plans/my-plan.md"));
    }

    #[test]
    fn build_prompt_without_plan_omits_plan_section() {
        let prompt = build_prompt(1, "Task", "Desc", None);
        assert!(!prompt.contains("Plan:"));
    }

    #[test]
    fn build_quick_dispatch_prompt_differs_from_regular() {
        let regular = build_prompt(1, "Task", "Desc", None);
        let quick = build_quick_dispatch_prompt(1, "Task", "Desc", 3142);
        assert!(quick.contains("placeholder"));
        assert!(!regular.contains("placeholder"));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test build_prompt`
Expected: FAIL — `build_prompt` still takes 5 params

- [ ] **Step 3: Remove the parameter**

In `src/dispatch.rs`, change the `build_prompt` signature from:

```rust
fn build_prompt(task_id: i64, title: &str, description: &str, _mcp_port: u16, plan: Option<&str>) -> String {
```

To:

```rust
fn build_prompt(task_id: i64, title: &str, description: &str, plan: Option<&str>) -> String {
```

Update the call site in `dispatch_agent` (line 57) from:

```rust
    let prompt = build_prompt(task.id, &task.title, &task.description, mcp_port, task.plan.as_deref());
```

To:

```rust
    let prompt = build_prompt(task.id, &task.title, &task.description, task.plan.as_deref());
```

- [ ] **Step 4: Run all tests**

Run: `cargo test`
Expected: All tests PASS

- [ ] **Step 5: Commit**

```bash
git add src/dispatch.rs
git commit -m "fix: remove unused _mcp_port parameter from build_prompt"
```

---

### Task 3: Add `update_task_partial` with transaction to `db.rs`

**Files:**
- Modify: `src/db.rs:13-28` (TaskStore trait)
- Modify: `src/db.rs` (Database impl, after `find_task_by_plan`)
- Modify: `src/db.rs` (test module)

- [ ] **Step 1: Write failing tests**

Add to the test module in `src/db.rs`:

```rust
    #[test]
    fn update_task_partial_applies_all_fields() {
        let db = in_memory_db();
        let id = db
            .create_task("title", "desc", "/repo", None, TaskStatus::Backlog)
            .unwrap();
        db.update_task_partial(
            id,
            Some(TaskStatus::Ready),
            Some(Some("plan.md")),
            Some("new title"),
            None,
        )
        .unwrap();
        let task = db.get_task(id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Ready);
        assert_eq!(task.plan.as_deref(), Some("plan.md"));
        assert_eq!(task.title, "new title");
        assert_eq!(task.description, "desc"); // unchanged
    }

    #[test]
    fn update_task_partial_none_fields_unchanged() {
        let db = in_memory_db();
        let id = db
            .create_task("title", "desc", "/repo", Some("plan.md"), TaskStatus::Ready)
            .unwrap();
        db.update_task_partial(id, None, None, None, None).unwrap();
        let task = db.get_task(id).unwrap().unwrap();
        assert_eq!(task.title, "title");
        assert_eq!(task.plan.as_deref(), Some("plan.md"));
        assert_eq!(task.status, TaskStatus::Ready);
    }

    #[test]
    fn update_task_partial_clears_plan() {
        let db = in_memory_db();
        let id = db
            .create_task("title", "desc", "/repo", Some("plan.md"), TaskStatus::Ready)
            .unwrap();
        db.update_task_partial(id, None, Some(None), None, None)
            .unwrap();
        let task = db.get_task(id).unwrap().unwrap();
        assert!(task.plan.is_none());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test update_task_partial`
Expected: FAIL — method doesn't exist

- [ ] **Step 3: Add the trait method**

In `src/db.rs`, add to the `TaskStore` trait (after `find_task_by_plan`):

```rust
    fn update_task_partial(
        &self,
        id: i64,
        status: Option<TaskStatus>,
        plan: Option<Option<&str>>,
        title: Option<&str>,
        description: Option<&str>,
    ) -> Result<()>;
```

- [ ] **Step 4: Implement with transaction**

Add the implementation in `impl TaskStore for Database` (after `find_task_by_plan`):

```rust
    fn update_task_partial(
        &self,
        id: i64,
        status: Option<TaskStatus>,
        plan: Option<Option<&str>>,
        title: Option<&str>,
        description: Option<&str>,
    ) -> Result<()> {
        let mut conn = self.conn.lock().map_err(|_| anyhow::anyhow!("db lock poisoned"))?;
        let tx = conn.transaction().context("Failed to begin transaction")?;

        let mut parts = Vec::new();
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(s) = status {
            parts.push("status = ?");
            params_vec.push(Box::new(s.as_str().to_string()));
        }
        if let Some(p) = plan {
            parts.push("plan = ?");
            params_vec.push(Box::new(p.map(|s| s.to_string())));
        }
        if let Some(t) = title {
            parts.push("title = ?");
            params_vec.push(Box::new(t.to_string()));
        }
        if let Some(d) = description {
            parts.push("description = ?");
            params_vec.push(Box::new(d.to_string()));
        }

        if parts.is_empty() {
            return Ok(());
        }

        parts.push("updated_at = datetime('now')");
        params_vec.push(Box::new(id));

        let sql = format!("UPDATE tasks SET {} WHERE id = ?", parts.join(", "));
        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();
        let rows = tx
            .execute(&sql, params_refs.as_slice())
            .context("Failed to update task fields")?;
        if rows == 0 {
            anyhow::bail!("Task {id} not found");
        }

        tx.commit().context("Failed to commit task update")?;
        Ok(())
    }
```

Note: `conn` must be `let mut conn` (not `let conn`) because `transaction()` requires `&mut Connection`.

- [ ] **Step 5: Run tests**

Run: `cargo test update_task_partial`
Expected: All 3 tests PASS

- [ ] **Step 6: Commit**

```bash
git add src/db.rs
git commit -m "feat: add update_task_partial with transaction for atomic MCP updates"
```

---

### Task 4: Wire `update_task_partial` into MCP handler

**Files:**
- Modify: `src/mcp/handlers.rs:270-332`

- [ ] **Step 1: Update the MCP handler test to verify atomicity**

Add a test in `src/mcp/handlers.rs` test module:

```rust
    #[tokio::test]
    async fn update_task_partial_sets_multiple_fields() {
        let state = test_state().await;
        let create_resp = handle_mcp(
            State(state.clone()),
            Json(json!({
                "jsonrpc": "2.0", "id": 1,
                "method": "tools/call",
                "params": {"name": "create_task", "arguments": {"title": "Test", "description": "Desc", "repo_path": "/repo"}}
            })),
        )
        .await;
        let task_id = create_resp.result.as_ref().unwrap()["content"][0]["text"]
            .as_str().unwrap()
            .split_whitespace().last().unwrap()
            .parse::<i64>().unwrap();

        let update_resp = handle_mcp(
            State(state.clone()),
            Json(json!({
                "jsonrpc": "2.0", "id": 2,
                "method": "tools/call",
                "params": {"name": "update_task", "arguments": {
                    "task_id": task_id,
                    "status": "ready",
                    "title": "Updated Title"
                }}
            })),
        )
        .await;
        assert!(update_resp.error.is_none());

        let task = state.db.get_task(task_id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Ready);
        assert_eq!(task.title, "Updated Title");
    }
```

- [ ] **Step 2: Run test to verify it passes with current code**

Run: `cargo test update_task_partial_sets_multiple_fields`
Expected: PASS (current 3-call approach still works)

- [ ] **Step 3: Replace 3 DB calls with single `update_task_partial`**

In `src/mcp/handlers.rs`, replace the three if-blocks (lines 290-320) with:

```rust
    let plan_value = parsed.plan.as_ref().map(|p| Some(p.as_str()));

    if let Err(e) = state.db.update_task_partial(
        parsed.task_id,
        parsed.status.as_ref().and_then(|s| TaskStatus::parse(s)),
        plan_value,
        parsed.title.as_deref(),
        parsed.description.as_deref(),
    ) {
        return JsonRpcResponse::err(id, -32603, format!("Database error: {e}"));
    }
```

But we still need to validate the status string first. The full replacement for lines 290-320:

```rust
    let status = if let Some(ref status_str) = parsed.status {
        match TaskStatus::parse(status_str) {
            Some(s) => Some(s),
            None => {
                return JsonRpcResponse::err(
                    id,
                    -32602,
                    format!(
                        "Unknown status: {status_str}. Valid values: backlog, ready, running, review, done"
                    ),
                )
            }
        }
    } else {
        None
    };

    let plan = parsed.plan.as_ref().map(|p| Some(p.as_str()));

    if let Err(e) = state.db.update_task_partial(
        parsed.task_id,
        status,
        plan,
        parsed.title.as_deref(),
        parsed.description.as_deref(),
    ) {
        return JsonRpcResponse::err(id, -32603, format!("Database error: {e}"));
    }
```

- [ ] **Step 4: Run all MCP tests**

Run: `cargo test --lib mcp`
Expected: All MCP tests PASS

- [ ] **Step 5: Commit**

```bash
git add src/mcp/handlers.rs
git commit -m "refactor: use atomic update_task_partial in MCP handler"
```

---

### Task 5: Surface swallowed errors in runtime

**Files:**
- Modify: `src/runtime.rs:175-222` (exec_quick_dispatch)
- Modify: `src/runtime.rs:310-389` (exec_edit_in_editor)
- Modify: `src/runtime.rs:391-395` (exec_save_repo_path)

- [ ] **Step 1: Fix `exec_save_repo_path`**

Replace `exec_save_repo_path` (lines 391-395):

```rust
    fn exec_save_repo_path(&self, app: &mut App, path: String) {
        if let Err(e) = self.database.save_repo_path(&path) {
            tracing::warn!("failed to save repo path: {e}");
        }
        let paths = self.database.list_repo_paths().unwrap_or_else(|e| {
            tracing::warn!("failed to list repo paths: {e}");
            vec![]
        });
        app.update(Message::RepoPathsUpdated(paths));
    }
```

- [ ] **Step 2: Fix `exec_quick_dispatch` repo path calls**

In `exec_quick_dispatch` (around lines 194-197), replace:

```rust
                let _ = self.database.save_repo_path(&repo_path);
                let paths = self.database.list_repo_paths().unwrap_or_default();
```

With:

```rust
                if let Err(e) = self.database.save_repo_path(&repo_path) {
                    tracing::warn!("failed to save repo path: {e}");
                }
                let paths = self.database.list_repo_paths().unwrap_or_else(|e| {
                    tracing::warn!("failed to list repo paths: {e}");
                    vec![]
                });
```

- [ ] **Step 3: Fix `exec_edit_in_editor` silent failures**

In `exec_edit_in_editor`, replace the `if let Ok(exit) = status` block (lines 349-386) with:

```rust
        match status {
            Ok(exit) if exit.success() => {
                if let Ok(edited) = std::fs::read_to_string(tmp.path()) {
                    let mut title = task.title.clone();
                    let mut description = task.description.clone();
                    let mut repo_path = task.repo_path.clone();
                    let mut new_status = task.status;
                    let fields = parse_editor_content(&edited);
                    if !fields.title.is_empty() {
                        title = fields.title;
                    }
                    if !fields.description.is_empty() {
                        description = fields.description;
                    }
                    if !fields.repo_path.is_empty() {
                        repo_path = fields.repo_path;
                    }
                    if let Some(s) = models::TaskStatus::parse(&fields.status) {
                        new_status = s;
                    }
                    let plan = if fields.plan.is_empty() { None } else { Some(fields.plan) };

                    if let Err(e) = self.database.update_task(
                        task_id, &title, &description, &repo_path, new_status, plan.as_deref(),
                    ) {
                        app.update(Message::Error(format!("DB error updating task: {e}")));
                    }
                    app.update(Message::TaskEdited {
                        id: task_id,
                        title,
                        description,
                        repo_path,
                        status: new_status,
                        plan,
                    });
                } else {
                    tracing::warn!(task_id, "failed to read edited temp file");
                }
            }
            Ok(exit) => {
                tracing::warn!(task_id, ?exit, "editor exited with non-zero status");
            }
            Err(e) => {
                tracing::warn!(task_id, "failed to spawn editor: {e}");
            }
        }
```

- [ ] **Step 4: Add comment on first `tx.send()` pattern**

Find the first `let _ = tx.send(...)` in `exec_dispatch` (around line 251) and add a comment above it:

```rust
                    // receiver dropped = app shutting down; nothing to log
                    let _ = tx.send(Message::Dispatched {
```

- [ ] **Step 5: Run all tests**

Run: `cargo test`
Expected: All tests PASS

- [ ] **Step 6: Commit**

```bash
git add src/runtime.rs
git commit -m "fix: surface swallowed errors with tracing::warn instead of silent discard"
```

---

### Task 6: Extract `dispatch_with_prompt` in dispatch.rs

**Files:**
- Modify: `src/dispatch.rs:49-113`

- [ ] **Step 1: Run existing dispatch tests to establish baseline**

Run: `cargo test --lib dispatch`
Expected: All tests PASS

- [ ] **Step 2: Extract the shared helper**

Add a new private function after `provision_worktree` (before `dispatch_agent`):

```rust
/// Provision worktree, write prompt file, launch Claude via tmux.
/// Shared by all dispatch variants.
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

    tracing::info!(task_id = task.id, worktree = %provision.worktree_path, "agent dispatched");

    Ok(DispatchResult {
        worktree_path: provision.worktree_path,
        tmux_window: provision.tmux_window,
    })
}
```

- [ ] **Step 3: Rewrite the three public functions to use the helper**

Replace `dispatch_agent`:

```rust
pub fn dispatch_agent(task: &Task, mcp_port: u16, runner: &dyn ProcessRunner) -> Result<DispatchResult> {
    let prompt = build_prompt(task.id, &task.title, &task.description, task.plan.as_deref());
    dispatch_with_prompt(task, &prompt, runner)
}
```

Replace `brainstorm_agent`:

```rust
pub fn brainstorm_agent(task: &Task, mcp_port: u16, runner: &dyn ProcessRunner) -> Result<DispatchResult> {
    let prompt = build_brainstorm_prompt(task.id, &task.title, &task.description, mcp_port);
    dispatch_with_prompt(task, &prompt, runner)
}
```

Replace `quick_dispatch_agent`:

```rust
pub fn quick_dispatch_agent(task: &Task, mcp_port: u16, runner: &dyn ProcessRunner) -> Result<DispatchResult> {
    let prompt = build_quick_dispatch_prompt(task.id, &task.title, &task.description, mcp_port);
    dispatch_with_prompt(task, &prompt, runner)
}
```

- [ ] **Step 4: Run all dispatch tests**

Run: `cargo test --lib dispatch`
Expected: All tests PASS (public API unchanged)

- [ ] **Step 5: Commit**

```bash
git add src/dispatch.rs
git commit -m "refactor: extract dispatch_with_prompt to eliminate dispatch triplication"
```

---

### Task 7: Extract `spawn_dispatch` in runtime.rs

**Files:**
- Modify: `src/runtime.rs:241-285`

- [ ] **Step 1: Run full test suite as baseline**

Run: `cargo test`
Expected: All tests PASS

- [ ] **Step 2: Add the `spawn_dispatch` helper method**

Add to `impl TuiRuntime`, before `exec_dispatch`:

```rust
    fn spawn_dispatch<F>(&self, task: models::Task, dispatch_fn: F, label: &'static str)
    where
        F: FnOnce(&models::Task, u16, &dyn ProcessRunner) -> Result<models::DispatchResult>
            + Send
            + 'static,
    {
        let tx = self.msg_tx.clone();
        let port = self.port;
        let runner = self.runner.clone();

        tokio::task::spawn_blocking(move || {
            let id = task.id;
            tracing::info!(task_id = id, label, "dispatching");
            match dispatch_fn(&task, port, &*runner) {
                Ok(result) => {
                    // receiver dropped = app shutting down; nothing to log
                    let _ = tx.send(Message::Dispatched {
                        id,
                        worktree: result.worktree_path,
                        tmux_window: result.tmux_window,
                    });
                }
                Err(e) => {
                    let _ = tx.send(Message::Error(format!("{label} failed: {e:#}")));
                }
            }
        });
    }
```

- [ ] **Step 3: Replace `exec_dispatch` and `exec_brainstorm`**

Replace both methods with:

```rust
    fn exec_dispatch(&self, task: models::Task) {
        self.spawn_dispatch(task, dispatch::dispatch_agent, "Dispatch");
    }

    fn exec_brainstorm(&self, task: models::Task) {
        self.spawn_dispatch(task, dispatch::brainstorm_agent, "Brainstorm");
    }
```

- [ ] **Step 4: Run full test suite**

Run: `cargo test`
Expected: All tests PASS

- [ ] **Step 5: Run clippy**

Run: `cargo clippy`
Expected: No warnings

- [ ] **Step 6: Commit**

```bash
git add src/runtime.rs
git commit -m "refactor: extract spawn_dispatch to eliminate exec_dispatch/exec_brainstorm duplication"
```

---

### Task 8: Extract `finish_task_creation` in input.rs

**Files:**
- Modify: `src/tui/input.rs:192-244`

- [ ] **Step 1: Run input-related tests as baseline**

Run: `cargo test --lib tui`
Expected: All tests PASS

- [ ] **Step 2: Add the helper method**

Add to `impl App` in `src/tui/input.rs` (after `handle_key_quick_dispatch`, before the closing `}`):

```rust
    fn finish_task_creation(&mut self, repo_path: String) -> Vec<Command> {
        let draft = self.task_draft.take().unwrap_or_default();
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

- [ ] **Step 3: Replace the first occurrence (Enter on InputRepoPath)**

In `handle_key_text_input`, replace the `InputMode::InputRepoPath` match arm (lines 192-216):

```rust
                    InputMode::InputRepoPath => {
                        let repo_path = if value.is_empty() {
                            if let Some(first) = self.repo_paths.first() {
                                first.clone()
                            } else {
                                let draft = self.task_draft.take().unwrap_or_default();
                                self.task_draft = Some(draft);
                                self.status_message =
                                    Some("Repo path required (no saved paths available)".to_string());
                                return vec![];
                            }
                        } else {
                            value
                        };
                        self.finish_task_creation(repo_path)
                    }
```

Note: The empty-buffer-no-paths error branch needs to restore the draft. The original code called `self.task_draft.take().unwrap_or_default()` then put it back — keep that behavior but move it before the helper call since `finish_task_creation` takes the draft.

- [ ] **Step 4: Replace the second occurrence (digit shortcut)**

In the `KeyCode::Char(c)` arm (around lines 226-245), replace the block that creates InsertTask+SaveRepoPath:

```rust
                if self.mode == InputMode::InputRepoPath
                    && self.input_buffer.is_empty()
                    && c.is_ascii_digit()
                    && c != '0'
                {
                    let idx = (c as usize) - ('1' as usize);
                    if idx < self.repo_paths.len() {
                        let repo_path = self.repo_paths[idx].clone();
                        return self.finish_task_creation(repo_path);
                    }
                }
```

- [ ] **Step 5: Run TUI tests**

Run: `cargo test --lib tui`
Expected: All tests PASS

- [ ] **Step 6: Commit**

```bash
git add src/tui/input.rs
git commit -m "refactor: extract finish_task_creation to deduplicate InsertTask+SaveRepoPath"
```

---

### Task 9: Add repo_paths DB tests

**Files:**
- Modify: `src/db.rs` (test module)

- [ ] **Step 1: Write the tests**

Add to the test module in `src/db.rs`:

```rust
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
```

- [ ] **Step 2: Run the tests**

Run: `cargo test repo_path`
Expected: All 3 tests PASS (testing existing functionality)

- [ ] **Step 3: Commit**

```bash
git add src/db.rs
git commit -m "test: add tests for save_repo_path and list_repo_paths"
```

---

### Task 10: Add `brainstorm_agent` and `quick_dispatch_agent` tests

**Files:**
- Modify: `src/dispatch.rs` (test module)

- [ ] **Step 1: Add brainstorm_agent test**

Add to the test module in `src/dispatch.rs`:

```rust
    #[test]
    fn brainstorm_creates_worktree_then_opens_tmux() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo_path = dir.path().to_str().unwrap().to_string();
        let worktree_dir = dir.path().join(".worktrees").join("42-fix-bug");
        std::fs::create_dir_all(&worktree_dir).unwrap();

        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // git worktree add
            MockProcessRunner::ok(), // tmux new-window
            MockProcessRunner::ok(), // tmux send-keys -l
            MockProcessRunner::ok(), // tmux send-keys Enter
        ]);

        let task = make_task(&repo_path);
        brainstorm_agent(&task, 3142, &mock).unwrap();

        let calls = mock.recorded_calls();
        assert_eq!(calls[0].0, "git", "first call should be git");
        assert!(calls[0].1.contains(&"worktree".to_string()));
        assert_eq!(calls[1].0, "tmux");
        assert_eq!(calls[1].1[0], "new-window");
    }

    #[test]
    fn brainstorm_sends_brainstorm_prompt() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo_path = dir.path().to_str().unwrap().to_string();
        let worktree_dir = dir.path().join(".worktrees").join("42-fix-bug");
        std::fs::create_dir_all(&worktree_dir).unwrap();

        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // git worktree add
            MockProcessRunner::ok(), // tmux new-window
            MockProcessRunner::ok(), // tmux send-keys -l
            MockProcessRunner::ok(), // tmux send-keys Enter
        ]);

        let task = make_task(&repo_path);
        brainstorm_agent(&task, 3142, &mock).unwrap();

        // Verify the prompt file was written with brainstorm content
        let prompt_file = worktree_dir.join(".claude-prompt");
        let prompt = std::fs::read_to_string(prompt_file).unwrap();
        assert!(prompt.contains("brainstorm"), "prompt should mention brainstorming");
        assert!(prompt.contains("implementation plan"), "prompt should mention planning");
    }
```

- [ ] **Step 2: Add quick_dispatch_agent test**

```rust
    #[test]
    fn quick_dispatch_creates_worktree_then_opens_tmux() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo_path = dir.path().to_str().unwrap().to_string();
        let worktree_dir = dir.path().join(".worktrees").join("42-fix-bug");
        std::fs::create_dir_all(&worktree_dir).unwrap();

        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // git worktree add
            MockProcessRunner::ok(), // tmux new-window
            MockProcessRunner::ok(), // tmux send-keys -l
            MockProcessRunner::ok(), // tmux send-keys Enter
        ]);

        let task = make_task(&repo_path);
        quick_dispatch_agent(&task, 3142, &mock).unwrap();

        let calls = mock.recorded_calls();
        assert_eq!(calls[0].0, "git");
        assert!(calls[0].1.contains(&"worktree".to_string()));
        assert_eq!(calls[1].0, "tmux");
        assert_eq!(calls[1].1[0], "new-window");
    }

    #[test]
    fn quick_dispatch_sends_rename_prompt() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo_path = dir.path().to_str().unwrap().to_string();
        let worktree_dir = dir.path().join(".worktrees").join("42-fix-bug");
        std::fs::create_dir_all(&worktree_dir).unwrap();

        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // git worktree add
            MockProcessRunner::ok(), // tmux new-window
            MockProcessRunner::ok(), // tmux send-keys -l
            MockProcessRunner::ok(), // tmux send-keys Enter
        ]);

        let task = make_task(&repo_path);
        quick_dispatch_agent(&task, 3142, &mock).unwrap();

        let prompt_file = worktree_dir.join(".claude-prompt");
        let prompt = std::fs::read_to_string(prompt_file).unwrap();
        assert!(prompt.contains("placeholder"), "prompt should mention placeholder title");
        assert!(prompt.contains("update_task"), "prompt should mention update_task for rename");
    }
```

- [ ] **Step 3: Run the tests**

Run: `cargo test --lib dispatch`
Expected: All tests PASS

- [ ] **Step 4: Commit**

```bash
git add src/dispatch.rs
git commit -m "test: add brainstorm_agent and quick_dispatch_agent integration tests"
```

---

### Task 11: Add basic runtime tests

**Files:**
- Modify: `src/runtime.rs` (add test module at bottom)

- [ ] **Step 1: Add the test module**

Add at the bottom of `src/runtime.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::process::MockProcessRunner;

    fn test_runtime() -> (TuiRuntime, App) {
        let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().unwrap());
        let (tx, _rx) = mpsc::unbounded_channel();
        let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
        let rt = TuiRuntime {
            database: db.clone(),
            msg_tx: tx,
            port: 3142,
            input_paused: Arc::new(AtomicBool::new(false)),
            runner,
        };
        let tasks = db.list_all().unwrap();
        let app = App::new(tasks);
        (rt, app)
    }

    #[test]
    fn exec_insert_task_adds_to_db_and_app() {
        let (rt, mut app) = test_runtime();
        rt.exec_insert_task(&mut app, "Test".into(), "Desc".into(), "/repo".into());
        assert_eq!(app.tasks().len(), 1);
        assert_eq!(app.tasks()[0].title, "Test");
        assert_eq!(rt.database.list_all().unwrap().len(), 1);
    }

    #[test]
    fn exec_delete_task_removes_from_db() {
        let (rt, mut app) = test_runtime();
        rt.exec_insert_task(&mut app, "Test".into(), "Desc".into(), "/repo".into());
        let id = app.tasks()[0].id;
        rt.exec_delete_task(&mut app, id);
        assert!(rt.database.list_all().unwrap().is_empty());
    }

    #[test]
    fn exec_persist_task_saves_status_to_db() {
        let (rt, mut app) = test_runtime();
        rt.exec_insert_task(&mut app, "Test".into(), "Desc".into(), "/repo".into());
        let mut task = app.tasks()[0].clone();
        task.status = models::TaskStatus::Ready;
        task.worktree = Some("/repo/.worktrees/1-test".into());
        rt.exec_persist_task(&mut app, task);
        let db_task = rt.database.get_task(app.tasks()[0].id).unwrap().unwrap();
        assert_eq!(db_task.status, models::TaskStatus::Ready);
        assert_eq!(db_task.worktree.as_deref(), Some("/repo/.worktrees/1-test"));
    }

    #[test]
    fn exec_save_repo_path_updates_app_state() {
        let (rt, mut app) = test_runtime();
        rt.exec_save_repo_path(&mut app, "/repo".into());
        assert!(app.repo_paths().contains(&"/repo".to_string()));
    }

    #[test]
    fn exec_refresh_from_db_syncs_external_changes() {
        let (rt, mut app) = test_runtime();
        // Insert directly into DB, bypassing app
        rt.database
            .create_task("External", "Added via CLI", "/repo", None, models::TaskStatus::Backlog)
            .unwrap();
        assert!(app.tasks().is_empty());
        rt.exec_refresh_from_db(&mut app);
        assert_eq!(app.tasks().len(), 1);
        assert_eq!(app.tasks()[0].title, "External");
    }
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test --lib runtime`
Expected: All 5 tests PASS

- [ ] **Step 3: Commit**

```bash
git add src/runtime.rs
git commit -m "test: add runtime exec method unit tests"
```

---

### Task 12: `TaskId` newtype

**Files:**
- Modify: `src/models.rs` (add newtype)
- Modify: `src/tui/types.rs` (Message/Command variants)
- Modify: `src/tui/mod.rs` (App fields, handler methods)
- Modify: `src/tui/input.rs` (key handlers)
- Modify: `src/tui/ui.rs` (rendering)
- Modify: `src/tui/tests.rs` (test helpers)
- Modify: `src/db.rs` (TaskStore trait + impl + tests)
- Modify: `src/dispatch.rs` (function params + tests)
- Modify: `src/runtime.rs` (exec methods + tests)
- Modify: `src/mcp/handlers.rs` (parse + wrap)
- Modify: `src/editor.rs` (not needed — editor doesn't use task IDs)
- Modify: `src/main.rs` (CLI commands)
- Modify: `tests/cli.rs` (if task IDs in output change)
- Modify: `tests/lifecycle.rs` (wrap IDs)

This is a compiler-guided mechanical refactor. The approach: define the type, change `Task.id`, then fix every compile error.

- [ ] **Step 1: Define `TaskId` in models.rs**

Add after the `TaskStatus` impl block, before the `Task` struct:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskId(pub i64);

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
```

Change `Task.id` from `pub id: i64` to `pub id: TaskId`.

- [ ] **Step 2: Fix `src/db.rs`**

In the `TaskStore` trait, change every `id: i64` parameter to `id: TaskId` for these methods: `get_task`, `update_status`, `update_dispatch`, `persist_task`, `delete_task`, `update_task`, `update_plan`, `update_title_description`, `update_task_partial`.

`create_task` returns `Result<i64>` — change to `Result<TaskId>`. Update the impl: `Ok(TaskId(conn.last_insert_rowid()))`.

In every `Database` method body, where `id` is passed to SQL params, use `id.0` instead:
```rust
params![id.0]
```

In `row_to_task`, change:
```rust
id: row.get("id")?,
```
To:
```rust
id: TaskId(row.get("id")?),
```

In error messages, `{id}` works because `TaskId` implements `Display`.

Update all tests to use `TaskId(n)` where integer IDs are used.

- [ ] **Step 3: Fix `src/tui/types.rs`**

Change every `id: i64` in `Message` and `Command` variants to `id: TaskId`:

```rust
    MoveTask { id: TaskId, direction: MoveDirection },
    DispatchTask(TaskId),
    BrainstormTask(TaskId),
    Dispatched { id: TaskId, worktree: String, tmux_window: String },
    DeleteTask(TaskId),
    TmuxOutput { id: TaskId, output: String },
    WindowGone(TaskId),
    ResumeTask(TaskId),
    Resumed { id: TaskId, tmux_window: String },
    TaskEdited { id: TaskId, title: String, description: String, repo_path: String, status: TaskStatus, plan: Option<String> },
```

```rust
    DeleteTask(TaskId),
    CaptureTmux { id: TaskId, window: String },
```

Add `use crate::models::TaskId;` at the top of types.rs.

- [ ] **Step 4: Fix `src/tui/mod.rs`, `input.rs`, `ui.rs`**

Follow compile errors. In `mod.rs`, the `tmux_outputs: HashMap<i64, String>` becomes `HashMap<TaskId, String>`. Handler methods that extract `id` from tasks already get `TaskId` from `task.id`.

In `input.rs`, `task.id` is already `TaskId` so no logic changes needed, just type annotations if any are explicit.

In `ui.rs`, anywhere `task.id` is formatted, `TaskId::Display` handles it.

- [ ] **Step 5: Fix `src/dispatch.rs`**

Change `build_tmux_window_name(task_id: i64)` to `build_tmux_window_name(task_id: TaskId)`. In the format string, `{task_id}` works via `Display`.

Change `build_prompt`, `build_quick_dispatch_prompt`, `build_brainstorm_prompt` first param from `task_id: i64` to `task_id: TaskId`.

Change `resume_agent(task_id: i64, ...)` to `resume_agent(task_id: TaskId, ...)`.

Update all tests to use `TaskId(42)`, `TaskId(1)`, `TaskId(7)` etc.

- [ ] **Step 6: Fix `src/runtime.rs`**

`exec_delete_task(&self, app: &mut App, id: i64)` → `id: TaskId`. And similar for any other method that takes a bare ID.

Update test helpers to match.

- [ ] **Step 7: Fix `src/mcp/handlers.rs`**

The `UpdateTaskArgs`, `GetTaskArgs`, `CreateTaskArgs` use `task_id: i64` — change to keep `i64` in the serde struct (it comes from JSON), but wrap in `TaskId` when calling DB methods:

```rust
state.db.get_task(TaskId(parsed.task_id))
```

Same for `update_task_partial`, `update_status`, etc.

- [ ] **Step 8: Fix `src/main.rs`**

CLI commands that pass task IDs: wrap in `TaskId(id)`.

- [ ] **Step 9: Fix integration tests**

Update `tests/cli.rs` and `tests/lifecycle.rs` as needed.

- [ ] **Step 10: Run full test suite**

Run: `cargo test`
Expected: All tests PASS

- [ ] **Step 11: Run clippy**

Run: `cargo clippy`
Expected: No warnings

- [ ] **Step 12: Commit**

```bash
git add src/ tests/
git commit -m "refactor: introduce TaskId newtype replacing raw i64 task identifiers"
```

---

### Task 13: `format_editor_content` takes `&Task`

**Files:**
- Modify: `src/editor.rs:9-13`
- Modify: `src/runtime.rs` (call site in exec_edit_in_editor)
- Modify: `src/editor.rs` (tests)

- [ ] **Step 1: Update the tests**

In `src/editor.rs`, update tests to construct `Task` values. Add `use crate::models::{Task, TaskStatus, TaskId};` and `use chrono::Utc;` at the top of the test module.

```rust
    fn make_test_task(title: &str, description: &str, repo_path: &str, status: TaskStatus, plan: Option<&str>) -> Task {
        Task {
            id: TaskId(1),
            title: title.to_string(),
            description: description.to_string(),
            repo_path: repo_path.to_string(),
            status,
            worktree: None,
            tmux_window: None,
            plan: plan.map(|s| s.to_string()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn editor_roundtrip_basic() {
        let task = make_test_task("My Task", "A description", "/repo", TaskStatus::Ready, Some("docs/plan.md"));
        let content = format_editor_content(&task);
        let fields = parse_editor_content(&content);
        assert_eq!(fields.title, "My Task");
        assert_eq!(fields.description, "A description");
        assert_eq!(fields.repo_path, "/repo");
        assert_eq!(fields.status, "ready");
        assert_eq!(fields.plan, "docs/plan.md");
    }

    #[test]
    fn editor_roundtrip_colons_in_title() {
        let task = make_test_task("Fix: auth bug", "desc", "/repo", TaskStatus::Backlog, None);
        let content = format_editor_content(&task);
        let fields = parse_editor_content(&content);
        assert_eq!(fields.title, "Fix: auth bug");
    }

    #[test]
    fn editor_roundtrip_colons_in_description() {
        let task = make_test_task("Title", "Step 1: do this\nStep 2: do that", "/repo", TaskStatus::Ready, None);
        let content = format_editor_content(&task);
        let fields = parse_editor_content(&content);
        assert_eq!(fields.description, "Step 1: do this\nStep 2: do that");
    }

    #[test]
    fn editor_multiline_description() {
        let task = make_test_task("Title", "Line 1\nLine 2\nLine 3", "/repo", TaskStatus::Done, None);
        let content = format_editor_content(&task);
        let fields = parse_editor_content(&content);
        assert_eq!(fields.description, "Line 1\nLine 2\nLine 3");
    }
```

Keep the `editor_unknown_section_ignored` test as-is (it tests `parse_editor_content` directly with raw input, doesn't call `format_editor_content`).

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib editor`
Expected: FAIL — `format_editor_content` still takes 5 params

- [ ] **Step 3: Change the function signature**

```rust
use crate::models::Task;

pub fn format_editor_content(task: &Task) -> String {
    format!(
        "--- TITLE ---\n{}\n--- DESCRIPTION ---\n{}\n--- REPO_PATH ---\n{}\n--- STATUS ---\n{}\n--- PLAN ---\n{}\n",
        task.title,
        task.description,
        task.repo_path,
        task.status.as_str(),
        task.plan.as_deref().unwrap_or(""),
    )
}
```

- [ ] **Step 4: Update the call site in runtime.rs**

In `exec_edit_in_editor`, replace:

```rust
        let content = format_editor_content(
            &task.title,
            &task.description,
            &task.repo_path,
            task.status.as_str(),
            task.plan.as_deref().unwrap_or(""),
        );
```

With:

```rust
        let content = format_editor_content(&task);
```

- [ ] **Step 5: Run all tests**

Run: `cargo test`
Expected: All tests PASS

- [ ] **Step 6: Commit**

```bash
git add src/editor.rs src/runtime.rs
git commit -m "refactor: format_editor_content takes &Task instead of 5 params"
```

---

### Task 14: Simplify `Command::InsertTask` with `TaskDraft`

**Files:**
- Modify: `src/tui/types.rs` (TaskDraft + Command)
- Modify: `src/tui/input.rs` (finish_task_creation)
- Modify: `src/runtime.rs` (exec_insert_task, exec_quick_dispatch)
- Modify: `src/tui/tests.rs` (any tests that construct InsertTask)

- [ ] **Step 1: Add `repo_path` to `TaskDraft`**

In `src/tui/types.rs`:

```rust
#[derive(Debug, Clone, Default)]
pub struct TaskDraft {
    pub title: String,
    pub description: String,
    pub repo_path: String,
}
```

Change `Command::InsertTask`:

```rust
    InsertTask(TaskDraft),
```

Also update `Command::QuickDispatch` to use `TaskDraft`:

```rust
    QuickDispatch(TaskDraft),
```

- [ ] **Step 2: Update `finish_task_creation` in input.rs**

```rust
    fn finish_task_creation(&mut self, repo_path: String) -> Vec<Command> {
        let mut draft = self.task_draft.take().unwrap_or_default();
        draft.repo_path = repo_path.clone();
        self.mode = InputMode::Normal;
        self.status_message = None;
        vec![
            Command::InsertTask(draft),
            Command::SaveRepoPath(repo_path),
        ]
    }
```

- [ ] **Step 3: Update QuickDispatch message handler**

In `src/tui/mod.rs`, find the `Message::QuickDispatch` handler. It currently returns `Command::QuickDispatch { title, description, repo_path }`. Update to:

```rust
Command::QuickDispatch(TaskDraft {
    title,
    description: String::new(),
    repo_path,
})
```

Where `title` is generated as before (the placeholder title).

- [ ] **Step 4: Update runtime exec methods**

In `src/runtime.rs`, update `exec_insert_task`:

```rust
    fn exec_insert_task(&self, app: &mut App, draft: TaskDraft) {
        match self.database.create_task(&draft.title, &draft.description, &draft.repo_path, None, models::TaskStatus::Backlog) {
```

And all internal references from `title`/`description`/`repo_path` to `draft.title`/`draft.description`/`draft.repo_path`.

Update `execute_commands` to match:

```rust
    Command::InsertTask(draft) => rt.exec_insert_task(app, draft),
    Command::QuickDispatch(draft) => rt.exec_quick_dispatch(app, draft),
```

Update `exec_quick_dispatch` signature similarly.

- [ ] **Step 5: Update tests**

In `src/tui/tests.rs`, update any tests that match on `Command::InsertTask { title, description, repo_path }` to match on `Command::InsertTask(draft)` and assert against `draft.title`, `draft.description`, `draft.repo_path`.

In `src/runtime.rs` tests, update `exec_insert_task` calls:

```rust
rt.exec_insert_task(&mut app, TaskDraft {
    title: "Test".into(),
    description: "Desc".into(),
    repo_path: "/repo".into(),
});
```

- [ ] **Step 6: Run all tests**

Run: `cargo test`
Expected: All tests PASS

- [ ] **Step 7: Run clippy**

Run: `cargo clippy`
Expected: No warnings

- [ ] **Step 8: Commit**

```bash
git add src/tui/types.rs src/tui/input.rs src/tui/mod.rs src/runtime.rs src/tui/tests.rs
git commit -m "refactor: use TaskDraft in Command::InsertTask and Command::QuickDispatch"
```

---

### Task 15: Final verification

- [ ] **Step 1: Run full test suite**

Run: `cargo test`
Expected: All tests PASS

- [ ] **Step 2: Run clippy**

Run: `cargo clippy`
Expected: No warnings

- [ ] **Step 3: Build release**

Run: `cargo build --release`
Expected: Compiles successfully
