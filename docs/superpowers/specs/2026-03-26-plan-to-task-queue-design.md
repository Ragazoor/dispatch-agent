# Plan-to-Task Queue — Design Spec

## Problem

After brainstorming and writing an implementation plan, the superpowers workflow transitions to execution (subagent-driven-development / executing-plans). There is no way to defer execution — to create a task in the task orchestrator's kanban board and dispatch it later from the TUI.

## Solution

Two components that bridge plan writing to the task orchestrator:

1. **`task-orchestrator create --from-plan`** — A CLI subcommand that parses a plan markdown file, extracts metadata, and creates a Ready task in SQLite.
2. **`/queue-plan` skill** — A Claude Code skill that finds the current session's plan file and invokes the CLI.

## Workflow

1. Brainstorm and approve design spec
2. Writing-plans skill produces `docs/superpowers/plans/YYYY-MM-DD-<name>.md`
3. User invokes `/queue-plan` instead of transitioning to execution
4. Skill runs CLI, task appears in kanban Ready column
5. User dispatches from TUI whenever they choose

## Component 1: CLI Subcommand

### Interface

```
task-orchestrator create --from-plan <path> [--repo-path <path>] [--title <override>] [--description <override>]
```

### Plan File Parsing

Plan files follow the writing-plans skill format:

```markdown
# Title Here — Implementation Plan

**Goal:** One-line description of what this plan accomplishes.
```

Extraction rules:
- **Title**: H1 heading content, with trailing ` — Implementation Plan` stripped if present
- **Description**: Content of the `**Goal:**` line, with the `**Goal:**` prefix removed
- Both fields can be overridden via `--title` and `--description` flags

### Task Creation

- `title` — extracted from plan or `--title` flag
- `description` — extracted from plan or `--description` flag
- `repo_path` — from `--repo-path` flag, defaulting to current working directory
- `plan` — absolute path to the plan file
- `status` — always `Ready` (the task has a plan and is dispatchable)

### Idempotency

Before creating, query the database for an existing task with the same `plan` path. If found, print the existing task info and exit successfully without creating a duplicate.

```
# First run:
$ task-orchestrator create --from-plan docs/superpowers/plans/2026-03-26-feature.md
Created task #12: "Feature Name" [ready]

# Subsequent runs:
$ task-orchestrator create --from-plan docs/superpowers/plans/2026-03-26-feature.md
Task #12 already exists for this plan [ready]
```

### Error Cases

- Plan file not found — exit with error
- H1 heading missing — exit with error asking for `--title` flag
- Goal line missing — create task with empty description (description is optional for tasks)
- Database unreachable — exit with error

## Component 2: `/queue-plan` Skill

A custom Claude Code skill in the user's plugin.

### Trigger

User invokes `/queue-plan`, or says "queue this plan", "create task from plan", "send to orchestrator".

### Behavior

1. **Find the plan file:**
   - If argument provided (`/queue-plan path/to/plan.md`), use that path
   - Otherwise, find the most recently modified `.md` file in `docs/superpowers/plans/`
2. **Determine repo path:** Use the current working directory
3. **Run CLI:** `task-orchestrator create --from-plan <absolute-path> --repo-path <cwd>`
4. **Report result:** Show the created task ID and title, or report that the task already exists

### Edge Cases

- No plan file found in `docs/superpowers/plans/` and no argument given — ask the user for the path
- `task-orchestrator` binary not on PATH — clear error message explaining how to install/build
- Task already exists (idempotency) — report existing task, not an error

## What Does NOT Change

- No modifications to the superpowers plugin (brainstorming, writing-plans, executing-plans)
- No changes to the TUI rendering, MCP server, or dispatch logic
- No new Rust dependencies
- No database schema changes (uses existing `plan` column and `create_task` method)

## Implementation Scope

### Rust Changes

| File | Change |
|------|--------|
| `src/main.rs` | Add `Create` variant to `Commands` enum with clap args |
| `src/main.rs` | Add match arm that calls plan parser + `db.create_task()` |
| New: `src/plan.rs` | Plan file parser (~30 lines): read file, extract H1 + Goal |
| `src/db.rs` | Add `find_task_by_plan(&self, plan: &str) -> Result<Option<Task>>` for idempotency check |

### Skill

| File | Change |
|------|--------|
| New: plugin skill file | `/queue-plan` skill definition with frontmatter and instructions |

### Tests

- Unit tests for plan file parser (valid plan, missing H1, missing Goal, extra whitespace)
- Unit test for `find_task_by_plan` DB method
- Integration: `create --from-plan` with a temp plan file, verify task in DB
- Integration: idempotency — run twice, verify single task
