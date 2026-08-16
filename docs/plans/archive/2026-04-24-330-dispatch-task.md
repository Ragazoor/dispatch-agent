# dispatch_task MCP Tool

## Context

Agents can already create tasks (`create_task`) and dispatch the next epic subtask (`dispatch_next`), but there is no way to dispatch a specific task by ID. `dispatch_next` is fire-and-forget — the agent doesn't know whether the dispatch succeeded or failed.

This plan adds `dispatch_task(task_id)` as a new sync MCP primitive: the agent waits for worktree + tmux creation to complete and gets back the worktree path and tmux window on success, or a clear error on failure. `dispatch_next` is refactored to share the same dispatch logic.

## Design Decisions

- `dispatch_task` is **sync** (awaits completion via `spawn_blocking(...).await`) — agents get a definitive result
- `dispatch_next` stays **async** (fire-and-forget) — suitable for "kick off next and move on" epic orchestration
- Both share a private `do_dispatch(task, db, runner)` helper to avoid logic duplication
- `dispatch_task` requires `task.status == backlog`; returns an error otherwise

## Implementation

### New tool: `dispatch_task(task_id)`

- Validates task exists and is in backlog status
- Calls `do_dispatch` via `spawn_blocking(...).await`
- On success: updates task to Running with worktree/tmux_window, returns worktree path and tmux window
- On failure: returns error, task remains Backlog

### Shared `do_dispatch` helper

Extracted from `handle_dispatch_next`'s inline closure. Routes via `DispatchMode::for_task()` to `dispatch_agent`/`brainstorm_agent`/`plan_agent`. Used by both handlers.

### Files changed

| File | Change |
|------|--------|
| `src/mcp/handlers/tasks.rs` | Added `DispatchTaskArgs`, `do_dispatch` helper, `handle_dispatch_task`; refactored `handle_dispatch_next` |
| `src/mcp/handlers/dispatch.rs` | Registered `dispatch_task` as async tool |
| `src/mcp/handlers/tests.rs` | 5 new tests: happy path, non-backlog error, not-found, tag routing, dispatch failure |
| `docs/specs/tasks.allium` | Added `DispatchTaskViaMcp` rule |
