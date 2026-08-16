# Agent-to-Agent Messaging

## Context

Agents running in separate tmux windows have no way to communicate with each other. The only inter-agent mechanism today is the post-`wrap_up` `/code-review` injection, which sends a slash command to the **same** agent's window — not to another agent.

This change adds a `send_message` MCP tool that lets any running agent send a prompt to another running agent's tmux window. Messages are delivered via file (not inline text) because `tmux send-keys -l` turns embedded newlines into Enter presses, which would submit each line as a separate Claude Code input. This is the same reason task dispatch uses `.claude-prompt` files.

Scope is fire-and-forget only — no request-response, no persistence. The receiving agent sees a short notification prompt and reads the full message from a file.

## Design

### New MCP Tool: `send_message`

Add to `src/mcp/handlers/dispatch.rs` tool definitions:

```json
{
  "name": "send_message",
  "description": "Send a message/prompt to another running agent",
  "inputSchema": {
    "type": "object",
    "properties": {
      "from_task_id": { "type": "integer", "description": "Your own task ID" },
      "to_task_id": { "type": "integer", "description": "Target agent's task ID" },
      "body": { "type": "string", "description": "Message content to send" }
    },
    "required": ["from_task_id", "to_task_id", "body"]
  }
}
```

### Handler: `handle_send_message`

New function in `src/mcp/handlers/tasks.rs`:

1. Parse `SendMessageArgs { from_task_id, to_task_id, body }` from request
2. Look up both tasks from DB via `state.db.get_task()`
3. Validate target task has `tmux_window` and `worktree` (is a running agent)
4. Format message file content:
   ```
   [Message from task {from_id}: "{from_title}"]
   {body}
   ```
5. Write to `{target_worktree}/.claude-message`
6. Inject via `tmux::send_keys(target_window, notification_prompt, runner)` where `notification_prompt` is a single-line string:
   ```
   You received a message from task {from_id}. Read .claude-message for the full content, then delete the file.
   ```
7. Signal TUI via notification channel with target task ID (for card flash)
8. Return success JSON

### Notification Channel: `()` → `McpEvent` enum

Change the notification channel from `mpsc::UnboundedSender<()>` to carry typed events.

**`src/mcp/mod.rs`** — new enum + update `McpState`:

```rust
pub enum McpEvent {
    Refresh,
    MessageSent { to_task_id: TaskId },
}
```

- `McpState.notify_tx` type changes from `Option<mpsc::UnboundedSender<()>>` to `Option<mpsc::UnboundedSender<McpEvent>>`
- Existing `notify()` helper sends `McpEvent::Refresh` (preserves current behavior)
- New `notify_message_sent(to_task_id)` sends `McpEvent::MessageSent`

**`src/runtime.rs`** — update channel creation and reception:

- Channel type: `mpsc::unbounded_channel::<McpEvent>()`
- Match on received event:
  - `McpEvent::Refresh` → existing `rt.exec_refresh_from_db(app)` behavior
  - `McpEvent::MessageSent { to_task_id }` → send `Message::MessageReceived(to_task_id)` to app, then refresh

### TUI Flash Indicator

**`src/tui/types.rs`**:
- Add `Message::MessageReceived(TaskId)` variant
- Add `message_flash: HashMap<TaskId, Instant>` field to `AgentTracking`
- Update `AgentTracking::new()` and `clear()` accordingly

**`src/tui/mod.rs`** — handle `Message::MessageReceived(task_id)`:
- Set `self.agents.message_flash.insert(task_id, Instant::now())`
- No commands needed

**`src/tui/ui.rs`** — modify `build_task_list_item` (line 505):
- Check `app.agents.message_flash.get(&task.id)` — if present and within 3 seconds, append a small badge (e.g., `" ✉"` or `" «msg»"`) to the task title on line 1
- This is additive — it doesn't replace the status indicator on line 2

Flash entries older than 3 seconds are cleaned up during tick processing in `handle_tick` or `handle_refresh_tasks`.

### Args Type

**`src/mcp/handlers/types.rs`** — add:

```rust
#[derive(Debug, Deserialize)]
pub struct SendMessageArgs {
    pub from_task_id: i64,
    pub to_task_id: i64,
    pub body: String,
}
```

### Routing

**`src/mcp/handlers/dispatch.rs`**:
- Add `"send_message"` to `tool_definitions()` list
- Add `"send_message" => tasks::handle_send_message(id, args, state)` to the match in `handle_tool_call`

## Files to Modify

| File | Change |
|------|--------|
| `src/mcp/mod.rs` | `McpEvent` enum, update `McpState` notify channel type |
| `src/mcp/handlers/types.rs` | `SendMessageArgs` struct |
| `src/mcp/handlers/tasks.rs` | `handle_send_message()` function |
| `src/mcp/handlers/dispatch.rs` | Tool definition, routing |
| `src/runtime.rs` | Channel type, `McpEvent` match arms |
| `src/tui/types.rs` | `Message::MessageReceived`, `message_flash` on `AgentTracking` |
| `src/tui/mod.rs` | Handle `MessageReceived` message |
| `src/tui/ui.rs` | Flash badge rendering on cards |

## Verification

1. `cargo build` — compiles without errors
2. `cargo test` — all existing tests pass
3. Add unit test for `handle_send_message`: mock both tasks, verify file is written and send_keys is called with correct args
4. Add unit test for error cases: target task not running, target task doesn't exist, missing worktree
5. Manual test: dispatch two agents, have one call `send_message` to the other, verify the message file appears and the prompt is injected
