# Runtime Error Propagation

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace silent `warn!()` logging on database errors in the runtime with proper error propagation so state updates don't silently fail.

## Context

This work package addresses findings from a code review. The runtime layer catches database errors and logs them as warnings, but continues as if the operation succeeded. This means the TUI can show stale state without the user knowing an update failed.

## Findings

### :warning: Silent failure on DB errors (`src/runtime.rs`)

**Issue:** Multiple `exec_*` methods in `TuiRuntime` follow this pattern:
```rust
if let Err(e) = db.patch_task(task_id, &patch) {
    tracing::warn!(...);
}
```
The caller has no way to know the operation failed. The TUI continues showing the pre-update state, and the user may think their action succeeded.

**Fix:** Audit all `exec_*` methods that write to the database. For each:
1. Propagate the error as a `Result` return type, OR
2. Send a `Message::Error(...)` back to the TUI so the status bar shows the failure

The choice depends on context — some callers (like tick handlers) can't easily surface errors, so a `Message::StatusMessage` with the error is more practical than a `Result`. The key invariant: the user must always know when a DB write fails.

## Changes

| File | Change |
|------|--------|
| `src/runtime.rs` | Audit `exec_*` methods; propagate DB errors via Result or Message::StatusMessage |
| `src/tui/mod.rs` | Handle error messages in `update()` if new Message variants are needed |
| `src/tui/types.rs` | Add error-display Message variant if needed |

## Verification

- [ ] Run existing tests — all pass
- [ ] Simulate a DB write failure (e.g., close connection) — verify error surfaces in TUI status bar
- [ ] No `warn!()` on DB errors remains without corresponding user-visible feedback
- [ ] `cargo clippy -- -D warnings` passes
