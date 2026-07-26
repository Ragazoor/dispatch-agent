# Automatic Task Status via Claude Code Hooks

## Problem

Agents dispatched from the TUI often forget to update their task status when they finish or need input. The kanban board gets out of sync with reality.

## Solution

Use Claude Code hooks (`Stop` and `UserPromptSubmit`) written into the agent's worktree to automatically manage status transitions. Complement with TUI-side status updates on resume.

### Status lifecycle

```
Dispatch (TUI)         --> Running
Agent stops            --> Review  (Stop hook)
User types in tmux     --> Running (UserPromptSubmit hook)
Agent stops again      --> Review  (Stop hook)
Resume from TUI (d)    --> Running (TUI sets status)
Move to Done (TUI)     --> Done    (manual)
```

## Changes

### 1. Write `.claude/settings.json` into the worktree

**File:** `src/dispatch.rs` — `dispatch_agent()`

After writing `.mcp.json` (step 3), also write `.claude/settings.json` with two hooks. The task ID and DB path are known at dispatch time and baked into the commands.

```json
{
  "hooks": {
    "Stop": [
      {
        "type": "command",
        "command": "task-orchestrator --db <db_path> update <task_id> review"
      }
    ],
    "UserPromptSubmit": [
      {
        "type": "command",
        "command": "task-orchestrator --db <db_path> update <task_id> running"
      }
    ]
  }
}
```

The `--db` flag ensures the hook hits the same database regardless of working directory or environment. The dispatch function already receives `task.id` and the DB path can be threaded through.

**Note:** Create the `.claude/` directory in the worktree before writing the file.

### 2. Simplify the agent prompt

**File:** `src/dispatch.rs` — `build_prompt()`

Remove the instruction telling the agent to update status to review. Replace with:

> "Task status transitions (running/review) are managed automatically via hooks. Use the MCP server to post notes as you work."

This prevents the agent from fighting the hooks while keeping MCP awareness for `add_note`.

### 3. Set status to Running on resume

**File:** `src/tui/mod.rs` — `handle_resumed()` (or the `Resumed` match arm)

Currently `Resumed` only sets `task.tmux_window`. Also set `task.status = TaskStatus::Running` before persisting, matching the pattern in `Dispatched`.

### 4. Thread DB path into dispatch

**File:** `src/dispatch.rs` — `dispatch_agent()` signature

Add a `db_path: &str` parameter so the hook commands can include `--db <path>`.

**File:** `src/runtime.rs` — `TuiRuntime`

Add a `db_path: PathBuf` field to `TuiRuntime`. The path is already available in `run_tui()` — store it when constructing `TuiRuntime`, then pass it to `dispatch_agent()` in the `Dispatch` command handler.

## Testing

- **`build_prompt` test:** Assert the prompt no longer contains "update the task status to 'review'" and instead mentions automatic hooks.
- **`Resumed` handler test:** Assert that after `Message::Resumed`, the task status is `Running`.
- **Hook content test:** Unit test that the generated `.claude/settings.json` content is valid JSON containing the expected hook commands with the correct task ID and DB path.
- **Integration (manual):** Dispatch an agent, let it stop, verify the task moves to review. Type input, verify it moves to running.

## Out of scope

- Distinguishing "done" vs "needs input" in review status (both show in Review column; agent can add notes to explain)
- Removing `update_task` from the MCP tool list (agent may still have legitimate reasons to call it)
