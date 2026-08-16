# Codebase Review — `dispatch`

**Date:** 2026-08-16 · **Commit:** `b26e98e2` · **Branch:** `4207-quick-task`
**Scope:** Rust workspace, 234 `src/*.rs` files, ~68.5k non-test LOC + ~75.8k test LOC, 4,082 tests.

---

## 1. Executive summary

- **The architecture is genuinely sound and the layering is compiler-enforced.** `grep` for `crate::tui|crate::runtime|crate::mcp` across `src/service`, `src/db`, `src/models` returns **zero** hits, and the mutation boundary is a type-level guarantee (`McpState.db` and `TuiRuntime.database` are both `Arc<dyn db::TaskReadStore>`). This is a well-tended codebase, not a rescue job. The findings below are refinements, not alarms.
- **The single biggest correctness risk is triplicated dispatch orchestration.** The claim→prepare→spawn→patch→release sequence — which enforces `DispatchClaimExclusive`, the most safety-critical rule in the system — is hand-written three times in two layers, and the copies' comments have already drifted apart. There is no `TaskServiceApi::dispatch` seam.
- **Change cost is dominated by one entity restated four times.** `Task` (28 fields) → `OwnedTaskPatch` (20) → `UpdateTaskParams` (17) → `UpdateTaskArgs` (15), plus a hand-written JSON schema. Three of those six declaration sites are compiler-enforced; the schema and arg-mapping are not, so a mismatch is a runtime `-32602` rather than a build error.
- **Test volume is excellent (87.99% coverage, 4,082 tests) but it isn't landing where the risk is.** `src/runtime/commands.rs` — which holds one of the three copies of the dispatch flow — is the least-covered real file at **40%**. Meanwhile a wall-clock-threshold assertion (`src/feed/mod.rs:414`) **actually failed** under load during this review, and `scripts/check-no-test-sleep.sh` cannot catch it because it only greps for `sleep(`.
- **CLAUDE.md is loaded into every dispatched agent (~6.5k tokens) and 7 of its 8 `file:NN` citations have rotted** — the exact failure mode the file itself warns against on line 36. ~40% of it is lookup material that belongs behind a pointer.

---

## 2. Architecture & patterns

**Pattern:** layered + Elm/Message→Command, applied consistently.

```
src/models (pure domain)
  → src/db      (*Store traits; one writer Connection + read pool)
    → src/service (TaskService/EpicService, ServiceError)
      → src/mcp   (Axum JSON-RPC transport)
      → src/tui + src/runtime (Elm: App::update → Vec<Command> → execute_commands)
src/dispatch, src/tmux, src/git, src/repo_sync = shell-out adapters behind ProcessRunner
```

**What is working (verified, not assumed):**

- Inward dependency direction is clean — no `crate::tui`/`runtime`/`mcp` references from `service`/`db`/`models`.
- Mutation boundary is compile-enforced via `TaskReadStore` typing (`src/mcp/mod.rs:135`, `src/runtime/mod.rs:296`).
- TUI is IO-free: no `Command::new` anywhere under `src/tui`; the only `std::fs` call is a bare `is_dir()` with an explicit exemption comment (`src/tui/update/forms.rs:92`).
- DI is textbook: one runner construction point (`src/runtime/mod.rs:430`) feeds both transports; `TaskService::new` takes the runner as a **required** arg with `new_with_real_runner` as the explicit CLI escape hatch.

### Leaks, ranked

**A1 — Dispatch orchestration triplicated (highest impact).**
The sequence *claim → `prepare_inputs` → `spawn_blocking(dispatch_agent|research_agent)` → patch `worktree`/`tmux_window` → `release_claim` on failure* is written independently in:
- `src/runtime/tasks.rs::exec_dispatch_agent`
- `src/mcp/handlers/tasks/dispatch.rs::handle_dispatch_task`
- `src/mcp/handlers/tasks/dispatch.rs::auto_dispatch_next`

The `DispatchMode` match is literally duplicated between `dispatch.rs::do_dispatch` and the closure at `src/runtime/tasks.rs:376`. Only `exec_quick_dispatch` reuses anything (`claim_for_dispatch`). **A spec rule enforced by three hand-written copies is a rule that will drift** — and the comments already have.

**A2 — Wrap-up/rebase business logic lives in the MCP transport.**
`src/mcp/handlers/tasks/wrap_up.rs::finish_wrap_up_rebase` calls `dispatch::finish_task` under `spawn_blocking`, sets/clears `SubStatus::Conflict`, and fires three notification kinds — the whole `WrapUpRebase` rule inside a JSON-RPC handler. The same file reaches directly to `crate::tmux::kill_window` (`wrap_up.rs:400`), the only MCP handler touching tmux. Defensible *only* because there is no board-initiated finish path; the moment one is added this becomes copy #2.

**A3 — Service layer depends outward on the dispatch adapter.**
Three inversions: `src/service/tasks/crud.rs:658` (`crate::dispatch::is_wrappable`), `crud.rs:68` (`parse_tmux_window_task_id`), `src/service/grouping.rs:7` (`repo_name_from_path`). All three are pure predicates that belong in `src/models`.

**A4 — `ProcessRunner` bypassed in three subsystems.**
Raw spawns at `src/feed/exec.rs:95` (`tokio::process::Command::new("sh")` — a **production polling hot path**, so the feed exec step is unreachable from `MockProcessRunner`), `src/feed/cycle.rs:329`, `src/setup/hooks.rs:225,312,787,898`, `src/setup/plugins.rs:408,426`, `src/main.rs:617`.

**A5 — God-module `crate::tui`:** 2,125 references, 43.9k lines (32% of crate). `src/tui/types.rs` is the hub — it owns `Message` and `Command`, so every `src/runtime` file imports `crate::tui`. The `Command` migration to `src/tui/commands/` is **half-done**: 15 domain-nested variants coexist with 5 stragglers still inline at `types.rs:170` (`SaveRepoPath`, `SaveBaseBranch`, `PersistSetting`, `PersistStringSetting`, `RecordUsageEvent`).

No true cycles found.

---

## 3. Test coverage & quality

### Measured coverage — **87.99%** (13,437 / 15,271 lines)

`cargo tarpaulin --out Xml` this session, excluding the flaky test in T1 below. Comfortably above the ~85% figure CLAUDE.md cites as a rough snapshot.

**Lowest-coverage files (≥40 tracked lines):**

| cov % | missed | lines | file |
|---:|---:|---:|---|
| **0.0** | 313 | 313 | `src/main.rs` |
| 40.0 | 141 | 235 | `src/runtime/commands.rs` |
| 48.7 | 78 | 152 | `src/cli/agent_tree.rs` |
| 51.1 | 135 | 276 | `src/runtime/mod.rs` |
| 66.1 | 79 | 233 | `src/tui/ui/input_form.rs` |
| 67.9 | 27 | 84 | `src/models/learnings.rs` |
| 69.3 | 71 | 231 | `src/setup/mod.rs` |
| 70.0 | 21 | 70 | `src/service/api.rs` |
| 72.3 | 54 | 195 | `src/runtime/editor.rs` |
| 76.7 | 28 | 120 | `src/service/embeddings.rs` |
| 77.1 | 22 | 96 | `src/mcp/handlers/types.rs` |
| 78.2 | 32 | 147 | `src/tui/update/navigation.rs` |

**Reading these numbers correctly:**

- **`src/main.rs` at 0% is partly an artefact, but not entirely.** `tests/cli.rs` exercises subcommands by spawning the binary as a *subprocess*, which tarpaulin does not instrument — so real behaviour is covered while the counter reads zero. However the gap is genuine for the in-process helpers (`hook_data_dir`, `report_hook_outcome`, `cmd_hook_*`), which have no unit tests at all. **Treat 0% as "unmeasured, and unit-untested where it matters".**
- **`src/runtime/commands.rs` at 40% is the finding that matters most.** This file contains `dispatch_task` (190 lines, `:142`) — one of the three hand-written copies of the dispatch orchestration flagged in A1. The least-covered non-artefact file in the repo is also the one holding a triplicated safety-critical invariant. Together with `src/runtime/mod.rs` (51%) and `src/runtime/editor.rs` (72%), **`src/runtime/` is the weakest-tested subsystem** despite `src/runtime/tests.rs` being the 6,366-line file noted below — volume is not landing where the risk is.
- `src/service/api.rs` at 70% and `src/mcp/handlers/types.rs` at 77% are mostly macro-generated / error-mapping arms; low value to chase.
- `src/tui/ui/input_form.rs` (66%) and `src/setup/mod.rs` (69%) are render and OS-interaction code that CLAUDE.md explicitly says not to chase. Agreed — leave them.

### Distribution (4,216 test attributes)

| Layer | Tests |
|---|---|
| `src/tui/tests/` (26 files, 27.5k LOC) | 1,466 |
| `src/db/tests/` | 421 |
| `src/mcp/handlers/tests/` | 326 |
| `src/dispatch/` | 311 |
| `src/service/` | 280 |
| `src/runtime/tests.rs` | 237 |
| `src/setup/` | 144 |
| `src/feed/` | 129 |
| `src/tmux.rs` (MockProcessRunner) | 129 |
| `tests/*.rs` integration | 145 (61 real-tmux) |

Healthy pyramid; TUI-heavy because the TUI *is* the product. **Property tests are near-absent** — 4 `mod property_tests` blocks / 7 `proptest!` invocations across 68k LOC, despite conventions endorsing them.

### 🔴 Finding T1 — a wall-clock test failed under load, and the gate can't see it

During this review `cargo tarpaulin` aborted with:

```
feed::tests::tick_does_not_block_event_loop
test result: FAILED. 4081 passed; 1 failed
```

It passes in 0.06s unloaded. The cause (`src/feed/mod.rs:409-416`):

```rust
let start = std::time::Instant::now();
runner.tick().await;
assert!(elapsed < Duration::from_millis(500), "tick() blocked for {elapsed:?}");
```

CLAUDE.md forbids exactly this — *"not to 'wait for' … and **not to cross a duration threshold**"* — but `scripts/check-no-test-sleep.sh` only greps for `tokio::time::sleep(` / `std::thread::sleep(`. A threshold assertion is invisible to it. **Two sibling sites have the same shape:** `src/process.rs:859` and `src/feed/exec.rs:437` (both `elapsed() < Duration::from_secs(5)`).

The fix is the one conventions already prescribe: assert the *deterministic signal* (tick returned before the child completed — e.g. observe the `McpEvent` ordering or a `Notify`), not the clock.

### Behaviour vs implementation

Mostly behaviour-driven, with unusually good test naming:

- **Strong:** `sync_repo_behind_only_merges_and_does_not_push`, `sync_repo_never_rewrites_local_base_history` (`src/repo_sync.rs`) — assert *which argv was and was not issued*, which here **is** the behaviour per conventions. `finish_task_dirty_primary_worktree_returns_error_before_pull` asserts ordering as a safety invariant. `wrap_up_success_omits/includes_verify_reminder_when_(un)configured` — paired positive/negative, the pattern that survives feature deletion.
- **Weak — the TUI suite sets state rather than driving it:** 1,073 direct `app.<field>.<field> = …` assignments in `src/tui/tests/*.rs` vs 833 `handle_key` drives. `snapshot_input_title_form` (`src/tui/tests/snapshots.rs:50`) hand-assigns `InputMode::InputTitle` + buffer + draft, rendering a state **no key sequence is proven to reach**; contrast `snapshot_help_overlay` (line 42), which presses `?`. Roughly half the TUI suite tests the renderer given a state, not the state machine.
- **Weakest — skill-copy `contains` tests** (`src/setup/plugins.rs:593,819,948`): assert markdown prose. Deliberate and endorsed, but they test a string and will churn on every copy edit.

### Snapshots

46 in `src/tui/tests/snapshots/`, 9 in `src/dispatch/snapshots/`. **No stale `.snap.new` files** — cleanup discipline is being followed. The 9 prompt snapshots are high-value (a prompt is a contract with an agent). Several TUI card snapshots are near-duplicates differing by one badge (`snapshot_card_running_with_subagents` / `_with_shells` / `_with_subagents_and_shells` / `snapshot_card_stale_shell`) — classic blind-re-accept candidates.

### Genuine gaps

- **`src/main.rs` — 996 lines, 0 inline tests.** `tests/cli.rs` covers subcommands as a black box, but the hook-ingestion path Claude Code actually calls has no direct coverage: `hook_data_dir:359`, `report_hook_outcome:387`, `cmd_hook_*:398-534`. Untested failure modes: unwritable data dir, malformed hook JSON, partial writes.
- **`src/mcp/mod.rs` — 307 lines, 0 tests.** Handlers are tested; the server assembly/routing is not.
- **`src/feed/cycle.rs` — 3 tests for 389 lines**, thinnest in `feed/`. Poll-loop scheduling and failure-backoff effectively untested vs `exec.rs` (20) / `routing.rs` (12).

*(False positive: `src/db/migrations.rs` has 0 inline tests but is covered from `src/db/tests/migrations.rs`, which is the documented convention.)*

### Test smells

- **`src/runtime/tests.rs`: 6,366 lines, 237 tests, flat module** with `// ---` comment banners instead of nested `mod` blocks — testing a `src/runtime/` whose largest file is 1,175 lines.
- **8 distinct `make_app*` helpers** across `helpers.rs`, `main_session.rs`, `todos.rs`, `task_detail.rs`, `epics.rs`, `scenarios.rs`, `runtime/tests.rs`, `tests/lifecycle.rs`; ~15 `make_task`/`seed_task` variants. `runtime/tests.rs:3349` defines a second `make_app()` shadowing `src/tui/tests/helpers.rs:132`.
- Residual clock dependence: ~30 `Utc::now()` uses in `src/db/tests/`; `src/db/tests/epics.rs:472-474` brackets a call with before/after timestamps (low-probability granularity flake).
- **Good:** 90 `DispatchScript` uses vs a single hand-rolled `vec![ok()…]`; zero `env::set_var`/`set_current_dir` in tests.

---

## 4. Complexity hotspots

### Largest non-test files

| Non-test LOC | Total | File |
|---|---|---|
| 1113 | 1113 | `src/db/queries/tasks.rs` |
| 1099 | 2229 | `src/models/tasks.rs` |
| 1085 | 1085 | `src/service/tasks/crud.rs` |
| 1032 | 2701 | `src/tmux.rs` |
| 1016 | 1258 | `src/tui/types.rs` |
| 996 | 996 | `src/main.rs` |
| 935 | 935 | `src/tui/input.rs` |
| 835 | 835 | `src/runtime/tasks.rs` |
| 747 | 747 | `src/tui/input/normal.rs` |
| 723 | 723 | `src/mcp/handlers/dispatch.rs` |

`db/queries/tasks.rs` + `service/tasks/crud.rs` = **2,200 lines of task CRUD with zero inline tests**, the widest fan-in surface in the codebase.

### Longest functions

| Lines | Location | Verdict |
|---|---|---|
| 300 | `src/tui/input/normal.rs:212` `handle_key_board_normal` | intrinsic-ish (dispatch table) but see S5 |
| 232 | `src/tui/ui/kanban/popups/repo_filter.rs:14` | **accidental** |
| 190 | `src/runtime/commands.rs:142` `dispatch_task` | intrinsic (one arm per `TaskCommand`) |
| 165 | `src/tui/ui/kanban/popups/help.rs:14` | accidental (see S6) |
| 165 | `src/mcp/handlers/tasks/wrap_up.rs:262` `handle_exit_session` | multi-step txn |
| 159 | `src/tui/input.rs:315` `handle_key_activate` | **accidental** |
| 147 | `src/db/queries/tasks.rs:965` `upsert_feed_tasks_inner` | documented deliberate trade |

`render_repo_filter_overlay` (232) is straight-line layout arithmetic with four `InputMode` variants interleaved into one body; the sibling popup renderers repeat the shape. That's **a missing shared overlay-frame abstraction**, not four irreducible problems.

*(False positive: `src/service/api.rs:168 update_task` scores 181 lines but is a trait signature block inside the `service_api!` macro.)*

### Deepest nesting (74 sites at depth ≥5)

Worst cluster: **depth 7** at `src/tui/input.rs:407,422,428` inside `handle_key_activate` — `fn → match mode → match status → if/else-if chain → closure`, each branch building a `Vec<Command>` inline. The branch conditions (`is_problematic`, `has_worktree`, `dispatch_may_be_in_flight`) are a 3-bit state that wants a named enum resolved *before* the match. Also depth 7 at `src/runtime/tasks.rs:622`.

### Wide signatures

8 params: `input_form.rs:31`, `input_form.rs:332`, `setup/mod.rs:307 run_setup_in`, `feed/ingest/grouped.rs:25`. 7 params: 12 more sites. **7 of the top 16 are in `src/tui/ui/`**, all threading `(lines, buffer, cursor, height_offset, area_height, hint, …)` — an unnamed render-context struct waiting to be extracted.

### Enums & matches

`TaskMessage` 41 variants, **`InputMode` 36**, `EpicMessage` 25, `InputMessage` 24, `TaskCommand` 22, `Message` 21. Largest matches: 42 arms (`input/normal.rs:227`), 37 (`messages/task.rs:129`), 34 (`status_bar.rs:178`).

The 42-arm keybinding table is the textbook intrinsic case. **The real pressure point is `InputMode` at 36 variants**, matched exhaustively in at least two places (`input.rs:139`, `status_bar.rs:178`) — every new modal costs edits in several files. That is accidental coupling: modal state expressed as one flat enum instead of per-surface state.

### Struct width

`Task` **28** fields → `OwnedTaskPatch` 20 → `UpdateTaskParams` 17 → `UpdateTaskArgs` 15. The same entity restated four times across layers, plus a hand-written JSON schema. This is the codebase's central gravity well and the largest single lever on change cost. (Partly mitigated: the `OwnedTaskPatch` parity hazard is compiler-enforced via exhaustive destructuring.)

---

## 5. Code smells

**S1 — The task field set is declared six times; only three are enforced.**
Adding a field touches: hand-written JSON schema (`src/mcp/handlers/dispatch.rs:121+`), `UpdateTaskArgs` (`tasks/mod.rs:38`), the arg→param mapping (`tasks/crud.rs:42-101` — **fifteen consecutive** `if let Some(x) = parsed.x { params = params.x(x) }` blocks), `UpdateTaskParams`, `updated_field_names`, `TaskPatch`, `OwnedTaskPatch`. The first three have **no** enforcement — `deny_unknown_fields` turns a schema/struct mismatch into a runtime `-32602`. `src/service/api.rs` already solves this exact problem with a macro; the MCP boundary should follow.

**S2 — Telemetry DB writes discarded silently.**
`src/runtime/commands.rs:77`, `src/mcp/handlers/dispatch.rs:675,693`: `tokio::spawn(async move { let _ = db.record_usage_event(...).await; })` — no `?`, no `warn!`. Conventions explicitly forbid discarding a DB write's `Result`, and the keybinding-telemetry design states that *absence of a count must mean "unused"* because pruning passes read it. **A dropped write makes a used binding look unused.**

**S3 — CLI boundary is stringly-typed, violating the repo's own "Border parsing" rule.**
`Commands::Update { status: String }` (`src/main.rs:38`), `HookSubagent { action: String }` (`:101`), `HookShell { action: String }` (`:124`), re-parsed by hand at `:446`, `:518`, with `if action == "start"` at `:452` matching the same literal twice in one function. Clap's `ValueEnum` would parse at the boundary *and* populate `--help`. Compounding: `cmd_update` takes **both** `sub_status: Option<String>` and a redundant `needs_input: bool`, resolved by an if/else-if precedence chain at `:307-316` — two spellings of one field.

**S4 — Primitive obsession at the MCP boundary despite existing newtypes.**
`TaskId`/`EpicId` exist in `src/models/ids.rs` and derive `Deserialize`, yet every args struct takes raw `i64` (~20 sites across `tasks/mod.rs`, `epics.rs`, `learnings.rs`, `wrap_up.rs`) then wraps at use. `watcher_task_id` / `target_task_id` as two adjacent bare `i64` (`tasks/mod.rs:152-154`) is the case where **a swap is silent**. Inconsistent within the same struct: `status`/`tag`/`sub_status` are typed while `url_type` is `Option<String>` re-parsed at `crud.rs:77`. Separately ~104 non-test sites thread bare `String` for `repo_path`/`base_branch`/`worktree`/`tmux_window`; the tmux window name already has a de-facto construct site (`prompts.rs:63`) and parse site (`:77`).

**S5 — `handle_key_board_normal` is a 299-line hand-rolled dispatch table** (`src/tui/input/normal.rs:212-511`). ~40 arms of identical shape, each repeating the fully-qualified `crate::tui::messages::TaskMessage::` path. This is **data, not logic** — a const table or small macro would collapse it and make the keybinding/telemetry pairing structural rather than per-arm discipline.

**S6 — Popup layout arithmetic duplicated with hand-counted magic offsets.**
Centred-rect computation copy-pasted six times (`popups/error.rs:21`, `help.rs:21`, `repo_filter.rs:46`, `ui/todos.rs:32`, `popups/reparent_epic.rs:85`) — **there is no `centered_rect` helper anywhere**. Scroll-window formula duplicated verbatim at `input_form.rs:85` and `repo_filter.rs:61`. Worst: `repo_filter.rs` maintains the same layout budget twice from opposite directions — `+7: blank(1) + toggle_row(1) + …` at `:42` and `non_repo_lines = preset_lines + input_line + 5` at `:55`. **Nothing checks that those two constants agree.**

**S7 — Migration error handling is inconsistent.** 16 sites use `let _ = conn.execute_batch("ALTER TABLE …")` (swallowing *every* error including I/O) vs 41 using the `column_exists` guard + `.context(…)?`. The `let _` form is concentrated in frozen early versions, but the two idioms side by side invite new migrations to copy the wrong one — `docs/how-to.md:122` documents only the guard form.

**S8 — Parallel-slice API where a struct belongs.** `upsert_feed_tasks_inner(epic_id, items: &[FeedItem], repo_paths: &[String], base_branches: &[String], delete_absent: bool)` (`db/queries/tasks.rs:965`) enforces a three-slice length invariant with a runtime `bail!` at `:975`; the comment concedes a mismatch would silently truncate. `Vec<(FeedItem, String, String)>` makes the check unnecessary. Same shape at `feed/ingest/grouped.rs:25`.

**S9 — Confirmed dead code** (independently verified — each grep returns exactly one hit, its own definition):
- `src/models/learnings.rs:41` `pub fn display_label`
- `src/tui/mod.rs:749` `pub fn split_pinned_task_id`
- `src/tui/types.rs:675` `pub fn list_state_index`

No stray `#[allow(dead_code)]` in production code — that convention is honoured.

**S10 — Vestigial CLI surface.** `dispatch list` and `dispatch update` appear only in `src/main.rs` and `docs/reference.md`. Verified: the installed hooks now forward to the dedicated `hook-*` subcommands (`src/setup/hooks.rs:106,127`), not `dispatch update`; agents use the MCP equivalents. These are leftovers from the pre-`hook-*` design.

**S11 — Lower impact.** 33 non-test occurrences of literal `-32602`/`-32603` despite `service_err_to_response` centralising the mapping (`handlers/types.rs:348`). `TuiRuntime` as service locator — 15 fields incl. two DB handles and four service handles, handed whole to every command handler, with `run_loop:714` taking 7 more params. Four hand-rolled schedulers on `App` (`spinner_tick`, `ticks_since_main_session_poll`, `ticks_since_budget_poll`, `ticks_since_last_refresh`, `src/tui/mod.rs:275-305`).

---

## 6. Magic wand: top 3 changes

### 🥇 1. Extract a single `TaskServiceApi::dispatch` seam
**Kills:** A1 (triplicated orchestration) + A2 (wrap-up logic in transport) + A4's testability gap.

One service method owning claim→prepare→spawn→patch→release. `exec_dispatch_agent`, `handle_dispatch_task`, and `auto_dispatch_next` all become thin callers. **Impact on bugs:** `DispatchClaimExclusive` is the system's most safety-critical invariant and is currently enforced by three copies that have *already* drifted in their comments — this converts a discipline problem into a structural one. **Impact on productivity:** dispatch is the highest-churn behaviour in the repo (`src/runtime/*` and `src/mcp/handlers/tasks/dispatch.rs` are both in the top-12 churn list), so every future change stops costing three edits.

### 🥈 2. Generate the MCP task-field boundary from one declaration
**Kills:** S1 (six declaration sites, three unenforced) + S4 (raw `i64` ids) + most of the `Task`-restated-four-times tax.

A macro emitting JSON schema + `UpdateTaskArgs` + the 15-block arg→param mapping from a single field list, mirroring what `service_api!` already does one layer down. **Impact:** adding a task field is currently the single most ripple-prone edit in the codebase, and half its surfaces fail at *runtime* rather than compile time. This is also the cheapest large win — the pattern is already proven in-repo.

### 🥉 3. Close the wall-clock hole and fix the three offending tests
**Kills:** T1 — a real, observed flake.

Extend `scripts/check-no-test-sleep.sh` to reject `Instant::elapsed()` comparisons in test code (same `allow-test-sleep:` escape hatch), then convert `src/feed/mod.rs:414`, `src/process.rs:859`, `src/feed/exec.rs:437` to await deterministic signals. **Impact on bugs:** flaky tests under load are how green suites stop meaning anything — and this one *already* aborted a coverage run during this review. Small, bounded, and it makes an existing documented rule actually enforceable.

*(Runner-up, cheap: a `centered_rect` helper — S6 — deletes six copies and one unchecked dual-maintained constant.)*

---

## 7. CLAUDE.md & docs

### 🔴 D1 — 7 of 8 `file:NN` citations have rotted

Independently verified against source:

| CLAUDE.md claims | Actual |
|---|---|
| `TaskTag` at `src/models/tasks.rs:438` | **558** |
| `DispatchMode::for_task()` at `:420` | **540** |
| `TaskTag::is_review()` at `:465` | **595** |
| `build_prompt` at `src/dispatch/prompts.rs:264` | **336** |
| `src/dispatch/agents.rs:50` (PR-head branch) | logic is at **253-280**; line 50 is an unrelated doc comment |
| `patch_struct!` at `src/db/mod.rs:30` | **38** |
| `mcp_tools!` at `src/mcp/handlers/dispatch.rs:39` | 38 (harmless) |
| `CallerIdentity::from_headers` at `src/mcp/identity.rs:21` | ✅ correct |

This is precisely the failure CLAUDE.md warns about on its own line 36 — and `check-doc-paths.sh` only *bounds-checks* line numbers, so the hook will never catch it. **Fix: rewrite all eight as `path::symbol`**, which moves them under `check-doc-symbols.sh`'s strict resolution check.

### D2 — Content errors

- **`src/cli/` listing is stale** (CLAUDE.md:240): says "(`agent_tree`, `caller_headers`)"; the directory also contains **`statusline.rs`**, a real subcommand documented in `docs/reference.md:127` and `docs/module-map.md:16`. CLAUDE.md is the only one of the three that's wrong. *(Verified by `ls src/cli`.)*
- **The test-target list (lines 41-55) is partial but presented as complete.** `tests/` also holds `active_health.rs`, `caller_identity.rs`, `dispatch_status_lifecycle.rs`, `feed_sync.rs`, `githooks.rs`, `managed_feeds.rs`, `task_watchers.rs`, `tmux_send_message_pane_state.rs`, `trajectory.rs`, `verify_command.rs`. Say "selected targets" or drop the enumeration.

### D3 — Missing context every agent pays for

- **`cargo fmt` in the pre-push hook has no `--check`** — pushing rewrites your working tree. The step is listed; the consequence is not.
- **This repo's own verify command is never stated.** A whole section explains the verify-command *mechanism* but never says `dispatch`'s is `cargo test`. One line saves every agent a `get_task` round-trip.
- **`plugin/skills/` is unexplained** — appears once, inside a table cell, with no statement that it is the source of truth for agent-facing skill copy or how it relates to `src/setup/plugins.rs::skill_body`.
- **`docs/plans/` commit policy is unstated** — the doc checker excludes it, some global rules forbid committing it, and recent repo history commits it. CLAUDE.md points at the directory without resolving which. Per its own "ambiguity is a stop condition" rule, this should be written down.
- **The sandbox denies `unshare`**, so plain `ls`/`wc` can fail with `apply-seccomp: unshare(CLONE_NEWUSER): Invalid argument` under parallel load — hit twice during this review, undocumented and immediately confusing.
- **`cargo insta` must be installed** for the documented `cargo insta review` line; nothing says so.
- **tmux must already be running** before `cargo run -- tui` (stated only in a code comment, `reference.md:116`).

### D4 — Size & signal: 246 lines / ~26KB / **~6.5k tokens on every dispatch**, ~40% lookup material

Move behind pointers, in priority order:

1. **"Running tests" table + snapshot section + "Where new tests go"** (lines 38-113, ~76 lines, ~2.5k tokens) → new `docs/testing.md`. Only two sentences earn every-context placement: *"the full suite needs tmux on PATH"* and *"don't pipe `cargo test` into `tail`"*.
2. **"Tag System"** (195-204) → `docs/conventions.md`. Relevant to ~1 task in 20.
3. **The 15-item Allium spec list** (163-177) → "specs live in `docs/specs/`; each filename names its domain." The filenames are self-describing; the glosses are 15 lines of restatement.
4. **"Running & Debugging Locally"** (115-126) → `docs/reference.md`, which already has a Configuration table.
5. **"External Dependencies"** (128-141) — keep the bubblewrap/socat sentence (a real silent-degradation trap), move the per-binary breakdown.

**Keep as-is:** the `main`-moves-under-you paragraph, First-time setup, Working With the User, TDD, Mutation boundary, the doc index. Those change agent *behaviour* rather than answer lookups. **Target ≈120 lines.**

### D5 — docs/ health

- **`docs/module-map.md:31` is stale:** the `src/tui/ui/shared.rs` row omits `staleness_color` and `feed_role_label` (both cited by CLAUDE.md as living there), and **`src/tui/ui/budget.rs` has no row at all**.
- **`docs/reference.md` "CLI Usage" lists 7 invocations; `src/main.rs` defines 19 subcommands.** Missing: `repo set-verify`/`clear-verify`/`list` (CLAUDE.md:145 cites `repo set-verify`, so a reader following the pointer won't find it), the five `hook-*` commands, `agent-tree`, `caller-headers`, `pr-gate`, `uninstall`, `prune-repo-paths`, `toggle-agent-tree-pane`.
- **`docs/architecture.md` is 51 lines, three of which are 300-word paragraphs** (bullets 6, 27, 28). Promote each to an `##` section — structure, not cuts.
- **`docs/plans/` is noise:** 104 top-level entries / ~133 files / 1.3MB, in two incompatible naming schemes (89 issue-numbered like `157-smaller-prompts.md`, 17 date-prefixed, plus five `review-<date>/` dirs). `docs/superpowers/` adds ~57 files / 1.3MB. Neither is checked by the doc hooks, so both rot freely.

---

## 8. Prioritized action items

### Quick wins (< 1 hour each)

| # | Action | Why |
|---|---|---|
| Q1 | Delete the 3 dead functions (S9) | Verified zero callers |
| Q2 | Rewrite CLAUDE.md's 8 `file:NN` → `path::symbol` (D1) | Puts them under the strict checker; stops the rot recurring |
| Q3 | Fix `src/cli/` listing + test-target list in CLAUDE.md (D2) | Actively wrong today |
| Q4 | Add `warn!` on `Err` to the 3 telemetry spawns (S2) | Silent drops corrupt the "unused binding" signal |
| Q5 | Add `centered_rect` helper, replace 6 copies (S6) | Deletes a dual-maintained unchecked constant pair |
| Q6 | Name the `-32602`/`-32603` literals as constants (S11) | 33 sites |
| Q7 | Add the 4 missing facts to CLAUDE.md (D3: verify command, `cargo fmt` rewrites, `cargo insta`, `docs/plans/` policy) | Each costs every agent time today |
| Q8 | Remove `dispatch list`/`dispatch update` (S10) | Verified vestigial; matches standing preference to delete unused CLI paths |

### Medium (half-day to a day)

| # | Action | Why |
|---|---|---|
| M1 | **Close the wall-clock hole + fix 3 tests (Magic Wand #3)** | An observed flake; do this first |
| M2 | Slim CLAUDE.md 246 → ~120 lines, spin out `docs/testing.md` (D4) | ~2.5k tokens saved on *every* dispatch |
| M3 | Extract a render-context struct for `src/tui/ui/` (7 wide signatures) | Also unblocks splitting the 232-line popup renderer |
| M4 | Convert MCP args `i64` → `TaskId`/`EpicId` (S4) | Newtypes already exist and derive `Deserialize`; kills a silent-swap class |
| M5 | Clap `ValueEnum` at the CLI boundary; collapse `sub_status`/`needs_input` (S3) | Repo's own border-parsing rule; improves `--help` |
| M6 | Fix `module-map.md` + `reference.md` CLI gaps; restructure `architecture.md` (D5) | Broken pointers |
| M7 | Archive `docs/plans/` below current milestone; pick one naming scheme (date-prefix) | 104 entries, unchecked, rotting |
| M8 | Struct-ify `upsert_feed_tasks_inner`'s parallel slices (S8) | Removes a runtime invariant check |
| M9 | Split `src/runtime/tests.rs` (6,366 lines) into nested modules; dedupe `make_app*` | 8 competing helpers, one shadowing |

### Larger efforts (multi-day, plan first)

| # | Action | Why |
|---|---|---|
| L1 | **Extract `TaskServiceApi::dispatch` (Magic Wand #1)** | Highest bug-risk reduction in the report — and it consolidates three copies into one testable seam, directly addressing `runtime/commands.rs`'s 40% coverage |
| L2 | **Generate MCP task-field boundary from one declaration (Magic Wand #2)** | Highest change-cost reduction |
| L3 | Finish the `Command` migration — move the 5 stragglers out of `types.rs:170` (A5) | Half-done migrations are worse than either endpoint |
| L4 | Move `is_wrappable` / `parse_tmux_window_task_id` / `repo_name_from_path` to `src/models` (A3) | Removes service→adapter inversion |
| L5 | Route `src/feed/exec.rs` and `src/setup/` spawns through `ProcessRunner` (A4) | Makes the feed hot path mockable |
| L6 | Decompose `InputMode` (36 variants, exhaustively matched in 2+ places) into per-surface state | Every new modal currently costs multi-file edits |
| L7 | Drive TUI tests through `handle_key` rather than field assignment (1,073 sites) | Half the largest suite doesn't test the state machine |

### Explicitly *not* recommended

- Chasing coverage % on `src/tui/ui/` render code or `src/setup/`'s OS-interaction branches — CLAUDE.md already says so and it's right.
- Splitting `handle_key_board_normal` (300 lines) or `dispatch_task` (190) by line count alone — they're exhaustiveness-checked dispatch tables. S5's table/macro idea is worth it for the *telemetry pairing*, not the length.
- Touching `src/db/migrations.rs`'s frozen `let _ =` history (S7) — document the guard form as mandatory for *new* migrations instead.
