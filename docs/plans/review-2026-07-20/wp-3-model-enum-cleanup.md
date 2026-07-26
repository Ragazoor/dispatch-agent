# Model Enum & Newtype Cleanup

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Reduce the parallel-match burden of `SubStatus`, collapse the hand-rolled enum trait impls, and fix `BranchName`'s validation-implying surface.

## Context

This work package addresses findings from a code review, all within `src/models/tasks.rs`.
Follow the spec-first workflow: `SubStatus` and the tag/status enums are domain types — consult
`docs/specs/core.allium` and `docs/specs/tasks.allium` before changing behaviour, and update the
spec via `allium:tend` if semantics move.

## Findings

### 💡 `SubStatus` god enum drives eight parallel matches (`src/models/tasks.rs:165-289`)

**Issue:** One 9-variant enum drives eight parallel match arms (`as_str`, `parse`, `is_valid_for`,
`default_for`, `column_priority`, `column_priority_detached`, `header_label`, `header_label_detached`).
Adding a variant means touching eight matches. The `_detached` variants special-case a single
`(AwaitingReview, true)` tuple — a *display* concern leaking into the model. Magic priority ints
(`5` with "same slot" comments, bare `7`) are scattered.

**Fix:** Move the `_detached` display special-casing out of the model into the presentation layer
(the caller already knows the detached flag). Replace magic priority ints with named constants or a
derived ordering. Consider consolidating the remaining per-variant data into a single table/match
(one match returning a struct of properties) so a new variant touches one place.

### 💡 ~7 enums hand-roll identical `as_str`/`parse`/`Display`/`FromStr` (~200 LOC)

**Issue:** ~7 enums repeat the same string-conversion boilerplate that a derive macro would collapse.

**Fix:** Adopt a derive (e.g. `strum`, if an acceptable dependency — vet via
`kognic-code-quality:dependency-review`) or a small local `macro_rules!` to generate
`as_str`/`FromStr`/`Display`. Preserve exact string values (DB-persisted — verify against
`src/db/tests/migrations.rs` and any `parse` round-trip tests).

### 💡 `BranchName` newtype implies validation it doesn't perform (`src/models/tasks.rs:311-387`)

**Issue:** ~11 trait impls suggest a validated type, but `from` just wraps and the field is
`pub String` — no validation.

**Fix:** Either make it a real validated newtype (private field, validating constructor returning
`Result`) or drop the newtype and use `String` directly. Pick whichever matches actual usage; do not
leave a newtype that pretends to guarantee something it doesn't.

## Changes

| File | Change |
|------|--------|
| `src/models/tasks.rs` | Move `_detached` display logic out of `SubStatus`; replace magic priority ints with named constants; consolidate per-variant data. |
| `src/models/tasks.rs` | Introduce derive/macro for the ~7 string-conversion enums; preserve exact string values. |
| `src/models/tasks.rs` | Make `BranchName` a validated newtype or replace with `String`. |
| `docs/specs/*.allium` | Update via `allium:tend` if any domain semantics change; verify with `allium:weed`. |

## Verification

- [ ] TDD: add/adjust tests for enum round-trips and `SubStatus` column priority BEFORE refactoring
- [ ] `cargo test db::tests::migrations` — persisted string values unchanged
- [ ] `cargo test && ./scripts/check-doc-paths.sh` — all pass
- [ ] `allium:weed` reports spec/code alignment
- [ ] `cargo clippy --all-targets -- -D warnings` clean
