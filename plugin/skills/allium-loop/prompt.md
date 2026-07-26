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
  `docs/specs/`. Use `git status --porcelain` rather than `git diff`: it reports both modified
  tracked specs and brand-new untracked ones that `allium:tend` may have created.
- Read the `open questions` section of EACH touched spec (not the whole directory). If any is
  non-empty, STOP and resolve it with the user via AskUserQuestion before proceeding — do not
  guess. Once resolved, write the resolution into the spec (clear or update the open-questions
  entry) and include that edit in this run's commit (step 8) before you end — the next iteration
  is a fresh agent with no memory of this conversation, so an unresolved-in-writing answer is
  lost outright and the next run will hit the same open question again.

  If you cannot get a response (e.g. no answer is available to you as a backgrounded subagent), do
  NOT guess: commit whatever partial progress is safe to commit, and end with `CONVERGED: no` /
  `SUMMARY: BLOCKED — <the specific question, verbatim>`. The driver session is interactive and
  will put the question to the user, then hand you the answer on the next iteration.
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
  guess: commit whatever partial progress is safe to commit, and end with `CONVERGED: no` /
  `SUMMARY: BLOCKED — <the specific question, verbatim>`.

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

### 9. Report

End your final message with exactly one of:

```
CONVERGED: yes
SUMMARY: <one line: what converged>
```

or

```
CONVERGED: no
SUMMARY: <one line: what changed this run, or "no changes" if genuinely blocked>
```

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
  available to you as a backgrounded subagent), do NOT guess: commit whatever partial progress is
  safe to commit, and end with `CONVERGED: no` / `SUMMARY: BLOCKED — <the specific question,
  verbatim>` so the interactive driver session can put the question to the user for you.
- Honor spec parameters; no magic numbers.
- Fix code, not the contract, when the spec is correct.
- Never commit files under `docs/plans/`, and never stage or commit the loop's own state file
  `.claude/allium-loop-state.local.md`.
- Always commit before ending, using the `wip(allium-loop):` prefix if verification is not green.
- Always end with the exact two-line `CONVERGED:` / `SUMMARY:` block from step 9 — the driver
  parses it literally.
