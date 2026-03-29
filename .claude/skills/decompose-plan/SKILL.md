---
name: decompose-plan
description: >-
  Read an implementation plan and create subtasks under an epic, one at a time.
  Use when user says "decompose plan", "split plan into tasks", "break down plan",
  "create tasks from plan", or provides an epic ID for decomposition.
---

# Decompose Plan

Break down an implementation plan into individual tasks under an epic.

## Usage

`/decompose-plan <epic_id>` or `/decompose-plan` (infers from context)

## Finding the Plan

1. Check current conversation context for recently written plan files
2. Look in `docs/superpowers/plans/` for the most recently modified file
3. If not found: ask the user for the plan file path

## Finding the Epic

1. If provided as argument, use it directly
2. Infer from git branch name: branches follow `{task_id}-{slug}` pattern — fetch the task via MCP `get_task`, then use its `epic_id`
3. If not found: ask the user for the epic ID

## Process

1. Determine the epic ID (see above)
2. Fetch the epic via MCP: `get_epic(epic_id)` to confirm it exists and get context
3. Read the plan file and parse it into ordered steps/tasks
4. For each step, one at a time:
   a. Present the user with a proposed task:
      - **Title**: concise summary of the work item
      - **Description**: what needs to be done. If this task depends on earlier tasks, note the dependency as plain text (e.g., "Depends on: the task that implements X")
      - **Plan**: detailed implementation plan for this specific subtask
   b. Ask: "Create this task? [y]es / [e]dit / [s]kip / [q]uit"
   c. On **yes**: call MCP `create_task` with the proposed fields, including `epic_id`
   d. On **edit**: let the user modify title/description/plan, then create
   e. On **skip**: move to next item
   f. On **quit**: stop processing, summarize what was created
5. After all items: summarize the tasks created (count, titles)

## Important

- Each task should be **independently dispatchable** — it gets its own worktree and agent
- Plans should be detailed enough for a Claude agent to implement without additional context
- Use the epic's `repo_path` for all created tasks
- Tasks with a plan file path start in `ready` status; tasks without start in `backlog`
- Dependencies are noted in task descriptions as plain text — there is no DB-level dependency tracking yet
