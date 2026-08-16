# Runtime Refactor

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Remove the concentrated debt in `src/runtime/` — duplicate detach logic, repeated error boilerplate, the `exec_quick_dispatch` god-function, and the procedural `run_tui` setup blob — without changing observable behaviour.

## Context

This work package addresses findings from the 2026-06-23 codebase review. The review found the runtime layer to be where the only real, actionable debt is concentrated. All changes here are behaviour-preserving refactors: the existing test suite (`src/runtime/tests.rs`, `src/runtime/editor.rs` tests, lifecycle integration tests) must stay green throughout. Use TDD — where a refactor extracts a unit (e.g. `detach_only`), add a focused test for the extracted unit first.

## Findings

### ⚠️ `exec_finish` / `exec_cleanup` duplication (`src/runtime/tasks.rs:473-614`)

**Issue:** The two functions are near-identical: same `has_other_tasks_with_worktree` check, same shared-worktree `worktree(Clear).tmux_window(Clear)` detach block, and the same error-handling skeleton, copy-pasted verbatim.

**Fix:** Extract `async fn detach_only(&self, id: TaskId) -> Result<(), ...>` for the shared detach block. Both functions then call it and keep only their unique tail (`exec_cleanup` → `spawn_blocking(cleanup_task)`; `exec_finish` → `spawn_blocking(finish_task)` + `FinishComplete` message).

### 💡 Repeated error-message boilerplate (`src/runtime/*`)

**Issue:** `let _ = self.msg_tx.send(Message::System(crate::tui::messages::SystemMessage::Error(format!(...))))` recurs dozens of times across the runtime layer.

**Fix:** Add a private helper `fn send_system_error(&self, msg: impl Into<String>)` (or an extension on the sender) and replace the call sites. Removes the most-repeated boilerplate and the associated `let _ =` noise.

### 💡 `exec_quick_dispatch` god-function (`src/runtime/tasks.rs:29-132`)

**Issue:** ~105 lines mixing task creation, repo-path persistence, embedding injection, dispatch, and a nested `tokio::spawn` → `spawn_blocking` → `catch_unwind` → `match` ladder reaching 24-space indentation in the panic arm.

**Fix:** Extract the `spawn_blocking` dispatch + panic-handling tail into a free `fn run_quick_dispatch(task, runner, ctx, injections, verify, msg_tx)`, leaving `exec_quick_dispatch` to do only create / persist / spawn. Drops nesting from ~6 levels to ~3.

### 💡 `run_tui` procedural setup blob (`src/runtime/mod.rs:69-272`)

**Issue:** ~200 lines mixing terminal init, tmux setup, MCP spawn, embedding-model loading, 7 sequential `load_*` settings calls, and `TuiRuntime` construction, with a `#[cfg(test)]` / `#[cfg(not(test))]` split for the embedding service.

**Fix:** Extract a composition root — e.g. `TuiRuntime::bootstrap(config)` — so `run_tui` reads as a sequence of named steps. Keep the test/non-test embedding split behind the builder so call sites don't branch on `cfg`. This is the larger item; it can be done as a separate commit after the smaller refactors land.

## Changes

| File | Change |
|------|--------|
| `src/runtime/tasks.rs` | Extract `detach_only`; refactor `exec_finish`/`exec_cleanup` to use it. Extract `run_quick_dispatch` free fn; slim `exec_quick_dispatch`. |
| `src/runtime/mod.rs` | Add `TuiRuntime::bootstrap` (or equivalent) composition root; refactor `run_tui` to call it. Optionally add `send_system_error` here if it lives on `TuiRuntime`. |
| `src/runtime/*.rs` | Replace `let _ = msg_tx.send(...Error(format!(...)))` call sites with `send_system_error`. |

## Verification

- [ ] `cargo test runtime::` — passes
- [ ] `cargo test --test lifecycle` and `cargo test --test epic_lifecycle` — pass (behaviour preserved)
- [ ] `cargo test` — full suite green
- [ ] `cargo clippy --all-targets -- -D warnings` — clean
- [ ] `./scripts/check-doc-paths.sh` and `./scripts/check-no-test-sleep.sh` pass
- [ ] Confirm no behavioural change: error messages still surface to the status bar, `FinishComplete` still emitted, quick-dispatch panic path still reports an error
