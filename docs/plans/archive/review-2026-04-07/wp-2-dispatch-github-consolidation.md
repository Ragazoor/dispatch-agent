# Dispatch & GitHub Consolidation

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Eliminate duplicated review-state parsing between github.rs and dispatch.rs, and consolidate the nearly-identical `dispatch_review_agent()` / `dispatch_fix_agent()` functions.

## Context

This work package addresses findings from a code review. The dispatch and GitHub modules have independently evolved similar parsing logic and dispatch patterns that should be unified.

## Findings

### :warning: Duplicated review-state parsing (`src/github.rs:93`, `src/dispatch.rs:813`)

**Issue:** Both files parse GitHub review decision strings (`"APPROVED"`, `"CHANGES_REQUESTED"`, `"REVIEW_REQUIRED"`) but handle different subsets of values. `github.rs` handles `APPROVED`, `CHANGES_REQUESTED`, and a wildcard. `dispatch.rs` additionally handles `REVIEW_REQUIRED`. This divergence means one file may miss new states that the other handles.

**Fix:** Create a single `ReviewDecision` enum (or parsing function) in `models.rs` or `github.rs` that both call sites use. Ensure the complete set of GitHub review states is handled in one place.

### :bulb: Consolidate `dispatch_review_agent` / `dispatch_fix_agent` (`src/dispatch.rs:858,1009`)

**Issue:** These two functions are nearly identical — both take 7-8 parameters, set up a worktree, build a prompt, and dispatch an agent. They differ only in the prompt content and a few context fields. ~180 lines of code with significant overlap.

**Fix:** Extract common worktree-setup and dispatch logic into a shared helper (e.g., `provision_and_dispatch(config: DispatchConfig)`). Parameterize the prompt builder so each variant only specifies what's unique. Consider a `DispatchConfig` struct to replace the long parameter lists.

## Changes

| File | Change |
|------|--------|
| `src/github.rs` | Extract review-state parsing into reusable function or enum |
| `src/dispatch.rs` | Use shared review-state parser, extract common dispatch logic from `dispatch_review_agent`/`dispatch_fix_agent` into shared helper |
| `src/models.rs` | Add `ReviewDecision` enum if placing it in domain types |

## Verification

- [ ] Run existing tests — all pass
- [ ] Review-state parsing handles `APPROVED`, `CHANGES_REQUESTED`, `REVIEW_REQUIRED`, and unknown values consistently
- [ ] Both dispatch paths still produce correct prompts (check tmux output in manual test)
- [ ] `cargo clippy -- -D warnings` passes
