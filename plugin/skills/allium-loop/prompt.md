# Allium Spec-First Loop — Per-Iteration Task

You are one iteration of a fresh-agent-per-run loop that drives the Allium spec-first
convergence loop (Loop A) from a design document toward converged spec, tests, and code. You
have NO memory of any previous iteration — everything you need to know about prior progress is
in the git history and the current state of the repo. This loop is language- and stack-agnostic —
do not assume any particular test runner, build tool, or toolchain beyond what this repo actually
uses.

**Input design/spec document:** `{{DESIGN_DOC}}`
**Verify command:** `{{VERIFY_COMMAND}}`
**Base branch:** `{{BASE_BRANCH}}`
**Iteration number:** `{{ITERATION_NUMBER}}`

## Your Task This Run

### 1. Rebase

```bash
git fetch origin {{BASE_BRANCH}}
git rebase origin/{{BASE_BRANCH}}
```

If the rebase produces conflicts inside `docs/specs/` files, resolve conservatively: preserve both
sides' content wherever the correct resolution isn't unambiguous from the diff alone, never
silently drop a clause, and call out exactly what you did in your final report rather than
resolving silently.

### 2. Advance the spec(s)

**First, before anything else in this step: did this prompt hand you an answer?** If the prompt you
were given includes an `**Answer to previous question:**` line, a previous iteration ended
`BLOCKED — <question>` because it could not reach the user, and the driver has now obtained the
answer for you. That answer exists nowhere but in this prompt — persist it before doing anything
else:

1. Locate the spec under `docs/specs/` whose `open questions` section contains the question the
   answer resolves. The question text was carried verbatim through the `BLOCKED —` report, so it
   is enough to find the entry (Grep for its distinctive wording).
2. Write the answer into that spec, clearing or updating the open-questions entry — exactly the
   same "resolve it in writing" treatment as the in-run case below.
3. Commit that change now, on its own — do not defer it to step 8, and do not proceed while it is
   uncommitted. Step 8's exclusions still apply (never `docs/plans/`, never the loop's state file).
   This is a spec-text-only commit, so it takes a normal message (e.g.
   `docs(specs): resolve open question — <topic>`) and needs no `wip(allium-loop):` prefix even if
   a prior run left verification red; the rest of this run's work is committed separately per
   step 8.
4. Treat that spec as **touched this run**, regardless of what the `git status --porcelain` check
   below reports. The previous iteration already committed everything it had, so the working tree
   is otherwise clean and that check will not surface this spec on its own — but the open-questions
   gate and the step-9 convergence check must both still see it.

If you cannot find the spec holding that question, do NOT guess and do NOT drop the answer: end
with `CONVERGED: no` / `SUMMARY: BLOCKED — cannot locate the spec holding the open question
"<question, verbatim>"; answer received was "<answer, verbatim>"` so the answer survives into the
next run's prompt.

Only once the carried answer is persisted and committed, continue with the normal checks below.

- If `{{ITERATION_NUMBER}}` is `1`, use the Agent tool with `subagent_type: "allium:tend"`,
  giving it the full design document at `{{DESIGN_DOC}}` and telling it to place or update spec
  content across `docs/specs/` using its own judgment — one file, several files, or a new file,
  whichever the behavior actually warrants. Do not pre-declare a target file; `allium:tend` has
  Read/Glob/Grep access to the whole directory and its own mandate to push back on ambiguity.
- On later iterations, only re-invoke `allium:tend` if this run's work reveals a spec error (a
  test or implementation detail that contradicts what the spec says).
- Determine which specs this run touched:

  ```bash
  git status --porcelain -- docs/specs/
  ```

  This step runs BEFORE this run's commit (step 8), and every iteration always commits before it
  ends — so "specs touched this run" is exactly the currently-uncommitted working-tree state under
  `docs/specs/`, **plus** any spec you resolved a carried answer into above (already committed by
  then, so invisible here). Use `git status --porcelain` rather than `git diff`: it reports both
  modified tracked specs and brand-new untracked ones that `allium:tend` may have created.
- Read the `open questions` section of EACH touched spec (not the whole directory). If any is
  non-empty, STOP and resolve it with the user via AskUserQuestion before proceeding — do not
  guess. Once resolved, write the resolution into the spec (clear or update the open-questions
  entry) and include that edit in this run's commit (step 8) before you end — the next iteration
  is a fresh agent with no memory of this conversation, so an unresolved-in-writing answer is
  lost outright and the next run will hit the same open question again.

  If you cannot get a response (e.g. no answer is available to you as a backgrounded subagent), do
  NOT guess: commit per step 8 (see "Committing on a BLOCKED exit" there), then end with
  `CONVERGED: no` / `SUMMARY: BLOCKED — <the specific question, verbatim>`. The driver session is
  interactive and will put the question to the user, then hand you the answer on the next
  iteration.
- Resolving an open question and committing that resolution is itself a complete, valid run: you
  may end here (go straight to step 8, then step 9) or continue into step 3 if there is more to do
  this run. Either is acceptable.

### 3. Propagate tests

Invoke the `/propagate` skill to generate tests for behavior that changed this run, using this
repo's own test framework and conventions. Never hand-edit generated tests.

### 4. Red check

Run the newly generated tests (using `{{VERIFY_COMMAND}}` or a narrower equivalent scoped to the
new tests) and confirm they FAIL. A new test that already passes signals redundancy or vacuity —
flag it and investigate rather than proceeding silently.

### 5. Implement

Write the minimum code needed to satisfy the spec(s) and the failing tests, following this
repo's existing language, style, and idioms. Follow the spec's rules and parameters exactly — no
magic numbers. Do NOT edit the generated tests.

### 6. Verify

```bash
{{VERIFY_COMMAND}}
```

- Verification fails → fix the CODE, not the tests.
- If a test genuinely contradicts correct implementation, the spec is likely wrong: STOP and ask
  the user via AskUserQuestion. Only then `allium:tend` the spec and re-run `/propagate`. If you
  cannot get a response (e.g. no answer is available to you as a backgrounded subagent), do NOT
  guess: commit per step 8 (see "Committing on a BLOCKED exit" there), then end with
  `CONVERGED: no` / `SUMMARY: BLOCKED — <the specific question, verbatim>`.

### 7. Weed

Use the Agent tool with `subagent_type: "allium:weed"` in check mode to compare all of
`docs/specs/` against the implementation in `src/` (weed already sweeps the whole directory — no
target file needed here). Reconcile divergence: update the spec for undocumented behavior or
spec bugs; for code bugs that contradict a correct spec, ask the user before fixing.

### 8. Commit

Stage and commit this run's changes (never `docs/plans/`, and never the loop's own state file
`.claude/allium-loop-state.local.md`). This is required, not optional: the next iteration is a
fresh agent with no memory of this one, so it can only see your progress via git history, and its
own rebase step needs a clean tree to start from.

- If `{{VERIFY_COMMAND}}` passes: commit normally, e.g.:
  ```
  feat(specs): <what changed this iteration>
  ```
- If `{{VERIFY_COMMAND}}` still fails and you cannot resolve it within this run: commit anyway —
  never leave the tree dirty — but prefix the message to mark it explicitly broken:
  ```
  wip(allium-loop): iteration {{ITERATION_NUMBER}}, verify failing — <what's broken and why>
  ```
  State this plainly in your final report too. The next iteration must treat fixing this as its
  first priority before starting any new work. These are working-history commits, expected to be
  squashed at the normal task wrap-up like any other task's commits.

**Committing on a BLOCKED exit.** Ending with `SUMMARY: BLOCKED — ...` does not exempt you from
this step. There is no "skip committing" escape hatch anywhere in this loop: you must never leave
the tree dirty, whatever the reason you are ending. Concretely:

- Do not commit something half-edited or syntactically broken. That is all "commit whatever
  partial progress is safe to commit" means — back out the incomplete edit, don't skip the commit.
- If, after backing that out, there is genuinely nothing left to commit, a clean tree with no new
  commit is fine — a BLOCKED run legitimately may produce nothing.
- If verification was left failing at the moment you blocked, the commit still uses the
  `wip(allium-loop):` prefix above, exactly as for any other red iteration.

### 9. Report

End your final message with exactly one of:

```
CONVERGED: yes
SUMMARY: <one line: what converged>
```

or

```
CONVERGED: no
SUMMARY: <one line: what changed this run, or "no changes" if this run genuinely produced none>
```

`BLOCKED — <question>` is a distinct, separately-documented `SUMMARY` form (see step 2, step 6, and
the guardrails): use it only for the specific case where you needed an answer from the user and
could not obtain one. Do not use the word "blocked" in an ordinary no-changes summary — the driver
matches the `BLOCKED —` prefix literally.

Emit `CONVERGED: yes` ONLY when ALL hold:
- `{{VERIFY_COMMAND}}` passes.
- `allium:weed` reports no spec-code divergence.
- Every spec touched this run has an empty `open questions` section.

Resolving an open question and committing that resolution counts as "changed this run," even if
steps 3-8 didn't otherwise execute.

## Guardrails (non-negotiable)

- Never skip the rebase step.
- Confirm new tests fail before implementing (spec-first red check).
- Never weaken or hand-edit generated tests.
- Escalate ambiguity, open questions, and any code-vs-test conflict by PAUSING and asking the
  user via AskUserQuestion — never guess silently. If you cannot get a response (e.g. no answer is
  available to you as a backgrounded subagent), do NOT guess: commit per step 8 (including its
  "Committing on a BLOCKED exit" rules), then end with `CONVERGED: no` / `SUMMARY: BLOCKED — <the
  specific question, verbatim>` so the interactive driver session can put the question to the user
  for you.
- If this run's prompt carries an `**Answer to previous question:**` line, persist that answer into
  the spec holding the question and commit it BEFORE any other step-2 work — it exists nowhere but
  in this prompt, and the next iteration will not receive it again.
- Honor spec parameters; no magic numbers.
- Fix code, not the contract, when the spec is correct.
- Never commit files under `docs/plans/`, and never stage or commit the loop's own state file
  `.claude/allium-loop-state.local.md`.
- Never end with a dirty tree: always commit this run's work before ending, using the
  `wip(allium-loop):` prefix if verification is not green. This holds for a `BLOCKED —` exit too —
  there is no "skip committing" escape hatch. Ending with no new commit is acceptable only when the
  tree is genuinely clean and there was nothing to commit.
- Always end with the exact two-line `CONVERGED:` / `SUMMARY:` block from step 9 — the driver
  parses it literally.
