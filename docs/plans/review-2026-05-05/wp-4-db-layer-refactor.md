# WP-4: DB Layer Refactor

## Context

Three related improvements in the database layer were identified in the code review: a long-parameter-list smell on `create_task`, a 2,033-line query file that mixes SQL for all domains, and a 6,488-line test file that's hard to navigate.

## Findings

### M4 — `create_task()` has 10 raw parameters in the DB trait
- **Severity**: medium
- **Files**: `src/db/mod.rs`, `src/db/queries.rs`
- **Issue**: The `TaskCrud::create_task` trait method takes 10 positional parameters. The service layer already has `CreateTaskParams`, but the DB-layer trait method bypasses it. Callers must supply all 10 values in order, which is error-prone and makes future additions a breaking change to the trait.
- **Fix**: Introduce a `CreateTaskRequest` struct at the DB layer (similar to `TaskPatch`). Change the trait method to `fn create_task(&self, req: CreateTaskRequest) -> Result<TaskId>`. The service layer constructs `CreateTaskRequest` from its `CreateTaskParams`.

### L2 — `src/db/queries.rs` mixes SQL for all domains (2,033 lines)
- **Severity**: large
- **Files**: `src/db/queries.rs`
- **Issue**: A single file implements `TaskCrud`, `EpicCrud`, `PrStore`, `AlertStore`, `SettingsStore`, `PrWorkflowStore`, `ProjectCrud`, and `LearningStore`. Finding the query for a specific domain requires scrolling through 2,000 lines.
- **Fix**: Split into per-domain files:
  - `src/db/queries/tasks.rs` — `TaskCrud` impl
  - `src/db/queries/epics.rs` — `EpicCrud` impl
  - `src/db/queries/learnings.rs` — `LearningStore` impl
  - `src/db/queries/projects.rs` — `ProjectCrud` impl
  - `src/db/queries/prs.rs` — `PrStore` + `PrWorkflowStore` impl
  - `src/db/queries/alerts.rs` — `AlertStore` impl
  - `src/db/queries/settings.rs` — `SettingsStore` impl
  - `src/db/queries/mod.rs` — re-exports, shared row helpers

### L4 — `src/db/tests.rs` mixes tests for all domains (6,488 lines)
- **Severity**: medium
- **Files**: `src/db/tests.rs`
- **Issue**: All 198 database tests live in one file alongside 6,488 lines. Hard to run or review tests for a specific domain.
- **Fix**: Mirror the queries split: `src/db/tests/tasks.rs`, `epics.rs`, `learnings.rs`, `projects.rs`, `prs.rs`, `alerts.rs`, `migrations.rs` and a `mod.rs` with shared setup.

## Implementation Notes

Do M4 first — the `CreateTaskRequest` struct is a prerequisite for a clean queries split. Then L2 (split queries), then L4 (split tests).

All changes are pure refactors — no behaviour changes.

## Changes Table

| File | What to change |
|---|---|
| `src/db/mod.rs` | Add `CreateTaskRequest` struct; update `TaskCrud` trait signature |
| `src/db/queries.rs` | Delete after contents migrated to `queries/` |
| `src/db/queries/mod.rs` | New — shared helpers, re-exports |
| `src/db/queries/tasks.rs` | New — TaskCrud impl |
| `src/db/queries/epics.rs` | New — EpicCrud impl |
| `src/db/queries/learnings.rs` | New — LearningStore impl |
| `src/db/queries/projects.rs` | New — ProjectCrud impl |
| `src/db/queries/prs.rs` | New — PrStore + PrWorkflowStore impl |
| `src/db/queries/alerts.rs` | New — AlertStore impl |
| `src/db/queries/settings.rs` | New — SettingsStore impl |
| `src/db/tests.rs` | Delete after contents migrated to `tests/` |
| `src/db/tests/mod.rs` | New — shared setup |
| `src/db/tests/tasks.rs` | New — task DB tests |
| `src/db/tests/epics.rs` | New — epic DB tests |
| `src/db/tests/learnings.rs` | New — learning DB tests |
| `src/db/tests/projects.rs` | New — project DB tests |
| `src/db/tests/migrations.rs` | New — migration version tests |

## Verification

```bash
cargo test db::
```

All 198 existing tests must pass. The `fresh_db_has_latest_schema_version` test must still exist in `tests/migrations.rs`.
