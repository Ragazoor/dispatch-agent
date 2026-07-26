# Automatic Task Status Hooks — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Automatically manage task status transitions via Claude Code hooks so agents never forget to update the kanban board.

**Architecture:** Dispatch writes `.claude/settings.json` into each worktree with `Stop` (→ review) and `UserPromptSubmit` (→ running) hooks that call `task-orchestrator update`. The TUI also sets status to Running on resume. The agent prompt is simplified to tell agents that status is managed automatically.

**Tech Stack:** Rust, Claude Code hooks (settings.json), `task-orchestrator` CLI

**Spec:** `docs/superpowers/specs/2026-03-26-automatic-task-status-hooks-design.md`

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `src/dispatch.rs` | Modify | Add `db_path` param, write `.claude/settings.json`, update `build_prompt` |
| `src/tui/mod.rs` | Modify | Set `task.status = Running` in `handle_resumed` |
| `src/tui/tests.rs` | Modify | Add test for resumed-sets-running |
| `src/runtime.rs` | Modify | Store `db_path` in `TuiRuntime`, pass to `dispatch_agent` |

---

### Task 1: Resumed handler sets status to Running

**Files:**
- Modify: `src/tui/tests.rs`
- Modify: `src/tui/mod.rs:321-329` (`handle_resumed`)

- [ ] **Step 1: Write the failing test**

Add to the end of `src/tui/tests.rs`:

```rust
#[test]
fn resumed_sets_status_to_running() {
    let mut task = make_task(4, TaskStatus::Review);
    task.worktree = Some("/repo/.worktrees/4-task-4".to_string());
    task.tmux_window = None;
    let mut app = App::new(vec![task]);

    let cmds = app.update(Message::Resumed {
        id: 4,
        tmux_window: "task-4".to_string(),
    });

    let task = app.tasks.iter().find(|t| t.id == 4).unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.tmux_window.as_deref(), Some("task-4"));
    assert_eq!(cmds.len(), 1);
    assert!(matches!(&cmds[0], Command::PersistTask(t) if t.status == TaskStatus::Running));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test resumed_sets_status_to_running`
Expected: FAIL — the assertion `assert_eq!(task.status, TaskStatus::Running)` fails because `handle_resumed` does not set status.

- [ ] **Step 3: Implement — set status in handle_resumed**

In `src/tui/mod.rs`, change `handle_resumed` (lines 321-329) from:

```rust
fn handle_resumed(&mut self, id: i64, tmux_window: String) -> Vec<Command> {
    if let Some(task) = self.find_task_mut(id) {
        task.tmux_window = Some(tmux_window);
        let task_clone = task.clone();
        vec![Command::PersistTask(task_clone)]
    } else {
        vec![]
    }
}
```

to:

```rust
fn handle_resumed(&mut self, id: i64, tmux_window: String) -> Vec<Command> {
    if let Some(task) = self.find_task_mut(id) {
        task.tmux_window = Some(tmux_window);
        task.status = TaskStatus::Running;
        let task_clone = task.clone();
        self.clamp_selection();
        vec![Command::PersistTask(task_clone)]
    } else {
        vec![]
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/tui/mod.rs src/tui/tests.rs
git commit -m "feat: set task status to Running on resume"
```

---

### Task 2: Thread db_path into TuiRuntime and dispatch

**Files:**
- Modify: `src/runtime.rs:109-114` (`TuiRuntime` struct)
- Modify: `src/runtime.rs:80-86` (`TuiRuntime` construction in `run_tui`)
- Modify: `src/runtime.rs:155-174` (`exec_dispatch`)
- Modify: `src/dispatch.rs:17` (`dispatch_agent` signature)

- [ ] **Step 1: Add `db_path` field to `TuiRuntime`**

In `src/runtime.rs`, change the `TuiRuntime` struct (lines 109-114) from:

```rust
struct TuiRuntime {
    database: Arc<dyn db::TaskStore>,
    msg_tx: mpsc::UnboundedSender<Message>,
    port: u16,
    input_paused: Arc<AtomicBool>,
}
```

to:

```rust
struct TuiRuntime {
    database: Arc<dyn db::TaskStore>,
    msg_tx: mpsc::UnboundedSender<Message>,
    port: u16,
    db_path: PathBuf,
    input_paused: Arc<AtomicBool>,
}
```

- [ ] **Step 2: Store `db_path` when constructing TuiRuntime**

In `src/runtime.rs`, change the `TuiRuntime` construction (lines 81-86) from:

```rust
    let runtime = TuiRuntime {
        database,
        msg_tx,
        port,
        input_paused,
    };
```

to:

```rust
    let runtime = TuiRuntime {
        database,
        msg_tx,
        port,
        db_path: db_path.to_path_buf(),
        input_paused,
    };
```

- [ ] **Step 3: Add `db_path` parameter to `dispatch_agent`**

In `src/dispatch.rs`, change the signature (line 17) from:

```rust
pub fn dispatch_agent(task: &Task, mcp_port: u16) -> Result<DispatchResult> {
```

to:

```rust
pub fn dispatch_agent(task: &Task, mcp_port: u16, db_path: &str) -> Result<DispatchResult> {
```

The `db_path` parameter is not used yet — that comes in Task 3. This step just threads it through.

- [ ] **Step 4: Pass `db_path` from `exec_dispatch` to `dispatch_agent`**

In `src/runtime.rs`, change `exec_dispatch` (lines 155-174) from:

```rust
    fn exec_dispatch(&self, task: models::Task) {
        let tx = self.msg_tx.clone();
        let port = self.port;

        tokio::task::spawn_blocking(move || {
            let id = task.id;
            match dispatch::dispatch_agent(&task, port) {
```

to:

```rust
    fn exec_dispatch(&self, task: models::Task) {
        let tx = self.msg_tx.clone();
        let port = self.port;
        let db_path = self.db_path.to_string_lossy().to_string();

        tokio::task::spawn_blocking(move || {
            let id = task.id;
            match dispatch::dispatch_agent(&task, port, &db_path) {
```

- [ ] **Step 5: Verify it compiles and tests pass**

Run: `cargo test`
Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/runtime.rs src/dispatch.rs
git commit -m "refactor: thread db_path into TuiRuntime and dispatch_agent"
```

---

### Task 3: Write Claude Code hooks into the worktree

**Files:**
- Modify: `src/dispatch.rs:17-75` (`dispatch_agent`)

- [ ] **Step 1: Write the failing test for hook config generation**

Add to the `tests` module in `src/dispatch.rs`:

```rust
#[test]
fn build_hooks_config_contains_task_id_and_db_path() {
    let config = build_hooks_config(42, "/home/user/.local/share/task-orchestrator/tasks.db");
    let parsed: serde_json::Value = serde_json::from_str(&config)
        .expect("hooks config should be valid JSON");

    let stop_cmd = parsed["hooks"]["Stop"][0]["command"].as_str().unwrap();
    assert!(stop_cmd.contains("update 42 review"), "Stop hook should update to review");
    assert!(stop_cmd.contains("--db /home/user/.local/share/task-orchestrator/tasks.db"));

    let submit_cmd = parsed["hooks"]["UserPromptSubmit"][0]["command"].as_str().unwrap();
    assert!(submit_cmd.contains("update 42 running"), "UserPromptSubmit hook should update to running");
    assert!(submit_cmd.contains("--db /home/user/.local/share/task-orchestrator/tasks.db"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test build_hooks_config_contains_task_id`
Expected: FAIL — `build_hooks_config` does not exist.

- [ ] **Step 3: Implement `build_hooks_config`**

Add this helper function in `src/dispatch.rs` (above `build_prompt`):

```rust
fn build_hooks_config(task_id: i64, db_path: &str) -> String {
    format!(
        r#"{{"hooks":{{"Stop":[{{"type":"command","command":"task-orchestrator --db {db_path} update {task_id} review"}}],"UserPromptSubmit":[{{"type":"command","command":"task-orchestrator --db {db_path} update {task_id} running"}}]}}}}"#
    )
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test build_hooks_config_contains_task_id`
Expected: PASS

- [ ] **Step 5: Write the hooks config in `dispatch_agent`**

In `src/dispatch.rs`, in `dispatch_agent`, after step 3 (writing `.mcp.json`, around line 57), add:

```rust
    // 3b. Write .claude/settings.json with status hooks.
    let claude_dir = format!("{worktree_path}/.claude");
    fs::create_dir_all(&claude_dir)
        .with_context(|| format!("failed to create {claude_dir}"))?;
    let hooks_config = build_hooks_config(task.id, db_path);
    fs::write(format!("{claude_dir}/settings.json"), &hooks_config)
        .with_context(|| format!("failed to write {claude_dir}/settings.json"))?;
```

- [ ] **Step 6: Verify it compiles and tests pass**

Run: `cargo test`
Expected: All tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/dispatch.rs
git commit -m "feat: write Claude Code hooks into worktree for automatic status transitions"
```

---

### Task 4: Simplify the agent prompt

**Files:**
- Modify: `src/dispatch.rs:164-186` (`build_prompt` and its tests)

- [ ] **Step 1: Update the `build_prompt_contains_task_info` test**

In `src/dispatch.rs`, change `build_prompt_contains_task_info` from:

```rust
    #[test]
    fn build_prompt_contains_task_info() {
        let prompt = build_prompt(42, "Fix bug", "A nasty crash", 3142, None);
        assert!(prompt.contains("42"));
        assert!(prompt.contains("Fix bug"));
        assert!(prompt.contains("A nasty crash"));
        assert!(prompt.contains("3142"));
        assert!(prompt.contains("review"));
    }
```

to:

```rust
    #[test]
    fn build_prompt_contains_task_info() {
        let prompt = build_prompt(42, "Fix bug", "A nasty crash", 3142, None);
        assert!(prompt.contains("42"));
        assert!(prompt.contains("Fix bug"));
        assert!(prompt.contains("A nasty crash"));
        assert!(prompt.contains("3142"));
        assert!(prompt.contains("automatically via hooks"));
    }
```

- [ ] **Step 2: Update the `build_prompt_contains_mcp_fallback` test**

Replace `build_prompt_contains_mcp_fallback` with:

```rust
    #[test]
    fn build_prompt_mentions_automatic_hooks() {
        let prompt = build_prompt(7, "Title", "Desc", 3142, None);
        assert!(prompt.contains("automatically via hooks"));
        assert!(!prompt.contains("update the task status to 'review'"));
    }
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test build_prompt`
Expected: FAIL — the old prompt text doesn't match the new assertions.

- [ ] **Step 4: Update `build_prompt`**

In `src/dispatch.rs`, change `build_prompt` (lines 164-186) from:

```rust
fn build_prompt(task_id: i64, title: &str, description: &str, mcp_port: u16, plan: Option<&str>) -> String {
    let plan_section = match plan {
        Some(path) => format!(
            "\n\nPlan: {path}\nRead this file for the full implementation plan. Follow it step by step."
        ),
        None => String::new(),
    };

    format!(
        "You are an autonomous coding agent. \
Your task is:\n\
  ID: {task_id}\n\
  Title: {title}\n\
  Description: {description}\
{plan_section}\n\
\n\
An MCP server is available at http://localhost:{mcp_port}/mcp — use it to \
update task status and post notes as you work (tool: task-orchestrator). \
When your work is complete, update the task status to 'review' via the MCP \
server. If MCP is unavailable, run: \
task-orchestrator update {task_id} review"
    )
}
```

to:

```rust
fn build_prompt(task_id: i64, title: &str, description: &str, mcp_port: u16, plan: Option<&str>) -> String {
    let plan_section = match plan {
        Some(path) => format!(
            "\n\nPlan: {path}\nRead this file for the full implementation plan. Follow it step by step."
        ),
        None => String::new(),
    };

    format!(
        "You are an autonomous coding agent. \
Your task is:\n\
  ID: {task_id}\n\
  Title: {title}\n\
  Description: {description}\
{plan_section}\n\
\n\
Task status transitions (running/review) are managed automatically via hooks. \
Do not call update_task for status changes. \
An MCP server is available at http://localhost:{mcp_port}/mcp — use it to \
post notes as you work (tool: task-orchestrator, tool name: add_note)."
    )
}
```

- [ ] **Step 5: Run all tests to verify they pass**

Run: `cargo test`
Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/dispatch.rs
git commit -m "feat: simplify agent prompt — status managed by hooks"
```

---

### Task 5: Final verification

- [ ] **Step 1: Run the full test suite**

Run: `cargo test`
Expected: All tests pass.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy`
Expected: No warnings.

- [ ] **Step 3: Verify the build**

Run: `cargo build`
Expected: Compiles cleanly.
