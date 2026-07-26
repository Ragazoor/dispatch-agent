# tmux Shell-Out Boilerplate

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Collapse the ~90% repeated shell-out boilerplate in `tmux.rs` behind a single checked-run helper, and remove the hand-copied test arg-builders.

## Context

This work package addresses findings from a code review. `src/tmux.rs` is ~1294 lines, most of it
mechanical.

## Findings

### 💡 tmux.rs is ~90% mechanical boilerplate (`src/tmux.rs:10-415`)

**Issue:** ~20 public functions each repeat the identical shape:
`runner.run(...)` → `if !status.success() { bail!(...) }`, several also re-doing the
`String::from_utf8_lossy(&stderr).trim()` dance. A single helper collapses most of the file.

**Fix:** Introduce `run_checked(runner, args, context) -> anyhow::Result<Output>` (or
`-> Result<String>` for stdout-returning calls) that runs the command, checks status, and formats a
consistent error with the trimmed stderr and a caller-supplied context string. Migrate the ~20
functions to call it. Keep behaviour identical — the existing tmux tests assert on exact argv, so
they act as the safety net.

### 💡 Test-only arg-builders hand-copy production arg vectors (`src/tmux.rs:421-500`)

**Issue:** Helpers like `select_pane_args` hand-copy the production arg vectors, locking in a parallel
copy that must be kept in sync by hand.

**Fix:** Have production code build the arg vector via a shared function that the tests can call
directly (single source of truth), or assert against the args captured by `MockProcessRunner` rather
than re-deriving them. Remove the duplicated builders.

## Changes

| File | Change |
|------|--------|
| `src/tmux.rs` | Add `run_checked` helper; migrate ~20 shell-out fns; eliminate `String::from_utf8_lossy` repetition. |
| `src/tmux.rs` (tests) | Remove hand-copied arg-builders; assert on `MockProcessRunner`-captured args or a shared arg-builder. |

## Verification

- [ ] `cargo test tmux` — all existing argv assertions pass unchanged
- [ ] `cargo test && ./scripts/check-doc-paths.sh` — all pass
- [ ] `cargo clippy --all-targets -- -D warnings` clean
- [ ] Net line count of `tmux.rs` meaningfully reduced with no behaviour change
