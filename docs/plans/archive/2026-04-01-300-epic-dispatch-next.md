# Epic Auto-Dispatch on Review

## Context

When an agent finishes work on an epic subtask, a Claude Code Stop hook sets the task status to Review. Currently, the next subtask is only auto-dispatched when a task completes `wrap_up` (rebase or PR). This leaves idle time between tasks — the user must manually press `d` on the epic to dispatch the next subtask.

This change makes epic subtask dispatch fully automatic: when a subtask moves to Review, the next backlog subtask is immediately dispatched. Each task branches independently from main, and concurrent subtasks are allowed.

## Design

### New Message: `AutoDispatchEpic(EpicId)`

Add a new message variant to the `Message` enum in `src/tui/types.rs`. This follows the project's [Adding a New Message](../../CLAUDE.md) pattern.

### Detection: Emit in `handle_refresh_tasks`

In `src/tui/mod.rs`, `handle_refresh_tasks` already detects Review transitions (line 1008). After the existing notification push, add:

```rust
// Auto-dispatch next epic task when subtask enters review
if let Some(epic_id) = new_task.epic_id {
    if !self.agents.auto_dispatched_epics.contains(&epic_id) {
        self.agents.auto_dispatched_epics.insert(epic_id);
        cmds.push(Command::SendMessage(Message::AutoDispatchEpic(epic_id)));
    }
}
```

This goes inside the existing `if new_task.status == TaskStatus::Review && !was_review` block, so it only fires on actual transitions, not repeated refreshes.

### Dedup: `auto_dispatched_epics` on `AgentTracking`

Add `auto_dispatched_epics: HashSet<EpicId>` to the `AgentTracking` struct in `src/tui/types.rs`. This prevents dispatching multiple tasks from the same epic in a single refresh cycle.

Clear an epic from the set when it has no more backlog subtasks, or when a Review task moves back to Running (allowing future re-triggers). The clearing logic goes in the existing stale-state cleanup section of `handle_refresh_tasks` (after line 1023):

```rust
// Clear auto-dispatch guard when epic has no review tasks
// (allows re-triggering if a task bounces back from review)
if let Some(epic_id) = new_task.epic_id {
    if new_task.status != TaskStatus::Review {
        let has_review = new_tasks.iter().any(|t| {
            t.epic_id == Some(epic_id) && t.status == TaskStatus::Review
        });
        if !has_review {
            self.agents.auto_dispatched_epics.remove(&epic_id);
        }
    }
}
```

### Handler: `handle_auto_dispatch_epic`

New method on `App` in `src/tui/mod.rs`:

```rust
fn handle_auto_dispatch_epic(&mut self, epic_id: EpicId) -> Vec<Command> {
    let Some(epic) = self.epics.iter().find(|e| e.id == epic_id) else {
        return vec![];
    };

    // Only auto-dispatch for epics with a plan
    if epic.plan.is_none() {
        return vec![];
    }

    // Find next backlog subtask by sort_order
    let mut backlog: Vec<&Task> = self
        .tasks
        .iter()
        .filter(|t| t.epic_id == Some(epic_id) && t.status == TaskStatus::Backlog)
        .collect();
    backlog.sort_by_key(|t| (t.sort_order.unwrap_or(t.id.0), t.id.0));

    let Some(task) = backlog.first() else {
        return vec![];
    };

    self.set_status(format!(
        "Auto-dispatching #{}: {}",
        task.id.0, task.title
    ));

    // Same dispatch routing as handle_dispatch_epic
    if task.plan.is_some() {
        vec![Command::Dispatch { task: (*task).clone() }]
    } else {
        match task.tag.as_deref() {
            Some("epic") => vec![Command::Brainstorm { task: (*task).clone() }],
            Some("feature") => vec![Command::Plan { task: (*task).clone() }],
            _ => vec![Command::Dispatch { task: (*task).clone() }],
        }
    }
}
```

### Routing

One line in `App::update()`:

```rust
Message::AutoDispatchEpic(id) => self.handle_auto_dispatch_epic(id),
```

### Delivering the message

`handle_refresh_tasks` returns `Vec<Command>`, not `Vec<Message>`. We need a way to deliver `Message::AutoDispatchEpic`. Two options:

**Option A: Use `Command::SendMessage`** — Add a `SendMessage(Message)` command variant that the runtime delivers back to the message channel. This is a generic mechanism for "handler wants to trigger another message."

**Option B: Inline the logic** — Instead of emitting a message, call `handle_auto_dispatch_epic` directly from `handle_refresh_tasks` and append its commands. Simpler but slightly mixes concerns.

**Recommended: Option B.** A dedicated `SendMessage` command is useful but adds infrastructure. Since this is the only use case, call the handler directly:

```rust
// In handle_refresh_tasks, after detecting Review transition for epic subtask:
if let Some(epic_id) = new_task.epic_id {
    if !self.agents.auto_dispatched_epics.contains(&epic_id) {
        self.agents.auto_dispatched_epics.insert(epic_id);
        cmds.extend(self.handle_auto_dispatch_epic(epic_id));
    }
}
```

Note: This requires careful ordering — the auto-dispatch call reads `self.tasks`, but `self.tasks` is only updated at line 1036 (after the loop). The handler will read the *old* task list. This is fine because we only need the epic's backlog subtasks, which haven't changed in this refresh — only the transitioning task's status changed, and it moved *out* of backlog, not into it.

Actually — there's a subtlety. Since `self.tasks` still has the old state when the handler runs, the task that just moved to Review will still appear as its old status. But the handler filters for `status == Backlog`, so it won't pick up the transitioning task regardless. The next backlog task is unaffected.

## Files to Modify

| File | Change |
|------|--------|
| `src/tui/types.rs` | Add `AutoDispatchEpic(EpicId)` to `Message`, add `auto_dispatched_epics: HashSet<EpicId>` to `AgentTracking` |
| `src/tui/mod.rs` | Add routing arm in `update()`, add `handle_auto_dispatch_epic()`, emit from `handle_refresh_tasks` |
| `src/tui/tests.rs` | Test: auto-dispatch triggers on Review transition; test: no dispatch for planless epic; test: dedup prevents double dispatch; test: clearing dedup when no review tasks remain |

## Edge Cases

- **Epic has no plan:** Skip. Unplanned epics need manual intervention.
- **No backlog subtasks left:** Do nothing silently.
- **Multiple tasks hit Review in same refresh:** Dedup set limits to one dispatch per epic per refresh cycle.
- **Task bounces Review → Running:** Dedup set cleared, allowing future re-trigger.
- **Notifications disabled:** Auto-dispatch still fires — it's independent of the notification flag. The auto-dispatch check must be placed **outside** the `if self.notifications_enabled` block in `handle_refresh_tasks`. The Review transition detection needs to be duplicated (or extracted) so it runs unconditionally, while the notification push remains gated.

## Verification

1. `cargo test` — all existing tests pass
2. `cargo clippy` — no warnings
3. New tests cover:
   - Review transition of epic subtask triggers auto-dispatch of next backlog subtask
   - No auto-dispatch for epics without a plan
   - Dedup prevents dispatching twice for the same epic
   - Tag-based routing works (epic → brainstorm, feature → plan, has plan → dispatch)
   - No dispatch when no backlog subtasks remain
4. Manual test: run TUI, create epic with 2+ subtasks, dispatch first, let it complete to Review, verify second auto-dispatches
