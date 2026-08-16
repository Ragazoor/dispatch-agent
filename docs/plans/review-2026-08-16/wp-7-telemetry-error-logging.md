# Telemetry Write-Error Logging

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Stop silently discarding `record_usage_event` write errors in three detached spawns, behind one shared helper.

## Context

This work package addresses findings from the 2026-08-16 codebase review
(`docs/plans/2026-08-16-codebase-review.md`, commit `c05f512c`).

> ⚠️ **File overlap.** This package touches `src/runtime/commands.rs` (also in
> **WP-2**) and `src/mcp/handlers/dispatch.rs` (also in **WP-3**). It is
> deliberately tiny — three one-line changes plus a helper — so it should land
> **first**, before WP-2 and WP-3 start, or be folded into whichever of them
> runs first. Do not dispatch it in parallel with those two.

## Findings

### 💡 Telemetry DB writes are discarded silently in three detached spawns

**Issue:** All three sites do:

```rust
tokio::spawn(async move { let _ = db.record_usage_event(&event).await; });
```

- `src/runtime/commands.rs:77`
- `src/mcp/handlers/dispatch.rs:675`
- `src/mcp/handlers/dispatch.rs:693`

No `?`, no `tracing::warn!`. `docs/conventions.md` ("Intentional `let _ =`")
explicitly forbids discarding a DB write's `Result` — the sanctioned uses of
`let _ =` are things like `tx.send` on a closed channel, not writes.

This matters beyond tidiness: the keybinding-telemetry design states that the
**absence** of a count must mean "unused", because pruning passes read those
counts to decide what to remove. A silently dropped write makes a *used* binding
look unused — so a swallowed error here can cause a real keybinding to be pruned.

**Fix:** Add a small shared helper that spawns the write and logs on `Err` with
`tracing::warn!`, including enough context to identify the event. Route all
three sites through it. Keep the fire-and-forget shape — the point is
observability, not making callers await telemetry.

Note the three sites are near-identical, which is why a helper is preferable to
three inline `if let Err(e) = … { warn!(…) }` blocks.

## Changes

| File | Change |
|------|--------|
| `src/runtime/commands.rs:77` | Route through the shared helper instead of `let _ =` |
| `src/mcp/handlers/dispatch.rs:675` | Route through the shared helper |
| `src/mcp/handlers/dispatch.rs:693` | Route through the shared helper |
| (helper location) | Place next to `record_usage_event`'s other callers — likely `src/service/` or a small `usage` module; avoid creating a new top-level module for one function |
| `docs/conventions.md` | If the "Intentional `let _ =`" section does not already name telemetry writes as a non-example, add it |

## Verification

- [ ] Run existing tests — all pass (`cargo test`)
- [ ] `grep -rn "let _ = .*record_usage_event" src/` returns nothing
- [ ] Add a test that a failing `record_usage_event` produces a warning rather than being swallowed — inject a failing store and assert the log/behaviour; a test that only asserts the happy path does not cover this finding
- [ ] Confirm the spawns remain fire-and-forget: no caller newly awaits telemetry, and no test needs to wait on it
- [ ] `cargo test runtime::` and `cargo test mcp::handlers::tests` pass
- [ ] `cargo clippy --all-targets -- -D warnings` clean
