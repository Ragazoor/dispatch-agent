# Codebase Review — `dispatch` (follow-up)

**Date:** 2026-08-16 · **Commit:** `4bf19b04` · **Branch:** `4220-quick-task` (in sync with `main`)
**Scope:** 244 `src/*.rs` files — 46,418 production LOC (excluding `#[cfg(test)]` bodies and test dirs) + ~69.6k test LOC · 4,281 tests
**Prior review:** `docs/plans/2026-08-16-codebase-review.md` at `b26e98e2`, 18 commits ago. This review verifies its findings at HEAD and adds what's new.

---

## 1. Executive summary

- **All eight work packages from this morning's review landed, and the verification holds.** Every quick win and every "larger effort" I re-checked is genuinely closed at HEAD — not partially, not with a TODO. Coverage rose 87.99% → **90.32%** (14,655/16,226) and the suite went 4,082 → **4,281 tests**, all green. `cargo clippy --all-targets -- -D warnings` is clean. This is a follow-up on a well-tended codebase; the findings below are the residue plus one gate gap.
- **The single most consequential remaining risk is the Command→effect wiring layer.** `src/runtime/commands.rs` is the least-covered real file at **36.8%** (down from 40% — it grew faster than its tests), sits in the **top-15 churn** list, and routes 63 `Command` variants onto their handlers. A new `command_dispatch` test module now drives **29 of 63 (46%)** through the real dispatcher — but every arm it covers is a DB-effect arm. Every tmux/process-effect arm (`Split`, `MainSession`, `Pr`, `RepoSync`, `System`, `Feed`, and 14 of 22 `TaskCommand` variants) is still reachable only by calling `exec_*` directly, so a mis-wired arm compiles, ships, and passes CI.
- **Two of the five local gate scripts have no CI counterpart, including the one built to fix the last review's observed flake.** CI mirrors only `check-doc-paths.sh`. `check-doc-symbols.sh`, `check-no-test-sleep.sh`, and `test-fetch-reviews.sh` exist *only* in `.githooks/pre-push` — which CLAUDE.md itself says is "silently inert" until each clone runs `git config core.hooksPath .githooks`. The wall-clock rule the last review hardened is guarded by the weakest gate in the repo.
- **Coverage is measured on every push but gated on nothing.** The `coverage` CI job runs `cargo tarpaulin` **twice** (once for XML, once for stdout) and uploads an artifact. No threshold, no ratchet — so 90.32% can slide back without failing anything, which is exactly how `commands.rs` went 40% → 36.8%.
- **Structural health is genuinely strong and worth stating plainly.** Layering is compiler-enforced and *verified zero-violation*: no `crate::tui`/`runtime`/`mcp` reference from `service`/`db`/`models`, and `src/tui` has **zero** references to `crate::db`. 3,038 production functions with a **median length of 10 lines** and p90 of 33. **54** duplicated 8-line blocks, all intra-file, **zero cross-file duplication**. **30** panic-capable sites in all of production, every one annotated. No sync lock held across an `.await` anywhere.

---

## 2. Verification of the prior review

### Closed — confirmed at HEAD

| Prior | Finding | Evidence at `4bf19b04` |
|---|---|---|
| Q1 / S9 | 3 dead functions | `display_label`, `split_pinned_task_id`, `list_state_index` — **0 hits each** |
| Q2 / D1 | 7 of 8 `file:NN` citations rotted | **Zero** `file:NN` citations remain in CLAUDE.md; all are `path::symbol`, under the strict checker |
| Q4 / S2 | Telemetry writes discarded silently | `service::usage::record_usage_event_logged` wraps them with a `warn!` on `Err`; 2 tests pin both branches |
| Q5 / S6 | Centred-rect copy-pasted 6× | `src/tui/ui/shared.rs::centered_rect` exists with 4 dedicated tests; callers migrated |
| Q6 / S11 | 33 raw `-32602`/`-32603` literals | Down to `INVALID_PARAMS`/`INTERNAL_ERROR` consts; remaining 8 hits are the definitions, their assertions, and prose |
| Q8 / S10 | Vestigial `dispatch list` / `dispatch update` | Subcommands removed from `src/main.rs` |
| M1 | Wall-clock gate blind to `.elapsed()` | `scripts/check-no-test-sleep.sh` now rejects `.elapsed()` in test code; the 3 offending tests are gone — every remaining `.elapsed()` is production |
| M4 / S4 | MCP args used raw `i64` | Args structs take `TaskId`/`EpicId`; only `src/mcp/trajectory.rs` keeps a bare `i64` |
| M5 / S3 | Stringly-typed CLI boundary | `clap::ValueEnum` in use; the one remaining `String` is a documented deliberate exception |
| M7 / D5 | `docs/plans/` 104 unchecked entries | 12 top-level + 99 archived under `docs/plans/archive/` |
| L1 | Dispatch orchestration triplicated | `TaskService::dispatch(DispatchRequest) -> DispatchOutcome` (`src/service/tasks/dispatch.rs`); both MCP entry points call it. The runtime keeps its own persist/release, which is now *documented* rather than accidental |
| L3 / A5 | `Command` migration half-done | All 5 stragglers gone from `src/tui/types.rs`; 63 variants across 15 domain enums |
| L4 / A3 | `service` → `dispatch` inversion | Zero `crate::dispatch::` references from `src/service` production code |
| S7 | Migration `let _ =` idiom ambiguity | `docs/how-to.md` now names them "frozen history" and prescribes the guard form for new migrations |
| D5 | `module-map.md` missing `ui/budget.rs`, stale `ui/shared.rs` | Both rows present and correct |

Both doc checkers pass: `check-doc-paths: all references resolve`, `check-doc-symbols: all symbol references resolve`.

### Still open

| Prior | Finding | State at HEAD |
|---|---|---|
| L5 / A4 | `ProcessRunner` bypassed | **9 raw spawns remain**, incl. `src/feed/exec.rs:95` (production polling hot path — unreachable from `MockProcessRunner`), `src/main.rs:549`, `src/feed/cycle.rs:329`, `src/setup/plugins.rs:408,426`, `src/setup/hooks.rs:225,312,787,898` |
| L6 | `InputMode` at 36 variants | Still **36**; matched in 51 places in `src/tui/input.rs` alone, 35 in `status_bar.rs`, 19 in `kanban/mod.rs` |
| L7 | TUI tests set state rather than drive it | **1,081** direct `app.x.y = …` assignments vs **679** `handle_key` drives |
| M3 | Wide render signatures | 7-param `src/tui/ui/input_form.rs::repo_picker_lines`; `render_scroll_indicators` (6), `render_task_prompt` (6). No render-context struct |
| M8 | Parallel-slice API | `upsert_feed_tasks_inner` still takes `items` / `repo_paths` / `base_branches` as three slices with a runtime `bail!` on length mismatch |
| M9 | `runtime/tests.rs` monolith | **6,430 lines**, one nested `mod`; `make_app()` at `:3400` still shadows `src/tui/tests/helpers.rs:133`. 12 `make_app*` variants repo-wide |
| S11 | Hand-rolled tick schedulers | 3 remain (`ticks_since_main_session_poll`, `ticks_since_budget_poll`, `ticks_since_last_refresh`), all in `src/tui/update/agent.rs:434-461` |

---

## 3. Architecture & patterns

**Pattern:** layered + Elm (Message→Command), applied consistently. Unchanged from the prior review and still verified clean:

```
src/models (pure domain — zero outbound layer references)
  → src/db      (*Store traits; one writer Connection + read pool)
    → src/service (TaskService/EpicService/…, ServiceError)
      → src/mcp   (Axum JSON-RPC transport)
      → src/tui + src/runtime (App::update → Vec<Command> → execute_commands)
src/dispatch, src/tmux, src/git, src/repo_sync = shell-out adapters behind ProcessRunner
```

**Verified this session (not assumed):**

- `src/models` → zero references to any other layer.
- `src/db`, `src/service` → zero references to `tui`/`runtime`/`mcp`.
- **`src/tui` → zero references to `crate::db`.** The TUI cannot reach the database even by accident; it touches `crate::service` 9 times, `crate::dispatch` twice, `crate::editor` once, and `crate::tmux`/`git`/`feed`/`notify` never.
- The one-way street is `src/runtime` (169 refs to `crate::tui`, 41 to `service`, 27 to `db`) — it is the composition root, and the fan-in is where it belongs.
- **No sync lock held across an `.await`** anywhere in production. Every production `Mutex`/`RwLock` acquisition handles poisoning explicitly (`unwrap_or_else(|e| e.into_inner())` or a `match`); the bare `.lock().unwrap()` sites are all in test-support code (`MockProcessRunner`, `test_log`).

**Dependency injection** remains textbook: `TaskService::new(db, runner)` takes the runner as a *required* argument, with `new_with_real_runner` as the explicitly-named CLI escape hatch. `Clock` is an optional builder because `SystemClock` costs determinism, not side effects — a distinction the code documents.

### The one structural gap: the wiring layer is not exercised

`commands::dispatch` (`src/runtime/commands.rs`) is the sole `Command` → side-effect entry point. `src/runtime/tests.rs` holds 202 tests, but the overwhelming majority call `rt.exec_*` **directly**, bypassing the dispatcher — 15 calls to `exec_insert_task`, 13 to `exec_trigger_epic_feed`, and so on. A new `mod command_dispatch` (added since the prior review, and well designed — it asserts observable effects, never match shape) drives commands through the real dispatcher, but only **29 of 63** variants:

| Covered through the dispatcher | Not covered |
|---|---|
| `Settings` (4/4), `Todo` (5/6), `Epic` (5/7), `RepoFilter` (3/3), `Editor` (2/2), `Task` (8/22) | **`Budget`, `Feed`, `Learning`, `MainSession`, `Pr`, `RepoSync`, `Split`, `System`, `Usage` — 0 of 18 combined**, plus 14 of 22 `TaskCommand` |

The split is not random: everything covered has a **DB-observable** effect; everything uncovered has a **tmux/process/notification** effect. That is the half where a mis-wired arm is invisible — and `src/runtime/commands.rs` is simultaneously the least-covered real file (36.8%) and in the top-15 most-churned. Coverage shows the entire top-level `match` at `commands.rs:15-70` unexecuted.

---

## 4. Test coverage

### Measured: **90.32%** (14,655 / 16,226 lines) — up from 87.99%

`cargo tarpaulin --engine llvm` this session. Suite: **4,281 tests, all green**, unsandboxed.

> **Environment note:** under Claude Code's sandbox, `tests/tmux_editor_pane.rs` fails all 9 tests with `error connecting to /tmp/tmux-1000/… (Operation not permitted)` — the sandbox blocks the tmux socket. `tmux` *is* on `PATH`, so the documented skip path doesn't trigger; the target fails loudly instead of skipping, and `cargo test` aborts there without running the six later targets. Real result requires an unsandboxed run. Worth a line in CLAUDE.md next to the existing "full suite needs tmux on PATH" note.

**Coverage by module:**

| Module | Coverage | |
|---|---:|---|
| `src/cli` | **59.6%** | 127/213 — `agent_tree.rs` at 50.6% dominates |
| `src/runtime` | **66.2%** | 1,006/1,519 — the weak subsystem |
| `src/setup` | 80.3% | OS-interaction; correctly not chased |
| `src/mcp` | 90.1% | |
| `src/feed` | 92.6% | up sharply — `cycle.rs` now 91.1% (prior review flagged 3 tests/389 lines) |
| `src/models` | 92.7% | |
| `src/tui` | 93.1% | |
| `src/service` | 93.6% | |
| `src/dispatch` | 96.0% | |
| `src/db` | **97.0%** | |

**Worst files (≥40 coverable lines):**

| cov | missed | file | verdict |
|---:|---:|---|---|
| **36.8%** | 172 | `src/runtime/commands.rs` | 🔴 the finding that matters — see §3 |
| **48.3%** | 154 | `src/runtime/mod.rs` | loop assembly + bootstrap; partly intrinsic |
| **50.6%** | 81 | `src/cli/agent_tree.rs` | now the worst real file; its `run()` poll loop is untested |
| 68.7% | 66 | `src/runtime/editor.rs` | |
| 69.7% | 77 | `src/setup/mod.rs` | OS interaction — leave it |
| 75.3% | 38 | `src/tui/update/navigation.rs` | |
| 80.0% | 24 | `src/service/embeddings.rs` | model download — leave it |

`src/main.rs` is now at **89.2%**, up from an unmeasured 0% — the prior review's "unmeasured, and unit-untested where it matters" gap has been genuinely filled.

### Test quality

**Distribution** (4,281 attributes, 1,503 of them `#[tokio::test]`): `tui` 1,604 · `db` 422 · `mcp` 379 · `dispatch` 312 · `service` 278 · `runtime` 238 · `models` 216 · `setup` 145 · `feed` 128, plus ~140 integration tests across 19 `tests/*.rs` targets. Healthy pyramid; TUI-heavy because the TUI is the product.

**Snapshots:** 59 `.snap` files, 59 `assert_snapshot!` sites — a clean 1:1 with no orphans and no stale `.snap.new`.

**Property tests remain near-absent:** `proptest!` appears in 7 files across 46k production LOC, despite the conventions endorsing them. Unchanged from the prior review.

**The behaviour-vs-implementation gap persists (L7).** 1,081 direct `app.<field>.<field> = …` assignments in `src/tui/tests/` against 679 `handle_key` drives. Roughly half the largest suite in the repo tests *the renderer given a state*, not the state machine that reaches it — so a state no key sequence can produce still renders green.

---

## 5. Complexity hotspots

**The distribution is excellent and deserves saying:** 3,038 production functions, **median 10 lines**, p90 **33**, only **32 over 100 lines** (1.1%) and 128 over 50 (4.2%).

**Longest functions** (all re-measured this session):

| Lines | Nest | Branch | Location | Verdict |
|---:|---:|---:|---|---|
| 300 | 5 | 12 | `src/tui/input/normal.rs::handle_key_board_normal` | Data, not logic — 12 branches over 300 lines. See S5 below |
| 190 | 5 | 5 | `src/runtime/commands.rs::dispatch_task` | Intrinsic (one arm per `TaskCommand`) |
| 168 | 6 | 9 | `src/dispatch/worktree.rs::provision_worktree` | Multi-step provisioning |
| 165 | 6 | **18** | `src/mcp/handlers/tasks/wrap_up.rs::handle_exit_session` | See below |
| 159 | 6 | 10 | `src/tui/input.rs::handle_key_activate` | Accidental — the depth-6 cluster |
| 157 | 4 | 5 | `src/runtime/mod.rs::bootstrap` | Intrinsic |
| 147 | 4 | 6 | `src/db/queries/tasks.rs::upsert_feed_tasks_inner` | Documented deliberate trade (see M8) |
| 115 | 4 | **23** | `src/service/epics.rs::update_epic` | **Highest branch count in the repo** |

The prior review's 232-line `render_repo_filter_overlay` and 165-line help overlay are gone — the shared overlay-frame extraction worked.

**`handle_exit_session` (165 lines, 18 branches, depth 6)** is now the densest control flow in the codebase. Much of the length is genuinely load-bearing rationale comments, but the shape is a validation gauntlet: **eight early-return error paths inline inside a single `RwLock` write scope**, then a second phase that spawns teardown and chains the next subtask. The token-consumption block (`:231-283`) wants to be a `consume_exit_token(...) -> Result<(WrapUpAction, Option<String>), JsonRpcResponse>` — the atomicity comment would then describe a function rather than a block.

**Deepest nesting:** depth 7 at `src/mcp/handlers/dispatch.rs::handle_mcp`, `src/mcp/handlers/tasks/crud.rs::handle_list_tasks`, `src/runtime/tasks.rs::spawn_refresh_epic`. The prior review's worst cluster (`input.rs:407-428`) has come down to 6.

**Widest signatures:** 7 params at `src/feed/ingest/grouped.rs::upsert_sub_epic_and_recalc`, `src/runtime/mod.rs::run_loop`, `src/tui/ui/input_form.rs::repo_picker_lines`. The render-context struct (M3) is still unextracted, but the count dropped from 8-param sites to 7.

**Enum width:** `InputMode` remains **36 variants** — still the pressure point. `TaskCommand` at 22 is the widest `Command` sub-enum. `Task` at 28 fields is still the gravity well, but the MCP boundary macro (L2, landed) removed three of its six unenforced restatements.

**Duplication is effectively a non-issue.** 54 duplicated 8-line blocks across all production code, **every one intra-file, zero cross-file**. The clearest single case is `handle_navigate_row_first` / `handle_navigate_row_last` (`src/tui/update/navigation.rs:115-163`) — 25 identical lines differing only in `0` vs `count - 1`.

---

## 6. Code smells

**S-A — The dispatcher's process-effect arms are untested (new, highest impact).** See §3. 34 of 63 `Command` variants never pass through `commands::dispatch` in any test; the uncovered set is exactly the tmux/process half.

**S-B — Two of five gate scripts are CI-invisible (new).** `.githooks/pre-push` runs `cargo fmt` (no `--check`), clippy, `check-doc-paths` + self-test, `check-doc-symbols` + self-test, `check-no-test-sleep` + self-test, `test-fetch-reviews`. CI runs `test`, `clippy`, `coverage`, `fmt --check`, and `check-doc-paths` only. So **`check-doc-symbols.sh`, `check-no-test-sleep.sh` and `test-fetch-reviews.sh` are enforced nowhere except a hook that CLAUDE.md says is inert until each clone opts in**. The wall-clock rule the last review specifically hardened is the least-protected rule in the repo.

**S-C — Coverage is measured, never gated (new).** The `coverage` job invokes `cargo tarpaulin` **twice** (`--out xml` then `--out stdout --skip-clean`) and uploads an artifact. Nothing compares the number to anything, so nothing stops it falling — which is how `commands.rs` slid 40% → 36.8% across 18 commits while total coverage rose.

**S-D — `src/runtime/agents.rs` is a 3-line empty module (new).** Its entire content is `use super::*;` + `impl TuiRuntime {}`, and `src/runtime/mod.rs:344` still declares `mod agents;`. `module-map.md` documents it as vestigial rather than deleting it. The prior S9 dead-code sweep looked for functions and missed it.

**S-E — `docs/module-map.md` has a right-path/wrong-description row (new).** Line 95 lists `src/setup/{config,plugins,hooks}.rs` as "MCP config merging, plugin installation, **git hook installation**". `src/setup/hooks.rs` is **1,026 lines of pure `#[cfg(test)] mod tests`** — its own doc comment says "Hook installation itself is part of `install_plugin_in`". An agent asked to change hook installation opens a test file. Neither checker can see this: `check-doc-paths.sh` confirms the path exists and `check-doc-symbols.sh` resolves identifiers — neither validates that a prose description matches. The same row-family problem: line 20's `src/runtime/{editor,epics,learnings,pr,settings,split,todos}.rs` omits the real `budget.rs` and `repo_sync.rs`.

**S5 (carried) — `handle_key_board_normal` is a 300-line hand-rolled dispatch table.** ~40 arms of `self.dispatch_keyed(Message::X(crate::tui::messages::YMessage::Z), "telemetry_name", &label)`, with the fully-qualified path repeated **40 times** in one file that imports nothing from `messages`. The repo already has the right tool for this six times over (`mcp_tools!`, `patch_struct!`, `define_str_enum!`, `service_api!`, `mcp_args!`, `set_field!`). The win isn't length — it's making the keybinding↔telemetry-name pairing structural instead of per-arm discipline.

**S-F — 18 bespoke `macro_rules!` DSLs is a real onboarding cost (new, low severity).** The generation strategy is the right call and demonstrably works — it's what made L2 cheap. But a contributor now has to learn `patch_struct!`, `mcp_tools!`, `mcp_args!`, `service_api_trait!`/`_delegate!`/`_stub_trait!`/`_stub_bridge!` + four spec macros, `define_id_newtype!`, `define_str_enum!`, and `set_field!` before touching a boundary. `docs/conventions.md` is now **70KB** — the largest doc in the repo — which is where that cost is being paid.

**Not smells (verified good):** zero cross-file duplication · 30 panic-capable production sites, all annotated · zero `#[allow(dead_code)]` outside two macro-internal sites · every production lock handles poisoning · no lock held across `.await` · `LATEST_SCHEMA_VERSION` derived from the `MIGRATIONS` array rather than hand-maintained across 88 migrations.

---

## 7. Magic wand: top 3 changes

### 🥇 1. Finish `mod command_dispatch` — the 34 uncovered `Command` arms
**Kills:** S-A, and most of `runtime/commands.rs`'s 36.8%.

The module, the helpers (`dispatch_one`, `drain`, `seed`) and the "assert observable effects, never match shape" discipline **already exist and are good**. This is extending a proven pattern, not designing one. The uncovered arms are process-effect arms, so the assertions are `MockProcessRunner` call-sequence assertions — which the repo is already excellent at (`DispatchScript`, `sync_repo_behind_only_merges_and_does_not_push`).

**Impact on bugs:** this is the only layer in the codebase where a wrong wire compiles, ships, and passes CI. Every other seam is either type-checked or exercised. **Impact on productivity:** `commands.rs` is top-15 churn, so it's paid back continuously.

### 🥈 2. Mirror the three orphan gate scripts into CI, and ratchet coverage
**Kills:** S-B + S-C.

Add `check-doc-symbols.sh`, `check-no-test-sleep.sh`, `test-fetch-reviews.sh` (and their self-tests) to the existing `doc-paths` job — it's a handful of YAML lines against jobs that already exist. Then collapse the double `tarpaulin` invocation into one (`--out Xml --out Stdout`, halving that job's cost) and add `--fail-under 88`.

**Impact:** a rule enforced only by an opt-in hook is a rule with a hole in it, and the last review's own headline fix sits behind that hole. The ratchet turns a measurement into a guarantee — with the current 90.32% giving ~2 points of slack, this fails only on a genuine regression.

### 🥉 3. Collapse the keybinding table (S5) and the wide-signature render helpers (M3)
**Kills:** S5, M3, and most of the remaining >100-line functions.

A `keymap!` macro over `(KeyCode, Message, telemetry_name)` turns `handle_key_board_normal` from 300 lines of repeated ceremony into a table where the keybinding/telemetry pairing is structural — and it makes the "remapping a binding touches ~6 surfaces" learning (KB #88, ↑18) materially cheaper. Pairing it with a render-context struct for `src/tui/ui/` finishes the overlay-consolidation work already begun in `8709900b`.

*(Runner-up, ~5 minutes: delete `src/runtime/agents.rs` and its `mod agents;` — S-D.)*

---

## 8. CLAUDE.md & docs

The prior review's doc work landed well. CLAUDE.md is now **15.7KB** with **zero** `file:NN` citations, the `src/cli/` listing is correct, the verify command is stated, the `cargo fmt`-rewrites-your-tree trap is called out, the `docs/plans/` commit policy is resolved, and the sandbox `unshare` error is documented with the `/tmp` writability trap. Both checkers pass.

### Remaining, in priority order

**D-A — `docs/module-map.md:95` describes `src/setup/hooks.rs` as doing git-hook installation.** It is a 1,026-line test-only module. Fix the row; consider whether `hooks.rs` should be named `hooks_tests.rs` so the filename doesn't mislead independently of the map. Also line 20 omits `src/runtime/budget.rs` and `repo_sync.rs`.

**D-B — Add the sandbox tmux-socket failure to CLAUDE.md's testing section.** It already warns that missing tmux makes `tmux_*` targets *skip*; the sandboxed case is worse — they **fail**, aborting the run before six later targets execute, with an error naming neither the sandbox nor tmux. One sentence next to the existing note.

**D-C — `docs/conventions.md` is 70KB and growing.** It's the correct destination for everything the last review moved out of CLAUDE.md, but it has become the file nobody reads end-to-end. Worth a table of contents at minimum, or a split along the seams it already has (patch/DB conventions · TUI conventions · testing conventions).

**D-D — Property testing is endorsed but essentially unused** (7 files). Either state where it's expected (`src/models` predicates, `text_caret` mechanics, `fair_truncate_segments` are natural fits) or stop endorsing it.

---

## 9. Prioritized action items

### Quick wins (< 1 hour each)

| # | Action | Why |
|---|---|---|
| Q1 | Delete `src/runtime/agents.rs` + its `mod agents;` (S-D) | 3 lines of nothing, still compiled and documented |
| Q2 | Add `check-doc-symbols`, `check-no-test-sleep`, `test-fetch-reviews` (+ self-tests) to the CI `doc-paths` job (S-B) | 3 of 5 gates currently enforced only by an opt-in hook |
| Q3 | Collapse the double `tarpaulin` run into `--out Xml --out Stdout` (S-C) | Halves the coverage job's CI time |
| Q4 | Add `--fail-under 88` to the coverage job (S-C) | Turns a measurement into a ratchet; 2.3pt of slack today |
| Q5 | Fix `module-map.md` lines 20 and 95 (D-A) | Line 95 sends readers into a test file |
| Q6 | Add the sandbox tmux-socket note to CLAUDE.md (D-B) | Cost me an aborted suite run this session |
| Q7 | Extract the shared body of `handle_navigate_row_first`/`_last` | 25 duplicated lines, the clearest dup in the repo |

### Medium (half-day to a day)

| # | Action | Why |
|---|---|---|
| M1 | **Extend `mod command_dispatch` to the 34 uncovered arms (Magic Wand #1)** | The only layer where a wrong wire ships silently |
| M2 | Extract `consume_exit_token` from `handle_exit_session` | 18 branches / depth 6 / a `RwLock` scope with 8 early returns — the densest control flow left |
| M3 | Render-context struct for `src/tui/ui/` (carried M3) | 3 remaining 6-7 param render helpers |
| M4 | Struct-ify `upsert_feed_tasks_inner`'s 3 parallel slices (carried M8) | Replaces a runtime `bail!` with a type |
| M5 | Split `src/runtime/tests.rs` into nested mods; dedupe the 12 `make_app*` (carried M9) | 6,430 lines, one `mod`, one shadowed helper |
| M6 | Route `src/feed/exec.rs:95` through `ProcessRunner` (part of L5) | The one bypass on a production polling hot path — the rest are setup-time and lower value |
| M7 | Add a `docs/conventions.md` table of contents or split it (D-C) | 70KB single file |

### Larger efforts (multi-day, plan first)

| # | Action | Why |
|---|---|---|
| L1 | **`keymap!` table for `handle_key_board_normal` (Magic Wand #3)** | Makes keybinding↔telemetry pairing structural; directly cheapens KB #88 (↑15 ratings) |
| L2 | Decompose `InputMode` (36 variants, exhaustively matched in 3+ files) | Every new modal still costs multi-file edits |
| L3 | Drive TUI tests through `handle_key` rather than field assignment (1,081 sites) | Half the largest suite doesn't test the state machine |
| L4 | Finish routing `src/setup/` spawns through `ProcessRunner` | Lower value than M6 — setup runs once, not in a loop |

### Explicitly *not* recommended

- Chasing coverage on `src/setup/mod.rs` (69.7%) or `src/service/embeddings.rs` (80%) — OS interaction and a model download. CLAUDE.md says so and it's right.
- Splitting `handle_key_board_normal` or `dispatch_task` **by line count**. They're exhaustiveness-checked dispatch tables; L1's value is the telemetry pairing, not the length.
- Touching `src/db/migrations.rs`'s 14 frozen `let _ = conn.execute_batch` sites — `docs/how-to.md` now documents them as frozen and prescribes the guard form for new work. That's the right resolution.
- Adding more `macro_rules!` DSLs without a strong case (S-F). The existing 18 earn their keep; the marginal one costs more onboarding than it saves keystrokes.
