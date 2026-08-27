# 4366 — Replace `/brainstorming` with an Allium-first `allium:elicit` workflow

## Goal

`/brainstorming` (the superpowers design-doc-then-plan skill) is currently the
named design step in two dispatch prompt addenda. Replace it with a spec-first
sequence: interview with `allium:elicit`, capture the agreed behaviour in
`docs/specs/*.allium`, propagate tests from the spec, implement to green, weed.

## Decisions (settled with the user, 2026-08-27)

1. **The Allium spec is the design artefact.** A `docs/plans/` doc is optional —
   the agent writes and attaches one only when the implementation is large
   enough that recording the steps is worth it. It is not required and not
   enforced by the prompt.
2. **Interview skill: `allium:elicit`.** Not `/grill`, not both.
3. **`/allium-loop` is named as an optional convergence path**, at the agent's
   judgement — for large or stubborn work, hand the propagate/implement/weed
   iterations to the loop instead of doing them inline.
4. **Two prompt surfaces change**: the no-plan dispatch addendum
   (`plan_or_brainstorm_instruction`) and the quick-dispatch addendum
   (`plan_and_attach_instruction`). The with-plan review-and-confirm addendum is
   unchanged.
5. **`.claude/skills/brainstorm-features/` stays.** It generates feature ideas
   for the TUI backlog — a different job from the interview/design workflow.

## Steps

### 1. Spec first

- `docs/specs/dispatch.allium` — rewrite the prompt-skeleton guidance for the
  `has_plan = false` variant: the addendum is now `spec_first_instruction`
  (unconditional, no vague/clear branch). Keep the epic-decomposition carve-out
  and the "not the end of the task" framing. Update the `plan_or_brainstorm`
  references in the Dependabot/PrReview exception notes, the `CreateQuickTask`
  note, and `RetryFresh`'s guidance.
- `docs/specs/mcp-task-tools.allium` — `DispatchTaskViaMcp` guidance calls the
  no-plan variant the "brainstorm-or-plan prompt"; rename to "spec-first
  prompt".

### 2. Tests (red)

`src/dispatch/prompts.rs` tests:

- `spec_first_instruction_names_the_elicit_spec_test_implement_sequence` —
  asserts `allium:elicit`, `docs/specs/`, `allium:propagate`, `allium:weed`
  appear in order-independent fashion.
- `spec_first_instruction_makes_the_plan_doc_optional` — asserts the wording
  says a plan is not required (`only if`/`not required`), and still names
  `docs/plans/` + `update_task` for when it is written.
- `spec_first_instruction_offers_allium_loop_as_a_judgement_call` — asserts
  `/allium-loop` is named.
- `spec_first_instruction_frames_the_spec_as_an_intermediate_step` — replaces
  `plan_or_brainstorm_instruction_frames_plan_as_intermediate_step`.
- `no_prompt_variant_mentions_brainstorming` — sweeps every `build_*_prompt`
  variant (dispatch no-plan/with-plan, quick dispatch, research, dependabot,
  pr-review) and asserts none contains `/brainstorming`. This is the
  regression guard the task exists for.
- Update `no_plan_addendum_instructs_implementation_for_every_working_tag` and
  `wrap_up_instruction_no_longer_treats_plan_attach_as_sufficient` to the new
  wording.

`src/dispatch/tests.rs`:

- Replace `plan_and_attach_instruction_mentions_docs_plans_and_update_task`,
  `quick_dispatch_uses_unconditional_plan_and_attach_instruction` and
  `plan_and_attach_instruction_is_concise` with equivalents against
  `spec_first_instruction`.
- Update `build_quick_dispatch_prompt_includes_planning_instruction` to assert
  the spec-first sequence rather than a plan.

### 3. Implement

`src/dispatch/prompts.rs`:

- `plan_or_brainstorm_instruction()` → `spec_first_instruction()`, rewritten to
  the five-step sequence with the optional-plan and optional-`/allium-loop`
  clauses.
- Delete `plan_and_attach_instruction()`; quick dispatch embeds
  `spec_first_instruction()` and its surrounding sentence changes from "write a
  focused plan" to "design it spec-first".
- `wrap_up_instruction()` — generalise "Writing or attaching a plan" to cover a
  spec too, so a spec-only agent does not read the spec as a stopping point.

### 4. Snapshots

`cargo insta` / `INSTA_UPDATE` refresh of
`snapshot_dispatch_prompt_no_plan`, `snapshot_dispatch_prompt_with_epic`, and
`snapshot_quick_dispatch_prompt`. Review each diff by hand — the whole point of
this task is the prompt copy, so a blanket accept would hide a mistake.

### 5. `/allium-loop` input resolution

`plugin/skills/allium-loop/SKILL.md` kickoff step 2 resolves the design doc from
`docs/superpowers/specs/` or `docs/plans/`. The new flow makes a
`docs/specs/*.allium` file the natural input, so add it to that list.

### 6. The TUI's `[Space]` action-hint label

Found mid-task, and confirmed in scope with the user: the kanban action-hint bar
labelled the Space key `brainstorm` for a backlog task with no plan and
`dispatch` for one with a plan. The label named the retired design skill, and it
made a prompt-internal detail into a second user-facing verb for one action.

The user's call: **it always says `dispatch`** — that is the name for starting a
task. Drop the conditional in `src/tui/ui/kanban/mod.rs` entirely, update
`action_hints_backlog_task` in `src/tui/tests/rendering.rs` (assert `dispatch`,
and guard against `brainstorm` coming back), and refresh the eleven hint-bar
snapshots. Record the decision as a `@guarantee` on `DispatchTask` in
`docs/specs/dispatch.allium`, since the hint bar is an interaction surface.

### 7. Verify

`cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`, plus
`./scripts/check-doc-symbols.sh` (the pre-push hook runs it and the spec cites
the renamed symbols).

## Out of scope

- The with-plan addendum's review-and-confirm step.
- `.claude/skills/brainstorm-features/`.
- The superpowers plugin itself (external; its `using-superpowers` skill still
  points at `brainstorming`, which this repo cannot change).
- Archived `docs/plans/` and `docs/superpowers/` docs that mention brainstorming
  as history. Neither doc checker scans them.
