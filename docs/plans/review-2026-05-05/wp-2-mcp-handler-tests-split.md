# WP-2: MCP Handler Test Split

## Context

`src/mcp/handlers/tests.rs` has grown to 8,550 lines with 221 test functions covering tasks, epics, learnings, and review tools. It is hard to navigate, slow to scan for a specific test, and costly to rebase when multiple features touch the same file.

## Findings

### M1 — Monolithic MCP handler test file
- **Severity**: medium
- **Files**: `src/mcp/handlers/tests.rs` (8,550 lines, 221 tests)
- **Issue**: All MCP handler integration tests live in a single file. Finding a test for a specific tool requires grepping. Adding tests for a new tool means appending to a file that already has 8k lines.
- **Fix**: Convert to a `tests/` sub-module under `src/mcp/handlers/`. Split by domain:
  - `src/mcp/handlers/tests/mod.rs` — shared helpers (test db, state setup)
  - `src/mcp/handlers/tests/tasks.rs` — task tool tests
  - `src/mcp/handlers/tests/epics.rs` — epic tool tests
  - `src/mcp/handlers/tests/learnings.rs` — learning tool tests
  - `src/mcp/handlers/tests/review.rs` — review/alert tool tests
  - `src/mcp/handlers/tests/projects.rs` — project tool tests (if any)

## Implementation Notes

This is a pure refactor — no logic changes.

1. Create `src/mcp/handlers/tests/` directory
2. Move shared test helpers (database setup, `make_state`, etc.) to `mod.rs`
3. Split tests by tool prefix (`create_task`, `update_task`, etc. → `tasks.rs`; `create_epic` etc. → `epics.rs`)
4. Update `src/mcp/handlers/dispatch.rs` (or wherever `mod tests` is declared) to use `mod tests` pointing at the new directory
5. Confirm all 221 tests still pass

## Changes Table

| File | What to change |
|---|---|
| `src/mcp/handlers/tests.rs` | Delete after contents are migrated |
| `src/mcp/handlers/tests/mod.rs` | New — shared helpers |
| `src/mcp/handlers/tests/tasks.rs` | New — task tool tests |
| `src/mcp/handlers/tests/epics.rs` | New — epic tool tests |
| `src/mcp/handlers/tests/learnings.rs` | New — learning tool tests |
| `src/mcp/handlers/tests/review.rs` | New — review + alert tests |
| `src/mcp/handlers/dispatch.rs` | Update `mod tests` declaration |

## Verification

```bash
cargo test mcp::handlers::tests
```

All 221 tests must pass. Count should not change.
