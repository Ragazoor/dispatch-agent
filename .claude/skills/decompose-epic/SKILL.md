---
name: decompose-epic
description: Walk through an epic's high-level plan and interactively create a task with a detailed plan for each item. Use when user says "decompose epic", "split epic into tasks", "break down epic", or provides an epic ID for decomposition.
---

# Decompose Epic

Break down an epic's high-level plan into individual tasks, one at a time.

## Usage

`/decompose-epic <epic_id>`

## Process

1. Fetch the epic via MCP: `get_epic(epic_id)`
2. Read the epic's plan field — this is a high-level markdown plan
3. Parse the plan into logical sections/items (headings, bullet points, numbered items)
4. For each item, one at a time:
   a. Present the user with a proposed task:
      - **Title**: concise summary of the work item
      - **Description**: what needs to be done
      - **Plan**: a detailed implementation plan for this specific subtask
   b. Ask: "Create this task? [y]es / [e]dit / [s]kip / [q]uit"
   c. On **yes**: call MCP `create_task` with the proposed fields, including `epic_id`
   d. On **edit**: let the user modify title/description/plan, then create
   e. On **skip**: move to next item
   f. On **quit**: stop processing, summarize what was created
5. After all items: summarize the tasks created (count, titles, statuses)

## Important

- Each task should be **independently dispatchable** — it gets its own worktree
- Plans should be detailed enough for a Claude agent to implement without additional context
- Use the epic's `repo_path` for all created tasks
- Created tasks start in `backlog` status (they have no plan file yet — the plan is in the description)
- The epic's plan is always editable — re-running this skill on the same epic will let you create tasks for new items
