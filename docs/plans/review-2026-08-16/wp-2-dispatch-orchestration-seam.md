# Dispatch Orchestration Seam

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Collapse three hand-written copies of the dispatch orchestration flow into a single `TaskServiceApi::dispatch` seam, and move wrap-up/rebase business logic out of the MCP transport.

## Context

This work package addresses findings from the 2026-08-16 codebase review
(`docs/plans/2026-08-16-codebase-review.md`, commit `c05f512c`).

This is the highest bug-risk item in the review. `DispatchClaimExclusive` (see
`docs/specs/dispatch.allium`) is the most safety-critical rule in the system,
and it is currently enforced by three independent hand-written copies whose
comments have already drifted apart. Coverage corroborates the risk:
`src/runtime/commands.rs` is the least-covered real file in the repo at **40%**,
and `src/runtime/mod.rs` is at 51%.

**Spec-first.** Per `CLAUDE.md`, behaviour changes start in the spec. This work
package should be *behaviour-preserving* — if you find the three copies disagree
on actual behaviour (likely, given the comment drift), that is a spec question:
stop and resolve which behaviour is correct against `docs/specs/dispatch.allium`
before collapsing them. Do not silently pick one copy's behaviour.

## Findings

### ⚠️ Dispatch orchestration is triplicated across two layers

**Issue:** The sequence *claim → `prepare_inputs` → `spawn_blocking(dispatch_agent|research_agent)`
→ patch `worktree`/`tmux_window` → `release_claim` on failure* is written out
three independent times:

- `src/runtime/tasks.rs::exec_dispatch_agent`
- `src/mcp/handlers/tasks/dispatch.rs::handle_dispatch_task`
- `src/mcp/handlers/tasks/dispatch.rs::auto_dispatch_next`

The `DispatchMode` match is literally duplicated between
`src/mcp/handlers/tasks/dispatch.rs::do_dispatch` and the closure at
`src/runtime/tasks.rs:376`. Only `exec_quick_dispatch` reuses anything
(`src/runtime/tasks.rs::claim_for_dispatch`). There is no `TaskServiceApi::dispatch`.

**Fix:** Add a single dispatch method to the service layer owning the whole
sequence. `exec_dispatch_agent`, `handle_dispatch_task`, and `auto_dispatch_next`
become thin callers that supply inputs and handle their own transport-shaped
responses. Follow the existing service-trait pattern in `src/service/api.rs`
(the `service_api!` macro) so the new method is part of the generated seam.

### ⚠️ Wrap-up/rebase business logic lives in the MCP transport (`src/mcp/handlers/tasks/wrap_up.rs`)

**Issue:** `finish_wrap_up_rebase` calls `dispatch::finish_task` under
`spawn_blocking`, sets and clears `SubStatus::Conflict`, and fires three
notification kinds — the entire `WrapUpRebase` rule implemented inside a
JSON-RPC handler. The same file reaches past every abstraction directly to
`crate::tmux::kill_window` (`src/mcp/handlers/tasks/wrap_up.rs:400`), the only
MCP handler that touches tmux.

This is defensible *only* because there is currently no board-initiated finish
path (`docs/specs/pr-workflow.allium` states wrap-up is MCP-only). The moment
one is added, this becomes copy #2 of the same drift problem.

**Fix:** Move the rebase/conflict/notification sequence behind the service
layer alongside the dispatch seam. The handler should own request parsing and
response shaping only. Route the `kill_window` call through the same abstraction
the runtime uses rather than calling `crate::tmux` directly from a handler.

### 💡 Telemetry DB writes discarded silently (`src/runtime/commands.rs:77`)

**Issue:** `tokio::spawn(async move { let _ = db.record_usage_event(...).await; })`
— no `?`, no `tracing::warn!`. `docs/conventions.md` ("Intentional `let _ =`")
explicitly forbids discarding a DB write's `Result`. The keybinding-telemetry
design states that the *absence* of a count must mean "unused", because pruning
passes read it — so a silently dropped write makes a used binding look unused.

**Fix:** Log on `Err` with `tracing::warn!`. Two sibling sites exist at
`src/mcp/handlers/dispatch.rs:675,693` — those are **owned by WP-7**, which
extracts a shared helper. Coordinate: if WP-7 has already landed, use its
helper here rather than adding a fourth inline copy.

## Changes

| File | Change |
|------|--------|
| `docs/specs/dispatch.allium` | Confirm the collapsed flow matches the spec; update via `allium:tend` if behaviour is clarified. Resolve any disagreement between the three copies here first |
| `src/service/api.rs` | Add the `dispatch` method to the service-API seam |
| `src/service/tasks/` | Implement the orchestration: claim → prepare → spawn → patch → release-on-failure, including the `DispatchMode` match |
| `src/runtime/tasks.rs` | Reduce `exec_dispatch_agent` to a thin caller; remove the duplicated `DispatchMode` closure at `:376`; keep `exec_quick_dispatch` on the shared path |
| `src/mcp/handlers/tasks/dispatch.rs` | Reduce `handle_dispatch_task`, `auto_dispatch_next`, and `do_dispatch` to thin callers |
| `src/mcp/handlers/tasks/wrap_up.rs` | Move `finish_wrap_up_rebase`'s business logic to the service layer; remove the direct `crate::tmux::kill_window` call at `:400` |
| `src/runtime/commands.rs:77` | Add `warn!` on `Err` for the discarded `record_usage_event` write |
| `src/mcp/handlers/tests/tasks/dispatch.rs` | Update/extend handler tests against the new seam |
| `src/runtime/tests.rs` | Update runtime tests; add coverage for the seam (this file's subject is currently at 40%) |

## Verification

- [ ] Run existing tests — all pass (`cargo test`, needs `tmux` on PATH)
- [ ] `cargo test --test lifecycle` and `cargo test --test dispatch_status_lifecycle` pass
- [ ] `cargo test mcp::handlers::tests` and `cargo test service::` pass
- [ ] Write tests **first** (TDD) covering: successful dispatch, failure-path `release_claim`, research-vs-dispatch mode routing, and concurrent-claim exclusion — these are the invariants the three copies were each asserting separately
- [ ] `allium:weed` reports no drift between `docs/specs/dispatch.allium` and the new implementation
- [ ] Confirm `DispatchClaimExclusive` still holds: a second dispatch of an already-claimed task must be rejected on **every** entry point (runtime, MCP `dispatch_task`, `auto_dispatch_next`, quick dispatch)
- [ ] Coverage of `src/runtime/commands.rs` improves from its 40% baseline (`cargo tarpaulin --out Html --skip-clean`)
- [ ] `cargo clippy --all-targets -- -D warnings` clean
