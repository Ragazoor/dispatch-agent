# Retro skill: fewer, higher-value follow-up tasks

**Task:** #3825
**Date:** 2026-07-31
**Status:** approved

## Problem

The `/retro` skill (`plugin/skills/retro/SKILL.md`) creates too many low-value
tasks. 53 tasks in the dispatch DB carry retro's `"Found during task #N"`
fingerprint:

| tag | done | archived | open |
|---|---|---|---|
| chore | 38 | 6 | 3 |
| bug | 2 | 1 | 2 |

Three failure modes, all traceable to the skill's wording rather than to agent
misbehaviour.

### 1. The admission test is correctness, not value

Step 2 asks: *"does anything this session built make `CLAUDE.md` or a spec now
stale or wrong?"* That is a documentation-accuracy audit. Every trivial nit
passes it, because every trivial nit is genuinely an inaccuracy. Nothing in the
skill asks whether fixing it is worth a task.

The result is 38 `chore` tasks of the shape "Add `GG_CHORD_TIMEOUT` to
CLAUDE.md's Timing Constants section", "Add `docs/module-map.md` entry for
`src/notify.rs`", "Add a real-tmux row to CLAUDE.md's 'Where new tests go'
table". Each true, each accurate, each costing a full task + worktree + agent
dispatch to add one line.

The question also never returns "no". An agent that has just spent a session
inside one subsystem will always find *some* doc nit if instructed to look for
one.

### 2. Step 1's reflection is decorative

The agent writes a "went well / could improve" list, prints it in Step 4, and
discards it. Step 2's audit ignores it entirely. The friction the agent actually
experienced never informs what gets fixed.

### 3. The no-edit rule manufactures tasks

Step 3 says *"do not modify `CLAUDE.md` or any spec file yourself here"*. So the
one agent with full context — already in a worktree, mid-commit — is forbidden
from making a 30-second correction, and must hand it to a future agent who
rebuilds that context from scratch.

### 4. Duplicates and speculative refactors

- #3696 and #3699 are the same stale "rehearsal track" wording, filed twice
  because it appeared in two files. #3609 and #3673 are the same shape twice.
- The archived cluster is almost entirely speculative refactors dressed as
  chores: #3748 "make epic creation a single atomic insert", #3749 "make the
  epic-recalculation invariant structural". Both are of the form *"the invariant
  is enforced by convention, so the same omission could recur"* — a hypothetical,
  not a defect.

## Design

### Reframe: harness improvement, not documentation audit

Steps 1 and 2 merge into a single question, asked in this order:

1. **Where did I lose time or get misled this session?** Concrete moments: an
   assumption from `CLAUDE.md` that turned out wrong, a convention discovered
   only by reading source, a spec describing behaviour the code no longer had, a
   test-placement rule guessed at, a command that failed until the right
   invocation was found.
2. **For each: is there a change to the agent-facing context that would have
   prevented it?** The surfaces that reach the next agent are `CLAUDE.md` (in
   every dispatch prompt), `docs/specs/*.allium`, `docs/*.md`,
   `plugin/skills/*`, the knowledge base, and the repo's verify command.

The admission test becomes **"would the next agent do better?"**, and the finding
must trace to a concrete moment in *this* session. A finding with no lost time
behind it is not a finding.

This inverts the current failure mode. "CLAUDE.md's 'single DB connection'
callout is wrong; I designed against it and had to back out" traces to real lost
time. "`GG_CHORD_TIMEOUT` isn't listed in the constants table" cost nobody
anything and dies at the gate.

**Zero findings is the expected outcome** on a session that ran smoothly. The
current skill permits this only in a line buried after three steps that read as
a checklist to satisfy.

### Fix in-session vs. file a task

Retro edits in-session when **all** of:

- the surface is agent-facing prose — `CLAUDE.md`, `docs/*.md`, or a skill under
  `plugin/skills/`;
- this session's own work made it wrong, or this session proved it wrong;
- the correction is small and self-evident, needing no design judgement.

Plus one Allium case: a spec edit that makes the spec describe behaviour **this
session already implemented** is documentation catching up, and is allowed.

Retro files a task instead — never edits — when:

- the change would have a spec describe behaviour the code does not have yet.
  That is a `spec → tests → code` loop and needs its own dispatch;
- the fix needs a judgement call about intended design;
- it is a pre-existing inaccuracy whose history this session does not know;
- it is not small.

### What may still be filed

Two admissible tags, down from three:

- **`bug`** — a concrete defect noticed but out of scope, with observable wrong
  behaviour. Always filable.
- **`chore`** — a context/harness improvement that passed the admission test but
  failed the edit test (too big, or needs judgement).

**`feature` leaves retro's vocabulary.** Speculative refactors and enhancement
ideas are explicitly non-findings. If one matters, it resurfaces later as a bug
with a real incident behind it.

Two further filing rules:

- **Check for an existing task before filing** — `list_tasks` before
  `create_task`.
- **One task per finding-cluster, not per file** — the same stale statement
  across three docs is one task.

### Ordering: retro moves pre-commit

Retro currently runs at wrap-up **Step 5D**, after `wrap_up`. By then the rebase
path has already fast-forwarded `base_branch` and the PR path has already pushed.
An edit made during retro would be orphaned on the branch or land after the PR
was opened — so the edit licence above cannot work where retro sits today.

Retro moves to **Step 2.6**, immediately after `simplify` (Step 2.5) and before
Step 3's commit, so its edits are committed with the session's work and flow into
the rebase or PR naturally. This mirrors the existing `simplify` precedent
exactly: `simplify` also edits pre-commit and relies on Step 3 to pick the
changes up.

```
Step 2.5  simplify        (edits code)
Step 2.6  retro           (reflect, check context, fix small drift, file the rest)
Step 3    commit          (picks up 2.5 + 2.6 edits)
Step 4    ask user: rebase / pr / done
Step 5    A summarize
          B rate learnings
          C wrap_up(action)
          D exit_session          <- retro no longer here
```

Step 5D's `/retro` invocation is removed. Reflection still happens before the
session closes, which is all the 5D placement was protecting — the constraint
recorded at `plugin/skills/wrap-up/SKILL.md:243` is against reordering retro to
*after* the close, not against moving it earlier. That line's stated reason (until
`exit_session` runs, no PR-merge polling is armed, so a merge cannot tear the
session down mid-retro) is strengthened by the move, not weakened: at Step 2.6 the
PR does not exist yet.

**Accepted consequence.** Retro now runs before Step 4, where the user may cancel
the wrap-up — but Step 3 (the commit) runs between retro and that cancel point, so
by the time the user can cancel, retro's edits are already committed and its filed
tasks are already on the board. A cancelled wrap-up therefore leaves retro's edits
committed in the worktree, just not yet merged or pushed anywhere. Both are
acceptable: the edits are correct regardless of how the session ends, and the
findings were true when filed. The alternative — placing retro after Step 4 —
would put it after the user's irreversible choice and back into the ordering trap
this move exists to escape.

## Testing

Agent-facing skill copy is tested in `mod tests` in `src/setup/plugins.rs` via
`skill_body`, with targeted `contains` checks rather than snapshots, so deleting
an instruction reads as a regression instead of an edit. Each assertion is scoped
to its heading section using the `failed_close_guidance()` pattern at
`src/setup/plugins.rs:585` — sibling sections repeat phrases, so a
whole-document `contains` can still pass after the instruction is gone.

All tests written first, per the repo's TDD rule.

| Test | Asserts |
|---|---|
| `retro_admission_test_is_next_agent_benefit_not_doc_accuracy` | the check anchors on next-agent benefit and traceable lost time, not on "stale or wrong" |
| `retro_skill_permits_fixing_small_context_drift_in_session` | the blanket no-edit ban is gone and a bounded edit licence is present |
| `retro_skill_forbids_speccing_unimplemented_behaviour` | spec edits limited to behaviour already implemented; the `spec → tests → code` case is filed, not edited |
| `retro_skill_does_not_file_feature_tasks` | `feature` absent from retro's tag vocabulary; speculative refactors named as a non-finding |
| `retro_skill_requires_duplicate_check_before_filing` | `list_tasks` before `create_task` |
| `retro_skill_states_zero_findings_is_normal` | the zero-is-expected line is present |
| `wrap_up_skill_runs_retro_before_the_commit_step` | retro is invoked pre-commit and **not** between `wrap_up` and `exit_session` |

`retro_skill_tells_agent_to_resume_the_caller` (`src/setup/plugins.rs:640`) still
holds — retro must resume its caller — but it now resumes into wrap-up's Step 3
rather than `exit_session`. Its comment and failure message need updating; the
assertion itself does not change.

## Allium specs

No new spec. Retro has no Allium spec today; it is agent-facing prose with no
runtime behaviour to model, and it stays that way — the rules are encoded in
`SKILL.md` and locked by the tests above.

Three existing specs assert the old ordering in `@guidance` and become wrong:

- `docs/specs/mcp-task-tools.allium:623` — "The /wrap-up skill runs the /retro
  skill between wrap_up and exit_session."
- `docs/specs/pr-workflow.allium:296` — "then call wrap_up(action=\"pr\") (no
  pr_url), run the /retro skill, …"
- `docs/specs/pr-workflow.allium:352,366` — same ordering claim.

`mcp-task-tools.allium:682` and `pr-workflow.allium:425` additionally justify the
removal of the old in-handler `record_learning` nudge on the grounds that
`/retro` runs *before* `exit_session`. That justification survives the move
unchanged — retro still runs before `exit_session`, just earlier — so only the
"between `wrap_up` and `exit_session`" phrasing needs correcting, not the
reasoning.

These are corrected with `allium:tend` and verified with `allium:weed`.

## Out of scope

- The 5 open retro-created tasks already on the board (#3767, #3786, #3822,
  #3823, #3829). This change affects what retro files from now on; it does not
  retro-actively triage the backlog.
- The `learnings` skill and `record_learning`. Retro already delegates
  reusable pitfalls/conventions there and that relationship is unchanged.
- Any change to `simplify`, `summarize`, or the `wrap_up`/`exit_session`
  handlers. The only wrap-up change is where `/retro` is invoked.

## Verification

```
cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh
```
