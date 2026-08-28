# Codebase Review — dispatch

**Date:** 2026-08-28
**Reviewed at:** `main` @ `6438f780`
**Scope:** whole repository (`src/`, `tests/`, `docs/`, `scripts/`, CI)
**Method:** static measurement (function length / nesting / fan-in), `cargo tarpaulin --engine llvm`, `cargo clippy --all-targets`, full `cargo test`, all four gate scripts, plus targeted reading of the seams named in `CLAUDE.md`.

---

## 1. Executive summary

- **The engineering discipline here is unusually high, and it is measurable.** 91.56% line coverage (15134/16529, llvm engine), zero clippy warnings under `--all-targets`, all four gate scripts green, `cargo fmt --check` clean, 4328 unit tests + ~150 integration tests passing in ~10s of test execution. Nothing in this review is a firefight.
- **Layering is enforced, not aspirational.** Zero `tui → db`, `tui → tmux`, `mcp → tui`, `service → tui`, or `models → db` references in production code. The mutation boundary described in `CLAUDE.md` is real and compiler-enforced.
- **The one genuine architectural debt is `App`: 354 methods on a single type**, spread across 22 `impl App` blocks in 22 files. The files are split; the type is not. Every method can reach every field.
- **The highest-frequency papercut is the `Task` fixture.** `Task` has 29 fields, no `Default`, and 23 sites construct it field-by-field. Adding a field is a 23-file edit, most of it mechanical.
- **`docs/` is a strength, but `CLAUDE.md` has drifted in one load-bearing place** — it states the verify command is `cargo test`, while the command actually stored for the repo is `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`. 18% of the file is sandbox history the file itself marks as moot.

---

## 2. Architecture & patterns

### Pattern

Layered, with an Elm-ish core for the TUI:

```
tui  (App: state + update)  ─┐
                             ├─→ service (TaskService, EpicService, …)  ─→  db  ─→  models
runtime (Command → effect)  ─┘                                                       ↑
mcp  (JSON-RPC handlers)   ────────────────────────────────────────────────────────────┘
feed (FeedRunner)          ──→ db (sanctioned direct-mutation exception)
```

Per-directory production weight:

| Directory | LOC (incl. inline tests) | Files |
|---|---:|---:|
| `src/tui/` | 45,139 | 100 |
| `src/db/` | 20,615 | 24 |
| `src/mcp/` | 17,185 | 28 |
| `src/service/` | 12,789 | 18 |
| `src/runtime/` | 12,576 | 13 |
| `src/dispatch/` | 10,567 | 12 |
| `src/feed/` | 6,475 | 13 |
| `src/models/` | 5,623 | 15 |
| `src/setup/` | 4,374 | 5 |
| `src/cli/` | 1,740 | 4 |

### Is it consistently applied?

Yes — and this is the review's strongest positive finding. Measured cross-module `crate::` references in production regions only:

| Edge | Count | Verdict |
|---|---:|---|
| `runtime → tui` | 147 | Expected — runtime owns `App` |
| `service → models` | 91 | Correct direction |
| `tui → models` | 87 | Correct direction |
| `mcp → service` | 25 | Correct direction |
| `service → db` | 12 | Correct direction |
| `feed → db` | 9 | Documented exception (`FeedRunner`) |
| **`tui → db`** | **0** | Boundary holds |
| **`tui → tmux`** | **0** | Boundary holds |
| **`mcp → tui`** | **0** | Boundary holds |
| **`service → tui`** | **0** | Boundary holds |
| **`models → db` / `models → service`** | **0** | `models` is a leaf |

`models` is a true leaf. `tui` never touches the database or tmux directly. These are the four boundaries that decay first in a codebase this size, and none of them has.

### Dependency injection

Constructor injection over narrow `Arc<dyn Trait>` seams, with the trait narrowed per consumer rather than one god-trait:

- `TaskService::new(db: Arc<dyn db::TaskStore>, runner: Arc<dyn ProcessRunner>)`
- `EpicService::new(db: Arc<dyn db::TaskAndEpicStore>)`
- `TodoService::new(db: Arc<dyn TodoStore>)`
- `LearningService::new(db: Arc<dyn db::TaskStore>, embeddings: Arc<EmbeddingService>)`

Two details worth calling out as good design, not accidents:

1. **`runner` is a required constructor argument.** The real runner is reachable only through the explicitly-named `TaskService::new_with_real_runner`. A test cannot forget to inject the mock, because there is no default.
2. **`clock` is deliberately *not* required.** `SystemClock` only reads the wall clock, so a default costs determinism but never a side effect. The asymmetry is documented in `docs/architecture.md` and is the right call.

### The `service_api!` macro family — power with a cost

`src/service/api.rs` declares each service seam once as a `macro_rules!` "spec" macro that replays its signature list into one of four emitter macros:

| Emitter | Generates |
|---|---|
| `service_api_trait!` | the `#[async_trait]` trait |
| `service_api_delegate!` | production impl, delegating via UFCS |
| `service_api_stub_trait!` | test-only stub trait, all methods `panic!` by default |
| `service_api_stub_bridge!` | `impl <Api> for <MockType>` |

This is genuinely excellent: adding a method to a seam is one edit to one signature list, and a new method cannot break an unrelated mock (mocks implement the stub trait and override only what they exercise).

The cost is mastery. It is two macro layers deep, uses `$crate::`-qualified types because `macro_rules!` resolves type paths at the call site, and it is **not mentioned in `CLAUDE.md`** — which does list `patch_struct!` and `mcp_tools!` as the "workhorse macros" to read before touching. An agent adding a service method will find this pattern by tripping over it. See §6.

---

## 3. Test coverage

### Numbers

| Metric | Value |
|---|---|
| Line coverage | **91.56%** (15134 / 16529, `--engine llvm`) |
| CI floor | 88 (a gate, not a report) |
| Headroom | +3.56 points |
| Unit tests (lib target) | 4328 passing, 1 ignored |
| Integration tests (`tests/`) | ~150 across 20 targets |
| `insta` snapshots | 59 |
| `proptest!` blocks | 7 |
| Test execution | 9.5s (lib) / ~80s wall including a cold compile |

Coverage has **risen** since it was last recorded: `docs/testing.md` cites 90.28% as of 2026-08-16; today's run on the same engine reads 91.56%. The floor is doing its job without being chased.

### Unit vs integration ratio

Skewed hard toward in-crate white-box tests: ~72.8k LOC of test code lives inside `src/` (in files named `tests.rs` / under `tests/` subdirs, plus 86 inline `#[cfg(test)]` modules in production-named files), against 6,349 LOC in `tests/`. By test count, 4328 unit vs ~150 integration — roughly **97% unit**.

This is a defensible choice for a TUI (the `Command` vector *is* the observable output, and it is only reachable in-crate), but it has a consequence: **2,053 direct reads of `app.board.*` / `app.input.*` / `app.select.*` / `app.interaction.*` / `app.layout.*` appear in `src/tui/tests/`.** Tests are coupled to `App`'s internal shape, which is exactly the shape §5 recommends changing. Any `App` split will churn those tests.

### Are tests testing behaviour or implementation?

Mixed, leaning behaviour. A representative test from `src/tui/tests/navigation.rs`:

```rust
#[test]
fn move_task_forward() {
    let mut app = make_app();
    let cmds = app.update(Message::Task(TaskMessage::Move {
        id: TaskId(1), direction: MoveDirection::Forward,
    }));
    assert_eq!(/* task 1 status */, TaskStatus::Running);       // behaviour
    assert!(matches!(cmds[0], Command::Task(TaskCommand::Persist(_)))); // structure
}
```

The state assertion is behavioural. The `cmds[0]` assertion is positional and structural — but in a Message→Command architecture the emitted command genuinely *is* the contract, so this is closer to behaviour than it looks. The positional index (`cmds[0]` rather than a `.iter().any(…)`) is the part that will break on unrelated changes.

### Untested critical paths

Ranked by uncovered lines:

| File | Coverage | Uncovered | Assessment |
|---|---:|---:|---|
| `src/runtime/mod.rs` | 47.8% | 157 | **The real gap.** Terminal setup/teardown and `run_loop`. Genuinely hard to cover, but it also holds `execute_commands`' drain loop and `apply_loop_event` (nesting depth 6), which are not untestable. |
| `src/cli/agent_tree.rs` | 56.0% | 81 | A second TUI with its own `handle_key` (cyc~25) and `run_loop` (depth 6). Lower stakes than the board, but it is a user-facing surface at 56%. |
| `src/setup/mod.rs` | 69.7% | 77 | OS-interaction branches. `docs/testing.md` explicitly excuses this — agreed. |
| `src/tui/input.rs` | 87.5% | 61 | Fine. |
| `src/tui/ui/input_form.rs` | 79.0% | 55 | Render-heavy. Excused by the same rule. |
| `src/tui/update/navigation.rs` | 75.3% | 38 | Lowest-coverage `update/` module; worth a look, since navigation is the most-exercised code path in the product. |

The two worth acting on are `src/runtime/mod.rs` and `src/tui/update/navigation.rs`. The rest are legitimately excused by the policy already written in `docs/testing.md`.

---

## 4. Complexity hotspots

1,312 production functions measured (inline test modules excluded).

### Longest

| Lines | Cyc (rough) | Nesting | Location |
|---:|---:|---:|---|
| 300 | 67 | 5 | `src/tui/input/normal.rs::handle_key_board_normal` |
| 238 | 27 | 0 | `src/db/mod.rs::create_task` |
| 204 | 12 | 0 | `src/service/api.rs::update_task` |
| 197 | 42 | 5 | `src/runtime/commands.rs::dispatch_task` |
| 168 | 31 | 6 | `src/dispatch/worktree.rs::provision_worktree` |
| 165 | 41 | 5 | `src/mcp/handlers/tasks/wrap_up.rs::handle_exit_session` |
| 147 | 18 | 4 | `src/db/queries/tasks.rs::upsert_feed_tasks_inner` |
| 130 | 25 | 6 | `src/feed/mod.rs::tick` |
| 127 | 44 | 4 | `src/tui/ui/kanban/status_bar.rs::status_line` |
| 126 | 30 | 7 | `src/mcp/handlers/dispatch.rs::handle_mcp` |

### Read these numbers carefully

Three of the top entries are **flat dispatch tables, not tangled logic**, and should not be refactored on the strength of a cyclomatic score:

- `handle_key_board_normal` (300 lines, cyc~67) is one `match` arm per key. Each arm is 3–8 lines and calls `dispatch_keyed`. The score reflects the size of the keymap, not branching depth. *If* it is ever touched, the win is making it a declarative `(KeyCode, Message, telemetry_label)` table — which would also make the keymap greppable and testable as data. That is a nice-to-have, not a debt.
- `db/mod.rs::create_task` (238 lines, nesting 0) and `service/api.rs::update_task` (204 lines, nesting 0) are wide-but-flat field plumbing over a 29-field struct. Nesting depth 0 is the tell. **These shrink as a side effect of fixing the `Task` fixture problem (§5.2), not by splitting the function.**

The ones that *are* worth attention:

- **`src/mcp/handlers/dispatch.rs::handle_mcp` — nesting depth 7.** The deepest function in the codebase, on the MCP request path, in a module `CLAUDE.md` explicitly says must never panic. Depth 7 is where a missing `else` hides.
- **`src/mcp/handlers/tasks/crud.rs::handle_list_tasks` — depth 7, cyc~23.** Same concern, same surface.
- **`src/dispatch/worktree.rs::provision_worktree` — 168 lines, cyc~31, depth 6.** This is inside the dispatch seam, which `CLAUDE.md` names as the most safety-critical sequence in the system (`DispatchClaimExclusive`).
- **`src/feed/mod.rs::tick` — 130 lines, cyc~25, depth 6.** The feed poll loop.
- **`src/runtime/tasks.rs::spawn_refresh_epic` — depth 7 in 41 lines.** Highest density of nesting per line anywhere; likely nested `if let` / `match` on `Option<Result<…>>` that `let ... else` would flatten in an afternoon.

### Long parameter lists

47 production functions take 6+ parameters (excluding `self`). Worst offenders:

| Params | Location |
|---:|---|
| 8 | `src/tui/ui/input_form.rs::repo_picker_lines` |
| 8 | `src/setup/mod.rs::run_setup_in` (carries `#[allow(clippy::too_many_arguments)]`) |
| 8 | `src/runtime/mod.rs::run_loop` |
| 8 | `src/feed/ingest/grouped.rs::upsert_sub_epic_and_recalc` |
| 7 | `src/tui/ui/kanban/cards.rs::build_task_list_item` |
| 7 | `src/dispatch/prompts.rs::build_prompt` |

The render-side ones (`input_form.rs`, `cards.rs`, `columns.rs`) share a clear cause: `Style` values threaded individually as `completed`, `active`, `hint`. A single `FormStyles { completed, active, hint }` struct — the codebase already has the pattern, `RepoListCtx` in `input_form.rs` — removes 2–3 parameters from about a dozen signatures.

Notably, only **one** function in the whole tree carries `#[allow(clippy::too_many_arguments)]`, so this is mostly sitting just under clippy's threshold rather than being suppressed.

### God types

| Fields / variants | Type |
|---:|---|
| 29 fields | `src/models/tasks.rs::Task` |
| 21 fields | `src/db/queries/tasks.rs::OwnedTaskPatch` |
| 18 fields | `src/service/tasks/params.rs::UpdateTaskParams` |
| 16 fields | `src/runtime/mod.rs::TuiRuntime` |
| 32 variants | `src/tui/types.rs::InputMode` |
| 29 variants | `src/tui/messages/task.rs::TaskMessage` |

`App` itself is **not** in this list, and that is deliberate and good: its state is grouped into `BoardState`, `InputState`, `SelectionState`, `FilterState`, `InteractionState`, `LayoutCache`, etc. The grouping is real encapsulation — `LayoutCache` exists specifically so the five mutually-coherent caches can only be invalidated as a unit.

The problem is on the *behaviour* side, not the data side. See §5.1.

---

## 5. Code smells

### 5.1 God object: 354 methods on `App` — the top structural finding

`App`'s data is well-grouped. Its behaviour is not:

| Methods | File |
|---:|---|
| 101 | `src/tui/mod.rs` |
| 32 | `src/tui/update/agent.rs` |
| 23 | `src/tui/input.rs` |
| 22 | `src/tui/update/epics.rs` |
| 22 | `src/tui/update/forms.rs` |
| 19 | `src/tui/update/todos.rs` |
| 18 | `src/tui/update/system.rs` |
| 17 | `src/tui/update/lifecycle.rs` |
| … | 14 further `impl App` blocks |
| **354** | **22 files, 22 `impl App` blocks** |

Splitting across files gives navigability but **zero encapsulation**: every one of the 354 methods can read and write every field of `App`. `src/tui/mod.rs` alone holds 101 methods in a single `impl App` block.

Why it matters concretely: the layout-cache coherence machinery in `docs/architecture.md` exists *because* any handler might mutate `board.tasks` without invalidating. The fingerprint-and-self-heal design is a well-built guard rail around a problem that a narrower method surface would not have. The self-healing is the right mitigation given the current shape — but it is a mitigation.

### 5.2 Duplicated `Task` fixtures — the top papercut

`Task` has 29 fields, no `Default` impl, and **23 sites construct it exhaustively**:

| Sites | File |
|---:|---|
| 4 | `src/models/epics.rs` |
| 3 | `src/models/tasks.rs` |
| 2 | `src/tui/tests/input_handlers.rs` |
| 1 each | `src/tui/tests/{helpers,todos,status_and_presets,dispatch}.rs`, `src/tui/types.rs`, `src/mcp/handlers/tests/tasks/crud.rs`, `src/mcp/handlers/tasks/mod.rs`, `src/feed/ingest/routing.rs`, `src/editor.rs`, `src/dispatch/tests.rs`, `src/db/queries/mod.rs`, `tests/tmux_lifecycle.rs`, `tests/lifecycle.rs`, `tests/dispatch_status_lifecycle.rs` |

They hide behind at least 11 differently-named local helpers: `make_task`, `make_task_with`, `test_task`, `sample_task`, `sample_task_with_url`, `make_unprovisioned_task`, `make_task_params`, `test_task_repo`, … Three of them are near-identical 29-field literals differing only in which fields are parameterised:

```rust
// src/tui/tests/helpers.rs:84
pub(in crate::tui) fn make_task(id: i64, status: TaskStatus) -> Task { Task { /* 29 fields */ } }
// src/models/epics.rs:361
fn make_task(id: i64, status: TaskStatus, sub_status: SubStatus, epic: Option<i64>) -> Task { Task { /* 29 fields */ } }
// src/dispatch/tests.rs:78
pub(super) fn make_task(repo_path: &str) -> Task { Task { /* 29 fields */ } }
```

Adding one field to `Task` is a 23-site mechanical edit. Only 2 sites in the whole tree use struct-update syntax (`..`), so the pattern that would make this free is known but unused.

The fix is small and safe: `impl Default for Task` (or a `Task::fixture()` behind `#[cfg(any(test, feature = "test-support"))]`), then rewrite the 23 sites as `Task { id: TaskId(1), status, ..Default::default() }`. It also shrinks `db/mod.rs::create_task` (238 lines) and `service/api.rs::update_task` (204 lines).

### 5.3 Primitive obsession on paths and window names

`Task` carries five stringly-typed identifiers:

```rust
pub repo_path: String,
pub worktree: Option<String>,
pub tmux_window: Option<String>,
pub plan_path: Option<String>,
pub base_branch: String,
```

The domain vocabulary already exists — but as free functions over `&str`, not types:

- `src/models/tmux_window.rs`: `build_tmux_window_name(TaskId) -> String`, `parse_tmux_window_task_id(&str) -> Option<TaskId>`
- `src/models/paths.rs`: `expand_tilde`, `repo_name_from_path`, `repo_name_from_url`, `extract_github_repo`

So `Command::Task(TaskCommand::KillTmuxWindow { window: String })` accepts any string, including a repo path or a branch name. This is not hypothetical risk: `CLAUDE.md` and `MockProcessRunner`'s own docs describe the tmux prefix-matching hazard at length —

> Every `tmux::` helper that takes a window *name* resolves it to a pane ID first, because tmux resolves a bare `-t <name>` by **prefix** and would otherwise act on a different task's window

— i.e. the codebase already knows that a bare window-name string is dangerous, and mitigates it with a *convention* (always call `window_target` first) plus a mock policy (`WindowLookup`) rather than a type. A `TmuxWindow(String)` newtype whose only constructors are `build_tmux_window_name(TaskId)` and a validating parse would make "killed the wrong task's window" unrepresentable instead of merely tested-for.

Contrast with the IDs, which *are* newtyped (`TaskId`, `EpicId`) and where no such class of bug exists. The pattern is established; it just has not reached the path/window fields.

### 5.4 Duplicated draft-summary preamble in the input form

`src/tui/ui/input_form.rs` repeats the same "read title / tag / description off `app.input.task_draft`" preamble across `input_description_lines`, `input_repo_path_lines`, `input_base_branch_lines`, and neighbours — 12+ identical lines each, four or more times:

```rust
let title = app.input.task_draft.as_ref().map(|d| d.title.as_str()).unwrap_or("");
let tag = app.input.task_draft.as_ref().and_then(|d| d.tag.as_ref())
    .map(|t| t.to_string()).unwrap_or_else(|| "none".to_string());
```

A `DraftSummary::from(&app.input)` returning `{ title, tag, description }` collapses all of it and removes a parameter or two from each signature at the same time (§4, long parameter lists).

### 5.5 Duplicated test harness setup in `src/setup/hooks.rs`

`hook_dispatches_user_prompt_submit_event` (≈line 278) re-inlines the entire body of the `spawn_hook_harness` helper (≈line 177) — git init, hook script write + chmod, `dispatch` shim on `PATH`, permissions — instead of calling it. ~30 duplicated lines. Test-only, but it is the kind of copy that silently stops matching the helper it was cloned from.

### 5.6 Migration DDL duplication — **not** a defect

`src/db/migrations.rs` contains 5 hand-written `CREATE TABLE tasks_new (…)` blocks across 93 migrations. This looks like duplication and is not: a shipped migration must be frozen, so it cannot be refactored to share a helper without changing history for existing databases. The repo has already drawn the right line — later migrations delegate to `rebuild_tasks_table_with_check` (`migrations.rs:1391`), and only the early frozen ones inline the DDL. **Leave this alone.** Flagged here only so a future reviewer does not "fix" it.

### 5.7 Dead code

Zero unreferenced public functions. Five public functions are referenced **only** from test code:

| Function | Location |
|---|---|
| `VisualColumn::parent_group_span` | `src/models/columns.rs:78` |
| `ReviewDecision::from_db_str` | `src/models/review.rs:63` |
| `TaskTag::short_label` | `src/models/tasks.rs:629` |
| `RepoSyncState::is_diverged` | `src/repo_sync.rs:34` |
| `RepoSyncState::is_measured` | `src/repo_sync.rs:348` |

At least two are deliberate: `is_measured` carries the comment `// derived.RepoSyncState.is_measured`, i.e. it exists to satisfy an Allium `derived` clause. That is a legitimate reason to keep a function with no production caller — but it is also a spec-vs-code signal worth resolving per case: either the UI should be *reading* that derived predicate (and currently open-codes the same condition), or the spec should not declare it. Worth one pass with `allium:weed`, not a deletion sweep.

`TaskTag::short_label` and `parent_group_span` have no such justification visible and look like genuine leftovers.

### 5.8 Test doubles in the production binary

`MockProcessRunner` (~310 lines, `src/process.rs:296–630`) plus `exit_ok` / `exit_fail` / `exit_code` are **not** `#[cfg(test)]`-gated, so they compile into the release binary.

This is a constraint, not carelessness: 9 files under `tests/` use `MockProcessRunner`, and `#[cfg(test)]` items are invisible to integration-test targets. The codebase is otherwise rigorous about this — `src/dispatch/mock_sequence.rs` (1,993 lines), `MockLearningService`, and the whole `service_api_stub_trait!` family are all correctly `#[cfg(test)]`-gated. `MockProcessRunner` is the single exception, and it is the one case where the gate genuinely cannot be `cfg(test)`.

The idiomatic fix is a `test-support` cargo feature (`#[cfg(any(test, feature = "test-support"))]`, enabled via a dev-dependency on the crate itself). Low priority — it costs binary size and a little API surface, not correctness.

### 5.9 Panic discipline — a strength, worth recording

Every `unwrap()` / `expect()` in the tree is either inside a `#[cfg(test)]` module or explicitly `#[allow]`-annotated: `cargo clippy --all-targets` produces **zero** diagnostics while `Cargo.toml` sets `unwrap_used = "warn"` / `expect_used = "warn"`, and the pre-push hook escalates to `-D warnings`.

Of 79 `#![allow(...)]` inner attributes, the overwhelming majority sit at the top of a `mod tests`. The production-path exceptions are individually justified in a trailing comment, e.g.:

```rust
#[allow(clippy::expect_used)] // invariant: we set Stdio::piped() above
let mut pipe = child.stdin.take().expect("stdin is piped");
```

Only one `#[allow(clippy::too_many_arguments)]` and one `#[allow(clippy::needless_range_loop)]` exist in the whole codebase. Suppression is not being used to dodge the lints.

### 5.10 Test file size

| Lines | File |
|---:|---|
| 7,916 | `src/runtime/tests.rs` |
| 5,138 | `src/service/tasks/tests.rs` |
| 4,456 | `src/db/tests/migrations.rs` |
| 4,298 | `src/db/tests/tasks.rs` |
| 3,933 | `src/mcp/handlers/tests/tasks/crud.rs` |
| 3,560 | `src/dispatch/tests.rs` |

`src/runtime/tests.rs` at 7,916 lines is the largest file in the repository by a wide margin — larger than any production file (`src/tmux.rs`, 2,700). The neighbouring `src/db/tests/` and `src/mcp/handlers/tests/` are already directories; `runtime` and `service/tasks` are not. Mechanical, zero-risk split whenever someone is next in there.

---

## 6. Magic wand: top 3 changes

### 1. Break `App` into per-domain façades — *maintainability*

**What:** replace the 22 `impl App` blocks (354 methods) with narrower borrow-scoped views — e.g. `BoardOps<'a>`, `SelectionOps<'a>`, `FormOps<'a>` — each holding `&mut` to only the sub-state it needs (`board`, `select`, `input`, …) rather than all of `App`.

**Why it is #1:** it converts a whole class of bug from "guarded at runtime" to "impossible". `LayoutCache`'s fingerprint-and-self-heal machinery exists precisely because any of 354 methods could mutate `board.tasks` and forget to invalidate. That mitigation is well-built and correct — but a `BoardOps` that owns both the mutation and the invalidation makes it unnecessary. Same story for the render dirty flag, whose history (`docs/architecture.md`) is three separate fixes for handlers that forgot to set it, ending in a fail-open design.

**Cost:** the largest item here, and it churns the 2,053 direct `app.<field>.<field>` reads in `src/tui/tests/`. Worth doing incrementally, one sub-state at a time, starting with `board` (highest value: it is what the caches derive from).

### 2. One `Task` fixture — *developer productivity*

**What:** add `impl Default for Task` (or `Task::fixture()` behind `#[cfg(any(test, feature = "test-support"))]`); rewrite the 23 exhaustive literal sites as `Task { id: TaskId(1), status, ..Default::default() }`; collapse the 11 near-duplicate `make_task` / `test_task` / `sample_task` helpers into one shared fixture.

**Why:** best value-per-hour in this document. It turns "add a field to `Task`" from a 23-site mechanical edit into a 1-line change, removes ~600 lines of boilerplate, and shrinks the #2 and #3 longest functions in the codebase (`db/mod.rs::create_task`, 238 lines; `service/api.rs::update_task`, 204 lines) as a side effect — both of which are wide-and-flat field plumbing at nesting depth 0, not complex logic.

**Cost:** one focused session. Fully mechanical, entirely test-guarded by an already-green 4328-test suite.

### 3. Newtype the path and window strings — *bug reduction*

**What:** introduce `RepoPath`, `WorktreePath`, `TmuxWindow`, `BranchName` and thread them through `Task`, `TaskCommand`, and the `tmux::` helpers. Give `TmuxWindow` exactly two constructors: `build_tmux_window_name(TaskId)` and a validating parse.

**Why:** the codebase already documents the exact bug this prevents — tmux resolves a bare `-t <name>` by prefix, so `task-4` can act on `task-42`'s window. Today that is defended by a convention ("always call `window_target` first"), a doc paragraph, and a mock policy enum (`WindowLookup`). A newtype makes it unrepresentable. The pattern is already proven in this codebase: `TaskId` / `EpicId` are newtyped, and there is no corresponding class of ID-confusion bug.

**Cost:** wide but shallow, and the compiler drives it end to end. Can land field-by-field, starting with `tmux_window` (highest risk, smallest surface — grep gives ~8 production call sites).

**Honourable mention (fourth):** flatten the three depth-7 functions — `mcp/handlers/dispatch.rs::handle_mcp`, `mcp/handlers/tasks/crud.rs::handle_list_tasks`, `runtime/tasks.rs::spawn_refresh_epic`. Mostly `let ... else` and early returns. Half a day, and it is on the MCP request path that `CLAUDE.md` says must never panic.

---

## 7. `CLAUDE.md` improvements

`CLAUDE.md` is 159 lines / 20,384 bytes, loaded into every agent's context. It is genuinely good — dense, opinionated, specific about hazards, and it links out rather than duplicating. The following are gaps, not rewrites.

### 7.1 Fix the verify-command drift — do this first

`CLAUDE.md` states:

> **This repo's verify command is `cargo test`** — the thing every dispatched agent must run green before declaring work complete.

The command actually stored on the `repo_paths` row, as returned by `get_task`, is:

```
cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh
```

An agent that trusts `CLAUDE.md` runs a weaker gate than the one the tool reports. This is the single highest-value fix in this section: the file's own "Verify Command" section explains that the value reaches agents through `get_task`, so `CLAUDE.md` should cite that as the source of truth rather than restating a stale copy.

### 7.2 Reclaim ~18% of the context budget

Six paragraphs (3,758 bytes, **18% of the file**) describe sandbox-specific failures. The file itself opens that discussion by saying they no longer apply:

> **Dispatch-spawned sessions no longer run under Claude Code's sandbox** (as of task #4373)

Each paragraph then repeats a variant of "this exception is now moot … still relevant if you enable the sandbox yourself." That is history worth keeping, but not in the file loaded into every agent's context. Move it to `docs/reference.md` under a "Sandbox (historical)" heading and leave a two-line pointer.

Section budget for reference:

| Bytes | % | Section |
|---:|---:|---|
| 5,995 | 29% | `## Documentation` |
| 5,247 | 25% | `## Build & Test` |
| 1,936 | 9% | `### First-time setup` |
| 1,129 | 5% | `## Working With the User` |
| 1,075 | 5% | `## MCP Tools for Agents` |

### 7.3 Missing context that would speed an agent up

- **The `service_api!` macro family.** The "Workhorse macros" note names `patch_struct!` and `mcp_tools!` but not `task_service_api!` / `service_api_trait!` / `service_api_delegate!` / `service_api_stub_trait!` / `service_api_stub_bridge!` in `src/service/api.rs`. Adding a method to a service seam is a common task and the mechanism is two macro layers deep with call-site type resolution. One line, same style as the existing macro note.
- **Suite timing.** Measured: 9.5s for the 4328-test lib target, ~80s wall from cold including compile. `CLAUDE.md` says nothing, so agents guess and some background the run. This already exists as knowledge-base entry #428 — it belongs in the file that is always loaded.
- **How to run coverage locally.** `docs/testing.md` gives the CI invocation and is emphatic that the engine is part of the measurement, but neither file gives the copy-pasteable local command. Worth one line: `cargo tarpaulin --engine llvm --out stdout` (default `Auto` engine reads ~1.8 points lower and must not be compared against the floor).
- **Where `Task` fixtures come from.** Currently nowhere, which is why there are 11 of them. Once §5.2 lands, one line naming the single fixture prevents the twelfth.
- **Why `MockProcessRunner` is ungated.** It is the one deliberate exception to an otherwise strict `#[cfg(test)]` rule. Without a note, a well-meaning agent will "fix" it and break the 9 integration-test files that depend on it.

### 7.4 Implicit assumptions worth making explicit

- **`models` is a leaf, and the four `tui`/`mcp` boundaries are at zero.** `CLAUDE.md` documents the *mutation* boundary (`state.db` typed as `TaskReadStore`) because the compiler enforces it, but the read-side layering (`tui → db` = 0, `tui → tmux` = 0, `mcp → tui` = 0, `models → db` = 0) is enforced only by habit. It is currently perfect. Stating it as a rule is what keeps it perfect — right now an agent has no way to know it is a rule.
- **The `#[cfg(test)]` gating rule itself.** The codebase applies it rigorously (`mock_sequence`, `MockLearningService`, all stub traits) with one documented exception. The rule is not written down anywhere.
- **Coverage is at 91.56%, not 90.28%.** `docs/testing.md` cites the 2026-08-16 figure. Not urgent — the floor is the contract, and it correctly has not moved — but a stale headline number invites someone to "restore" coverage that never dropped.

---

## 8. Prioritised action items

### Quick wins (< 1 day each, low risk)

| # | Action | Where | Payoff |
|---|---|---|---|
| 1 | Correct the verify-command line to point at `get_task` rather than restating `cargo test` | `CLAUDE.md` | Agents stop running a weaker gate than CI |
| 2 | Move the six sandbox-history paragraphs to `docs/reference.md`, leave a pointer | `CLAUDE.md` → `docs/reference.md` | Frees 18% of the always-loaded context |
| 3 | Add `impl Default for Task`; convert the 23 exhaustive literals to `..Default::default()` | `src/models/tasks.rs` + 23 sites | "Add a `Task` field" becomes a 1-line change |
| 4 | Extract `DraftSummary` for the repeated draft-reading preamble | `src/tui/ui/input_form.rs` | Kills 4× 12-line duplication, drops params |
| 5 | Make `hook_dispatches_user_prompt_submit_event` call `spawn_hook_harness` | `src/setup/hooks.rs:278` | ~30 duplicated lines gone |
| 6 | Add the `service_api!` family, suite timing, and the local coverage command | `CLAUDE.md` | Removes three recurring unknowns |
| 7 | Resolve the 5 test-only predicates: wire into production, or drop from spec + code | `columns.rs`, `review.rs`, `tasks.rs`, `repo_sync.rs` | Real spec/code alignment; run under `allium:weed` |
| 8 | Introduce `FormStyles { completed, active, hint }` | `src/tui/ui/{input_form,kanban/*}.rs` | −2/−3 params across ~12 signatures |

### Medium (2–5 days, contained)

| # | Action | Payoff |
|---|---|---|
| 9 | Flatten the three depth-7 functions with `let ... else` / early return | Removes the deepest nesting from the MCP request path |
| 10 | Collapse the 11 `make_task`/`test_task`/`sample_task` helpers onto the §5.2 fixture | One fixture, one place to change |
| 11 | Split `src/runtime/tests.rs` (7,916 lines) and `src/service/tasks/tests.rs` (5,138) into directories, matching `src/db/tests/` | The two largest files in the repo become navigable |
| 12 | Raise `src/runtime/mod.rs` (47.8%) and `src/tui/update/navigation.rs` (75.3%) | The only two coverage gaps not excused by existing policy |
| 13 | Gate `MockProcessRunner` behind a `test-support` feature | Test scaffolding out of the release binary |
| 14 | Decompose `provision_worktree` (168 lines, cyc~31, depth 6) and `feed::tick` (130, cyc~25, depth 6) | Both sit on safety-critical paths (`DispatchClaimExclusive`, feed poll) |

### Larger efforts (deliberate, staged)

| # | Action | Payoff |
|---|---|---|
| 15 | Newtype `TmuxWindow`, then `RepoPath` / `WorktreePath` / `BranchName` | Makes "killed the wrong task's window" unrepresentable. Start with `tmux_window` — ~8 production call sites |
| 16 | Split `App`'s 354 methods into borrow-scoped per-domain façades, starting with `board` | Retires the layout-cache and dirty-flag guard rails by removing what they guard against |
| 17 | Turn `handle_key_board_normal` (300 lines) into a declarative keymap table | Keymap becomes greppable and testable as data. Genuinely optional |

### Explicitly do not do

- **Do not deduplicate the 5 `CREATE TABLE tasks_new` blocks** in `src/db/migrations.rs` (§5.6). Shipped migrations must stay frozen. The repo has already drawn this line correctly.
- **Do not chase coverage in `src/setup/` or the render modules.** `docs/testing.md` excuses them by policy and the reasoning is sound.
- **Do not raise the coverage floor to today's 91.56%.** It is a regression tripwire, not a target, and `docs/testing.md` says so explicitly.

---

## 9. Closing note

The measurable health of this codebase is in the top few percent of what a review like this normally finds: coverage above 91% against an enforced floor, zero clippy warnings with `unwrap_used` and `expect_used` set to warn, four green gate scripts, an enforced mutation boundary, and clean layering at every boundary that usually rots first.

The debts that remain are the kind a codebase earns by growing successfully rather than by being built carelessly: a `Task` struct that accumulated 29 fields faster than its fixtures were consolidated, an `App` whose files were split but whose type never was, and domain strings that were never promoted to types even though the ID newtypes next to them prove the team knows the pattern.

Items 1–8 are worth doing on the next quiet afternoon. Items 15–16 are worth a plan.
