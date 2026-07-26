# WP7: Add end-to-end integration tests

## Context

The repo has 2,296 unit tests but only 22 integration tests across 4 files (`tests/lifecycle.rs`, `tests/epic_lifecycle.rs`, `tests/dispatch_status_lifecycle.rs`, `tests/cli.rs`). Several cross-layer flows are critical and currently only exercised in isolation.

## Findings

- **Severity:** Medium
- **Files:** `tests/`
- **Issue:** Lopsided unit:integration ratio; cross-layer regressions can pass unit suites and break end-to-end flows.
- **Suggestion:** Add 3 focused scenario tests in `tests/`.

## Plan (TDD)

Each scenario is a new integration test file using `Database::open_in_memory()` and `MockProcessRunner`. Write the test first, then patch any wiring that the test exposes.

### Scenario A — Feed sync end-to-end (`tests/feed_sync.rs`)

1. Create a feed epic with a `feed_command` pointing at `MockProcessRunner` that returns a fixed JSON array of 3 `FeedItem`s.
2. Trigger feed sync.
3. Assert: 3 tasks created, each with the correct `external_id` and `SubStatus::Feed`.
4. Re-run with one item removed and one item updated.
5. Assert: original 3 tasks still exist (no deletes); the updated item's task reflects new title.

### Scenario B — Review-agent state machine (`tests/review_agent_lifecycle.rs`)

1. Dispatch a review agent for a PR task.
2. Assert agent_status = `Reviewing`.
3. Call `update_review_status(findings_ready)` via the MCP handler directly.
4. Assert status flipped, `pr_workflow_states` upserted, card-flash flag set.
5. Call `update_review_status(idle)`.
6. Assert status flipped to `Idle`; re-review (`r`) is allowed.
7. Detach (`T`) and assert `tmux_window`, `worktree`, `agent_status` are all NULL.

### Scenario C — Project delete cascade (`tests/project_delete.rs`)

1. Create project P (non-default), add 3 tasks and 1 epic to it.
2. Verify Default project has its seed values only.
3. Call `delete_project_and_move_items(P)`.
4. Assert: all 3 tasks and 1 epic now have `project_id = default_id` in a single transaction.
5. Assert deleting the Default project errors.

## Files to change

| File | Change |
|---|---|
| `tests/feed_sync.rs` | New. Scenario A. |
| `tests/review_agent_lifecycle.rs` | New. Scenario B. |
| `tests/project_delete.rs` | New. Scenario C. |
| Source files | Only if a test exposes a real bug — fix it, don't paper over it. |

## Verification

```bash
cargo test --test feed_sync
cargo test --test review_agent_lifecycle
cargo test --test project_delete
cargo test                       # full suite
cargo clippy --all-targets -- -D warnings
```
