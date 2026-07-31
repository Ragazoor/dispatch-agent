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
   with `active: true`. If found, read it and tell the user a loop is already active, including its
   current progress (`started_at`, and `runs_completed` of `max_iterations`). Then use
   AskUserQuestion to ask whether to:
   - **Resume** — dispatch the next iteration using the existing file's values (do not reset
     `runs_completed`, `retry_count`, or `consecutive_no_change_runs`).
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

6. **Resolve `model`** — the model each iteration agent runs on: if the user explicitly asked for
   one when invoking the skill (e.g. "run allium-loop on opus"), use it; otherwise default to
   `sonnet`. A run does up to `max_iterations` full iterations, so the iteration agent's model
   multiplies across the whole loop; sonnet is the default because the iteration agent's own work
   (rebase, `/propagate`, red check, implement, verify, commit) is mechanical. The judgement-heavy
   spec steps are not affected — see the note in step 2 of the prompt file. Recording this as a
   loop parameter is what lets a hard convergence opt back into a stronger model without editing
   this skill.

7. **Read the prompt file** at
   `~/.claude/plugins/local/dispatch/skills/allium-loop/prompt.md`.

8. **Create the loop state file** directly at `.claude/allium-loop-state.local.md` using the
   Write tool:

```markdown
---
active: true
runs_completed: 0
max_iterations: MAX_ITERATIONS
retry_count: 0
consecutive_no_change_runs: 0
design_doc: "DESIGN_DOC_PATH"
base_branch: "BASE_BRANCH"
verify_command: "VERIFY_COMMAND"
model: "MODEL"
started_at: "TIMESTAMP"
---
```

   Get the timestamp with `date -u +%Y-%m-%dT%H:%M:%SZ`.

9. **Tell the user** the loop is active (naming the design doc, verify command, base branch,
   `model`, and `max_iterations` — noting both can be changed by asking), then dispatch iteration 1
   immediately (see "Each Iteration" below).

### Each Iteration

1. Substitute `{{DESIGN_DOC}}`, `{{VERIFY_COMMAND}}`, `{{BASE_BRANCH}}`, and
   `{{ITERATION_NUMBER}}` into the prompt content read in kickoff step 7. The iteration number is
   1-indexed and always derived from the state file as `runs_completed + 1` — so it is correct
   after a resume or a context compaction, not only on a clean run.

2. Dispatch it: call the Agent tool with a **fresh subagent** (do not pass `subagent_type:
   "fork"`), passing `model` set to the state file's `model` value (`sonnet` unless this run
   resolved otherwise in kickoff step 6), and this filled-in prompt as its task.

   The two rules reinforce each other: a `fork` ignores the `model` override entirely and runs on
   the session model, so dispatching a fork would silently discard the pin — passing a fresh
   subagent is what makes the model override take effect at all. Read `model` from the state file
   rather than remembering it, for the same reason the iteration number is derived there: a resume
   or a context compaction must not lose it.

3. When that call's result arrives (a task-notification, per the Agent tool's async dispatch
   model — this may land in a different turn than the one that dispatched it):

   - **The subagent errored, was skipped, or returned an unparseable report** — treat a report as
     unparseable if it is missing EITHER required label (`CONVERGED:` or `SUMMARY:`), not just
     `CONVERGED:`; a partial or malformed report is an error, not a result. Read `retry_count` from
     the state file.
     - If `retry_count == 0`: set it to `1`, keep `{{ITERATION_NUMBER}}` unchanged, and
       re-dispatch the same iteration (repeat step 2 above).
     - If `retry_count` was already `1`: delete the state file and tell the user this iteration
       failed twice and the loop has stopped — do not retry indefinitely or silently treat this
       as "no progress."
   - **A real report was returned** (both labels present): increment `runs_completed` in the state
     file by exactly 1 and reset `retry_count` to `0`. Then update `consecutive_no_change_runs`: if
     this run's `SUMMARY` reports no changes, increment it by 1; otherwise reset it to `0`.
     - **`CONVERGED: yes`**: delete `.claude/allium-loop-state.local.md` and report success to
       the user, including the final `SUMMARY`.
     - **`CONVERGED: no`** and either `runs_completed >= max_iterations` or
       `consecutive_no_change_runs >= 2`: delete the state file, summarize to the user exactly
       what's unresolved and why, and stop. Never emit a false convergence claim to exit early.
     - **`CONVERGED: no`** and budget remains (`runs_completed < max_iterations` **and**
       `consecutive_no_change_runs < 2`): dispatch the next iteration (repeat step 1 above).

   When dispatching the next iteration the number advances on its own: `runs_completed` was just
   incremented, and step 1 derives the number as `runs_completed + 1`. (The retry re-dispatch above
   is the one case that deliberately reuses the same number.)
