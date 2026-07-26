# Documentation Improvements

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Close the small documentation gaps the codebase review surfaced so future contributors don't mis-read coverage, conventions, or file sizes.

## Context

This work package addresses documentation findings from the 2026-06-23 codebase review. CLAUDE.md and `docs/` are already excellent; these are targeted additions, not a rewrite. Keep CLAUDE.md slim (it loads into every agent's context) — prefer adding detail to `docs/conventions.md` and keeping CLAUDE.md to one-line pointers where possible.

Note: WP-1 (dispatch agent tests) and WP-3 (LearningService trait seam) may close two of these gaps in code. If those land first, adjust the wording here to "now enforced" rather than "not enforced". Sequence this work package last, or reconcile at implementation time.

## Findings

### 💡 Untested worktree-confinement invariant is undocumented (`CLAUDE.md`, `docs/conventions.md`)

**Issue:** Worktree confinement is a stated invariant but (prior to WP-1) is not test-enforced. Contributors may assume coverage exists.

**Fix:** Add a note stating whether the invariant is test-enforced. If WP-1 has landed, point to the test; otherwise flag it as construction-only.

### 💡 `LearningService` injection asymmetry is undocumented (`docs/conventions.md`)

**Issue:** The "Service trait narrowing" section lists task/epic/todo APIs but doesn't mention that learnings lack a trait seam (or, after WP-3, that they now have one).

**Fix:** Document the current state of `LearningService` injection in the service-trait-narrowing section.

### 💡 `vec![]` command convention is implicit (`docs/architecture.md` or `docs/conventions.md`)

**Issue:** Most `commands::dispatch` arms return `vec![]`; the cascade mechanism is the exception. Readers hunt for meaning in every arm.

**Fix:** Add a one-line note that the empty-vec return is the norm and follow-on commands (the cascade) are the exception.

### 💡 Prod-vs-test LOC split is implicit (`CLAUDE.md` testing section or `docs/conventions.md`)

**Issue:** Files like `src/models/tasks.rs` (1734 LOC) are ~50% inline tests; a reader sizing files by raw LOC will over-estimate complexity.

**Fix:** Add a sentence: tests live inline behind `#[cfg(test)]` or in sibling `tests.rs`; expect roughly half of a large file's LOC to be tests.

## Changes

| File | Change |
|------|--------|
| `CLAUDE.md` | Add a one-line note on the worktree-confinement invariant's enforcement status and the prod-vs-test LOC expectation (keep slim; link to conventions if longer). |
| `docs/conventions.md` | Document the `LearningService` injection state in the service-trait-narrowing section; add the `vec![]` command-return convention note; add the prod-vs-test LOC note if not in CLAUDE.md. |
| `docs/architecture.md` | Optionally add the `vec![]` convention note near the Message→Command section if that's the better home. |

## Verification

- [ ] `./scripts/check-doc-paths.sh` passes (all doc links valid)
- [ ] Wording reflects the actual state after WP-1/WP-3 (test-enforced vs. construction-only; trait seam present vs. absent)
- [ ] CLAUDE.md remains concise — no large prose blocks added
