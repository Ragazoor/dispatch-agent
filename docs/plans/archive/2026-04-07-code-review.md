# Code Review Report: Dispatch

**Date**: 2026-04-07
**Codebase**: 40,665 lines of Rust across 26 source files
**Tests**: 1,253 passing (1,239 unit + 14 integration), 0 failures

---

## Executive Summary

- **Well-architected**: Elm-inspired Message/Command pattern with clean layered architecture, consistently applied across TUI, runtime, service, and data layers
- **Excellent test count** (1,253 tests) but **significant coverage gaps** in keyboard input handling (1,029 lines untested), database queries (1,001 lines tested only indirectly), and migrations (516 lines untested)
- **TUI layer is the primary complexity hotspot**: `tui/mod.rs` (3,594 LOC), `tui/ui.rs` (3,457 LOC), and `runtime.rs` (2,712 LOC) total ~10K lines — the `App` struct is a 20-field god object with a 60+ variant `Message` enum
- **Clean compiler state**: Zero clippy warnings, zero TODO/FIXME comments, no dead code
- **Key risk**: `unreachable!()` in production MCP handler (`tasks.rs:557`) will panic on unexpected input instead of returning an error

---

## 1. Architecture & Patterns

### Pattern: Elm-inspired Message/Command + Layered Architecture

The architecture is a hybrid:
1. **Presentation** (TUI): `App::update(Message) -> Vec<Command>` — pure state transformation
2. **Application** (Runtime): Executes `Command`s as side effects (DB writes, process spawns)
3. **Domain** (Service/Models): Business logic via `TaskService`/`EpicService` with injected `Arc<dyn TaskStore>`
4. **Infrastructure** (DB/Process): `TaskStore` trait abstracts SQLite, `ProcessRunner` trait abstracts shell commands

### Consistency: 8.5/10

The pattern is applied uniformly. The one documented exception — inline mutation in `input.rs` for UI-only state — is intentional and well-documented in CLAUDE.md. Visibility is enforced via `pub(in crate::tui)` on `App` fields.

### Architectural Concerns

| Finding | Location | Severity |
|---------|----------|----------|
| `App` struct is a god object (20 fields, 3,594 LOC file) | `tui/mod.rs:37` | Medium |
| `Message` enum has 60+ variants — hard to reason about valid transitions | `tui/types.rs:127` | Medium |
| MCP handlers create `TaskService` inline per request instead of sharing | `mcp/handlers/tasks.rs` | Low |
| Runtime file (2,712 LOC) has 44+ `exec_*` methods | `runtime.rs` | Low |

---

## 2. Test Coverage

**1,253 tests total** (1,239 unit + 14 integration). All passing.

### Well-Tested Areas
- Task/epic state transitions, CRUD operations
- TUI message handling (672 tests in `tui/tests.rs`)
- MCP tool handlers (127 tests)
- Database persistence with real in-memory SQLite (86 tests)
- GitHub PR parsing (35 tests)
- Dispatch logic (74 tests with MockProcessRunner)

### Critical Untested Areas

| Module | Lines | Risk | Why It Matters |
|--------|-------|------|---------------|
| `tui/input.rs` — all keyboard handling | 1,029 | **High** | No regression detection for keybinding changes |
| `db/queries.rs` — SQL implementations | 1,001 | **High** | Tested indirectly via `Database` API, but SQL bugs hard to isolate |
| `db/migrations.rs` — schema upgrades | 516 | **High** | Data loss risk on upgrades; zero direct migration tests |
| `main.rs` — CLI argument parsing | 279 | Medium | Only 3 CLI integration tests |
| `mcp/mod.rs` — server setup, event flow | 68 | Medium | Server startup and routing untested |

### Unit/Integration Ratio

**99:1** — heavily weighted toward unit tests. Integration tests only cover happy-path lifecycle for tasks and epics. Missing: MCP HTTP endpoint tests, concurrent operations, error recovery paths.

### Unused Dependency
`proptest` is declared in dev-dependencies but never used anywhere in tests.

---

## 3. Complexity Hotspots

### Largest Source Files (excluding tests)

| File | Lines | Role |
|------|-------|------|
| `tui/mod.rs` | 3,594 | App state machine |
| `tui/ui.rs` | 3,457 | Rendering |
| `runtime.rs` | 2,712 | Event loop + command execution |
| `dispatch.rs` | 2,601 | Agent lifecycle |
| `models.rs` | 2,200 | Domain types |
| `service.rs` | 1,296 | Business logic |
| `github.rs` | 1,268 | GitHub integration |

### Largest Enums/Structs

| Type | Location | Size | Concern |
|------|----------|------|---------|
| `Message` | `tui/types.rs:127` | 60+ variants | Dispatch target too large |
| `Command` | `tui/types.rs:368` | ~50 variants | Correlated growth |
| `InputMode` | `tui/types.rs:501` | 20+ variants | Could split by domain |
| `App` | `tui/mod.rs:37` | 20 fields | God object |
| `ReviewBoardState` | `tui/types.rs:689` | 11+ fields | Complex nested state |

### Functions with Most Parameters (6+)

| Function | File | Params |
|----------|------|--------|
| `dispatch_review_agent()` | `dispatch.rs:858` | 7 |
| `dispatch_fix_agent()` | `dispatch.rs:1009` | ~8 |
| `build_task_list_item()` | `tui/ui.rs:529` | 7 |
| `finish_task()` | `dispatch.rs:356` | 7 |

### Repeated Computation in Rendering
`render_columns()` and `render_epic_item()` in `tui/ui.rs` filter subtask lists multiple times per frame — same filter/count pattern appears 3+ times for status bucketing. Should pre-compute once.

---

## 4. Code Smells

### High Severity

1. **`unreachable!()` in production MCP handler** — `mcp/handlers/tasks.rs:557`
   ```rust
   match parsed.action.as_str() {
       "rebase" => { ... }
       "pr" => { ... }
       _ => unreachable!(),
   }
   ```
   If a client sends an unexpected action string, this panics. Should return `JsonRpcResponse::err()`.

2. **Stringly-typed `WrapUpArgs.action`** — `mcp/handlers/tasks.rs:103`
   ```rust
   pub(super) action: String, // "rebase" | "pr"
   ```
   Should be an enum with serde deserialization, making the `unreachable!()` a compile-time guarantee.

### Medium Severity

3. **Duplicated review-state parsing** — `github.rs:93` and `dispatch.rs:813` both parse `"APPROVED" | "CHANGES_REQUESTED"` strings but handle different sets of values. Should consolidate.

4. **Primitive obsession in MCP handlers** — Status, sub-status, and tag are `Option<String>` in handler args (`tasks.rs:28,42,44`) instead of typed enums. Validation happens downstream instead of at deserialization.

5. **Silent failure on DB errors** — `runtime.rs` logs warnings on failed `patch_task` calls but doesn't propagate errors:
   ```rust
   if let Err(e) = db.patch_task(task_id, &patch) {
       tracing::warn!(...);
   }
   ```

### Low Severity

6. **High `.unwrap()` count in non-test code** — 1,090 total occurrences, ~332 in production code (excluding test files). Most are in `dispatch.rs` (115) and `runtime.rs` (91). Many are likely safe (e.g., regex compilation) but worth auditing.

7. **No TODO/FIXME comments found** — This is actually good; no known technical debt markers.

---

## 5. Magic Wand: Top 3 Changes

### 1. Split `App` into focused sub-structs

**Impact**: Developer productivity + maintainability

The `App` struct manages board state, input state, agent tracking, review board, security board, filters, usage stats, and merge queues — all in one 3,594-line file. Breaking it into:
```rust
pub struct App {
    board: BoardState,           // tasks, epics, selections, view_mode
    input: InputState,           // mode, cursor, text buffers
    agents: AgentState,          // tracking, tmux outputs, merge queue
    integrations: IntegrationState, // review, security, usage, filters
}
```
would make each subsystem independently testable and reduce cognitive load when adding features.

### 2. Add input handler tests (`tui/input.rs`)

**Impact**: Reducing bugs

1,029 lines of keyboard handling code with zero tests is the biggest regression risk. Every keybinding change, mode transition, and navigation shortcut is untested. A test harness that creates an `App`, sends `KeyEvent`s, and asserts resulting `Message`s/state changes would catch regressions cheaply.

### 3. Type the MCP action/status strings as enums

**Impact**: Maintainability + reducing bugs

Converting `WrapUpArgs.action: String` to an enum, and status/sub-status/tag `Option<String>` fields to their corresponding domain enums, would:
- Eliminate the `unreachable!()` panic
- Move validation to deserialization time
- Make invalid states unrepresentable
- Let the compiler catch missing match arms when new variants are added

---

## 6. CLAUDE.md Improvements

The current CLAUDE.md is excellent — one of the best project docs I've seen. A few additions that would help:

### Missing Context
- **Error handling strategy**: The codebase mixes `anyhow::Result`, silent `warn!()` logging, and `unwrap()`. Documenting the intended pattern (when to propagate vs log vs panic) would help contributors.
- **MCP protocol version/compliance**: Which JSON-RPC/MCP spec version is targeted? This affects how strictly handlers should validate inputs.

### Undocumented Conventions
- **Inline mutation rule scope**: CLAUDE.md documents this for `input.rs` but the same convention applies in `update()` for some `Message` handlers. Clarifying the boundary (any UI-only state, not just input.rs) would prevent confusion.
- **Test naming convention**: Tests follow `snake_case_describing_behavior` but this isn't stated. Worth a one-liner.

### Missing Workflows
- **How to run a single test**: `cargo test test_name` — basic but helpful for newcomers.
- **How to add a new tag type**: Tags drive dispatch behavior but the process for adding one isn't documented (modify `TaskTag` enum, update `DispatchMode::for_task()`, add tag key in input handler).

---

## Prioritized Action Items

### Quick Wins (hours)

| # | Item | Location | Impact |
|---|------|----------|--------|
| 1 | Replace `unreachable!()` with `JsonRpcResponse::err()` | `mcp/handlers/tasks.rs:557` | Prevents production panic |
| 2 | Convert `WrapUpArgs.action` to enum | `mcp/handlers/tasks.rs:100-103` | Type safety |
| 3 | Remove unused `proptest` dev-dependency | `Cargo.toml` | Cleanliness |
| 4 | Pre-compute subtask status counts once per frame | `tui/ui.rs:660-690` | Performance |

### Medium Effort (days)

| # | Item | Location | Impact |
|---|------|----------|--------|
| 5 | Add keyboard input handler tests | `tui/input.rs` | Regression safety |
| 6 | Type MCP handler string fields as domain enums | `mcp/handlers/tasks.rs` | Validation at boundary |
| 7 | Consolidate review-state parsing | `github.rs` + `dispatch.rs` | Remove duplication |
| 8 | Add database migration tests | `db/migrations.rs` | Data safety on upgrades |
| 9 | Consolidate `dispatch_review_agent` / `dispatch_fix_agent` | `dispatch.rs:858,1009` | Reduce duplication |

### Larger Efforts (weeks)

| # | Item | Location | Impact |
|---|------|----------|--------|
| 10 | Split `App` struct into focused sub-structs | `tui/mod.rs` | Maintainability |
| 11 | Group `Message` enum into sub-enums | `tui/types.rs` | Readability |
| 12 | Add MCP server integration tests (HTTP level) | `mcp/` | End-to-end confidence |
| 13 | Set up cargo-tarpaulin for coverage reporting | CI | Visibility into gaps |
