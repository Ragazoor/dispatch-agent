# WP-2 — Runtime Dispatcher Coverage

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Close the one layer in the codebase where a mis-wired `Command` compiles, ships, and passes CI — then tidy the test file that layer's coverage lives in.

## Context

From the follow-up codebase review at `4bf19b04` (`docs/plans/2026-08-16-4220-codebase-review-followup.md`, smells A and D, carried M9; Magic Wand #1).

`commands::dispatch` is the sole `Command` → side-effect entry point. `src/runtime/tests.rs` holds 202 tests, but the overwhelming majority call `rt.exec_*` **directly**, bypassing the dispatcher entirely. Measured coverage of `src/runtime/commands.rs` is **36.8%** — the least-covered real file in the repo — and the whole top-level `match` is unexecuted. It is also in the top-15 most-churned files.

A `mod command_dispatch` already exists in `src/runtime/tests.rs` and is well built: 23 tests, helpers `dispatch_one` / `drain` / `seed`, and an explicit doc comment stating that assertions are on observable effects and never on match shape. **This package extends a proven pattern — it does not design one.**

## Findings

### ⚠️ 34 of 63 `Command` variants never pass through the real dispatcher

**Issue:** `mod command_dispatch` covers 29 of 63 variants. The split is not random — every covered arm has a **DB-observable** effect; every uncovered arm has a **tmux / process / notification** effect:

| Enum | Covered | Total |
|---|---:|---:|
| `SettingsCommand` | 4 | 4 |
| `TodoCommand` | 5 | 6 |
| `EpicCommand` | 5 | 7 |
| `RepoFilterCommand` | 3 | 3 |
| `EditorCommand` | 2 | 2 |
| `TaskCommand` | 8 | 22 |
| `BudgetCommand` | 0 | 1 |
| `FeedCommand` | 0 | 1 |
| `LearningCommand` | 0 | 1 |
| `MainSessionCommand` | 0 | 3 |
| `PrCommand` | 0 | 1 |
| `RepoSyncCommand` | 0 | 2 |
| `SplitCommand` | 0 | 7 |
| `SystemCommand` | 0 | 2 |
| `UsageCommand` | 0 | 1 |

The uncovered half is exactly where a wrong wire is invisible: routing `Split(cmd)` to `dispatch_system` would compile and no test would notice.

**Fix:** Extend `mod command_dispatch` until every variant is driven through `commands::dispatch` at least once. Assertions stay on observable effects — for the process-effect arms that means `MockProcessRunner` call-sequence assertions, which this repo is already strong at (`DispatchScript`, `sync_repo_behind_only_merges_and_does_not_push`).

### 💡 `src/runtime/agents.rs` is a three-line empty module

**Issue:** Its entire content is `use super::*;` followed by `impl TuiRuntime {}`. `src/runtime/mod.rs` still declares `mod agents;`. `docs/module-map.md` documents it as vestigial rather than removing it. The prior review's dead-code sweep looked for functions and missed it.

**Fix:** Delete the file and its `mod agents;` declaration. Update the `src/runtime/{…}.rs` row in `docs/module-map.md` to drop the vestigial-module note (that row also omits the real `budget.rs` and `repo_sync.rs` — WP-3 owns that half; coordinate so you don't both edit the row).

### 💡 `src/runtime/tests.rs` is a 6,430-line flat module with a shadowed helper

**Issue:** One nested `mod` in 6,430 lines; the rest is separated by `// ---` comment banners. `make_app()` is defined at `src/runtime/tests.rs` *and* in `src/tui/tests/helpers.rs`, the former shadowing the latter. Twelve `make_app*` variants exist repo-wide.

**Fix:** Split into nested `mod` blocks along the existing banner boundaries. Resolve the `make_app` shadowing — either reuse the shared helper or rename the local one to say what makes it different (`make_runtime_app`, etc.). Do **not** attempt the full twelve-helper consolidation here; that is a separate, wider change.

## Changes

| File | Change |
|------|--------|
| `src/runtime/tests.rs` | Extend `mod command_dispatch` to cover the remaining 34 `Command` variants |
| `src/runtime/tests.rs` | Convert `// ---` banner sections into nested `mod` blocks; resolve the `make_app` shadowing |
| `src/runtime/agents.rs` | Delete |
| `src/runtime/mod.rs` | Remove `mod agents;` |
| `docs/module-map.md` | Drop the vestigial-`agents.rs` note from the `src/runtime/{…}` row |

## Implementation notes — order matters

Do the coverage work **first**, while the file is still in its current shape, then split. Splitting first creates a large diff that makes the genuinely valuable change hard to review.

- **This is TDD in its natural form:** each new test is written against a dispatcher arm that is currently unexecuted. Write the test, watch it exercise the arm, assert the effect.
- **Prove each test discriminates.** KB #398 applies directly here: a green suite routinely hides vacuous tests, and this repo's mock/`spawn_blocking` layering makes that easy. For at least a representative sample, temporarily mis-wire the arm under test (route it to a different handler) and confirm your test fails. A test that passes against a deliberately broken wire is testing nothing — and a wiring test that can't detect a wrong wire is worse than no test, because it reads as coverage.
- **Beware the detached-spawn trap.** KB #336 and #353: a `MockProcessRunner` panic on a detached `spawn_blocking` thread does *not* fail the test, and an under-scripted runner can make an error-path test pass on the mock's own panic. Several uncovered arms (`Split*`, `Task::Cleanup`, `Task::KillTmuxWindow`) spawn work. Script the runner fully and use `DispatchScript::assert_matches` where a dispatch sequence is involved.
- **Derive mock indices, never hardcode.** KB #384: use `script.index_of`, not `calls[N]`.
- Some arms fire an `McpEvent` or a `Message` rather than touching the DB — assert on the emitted value. `state.test_hooks.bg_write_done_tx` exists for exactly this kind of detached-write observation; check whether the runtime has an equivalent seam before inventing one.
- No behaviour changes anywhere in this package, so no Allium spec edits are expected. If you find yourself needing to change production code to make an arm testable, stop and confirm — that is a finding, not a step.

## Verification

- [ ] `cargo test` green — redirect, don't pipe: `cargo test > /tmp/claude-1000/t.txt 2>&1; echo $?`
- [ ] `cargo clippy --all-targets -- -D warnings` clean
- [ ] `cargo tarpaulin --engine llvm --skip-clean --out Stdout` shows `src/runtime/commands.rs` materially above its 36.8% baseline (target: 85%+)
- [ ] Every `Command` sub-enum appears at least once inside `mod command_dispatch` — enumerate and check, don't eyeball
- [ ] At least three tests verified to fail against a deliberately mis-wired arm, then reverted
- [ ] `rg 'mod agents' src/` returns nothing
- [ ] `./scripts/check-doc-paths.sh` and `./scripts/check-doc-symbols.sh` pass (the module-map edit is under their watch)
