# Service/Adapter Boundary & Command Migration

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Remove the service→adapter dependency inversion by moving three pure predicates into `src/models`, and finish the half-done `Command` migration.

## Context

This work package addresses findings from the 2026-08-16 codebase review
(`docs/plans/2026-08-16-codebase-review.md`, commit `c05f512c`).

The review found the layering otherwise clean — `grep` for
`crate::tui|crate::runtime|crate::mcp` across `src/service`, `src/db`, and
`src/models` returns **zero** hits. These are the remaining exceptions in the
other direction.

> ⚠️ **File overlap.** This package touches `src/service/tasks/crud.rs` (also in
> **WP-2**) and `src/tui/types.rs` (also in **WP-5**). Sequence it **after**
> WP-2 and WP-5, or coordinate — do not dispatch all three in parallel.

## Findings

### 💡 Service layer depends outward on the dispatch adapter

**Issue:** Three sites invert the layering — `src/service` reaching into
`src/dispatch`, an adapter:

- `src/service/tasks/crud.rs:658` → `crate::dispatch::is_wrappable`
- `src/service/tasks/crud.rs:68` → `crate::dispatch::prompts::parse_tmux_window_task_id`
- `src/service/grouping.rs:7` → `crate::dispatch::repo_name_from_path`

All three are **pure predicates** with no IO and no process spawning. As
written, `TaskService` cannot be reasoned about without reading the
agent-launcher module.

**Fix:** Move all three into `src/models` (or a small pure helper module beside
it) and update both the service and dispatch call sites to import from there.
This is a pure move — no behaviour change. Keep the existing tests with the
functions; if they currently live in `src/dispatch`'s test modules, move them
too so the tests travel with the code.

Note `parse_tmux_window_task_id` (`src/dispatch/prompts.rs:77`) pairs with a
construct site at `src/dispatch/prompts.rs:63` — the two together are a de-facto
tmux-window-name type. Moving the parser is in scope; introducing the newtype is
**not** (it would ripple into ~104 sites). If you see the opportunity, record it
via `record_learning` or open a follow-up task rather than expanding this one.

### 💡 The `Command` migration is half-done

**Issue:** `src/tui/types.rs` owns `Message` and `Command`, which every
`src/runtime` file consumes — making `crate::tui` the codebase's god-module
(2,125 references, 43.9k lines, 32% of the crate). A migration to
`src/tui/commands/` is underway but stalled: 15 domain-nested variants coexist
with **5 stragglers still inline** at `src/tui/types.rs:170`:

- `SaveRepoPath`
- `SaveBaseBranch`
- `PersistSetting`
- `PersistStringSetting`
- `RecordUsageEvent`

A half-done migration is worse than either endpoint — the next agent cannot tell
which convention is current.

**Fix:** Move the five stragglers into the appropriate module under
`src/tui/commands/` alongside the 15 already migrated. Follow the existing
grouping convention (see `src/tui/commands/task.rs` for the established shape).
These five are settings/persistence-flavoured, so they likely want a
`settings.rs` sibling rather than being scattered.

Also fix the stale citation this exposes: `docs/architecture.md:44` cites
`Command::QuickDispatch { draft … }` in `src/tui/mod.rs`, but it now lives at
`src/tui/commands/task.rs::QuickDispatch` and is emitted from
`src/tui/input/normal.rs:646`. *(WP-4 also covers this line — whichever lands
second should verify it is already correct rather than re-editing.)*

## Changes

| File | Change |
|------|--------|
| `src/models/` | Receive `is_wrappable`, `parse_tmux_window_task_id`, `repo_name_from_path` (+ their tests) |
| `src/service/tasks/crud.rs:68,658` | Import the predicates from `src/models` instead of `crate::dispatch` |
| `src/service/grouping.rs:7` | Import `repo_name_from_path` from `src/models` |
| `src/dispatch/mod.rs`, `src/dispatch/prompts.rs` | Remove the moved definitions; re-import from `src/models` at remaining call sites |
| `src/tui/types.rs:170` | Remove the 5 inline `Command` variants |
| `src/tui/commands/` | Add the 5 migrated variants (likely a new `settings.rs`, following `task.rs`'s shape) |
| `src/runtime/` | Update imports for the migrated variants |
| `docs/architecture.md:44` | Fix the `Command::QuickDispatch` citation if WP-4 has not already |
| `docs/module-map.md` | Update if the module responsibilities shift |

## Verification

- [ ] Run existing tests — all pass (`cargo test`)
- [ ] `grep -rn "crate::dispatch" src/service/` returns nothing
- [ ] `grep -rn "crate::tui\|crate::runtime\|crate::mcp" src/service src/db src/models` still returns nothing (the invariant must not regress)
- [ ] No `Command` variants remain inline in `src/tui/types.rs` — all live under `src/tui/commands/`
- [ ] This is a pure move: `cargo test tui::tests::snapshots` shows **no** snapshot changes, and no behavioural test needed editing beyond import paths
- [ ] `cargo test service::` and `cargo test dispatch::` pass
- [ ] `./scripts/check-doc-symbols.sh` passes — the moved symbols are cited in docs
- [ ] `cargo clippy --all-targets -- -D warnings` clean
