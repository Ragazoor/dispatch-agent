# Dispatch: Remove ClaudeMode enum

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Remove the single-variant `ClaudeMode` enum and inline the permission mode string.

## Context

This work package addresses findings from a code review.

## Findings

### :bulb: `ClaudeMode` has a single variant and is bypassed by review/fix agents (`src/dispatch.rs:133-143`)

**Issue:** `ClaudeMode` is an enum with one variant (`Plan`). All `dispatch_with_prompt` call sites pass `ClaudeMode::Plan`. Meanwhile, `dispatch_review_agent` and `dispatch_fix_agent` construct `--permission-mode acceptEdits` directly at line 1103, bypassing `ClaudeMode` entirely. The abstraction suggests the permission mode is consistently parameterised when it is not.

**Fix:** Remove the `ClaudeMode` enum (lines 133-143). Remove the `mode: ClaudeMode` parameter from `dispatch_with_prompt`. Inline the `"plan"` literal where `mode.as_flag()` was called. Add a comment near the review/fix dispatch explaining why they use `acceptEdits` instead.

## Changes

| File | Change |
|------|--------|
| `src/dispatch.rs:133-143` | Delete `ClaudeMode` enum and `impl` block |
| `src/dispatch.rs` | Remove `mode: ClaudeMode` parameter from `dispatch_with_prompt` signature |
| `src/dispatch.rs` | Replace `mode.as_flag()` with `"plan"` in the format string |
| `src/dispatch.rs` | Update all call sites that pass `ClaudeMode::Plan` — remove the argument |
| `src/dispatch.rs:~1103` | Add comment: review/fix agents use `acceptEdits` because they make direct code changes |

## Verification

- [ ] Run existing tests — `cargo test dispatch::tests` — all pass (tests assert on `--permission-mode plan` which is unchanged)
- [ ] `grep "ClaudeMode" src/dispatch.rs` returns no matches
- [ ] `cargo clippy -- -D warnings` passes
