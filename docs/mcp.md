# MCP & Feeds

## MCP Notification Flow

When an MCP handler mutates the database, the TUI must refresh to show the change. This is the propagation path:

```
MCP handler (e.g. handle_update_task)
  → mutates via state.task_svc / state.epic_svc   # never state.db — that's a compile error
  → calls state.notify()                          # McpState method
    → sends McpEvent::Refresh via mpsc::UnboundedSender
      → runtime event loop receives it             # tokio::select! in run_event_loop()
        → calls rt.exec_refresh_from_db(app)
          → reads all tasks/epics from DB
          → calls app.update(Message::Task(TaskMessage::Refresh(tasks)))
            → App replaces its in-memory task list, re-renders
```

Key types in the chain:
- `McpEvent` (`src/mcp/mod.rs::McpEvent`) — `Refresh` (catch-all full reload), `TaskChanged(TaskId)` / `EpicChanged(EpicId)` (targeted single-row reloads, preferred when the changed entity is known)
- `McpState::notify()` — fire-and-forget send on the channel
- `TuiRuntime::exec_refresh_from_db()` (`src/runtime/tasks.rs`) — reloads tasks, epics, and usage from DB
- `TaskMessage::Refresh` (`src/tui/messages/task.rs`) — carries the fresh task list into the App, wrapped as `Message::Task`

Agent-to-agent messaging (task #4098) no longer has an `McpEvent` of its own —
agents call Claude Code's native `SendMessage` directly, and dispatch observes
it via the Claude Code hook pipeline rather than through this MCP-server
notification channel. The observed timestamps
(`Task.last_peer_message_sent_at`/`last_peer_message_received_at`) ride the
ordinary DB-refresh path above instead: `detect_task_transition_notifications`
(`src/tui/update/agent.rs`) diffs them against the previous in-memory row on
each refresh and flashes `AgentTracking::message_flash_sent`/`message_flash`
accordingly. See `HookPeerMessageSent` in `docs/specs/agent-health.allium`.

## MCP State Machines

Some MCP tools drive multi-call handshakes via in-memory state on `McpState`. The state is **not persisted** — a process restart loses it, and the agent will start the handshake from scratch on its next call.

**`wrap_up` → `exit_session` handoff** (`src/mcp/handlers/tasks/wrap_up.rs`):

`wrap_up(task_id, action)` issues an `ExitToken { token, action }` (`src/mcp/mod.rs`, keyed by `TaskId` in `McpState::exit_tokens: RwLock<HashMap<TaskId, ExitToken>>`), recording which action (`rebase` | `done` | `pr`) issued it. For `rebase` this call also performs the actual git rebase/fast-forward synchronously; for `done`/`pr` it performs no mutation at all. The task's `status` is unchanged by `wrap_up` in every case — it stays whatever it was (`running`) until the closing call.

The `/retro` skill (the mandatory reflection step — there is no in-handler reflection prompt anymore) does **not** run between these two calls. The `/wrap-up` skill invokes it earlier, before its commit step and ahead of `wrap_up`, so any agent-context fix retro makes is committed with the session's work rather than stranded after the rebase or the push.

`exit_session(task_id, token, action, pr_url?)` is a **single call** that:
1. Validates the token, and that `action` matches the action recorded on the token (mismatch → error naming both actions, no mutation).
2. Requires `pr_url` iff `action = "pr"`.
3. Requires `task.tmux_window` to still be set (if some other path already tore the session down, this errors with "no active session" instead of mutating).
4. Applies the terminal mutation as **one patch** carrying status, sub-status, `url` and the cleared `tmux_window`: `rebase`/`done` → `status = Done`; `pr` → `status = Review`, `url` set to the pr-typed URL. The token is consumed before this patch is attempted. **Only if that write persisted** does it then kill the tmux window — the teardown is gated on the same write as the mutation, so window and `tmux_window` reference can never disagree.
5. **On a failed write** (`close_persisted = false` in the specs) the failure is logged at `warn`, the task/epic-changed notification still goes out, and the call returns a **successful** JSON-RPC response whose text says the close did not take effect, the tmux session is still alive, and the task needs closing by hand. It is deliberately not an error: the token was already consumed, so an error would strand the agent with no retry path. Nothing is torn down — the window survives, still hosting a live agent the human can attach to, and because `tmux_window` stays set the task can never satisfy `is_detached` and drift into the awaiting-merge rendering looking finished. `SessionClosed` is not emitted, so step 6 does not run.
6. **Last**, and only on a close that persisted, if the task has an `epic_id` and that epic has `auto_dispatch` on, dispatches the epic's next backlog subtask (`auto_dispatch_next` in `src/mcp/handlers/tasks/dispatch.rs`) and names it in the response text. Placing this after every mutation above is the point: the successor's worktree is cut from a `base_branch` that already contains this task's work. The chain is fire-and-forget — a missing epic, a claim error or a failed dispatch is logged at `warn` and never turns a successful close into an error. The `auto_dispatch` read also **fails closed**: a DB error fetching the epic stops the chain, because nothing requested it and a hiccup must not launch an agent on an epic whose operator turned chaining off. Withholding the chain on a failed close is the same reasoning — compounding a broken close into a second dispatch is harder for a human to notice than the broken close alone. There is deliberately no agent-facing tool for it; the selection is made exclusive by an atomic select-and-claim (`try_claim_next_backlog_task`), so two concurrent closes on one epic can never launch two agents for the same subtask. That exclusivity is not chain-only: `dispatch_task` and the TUI dispatch key take the by-id twin of the claim (`TaskService::claim_backlog_task`) before they provision, so no two entry points can provision one task. A dispatch that then fails releases the claim. See `AutoDispatchNextSubtask` in `docs/specs/epics.allium` and `DispatchClaimExclusive` in `docs/specs/dispatch.allium`.

This closes a race that existed when `wrap_up("pr")` used to set `status = review` immediately: that armed PR-merge polling (`PollPrStatus`) before the session was actually closed, so a merge/close could null `tmux_window` while the agent was still working. Now a PR task never becomes poll-visible until the exact same call that also ends the session.

A crash before the closing call leaves no stranded DB state — the task simply hasn't transitioned yet, and a stale token is simply never consumed (the in-memory map is not persisted, so it's gone on restart anyway).

Do not add new ad-hoc state machines on `McpState` without documenting them here.

## MCP Error Codes

MCP handlers in `src/mcp/handlers/` return JSON-RPC error objects using two codes:

| Code | Meaning | When to use |
|------|---------|-------------|
| `-32602` | Invalid params | Validation failure, missing required field, unknown tool name — maps to `ServiceError::Validation` |
| `-32603` | Internal error | Unexpected DB error, I/O failure — maps to `ServiceError::Internal` or `anyhow` errors |

Use `JsonRpcResponse::err(id, -32602, msg)` for anything the caller can fix; use `-32603` for anything they can't.

## Notifications

JSON-RPC 2.0 §4.1 forbids replying to a Notification (a request with no `id`). The MCP streamable-HTTP transport maps this to `HTTP 202 Accepted` with an empty body. `handle_mcp` short-circuits any request where `id.is_none()` to a 202 — including unknown methods. Claude Code sends `notifications/initialized` after every `initialize`; replying to it (even with an error) makes its strict response schema reject `id: null` and aborts the MCP session.

## Debugging MCP handlers

The MCP server listens on port 3142 by default (override with `DISPATCH_PORT`). When a handler misbehaves you can reproduce it without going through Claude Code:

```bash
# Tail server logs while the TUI runs (logs go to stderr; redirect when launching)
RUST_LOG=dispatch=debug cargo run -- tui 2> /tmp/dispatch.log
tail -f /tmp/dispatch.log

# Send a manual JSON-RPC request to a tool (e.g. list_tasks)
curl -s -X POST http://127.0.0.1:3142 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_tasks","arguments":{}}}' \
  | jq

# Reproduce a failing update — substitute the offending arguments
curl -s -X POST http://127.0.0.1:3142 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"update_task","arguments":{"task_id":42,"status":"done"}}}' \
  | jq
```

`tools/list` returns the tool schemas — useful when the argument shape isn't obvious.

## Feed Epics

Feed epics are epics whose tasks are populated externally by a shell command rather than by a human. When an epic has a `feed_command` set, the runtime runs it periodically (`feed_interval_secs`) and calls `upsert_feed_tasks()` to sync the results. Each feed task has an `external_id` that is used as the upsert key — tasks are created on first appearance and updated (but not deleted) on subsequent runs.

Feed tasks appear in their own column on the kanban board (`SubStatus::Feed`). The schema is backed by migration v38. See `docs/specs/feeds.allium` for the full specification.

## Knowledge Base Flow

The Knowledge Base lets dispatched agents record knowledge entries that are automatically injected into future dispatch prompts.

### End-to-end lifecycle

1. **Agent records** — calls `record_learning(task_id, kind, summary, scope, ...)` during a task or at wrap-up. The entry is immediately active and will appear in future dispatch prompts for agents working in the matching scope.
2. **Future dispatches** — when an agent is launched, `dispatch_with_prompt()` queries approved entries for the task's context and prepends them to the prompt (see `docs/specs/learnings.allium`).
3. **Agent rates** — calls `rate_learning(learning_id, task_id, verdict)` when it acts on a retrieved entry. `helped` increments `upvote_count` (raising the entry's priority in future results); `wrong` decrements `upvote_count` (a downvote; may go negative) and leaves the status unchanged. Only entries surfaced to the task (injected or returned by `query_learnings`) can be rated. There is no human-approval step and no human-facing curation surface: entries land approved, and curation happens through MCP (`rate_learning`, `delete_learning`) plus a background job that archives approved entries with a non-positive score that have gone stale (see `docs/specs/learnings.allium`: `ArchiveStaleLearning`).

### Scope model

Each learning has a `scope` that determines which tasks receive it:

| Scope | Included when | `scope_ref` |
|-------|---------------|-------------|
| `user` | Always | `null` |
| `repo` | Task's repo path matches | `repo_path` |
| `epic` | Task belongs to this epic | `str(epic_id)` |
| `task` | Only via explicit `query_learnings` | `str(task_id)` |

`scope_ref` is auto-derived from the task context when omitted. `task`-scoped entries are excluded from auto-injection (they capture task-specific outcomes and must be fetched on demand).

### Ordering

Candidate entries are fetched from SQLite ordered by kind (`procedural` first), then scope specificity (`epic` → `repo` → `user`), then `upvote_count DESC`. That ordering only selects the candidate set: the injected block itself is RAG-ranked by relevance to the task, so `kind` confers no precedence in the final prompt. Procedural entries are **not** prepended as a verbatim prefix — every retrieved entry, procedural included, goes into the single ranked block.

### Status lifecycle

```
approved → archived (terminal)
```

Approved entries affect dispatch; archived entries do not. `rejected` remains a valid `LearningStatus` because existing rows still carry it, but nothing writes it any more — the reject path was removed with the TUI overlay.

### Implementation references

- `src/mcp/handlers/learnings.rs` — MCP tool handlers
- `src/service/learnings.rs` — `LearningService` (create, query, retrieval/verdicts, stale sweep, delete)
- `src/db/` — `LearningStore` trait, `LearningPatch`, `LearningFilter`
- `src/dispatch/agents.rs` — prompt augmentation in `dispatch_with_prompt()`
- `docs/specs/learnings.allium` — full domain specification
