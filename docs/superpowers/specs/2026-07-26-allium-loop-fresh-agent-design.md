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

Each dispatch also carries an explicit `model` (the state file's `model`, `sonnet` by default).
This depends on the non-`fork` requirement above rather than merely coexisting with it: a `fork`
ignores the `model` override entirely and runs on the session model, so the two rules are one
mechanism — dispatching a fresh subagent is what makes the model pin take effect at all.

### Nested spec agents keep their own model

Each iteration spawns `allium:tend` (prompt step 2) and `allium:weed` (step 7). Those agent
definitions live outside this repo and pin a strong model (Opus) in their own frontmatter, which
wins over inheriting the spawning parent's — so the loop's two judgement-heavy steps keep that
model even when the iteration agent itself runs on sonnet. The cost reduction therefore lands on
the mechanical work only, and no override is needed to protect spec quality.

`prompt.md` states this inline at both call sites, because the failure mode is a plausible-looking
edit: a reader who pins the iteration agent and then adds a matching override "for consistency"
would downgrade exactly the two steps the cheaper iteration model was chosen to leave alone.

### State file

SKILL.md creates `.claude/allium-loop-state.local.md` (no relation to `ralph-loop.local.md`,
which is no longer used):

```markdown
---
active: true
runs_completed: 0
max_iterations: 6
retry_count: 0
consecutive_no_change_runs: 0
design_doc: "docs/superpowers/specs/2026-07-26-....md"
base_branch: "main"
verify_command: "cargo test && ./scripts/check-doc-paths.sh"
model: "sonnet"
started_at: "2026-07-26T12:00:00Z"
---
```

The file itself is gitignored (`.claude/allium-loop-state.local.md`), and prompt.md's commit step
explicitly forbids staging it: it is mutated after every iteration, so a committed copy would leave
the tree dirty and break the next iteration's rebase.

This is a plain durable record for the driver session itself (resilient to context compaction,
inspectable by the user), not read by any hook.

- `retry_count`: how many times the *current* iteration has been retried after an error (0 or 1
  — see the "Subagent errored or was skipped" case in the SKILL.md flow below). Reset to 0 when a
  real (parseable) report is returned — not on every error, since the error path is exactly what
  the counter exists to bound.
- `consecutive_no_change_runs`: how many completed runs in a row reported no changes. Incremented
  when a run's `SUMMARY` reports no changes, reset to 0 otherwise, and consulted by the
  give-up condition (`>= 2`). It exists because that stop condition must be evaluable from the
  state file alone — the driver cannot rely on remembering the *previous* run's summary across a
  context compaction.
- `model`: the model each iteration agent is dispatched on, defaulting to `sonnet`. It is a
  recorded loop parameter rather than a constant in the skill for two reasons: the driver must read
  it back after a compaction (same reason as the counters above), and a hard convergence can then
  opt back into a stronger model without editing the skill. Sonnet is the default because the cost
  multiplies by `max_iterations` while the iteration agent's own work — rebase, `/propagate`, red
  check, implement, verify, commit — is mechanical. The judgement-heavy spec steps are unaffected;
  see "Nested spec agents keep their own model" below.
- The current iteration number is not stored; it is always derived as `runs_completed + 1`, which
  stays correct across a resume or a compaction.

### Resuming an orphaned loop

Because dispatch is async and nothing external forces continuation anymore (unlike the old
Stop-hook, which mechanically re-fed the prompt regardless of what the model did), the driver can
in principle fail to dispatch the next iteration — the session is interrupted, or its context gets
compacted between receiving a result and acting on it. Left unhandled, `.claude/allium-loop-state.local.md`
would sit with `active: true` forever, and a later re-invocation of this skill would silently
clobber it, resetting the budget.

SKILL.md's kickoff step therefore checks for an existing `active: true` state file **before**
creating a new one:

- If found, read it and tell the user a loop is already active, including its current progress
  (`started_at`, `runs_completed`/`max_iterations`), then ask via AskUserQuestion whether to
  resume, abandon (delete the state file and start fresh), or cancel.
- **Resume**: dispatch the next iteration using the existing state file's values (do not reset
  `runs_completed`, `retry_count`, or `consecutive_no_change_runs` — all three bound the loop and
  resetting any of them silently re-grants budget).
- **Abandon**: delete the state file and proceed with normal kickoff.
- **Cancel**: do nothing further.
- Never silently overwrite an active state file.

### SKILL.md flow

1. Check for an existing active loop first (see "Resuming an orphaned loop" above). If found,
   resolve resume/abandon/cancel with the user before doing anything else.
2. Resolve design doc / verify command / base branch — same priority order as today (explicit
   arg → session/task context → project docs → ask).
3. Resolve `max_iterations`: an explicit override if the user asked for one when invoking the
   skill (e.g. "run allium-loop with max_iterations 20"), else default `6`. No target-spec
   resolution step — removed entirely (see below).
4. Resolve `model`: an explicit override if the user asked for one when invoking the skill (e.g.
   "run allium-loop on opus"), else default `sonnet`.
5. Read `prompt.md` (the per-iteration template).
6. Write `.claude/allium-loop-state.local.md` as above (`retry_count: 0`,
   `consecutive_no_change_runs: 0`).
7. Tell the user the loop is active: design doc, verify command, base branch, `model`, and
   `max_iterations` (noting the last two can be changed by asking).
8. Dispatch iteration 1: an `Agent` tool call — a **fresh subagent, not `fork`** — with the
   filled-in `prompt.md` content as its task, `model` set to the state file's value, substituting
   `{{ITERATION_NUMBER}}: 1` so the subagent (which has no memory of prior runs) knows definitively
   whether it's the first run. Subsequent dispatches derive the number as `runs_completed + 1` and
   re-read `model` from the state file for the same reason.
9. On receiving that call's result (a later task-notification — see the verified async dispatch
   semantics above), and on every subsequent iteration's result:
   - Read the subagent's two-line final report (`CONVERGED: yes/no` and a one-line `SUMMARY` of
     changes made this run) and `.claude/allium-loop-state.local.md`.
   - **Subagent errored or was skipped** (crashed, a harness error rather than a completed report,
     or a report missing EITHER required label — a partial/malformed report counts as an error, not
     a result): if `retry_count == 0`, set it to 1 and re-dispatch the *same* iteration number
     unchanged; if `retry_count` was already 1, stop, delete the state file, and surface the failure
     to the user rather than retrying indefinitely or silently treating it as "no progress."
   - Otherwise (a real report was returned): increment `runs_completed` — exactly once, because a
     full run just genuinely finished — reset `retry_count` to 0, and update
     `consecutive_no_change_runs` (increment if the summary reports no changes, else reset to 0).
     - **Converged** → delete the state file, report success to the user.
     - **Not converged** and (`runs_completed >= max_iterations` OR
       `consecutive_no_change_runs >= 2`) → delete the state file, summarize what's unresolved,
       stop.
     - **Not converged** and budget remains (`runs_completed < max_iterations` AND
       `consecutive_no_change_runs < 2`) → dispatch the next iteration. The iteration number
       advances on its own, since `runs_completed` was just incremented and the number is always
       derived as `runs_completed + 1`.

### prompt.md — the per-iteration template

Content handed to each freshly-dispatched subagent (steps renumbered, no shared state across
iterations beyond the repo itself):

```
1. Rebase: git fetch origin {{BASE_BRANCH}} && git rebase origin/{{BASE_BRANCH}}
   If the rebase produces conflicts inside docs/specs/ files, resolve conservatively (preserve both
   sides' content where the intent isn't unambiguous from the diff alone, never silently drop a
   clause) and call out the resolution explicitly in the final report rather than guessing silently.

2. Advance the spec(s):
   - If `{{ITERATION_NUMBER}} == 1`, use the Agent tool with subagent_type "allium:tend", given
     the FULL design doc, and told to place/update spec content across docs/specs/ using its own
     judgment — one file, several files, or a new file, as the behavior warrants. No pre-declared
     target file.
   - Otherwise, only re-invoke tend if this run's work reveals a spec error.
   - No `model` override on tend (nor on weed in step 7) — both pin their own; see "Nested spec
     agents keep their own model" above.
   - Determine this run's touched specs: `git status --porcelain -- docs/specs/`. No history diff is
     involved: this step runs BEFORE the run's own commit (step 8) and every iteration always
     commits before ending, so "touched this run" is exactly the currently-uncommitted working-tree
     state. `git status --porcelain` rather than `git diff` so that new *untracked* spec files
     `allium:tend` may have created are counted too.
   - Check the `open questions` section of EACH touched spec (not the whole directory). Non-empty
     in any → STOP and resolve via AskUserQuestion before proceeding. The resolution MUST be
     written into the spec (clearing/updating the open-questions entry) and included in this
     iteration's commit (step 8) before ending — the next iteration is a fresh agent with no
     memory of this conversation, so an answer that isn't committed is lost outright, and the next
     run would face the same open question again.
   - Resolving and committing an open question's answer is itself a complete, valid run: the
     iteration may end there (step 8, then 9) or continue into step 3. Either is acceptable.

3. Propagate tests: /propagate for behavior changed in the touched specs this run.

4. Red check: run the new tests, confirm they FAIL.

5. Implement: minimum code to satisfy the spec(s) and failing tests.

6. Verify: {{VERIFY_COMMAND}}. Failure → fix the code, not the tests. A test that contradicts a
   correct implementation means the spec is likely wrong: STOP, ask the user, only then tend +
   re-propagate.

7. Weed: Agent tool, subagent_type "allium:weed", check mode, comparing all of docs/specs/
   against the implementation (unchanged — weed already sweeps the whole directory; no target
   needed).

8. Commit: stage and commit this run's changes (never `docs/plans/`, and never the loop's own
   `.claude/allium-loop-state.local.md`) so the next iteration — a fresh agent with no memory of
   this one — can reconstruct progress via git history, and so its own rebase step doesn't hit a
   dirty tree. If verify (step 6) still fails and can't be resolved
   within this run, commit anyway but prefix the message `wip(allium-loop): iteration N, verify
   failing — <what's broken>` — never leave the tree dirty — and say so plainly in the final
   report; the next iteration must treat fixing it as its first priority before any new work. A
   green iteration commits normally (no `wip` prefix). These are working-history commits, expected
   to be squashed at the normal task wrap-up like any other task's commits. There is no "skip
   committing" escape hatch anywhere in the design; ending with no new commit is acceptable only
   when the tree is genuinely clean and there was nothing to commit.

9. Report: end with exactly two labelled lines — `CONVERGED: yes|no` and a one-line `SUMMARY`
   (`<what changed this run, or "no changes" if this run genuinely produced none>`).
   `CONVERGED: yes` only when verify passes, weed reports no divergence, AND every touched spec's
   open-questions section is empty; otherwise `CONVERGED: no`. Resolving an open question and
   committing that resolution counts as a change for the summary, even if steps 3-8 didn't
   otherwise run. The driver parses both labels literally, and a report missing either one is
   treated as an error rather than a result.
```

Guardrails carried over unchanged: never weaken/hand-edit generated tests, escalate ambiguity via
AskUserQuestion rather than guessing, honor spec parameters (no magic numbers), fix code not the
contract when the spec is correct, never commit `docs/plans/`, never skip rebase.

**Escalation happens in-run, with no relay through the driver.** An iteration that hits an open
question or ambiguity calls AskUserQuestion itself, writes the answer into the spec (or wherever it
belongs), commits that resolution, and continues — all within the same run. Nothing is handed back
to the driver for it.

An earlier revision of this design added a defensive fallback for the hypothetical case of a
backgrounded subagent being unable to reach a human: the iteration would report
`SUMMARY: BLOCKED — <question>`, the driver would ask the user itself, and the answer would be
carried into the next iteration's prompt as an `**Answer to previous question:**` line. That
fallback was **removed as unwarranted speculative complexity** — nothing ever established that a
backgrounded subagent's AskUserQuestion actually fails to reach a human, and defending the
hypothesis produced three rounds of increasingly narrow bugs (lost answers, wrong-origin
assumptions, broken carry-forward chains) for no observed benefit. If background AskUserQuestion
unreachability turns out to be real, it surfaces as an observably stuck or hanging loop in normal
use, which is far more tractable to diagnose and fix with actual evidence than to engineer around
speculatively.

## Design Decisions

- **Multi-spec support is "stop assuming one file," not "track a list."** `allium:weed` already
  sweeps all of `docs/specs/`; `allium:tend` has full Read/Glob/Grep access and its own mandate to
  push back on ambiguity, so it's trusted to decide placement (single file, several, or new)
  without an upfront enumeration step. Each run's actual touched-file set is derived after the
  fact from git (see the touched-spec-scope decision below), not declared beforehand.
- **Open-questions convergence gate scopes to files touched this run**, not the whole directory —
  this also fixes learning #305 (pre-existing, unrelated open questions blocking convergence) as
  a natural side effect of deriving scope from git state instead of a fixed target.
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
- **Touched-spec scope needs no history diff at all — it is the uncommitted working tree.** The
  open-questions gate (prompt.md step 2) runs *before* the iteration's own commit (step 8), and
  every iteration commits before it ends, so nothing from a prior run is ever left uncommitted:
  "specs touched this run" is exactly `git status --porcelain -- docs/specs/`. An earlier draft
  anchored this to a start-of-iteration commit SHA captured right after the rebase, on the theory
  that a merge-base diff would cumulatively re-include earlier iterations' spec changes. That
  mechanism was both unnecessary (there is no prior-iteration history to exclude) and broken in
  practice: at step 2, `HEAD` still equals that captured commit, so the diff was guaranteed empty —
  which would have made the gate conclude "no specs touched" and skip the loop's single most
  important safety check. It also never survived the trip: the capture was a bare shell assignment
  producing no stdout, and shell variables do not persist across separate Bash tool calls. Plain
  `git status` also catches brand-new *untracked* spec files that `allium:tend` may create, which
  `git diff` would miss. Nothing is reported back to the driver for this, so the report is exactly
  two lines (`CONVERGED:` / `SUMMARY:`).
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
