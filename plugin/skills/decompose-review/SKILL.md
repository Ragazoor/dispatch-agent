---
name: decompose-review
description: >-
  Create work packages from code review findings, grouped by files they touch.
  Use after running "kognic-code-quality:code-review" or any code review that
  produces findings with file:line references. Creates an epic with subtasks
  and attaches a plan to each subtask.
---

# Decompose Review

Convert code review findings into actionable dispatch work packages. Each work
package becomes a subtask in an epic, with a plan file attached.

**Announce at start:** "I'm using the decompose-review skill to create work packages from the code review."

**Precondition:** A code review report must exist in the current conversation. The report should contain findings with `file:line` references grouped by severity.

## Step 1: Extract Review Findings

Read the code review report from conversation context. Extract all findings from the severity sections:

- 🚨 **Blockers** (Must Fix)
- ⚠️ **High Priority** (Strongly Recommend Fixing)
- 💡 **Medium Priority** (Consider for Follow-up)

For each finding, extract:
- **Severity** (blocker / high / medium)
- **File references** (`file:line` format — some findings may reference multiple files or omit line numbers)
- **Description** of the issue
- **Suggestion** for the fix

If no actionable findings exist (only Good Practices), inform the user:
> "The code review found no actionable issues. Nothing to decompose."

Then exit.

## Step 2: Group into Work Packages

Group findings by the files they touch:

1. Extract file paths from all `file:line` references
2. Group by directory prefix (first 2 path components, e.g., `src/mcp`, `src/tui`, `src/db`)
3. If a single finding references files in multiple groups, merge those groups
4. Order work packages by maximum severity (blockers first, then high, then medium)
5. Name each work package after its primary directory or module (e.g., "MCP Handler Cleanup", "TUI Rendering Improvements")

## Step 3: Present Work Packages — MANDATORY

**You MUST use the `AskUserQuestion` tool here.** Do NOT skip this step. Do NOT create tasks without user confirmation.

Present a numbered list of work packages. For each, show:
- Name and finding count by severity
- File list
- One-line summary

Example:

> Work packages from code review:
>
> 1. **MCP Handler Cleanup** — 1 blocker, 2 high priority
>    Files: `src/mcp/handlers/tasks.rs`, `src/mcp/handlers/types.rs`
>    Fix error handling and add missing validation
>
> 2. **TUI Rendering** — 3 medium priority
>    Files: `src/tui/ui.rs`, `src/tui/mod.rs`
>    Simplify complex rendering logic
>
> 3. **Database Queries** — 1 high priority
>    Files: `src/db/queries.rs`
>    Fix N+1 query pattern
>
> Confirm all (Enter), drop by number (e.g. "drop 2"), or merge (e.g. "merge 1 3")?

Wait for the user's response. Apply any adjustments they request (dropping, merging, renaming).

## Step 4: Resolve Repository Path

Resolve the main repository root (not a worktree path):

```bash
git rev-parse --path-format=absolute --git-common-dir
```

Strip the trailing `/.git` to get the main repo path. This is critical because `repo_path` on tasks must point to the main repo root so dispatch can create worktrees from it.

## Step 5: Create Epic

Call the `dispatch` MCP tool `create_epic` with:
- `title`: "Code Review: <YYYY-MM-DD>" (today's date)
- `repo_path`: the main repo path from Step 4
- `description`: the executive summary from the code review report (3-5 bullet points)

Save the returned `epic_id` for use in Step 6.

## Step 6: Write Plan Files and Create Tasks

Create the plan directory:
```bash
mkdir -p docs/plans/review-<YYYY-MM-DD>
```

For each confirmed work package (in severity order):

### 6a. Write the plan file

Write a plan file at `docs/plans/review-<YYYY-MM-DD>/wp-<N>-<slug>.md` following the template in `references/plan-template.md`.

The plan must include:
- All findings for this work package with their severity, file:line, description, and fix
- A changes table listing every file and what to change
- A verification section with test commands

### 6b. Create the subtask

Call the `dispatch` MCP tool `create_task` with:
- `title`: the work package name
- `repo_path`: main repo path from Step 4
- `description`: one-sentence summary of what this work package addresses
- `epic_id`: the epic ID from Step 5
- `plan_path`: absolute path to the plan file just written
- `tag`: `"chore"`
- `sort_order`: N (preserves severity ordering — blockers first)
- `auto_run_plan`: `true` — the plan was already reviewed via the work-package
  confirmation in Step 3, so the dispatched agent should implement it
  immediately instead of asking to proceed
- `wrap_up_mode`: `"rebase"` — so the agent rebases onto `base_branch` at
  wrap-up without asking which path to take. Review work packages are small and
  land on main; they don't each need their own PR.

## Step 7: Report Results

Display the created epic and all task IDs:

> Created epic #<id>: "Code Review: <date>" with <N> subtasks:
> - Task #<id>: <name> (blocker)
> - Task #<id>: <name> (high priority)
> - Task #<id>: <name> (medium)
>
> Dispatch the epic from the TUI to start working through the tasks.
