# WP-3: Dispatch Layer Tests

## Context

The dispatch layer (`src/dispatch/agents.rs`, `src/dispatch/worktree.rs`, `src/dispatch/finish.rs`) handles the critical path of worktree creation, tmux session setup, and post-task rebase/cleanup. These files have no dedicated unit tests — bugs surface only when a real agent dispatch fails. Adding MockProcessRunner-based tests for this layer closes the most important coverage gap identified in the code review.

## Findings

### M2 — No unit tests for `dispatch/agents.rs`
- **Severity**: medium (coverage gap)
- **Files**: `src/dispatch/agents.rs` (359 lines), `src/dispatch/tests.rs`
- **Issue**: `provision_and_dispatch` (114 lines) handles multiple git strategies (`CheckoutRemote` vs `NewBranch`) with nested match arms and no unit tests. A regression here won't be caught until a user attempts to dispatch.
- **Fix**: Add MockProcessRunner-based tests covering:
  - Happy path: `NewBranch` strategy creates worktree and starts tmux window
  - Happy path: `CheckoutRemote` strategy fetches and checks out remote branch
  - Error path: worktree creation fails → function returns error (no tmux window created)
  - Error path: tmux window creation fails after worktree created → worktree is cleaned up

### M3 — No unit tests for `dispatch/worktree.rs` and `dispatch/finish.rs`
- **Severity**: medium (coverage gap)
- **Files**: `src/dispatch/worktree.rs` (~170 lines), `src/dispatch/finish.rs` (~140 lines)
- **Issue**: Worktree creation/cleanup and the rebase mechanic are end-to-end tested only via integration tests. Unit tests with MockProcessRunner would catch regressions faster.
- **Fix**: Add tests for:
  - `worktree.rs`: create succeeds, create fails (path already exists), remove succeeds, remove when path missing
  - `finish.rs`: rebase succeeds, rebase conflicts detected and reported, cleanup runs even on rebase failure

## Implementation Notes

All new tests go in `src/dispatch/tests.rs` alongside the existing 112 tests.

Use the existing `MockProcessRunner` from `src/process.rs` — queue expected shell command responses with `.push_response()`. Follow the pattern already established in `src/dispatch/tests.rs`.

## Changes Table

| File | What to change |
|---|---|
| `src/dispatch/tests.rs` | Add test functions for agents.rs, worktree.rs, finish.rs |

## Verification

```bash
cargo test dispatch::tests
```

Existing 112 tests must continue to pass. New tests added.
