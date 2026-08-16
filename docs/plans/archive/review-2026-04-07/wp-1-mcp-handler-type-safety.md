# MCP Handler Type Safety

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Eliminate panic risk and stringly-typed code in MCP handlers by converting string fields to enums and replacing `unreachable!()` with proper error responses.

## Context

This work package addresses findings from a code review. The MCP handler layer accepts untrusted JSON-RPC input from agents. String-based dispatch with `unreachable!()` fallbacks means unexpected input causes a server panic instead of a graceful error.

## Findings

### :rotating_light: Replace `unreachable!()` with error response (`src/mcp/handlers/tasks.rs:557`)

**Issue:** The `handle_wrap_up` function matches `parsed.action.as_str()` against `"rebase"` and `"pr"`, with `_ => unreachable!()` as the fallback. If a client sends any other string, the MCP server panics.

**Fix:** Replace `unreachable!()` with `JsonRpcResponse::err(id, -32602, format!("unknown wrap_up action: {}", parsed.action))`. Better yet, make this a compile-time guarantee by converting to an enum (see next finding).

### :rotating_light: Convert `WrapUpArgs.action` to enum (`src/mcp/handlers/tasks.rs:100-103`)

**Issue:** `WrapUpArgs { action: String }` with comment `// "rebase" | "pr"` is a stringly-typed field. The valid values are known at compile time but enforced at runtime via match arms.

**Fix:** Define an enum:
```rust
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum WrapUpAction {
    Rebase,
    Pr,
}
```
Replace `action: String` with `action: WrapUpAction`. Serde will reject invalid values at deserialization time with a clear error. The match in `handle_wrap_up` becomes exhaustive — no `unreachable!()` needed.

### :warning: Type status/sub_status/tag as domain enums (`src/mcp/handlers/tasks.rs:28,42,44,96`)

**Issue:** `UpdateTaskArgs` uses `Option<String>` for `status`, `sub_status`, and `tag` fields. These are parsed from strings into domain enums downstream, but validation errors surface late and with poor messages.

**Fix:** Deserialize directly into `Option<TaskStatus>`, `Option<SubStatus>`, `Option<TaskTag>` using serde. Add `#[serde(deserialize_with = ...)]` or implement `Deserialize` on the domain enums if not already present. This moves validation to the JSON-RPC boundary where it belongs.

## Changes

| File | Change |
|------|--------|
| `src/mcp/handlers/tasks.rs` | Add `WrapUpAction` enum, replace `action: String` with `action: WrapUpAction`, remove `unreachable!()`, type status/sub_status/tag fields |
| `src/mcp/handlers/types.rs` | Add serde support for domain enum deserialization if needed |
| `src/models.rs` | Add/verify `Deserialize` impl on `TaskStatus`, `SubStatus`, `TaskTag` |
| `src/mcp/handlers/tests.rs` | Add tests for invalid action/status/tag values returning errors |

## Verification

- [ ] Run existing tests — all pass
- [ ] Send invalid `action` to `wrap_up` tool — returns JSON-RPC error, not panic
- [ ] Send invalid `status`/`tag` to `update_task` — returns clear error message
- [ ] `cargo clippy -- -D warnings` passes
