# Plan: MCP tools — query_learnings, record_learning, confirm_learning (WP4)

## Context

This work package wires three agent-facing MCP tools that let dispatched agents interact with the
learning store built in WP3 (task 343). Agents call `record_learning` to surface what they learn,
`query_learnings` to pull relevant context before starting or mid-task, and `confirm_learning`
when an approved learning proves correct during execution.

The spec lives in `docs/specs/tasks.allium` (rules `RecordLearningViaMcp`, `QueryLearningsViaMcp`,
`ConfirmLearningViaMcp`). The DB/service layer is already in place post-rebase.

## Design

All three tools follow the standard dispatch pattern:
- Typed `*Args` struct (Deserialize) in a new `src/mcp/handlers/learnings.rs`
- Sync handler functions (`&McpState, Option<Value>, Value) -> JsonRpcResponse`)
- Tool registered in the `mcp_tools!` macro in `dispatch.rs`
- `mod learnings;` added to `handlers/mod.rs`
- `use super::learnings;` added to `dispatch.rs`

**`record_learning`** — agents propose a new learning entry. Always lands as `proposed`; no TUI
notification needed. `scope_ref` auto-derives when omitted (spec §RecordLearningViaMcp).
The handler must look up the source task to derive project_id/repo_path/epic_id when
the agent omits scope_ref. Calls `LearningService::create_learning`.

**`query_learnings`** — retrieves approved learnings for the calling task's context. Calls
`state.db.list_learnings_for_dispatch(project_id, repo_path, epic_id)` (already implements
the union + priority ordering). Applies optional `tag_filter` as a post-filter on results
(because `list_learnings_for_dispatch` doesn't take a tags argument). Caps at `min(limit ?? 20, 50)`.
Needs to fetch the task first to resolve project_id/repo_path/epic_id.

**`confirm_learning`** — increments confirmed_count on an approved learning. Calls
`LearningService::confirm_learning`. Lightweight; no notify needed (no TUI change).

## Scope-ref auto-derivation (record_learning)

When `scope_ref` is omitted:
- `scope=project` → `str(task.project_id)`
- `scope=repo` → `task.repo_path`
- `scope=epic` → `str(task.epic_id)`, error if task has no epic
- `scope=task` → `str(task.id)`
- `scope=user` → null (no scope_ref needed)

This requires fetching the source task when scope_ref is absent and scope != user.

## Files to change

| File | Change |
|------|--------|
| `src/mcp/handlers/learnings.rs` | **new** — all three arg structs + handlers |
| `src/mcp/handlers/mod.rs` | add `mod learnings;` |
| `src/mcp/handlers/dispatch.rs` | add `use super::learnings;` + 3 entries in `mcp_tools!` |
| `src/mcp/handlers/tests.rs` | new tests for all three tools |

## Implementation steps (TDD)

### Step 1 — Tests first

Write failing tests in `src/mcp/handlers/tests.rs`:

**record_learning tests:**
- `record_learning_creates_proposed_entry` — valid call with explicit scope_ref; assert status=proposed
- `record_learning_derives_scope_ref_for_repo` — omit scope_ref with scope=repo; assert scope_ref=task.repo_path
- `record_learning_derives_scope_ref_for_epic` — scope=epic, task has epic; assert scope_ref=str(epic_id)
- `record_learning_epic_scope_no_epic_fails` — scope=epic, task has no epic; assert error
- `record_learning_user_scope_ignores_scope_ref` — scope=user; assert scope_ref=null
- `record_learning_empty_summary_fails` — assert validation error
- `record_learning_unknown_task_fails` — bad task_id; assert error

**query_learnings tests:**
- `query_learnings_returns_approved_for_task` — insert approved learning matching task's repo; assert it's returned
- `query_learnings_excludes_proposed` — proposed learning not returned
- `query_learnings_tag_filter_narrows_results` — two approved learnings, one matches tag_filter
- `query_learnings_respects_limit` — insert 5, request limit=2; assert 2 returned
- `query_learnings_unknown_task_fails` — bad task_id; assert error

**confirm_learning tests:**
- `confirm_learning_increments_count` — create+approve learning, confirm; assert confirmed_count=1
- `confirm_learning_proposed_fails` — proposed learning; assert validation error
- `confirm_learning_unknown_learning_fails` — bad learning_id; assert error

### Step 2 — New handler module

Create `src/mcp/handlers/learnings.rs`:

```
// Arg structs:
RecordLearningArgs { task_id: i64, kind: LearningKind, summary: String,
                     scope: LearningScope, detail?: String, scope_ref?: String, tags?: Vec<String> }
QueryLearningsArgs { task_id: i64, tag_filter?: String, limit?: i64 }
ConfirmLearningArgs { learning_id: i64, task_id: i64 }
```

Handler functions: `handle_record_learning`, `handle_query_learnings`, `handle_confirm_learning`

`handle_record_learning`:
1. Parse args
2. Fetch task (error on not found)
3. Derive scope_ref if absent (per rules above)
4. Call `LearningService::create_learning`
5. Return ok with "Learning {id} recorded (proposed)"

`handle_query_learnings`:
1. Parse args
2. Fetch task (error on not found)
3. Call `state.db.list_learnings_for_dispatch(task.project_id, &task.repo_path, task.epic_id)`
4. Apply tag_filter post-filter (keep if any tag matches)
5. Apply limit (min(limit.unwrap_or(20), 50))
6. Format as text table/list
7. Return ok

`handle_confirm_learning`:
1. Parse args
2. Call `LearningService::confirm_learning(learning_id)`
3. Return ok with "Learning {id} confirmed (count now N)"

### Step 3 — Wire into dispatch.rs

Add `mod learnings;` to `handlers/mod.rs`.
Add `use super::learnings;` to `dispatch.rs`.
Add 3 entries to `mcp_tools!` macro:

```
sync "record_learning" => learnings::handle_record_learning, ...;
sync "query_learnings" => learnings::handle_query_learnings, ...;
sync "confirm_learning" => learnings::handle_confirm_learning, ...;
```

### Step 4 — Build + test

```bash
cargo build
cargo test mcp::handlers::tests
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## Response format

**record_learning** success: `"Learning {id} recorded as proposed. Awaiting human approval before it affects future dispatches."`

**query_learnings** success: formatted list per learning:
```
[{id}] ({kind}/{scope}) {summary}
  Tags: {tags}
  Confirmed: {confirmed_count}x
```
If empty: `"No approved learnings found for this task's context."`

**confirm_learning** success: `"Learning {id} confirmed. Confirmed {confirmed_count} time(s) total."`

## CQ-shape compatibility

The three tool names and parameter shapes are intentionally cq-compatible:
- `task_id` is the calling-task anchor (cq uses task context for scoping)
- `learning_id` is a plain integer (no namespacing needed)
- `scope` + `scope_ref` mirror the stored fields verbatim
- No tool-internal state; all mutations go through the LearningService

## Verification

```bash
cargo test mcp::handlers::tests   # all new tests pass
cargo test                        # full suite green
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Manual smoke test (with dispatch running):
1. Dispatch a task; agent calls `record_learning` → appears in DB as proposed
2. Agent calls `query_learnings` → returns approved learnings for that task's repo/epic
3. Agent calls `confirm_learning` on an approved learning → confirmed_count increments
