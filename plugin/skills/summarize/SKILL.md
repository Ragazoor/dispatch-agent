---
name: summarize
description: >-
  Summarize what the agent did in this session — goal, commits, files changed,
  outcome. Use when asked to "summarize", "recap", or "what did you do".
---

# Summarize Session

Generate a structured summary of what was accomplished in this session.

**Announce at start:** "I'm using the summarize skill to generate a session summary."

## Step 1: Get context

### Determine branch and task ID

Run:
```bash
git rev-parse --abbrev-ref HEAD
```

If the branch matches `{integer}-{slug}` (e.g. `42-fix-login-bug`), extract the integer as the task ID. Otherwise, proceed without a task ID.

### Get task details (if task ID found)

Call the `dispatch` MCP tool `get_task` with the task ID. Read:
- `title` — what the task was called
- `description` — the original goal
- `base_branch` — the branch to diff against; fall back to `main` if absent or empty

If dispatch MCP is unavailable or no task ID was found, skip this and derive context from git history and the conversation.

## Step 2: Gather git changes

Diff target is the `base_branch` from Step 1 (falling back to `main` if absent).

Run:
```bash
git log {base_branch}..HEAD --oneline
```

```bash
git diff {base_branch}...HEAD --stat
```

If the branch has no commits ahead (e.g. the work is only staged or unstaged), fall back to:
```bash
git diff --stat
```

## Step 3: Synthesize the summary

Using:
- The task title and description from Step 1 (if available)
- The commit list and diff stat from Step 2
- Your conversation context — what you actually did and why during this session

Lead with how the observable behaviour changed — what a user or the next agent would now see or do differently — not just which files changed.

Write a structured summary in this format:

```
## Session Summary: {task title or branch name}

**Goal**: {task description, or a brief statement inferred from the work done}

**Behaviour changes**:
- {one user-visible or observable behaviour delta per bullet}
- {if the change is purely internal — refactor, docs, tests — say "No user-visible behaviour change" and name what changed internally}

**Commits** ({N} commits):
- `{short hash}` {commit message}
- ...

**Changes** ({N} files):
- `{file path}` — {one phrase describing what changed}
- ...

**Outcome**: {one sentence — what was accomplished, and any remaining issues or caveats}
```

**Rules**:
- Every bullet stays on one line
- File descriptions come from your knowledge of the session (not just git stat)
- If there are no commits (only uncommitted changes), omit the Commits section and describe the uncommitted work under Changes
- If the diff is large (>20 files), group files by directory and summarise each group rather than listing every file
- Skip any section that doesn't apply (e.g. no Changes if nothing was modified)
- "Outcome" must name what was actually achieved, not just restate the goal
- The Behaviour changes section is the most important part — describe effects (what now happens differently), not mechanics (which lines moved). If nothing user-visible changed, state that explicitly rather than omitting the section.

## Step 4: Display the summary

Output the summary to the user. Do not offer to post the summary anywhere unless the user explicitly asks.

If this skill was invoked standalone (the user ran `/summarize` or asked to "summarize"/"recap"), this is the final step — stop here.

If this skill was invoked as a sub-step of another skill, it is **not** the final step: immediately resume the calling skill's next instruction in the same turn. Do not stop here.

The summary can be reused directly as the body of a PR or as notes for a follow-up task.
