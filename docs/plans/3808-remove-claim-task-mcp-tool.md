# 3808 — Remove the `claim_task` MCP tool

**Goal:** Delete the vestigial `claim_task` MCP tool and its entire supporting
stack (handler, args struct, `TaskService::claim_task`, `ClaimTaskParams`, spec
rule), eliminating the last Backlog→Running path that is not an atomic claim.

## Premise verification (done — findings)

The task description asked for three checks before deleting anything. All three
are settled:

1. **Removal target confirmed.** The request's `claim_backlog_task` is not a
   real symbol in this tree — the atomic claim added by #3802's sibling commit
   (`ac765e9a`) is `TaskService::claim_next_backlog_task`, which is *epic*-scoped
   (`claim_next_backlog_task(epic_id)`) and load-bearing for `auto_dispatch_next`.
   It is **not** being touched. The removal target is the MCP tool `claim_task`
   registered at `src/mcp/handlers/dispatch.rs:254`, plus the task-scoped
   `TaskService::claim_task` that only it calls.

2. **Zero real usage.** Trajectories are JSONL files under
   `~/.local/share/dispatch/trajectories/`, not a DB table. Tallying `method`
   across all 627 task files yields **0 `claim_task` calls** (24 distinct methods
   recorded, `dispatch_task` at 9 being the rarest non-zero one). No
   `plugin/skills/**/SKILL.md` mentions it either.

3. **No production caller of the service method.** `TaskService::claim_task` is
   called from exactly one non-test place: `handle_claim_task`. The TUI and CLI
   do not use it. One integration test (`tests/active_health.rs:40`) uses it as
   a "make this task Running with a worktree" fixture step; that gets rewritten
   onto `update_task`.

**Read-then-write hazard (context for the wrap-up):** `ClaimTaskViaMcp` guards
`requires: task.status = backlog` as a read-then-write — `claim_task` reads and
validates, then patches — so two agents could both claim the same Backlog task.
#3802 deliberately left it alone (its scope was *dispatch* entry points, and
`claim_task` provisions nothing). Removing the tool reaches the same end state
as rewriting it on top of the atomic claim, for less code. Note this in wrap-up.

## Order of work

Spec → tests → code, per the repo's TDD rule.

### Step 1 — Spec

- `docs/specs/mcp-task-tools.allium` — delete the whole `rule ClaimTaskViaMcp`
  block (lines ~362–395), which is the only place the `McpClaimTask` trigger
  appears (there is no separate trigger declaration to remove).
- `docs/specs/dispatch.allium:942` — the `RunningTaskHasWorktree` invariant
  names `ClaimTaskViaMcp` explicitly. Reword to reference `DispatchTask` (and
  the research-dispatch sibling) only, keeping the "manual `m`-key moves are an
  override" carve-out intact.
- Run `allium check` on both files. Use the `allium:tend` skill for the edits.

### Step 2 — Tests (red first)

Add a regression test that fails while the tool still exists:

- In `src/mcp/handlers/tests/mod.rs`, assert `claim_task` is absent from
  `TOOL_NAMES` **and** that a `tools/call` for `name: "claim_task"` comes back
  as an unknown-tool JSON-RPC error. The existing `tools/list` test compares
  against `TOOL_NAMES` and is therefore self-consistent — it would stay green
  after removal, so it cannot serve as the regression guard on its own.

Then delete the tests that assert the removed behaviour:

- `src/mcp/handlers/tests/tasks/dispatch.rs` — the nine `claim_task_*` tests
  (lines ~36–380).
- `src/mcp/handlers/tests/tasks/wrap_up.rs:1191` —
  `claim_task_sends_refresh_notification`.
- `src/mcp/handlers/tests/mod.rs:560` — the `claim_task` payload entry in
  `every_tool_with_args_rejects_unknown_field` (leaving it makes that test fail
  on an unknown tool name; it also cross-checks coverage against `TOOL_NAMES`
  at line 608).
- `src/service/tasks/tests.rs` — the six `claim_task_*` service tests
  (lines ~396–620).

Rewrite the fixture caller:

- `tests/active_health.rs:40` — replace the `svc.claim_task(ClaimTaskParams{..})`
  setup with `svc.update_task(UpdateTaskParams{ status: Some(Running), worktree:
  Some(FieldUpdate::Set(..)), tmux_window: Some(FieldUpdate::Set(..)), ..})`.
  The test's own assertions do not depend on `claim_task` seeding
  `last_pre_tool_use_at` — it only asserts `is_some()` *after* firing a
  `PreToolUse` hook event — so no clock seeding is needed. Drop the
  `ClaimTaskParams` import.

### Step 3 — Code

- `src/mcp/handlers/dispatch.rs:254` — remove the `async "claim_task" =>`
  entry from the `mcp_tools!` list. This drops it from `tool_definitions()`,
  `dispatch_tool()`, and `TOOL_NAMES` together.
- `src/mcp/handlers/tasks/dispatch.rs` — delete `handle_claim_task` (lines
  39–69) and prune the now-unused `ClaimTaskArgs` / `ClaimTaskParams` imports
  (`TaskId` may also become unused — check).
- `src/mcp/handlers/tasks/mod.rs` — delete `struct ClaimTaskArgs` (line ~91)
  and drop `handle_claim_task` from the `pub(super) use dispatch::{..}` re-export
  (line 27).
- `src/service/tasks/crud.rs:607` — delete `TaskService::claim_task`.
- `src/service/tasks/params.rs:218` — delete `ClaimTaskParams` and its banner
  comment.
- `src/service/tasks/mod.rs:11` and `src/service/mod.rs:26` — drop
  `ClaimTaskParams` from both re-exports.
- `src/service/api.rs:221` — remove the `claim_task` entry from the
  `task_service_api!` macro list.
- Fix the stale doc comment at `src/service/tasks/crud.rs:835`, which cites
  "one that `claim_task` legitimately took" as a live example.

### Step 4 — Docs

- `docs/module-map.md:101` — drop `claim_task` from the dispatch-handlers row.
- `docs/mcp.md` needs no change: its only `claim` hit (line 43) is about
  `auto_dispatch_next`'s atomic claim, not this tool.
- `docs/plans/**` hits (`388-remove-cost-calculations.md:34`,
  `review-2026-05-05/wp-5-service-layer-split.md:15`,
  `review-2026-05-07/wp-4-service-tasks-decompose.md:21,24`) are historical
  artifacts of completed work — leave them as-is rather than rewriting history.

### Step 5 — Verify

- `allium check` clean on the edited specs; run the `allium:weed` skill to
  confirm no spec/code divergence was introduced.
- `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`
- `cargo clippy --all-targets -- -D warnings` (the pre-push hook runs it; a
  plain `cargo build` will not flag the newly-unused imports as errors).

## Risk

Low. The tool has never been called in 627 recorded task trajectories, nothing
instructs an agent to call it, and no production code path depends on the
service method. The only behavioural consequence is that a hypothetical
out-of-tree MCP client calling `claim_task` would now get an unknown-tool
error — an acceptable break for a tool no agent is told about.
