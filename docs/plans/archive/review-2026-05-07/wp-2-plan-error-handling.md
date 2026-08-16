# WP2: Replace prod `.unwrap()` in `src/plan.rs`

## Context

`src/plan.rs` parses markdown plan files to extract a title and description for tasks. The current implementation panics on malformed input via several `.unwrap()` calls (~lines 90–140). A malformed plan attached via `/queue-plan` or `update_task --plan_path` should surface as a user-visible validation error, not crash the runtime.

## Findings

- **Severity:** High
- **File:** `src/plan.rs` (~lines 90–140)
- **Issue:** Production code uses `.unwrap()` on plan parsing. Any malformed plan file would panic the dispatch process.
- **Suggestion:** Propagate failures as `ServiceError::Validation` (or `anyhow` at the outer edge — match the existing call-site convention).

## Plan (TDD)

1. **Audit** `src/plan.rs` and list every `.unwrap()` / `.expect()`. Classify each: invariant violation (keep `expect` with message) vs. user-input failure (convert to `?`).
2. **Test first:**
   - Test that a plan file missing a title returns a validation error (no panic).
   - Test that an empty plan file returns a validation error.
   - Test that a plan with only whitespace returns a validation error.
   - Test the happy path stays unchanged.
3. **Implement:** convert user-input `.unwrap()`s to `?` and adjust the function signature to return `Result<Plan, ServiceError>` (or `anyhow::Result` if the caller already uses anyhow — check call sites first).
4. **Update callers** in `src/mcp/handlers/` to map the new error to JSON-RPC `-32602 Invalid params`.

## Files to change

| File | Change |
|---|---|
| `src/plan.rs` | Replace user-input `.unwrap()`s with `?`; choose error type based on existing call-site contract. |
| `src/mcp/handlers/tasks.rs` (or wherever plan parsing is invoked) | Map plan errors to `-32602` validation errors. |
| `src/plan.rs` tests | Add the 4 tests above. |

## Verification

```bash
cargo test plan
cargo test mcp::handlers
cargo clippy --all-targets -- -D warnings
```

Manually attach a malformed plan via `update_task --plan_path` and confirm the MCP response is a validation error, not a panic.
