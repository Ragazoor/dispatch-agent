# 3812 — allium-loop: dispatch iteration agents on sonnet

## Problem

The `allium-loop` skill dispatches one fresh subagent per iteration with no `model` override, so
each iteration inherits the session model (Opus). A run does up to `max_iterations` (default 6)
full iterations of rebase + tend + propagate + red-check + implement + verify + weed, so token
usage compounds.

## Finding that settles the open design question

The task description asks whether the nested `allium:tend` (prompt step 2) and `allium:weed`
(prompt step 7) calls should also be pinned to sonnet, warning that pinning the iteration agent
does not cover them.

Both agent definitions already pin Opus themselves:

- `~/.claude/plugins/cache/juxt-plugins/allium/3.8.0/agents/tend.md:4` — `model: opus`
- `~/.claude/plugins/cache/juxt-plugins/allium/3.8.0/agents/weed.md:4` — `model: opus`

An agent definition's `model` wins over inheritance from the spawning parent, so a sonnet
iteration agent still gets Opus tend and Opus weed. The judgement-heavy spec-reasoning steps keep
the stronger model for free, and the mechanical steps (rebase, `/propagate`, red check, implement,
verify, commit) drop to sonnet.

Decision: **do not** pass `model` on the nested calls. Instead document in `prompt.md` that their
model comes from their own definitions and must not be overridden to match the iteration agent —
otherwise the next reader "consistency-fixes" it and silently downgrades spec quality.

## Changes

1. `plugin/skills/allium-loop/SKILL.md` — kickoff: resolve a `model` loop parameter (default
   `sonnet`, overridable when the user asks at invocation, same shape as `max_iterations`) and
   record it in `.claude/allium-loop-state.local.md` next to `base_branch` / `verify_command` /
   `max_iterations`, so a hard convergence can opt back into Opus without editing the skill and a
   resume picks the value back up.
2. `plugin/skills/allium-loop/SKILL.md` — "Each Iteration" dispatch step: pass `model` from the
   state file, and state inline why the no-fork rule and the model override reinforce each other
   (`fork` ignores `model` entirely, so a fork would silently run on the session model).
3. `plugin/skills/allium-loop/prompt.md` — note on steps 2 and 7 that tend/weed resolve Opus from
   their own agent definitions and must not be downgraded.
4. `docs/superpowers/specs/2026-07-26-allium-loop-fresh-agent-design.md` — update the
   dispatch-semantics, kickoff, and state-file sections to match.

No `docs/specs/*.allium` change: that design doc already records (line 307) that no domain spec
models this skill's own workflow.

## Tests (first)

In `mod tests` in `src/setup/plugins.rs`, via the existing `skill_body` helper, section-scoped per
CLAUDE.md (a whole-document `contains` still passes after the instruction is deleted). Anchor on
the "Each Iteration" heading and end at the next heading of any depth, mirroring the existing
`failed_close_guidance()` helper.

- `allium_loop_dispatches_iteration_agents_on_sonnet` — the dispatch instruction names the sonnet
  model, so deleting it later reads as a regression.
- `allium_loop_dispatch_still_forbids_fork` — the no-fork constraint survives the edit; `fork`
  ignores the model override, so losing it would silently undo the saving.

## Verification

```
cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh
```
