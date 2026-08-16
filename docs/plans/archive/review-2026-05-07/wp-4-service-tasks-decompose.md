# WP4: Decompose `src/service/tasks.rs`

## Context

`src/service/tasks.rs` is 2,634 LOC — the largest non-test file in the repo and the navigational bottleneck for almost every task-related change. Its concerns split cleanly along three axes (CRUD, validation, parameter shapes) that already exist in the file's structure.

## Findings

- **Severity:** Medium (code-organisation)
- **File:** `src/service/tasks.rs` (2,634 LOC)
- **Issue:** Single file holds CRUD ops, business-rule validators, and parameter builders. Time-to-find for any single concern is high.
- **Suggestion:** Promote `tasks.rs` to a `tasks/` submodule split by concern.

## Plan (refactor — TDD-friendly because tests stay green throughout)

Pure refactor: no behaviour change, all tests must pass at every step.

1. **Pre-step:** run `cargo test service::` and capture passing baseline.
2. **Convert** `src/service/tasks.rs` → `src/service/tasks/mod.rs`. Re-export everything publicly used externally (likely just `TaskService`, `*Params` structs, and any error variants).
3. **Extract** parameter structs into `src/service/tasks/params.rs`:
   - `CreateTaskParams`, `UpdateTaskParams`, `ClaimTaskParams`, `ListTasksFilter`, and any builder helpers.
   - Keep `Default`/`new` impls with the structs.
4. **Extract** CRUD methods into `src/service/tasks/crud.rs`:
   - `create_task`, `get_task`, `list_tasks`, `update_task`, `claim_task`, `delete_task`.
   - Methods can stay as `impl TaskService` blocks split across files.
5. **Extract** validators into `src/service/tasks/validators.rs`:
   - `has_any_field`, sub-status legality checks, transition guards, etc.
   - Pure functions where possible; methods on `TaskService` only when they touch `self.db`.
6. **Inline tests** — each test should follow the code it tests:
   - Parameter-shape tests → `params.rs`
   - CRUD tests → `crud.rs`
   - Validation tests → `validators.rs`
7. **Run** `cargo test service::` and `cargo clippy --all-targets -- -D warnings` after each extraction. Any breakage means stop and fix before continuing.

## Files to change

| File | Change |
|---|---|
| `src/service/tasks.rs` | Delete (replaced by submodule). |
| `src/service/tasks/mod.rs` | New. Re-exports + any glue. |
| `src/service/tasks/crud.rs` | New. CRUD `impl TaskService` block + tests. |
| `src/service/tasks/validators.rs` | New. Validation helpers + tests. |
| `src/service/tasks/params.rs` | New. Parameter structs + tests. |
| `src/service/mod.rs` | Adjust `pub use` if needed. |

## Verification

```bash
cargo test           # full suite must pass
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

`grep -n "use crate::service::tasks" src/` should still resolve cleanly. No call sites should need changes.
