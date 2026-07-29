# 3790 — Fix `exec_persist_task`'s whole-snapshot splice in the Done-transition write-back

## Context

Task #3761 added a write-back to `exec_persist_task` (`src/runtime/tasks.rs`) so the
in-memory board learns the `sort_order` the *service* computes on a Done transition
(`sort_order_for_status_transition`, run inside `TaskService::update_task`). The
write-back currently does:

```rust
if let Some(new_sort_order) = result.sort_order_after_write {
    let mut updated = task;                 // the caller's whole snapshot
    updated.sort_order = new_sort_order;
    app.update(Message::Task(TaskMessage::Updated(updated)));
}
```

Three problems, all in the same few lines:

1. **Whole-snapshot splice.** `TaskMessage::Updated` replaces the board slot wholesale
   (`handle_task_updated`, `src/runtime/../tui/update/agent.rs:83`). Splicing the
   caller's snapshot re-imposes every field it holds — including `last_pre_tool_use_at`
   — which is exactly the clobber the comment directly above this code warns against
   for the DB write ("a stale in-memory snapshot would overwrite a fresher hook
   write, flipping the task to Stale on the next tick"). Latent today because
   `execute_commands` holds `&mut App` exclusively across the `update_task` await and
   drains commands without polling `msg_rx`, so no hook-driven refresh can interleave —
   but the invariant should not depend on that.
2. **No "not in board" guard.** `handle_task_updated` *pushes* when the id isn't found,
   so a concurrent delete/archive would let this write-back resurrect a ghost card.
   The epic-side equivalent (`write_back_epic_sort_order`, `src/runtime/epics.rs:102`)
   already guards with `let Some(..) = app.epics()...cloned() else { return }`.
3. **Test gap.** Both new runtime tests (task and epic side) only cover *entering*
   Done. The leaving-Done direction — `sort_order_after_write == Some(None)`, i.e. an
   explicit clear — is untested at the runtime level.

The correct shape already exists on the epic side: read the *live* board item, clone it,
patch only `sort_order`, splice that. Mirror it for tasks.

## Domain-behaviour / spec impact

No change to observable domain behaviour: the spec's `ensures: task.sort_order =
completion_recency_rank(now)` / `= null` obligations (`docs/specs/tasks.allium:280`,
`:252`) are unchanged, and the DB write is untouched. What changes is *which* in-memory
fields a persist is allowed to overwrite.

That *is* already a spec'd invariant, though: the `last_pre_tool_use_at` ownership
invariant in `docs/specs/agent-health.allium:61-70` says generic in-memory persists
from the TUI "MUST NOT include this column". It currently speaks only of the write
*to the database*. Extend that guidance to cover the in-memory write-back direction
too, so the rule reads as field-ownership rather than as a DB-only rule
(`allium:tend`, then `allium:weed` to confirm alignment).

## Steps (TDD — tests first in every step)

### Step 1 — Tests for the leaving-Done (clear) direction

`src/runtime/tests.rs`, new test
`exec_persist_task_writes_back_leaving_done_sort_order_clear_immediately`:

- Create a task, put it in Done in the DB **with a non-null `sort_order`**, and refresh
  so the in-memory board holds that Done+sort_order state.
- Mutate the board's copy to Review the way a real handler does (`TaskMessage::Updated`
  with status Review, mirroring `handle_move_task_backward`'s find_task_mut-then-Persist
  pattern), then hand that clone to `exec_persist_task`.
- Assert, with no `exec_refresh_from_db` in between, that the in-memory task's
  `sort_order` is now `None`, and that it matches the DB.

This currently passes with the old code too (the snapshot happens to carry the right
status); it is the missing-coverage half of the task, and it must keep passing after
Step 2's rewrite.

### Step 2 — Test that the write-back patches only `sort_order`

New test `exec_persist_task_write_back_does_not_clobber_fresher_board_fields`:

- Board holds a task whose `last_pre_tool_use_at` is a fresh timestamp (simulating a
  hook write that a refresh has already brought into memory).
- Hand `exec_persist_task` a *stale* snapshot of the same task (`last_pre_tool_use_at =
  None`) with status Done, so the service computes a `sort_order` and the write-back
  fires.
- Assert the board task's `last_pre_tool_use_at` is **still** the fresh timestamp, and
  its `sort_order` is the new negative value.

This fails on the current whole-snapshot splice. It is the regression lock for finding 1.

### Step 3 — Test the "not currently in board" guard

New test `exec_persist_task_write_back_does_not_resurrect_task_absent_from_board`:

- Create a task in the DB, put it in Review, and **do not** load it into the board
  (`app.tasks()` empty for that id).
- Call `exec_persist_task` with a Done snapshot.
- Assert `app.tasks()` still contains no task with that id (no ghost card), while the
  DB write did land.

Fails today (the current code pushes via `handle_task_updated`). Regression lock for
finding 2.

### Step 4 — Adapt the existing entering-Done test

`exec_persist_task_writes_back_done_transition_sort_order_immediately` currently asserts
`in_memory.status == Done` while never putting Done on the *board* copy — it only sets it
on the snapshot it passes in. That assertion passes today purely because of the
whole-snapshot splice being fixed. Make the test model the real handler: mutate the board
copy to Done first (as `detach_tmux_panels`/`handle_confirm_done` do via
`find_task_mut`), then persist. The `sort_order` assertions stay as-is.

### Step 5 — Implementation

In `src/runtime/tasks.rs`:

- Replace the inline write-back with a `write_back_task_sort_order(&self, app, result)`
  helper mirroring `write_back_epic_sort_order` (`src/runtime/epics.rs:102`):

  ```rust
  fn write_back_task_sort_order(&self, app: &mut App, result: crate::service::UpdateTaskResult) {
      let Some(new_sort_order) = result.sort_order_after_write else { return };
      let Some(mut task) = app.tasks().iter().find(|t| t.id == result.task_id).cloned() else { return };
      task.sort_order = new_sort_order;
      app.update(Message::Task(crate::tui::messages::TaskMessage::Updated(task)));
  }
  ```

- Keep the existing explanatory comment (why a write-back is needed at all, and why it
  routes through `TaskMessage::Updated` rather than touching `App.board`), and add the
  two new facts: clone-the-live-task (not the caller's snapshot) so hook-owned columns
  survive, and the absent-from-board no-op.

### Step 6 — Spec alignment

`allium:tend` on `docs/specs/agent-health.allium`: extend the ownership-invariant
guidance so it covers in-memory write-backs, not just DB writes. Then `allium:weed` to
verify no drift was introduced.

### Step 7 — Verify

```
cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh
```

Plus `cargo clippy --all-targets -- -D warnings` (the pre-push hook's gate).

## Out of scope

- The epic side (`write_back_epic_sort_order`) is already correct; only its
  leaving-Done test coverage is thin. The task description scopes the new test to the
  task path, so the epic clear-direction test is not added here.
- No change to `UpdateTaskResult` / `sort_order_after_write` semantics.
