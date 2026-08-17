# Make `TaskCommand::Persist` carry a field patch, not a whole `Task`

Task #4232.

## Problem

`TaskCommand::Persist(Box<Task>)` moves a whole ~470-byte `Task` (plus
~6 String allocations) through the command bus so `exec_persist_task` can read
7 fields (`id`, `status`, `sub_status`, `worktree`, `tmux_window`, `url`,
`sort_order`) and discard the rest. All ~14 prod call sites do
`Box::new(task.clone())` to get there. `TaskCommand::Resume` has the same
shape problem but only needs `id` and `worktree`.

## Design

Add `PersistFields` (`src/tui/commands/task.rs`, next to `TaskCommand`):

```rust
#[derive(Debug, Clone)]
pub struct PersistFields {
    pub id: TaskId,
    pub status: TaskStatus,
    pub sub_status: SubStatus,
    pub worktree: Option<String>,
    pub tmux_window: Option<String>,
    pub url: Option<TaskUrl>,
    pub sort_order: Option<i64>,
}

impl PersistFields {
    pub fn from_task(task: &Task) -> Self { ... }
}
```

Field names/types mirror `Task`'s so existing test field accesses
(`t.id`, `t.status`, `t.tmux_window`, `t.sort_order`) keep compiling unchanged.
No `FieldUpdate`/tri-state semantics at this layer: `Persist` always carries
the current value of each of the 7 fields (matching today's behavior, where
`exec_persist_task` maps `None` -> `Clear` unconditionally via
`option_to_field_update`).

Change:
- `TaskCommand::Persist(Box<Task>)` -> `TaskCommand::Persist(PersistFields)` (no box — the struct is well under `size_of::<Task>()`, so `assert_no_entity_inline` still holds).
- `TaskCommand::Resume { task: Box<Task> }` -> `TaskCommand::Resume { id: TaskId, worktree: Option<String> }`.

`exec_persist_task`/`exec_resume` (`src/runtime/tasks.rs`) take the new
payloads directly instead of a whole `Task`; `exec_persist_task`'s body barely
changes (it already built `UpdateTaskParams` from 7 fields — now it reads them
off `PersistFields` instead of `Task`).

Every prod call site swaps `Box::new(task.clone())` for
`PersistFields::from_task(&task)` (or `task.id, task.worktree.clone()` for
`Resume`). Two sites need care because they don't just build-and-forget:

- `retry.rs::handle_retry_fresh` reuses the task clone for a following
  `DispatchAgent` command — keep that clone, build `PersistFields` from a
  borrow of it instead of cloning twice.
- `retry.rs::handle_archive_task` currently clones the task then overwrites
  `worktree`/`tmux_window` on the clone (because the live board task already
  had them taken by `take_cleanup`). With `PersistFields` this is just
  constructing the struct directly with the pre-take values — no clone of the
  whole task needed at all.

## Test changes

- `src/runtime/tests.rs`: ~8 `exec_persist_task(&mut app, task)` calls become
  `exec_persist_task(&mut app, PersistFields::from_task(&task))`; 2
  `exec_resume(task)` calls become `exec_resume(task.id, task.worktree.clone())`.
- `src/tui/tests/dispatch.rs`: two assertions read
  `persisted.last_pre_tool_use_at.is_some()` off the `Persist` payload — that
  field is not one of the 7 `PersistFields` carries (this was never written by
  `exec_persist_task` either — see its doc comment), so these two assertions
  are deleted; the same behavior is already covered by the preceding assertion
  on the board task's `last_pre_tool_use_at` directly.
- All other `Persist(t)`/`Persist(_)` test matchers need no change: they only
  read `.id`, `.status`, `.tmux_window`, `.sort_order`, all present on
  `PersistFields` with identical names/types.
- All `Resume { .. }` test matchers (no bindings) need no change.

## Verify

`cargo clippy --all-targets -- -D warnings` and `cargo test`, including the
`assert_no_entity_inline` guard-rail tests in `src/tui/types.rs`.
