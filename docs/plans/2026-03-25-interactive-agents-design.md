# Interactive Agents Design

## Summary

Change agent dispatch from autonomous headless mode (`claude -p`) to interactive sessions that users can interact with directly. Add the ability to jump to agent tmux windows from the TUI and resume closed sessions.

## Motivation

Currently agents run autonomously in the background. The user wants to start agents in interactive sessions, steer them directly, and jump between the TUI dashboard and agent windows seamlessly.

## Design

### Dispatch changes (`dispatch.rs`)

Two functions instead of one:

**`dispatch_agent()`** — same as today (worktree creation, `.mcp.json`, tmux window) but the final step changes:
- Before: `claude -p < .claude-prompt` (headless, autonomous)
- After: interactive mode with initial task context
- Write the prompt to `.claude-prompt`, then launch via `send-keys`: `claude --prompt-file .claude-prompt` (starts interactive session with the file content as first message)
- **Implementation note:** The exact CLI flag must be verified during implementation. If `--prompt-file` doesn't exist, fallback options: (a) `claude "prompt text"` as positional arg (shell escaping concern for long prompts), (b) write prompt file and `send-keys` with `cat .claude-prompt | xargs -0 claude` or similar. The key requirement is: interactive mode, not print mode.

**`resume_agent()`** — new function for resuming a closed session:
- Takes the existing worktree path and tmux window name
- Creates a new tmux window at the worktree path (the old window is gone)
- Runs `claude --continue` via `send-keys` (not `--resume`, which opens a session picker when multiple sessions exist; `--continue` always picks the most recent conversation in the directory)
- Reuses the existing `.mcp.json` already in the worktree

**`cleanup_task()`** — the function signature changes: `tmux_window` becomes `Option<&str>`. When `Some`, it kills the window first; when `None`, it skips straight to worktree removal. This is necessary because `WindowGone` clears `tmux_window` while keeping the worktree, so cleanup may be called with no window to kill. The function is now called at two exit points (Done or backward past Ready), not on window exit.

### Window exit behavior (`tui/mod.rs`)

`Message::WindowGone(id)` no longer auto-advances Running to Review. Instead it:
- Clears `tmux_window` from the task in-memory
- Emits `Command::PersistTask` to write the cleared `tmux_window` to the database (critical: without this, the next `RefreshFromDb` tick would restore the stale value from SQLite, making the TUI think the window is still alive)
- Keeps the `worktree` field intact — the worktree outlives individual sessions
- Does **not** change task status

The agent is responsible for advancing the task to Review via the MCP server's `update_task` tool when it considers its work complete.

### Keybinding changes (`tui/input.rs`)

**`d` key — context-sensitive dispatch/resume:**

The branching logic lives in `input.rs`'s `handle_key_normal()` method. It inspects the selected task's status, `tmux_window`, and `worktree` fields to decide which message to emit:

| Task state | Behavior | Message emitted |
|---|---|---|
| Backlog or Done | Warning: "Move task to Ready before dispatching" | none (status_message only) |
| Ready | Dispatch: create worktree + new interactive session | `Message::DispatchTask(id)` |
| Running/Review with `tmux_window.is_some()` | Warning: "Agent already running, press g to jump" | none (status_message only) |
| Running/Review with `tmux_window.is_none()` and `worktree.is_some()` | Resume: new tmux window + `claude --continue` | `Message::ResumeTask(id)` |
| Running/Review with both `None` | Warning: "No worktree to resume, move to Ready and re-dispatch" | none (status_message only) |

"Live window" is determined by `task.tmux_window.is_some()`. Since `WindowGone` clears this field (and persists to DB), it's reliable without probing tmux.

The existing `Message::DispatchTask` handler in `App::update()` is simplified: it only handles Ready status. The Running/Review branching that currently exists there is removed — `input.rs` now owns that decision.

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

**New forward-cleanup code path** in the `MoveTask` handler. The existing handler only has cleanup logic for `MoveDirection::Backward`. A new branch is needed for `MoveDirection::Forward` when `new_status == TaskStatus::Done`:

- If the task has a worktree (`worktree.is_some()`), emit `Command::Cleanup` with `tmux_window` as `Option` (may be `None` if the session was closed)
- `Command::Cleanup` struct changes: `tmux_window` becomes `Option<String>` instead of `String`
- Existing backward-move cleanup also updated to use the new optional `tmux_window` pattern — it now triggers cleanup when *worktree* is present, regardless of whether `tmux_window` is set

Worktrees survive across multiple sessions through Running/Review and only get cleaned up at exit points: Done (forward) or backward past Ready.

### tmux module (`tmux.rs`)

Add `select_window(window: &str)` function that calls `tmux select-window -t <window>`.

### Execution in main loop (`main.rs`)

`execute_commands()` gets two new arms:

- `Command::Resume { task }` — mirrors `Command::Dispatch`. Spawns blocking `dispatch::resume_agent()`, sends `Message::Resumed` on success or `Message::Error` on failure.
- `Command::JumpToTmux { window }` — synchronous `tmux::select_window()` call.

### Tick loop changes (`tui/mod.rs`)

The `Tick` handler currently only emits `CaptureTmux` for tasks with `status == Running`. This must be extended to also capture for **Review** tasks that have `tmux_window.is_some()`.

Reason: an agent can update its own status to Review via MCP while the tmux window is still alive. Without monitoring Review tasks, `WindowGone` would never fire for them, leaving a stale `tmux_window` permanently. The `d` key would then show "already running" forever for a dead window.

Updated filter: `tasks.iter().filter(|t| t.tmux_window.is_some())` — capture for any task with a live window regardless of status.

## What doesn't change

- DB schema
- MCP server and handlers
- Task model / `models.rs`
- UI rendering (`tui/ui.rs`)
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
                                       |    - claude --continue
                                       |
                                       |  [g: jump]
                                       |    - tmux select-window
                                       |
                                       |  Agent calls MCP update_task
                                       |    -> status changes to Review
```
