# Formatting & Prompt Boilerplate

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Remove positional-argument fragility from editor formatting, make the pop-out editor orchestration testable, and de-duplicate the agent-prompt builders (fixing the research-prompt drift).

## Context

This work package addresses findings from a code review across `src/editor.rs`,
`src/runtime/editor.rs`, and `src/dispatch/prompts.rs`. Note: `editor.rs` (pure parse/format/apply)
vs `runtime/editor.rs` (tmux/tempfile/watcher orchestration) is *correct* layering — do NOT collapse
them. Prompt bodies live in `src/dispatch/prompts/` markdown files and are snapshot-locked in
`src/dispatch/prompts_snapshots.rs`.

## Findings

### 💡 `format_editor_content` uses a 10-positional `format!` (`src/editor.rs:164`)

**Issue:** A `format!` with 10 positional args where reordering silently mismaps fields, with no
compile-time protection.

**Fix:** Switch to named format arguments (`format!("{title}", title = ...)`) or build from a struct
with named fields, so a mismatched field is a compile error rather than a silent swap.

### 💡 `exec_pop_out_editor` god function (`src/runtime/editor.rs:127-236`)

**Issue:** ~110 lines (guard-check, tempfile, window naming, `$EDITOR` lookup, tmux launch, session
registration, watcher spawn) with four early-return paths each repeating the same
`app.update(SystemMessage::Error(...))` boilerplate. Only the guard path is tested.

**Fix:** Extract the pure/decidable pieces (window naming, `$EDITOR` resolution, tempfile content
prep) into testable helpers; funnel the four error early-returns through one helper. Add unit tests
for the extracted helpers.

### 💡 `finalize_task_edit` re-derives already-computed state (`src/runtime/editor.rs:297-382`)

**Issue:** Re-derives `resolved_url`/`resolved_plan_path` that `editor.rs` already computed, and
maintains two representations (DB patch + in-memory `TaskEdit`) of the same post-edit state by hand —
a leaky layer boundary.

**Fix:** Have `editor.rs` return the resolved values once and thread them through, so
`finalize_task_edit` consumes rather than re-derives. (Cross-check the pop-out-editor 6-surface
learning [#164] before touching the editor field pipeline.)

### 💡 `build_*_prompt` skeleton copy-pasted 3× (`src/dispatch/prompts.rs:210,288,321`)

**Issue:** Three builders share a copy-pasted skeleton; `build_research_prompt` silently omits the
knowledge block — easy drift.

**Fix:** Extract the shared skeleton into one function parameterised by the varying sections, so all
variants stay in sync and the research-prompt omission is either intentional-and-explicit or fixed.
Re-accept prompt snapshots only if output legitimately changes (`INSTA_UPDATE=always cargo test
dispatch::prompts_snapshots`, then clean up `.snap.new`).

## Changes

| File | Change |
|------|--------|
| `src/editor.rs` | Named format args (or struct) for `format_editor_content`; return resolved url/plan once. |
| `src/runtime/editor.rs` | Extract testable helpers from `exec_pop_out_editor`; funnel error early-returns; stop re-deriving resolved values in `finalize_task_edit`. |
| `src/dispatch/prompts.rs` | Extract shared `build_*_prompt` skeleton; resolve research-prompt knowledge-block drift. |

## Verification

- [ ] TDD: tests for the extracted editor helpers and prompt skeleton BEFORE refactoring
- [ ] `cargo test dispatch::prompts_snapshots` — pass (re-accept only if intended; delete `.snap.new`)
- [ ] `cargo test editor` and `cargo test --test lifecycle` — pass
- [ ] `cargo test && ./scripts/check-doc-paths.sh` — all pass
- [ ] `cargo clippy --all-targets -- -D warnings` clean
