# Smaller Prompts

**Goal:** Make all planning-phase agent prompts design-first by pointing them at `/brainstorming` instead of telling them to write a plan directly.

## What changed

`plan_and_attach_instruction()` in `src/dispatch.rs` is shared by all four planning-phase prompts (brainstorm, plan, standard-no-plan, quick-dispatch). It previously said:

> "Your goal is to explore the codebase and write a focused implementation plan. Use /plan mode for a structured planning session…"

It now says:

> "Use /brainstorming to design the solution, then save the plan to docs/plans/ and call update_task to attach it."

`build_brainstorm_prompt()` additionally drops the TDD instruction and "clarifying questions" opener — both are handled by the `/brainstorming` skill itself.

Tasks with an existing plan (the `Some(path)` branch in `build_prompt`) are not affected — they are already past the design phase.

## Implementation

All changes are in `src/dispatch.rs`:
- `plan_and_attach_instruction()` — replaced with one-liner referencing `/brainstorming`
- `build_brainstorm_prompt()` — removed `{tdd}` and clarifying-questions opener
- Tests updated to match new behaviour; four new tests added (TDD-first)
