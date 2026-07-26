# Remove Redundant Integration Tests

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove two redundant integration tests from `tests/lifecycle.rs` that duplicate unit test coverage without adding integration value.

**Architecture:** Delete `dispatch_only_from_ready` and `window_gone_clears_tmux_window_without_advancing` from the integration test file. Both tests don't use the database (their `_db` binding is unused) and their assertions are already covered by unit tests in `src/tui/tests.rs`.

**Tech Stack:** Rust, cargo test

---

### Task 1: Remove redundant integration tests and verify

**Files:**
- Modify: `tests/lifecycle.rs:121-172` (delete two test functions)

**Context:**
- `dispatch_only_from_ready` (lines 121-136) is redundant with unit test `dispatch_only_ready_tasks` in `src/tui/tests.rs:72-86`
- `window_gone_clears_tmux_window_without_advancing` (lines 138-172) is redundant with unit test `window_gone_clears_tmux_window_and_persists` in `src/tui/tests.rs:336-354`
- The remaining `full_lifecycle` test (lines 37-119) must be kept — it's the only test that exercises the real SQLite database integration

- [ ] **Step 1: Delete the two redundant tests**

Remove `dispatch_only_from_ready` (lines 121-136) and `window_gone_clears_tmux_window_without_advancing` (lines 138-172) from `tests/lifecycle.rs`. Keep `full_lifecycle` intact.

The file should end after line 119 (the closing `}` of `full_lifecycle`).

- [ ] **Step 2: Run all tests to verify nothing breaks**

Run: `cargo test`
Expected: All tests pass. The removed tests' coverage is already provided by unit tests.

- [ ] **Step 3: Run clippy**

Run: `cargo clippy`
Expected: No warnings or errors.

- [ ] **Step 4: Commit**

```bash
git add tests/lifecycle.rs
git commit -m "test: remove redundant integration tests

Remove dispatch_only_from_ready and
window_gone_clears_tmux_window_without_advancing from
tests/lifecycle.rs. Both duplicated unit test coverage
without exercising the database."
```
