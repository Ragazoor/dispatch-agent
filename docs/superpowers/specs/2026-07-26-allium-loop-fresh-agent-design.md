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

**Verified dispatch semantics**: a plain (non-`fork`) `Agent` tool call in this harness is
asynchronous — the call returns immediately with a handle, the subagent runs in the background,
and its result arrives later as a task-notification in a subsequent turn. This is not an
assumption; it's the directly observed behavior of every `Agent` dispatch made while designing
this spec. The driver must therefore treat "dispatch iteration N" and "receive iteration N's
result" as two separate turns, potentially far apart in wall-clock time, and the state file must
be durable enough to survive the driver's own context being compacted in between (see "Resuming
an orphaned loop" below) — this is not optional hardening, it is required for correctness given
the confirmed async model.

### State file

SKILL.md creates `.claude/allium-loop-state.local.md` (no relation to `ralph-loop.local.md`,
which is no longer used):

```markdown
---
active: true
runs_completed: 0
max_iterations: 6
retry_count: 0
design_doc: "docs/superpowers/specs/2026-07-26-....md"
base_branch: "main"
verify_command: "cargo test && ./scripts/check-doc-paths.sh"
iteration_start_sha: "abc1234"
started_at: "2026-07-26T12:00:00Z"
---
```

This is a plain durable record for the driver session itself (resilient to context compaction,
inspectable by the user), not read by any hook.

- `retry_count`: how many times the *current* iteration has been retried after an error (0 or 1
  — see the "Subagent errored or was skipped" case in the SKILL.md flow below). Reset to 0
  whenever an iteration completes (errored or not).
- `iteration_start_sha`: the commit the current iteration's subagent rebased onto, captured by
  the subagent itself right after its rebase step and reported back to the driver, which writes
  it here before the *next* dispatch. Used to scope "this run's" touched specs (see prompt.md
  step 2) — a merge-base diff would be wrong here because every iteration rebases and replays all
  prior iterations' commits, so a merge-base-anchored diff widens cumulatively across the whole
  loop instead of reflecting just the current run.

### Resuming an orphaned loop

Because dispatch is async and nothing external forces continuation anymore (unlike the old
Stop-hook, which mechanically re-fed the prompt regardless of what the model did), the driver can
in principle fail to dispatch the next iteration — the session is interrupted, or its context gets
compacted between receiving a result and acting on it. Left unhandled, `.claude/allium-loop-state.local.md`
would sit with `active: true` forever, and a later re-invocation of this skill would silently
clobber it, resetting the budget.

SKILL.md's kickoff step therefore checks for an existing `active: true` state file **before**
creating a new one:

- If found, read it and tell the user: "An allium-loop is already active — started at `X`,
  `runs_completed`/`max_iterations` so far, last known state `Y`. Resume it, abandon it (delete
  the state file and start fresh), or cancel?" — via AskUserQuestion.
- **Resume**: dispatch the next iteration using the existing state file's values (do not reset
  `runs_completed`).
- **Abandon**: delete the state file and proceed with normal kickoff.
- Never silently overwrite an active state file.

### SKILL.md flow

1. Check for an existing active loop first (see "Resuming an orphaned loop" above). If found,
   resolve resume/abandon/cancel with the user before doing anything else.
2. Resolve design doc / verify command / base branch — same priority order as today (explicit
   arg → session/task context → project docs → ask).
3. Resolve `max_iterations`: an explicit override if the user asked for one when invoking the
   skill (e.g. "run allium-loop with max_iterations 20"), else default `6`. No target-spec
   resolution step — removed entirely (see below).
4. Read `prompt.md` (the per-iteration template).
5. Write `.claude/allium-loop-state.local.md` as above (`retry_count: 0`, no `iteration_start_sha`
   yet — the first iteration's subagent reports its own after rebasing).
6. Tell the user the loop is active: design doc, verify command, base branch, and
   `max_iterations` (noting it can be raised by asking).
7. Dispatch iteration 1: an `Agent` tool call — a **fresh subagent, not `fork`** — with the
   filled-in `prompt.md` content as its task, substituting `{{ITERATION_NUMBER}}: 1` so the
   subagent (which has no memory of prior runs) knows definitively whether it's the first run.
8. On receiving that call's result (a later task-notification — see the verified async dispatch
   semantics above), and on every subsequent iteration's result:
   - Read the subagent's final report (`CONVERGED: yes/no`, `iteration_start_sha` it rebased onto,
     and a summary of changes made this run) and `.claude/allium-loop-state.local.md`.
   - **Subagent errored or was skipped** (crashed, no parseable `CONVERGED:` line, or a harness
     error rather than a completed report): if `retry_count == 0`, set it to 1 and re-dispatch the
     *same* iteration number unchanged; if `retry_count` was already 1, stop, delete the state
     file, and surface the failure to the user rather than retrying indefinitely or silently
     treating it as "no progress."
   - Otherwise (a real report was returned): increment `runs_completed` — exactly once, because a
     full run just genuinely finished — reset `retry_count` to 0, and store the reported
     `iteration_start_sha`.
     - **Converged** → delete the state file, report success to the user.
     - **Not converged**, budget remains, and this run made changes (per its report) → dispatch
       the next iteration with `{{ITERATION_NUMBER}}` incremented by one.
     - **Not converged** and (`runs_completed >= max_iterations`, OR this run and the previous run
       both reported no changes) → delete the state file, summarize what's unresolved, stop.

### prompt.md — the per-iteration template

Content handed to each freshly-dispatched subagent (steps renumbered, no shared state across
iterations beyond the repo itself):

```
1. Rebase: git fetch origin {{BASE_BRANCH}} && git rebase origin/{{BASE_BRANCH}}
   Then capture `ITERATION_START_SHA=$(git rev-parse HEAD)` — this anchors "this run's" diff in
   step 2 and gets reported back to the driver in step 9. If the rebase produces conflicts inside
   docs/specs/ files, resolve conservatively (preserve both sides' content where the intent isn't
   unambiguous from the diff alone, never silently drop a clause) and call out the resolution
   explicitly in the final report rather than guessing silently.

2. Advance the spec(s):
   - If `{{ITERATION_NUMBER}} == 1`, use the Agent tool with subagent_type "allium:tend", given
     the FULL design doc, and told to place/update spec content across docs/specs/ using its own
     judgment — one file, several files, or a new file, as the behavior warrants. No pre-declared
     target file.
   - Otherwise, only re-invoke tend if this run's work reveals a spec error.
   - Determine this run's touched specs: `git diff --name-only $ITERATION_START_SHA..HEAD -- docs/specs/`
     (plus working-tree changes) — anchored to this iteration's own start point, NOT a merge-base
     against {{BASE_BRANCH}}, since the latter would re-include every prior iteration's spec
     changes after each rebase and widen back toward whole-directory scope.
   - Check the `open questions` section of EACH touched spec (not the whole directory). Non-empty
     in any → STOP and resolve via AskUserQuestion before proceeding. The resolution MUST be
     written into the spec (clearing/updating the open-questions entry) and included in this
     iteration's commit (step 8) before ending — the next iteration is a fresh agent with no
     memory of this conversation, so an answer that isn't committed is lost outright, and the next
     run would face the same open question again.

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
   own rebase step doesn't hit a dirty tree. If verify (step 6) still fails and can't be resolved
   within this run, commit anyway but prefix the message `wip(allium-loop): iteration N, verify
   failing — <what's broken>` — never leave the tree dirty — and say so plainly in the final
   report; the next iteration must treat fixing it as its first priority before any new work. A
   green iteration commits normally (no `wip` prefix). These are working-history commits, expected
   to be squashed at the normal task wrap-up like any other task's commits.

9. Report: end with a clear final message containing `CONVERGED: yes` only when verify passes,
   weed reports no divergence, AND every touched spec's open-questions section is empty; otherwise
   `CONVERGED: no`. Either way, also report `ITERATION_START_SHA` (from step 1, for the driver to
   persist) and a one-line summary of what changed this run — resolving an open question and
   committing that resolution counts as a change, even if steps 3-8 didn't otherwise run.
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
  be visible via git history, and rebase needs a clean tree. A failing-verify iteration still
  commits, marked `wip(allium-loop): ...`, rather than choosing between a dirty tree and a silent
  red commit.
- **Touched-spec scope is anchored to `iteration_start_sha`, not a merge-base against the base
  branch.** Because every iteration rebases and replays all prior iterations' commits, a
  merge-base-anchored diff would cumulatively re-include earlier iterations' spec changes and
  widen the open-questions gate back toward whole-directory scope after a few runs — undoing the
  learning-#305 fix it's meant to provide.
- **Dropping the ralph-loop Stop hook also drops its one enforced guarantee**: it mechanically
  re-fed the prompt regardless of what the model did, so the loop couldn't silently die mid-flight.
  The fresh-agent design has no external equivalent, so it adds its own: an orphan check at
  kickoff (never silently clobber an active state file) and a bounded retry (`retry_count`) for a
  subagent that errors instead of completing normally.

### Accepted residual risks (not further mitigated)

- **`allium:tend` placement consistency across independent later-run invocations**: since tend
  only re-runs when a later run reveals a spec error (not every run), and always has full
  Glob/Grep visibility into the existing spec structure before deciding, the risk of it disagreeing
  with an earlier run's placement choice is judged low enough not to warrant a dedicated mechanism
  (e.g. recording placement rationale) — accepted as a minor residual risk rather than solved.
- **Per-iteration re-orientation cost**: a fresh agent re-reads the design doc and re-orients from
  scratch every run, with no shared context carried over conversationally — a real cost multiplier
  versus the old continuous-session model, accepted deliberately in exchange for unambiguous
  run-counting and the enforcement additions above.
- **`max_iterations: 6` is a starting default, not a re-derived number** for the new semantics (6
  real runs, each potentially invoking nested tend/weed subagents) — kept as-is per explicit
  product decision; tune via explicit override if a design needs more.

## Out of Scope

- `.claude/skills/allium-weed-loop/` — same underlying issues, follow-up task instead.
- Any change to the external `ralph-loop` plugin (shared infra, not owned by this repo).
- `docs/specs/*.allium` — no domain spec models this skill's own workflow.
