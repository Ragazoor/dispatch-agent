# Service: Unify field enumeration

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Eliminate duplicated field-list logic between `has_any_field()` and `updated_field_names()`.

## Context

This work package addresses findings from a code review.

## Findings

### :bulb: `has_any_field()` duplicates `updated_field_names()` field-by-field (`src/service.rs:68,84`)

**Issue:** Both `UpdateTaskParams` and `UpdateEpicParams` have `has_any_field()` and `updated_field_names()` methods that enumerate the same fields independently. When a new field is added, both methods must be updated — but only `has_any_field` is covered by the `update_task_params_has_any_field` test, so `updated_field_names` could silently miss a newly added field.

**Fix:** Replace the body of `has_any_field()` with `!self.updated_field_names().is_empty()` for both `UpdateTaskParams` (line 68) and `UpdateEpicParams` (line 671). This makes `updated_field_names()` the single source of truth.

## Changes

| File | Change |
|------|--------|
| `src/service.rs:68-82` | Replace `has_any_field()` body on `UpdateTaskParams` with `!self.updated_field_names().is_empty()` |
| `src/service.rs:671-680` | Replace `has_any_field()` body on `UpdateEpicParams` with `!self.updated_field_names().is_empty()` |

## Verification

- [ ] Run existing tests — `cargo test service::tests` — all pass
- [ ] `cargo test update_task_params_has_any_field` passes
- [ ] `cargo clippy -- -D warnings` passes
