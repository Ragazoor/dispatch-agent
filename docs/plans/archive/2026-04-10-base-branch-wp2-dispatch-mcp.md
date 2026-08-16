# WP2: Base Branch — Dispatch & MCP Integration

## Context

Second work package. Makes dispatch, rebase, and PR operations use `task.base_branch` instead of auto-detecting. Updates MCP tool schemas to accept `base_branch` on create/update. Safe to merge because WP1 ensures all tasks have `base_branch='main'`.

## Files to Modify

| File | Change |
|------|--------|
| `src/dispatch.rs` | `provision_worktree()`, `finish_task()`, `create_pr()` use explicit base_branch param |
| `src/mcp/handlers/tasks.rs` | Pass base_branch in create, allow in update, read in wrap_up |
| `src/mcp/handlers/dispatch.rs` | Update `create_task` and `update_task` tool schemas |
| `src/service.rs` | Thread base_branch through service methods if needed |
| `src/mcp/handlers/tests.rs` | Test MCP with base_branch |

## Steps

### 1. Dispatch layer (test first)

**Tests:** Update/add tests for:
- `provision_worktree` uses the provided base_branch argument
- `finish_task` rebases onto the provided base_branch (not auto-detected)
- `create_pr` uses the provided base_branch as `--base`

**Implement in `src/dispatch.rs`:**
- `finish_task()`: Accept `base_branch: &str` parameter, use it instead of calling `detect_default_branch()`. Verify repo is on `base_branch`, pull `base_branch`, rebase onto `base_branch`, fast-forward `base_branch`.
- `create_pr()`: Accept `base_branch: &str` parameter, use as `--base` argument to `gh pr create`.
- `dispatch_with_prompt()`: Read `task.base_branch` and pass it through to `provision_worktree()`. Remove the internal `detect_default_branch()` call.
- `detect_default_branch()` remains — still used at task creation time.

### 2. MCP handlers (test first)

**Tests in `src/mcp/handlers/tests.rs`:**
- `create_task` with explicit `base_branch` stores it
- `create_task` without `base_branch` defaults to `'main'`
- `update_task` with `base_branch` updates the field
- `wrap_up` (rebase) uses task's base_branch

**Implement:**
- `src/mcp/handlers/dispatch.rs`: Add `base_branch` (optional string) to `create_task` and `update_task` input schemas
- `src/mcp/handlers/tasks.rs`:
  - `handle_create_task`: Read `base_branch` from args. If not provided, call `detect_default_branch()` on the task's `repo_path` to auto-detect. Pass resolved value to `create_task()`
  - `handle_update_task`: Read optional `base_branch`, include in `TaskPatch`
  - `handle_wrap_up`: Read `task.base_branch`, pass to `finish_task()` / `create_pr()`

### 3. Service layer

Update `src/service.rs` if any service methods need to thread `base_branch` through (check if `validate_wrap_up` or similar need changes).

## Verification

1. `cargo test` — all tests pass
2. `cargo clippy -- -D warnings`
3. `cargo fmt --check`
4. Manual: create a task via MCP with `base_branch: "main"`, dispatch, wrap up — verify same behavior as before
