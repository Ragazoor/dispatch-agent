# Plan: Dependabot Merge Bug — Use Squash-and-Merge by Default

## Context

The TUI's "merge PR" action uses `gh pr merge --merge`, which creates a regular merge commit. Some repos are configured to disallow merge commits (only squash or rebase allowed), causing merge failures. The fix is to switch the default merge strategy to squash-and-merge (`--squash`), which is broadly compatible and produces a cleaner history.

## Changes

Two merge paths both updated to use `--squash`:

1. `src/dispatch.rs` — `merge_pr()`: task PR merge triggered by 'P' in TUI
2. `src/runtime.rs` — `exec_merge_bot_pr()`: bot/Dependabot PR merge from Review Board

Tests updated to assert `--squash` flag is passed.
