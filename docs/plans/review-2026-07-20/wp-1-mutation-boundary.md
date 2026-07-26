# Mutation Boundary Hardening

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Close the two un-flagged holes in the compiler-enforced service mutation boundary so the guarantee holds everywhere, not almost-everywhere.

## Context

This work package addresses findings from a code review. The codebase advertises a
compiler-enforced boundary: task/epic *writes* must go through `TaskServiceApi`/`EpicServiceApi`
(which own invariants like `recalculate_epic_status`), and non-service consumers hold
`Arc<dyn db::ReadStore>` so `state.db.patch_task(...)` is a compile error. Two seams undercut this.

## Findings

### ⚠️ `cmd_plan` bypasses the service (`src/main.rs:453`)

**Issue:** `cmd_plan` calls `database.patch_task(TaskPatch::new().plan_path(...))` directly on the
concrete `Database`, while sibling CLI handlers (`cmd_update`, `cmd_hook`, `cmd_pr_gate`) route
through `TaskService`. Harmless today (attaching a plan needs no epic recalculation) but it is an
un-flagged hole in the very boundary the codebase advertises, and it works only because it operates
on the concrete type rather than a narrowed handle.

**Fix:** Route the plan attachment through `TaskService` like the sibling handlers. If the service
lacks a plan-attach method, add one (thin wrapper over `patch_task` with the plan field). Add a test
asserting `cmd_plan` goes through the service path.

### ⚠️ `ReadStore` is a misnomer (`src/db/mod.rs:599`)

**Issue:** `ReadStore` permits settings/learning/usage *writes* (they carry no cross-entity
invariant), so the guarantee is "read tasks/epics, write everything else" — narrower than the name
implies. A reader could reasonably assume `ReadStore` is read-only and be surprised.

**Fix:** Prefer renaming `ReadStore` → `TaskReadStore` (or similar) to make the guarantee honest.
If a rename is too invasive, at minimum add a doc-comment caveat on the trait and a note in CLAUDE.md
near the mutation-boundary callout. Keep the existing `compile_fail` doctest passing.

### 💡 CLAUDE.md boundary notes

**Issue:** The CLAUDE.md mutation-boundary callout reads as absolute; the two caveats above are not
mentioned.

**Fix:** Add a short note documenting the `cmd_plan` expectation (CLI handlers route through the
service) and the `ReadStore`-writes-non-task-entities caveat.

## Changes

| File | Change |
|------|--------|
| `src/main.rs` | Route `cmd_plan` (~:453) through `TaskService` instead of `database.patch_task`. |
| `src/service/` | Add a plan-attach service method if one does not exist. |
| `src/db/mod.rs` | Rename `ReadStore` → `TaskReadStore` (or add doc-comment caveat ~:599); keep `compile_fail` doctest green. |
| `CLAUDE.md` | Add boundary caveats near the mutation-boundary callout. |

## Verification

- [ ] `cargo test && ./scripts/check-doc-paths.sh` — all pass
- [ ] New test asserts `cmd_plan` uses the service path (TDD: write it first)
- [ ] `compile_fail` doctest in `src/db/mod.rs` still rejects `patch_task` on the read handle
- [ ] `cargo clippy --all-targets -- -D warnings` clean
