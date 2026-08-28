# Task Fixture Consolidation

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Turn "add a field to `Task`" from a 20-site mechanical edit into a one-line change, by giving `Task` a single fixture and collapsing the 13 differently-named local helpers that exist because there wasn't one.

## Context

This work package addresses findings from the whole-repository codebase review of 2026-08-28 (`docs/plans/2026-08-28-codebase-review.md`, sections 5.2 and 6.2). The review rated it **best value-per-hour in the document**.

`Task` (`src/models/tasks.rs:361`) has **29 fields** and no `Default` impl. Every construction site therefore lists all 29. Only 2 sites in the whole tree use struct-update syntax (`..`), so the pattern that would make this free is known but unused.

This is fully mechanical and entirely guarded by an already-green 4328-test suite.

**Ordering note:** run this work package **before** WP5 (Dead and Test-Only Predicates). Both touch `src/models/tasks.rs`, in different regions.

## Findings

### ⚠️ 20 exhaustive 29-field `Task` literals (`src/models/tasks.rs:361` + 20 sites)

**Issue:** Adding one field to `Task` requires editing 20 places, almost all of it identical boilerplate. The full list, found by grepping for the last field (`oldest_live_shell_started_at:`):

| Site | Notes |
|---|---|
| `src/editor.rs:631` | inside `fn make_task` (test mod) |
| `src/dispatch/tests.rs:108` | inside `fn make_task` |
| `src/feed/ingest/routing.rs:163` | inside `fn make_task` (test mod) |
| `src/mcp/handlers/tests/tasks/crud.rs:601` | |
| `src/mcp/handlers/tasks/mod.rs:467` | inside `fn make_task` (test mod) |
| `src/tui/tests/todos.rs:40` | |
| `src/tui/tests/helpers.rs:116` | inside `fn make_task` |
| `src/tui/tests/status_and_presets.rs:691` | |
| `src/tui/tests/input_handlers.rs:39` | |
| `src/tui/tests/input_handlers.rs:732` | |
| `src/tui/tests/dispatch.rs:1766` | |
| `src/tui/types.rs:1076` | test mod |
| `src/models/epics.rs:391` | inside `fn make_task` |
| `src/models/epics.rs:623` | |
| `src/models/epics.rs:660` | |
| `src/models/epics.rs:760` | inside `fn test_task` |
| `src/models/tasks.rs:1979` | inside `fn make_task_with` |
| `tests/lifecycle.rs:91` | integration target |
| `tests/dispatch_status_lifecycle.rs:42` | integration target |
| `tests/tmux_lifecycle.rs:220` | integration target |

Three of them are near-identical literals differing only in which fields are parameterised:

```rust
// src/tui/tests/helpers.rs:84
pub(in crate::tui) fn make_task(id: i64, status: TaskStatus) -> Task { Task { /* 29 fields */ } }
// src/models/epics.rs:361
fn make_task(id: i64, status: TaskStatus, sub_status: SubStatus, epic: Option<i64>) -> Task { Task { /* 29 fields */ } }
// src/dispatch/tests.rs:78
pub(super) fn make_task(repo_path: &str) -> Task { Task { /* 29 fields */ } }
```

**Fix:** Add a single fixture and rewrite each site as struct-update syntax.

Because three of the sites live in `tests/` integration targets, a `#[cfg(test)]` fixture will **not** be visible to them (integration-test targets cannot see `cfg(test)` items — this is the same constraint that forces `MockProcessRunner` to stay ungated, see WP9). Two options:

1. **`impl Default for Task`** — always available, simplest, no feature plumbing. Costs a public `Default` impl on a domain type, which some would argue invites accidental construction of a meaningless `Task` in production code.
2. **`Task::fixture()` behind `#[cfg(any(test, feature = "test-support"))]`** — keeps the fixture out of production, but needs the `test-support` feature. WP9 introduces exactly that feature for `MockProcessRunner`.

**Recommendation:** implement option 1 (`impl Default for Task`) now, because it unblocks the mechanical work immediately and has no dependency on WP9. If WP9 lands first, prefer option 2 and reuse its feature. Whichever you pick, say which in the commit message.

Sensible defaults: `id: TaskId(0)`, empty `title`/`description`, `repo_path: "/repo"`, `status: TaskStatus::Backlog`, `sub_status: SubStatus::None`, `base_branch: "main"`, `labels: Vec::new()`, `created_at`/`updated_at: Utc::now()`, every `Option` `None`, every `bool` `false`, every count `0`.

Then each site becomes, e.g.:

```rust
Task { id: TaskId(id), title: format!("Task {id}"), status, ..Default::default() }
```

**Do not convert `src/db/queries/mod.rs:254`.** That is `row_to_task`, production code mapping DB columns to fields. Its exhaustiveness is a **feature** — it is what makes the compiler catch a new field that nobody wired to a column. Leave it listing all 29 fields.

Also leave alone `src/models/tasks.rs:422` (the field declaration itself) and `src/models/tasks.rs:1123` (a parameter of `classify_agent_activity`) — both matched the grep but are not literals.

### 💡 13 differently-named fixture helpers (across `src/` and `tests/`)

**Issue:** Because no shared fixture exists, each module grew its own:

| Helper | Location |
|---|---|
| `make_task` | `src/editor.rs:595`, `src/db/tests/mod.rs:68`, `src/feed/ingest/routing.rs:132`, `src/dispatch/tests.rs:78`, `src/mcp/handlers/tasks/mod.rs:436`, `src/dispatch/prompts.rs:1776`, `src/mcp/handlers/tests/tasks/watch.rs:4`, `src/models/epics.rs:361`, `src/service/tasks/tests.rs:643`, `src/tui/tests/helpers.rs:84`, `tests/dispatch_status_lifecycle.rs:11` |
| `sample_task` | `src/editor.rs:812` |
| `sample_task_with_url` | `src/editor.rs:1043` |
| `test_task` | `src/models/epics.rs:730`, `src/tui/tests/search.rs:5` |
| `test_task_repo` | `src/tui/tests/search.rs:9` |
| `make_unprovisioned_task` | `src/tui/tests/helpers.rs:124` |
| `make_task_with` | `src/models/tasks.rs:1948` |
| `make_task_params` | `src/service/tasks/tests.rs:46` |

Eleven functions share the name `make_task` with **five different signatures**, which makes them un-greppable as a group and means a reader has to check which one is in scope.

**Fix:** Once the fixture exists, most of these collapse to a one-line wrapper or disappear at the call site. Be conservative and judgement-driven here:

- Helpers that only fill in defaults (`sample_task`, `test_task`, the zero-arg `make_task`s) should go away — call `Task { ..Default::default() }` inline.
- Helpers that carry **meaning** should stay, and keep their doc comments. `make_unprovisioned_task` (`src/tui/tests/helpers.rs:124`) is the clearest example: its comment ties it to `UnprovisionedIndicator` in `docs/specs/dispatch.allium` and to `App::dispatch_may_be_in_flight`. That is a named domain state, not boilerplate. Reimplement it on top of the fixture; do not delete it.
- Helpers that are **not** `Task` constructors at all must be left alone: `make_task_params` returns `CreateTaskParams`, `src/db/tests/mod.rs:68`, `src/service/tasks/tests.rs:643`, `src/dispatch/prompts.rs:1776` and `src/mcp/handlers/tests/tasks/watch.rs:4` are `async` and go through the database or service. They are out of scope.

Do not rename surviving helpers just for consistency — that is churn on 2,053 test call sites for no behavioural gain.

## Changes

| File | Change |
|------|--------|
| `src/models/tasks.rs` | Add `impl Default for Task` (or `Task::fixture()` behind `test-support` if WP9 landed first) with the defaults listed above |
| `src/editor.rs` | Convert the literal at :631; drop `sample_task`/`sample_task_with_url` if they become trivial |
| `src/dispatch/tests.rs` | Convert :108 |
| `src/feed/ingest/routing.rs` | Convert :163 |
| `src/mcp/handlers/tests/tasks/crud.rs` | Convert :601 |
| `src/mcp/handlers/tasks/mod.rs` | Convert :467 |
| `src/tui/tests/todos.rs` | Convert :40 |
| `src/tui/tests/helpers.rs` | Convert :116; reimplement `make_unprovisioned_task` on the fixture, keeping its doc comment |
| `src/tui/tests/status_and_presets.rs` | Convert :691 |
| `src/tui/tests/input_handlers.rs` | Convert :39 and :732 |
| `src/tui/tests/dispatch.rs` | Convert :1766 |
| `src/tui/tests/search.rs` | Collapse `test_task` / `test_task_repo` onto the fixture |
| `src/tui/types.rs` | Convert :1076 |
| `src/models/epics.rs` | Convert :391, :623, :660, :760; collapse `test_task` |
| `tests/lifecycle.rs` | Convert :91 |
| `tests/dispatch_status_lifecycle.rs` | Convert :42; collapse local `make_task` |
| `tests/tmux_lifecycle.rs` | Convert :220 |
| `src/db/queries/mod.rs` | **No change** — `row_to_task` must stay exhaustive |

## Verification

- [ ] `cargo test` — all 4328 lib tests plus every integration target pass. This is the real proof; the change is mechanical and the suite is dense enough to catch a wrong default
- [ ] `cargo clippy --all-targets -- -D warnings` — clean. Watch for `clippy::derivable_impls` if you hand-write `Default` where a derive would do, and for `field_reassign_with_default`
- [ ] `cargo fmt` — run it before committing; the pre-push hook's `cargo fmt` step has no `--check` and will otherwise reformat your tree during the push
- [ ] Confirm `src/db/queries/mod.rs::row_to_task` still lists all 29 fields — deliberately not converted
- [ ] Sanity-check the fixture's defaults against a test that previously relied on a *different* default. `created_at`/`updated_at` and `base_branch: "main"` are the two most likely to matter
- [ ] Verify the win: adding a throwaway field to `Task` should now break only `row_to_task` and the `Default` impl. Revert the throwaway afterwards
