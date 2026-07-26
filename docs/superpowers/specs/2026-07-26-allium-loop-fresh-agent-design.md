# Allium-Loop: Fresh-Agent-Per-Run Design

**Date:** 2026-07-26
**Task:** #3715 — Improve allium-loop

## Background

`plugin/skills/allium-loop` drives the Allium spec-first convergence loop (Loop A) by writing a
ralph-loop state file (`.claude/ralph-loop.local.md`) that an external, unmodifiable Stop hook
(`~/.claude/plugins/marketplaces/claude-plugins-official/plugins/ralph-loop/hooks/stop-hook.sh`,
not part of this repo) uses to feed the same prompt back to the same continuous session on every
raw turn-end ("Stop" event), incrementing an `iteration` counter each time.

Two problems, both rooted in that architecture:

1. **The counter increments per raw turn-end, not per completed run.** A full pass through the
   documented "Each Iteration" sequence (rebase → tend → propagate → red-check → implement →
   verify → weed → convergence-check) can span more than one raw Stop event within the same
   session (e.g. the agent produces a shorter response and pauses partway through). Each such
   pause still consumes one of the `max_iterations: 6` budget, so the loop can exhaust its budget
   — and get killed by the hook — before even one full logical run completes.
2. **Only one target spec file is supported.** `{{TARGET_SPEC}}` is a single path. Real design
   docs frequently need changes across several of the ~15 files in `docs/specs/`; only the weed
   step already sweeps the whole directory, and even that is coincidental rather than by design.

The ralph-loop plugin is shared, external infrastructure and out of this repo's scope — the fix
cannot touch it.

## Scope

Redesign `plugin/skills/allium-loop/{SKILL.md,prompt.md}` only. `.claude/skills/allium-weed-loop/`
has the identical underlying problem (plus an already-stale hardcoded 3-file spec list) but is
explicitly **out of scope** — flagged as a follow-up task instead.

No Rust or `docs/specs/*.allium` changes: confirmed nothing outside these two skill files
references the ralph-loop mechanism, the `iteration`/`max_iterations` fields, or a target-spec
list. `src/setup/plugins.rs`'s `plugin_embeds_required_files` test only asserts the two files
exist inside the embedded plugin dir — it doesn't assert on their content.

## Core Change: Drop the Stop-Hook Loop Entirely

Instead of one continuous session whose turn-boundaries are counted by an external hook, the
skill becomes a **fresh-agent-per-run** loop, matching the standard Ralph Wiggum technique: one
iteration = one freshly-dispatched subagent that runs autonomously to completion, with no shared
conversation memory between iterations — only what's committed to the repo carries over. "Done"
is unambiguous: the `Agent` tool call returns.

The session that invokes the `allium-loop` skill becomes the loop **driver**. It never does the
rebase/tend/implement/etc. work itself — it only dispatches iterations and decides whether to
continue.

### State file

SKILL.md creates `.claude/allium-loop-state.local.md` (no relation to `ralph-loop.local.md`,
which is no longer used):

```markdown
---
active: true
runs_completed: 0
max_iterations: 6
design_doc: "docs/superpowers/specs/2026-07-26-....md"
base_branch: "main"
verify_command: "cargo test && ./scripts/check-doc-paths.sh"
started_at: "2026-07-26T12:00:00Z"
---
```

This is a plain durable record for the driver session itself (resilient to context compaction,
inspectable by the user), not read by any hook.

### SKILL.md flow

1. Resolve design doc / verify command / base branch — same priority order as today (explicit
   arg → session/task context → project docs → ask).
2. Resolve `max_iterations`: an explicit override if the user asked for one when invoking the
   skill (e.g. "run allium-loop with max_iterations 20"), else default `6`. No target-spec
   resolution step — removed entirely (see below).
3. Read `prompt.md` (the per-iteration template).
4. Write `.claude/allium-loop-state.local.md` as above.
5. Tell the user the loop is active: design doc, verify command, base branch, and
   `max_iterations` (noting it can be raised by asking).
6. Dispatch iteration 1: an `Agent` tool call — a **fresh subagent, not `fork`** — with the
   filled-in `prompt.md` content as its task, substituting `{{ITERATION_NUMBER}}: 1` so the
   subagent (which has no memory of prior runs) knows definitively whether it's the first run.
7. On receiving that call's result (this turn or a later task-notification), and on every
   subsequent iteration's result:
   - Read the subagent's final report (`CONVERGED: yes/no` + a summary of changes made this run)
     and `.claude/allium-loop-state.local.md`.
   - Increment `runs_completed` — exactly once, because a full run just genuinely finished.
   - **Converged** → delete the state file, report success to the user.
   - **Not converged**, budget remains, and this run made changes → dispatch the next iteration
     (repeat step 6/7).
   - **Not converged** and (`runs_completed >= max_iterations`, OR this run and the previous run
     both made no changes) → delete the state file, summarize what's unresolved, stop.
   - **Subagent errored or was skipped** → retry once; if it fails again, surface to the user
     rather than silently continuing to loop.
   - Otherwise, dispatch the next iteration with `{{ITERATION_NUMBER}}` incremented by one.

### prompt.md — the per-iteration template

Content handed to each freshly-dispatched subagent (steps renumbered, no shared state across
iterations beyond the repo itself):

```
1. Rebase: git fetch origin {{BASE_BRANCH}} && git rebase origin/{{BASE_BRANCH}}

2. Advance the spec(s):
   - If `{{ITERATION_NUMBER}} == 1`, use the Agent tool with subagent_type "allium:tend", given
     the FULL design doc, and told to place/update spec content across docs/specs/ using its own
     judgment — one file, several files, or a new file, as the behavior warrants. No pre-declared
     target file.
   - Otherwise, only re-invoke tend if this run's work reveals a spec error.
   - Determine this run's touched specs: `git diff --name-only <merge-base>...HEAD -- docs/specs/`
     (plus working-tree changes).
   - Check the `open questions` section of EACH touched spec (not the whole directory). Non-empty
     in any → STOP, resolve via AskUserQuestion before proceeding.

3. Propagate tests: /propagate for behavior changed in the touched specs this run.

4. Red check: run the new tests, confirm they FAIL.

5. Implement: minimum code to satisfy the spec(s) and failing tests.

6. Verify: {{VERIFY_COMMAND}}. Failure → fix the code, not the tests. A test that contradicts a
   correct implementation means the spec is likely wrong: STOP, ask the user, only then tend +
   re-propagate.

7. Weed: Agent tool, subagent_type "allium:weed", check mode, comparing all of docs/specs/
   against the implementation (unchanged — weed already sweeps the whole directory; no target
   needed).

8. Commit: stage and commit this run's changes (never `docs/plans/`) so the next iteration — a
   fresh agent with no memory of this one — can reconstruct progress via git history, and so its
   own rebase step doesn't hit a dirty tree.

9. Report: end with a clear final message: `CONVERGED: yes` only when verify passes, weed reports
   no divergence, AND every touched spec's open-questions section is empty; otherwise
   `CONVERGED: no` plus a summary of what changed this run (or "no changes" if genuinely blocked).
```

Guardrails carried over unchanged: never weaken/hand-edit generated tests, escalate ambiguity via
AskUserQuestion rather than guessing, honor spec parameters (no magic numbers), fix code not the
contract when the spec is correct, never commit `docs/plans/`, never skip rebase.

## Design Decisions

- **Multi-spec support is "stop assuming one file," not "track a list."** `allium:weed` already
  sweeps all of `docs/specs/`; `allium:tend` has full Read/Glob/Grep access and its own mandate to
  push back on ambiguity, so it's trusted to decide placement (single file, several, or new)
  without an upfront enumeration step. Each run's actual touched-file set is derived after the
  fact via `git diff`, not declared beforehand.
- **Open-questions convergence gate scopes to files touched this run**, not the whole directory —
  this also fixes learning #305 (pre-existing, unrelated open questions blocking convergence) as
  a natural side effect of deriving scope from git diff instead of a fixed target.
- **`max_iterations` default stays 6**, overridable by explicit user request — no invented
  "generous ceiling" magic number. Because iterations are now fresh-agent dispatches rather than
  raw turn-boundaries, 6 now means what it says: 6 real runs.
- **Give-up is explicit**: the driver deletes its own state file and explains what's unresolved,
  rather than relying on an external ceiling to eventually clean up.
- **Commit-per-iteration is new** (not explicit in the current prompt.md) and is required for
  this design to work at all: since iterations no longer share conversation memory, progress must
  be visible via git history, and rebase needs a clean tree.

## Out of Scope

- `.claude/skills/allium-weed-loop/` — same underlying issues, follow-up task instead.
- Any change to the external `ralph-loop` plugin (shared infra, not owned by this repo).
- `docs/specs/*.allium` — no domain spec models this skill's own workflow.
