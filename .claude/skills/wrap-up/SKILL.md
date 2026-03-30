---
name: wrap-up
description: Use when implementation is complete and the task is in review status, to wrap up a dispatch worktree. Presents the user with a choice between rebasing onto main or creating a GitHub PR, then calls the wrap_up MCP tool.
---

# Wrap Up

Wrap up a dispatch worktree: ask the user to choose between rebasing onto main or creating a PR, then call the `wrap_up` MCP tool.

**Announce at start:** "I'm using the wrap-up skill to complete this task."

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

If there are modified or untracked files, commit them before proceeding. Use a `/commit` skill if available, otherwise create a commit directly.

## Step 3: Ask the user to choose

Present:

> Wrap up task #{id} (`{title}`):
> **(r)** rebase onto main — fast-forwards main with this branch, kills this tmux window
> **(p)** create PR — pushes branch and opens a GitHub PR
> **(Esc / n)** cancel

Wait for the user's response. If they cancel or say no, exit without calling any tool.

## Step 4: Execute the chosen action

### If rebase:

Call the `dispatch` MCP tool `wrap_up` with:
- `task_id`: the integer from Step 1
- `action`: `"rebase"`

Inform the user:
> "Rebase started. The tmux window will be killed when the rebase completes, ending this session."

### If PR:

Call the `dispatch` MCP tool `wrap_up` with:
- `task_id`: the integer from Step 1
- `action`: `"pr"`

Inform the user:
> "PR creation started in the background. A code review will be automatically triggered once the PR is ready."
