---
name: wrap-up
description: Use to finish a dispatch task and close its session — whenever work in a dispatch worktree is complete and the user says to wrap up, finish, close out, finalise, or "we're done here", even if they don't name this skill. Commits remaining changes, then takes one of three paths chosen by the user: rebase onto the task's base_branch (dispatch-driven), author and open a draft GitHub PR yourself (you write the title and body), or done with no git operations. Always use this rather than calling wrap_up/exit_session by hand — the retro step must run before the commit and the two closing calls have an ordering that is easy to get wrong.
---

# Wrap Up

Wrap up a dispatch worktree. All three paths follow the same shape:

choose the action → `/retro` → commit → `wrap_up(action)` → a single `exit_session(token, action, ...)` call that applies the terminal state change and closes the session.

**`exit_session` is mandatory on every path.** `wrap_up` alone changes nothing terminal — it issues a token and, for `rebase`, does the git work. The task's status is not moved and the session is not closed until `exit_session` runs. A wrap-up that stops after `wrap_up` leaves the tmux window alive and the task stuck in its old status. Never end your turn between the two calls.

- **rebase** — dispatch handles the git work. `wrap_up(action="rebase")` fast-forwards `{base_branch}`; the closing `exit_session` call then marks the task Done and kills your tmux window. On a successful rebase, dispatch also re-indexes the repo in the background if it has a RAG index.
- **pr** — you handle it. Inspect the diff you produced, write a real title and body that describe what was actually built, and run `gh pr create --draft` yourself. Dispatch deliberately does not author PR bodies: an auto-generated body is always worse than what you can write after seeing the work.
- **done** — no git operations. Use for research, planning, or work already on `{base_branch}`.

**Announce at start:** "I'm using the wrap-up skill to complete this task."

## Argument check

If the skill was invoked with an argument (e.g. `/wrap-up rebase`, `/wrap-up pr`, or `/wrap-up done`):
- Treat the argument as the chosen action (`rebase`, `pr`, or `done`)
- Skip Step 4 (AskUserQuestion) entirely
- After completing Steps 1–3, go straight to Step 5 with that action (Step 4 is the only step skipped)

If the argument is anything other than `rebase`, `pr`, or `done`, ignore it and proceed normally (Step 4 will ask).

**Precondition:** The task must have a worktree and be in "running" or "review" status. This applies to all three paths — `wrap_up` validates it server-side (`is_wrappable`) and rejects anything else, so there is no path where you can wrap up a backlog task or one without a worktree.

## Step 1: Get the task ID from the current branch

Run:
```bash
git rev-parse --abbrev-ref HEAD
```

Extract the leading integer from the `{id}-{slug}` pattern (e.g. `42-fix-login-bug` → `42`).

If the branch does not match the `{id}-{slug}` pattern, stop and tell the user:
> "This branch doesn't follow the dispatch naming convention (`{id}-{slug}`). Cannot determine task ID."

## Step 2: Get task details

Call the `dispatch` MCP tool `get_task` with the task ID from Step 1. Read the `base_branch` field from the response and use it wherever the instructions below refer to `{base_branch}`. If the field is absent or empty, fall back to `main`. (The rebase path resolves the real base branch server-side from the task record, so `{base_branch}` only matters for the diff/PR commands you run locally.)

Also read the `wrap_up_mode` field. If it is set (`rebase`, `pr`, or `done`) **and** no argument was provided at invocation, treat it exactly like an argument: skip Step 4 (AskUserQuestion) and proceed to Step 5 with that action.

## Step 3: Simplify code changes (conditional)

Check whether code was written in this branch — both committed and uncommitted:

```bash
git diff {base_branch}..HEAD --name-only
git diff --name-only
```

If the combined output includes any source code files (`.rs`, `.py`, `.ts`, `.js`, `.tsx`, `.jsx`, `.go`, `.java`, `.cpp`, `.c`, `.h`, `.swift`, `.kt`, `.rb`, `.cs`) — i.e., not only docs, configs, snapshots, or lock files — invoke the `simplify` skill to review and apply improvements:

```
Skill({ skill: "simplify" })
```

Wait for the skill to complete before proceeding. If it makes additional changes, those will be picked up in Step 6.

If there are no code file changes, skip this step entirely.

## Step 4: Ask the user to choose

Use the `AskUserQuestion` tool, and wait for an actual answer before going on.

This is the one genuinely irreversible decision in the whole skill, which is why it belongs to the user rather than to you. Rebase fast-forwards a shared branch; the PR path pushes a branch and opens a PR under the user's GitHub identity; done closes the task with the work integrated nowhere. Guessing wrong is not a slightly-suboptimal choice — it publishes something or discards an integration step, and there is no undo from inside this skill. Even when the work obviously "looks like a PR", the user may have a reason to rebase. Ask.

The exception is when the action was already chosen for you — an invocation argument or `wrap_up_mode` from Step 2. That is the user's answer, given earlier; don't ask again.

Use the `AskUserQuestion` tool with a question like:

> Wrap up task #{id} (`{title}`):
> **(r)** rebase onto `{base_branch}` — dispatch fast-forwards `{base_branch}` with this branch and kills this tmux window
> **(p)** author and create a draft PR — you draft the title/body, run `gh pr create`, then record the URL via `exit_session`
> **(d)** done — no git operations (use for research, planning, or work already on `{base_branch}`)
> **(Esc / n)** cancel

If the user cancels or says no, exit without calling any tool. Nothing has been
committed yet at this point, and the retro has not run — a cancel here leaves the
worktree exactly as you found it.

## Step 5: Run the retro

Invoke the retro skill:

```
Skill({ skill: "retro" })
```

Wait for it to complete before proceeding.

Retro reflects on where this session lost time and may fix small inaccuracies in
`CLAUDE.md`, a page under `docs/`, or a skill so the next agent dispatched here
does better. Two things about this position are deliberate:

- It runs **after the action is settled** (Step 4), so retro knows whether its
  fix can reach `{base_branch}` at all. Tell it the action you are wrapping up
  with. On `done` there is no rebase and no push, so a fix would be stranded in
  the worktree and retro should file a task instead of editing.
- It runs **before the commit** (Step 6), so anything it does fix is committed
  with the session's work and travels with the rebase or the PR.

Do not defer it to the closing sequence. After `wrap_up` the rebase path has
already fast-forwarded `{base_branch}`, so a later commit strands those fixes on
a branch nobody merges, and the PR path has already pushed.

Retro may also file follow-up tasks. That is expected — leave them alone.

## Step 6: Commit uncommitted changes

Run:
```bash
git status --porcelain
```

If there are no changes, skip to Step 7.

If there are changes, commit them inline — run these commands yourself rather than invoking a commit skill or delegating to another tool. A commit skill would re-derive context you already have and can pull in its own conventions; you just watched this work happen, so you can stage and describe it in three commands:

1. `git add` the relevant files (prefer named files over `git add -A`)
2. `git diff --cached` to review what's staged
3. `git commit -m "..."` with a short message summarizing the changes

Don't polish the message. This commit exists so no work is lost before the branch is integrated — on the rebase path it lands on `{base_branch}` among your earlier commits, and on the PR path the PR body is where the real explanation goes. Once committed, move straight to Step 7.

## Step 7: The closing sequence

Every path ends with the same four steps. Only Step C differs by action, plus the PR path's authoring work which happens *before* this sequence (see *The PR path* below). The task moves to "done" (rebase, done) or "review" (pr) automatically — don't set the status by hand.

**A. Summarise behaviour changes.** Invoke the `summarize` skill (`Skill({ skill: "summarize" })`) and show the user the result, which leads with how behaviour changed. This is the user's last recap before the session closes. Skip it on the PR path — the PR body you just wrote already serves this purpose, and repeating it wastes the user's attention.

**B. Rate retrieved knowledge.** See *Rate retrieved knowledge* below.

**C. Call `wrap_up`** with `task_id` (the integer from Step 1) and `action`. This returns an **Exit token** (a UUID string). It does not close the session and does not move the task's status — that all waits for Step D. What it does beyond issuing the token depends on the action:

| action | what `wrap_up` does | notes |
|---|---|---|
| `rebase` | blocks until the rebase completes and fast-forwards `{base_branch}` | can fail on conflict, or if the repo isn't on `{base_branch}` |
| `pr` | nothing — no `pr_url` here, it travels with `exit_session` | |
| `done` | nothing | |

If `wrap_up` returns an error, show the user the exact message and stop. Do not call `exit_session` — you have no valid token, and the task stays in its current status. For a rebase conflict, suggest resolution steps.

**D. Call `exit_session`** with `task_id`, `token` (from Step C), `action` (must match the action you passed to `wrap_up`), and `pr_url` on the pr path only. This single call applies the terminal state change, clears the tmux window, and consumes the token — atomically. There is no follow-up call; this closes the loop.

Do not stop between C and D. Skipping `exit_session` leaves the tmux window alive and the task stuck in its old status — and on the PR path, the PR unrecorded.

### Don't dispatch the epic's next subtask yourself

If the task belongs to an epic with auto-dispatch on, `exit_session` chains the next backlog subtask server-side. You do nothing. This is worth stating only because `dispatch_task` is a tool you can call, and firing it yourself around a close produces two agents on one epic — one of them branched from a base that predates your own commits. Leave the chaining alone, including when the close fails (below) and no chain happens.

### If `exit_session` errors

- `"has no active session"` — something else (a merge, a manual close) already tore the session down. Treat this as already wrapped up, not a failure. Do not retry, and on the PR path do not re-create the PR.
- An error naming a mismatched action — the token doesn't match the action you're closing with. Show the user the exact error rather than guessing which action was intended.
- Missing/empty `pr_url` (pr path) — pass the URL you captured.

### If `exit_session` succeeds but says the close did not take effect

`exit_session` can return a **successful** response that nevertheless reports the close did **not** happen: text saying the task could not be moved to its terminal status, that your tmux session is still alive, and that it needs closing by hand. This is deliberate rather than an error — the exit token is consumed before the terminal write is attempted, so an error response would strand you with no retry path. Read the response text; don't infer success from the absence of an error.

When you get it:
- **Do not retry** `exit_session` — the token is gone, so a retry only produces "call wrap_up first".
- **Do not** call `wrap_up` again for a fresh token.
- Tell the user plainly: the close failed, the task is still in its previous status, the tmux window is still alive, and it needs closing by hand from the TUI.
- Your session stays open. Nothing was torn down, so the user can attach to the window.
- On the PR path, the PR itself still exists — don't re-create it. Only the task's move to Review failed.

### Rate retrieved knowledge

When dispatch starts an agent, it injects relevant knowledge into the prompt under "## Validated knowledge for this task". You may also call `query_learnings` mid-task. Each surfacing is recorded as a retrieval, and the knowledge base learns which entries are useful from your ratings.

Rate via the `rate_learning` MCP tool — ideally the moment you act on an entry, or at the latest before you wrap up. For every learning you acted on that was surfaced to you this task:

```
rate_learning(learning_id=<id>, task_id=<id>, verdict="helped")
```

- `verdict="helped"` — the entry was relevant and you applied it (upvotes it).
- `verdict="wrong"` — the entry was misleading, outdated, or contradicts current code (routes an approved entry to `needs_review` for human curation).

Only entries surfaced to you this task can be rated. There is no separate "unused" verdict — simply don't rate entries you didn't act on. `wrap_up` does not accept verdicts; rate through `rate_learning`.

## The PR path: author the PR before the closing sequence

You are creating a real PR with a title and body that reflect the actual work. Dispatch will not do this for you. Do all of the following *before* Step 7, then run the closing sequence with `action="pr"`.

### Inspect what changed

```bash
git log {base_branch}..HEAD --oneline
git diff {base_branch}...HEAD --stat
git diff {base_branch}...HEAD
```

Read the output. Build a mental model of what shipped: which files changed and why, which behaviours were added/removed/fixed, what the user-visible effect is. If the diff is large, focus on the changes that matter for review (skip generated files, snapshot updates, formatting churn).

### Draft the title and body

**Title** — imperative mood, ≤72 characters, describes the change as a single action. Examples:
- `fix(auth): handle expired refresh tokens without 500ing`
- `feat(tui): add project filter to archive view`
- `refactor(db): split TaskPatch builder into smaller methods`

Avoid `wip:`, `task #N:`, or anything that just restates the task title. The title should be useful in `git log --oneline`.

**Body** — Markdown, this structure:

```markdown
## Summary
- {what changed and why, 1–4 bullets, plain language}
- {keep one bullet per logical change so reviewers can scan}

## Test plan
- [ ] {how to verify the change manually or via tests}
- [ ] {any edge case worth re-running}
- [ ] {tests added/updated, if relevant}

Implements #{task_id}.
```

If the change has UI implications, add screenshots or a description of the visual effect under a `## Notes` section. Skip sections that don't apply (e.g. no Test plan if the change is documentation-only) — don't pad.

### Push and create the draft PR

Find the repo slug from the remote:

```bash
git remote get-url origin
```

The slug is the `owner/repo` portion (e.g. `git@github.com:Acme/dispatch.git` → `Acme/dispatch`).

Push the branch:

```bash
git push -u origin {branch}
```

If the push is rejected (non-fast-forward), STOP. Do not force-push without the user's explicit authorisation. Show them the error and ask how to proceed.

Create the PR. Use a HEREDOC for the body so newlines and Markdown survive shell quoting:

```bash
gh pr create --draft \
  --base {base_branch} \
  --head {owner}:{branch} \
  --repo {owner}/{repo} \
  --title "{your authored title}" \
  --body "$(cat <<'EOF'
{your authored body}
EOF
)"
```

`{owner}` is the first part of the repo slug. The `{owner}:{branch}` format is required so `gh` resolves the branch in the same repo as `--repo` (rather than your authenticated user's namespace).

`gh pr create` prints the PR URL on stdout. Capture it — it is the `pr_url` you pass to `exit_session` in Step D.

If `gh` reports `a pull request for branch '...' already exists`, parse the URL it returns and use that — the PR already exists and your job is just to record it.

### Then run the closing sequence

Go to Step 7 with `action="pr"`, skipping Step A (the PR body is your summary). The ordering matters: `wrap_up(action="pr")` deliberately doesn't move the task to Review or set the PR url — that's deferred to `exit_session`. Until `exit_session` runs, dispatch has no PR-merge polling armed for this task, so a merge can't tear the session down between the two calls. Don't reorder to "close first, then finish up".
