# Dispatch Agent Test Coverage

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add behavioural test coverage for the agent-spawn orchestration in `src/dispatch/`, the one genuinely under-tested core path, and assert the worktree-confinement invariant.

## Context

This work package addresses findings from the 2026-06-23 codebase review. `src/dispatch/agents.rs` had only 2 tests (covering `create_main_session`) and `src/dispatch/mod.rs` had none, despite this being the core product behaviour: spawning Claude Code agents into isolated git worktrees via tmux. The documented worktree-confinement invariant (every prompt instructs the agent to stay in its worktree, enforced in `dispatch_with_prompt()`) is not asserted by any test.

Tests must follow the repo conventions: use `MockProcessRunner` (never shell out), assert observable behaviour, and use the async-completion patterns (no `tokio::time::sleep`). Always use TDD — write the failing test first, then confirm the existing production code satisfies it (or fix the code if a real gap surfaces).

## Findings

### ⚠️ Agent-spawn orchestration is largely untested (`src/dispatch/agents.rs`, `src/dispatch/mod.rs`)

**Issue:** `dispatch_with_prompt`, `dispatch_agent`, `research_agent`, `quick_dispatch_agent`, and `resume_agent` have no direct tests. These functions build the tmux invocation and prompt that launch every agent — the thinnest coverage relative to importance in the codebase.

**Fix:** Add `MockProcessRunner`-based unit tests asserting the recorded process calls (argv) for each dispatch variant: correct worktree path, tmux window naming, and Claude Code invocation. Use `recorded_calls()` to assert exact arguments, mirroring the existing pattern in `src/git.rs` tests.

### ⚠️ Worktree-confinement invariant is not test-enforced (`src/dispatch/agents.rs` — `dispatch_with_prompt()`)

**Issue:** Every agent prompt includes an instruction to stay in the worktree and not `cd` to the parent repo (per CLAUDE.md and the Agent Working Directory section). This invariant is enforced only by construction, not by any test, so a refactor could silently drop it.

**Fix:** Add a test that builds a dispatch prompt and asserts the worktree-confinement instruction is present in the rendered prompt / spawn command. Prefer asserting against the prompt body produced for the agent (consistent with the existing prompt snapshots in `src/dispatch/snapshots/`).

## Changes

| File | Change |
|------|--------|
| `src/dispatch/agents.rs` | Add `#[cfg(test)] mod tests` (or extend existing) with `MockProcessRunner`-based tests for `dispatch_with_prompt`, `dispatch_agent`, `research_agent`, `quick_dispatch_agent`, `resume_agent`, asserting recorded argv. Add a test asserting the worktree-confinement instruction is present. |
| `src/dispatch/mod.rs` | Add tests for the public dispatch orchestration entry points that lack coverage (worktree creation → tmux session → agent launch wiring), using `MockProcessRunner`. |
| `src/dispatch/tests.rs` | If shared dispatch test helpers exist here, extend them rather than duplicating fixtures. |

## Verification

- [ ] `cargo test dispatch::` — new tests pass
- [ ] `cargo test` — full suite green
- [ ] `./scripts/check-doc-paths.sh` passes
- [ ] `./scripts/check-no-test-sleep.sh` passes (no `tokio::time::sleep` introduced)
- [ ] Confirm at least one test fails if the worktree-confinement instruction is removed from the prompt (red-then-green)
