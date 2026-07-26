# WP-6: Models Split

## Context

`src/models.rs` has grown to 3,616 lines containing ID newtypes, domain model structs, serialization/deserialization logic, and test code for tasks, epics, learnings, projects, and review PRs. It is the second-largest file in the codebase and mixes concerns across multiple domains.

## Findings

### L3 — `src/models.rs` is a 3,616-line god file
- **Severity**: large
- **Files**: `src/models.rs`
- **Issue**: All domain types live in one file. When searching for `EpicSubstatus` or `LearningScope`, you scan 3,600 lines rather than opening a focused file.
- **Fix**: Convert to a `models/` module:
  - `src/models/mod.rs` — the `define_id_newtype!` macro, shared enums used across domains, public re-exports
  - `src/models/tasks.rs` — `Task`, `TaskStatus`, `SubStatus`, `TaskTag`, `TaskId`, `DispatchMode`
  - `src/models/epics.rs` — `Epic`, `EpicStatus`, `EpicSubstatus`, `EpicId`
  - `src/models/learnings.rs` — `Learning`, `LearningKind`, `LearningScope`, `LearningStatus`, `LearningId`
  - `src/models/projects.rs` — `Project`, `ProjectId`
  - `src/models/review.rs` — `ReviewPr`, `ReviewAgentStatus`, `SecurityAlert`
  - Inline `#[cfg(test)]` modules stay in their respective files

## Implementation Notes

This is a pure refactor — no logic changes.

1. Create `src/models/` directory
2. Move the `define_id_newtype!` macro to `mod.rs` (used by all domain files)
3. Split types by domain, keeping each type's `impl` blocks and tests in the same file
4. Re-export everything from `src/models/mod.rs` so all existing `use crate::models::*` imports continue to work without changes
5. Delete `src/models.rs`

## Changes Table

| File | What to change |
|---|---|
| `src/models.rs` | Delete after contents are migrated |
| `src/models/mod.rs` | New — `define_id_newtype!` macro, shared types, re-exports |
| `src/models/tasks.rs` | New — Task, TaskStatus, SubStatus, TaskTag, DispatchMode |
| `src/models/epics.rs` | New — Epic, EpicStatus, EpicSubstatus |
| `src/models/learnings.rs` | New — Learning, LearningKind, LearningScope, LearningStatus |
| `src/models/projects.rs` | New — Project, ProjectId |
| `src/models/review.rs` | New — ReviewPr, ReviewAgentStatus, SecurityAlert |

## Verification

```bash
cargo build
cargo test models::
cargo clippy --all-targets -- -D warnings
```

All existing model tests must pass at their new module paths. No snapshot updates expected.
