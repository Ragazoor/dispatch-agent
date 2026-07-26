# Allium-Loop Fresh-Agent-Per-Run Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `plugin/skills/allium-loop`'s ralph-loop-stop-hook mechanism (whose raw
per-turn counter over-counts iterations and only supports one target spec file) with a
fresh-agent-per-run driver loop that counts real completed runs and lets `allium:tend`/`allium:weed`
operate across all of `docs/specs/`.

**Architecture:** The session that invokes the skill becomes a loop *driver* that never does the
actual spec/test/code work itself. Each iteration is one fresh `Agent` tool dispatch (never
`fork`) running the full rebase→tend→propagate→red-check→implement→verify→weed→commit→report
sequence in `prompt.md`, with no memory of prior iterations — only git history carries progress
forward. The driver reads each iteration's `CONVERGED: yes/no` report and a small
`.claude/allium-loop-state.local.md` state file to decide whether to stop or dispatch again.

**Tech Stack:** Markdown skill files (Claude Code plugin skill format), no Rust/application code
changes. Full design: `docs/superpowers/specs/2026-07-26-allium-loop-fresh-agent-design.md`.

## Global Constraints

- `max_iterations` defaults to `6`; overridable only via an explicit user request when invoking
  the skill (no invented "generous ceiling" value).
- Never commit files under `docs/plans/` (existing repo-wide rule, unchanged).
- Never skip the rebase step in any iteration.
- Never pass `subagent_type: "fork"` when dispatching an iteration — each iteration must have no
  inherited conversation memory.
- The state file lives at `.claude/allium-loop-state.local.md` and has no relation to (and does
  not read/write) `.claude/ralph-loop.local.md` — that mechanism is dropped entirely for this
  skill.
- `.claude/skills/allium-weed-loop/` is explicitly out of scope for this change.
- No Rust or `docs/specs/*.allium` file changes are required — this is a skill-content-only
  change. `src/setup/plugins.rs::plugin_embeds_required_files` only asserts that
  `skills/allium-loop/SKILL.md` and `skills/allium-loop/prompt.md` exist in the embedded plugin
  dir; it does not assert on content, so it is unaffected by a content rewrite.

---

### Task 1: Rewrite `prompt.md` (the per-iteration subagent template)

**Files:**
- Modify: `plugin/skills/allium-loop/prompt.md` (full rewrite)

**Interfaces:**
- Consumes: placeholders substituted by the driver before each dispatch —
  `{{DESIGN_DOC}}`, `{{VERIFY_COMMAND}}`, `{{BASE_BRANCH}}`, `{{ITERATION_NUMBER}}` (all plain
  string substitution, no code).
- Produces: the exact final-report format the driver (Task 2) must parse literally:
  ```
  CONVERGED: yes|no
  ITERATION_START_SHA: <sha>
  SUMMARY: <one line>
  ```
  Task 2's dispatch-decision logic depends on this exact three-line shape (label, colon, space,
  value) appearing as the final message content from each iteration's subagent.

There is no automated test harness for this file's content — it is a prompt consumed by an LLM
at runtime, not code a compiler or test runner checks. Verification for this task is: the content
faithfully implements every step of the design doc's "prompt.md — the per-iteration template"
section, and Task 3's `cargo test` run confirms no regression in the one existing test that
touches this file (`plugin_embeds_required_files`, which only checks the file still exists).

- [ ] **Step 1: Read the current file for reference**

Read `plugin/skills/allium-loop/prompt.md` to see the exact current content being replaced (used
only to confirm nothing outside this plan's scope is silently dropped — e.g. the guardrails list
and exit-conditions intent should carry forward, reworded for the new architecture).

- [ ] **Step 2: Write the new file**

Replace the entire contents of `plugin/skills/allium-loop/prompt.md` with:

```markdown
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

Then capture your starting point:

```bash
ITERATION_START_SHA=$(git rev-parse HEAD)
```

Report this value at the end (step 9) — the driver uses it to anchor the *next* iteration's
"what changed this run" diff. If the rebase produces conflicts inside `docs/specs/` files,
resolve conservatively: preserve both sides' content wherever the correct resolution isn't
unambiguous from the diff alone, never silently drop a clause, and call out exactly what you did
in your final report rather than resolving silently.

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
  git diff --name-only "$ITERATION_START_SHA"..HEAD -- docs/specs/
  ```

  (plus any currently-uncommitted working-tree changes under `docs/specs/`). Anchor to
  `$ITERATION_START_SHA`, not a merge-base against `{{BASE_BRANCH}}` — the latter would
  re-include every prior iteration's spec changes after each rebase and widen this run's scope
  back toward the whole directory.
- Read the `open questions` section of EACH touched spec (not the whole directory). If any is
  non-empty, STOP and resolve it with the user via AskUserQuestion before proceeding — do not
  guess. Once resolved, write the resolution into the spec (clear or update the open-questions
  entry) and include that edit in this run's commit (step 8) before you end — the next iteration
  is a fresh agent with no memory of this conversation, so an unresolved-in-writing answer is
  lost outright and the next run will hit the same open question again.

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
  the user via AskUserQuestion. Only then `allium:tend` the spec and re-run `/propagate`.

### 7. Weed

Use the Agent tool with `subagent_type: "allium:weed"` in check mode to compare all of
`docs/specs/` against the implementation in `src/` (weed already sweeps the whole directory — no
target file needed here). Reconcile divergence: update the spec for undocumented behavior or
spec bugs; for code bugs that contradict a correct spec, ask the user before fixing.

### 8. Commit

Stage and commit this run's changes (never `docs/plans/`). This is required, not optional: the
next iteration is a fresh agent with no memory of this one, so it can only see your progress via
git history, and its own rebase step needs a clean tree to start from.

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
ITERATION_START_SHA: <the value from step 1>
SUMMARY: <one line: what converged>
```

or

```
CONVERGED: no
ITERATION_START_SHA: <the value from step 1>
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
  user via AskUserQuestion — never guess silently.
- Honor spec parameters; no magic numbers.
- Fix code, not the contract, when the spec is correct.
- Never commit files under `docs/plans/`.
- Always commit before ending, using the `wip(allium-loop):` prefix if verification is not green.
- Always end with the exact `CONVERGED:` / `ITERATION_START_SHA:` / `SUMMARY:` block from step 9
  — the driver parses it literally.
```

- [ ] **Step 3: Confirm the file reads correctly**

Read `plugin/skills/allium-loop/prompt.md` back and check every `{{PLACEHOLDER}}` name matches
exactly what Task 2 will substitute (`{{DESIGN_DOC}}`, `{{VERIFY_COMMAND}}`, `{{BASE_BRANCH}}`,
`{{ITERATION_NUMBER}}` — no leftover `{{TARGET_SPEC}}` anywhere).

- [ ] **Step 4: Commit**

```bash
git add plugin/skills/allium-loop/prompt.md
git commit -m "feat(allium-loop): rewrite per-iteration prompt for fresh-agent-per-run loop"
```

---

### Task 2: Rewrite `SKILL.md` (the kickoff + loop-driver instructions)

**Files:**
- Modify: `plugin/skills/allium-loop/SKILL.md` (full rewrite)

**Interfaces:**
- Consumes: the exact `CONVERGED:` / `ITERATION_START_SHA:` / `SUMMARY:` report format produced
  by Task 1's `prompt.md` (parsed literally, three fixed labels).
- Produces: the state file schema `.claude/allium-loop-state.local.md`
  (`active`, `runs_completed`, `max_iterations`, `retry_count`, `design_doc`, `base_branch`,
  `verify_command`, `iteration_start_sha`, `started_at`) — no other file in this plan reads or
  writes it, but it's the durable record a human or a resumed session inspects.

- [ ] **Step 1: Read the current file for reference**

Read `plugin/skills/allium-loop/SKILL.md` to confirm the four resolution steps (design doc,
verify command, base branch — no target-spec resolution step carries forward) keep their exact
existing priority-order wording, since the design doc says these are unchanged.

- [ ] **Step 2: Write the new file**

Replace the entire contents of `plugin/skills/allium-loop/SKILL.md` with:

```markdown
---
description: "Drive the Allium spec-first convergence loop (Loop A) from a spec/design document: dispatch a fresh agent each run to tend the spec, propagate tests, implement to green, and weed, repeating until converged"
allowed-tools: ["Read", "Write", "Bash", "Agent", "AskUserQuestion"]
---

# Allium Spec-First Loop

This skill drives the Allium spec-first convergence loop (Loop A) from a design/spec document
toward converged spec, tests, and code. It is the spec-first sibling of `allium-weed-loop`. It is
language- and stack-agnostic: it never assumes a particular test runner or toolchain.

Unlike a ralph-loop-style skill, this skill does not rely on an external Stop hook or a
`.claude/ralph-loop.local.md` state file. The session that invokes this skill becomes the loop
**driver**: it dispatches one fresh subagent per iteration (via the Agent tool, never `fork` —
each iteration has no memory of prior ones, only the repo's own state carries over) and decides
whether to continue based on that subagent's final report. The driver never does the
rebase/tend/implement/verify/weed work itself.

## Instructions

### Kickoff

1. **Check for an existing active loop first.** Look for `.claude/allium-loop-state.local.md`
   with `active: true`. If found, read it and use AskUserQuestion to ask the user whether to:
   - **Resume** — dispatch the next iteration using the existing file's values (do not reset
     `runs_completed` or `retry_count`).
   - **Abandon** — delete the state file and continue with a fresh kickoff below.
   - **Cancel** — stop here, do nothing further.
   Never silently overwrite an active state file.

2. **Resolve the input design/spec document**, in priority order:
   1. **Explicit arg** — if a path was passed to the skill, use it.
   2. **Recent context** — otherwise scan the recent conversation for a design/spec document
      created or referenced this session (e.g. a file just written under
      `docs/superpowers/specs/` or `docs/plans/`, or a path the user just named). If exactly one
      clear candidate exists, use it and **tell the user which document was picked** so they can
      catch a wrong guess.
   3. **Ask** — if args and context yield nothing, or multiple candidates are ambiguous, ask for
      the doc path via AskUserQuestion.

3. **Resolve the verify command** for this repo — the command that runs its test suite (and any
   other required checks) — in priority order:
   1. **Task/session context** — a verify command already surfaced this session (e.g. a
      "Verification" section in the current task's prompt, or one set via a project's
      task-management tooling).
   2. **Project docs** — a documented test/build command in this repo's `CLAUDE.md`, `AGENTS.md`,
      `README`, or equivalent (e.g. `cargo test`, `npm test`, `pytest`, `go test ./...`,
      `mvn test`).
   3. **Ask** — if none is found, ask the user for the command via AskUserQuestion before
      starting the loop.

4. **Resolve the base branch** to rebase onto each iteration — in priority order:
   1. **Task context** — if this session is running as a dispatched task, use that task's
      `base_branch` (e.g. via the dispatch MCP `get_task` tool).
   2. **Ask** — if there is no task context to read a base branch from, ask the user via
      AskUserQuestion rather than assuming `main`.

5. **Resolve `max_iterations`**: if the user explicitly asked for a specific value when invoking
   the skill (e.g. "run allium-loop with max_iterations 20"), use it; otherwise default to `6`.

6. **Read the prompt file** at
   `~/.claude/plugins/local/dispatch/skills/allium-loop/prompt.md`.

7. **Create the loop state file** directly at `.claude/allium-loop-state.local.md` using the
   Write tool:

```markdown
---
active: true
runs_completed: 0
max_iterations: MAX_ITERATIONS
retry_count: 0
design_doc: "DESIGN_DOC_PATH"
base_branch: "BASE_BRANCH"
verify_command: "VERIFY_COMMAND"
started_at: "TIMESTAMP"
---
```

   Get the timestamp with `date -u +%Y-%m-%dT%H:%M:%SZ`. Leave `iteration_start_sha` unset for
   now — the first iteration reports its own.

8. **Tell the user** the loop is active (naming the design doc, verify command, base branch, and
   `max_iterations` — noting it can be raised by asking), then dispatch iteration 1 immediately
   (see "Each Iteration" below).

### Each Iteration

1. Substitute `{{DESIGN_DOC}}`, `{{VERIFY_COMMAND}}`, `{{BASE_BRANCH}}`, and
   `{{ITERATION_NUMBER}}` (the next unused iteration number, 1-indexed) into the prompt content
   read in kickoff step 6.

2. Dispatch it: call the Agent tool with a **fresh subagent** (do not pass `subagent_type:
   "fork"`) and this filled-in prompt as its task.

3. When that call's result arrives (a task-notification, per the Agent tool's async dispatch
   model — this may land in a different turn than the one that dispatched it):

   - **The subagent errored, was skipped, or its final message has no parseable `CONVERGED:`
     line**: read `retry_count` from the state file.
     - If `retry_count == 0`: set it to `1`, keep `{{ITERATION_NUMBER}}` unchanged, and
       re-dispatch the same iteration (repeat step 2 above).
     - If `retry_count` was already `1`: delete the state file and tell the user this iteration
       failed twice and the loop has stopped — do not retry indefinitely or silently treat this
       as "no progress."
   - **A real report was returned**: increment `runs_completed` in the state file by exactly 1,
     reset `retry_count` to `0`, and store the reported `ITERATION_START_SHA` as
     `iteration_start_sha`.
     - **`CONVERGED: yes`**: delete `.claude/allium-loop-state.local.md` and report success to
       the user, including the final `SUMMARY`.
     - **`CONVERGED: no`**, `runs_completed < max_iterations`, and the `SUMMARY` reports real
       changes this run: dispatch the next iteration (repeat step 1 above, with
       `{{ITERATION_NUMBER}}` incremented by one).
     - **`CONVERGED: no`** and either `runs_completed >= max_iterations`, or this run's and the
       previous run's `SUMMARY` both reported no changes: delete the state file, summarize to the
       user exactly what's unresolved and why, and stop. Never emit a false convergence claim to
       exit early.
```

- [ ] **Step 3: Confirm cross-references**

Read both `plugin/skills/allium-loop/SKILL.md` and `plugin/skills/allium-loop/prompt.md` back and
check: every placeholder SKILL.md says it substitutes (`{{DESIGN_DOC}}`, `{{VERIFY_COMMAND}}`,
`{{BASE_BRANCH}}`, `{{ITERATION_NUMBER}}`) appears in `prompt.md`, and vice versa — no orphaned
placeholder on either side.

- [ ] **Step 4: Commit**

```bash
git add plugin/skills/allium-loop/SKILL.md
git commit -m "feat(allium-loop): rewrite SKILL.md as a fresh-agent-per-run loop driver"
```

---

### Task 3: Verify no regressions and attach the plan to the task

**Files:**
- None modified — verification only.

**Interfaces:**
- Consumes: nothing new.
- Produces: nothing new; confirms Tasks 1-2 didn't break `src/setup/plugins.rs`'s
  `plugin_embeds_required_files` test or `scripts/check-doc-paths.sh` (which only scans
  `CLAUDE.md` and the `docs/architecture.md` / `docs/conventions.md` / `docs/module-map.md` /
  `docs/how-to.md` / `docs/mcp.md` files for `src/*.rs` path references — unaffected by this
  change, since it never touches those files or references any `src/*.rs` path).

- [ ] **Step 1: Run the specific existing test that touches these files**

```bash
cargo test plugin_embeds_required_files
```

Expected: PASS (it only asserts `skills/allium-loop/SKILL.md` and
`skills/allium-loop/prompt.md` exist inside the embedded plugin directory — unaffected by a
content-only rewrite).

- [ ] **Step 2: Run the full verification suite**

```bash
cargo test && ./scripts/check-doc-paths.sh
```

Expected: PASS. If anything fails, it indicates an unrelated pre-existing issue or a mistake
introduced in Tasks 1-2 (e.g. a stray reference elsewhere in the repo to the removed
`{{TARGET_SPEC}}` placeholder or to `.claude/ralph-loop.local.md` in the context of this skill) —
investigate and fix before proceeding; do not skip.

- [ ] **Step 3: Attach the plan to task 3715**

Call the dispatch MCP `update_task` tool for task 3715 with the plan's path
(`docs/superpowers/plans/2026-07-26-allium-loop-fresh-agent.md`) so it's recorded on the task.

- [ ] **Step 4: File a follow-up for `allium-weed-loop`**

`.claude/skills/allium-weed-loop/` has the identical raw-counter problem plus an already-stale
hardcoded 3-file spec list, and was explicitly scoped out of this task. Record this via the
dispatch MCP `record_learning` tool (or `create_task`, if the user prefers a tracked follow-up
task over a learning) so it isn't lost.
