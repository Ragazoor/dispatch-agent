---
name: wrap-up
description: Use when implementation is complete to wrap up a dispatch worktree. Commits remaining changes, asks the user to choose between rebasing onto main or creating a GitHub PR, then calls the wrap_up MCP tool. The task is moved to done automatically on success.
---

# Wrap Up

Wrap up a dispatch worktree: commit remaining changes, use the `AskUserQuestion` tool with a question like:

> Wrap up task #{id} (`{title}`):
> **(r)** rebase onto main — fast-forwards main with this branch, kills this tmux window
> **(p)** create PR — pushes branch and opens a GitHub PR
> **(Esc / n)** cancel

Then call the `wrap_up` MCP tool. If the user cancels or says no, exit without calling any tool. 

**Announce at start:** "I'm using the wrap-up skill to complete this task."

**Precondition:** The task must be in "running" or "review" status. The `wrap_up` MCP tool will reject tasks in any other status.

## Step 1: Get the task ID from the current branch

Run:
```bash
git rev-parse --abbrev-ref HEAD
```

Extract the leading integer from the `{id}-{slug}` pattern (e.g. `42-fix-login-bug` → `42`).

If the branch does not match the `{id}-{slug}` pattern, stop and tell the user:
> "This branch doesn't follow the dispatch naming convention (`{id}-{slug}`). Cannot determine task ID."

## Step 2: Commit uncommitted changes

Run:
```bash
git status --porcelain
```

If there are modified or untracked files, commit them before proceeding. Use a commit skill if available, otherwise create a commit directly.

## Step 3: Ask the user to choose — MANDATORY

**You MUST use the `AskUserQuestion` tool here.** Do NOT skip this step. Do NOT assume a default. Do NOT proceed to Step 4 without an explicit answer from the user.

Use the `AskUserQuestion` tool with a question like:

> Wrap up task #{id} (`{title}`):
> **(r)** rebase onto main — fast-forwards main with this branch, kills this tmux window
> **(p)** create PR — pushes branch and opens a GitHub PR
> **(Esc / n)** cancel

If the user cancels or says no, exit without calling any tool.

## Step 4: Execute the chosen action

The task is automatically moved to "done" on success. Do not update the task status yourself.

**Epic auto-dispatch:** If this task belongs to an epic, the next backlog subtask will be automatically dispatched after wrap-up (both rebase and PR). The next task's worktree branches from this task's branch. No extra action is needed.

### If rebase:

Call the `dispatch` MCP tool `wrap_up` with:
- `task_id`: the integer from Step 1
- `action`: `"rebase"`

The tool blocks until the rebase completes. On success, the task is moved to "done" and the tmux window is killed, ending this session. Do not attempt any further actions after a successful rebase.

If the tool returns an error (e.g. rebase conflict, repo not on main), show the user the exact error message from the response and suggest resolution steps. The task remains in its current status.

### If PR:

Call the `dispatch` MCP tool `wrap_up` with:
- `task_id`: the integer from Step 1
- `action`: `"pr"`

The tool blocks until the PR is created. On success, it returns the PR URL and number. A `/code-review` command will be injected into this session once the PR is ready.

If the tool returns an error (e.g. push failed, PR creation failed), show the user the exact error message from the response. The task remains in its current status.
