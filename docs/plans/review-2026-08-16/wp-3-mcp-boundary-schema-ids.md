# MCP Boundary: Generated Schema + Typed IDs

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Generate the MCP task-field boundary from one declaration instead of six, and replace raw `i64` ids with the existing `TaskId`/`EpicId` newtypes.

## Context

This work package addresses findings from the 2026-08-16 codebase review
(`docs/plans/2026-08-16-codebase-review.md`, commit `c05f512c`).

Adding a task field is currently the most ripple-prone edit in the codebase, and
half its surfaces fail at *runtime* rather than compile time. The pattern needed
to fix it is already proven one layer down in `src/service/api.rs`
(`service_api!`), so this is the cheapest of the large wins.

## Findings

### ⚠️ The task field set is declared six times; only three are compiler-enforced

**Issue:** Adding a task field means touching:

1. The hand-written JSON schema (`src/mcp/handlers/dispatch.rs:121`-onward)
2. `UpdateTaskArgs` (`src/mcp/handlers/tasks/mod.rs:38`)
3. The arg→param mapping (`src/mcp/handlers/tasks/crud.rs:42-101`) — **fifteen consecutive** `if let Some(x) = parsed.x { params = params.x(x) }` blocks
4. `UpdateTaskParams`
5. `updated_field_names`
6. `TaskPatch` / `OwnedTaskPatch`

`docs/conventions.md` documents exhaustive-destructuring enforcement for the
last group, but **1–3 have no enforcement at all**: the schema is prose, and
`deny_unknown_fields` turns a schema/struct mismatch into a runtime `-32602`
rather than a build error. Existing tests spot-check individual tools
(`src/mcp/handlers/tests/epics.rs:963`) but there is no generic parity check.

**Fix:** Introduce a macro that emits the JSON schema, the args struct, and the
arg→param mapping from a single field list — mirroring how `mcp_tools!`
(`src/mcp/handlers/dispatch.rs:38`) already generates the tool registry and how
`service_api!` generates the service seam. Read both macros' doc comments before
writing a third; prefer extending an existing one over adding a parallel
mechanism.

If a full macro proves too invasive, the **minimum acceptable outcome** is a
generic parity test asserting that every field in the args struct appears in the
schema and in the mapping — converting a runtime `-32602` into a test failure.

### 💡 Primitive obsession at the MCP boundary despite existing newtypes

**Issue:** `TaskId`/`EpicId` exist in `src/models/ids.rs` and already derive
`Deserialize`, yet every args struct takes raw `i64` and wraps at the point of
use (`TaskId(parsed.task_id)`):

- `src/mcp/handlers/tasks/mod.rs:40,62,75,84,124,132,152,154,161,170`
- `src/mcp/handlers/epics.rs:27,34,41`
- `src/mcp/handlers/learnings.rs:24,40,57,59,67`
- `src/mcp/handlers/tasks/wrap_up.rs:80`

The sharpest case is `watcher_task_id` and `target_task_id` as two adjacent bare
`i64` (`src/mcp/handlers/tasks/mod.rs:152-154`) — **a swap there is silent**.

The boundary is also internally inconsistent: `status`, `tag`, and `sub_status`
*are* parsed into typed values in the same struct, while `url_type` remains
`Option<String>` re-parsed at `src/mcp/handlers/tasks/crud.rs:77`.

**Fix:** Change the args structs to deserialize `TaskId`/`EpicId` directly.
Parse `url_type` into its enum at the boundary like the other typed fields.
Confirm the JSON wire format is unchanged — these are transparent newtypes over
`i64`, so `serde` output must be identical; assert this with a test.

### 💡 Raw JSON-RPC error codes (33 non-test sites)

**Issue:** Literal `-32602` / `-32603` appear 33 times in non-test code
(e.g. `src/mcp/handlers/tasks/dispatch.rs:207,266,270,294`;
`src/mcp/handlers/tasks/wrap_up.rs:102,110,199,232,281-311`) despite
`service_err_to_response` (`src/mcp/handlers/types.rs:348`) centralising the
mapping.

**Fix:** Name them as constants (e.g. `INVALID_PARAMS`, `INTERNAL_ERROR`) and
use those everywhere. Where a site is re-implementing what
`service_err_to_response` already does, route through it instead.

## Changes

| File | Change |
|------|--------|
| `src/mcp/handlers/dispatch.rs` | Generate the task-field schema from the single declaration; name the error-code literals |
| `src/mcp/handlers/tasks/mod.rs` | `UpdateTaskArgs` and siblings: `i64` → `TaskId`/`EpicId`; generated from the shared declaration |
| `src/mcp/handlers/tasks/crud.rs` | Replace the 15 hand-written `if let Some(..)` mapping blocks with the generated mapping; parse `url_type` at the boundary (`:77`) |
| `src/mcp/handlers/epics.rs` | `i64` → `EpicId`/`TaskId` in args structs |
| `src/mcp/handlers/learnings.rs` | `i64` → typed ids in args structs |
| `src/mcp/handlers/tasks/wrap_up.rs` | `i64` → `TaskId` (`:80`); use named error constants |
| `src/mcp/handlers/types.rs` | Define the named JSON-RPC error-code constants alongside `service_err_to_response` |
| `src/mcp/handlers/tests/tasks/crud.rs` | Add the schema/args/mapping parity test |
| `docs/how-to.md` | Update "adding an MCP tool" to describe the single-declaration flow |
| `docs/specs/mcp-task-tools.allium` | Verify the tool surface is unchanged; update if the boundary types are now spec-visible |

## Verification

- [ ] Run existing tests — all pass (`cargo test`)
- [ ] `cargo test mcp::handlers::tests` passes
- [ ] **Wire-format regression test**: an `update_task` request/response with every field produces byte-identical JSON to before the change (newtypes must be transparent)
- [ ] Parity test fails when a field is added to the args struct but not the schema — verify by deliberately introducing the mismatch
- [ ] Deliberately swap `watcher_task_id`/`target_task_id` at a call site and confirm it is now a **compile error**
- [ ] `tools/list` output unchanged (compare before/after)
- [ ] `cargo test --test cli` and `cargo test --test task_watchers` pass
- [ ] `cargo clippy --all-targets -- -D warnings` clean
