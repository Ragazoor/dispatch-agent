# WP-5: Service Layer Split

## Context

`src/service.rs` has grown to 3,519 lines containing three distinct services (TaskService, EpicService, LearningService), their parameter structs, error types, and extensive inline test modules. This makes it hard to navigate and understand the business logic for any one domain.

## Findings

### L1 — `src/service.rs` is a 3,519-line god file
- **Severity**: large
- **Files**: `src/service.rs`
- **Issue**: `TaskService` (~1,500 lines), `EpicService` (~900 lines), and `LearningService` (~400 lines) share a single file with `ServiceError`, `FieldUpdate`, and all their params structs. `cargo test service::tasks::tests` is impossible — all service tests run under a single module path.
- **Fix**: Convert to a `service/` module:
  - `src/service/mod.rs` — `ServiceError`, `FieldUpdate`, public re-exports
  - `src/service/tasks.rs` — `TaskService`, `UpdateTaskParams`, `CreateTaskParams`, `ClaimTaskParams`
  - `src/service/epics.rs` — `EpicService`, `UpdateEpicParams`
  - `src/service/learnings.rs` — `LearningService`, `LearningFilter`
  - Inline `#[cfg(test)]` modules stay in their respective files

## Implementation Notes

This is a pure refactor — no logic changes.

1. Create `src/service/` directory
2. Move `ServiceError` and `FieldUpdate` to `src/service/mod.rs`
3. Move `TaskService` + params to `src/service/tasks.rs`
4. Move `EpicService` + params to `src/service/epics.rs`
5. Move `LearningService` + filter to `src/service/learnings.rs`
6. Re-export everything from `src/service/mod.rs` so all existing `use crate::service::*` imports continue to work
7. Delete `src/service.rs`

All callers use `crate::service::TaskService`, `crate::service::ServiceError`, etc. — these paths survive as re-exports from `mod.rs`.

## Changes Table

| File | What to change |
|---|---|
| `src/service.rs` | Delete after contents are migrated |
| `src/service/mod.rs` | New — `ServiceError`, `FieldUpdate`, re-exports |
| `src/service/tasks.rs` | New — `TaskService`, task params structs |
| `src/service/epics.rs` | New — `EpicService`, epic params structs |
| `src/service/learnings.rs` | New — `LearningService`, learning filter |

## Verification

```bash
cargo build
cargo test service::
cargo clippy --all-targets -- -D warnings
```

All existing service tests must pass at their new module paths.
