# Interactive Agents Design

## Summary

Change agent dispatch from autonomous headless mode (`claude -p`) to interactive sessions (`claude --prompt`) that users can interact with directly. Add the ability to jump to agent tmux windows from the TUI and resume closed sessions.

## Motivation

Currently agents run autonomously in the background. The user wants to start agents in interactive sessions, steer them directly, and jump between the TUI dashboard and agent windows seamlessly.

## Design

### Dispatch changes (`dispatch.rs`)

Two functions instead of one:

**`dispatch_agent()`** — same as today (worktree creation, `.mcp.json`, tmux window) but the final step changes:
- Before: `claude -p < .claude-prompt` (headless, autonomous)
- After: `claude --prompt "..."` (interactive, with initial task context)
- No prompt file needed; the prompt is passed inline via the CLI flag

**`resume_agent()`** — new function for resuming a closed session:
- Takes the existing worktree path and tmux window name
- Creates a new tmux window at the worktree path (the old window is gone)
- Runs `claude --resume` via `send-keys`
- Reuses the existing `.mcp.json` already in the worktree

**`cleanup_task()`** — unchanged, but now only called at two exit points (Done or backward past Ready), not on window exit.

### Window exit behavior (`tui/mod.rs`)

`Message::WindowGone(id)` no longer auto-advances Running to Review. Instead it:
- Clears `tmux_window` from the task (in-memory and DB) so we know there's no live session
- Keeps the `worktree` field intact — the worktree outlives individual sessions
- Does **not** change task status

The agent is responsible for advancing the task to Review via the MCP server's `update_task` tool when it considers its work complete.

### Keybinding changes (`tui/input.rs`)

**`d` key — context-sensitive:**

| Task state | Behavior |
|---|---|
| Ready | Dispatch: create worktree + new interactive session |
| Running/Review with live window | Warning: "Agent already running, press g to jump" |
| Running/Review with no window, has worktree | Resume: new tmux window + `claude --resume` |

"Live window" is determined by `task.tmux_window.is_some()`. Since `WindowGone` clears this field, it's reliable without probing tmux.

**`g` key — jump to tmux window:**

| Task state | Behavior |
|---|---|
| Has `tmux_window` | `tmux select-window -t <window>` — switches focus, TUI keeps running |
| No window | Status message: "No active session" |

### New Message/Command variants (`tui/mod.rs`)

Follows the same 3-step async pattern as dispatch:

```
Message::ResumeTask(id)
  -> App::update() checks task has worktree, no window
    -> Command::Resume { task }
      -> execute_commands() spawns blocking: dispatch::resume_agent()
        -> Message::Resumed { id, tmux_window }
          -> App::update() sets task.tmux_window, persists to DB
```

New variants:
- `Message::ResumeTask(id)` — triggered by `d` key on resumable task
- `Message::Resumed { id, tmux_window }` — async result from resume_agent
- `Command::Resume { task }` — executed in main loop
- `Command::JumpToTmux { window }` — executed in main loop

### Cleanup on Done (`tui/mod.rs`)

When a task moves forward into Done via `MoveTask::Forward`:
- If the task has a worktree, emit `Command::Cleanup` (kills tmux window if alive + removes worktree + deletes branch)
- Existing backward-move cleanup stays as-is (moving backward from a dispatched state still tears down)

Worktrees survive across multiple sessions through Running/Review and only get cleaned up at exit points: Done (forward) or backward past Ready.

### tmux module (`tmux.rs`)

Add `select_window(window: &str)` function that calls `tmux select-window -t <window>`.

### Execution in main loop (`main.rs`)

`execute_commands()` gets two new arms:

- `Command::Resume { task }` — mirrors `Command::Dispatch`. Spawns blocking `dispatch::resume_agent()`, sends `Message::Resumed` on success or `Message::Error` on failure.
- `Command::JumpToTmux { window }` — synchronous `tmux::select_window()` call.

## What doesn't change

- DB schema
- MCP server and handlers
- Task model / `models.rs`
- UI rendering (`tui/ui.rs`)
- Tick loop (still captures tmux output for live sessions)
- Prompt content (same task context, just delivered differently)

## Task lifecycle

```
Backlog -> Ready -> [d: dispatch] -> Running -> Review -> [m: move] -> Done
                                       |          |                     |
                                       |          |                  cleanup
                                       |          |               (worktree +
                                       |          |                tmux + branch)
                                       +----------+
                                       |  Session closes:
                                       |    - tmux_window cleared
                                       |    - worktree preserved
                                       |    - status unchanged
                                       |
                                       |  [d: resume]
                                       |    - new tmux window
                                       |    - claude --resume
                                       |
                                       |  [g: jump]
                                       |    - tmux select-window
                                       |
                                       |  Agent calls MCP update_task
                                       |    -> status changes to Review
```
