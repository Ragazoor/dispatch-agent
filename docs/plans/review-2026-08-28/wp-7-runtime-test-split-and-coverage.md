# Runtime Test Split and Coverage

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Split the two largest files in the repository into directories, following the layout `src/db/tests/` and `src/tui/tests/` already use, then close the worst coverage gap in the codebase — `src/runtime/mod.rs` at 47.8%.

## Context

This work package addresses findings from the whole-repository codebase review of 2026-08-28 (`docs/plans/2026-08-28-codebase-review.md`, sections 5.10 and 3).

| File | Lines | Note |
|---|---:|---|
| `src/runtime/tests.rs` | 7,916 | Largest file in the repository — larger than any production file (`src/tmux.rs`, 2,700) |
| `src/service/tasks/tests.rs` | 5,138 | Second largest |
| `src/runtime/mod.rs` | 301 coverable, **47.8%**, 157 uncovered | Worst coverage gap in the codebase |

The split and the coverage work are in the same work package deliberately: raising `src/runtime/mod.rs` coverage means adding tests to `src/runtime/tests.rs`, so doing them in the other order guarantees a conflict.

**Do the split first, then the coverage work.** Two separate commits.

## Findings

### 💡 `src/runtime/tests.rs` is 7,916 lines (`src/runtime/tests.rs`)

**Issue:** Larger than any production file in the repo. Its siblings are already directories — `src/db/tests/` has 13 files, `src/tui/tests/` has 16, `src/mcp/handlers/tests/` has 6 plus a `tasks/` subdirectory. `runtime` and `service/tasks` are the two that never got the same treatment.

**The split is already marked out.** The file contains 28 named `mod` blocks that are natural file boundaries:

| Line | Module | Approx. size |
|---:|---|---:|
| 767 | `base_branch_history_and_task_exec` | 1,560 |
| 2333 | `filter_presets` | 33 |
| 2366 | `parse_raw_presets` | 70 |
| 2436 | `repo_path` | 17 |
| 2453 | `epic_tests` | 213 |
| 2666 | `split_mode` | 475 |
| 3141 | `split_mode_via_msg_tx` | 199 |
| 3340 | `spawn_refresh_from_db_via_msg_tx` | 44 |
| 3384 | `browser_and_tmux_window` | 62 |
| 3446 | `load_init_helpers` | 64 |
| 3510 | `ensure_statusline_settings_file` | 63 |
| 3573 | `feed_epic_trigger` | 694 |
| 4267 | `exec_open_main_session` | 50 |
| 4317 | `exec_check_main_session_liveness` | 59 |
| 4376 | `exec_create_main_session` | 37 |
| 4413 | `load_main_session` | 101 |
| 4514 | `prepare_inputs` | 99 |
| 4613 | `backfill_embeddings` | 123 |
| 4736 | `spawn_refresh_task` | 88 |
| 4824 | `spawn_refresh_epic` | 110 |
| 4934 | `epic_auto_dispatch_and_group_by_repo` | 88 |
| 5022 | `epic_group_by_repo_migration` | 47 |
| 5069 | `frame_rate_cap` | 50 |
| 5119 | `event_loop` | 229 |
| 5361 | `command_dispatch` | **2,120** |
| 7481 | `run_blocking_dispatch` | 115 |
| 7596 | `repo_sync` | 320 |

**Fix:** Convert to `src/runtime/tests/` with a `mod.rs` holding the shared harness and `mod` declarations. Group the small modules rather than creating 28 files — target 6–9 files of roughly 400–1,200 lines each. A reasonable grouping:

- `mod.rs` — shared test harness/helpers (currently at the top of the file, before line 767) plus `mod` declarations
- `command_dispatch.rs` — the 2,120-line block. Still the biggest, and worth splitting further only if a natural seam appears inside it; do not force one
- `task_exec.rs` — `base_branch_history_and_task_exec`
- `feeds.rs` — `feed_epic_trigger`, `epic_auto_dispatch_and_group_by_repo`, `epic_group_by_repo_migration`, `epic_tests`
- `split_mode.rs` — `split_mode`, `split_mode_via_msg_tx`
- `main_session.rs` — the four `exec_*_main_session` modules plus `load_main_session`
- `refresh.rs` — `spawn_refresh_task`, `spawn_refresh_epic`, `spawn_refresh_from_db_via_msg_tx`
- `event_loop.rs` — `event_loop`, `frame_rate_cap`, `run_blocking_dispatch`
- `misc.rs` — `filter_presets`, `parse_raw_presets`, `repo_path`, `browser_and_tmux_window`, `load_init_helpers`, `ensure_statusline_settings_file`, `prepare_inputs`, `backfill_embeddings`, `repo_sync`

**This must be a pure move.** No test body changes, no renames, no reordering within a module. Visibility will need adjusting (`use super::*;` at the top of each new file, and the shared harness items promoted to `pub(super)` or `pub(in crate::runtime::tests)`), but nothing else.

**Land this fast and watch `main`.** A large mechanical re-layout of a file collides badly with a concurrent unrelated edit to the same file on `main`: because nearly every line moves, git's diff3 line-matching is defeated and a plain `git merge` can produce syntactically-valid but semantically-corrupted output that splices fragments of different functions together. Before wrapping up, run `git log --oneline HEAD..main` and check whether anything touched `src/runtime/tests.rs` or `src/service/tasks/tests.rs`. If something did, **re-do the split on top of the new `main`** rather than merging into your version — reapplying a mechanical move is cheap, and reviewing a 7,900-line conflict is not.

### 💡 `src/service/tasks/tests.rs` is 5,138 lines (`src/service/tasks/tests.rs`)

**Issue:** Same problem, second largest file. Its production siblings are already well split — `src/service/tasks/` has `crud.rs`, `dispatch.rs`, `params.rs`, `validators.rs`, `watchers.rs`, `wrap_up.rs`.

**Fix:** Convert to `src/service/tasks/tests/` and mirror the production module names where the tests line up (`crud.rs`, `dispatch.rs`, `validators.rs`, `watchers.rs`, `wrap_up.rs`, plus `mod.rs` for the shared harness). Note the shared helpers `make_task_params` (`:46`) and `make_task` (`:643`) live here and will need `pub(super)`.

Same rule: pure move.

### 💡 `src/runtime/mod.rs` at 47.8% — 157 uncovered lines (`src/runtime/mod.rs`)

**Issue:** The worst coverage gap in the codebase, and the only one not already excused by policy (`docs/testing.md` explicitly excuses `src/setup/`'s OS-interaction branches and render-heavy code; it does not excuse this).

Some of it genuinely resists testing — `run_tui` (`:156`) does terminal setup and teardown against a real TTY. But not all of it. The following are ordinary async functions with clear inputs:

| Location | Function | Testable? |
|---|---|---|
| `:393` | `bootstrap` | Partly — it opens a DB and binds a port |
| `:608` | `next_loop_event` | Yes — event selection logic |
| `:722` | `run_loop<B: Backend>` | Yes — generic over `Backend`, so `TestBackend` works |
| `:769` | `execute_commands<B: Backend>` | **Yes — this is the priority.** The command-queue drain loop |
| `:788` | `load_main_session` | Yes — takes `&dyn db::SettingsStore` |
| `:804` | `load_notifications_pref` | Yes — same |
| `:813` | `load_repo_filter` | Yes — same |
| `:826` | `load_filter_presets` | Yes — same, returns `Option<Message>` |
| `:366` | `db_error` | Trivially yes |
| `:378` | `send_system_error` | Yes |
| `:559` | `invalidate_feed_cache` | Yes |

`execute_commands` is the highest-value target. `docs/architecture.md` documents its drain-loop semantics in detail — a single message can cascade into multiple commands via `queue.extend(extra)` — and that cascade behaviour is exactly the kind of thing that should have a direct test. Note the existing `mod event_loop` and `mod command_dispatch` test modules already exercise nearby ground, so check what is covered before writing new tests.

**Fix:** Add tests for the drain-loop cascade and the four `load_*` settings loaders. Target a meaningful improvement, not a number — the review recommends **not** chasing 100% here, and `docs/testing.md` is explicit that a single file below average is not by itself a problem. Getting `src/runtime/mod.rs` from 47.8% into the 70s by covering the genuinely-testable functions above is a good outcome.

Do **not** contort `run_tui` into testability. Terminal setup against a real TTY is the legitimate part of this gap.

## Changes

| File | Change |
|------|--------|
| `src/runtime/tests.rs` | Delete, replaced by `src/runtime/tests/` |
| `src/runtime/tests/mod.rs` | Shared harness + `mod` declarations |
| `src/runtime/tests/{command_dispatch,task_exec,feeds,split_mode,main_session,refresh,event_loop,misc}.rs` | The 28 `mod` blocks, grouped as above, moved verbatim |
| `src/runtime/mod.rs` | Confirm `mod tests;` still resolves (it will — `tests/mod.rs`) |
| `src/service/tasks/tests.rs` | Delete, replaced by `src/service/tasks/tests/` |
| `src/service/tasks/tests/mod.rs` | Shared harness (`make_task_params`, `make_task`) + `mod` declarations |
| `src/service/tasks/tests/{crud,dispatch,validators,watchers,wrap_up}.rs` | Moved verbatim, mirroring production module names |
| `src/runtime/tests/` | **Second commit:** new tests for `execute_commands`' drain cascade and the four `load_*` loaders |

## Verification

- [ ] **After the split commit:** `cargo test` reports the **same total test count** as before — 4328 lib tests. A different number means a module was dropped or duplicated. Capture the count before you start
- [ ] `cargo test --no-fail-fast` — one blocked target must not hide the six after it
- [ ] `cargo clippy --all-targets -- -D warnings` — clean. Visibility errors from the move surface here
- [ ] `cargo fmt` before committing
- [ ] Confirm the split commit contains **no** test-body changes: `git diff --stat` should show large deletions and large additions, and a careful read of a sample should show identical bodies
- [ ] No file in `src/runtime/tests/` exceeds ~1,500 lines (except `command_dispatch.rs` if no natural seam exists)
- [ ] **After the coverage commit:** `cargo tarpaulin --engine llvm --out stdout` — `src/runtime/mod.rs` is meaningfully above 47.8%, and the overall figure has not dropped below the 88 floor
- [ ] Quote the engine when reporting any coverage number: the default `Auto` engine reads ~1.8 points lower and must not be compared against the floor
- [ ] `./scripts/check-doc-paths.sh` and `./scripts/check-doc-symbols.sh` pass — `docs/testing.md` and `CLAUDE.md` cite test paths that the split may invalidate
- [ ] `./scripts/check-no-test-sleep.sh` passes — no wall-clock sleep in any new test
