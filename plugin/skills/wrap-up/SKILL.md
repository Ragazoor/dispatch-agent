---
name: wrap-up
description: Use when implementation is complete to wrap up a dispatch worktree. Commits remaining changes, asks the user to choose between rebasing onto the task's base_branch or authoring + creating a draft GitHub PR yourself. Rebase is dispatch-driven; PR is agent-driven (you write the title and body, then call dispatch to record the URL).
---

# Wrap Up

Wrap up a dispatch worktree. The three paths:

All three paths follow the same shape: `wrap_up(action)` → run the `/retro` skill → a single `exit_session(token, action, ...)` call that applies the terminal state change and closes the session.

- **rebase** — dispatch handles the git work. `wrap_up(action="rebase")` fast-forwards `{base_branch}`; the closing `exit_session` call then marks the task Done and kills your tmux window. On a successful rebase, dispatch also re-indexes the repo in the background if it has a RAG index.
- **pr** — you handle it. Inspect the diff you produced, write a real title and body that describe what was actually built, run `gh pr create --draft` yourself, call `wrap_up(action="pr")` (no `pr_url` — that comes later), run `/retro`, then call `exit_session(action="pr", pr_url=<url>)` to record the URL, move the task to Review, and close the session in one atomic step. Dispatch deliberately no longer authors PR bodies because the auto-generated bodies were always worse than what you can write after seeing the work.
- **done** — no git operations. `wrap_up(action="done")` returns an exit token without changing status; the closing `exit_session` call marks the task Done and kills the tmux window.

**Announce at start:** "I'm using the wrap-up skill to complete this task."

## Argument check

If the skill was invoked with an argument (e.g. `/wrap-up rebase`, `/wrap-up pr`, or `/wrap-up done`):
- Treat the argument as the chosen action (`rebase`, `pr`, or `done`)
- Skip Step 4 (AskUserQuestion) entirely
- After completing Steps 1–3, go straight to Step 5 with that action

If the argument is anything other than `rebase`, `pr`, or `done`, ignore it and proceed normally (Step 4 will ask).

**Precondition:** The task must be in "running" or "review" status. Both rebase and PR paths require the task to be wrappable.

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

## Step 2.5: Simplify code changes (conditional)

Check whether code was written in this branch — both committed and uncommitted:

```bash
git diff {base_branch}..HEAD --name-only
git diff --name-only
```

If the combined output includes any source code files (`.rs`, `.py`, `.ts`, `.js`, `.tsx`, `.jsx`, `.go`, `.java`, `.cpp`, `.c`, `.h`, `.swift`, `.kt`, `.rb`, `.cs`) — i.e., not only docs, configs, snapshots, or lock files — invoke the `simplify` skill to review and apply improvements:

```
Skill({ skill: "simplify" })
```

Wait for the skill to complete before proceeding. If it makes additional changes, those will be picked up in Step 3.

If there are no code file changes, skip this step entirely.

## Step 3: Commit uncommitted changes

Run:
```bash
git status --porcelain
```

If there are no changes, skip to Step 4.

If there are changes, commit them inline — do NOT invoke a commit skill or delegate to another tool. Run these commands directly:

1. `git add` the relevant files (prefer named files over `git add -A`)
2. `git diff --cached` to review what's staged
3. `git commit -m "..."` with a short message summarizing the changes

Do NOT spend time perfecting the commit message. The goal is to capture the changes, not write a polished commit. Once committed, proceed immediately to Step 4.

## Step 4: Ask the user to choose — MANDATORY

**You MUST use the `AskUserQuestion` tool here.** Do NOT skip this step. Do NOT assume a default. Do NOT proceed to Step 5 without an explicit answer from the user.

Use the `AskUserQuestion` tool with a question like:

> Wrap up task #{id} (`{title}`):
> **(r)** rebase onto `{base_branch}` — dispatch fast-forwards `{base_branch}` with this branch and kills this tmux window
> **(p)** author and create a draft PR — you draft the title/body, run `gh pr create`, then record the URL via update_task
> **(d)** done — no git operations (use for research, planning, or work already on `{base_branch}`)
> **(Esc / n)** cancel

If the user cancels or says no, exit without calling any tool.

## Step 5: Execute the chosen action

The task is automatically moved to "done" (rebase or done) or "review" (PR) on success. Do not update the task status manually except as described below for the PR path.

### If rebase:

This is a **two-call** sequence, with the retro skill run in between. Both calls are mandatory.

**Before wrapping up — summarise behaviour changes.** Invoke the `summarize` skill (`Skill({ skill: "summarize" })`) and show the user the resulting summary, which leads with how the behaviour changed. This gives the user a behaviour-focused recap before the session closes.

Before wrapping up, rate any knowledge surfaced during this task — see *Rate retrieved knowledge* below.

**Call 1 — rebase.** Call the `dispatch` MCP tool `wrap_up` with:
- `task_id`: the integer from Step 1
- `action`: `"rebase"`

The tool blocks until the rebase completes and fast-forwards `{base_branch}`. It does **not** close the session — the tmux window stays alive and the task stays in its current status until you make the second call. The response includes an **Exit token** (a UUID string).

If `wrap_up` returns an error (e.g. rebase conflict, repo not on `{base_branch}`), show the user the exact error message and suggest resolution steps. Do not call `exit_session`. The task remains in its current status.

**Run `/retro`.** Invoke the retro skill (`Skill({ skill: "retro" })`) now, before calling `exit_session`. This is the mandatory reflection step — it replaces the old inline reflection nudge. If retro surfaces pitfalls, conventions, or tool tips, it's already your job to call `record_learning` for those as part of that skill.

**Call 2 — close.** Call the `dispatch` MCP tool `exit_session` with:
- `task_id`: the integer from Step 1
- `token`: the exit token from the wrap_up response
- `action`: `"rebase"` — must match the action passed to `wrap_up`

This single call closes the session: it marks the task Done, clears the tmux window, and consumes the token. There is no separate reflection round-trip anymore.

If this task belongs to an epic with auto-dispatch on, `exit_session` also dispatches the epic's next backlog subtask, after the rebase has landed. That is automatic and server-side — do not try to dispatch the next subtask yourself.

Do NOT stop between Call 1 and Call 2. Skipping `exit_session` leaves the tmux window alive and the task stuck.

**If `exit_session` errors:**
- `"has no active session"` — something else (a merge, a manual close) already tore the session down. Treat this as already wrapped up, not a failure — do not retry.
- An error naming a mismatched action — this means the token doesn't match the action you're closing with. Show the user the exact error rather than guessing which action was intended.

#### Rate retrieved knowledge

When dispatch starts an agent, it injects relevant knowledge into the prompt under "## Validated knowledge for this task". You may also call `query_learnings` mid-task. Each surfacing is recorded as a retrieval, and the knowledge base learns which entries are useful from your ratings.

Rate via the `rate_learning` MCP tool — ideally the moment you act on an entry, or at the latest before you wrap up. For every learning you acted on that was surfaced to you this task:

```
rate_learning(learning_id=<id>, task_id=<id>, verdict="helped")
```

- `verdict="helped"` — the entry was relevant and you applied it (upvotes it).
- `verdict="wrong"` — the entry was misleading, outdated, or contradicts current code (routes an approved entry to `needs_review` for human curation).

Only entries surfaced to you this task can be rated. There is no separate "unused" verdict — simply don't rate entries you didn't act on. `wrap_up` no longer accepts verdicts; rate through `rate_learning` instead.

### If PR — author the PR yourself, then record the URL:

You are creating a real PR with a title and body that reflect the actual work. Dispatch will not do this for you. Follow this sub-flow:

#### 5a. Inspect what changed

```bash
git log {base_branch}..HEAD --oneline
git diff {base_branch}...HEAD --stat
git diff {base_branch}...HEAD
```

Read the output. Build a mental model of what shipped: which files changed and why, which behaviours were added/removed/fixed, what the user-visible effect is. If the diff is large, focus on the changes that matter for review (skip generated files, snapshot updates, formatting churn).

#### 5b. Draft the PR title and body

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

#### 5c. Push and create the draft PR

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

`gh pr create` prints the PR URL on stdout. Capture it.

If `gh` reports `a pull request for branch '...' already exists`, parse the URL it returns and use that — the PR already exists and your job is just to record it.

#### 5d. Call wrap_up, then run retro, then close — in that exact order

This ordering matters: `wrap_up(action="pr")` doesn't move the task to Review or set the PR url — that mutation is deferred to the single `exit_session` call at the end. Until `exit_session` runs, dispatch has no PR-merge polling armed for this task, so there's no race between you finishing retro and a merge tearing the session down mid-flight. Do not reorder these steps.

**Call 1 — wrap_up.** Call the `dispatch` MCP tool `wrap_up` with:
- `task_id`: the integer from Step 1
- `action`: `"pr"`

No `pr_url` here — that travels with `exit_session` instead. The response includes an **Exit token**. The task's status is unchanged by this call.

If `wrap_up` returns an error, show the user the exact error message.

**Run `/retro`.** Invoke the retro skill (`Skill({ skill: "retro" })`) now, before calling `exit_session`. Also rate any knowledge surfaced during this task — see *Rate retrieved knowledge* above.

**Call 2 — close.** Call the `dispatch` MCP tool `exit_session` with:
- `task_id`: the integer from Step 1
- `token`: the exit token from the wrap_up response
- `action`: `"pr"`
- `pr_url`: the URL from Step 5c

This single call moves the task to Review, sets the PR url, clears the tmux window, and starts PR status polling — all atomically. **Do NOT call `exit_session` again after this succeeds** — there's no second call for the pr action; this closes the loop.

If this task belongs to an epic with auto-dispatch on, `exit_session` also dispatches the epic's next backlog subtask. That is automatic and server-side — do not try to dispatch the next subtask yourself.

If `exit_session` returns an error:
- Missing/empty `pr_url` — pass the URL captured in 5c.
- `"has no active session"` — something else already tore the session down (e.g. the PR was merged/closed before you got here). Treat this as already wrapped up, not a failure — do not retry, and do not re-create the PR.
- An error naming a mismatched action — surface the exact error rather than guessing.

### If done — no git operations:

Use this path when the task requires no branch integration (research, planning, or changes already committed to `{base_branch}`).

**Before wrapping up — summarise behaviour changes.** Invoke the `summarize` skill (`Skill({ skill: "summarize" })`) and show the user the resulting summary, which leads with how the behaviour changed. This gives the user a behaviour-focused recap before the session closes.

Before wrapping up, rate any knowledge surfaced during this task — see *Rate retrieved knowledge* above.

**Call 1 — done.** Call the `dispatch` MCP tool `wrap_up` with:
- `task_id`: the integer from Step 1
- `action`: `"done"`

No git operations are performed. The task's status is unchanged by this call — it stays as-is until the closing call. The response includes an **Exit token**.

If `wrap_up` returns an error, show the user the exact error message. Do not call `exit_session`. The task remains in its current status.

**Run `/retro`.** Invoke the retro skill (`Skill({ skill: "retro" })`) now, before calling `exit_session`. This is the mandatory reflection step.

**Call 2 — close.** Call the `dispatch` MCP tool `exit_session` with:
- `task_id`: the integer from Step 1
- `token`: the exit token from the wrap_up response
- `action`: `"done"` — must match the action passed to `wrap_up`

This single call marks the task Done and kills the tmux window.

If this task belongs to an epic with auto-dispatch on, `exit_session` also dispatches the epic's next backlog subtask. That is automatic and server-side — do not try to dispatch the next subtask yourself.

**If `exit_session` errors:**
- `"has no active session"` — something else already tore the session down. Treat this as already wrapped up, not a failure — do not retry.
- An error naming a mismatched action — surface the exact error rather than guessing.
