# Confirmation Before Done — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Require human confirmation when moving tasks to Done — both in the TUI (status bar y/n prompt) and via MCP (reject with message telling the agent to ask a human).

**Architecture:** Two independent guards: (1) a new `ConfirmDone` input mode that intercepts Review→Done transitions from the `m`/`M` keys, and (2) an early-return rejection in the MCP `handle_update_task` handler. Both follow existing patterns in the codebase.

**Tech Stack:** Rust, ratatui, axum (MCP)

---

### Task 1: Add `ConfirmDone` types

**Files:**
- Modify: `src/tui/types.rs:134-152` (InputMode enum)
- Modify: `src/tui/types.rs:20-92` (Message enum)

- [ ] **Step 1: Add `InputMode::ConfirmDone` variant**

In `src/tui/types.rs`, add `ConfirmDone(TaskId)` to the `InputMode` enum, after `ConfirmFinish(TaskId)`:

```rust
    ConfirmFinish(TaskId),
    ConfirmDone(TaskId),
```

- [ ] **Step 2: Add `ConfirmDone` and `CancelDone` message variants**

In `src/tui/types.rs`, add two new variants to the `Message` enum, after the Finish group:

```rust
    // Finish (merge + cleanup)
    FinishTask(TaskId),
    ConfirmFinish,
    CancelFinish,
    FinishComplete(TaskId),
    FinishFailed { id: TaskId, error: String, is_conflict: bool },
    // Done confirmation (no cleanup, just status change)
    ConfirmDone,
    CancelDone,
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build 2>&1 | head -20`
Expected: Warnings about unused variants, no errors.

- [ ] **Step 4: Commit**

```bash
git add src/tui/types.rs
git commit -m "feat: add ConfirmDone InputMode and Message variants"
```

---

### Task 2: TUI confirmation for single task move Review→Done

**Files:**
- Modify: `src/tui/mod.rs` — `handle_move_task()`, add `handle_confirm_done()`, `handle_cancel_done()`, routing in `update()`
- Test: `src/tui/tests.rs`

- [ ] **Step 1: Write failing test — `m` on Review task enters ConfirmDone mode**

In `src/tui/tests.rs`, add:

```rust
#[test]
fn move_review_to_done_enters_confirm_mode() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Review),
    ], Duration::from_secs(300));
    app.selection_mut().set_column(3); // Review column

    let cmds = app.handle_key(make_key(KeyCode::Char('m')));
    assert!(cmds.is_empty());
    assert!(matches!(app.input.mode, InputMode::ConfirmDone(TaskId(1))));
    assert!(app.status_message.as_deref().unwrap().contains("Done"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test move_review_to_done_enters_confirm_mode 2>&1 | tail -5`
Expected: FAIL — task moves directly to Done without confirmation.

- [ ] **Step 3: Modify `handle_move_task()` to intercept Review→Done**

In `src/tui/mod.rs`, change `handle_move_task()`. Replace the current body with logic that checks if the transition is Review→Done (forward), and if so enters confirmation mode instead of moving immediately. The key change is at the top of the function, before the existing move logic:

```rust
fn handle_move_task(&mut self, id: TaskId, direction: MoveDirection) -> Vec<Command> {
    self.merge_conflict_tasks.remove(&id);
    if let Some(task) = self.find_task_mut(id) {
        let new_status = match direction {
            MoveDirection::Forward => task.status.next(),
            MoveDirection::Backward => task.status.prev(),
        };
        if new_status == task.status {
            return vec![];
        }

        // Confirm before moving to Done
        if new_status == TaskStatus::Done {
            let title = super::truncate_title(&task.title, 30);
            self.input.mode = InputMode::ConfirmDone(id);
            self.status_message = Some(format!("Move {title} to Done? (y/n)"));
            return vec![];
        }

        // Clean up worktree/tmux when moving backward from a dispatched state
        let cleanup = if matches!(direction, MoveDirection::Backward) {
            Self::take_cleanup(task)
        } else {
            None
        };

        task.status = new_status;
        let task_clone = task.clone();
        self.clear_agent_tracking(id);
        self.clamp_selection();

        let mut cmds = Vec::new();
        if let Some(c) = cleanup {
            cmds.push(c);
        }
        cmds.push(Command::PersistTask(task_clone));
        cmds
    } else {
        vec![]
    }
}
```

Note: this also removes the cleanup on forward-to-Done (`new_status == TaskStatus::Done` no longer reaches `take_cleanup`). Cleanup for Done tasks happens when they are archived.

- [ ] **Step 4: Add message routing and handlers**

In `src/tui/mod.rs`, add routing in `update()`:

```rust
Message::ConfirmDone => self.handle_confirm_done(),
Message::CancelDone => self.handle_cancel_done(),
```

Add the handler methods:

```rust
fn handle_confirm_done(&mut self) -> Vec<Command> {
    let id = match self.input.mode {
        InputMode::ConfirmDone(id) => id,
        _ => return vec![],
    };
    self.input.mode = InputMode::Normal;
    self.status_message = None;

    if let Some(task) = self.find_task_mut(id) {
        task.status = TaskStatus::Done;
        let task_clone = task.clone();
        self.clear_agent_tracking(id);
        self.clamp_selection();
        vec![Command::PersistTask(task_clone)]
    } else {
        vec![]
    }
}

fn handle_cancel_done(&mut self) -> Vec<Command> {
    self.input.mode = InputMode::Normal;
    self.status_message = None;
    vec![]
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test move_review_to_done_enters_confirm_mode 2>&1 | tail -5`
Expected: PASS

- [ ] **Step 6: Write test — confirming with `y` moves task to Done**

```rust
#[test]
fn confirm_done_y_moves_task() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Review),
    ], Duration::from_secs(300));
    app.selection_mut().set_column(3);

    app.input.mode = InputMode::ConfirmDone(TaskId(1));
    let cmds = app.handle_key(make_key(KeyCode::Char('y')));
    assert_eq!(app.input.mode, InputMode::Normal);
    let task = app.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Done);
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistTask(_))));
}
```

- [ ] **Step 7: Write test — cancelling with `n` does not move task**

```rust
#[test]
fn confirm_done_n_cancels() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Review),
    ], Duration::from_secs(300));
    app.selection_mut().set_column(3);

    app.input.mode = InputMode::ConfirmDone(TaskId(1));
    let cmds = app.handle_key(make_key(KeyCode::Char('n')));
    assert_eq!(app.input.mode, InputMode::Normal);
    let task = app.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Review);
    assert!(cmds.is_empty());
}
```

- [ ] **Step 8: Write test — moving non-Review tasks forward does NOT trigger confirmation**

```rust
#[test]
fn move_ready_to_running_no_confirmation() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Ready),
    ], Duration::from_secs(300));
    app.selection_mut().set_column(1); // Ready column

    let cmds = app.handle_key(make_key(KeyCode::Char('m')));
    assert_eq!(app.input.mode, InputMode::Normal);
    let task = app.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistTask(_))));
}
```

- [ ] **Step 9: Write test — no cleanup on confirm done (worktree preserved)**

```rust
#[test]
fn confirm_done_does_not_cleanup_worktree() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-test".to_string());
        t.tmux_window = Some("task-1".to_string());
        t
    }], Duration::from_secs(300));
    app.selection_mut().set_column(3);

    // Enter confirm mode and confirm
    app.update(Message::MoveTask { id: TaskId(1), direction: MoveDirection::Forward });
    assert!(matches!(app.input.mode, InputMode::ConfirmDone(TaskId(1))));

    let cmds = app.update(Message::ConfirmDone);
    // No Cleanup command — worktree stays for archive to clean up later
    assert!(!cmds.iter().any(|c| matches!(c, Command::Cleanup { .. })));
    let task = app.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Done);
    // Worktree is preserved (not taken)
    assert!(task.worktree.is_some());
}
```

- [ ] **Step 10: Run all tests**

Run: `cargo test 2>&1 | tail -5`
Expected: All tests pass.

- [ ] **Step 11: Commit**

```bash
git add src/tui/mod.rs src/tui/tests.rs
git commit -m "feat: require confirmation when moving task to Done via m key"
```

---

### Task 3: Key handler and UI for ConfirmDone

**Files:**
- Modify: `src/tui/input.rs` — add `handle_key_confirm_done()`, route in `handle_key()`
- Modify: `src/tui/ui.rs` — add status bar rendering for `ConfirmDone`

- [ ] **Step 1: Add key handler routing in `handle_key()`**

In `src/tui/input.rs`, add the routing arm in the `match self.input.mode.clone()` block, after `ConfirmFinish`:

```rust
InputMode::ConfirmFinish(_) => self.handle_key_confirm_finish(key),
InputMode::ConfirmDone(_) => self.handle_key_confirm_done(key),
```

- [ ] **Step 2: Add `handle_key_confirm_done()` method**

In `src/tui/input.rs`, add:

```rust
fn handle_key_confirm_done(&mut self, key: KeyEvent) -> Vec<Command> {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => self.update(Message::ConfirmDone),
        _ => self.update(Message::CancelDone),
    }
}
```

- [ ] **Step 3: Add status bar rendering**

In `src/tui/ui.rs`, in the `render_status_bar` function's match on `app.input_mode()`, add after `ConfirmFinish`:

```rust
InputMode::ConfirmDone(_) => {
    let text = app.status_message.as_deref().unwrap_or("Move to Done? (y/n)");
    let bar = Paragraph::new(text)
        .style(Style::default().fg(Color::Yellow));
    frame.render_widget(bar, area);
}
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build 2>&1 | head -20`
Expected: No errors.

- [ ] **Step 5: Run all tests**

Run: `cargo test 2>&1 | tail -5`
Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/tui/input.rs src/tui/ui.rs
git commit -m "feat: add key handler and status bar for ConfirmDone mode"
```

---

### Task 4: Batch move confirmation for Review→Done

**Files:**
- Modify: `src/tui/mod.rs` — `handle_batch_move_tasks()`
- Test: `src/tui/tests.rs`

- [ ] **Step 1: Write failing test — batch move with Review tasks triggers confirmation**

```rust
#[test]
fn batch_move_with_review_tasks_enters_confirm_done() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Review),
        make_task(2, TaskStatus::Review),
    ], Duration::from_secs(300));
    app.selection_mut().set_column(3);
    app.update(Message::ToggleSelect(TaskId(1)));
    app.update(Message::ToggleSelect(TaskId(2)));

    let cmds = app.handle_key(make_key(KeyCode::Char('m')));
    assert!(cmds.is_empty());
    assert!(app.status_message.as_deref().unwrap().contains("2 tasks"));
    assert!(app.status_message.as_deref().unwrap().contains("Done"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test batch_move_with_review_tasks_enters_confirm_done 2>&1 | tail -5`
Expected: FAIL

- [ ] **Step 3: Modify `handle_batch_move_tasks()` to intercept Review→Done**

In `src/tui/mod.rs`, replace `handle_batch_move_tasks()`:

```rust
fn handle_batch_move_tasks(&mut self, ids: Vec<TaskId>, direction: MoveDirection) -> Vec<Command> {
    if matches!(direction, MoveDirection::Forward) {
        let review_ids: Vec<TaskId> = ids.iter().copied().filter(|id| {
            self.find_task(*id).map_or(false, |t| t.status == TaskStatus::Review)
        }).collect();

        if !review_ids.is_empty() {
            // Move non-Review tasks immediately
            let mut cmds = Vec::new();
            for id in &ids {
                if !review_ids.contains(id) {
                    cmds.extend(self.handle_move_task(*id, direction.clone()));
                }
            }
            // Enter confirmation for Review→Done tasks
            self.pending_done_tasks = review_ids;
            let count = self.pending_done_tasks.len();
            self.input.mode = InputMode::ConfirmDone(self.pending_done_tasks[0]);
            self.status_message = Some(format!(
                "Move {} {} to Done? (y/n)",
                count,
                if count == 1 { "task" } else { "tasks" }
            ));
            return cmds;
        }
    }

    let mut cmds = Vec::new();
    for id in ids {
        cmds.extend(self.handle_move_task(id, direction.clone()));
    }
    cmds
}
```

- [ ] **Step 4: Add `pending_done_tasks` field to `App`**

In `src/tui/mod.rs`, add a new field to the `App` struct:

```rust
pub(in crate::tui) pending_done_tasks: Vec<TaskId>,
```

Initialize it as `Vec::new()` in `App::new()`.

- [ ] **Step 5: Update `handle_confirm_done()` to handle batch**

Replace `handle_confirm_done()` in `src/tui/mod.rs`:

```rust
fn handle_confirm_done(&mut self) -> Vec<Command> {
    let ids = if !self.pending_done_tasks.is_empty() {
        std::mem::take(&mut self.pending_done_tasks)
    } else {
        match self.input.mode {
            InputMode::ConfirmDone(id) => vec![id],
            _ => return vec![],
        }
    };
    self.input.mode = InputMode::Normal;
    self.status_message = None;

    let mut cmds = Vec::new();
    for id in ids {
        if let Some(task) = self.find_task_mut(id) {
            task.status = TaskStatus::Done;
            let task_clone = task.clone();
            self.clear_agent_tracking(id);
            cmds.push(Command::PersistTask(task_clone));
        }
    }
    self.selected_tasks.clear();
    self.clamp_selection();
    cmds
}
```

- [ ] **Step 6: Update `handle_cancel_done()` to clear batch state**

```rust
fn handle_cancel_done(&mut self) -> Vec<Command> {
    self.input.mode = InputMode::Normal;
    self.status_message = None;
    self.pending_done_tasks.clear();
    vec![]
}
```

- [ ] **Step 7: Run test to verify it passes**

Run: `cargo test batch_move_with_review_tasks_enters_confirm_done 2>&1 | tail -5`
Expected: PASS

- [ ] **Step 8: Write test — batch confirm moves all Review tasks to Done**

```rust
#[test]
fn batch_confirm_done_moves_all_review_tasks() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Review),
        make_task(2, TaskStatus::Review),
    ], Duration::from_secs(300));
    app.selection_mut().set_column(3);
    app.update(Message::ToggleSelect(TaskId(1)));
    app.update(Message::ToggleSelect(TaskId(2)));

    // Trigger batch move
    app.update(Message::BatchMoveTasks {
        ids: vec![TaskId(1), TaskId(2)],
        direction: MoveDirection::Forward,
    });
    // Confirm
    let cmds = app.update(Message::ConfirmDone);
    assert_eq!(app.input.mode, InputMode::Normal);
    for id in [TaskId(1), TaskId(2)] {
        let task = app.tasks.iter().find(|t| t.id == id).unwrap();
        assert_eq!(task.status, TaskStatus::Done);
    }
    assert!(cmds.len() >= 2); // two PersistTask commands
}
```

- [ ] **Step 9: Write test — mixed batch: non-Review tasks move immediately, Review tasks wait for confirmation**

```rust
#[test]
fn batch_move_mixed_statuses_moves_non_review_immediately() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Running),
        make_task(2, TaskStatus::Review),
    ], Duration::from_secs(300));
    app.update(Message::ToggleSelect(TaskId(1)));
    app.update(Message::ToggleSelect(TaskId(2)));

    let cmds = app.update(Message::BatchMoveTasks {
        ids: vec![TaskId(1), TaskId(2)],
        direction: MoveDirection::Forward,
    });
    // Running→Review moved immediately
    let t1 = app.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(t1.status, TaskStatus::Review);
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistTask(t) if t.id == TaskId(1))));

    // Review→Done waiting for confirmation
    let t2 = app.tasks.iter().find(|t| t.id == TaskId(2)).unwrap();
    assert_eq!(t2.status, TaskStatus::Review); // not moved yet
    assert!(matches!(app.input.mode, InputMode::ConfirmDone(_)));
}
```

- [ ] **Step 10: Run all tests**

Run: `cargo test 2>&1 | tail -5`
Expected: All tests pass.

- [ ] **Step 11: Commit**

```bash
git add src/tui/mod.rs src/tui/tests.rs
git commit -m "feat: batch move confirmation for Review to Done transitions"
```

---

### Task 5: MCP rejects `status=done`

**Files:**
- Modify: `src/mcp/handlers.rs` — `handle_update_task()`

- [ ] **Step 1: Write failing test — MCP update_task rejects status=done**

In `src/mcp/handlers.rs`, in the `#[cfg(test)]` module, add:

```rust
#[tokio::test]
async fn update_task_rejects_done_status() {
    let state = test_state();
    let task_id = create_task_fixture(&state);

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "status": "done" }
        })),
    ).await;
    assert_error(&resp, "Cannot set status to done via MCP");

    // Verify task status unchanged
    let task = state.db.get_task(task_id).unwrap().unwrap();
    assert_ne!(task.status, crate::models::TaskStatus::Done);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test update_task_rejects_done_status 2>&1 | tail -5`
Expected: FAIL — currently allows setting done.

- [ ] **Step 3: Add early-return rejection in `handle_update_task()`**

In `src/mcp/handlers.rs`, in `handle_update_task()`, add a check right after the status is parsed (after the `let status = ...` block, before `let mut patch`):

```rust
    if matches!(status, Some(TaskStatus::Done)) {
        return JsonRpcResponse::err(
            id,
            -32602,
            "Cannot set status to done via MCP. Please ask the human operator to move the task to done from the TUI.",
        );
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test update_task_rejects_done_status 2>&1 | tail -5`
Expected: PASS

- [ ] **Step 5: Write test — MCP can still set other statuses**

```rust
#[tokio::test]
async fn update_task_still_allows_other_statuses() {
    let state = test_state();
    let task_id = create_task_fixture(&state);

    for status in &["running", "review", "ready", "backlog"] {
        let resp = call(
            &state,
            "tools/call",
            Some(json!({
                "name": "update_task",
                "arguments": { "task_id": task_id.0, "status": status }
            })),
        ).await;
        assert!(resp.error.is_none(), "status={status} should be allowed, got: {:?}", resp.error);
    }
}
```

- [ ] **Step 6: Run all tests**

Run: `cargo test 2>&1 | tail -5`
Expected: All tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/mcp/handlers.rs
git commit -m "feat: MCP rejects status=done, agents must ask human"
```

---

### Task 6: Final verification

**Files:** None (verification only)

- [ ] **Step 1: Run full test suite**

Run: `cargo test 2>&1 | tail -10`
Expected: All tests pass.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy 2>&1 | tail -10`
Expected: No warnings.

- [ ] **Step 3: Manual smoke test (if TUI available)**

Launch: `cargo run -- tui`
- Create a task, move it to Review
- Press `m` — should see "Move ... to Done? (y/n)" in status bar
- Press `n` — should cancel
- Press `m` again, then `y` — should move to Done
