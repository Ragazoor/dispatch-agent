# Design: relay agent-initiated `send_message` onto native cross-session messaging

Task: #4098 (redirected mid-session from hardening dispatch's own transport)
Date: 2026-08-15

## Context

Original scope (`2026-08-15-send-message-delivery-hardening-design.md`): harden
dispatch's own file+tmux-send-keys transport against injecting keystrokes into
a pane that isn't at its normal chat input. Reproduced concretely (see that
doc). While reviewing that design, the direction changed: rather than making
the existing transport safer, **agent-initiated messaging moves onto Claude
Code's own native cross-session messaging** (`ListAgents`/`SendMessage`,
shipped v2.1.224+, confirmed live in this environment). Native delivery is
queued and read **between tool calls** — it structurally cannot land on a
permission prompt or dialog, because it never touches the pane's raw stdin at
all. It also gives the sender a real delivered/held/expired notice, which is
strictly better confirmation than anything achievable by watching `send-keys`
exit codes.

This does **not** cover task-watcher completion notices — see the scope note
at the top of the original design doc for why that path is unaffected and
still needs the pane-content-probe hardening.

#3983 (`docs/plans/3983-cross-session-messaging-investigation.md`) evaluated
and rejected swapping *dispatch's own* transport for a direct write to the
native inbox socket, because the wire format and session registry were
undocumented and dispatch's MCP server can't clear the own-child verification
check. Re-checked today: the socket's **auth frame** is now documented
(`{"type":"auth","token":"<token>"}` over `CLAUDE_CODE_MESSAGING_SOCKET`,
confirmed present in this very session's environment), but the **message
payload frame after auth is still undocumented**, and dispatch's server still
isn't a verified own-child of a target session. That path stays closed. What's
different here is *not* dispatch talking to the socket — it's the **agent
itself** calling the native `SendMessage` tool, which is a fully documented,
ordinary tool call. Dispatch's role shrinks to observation, not delivery.

## What dispatch keeps vs. loses

| Capability | Today (`send_message` MCP tool) | Native relay |
|---|---|---|
| Delivery | file write + tmux send-keys (the thing being hardened) | Claude Code's own queued, between-tool-calls delivery |
| Confirmation | none (exit 0 = keys accepted) | real delivered/held/expired notice to the sender, for free |
| Addressing | task ID, validated against the board | session name (`ListAgents`/`@mention`), not board-validated unless dispatch keeps a wrapper — see Decision 1 |
| Audit trail | automatic (every dispatch MCP call is trajectory-recorded) | lost unless reconstructed via hook (see below) |
| TUI flash | push-based, in-process channel, receiver only | needs a new mechanism entirely (see below) — this doc adds **sender flash too** |
| Gating | none — dispatch's own transport | inherits native messaging's platform/provider/env-var gating (macOS/Linux only, off on Bedrock/GCP-Agent-Platform/Foundry, off under `DO_NOT_TRACK` etc., off in `--bare`) |

## Decision 1: does dispatch keep a `send_message` MCP tool at all?

**Option A — thin orchestrator.** `send_message` stays as an MCP tool: it
validates `from_task_id`/`to_task_id` against the board (today's guard rail),
writes the trajectory record itself, flashes the sender's card, and returns a
tool result instructing the calling agent to now call
`SendMessage(name: "task-<to_id>", message: <body>)`. Keeps board-validated
addressing and a guaranteed audit write. Costs: an extra synchronous MCP round
trip on every send, and the actual delivery step depends on the model
following through on the instruction in the tool result — not a function call
dispatch's own code performs, so it's "very likely" rather than guaranteed.

**Option B — remove it, pure prompt guidance.** Delete the `send_message` MCP
tool. Update agent-facing prompt/skill guidance (`src/dispatch/prompts.rs`) to
tell agents to use native `ListAgents`/`SendMessage` directly for sibling
coordination. Dispatch never sits in the call path; it only *observes* via the
hook pipeline (below) to drive the TUI and reconstruct an audit entry. Fully
"peer to peer," zero added latency, no reliance on a model following an
instructions-in-tool-result step. Costs: no board-side validation that the
target task ID is even real/running before the agent tries to address it (the
agent already has to name a *session*, discovered via `ListAgents`, so a
stale/wrong target mostly self-corrects — `ListAgents` won't show a session
that isn't there — but dispatch itself no longer gate-checks it).

**Decided: Option B.** Remove the `send_message` MCP tool entirely. Agents use
native `ListAgents`/`SendMessage` directly per prompt guidance; dispatch never
sits in the call path and only observes via the hook pipeline below.

## Session naming

Both per-task agent launch sites in `src/dispatch/agents.rs` (initial dispatch
and the `--continue` resume relaunch — the board's own main session is
unrelated and untouched) need a stable `--name task-<id>` flag so
`ListAgents`/`@mention` addressing is deterministic instead of Claude Code's
auto-derived-from-cwd-folder name. `DISPATCH_PLUGIN_DIR` is currently a fixed
`concat!` string constant interpolated into the launch shell line
(`src/dispatch/prompts.rs:28`) — the per-task name has to become a
runtime-built string threaded into that same interpolation, at both call
sites. Mechanical, but touches a string every existing mock test currently
pins as a constant substring.

## TUI visibility: flash both sender and receiver

Today's flash is push-only: `state.notify_message_sent` sends an in-process
`McpEvent::MessageSent` that only exists inside the combined TUI+MCP-server
process. A `dispatch hook <id> ...` invocation is a **separate process** with
its own fresh DB connection — it cannot reach that channel. So observing a
native `SendMessage` call via the hook needs a DB-persisted signal instead,
picked up by the TUI's existing periodic DB-refresh, the same way other
transient tick-driven state is derived today.

Proposed mechanism:

1. **Hook extraction.** `plugin/hooks/scripts/task-status-hook` already parses
   `tool_name`/`tool_input` via `jq` for other tools (e.g. `Read`/`Write` file
   paths) and already does **not** filter out `SendMessage` (only
   `mcp__dispatch__*` is filtered). Add a case: when `tool_name == "SendMessage"`,
   pull `tool_input.name` (target) and `tool_input.message` (body), and forward
   both to `dispatch hook <id> peer-message --target <name> --body <body>`
   (new hook subcommand, alongside the existing PreToolUse/PostToolUse/
   Notification/Stop ones).
2. **Target resolution.** The CLI handler parses `task-<id>` back out of the
   target name (or logs and no-ops if it isn't one of dispatch's own agents —
   a message to an unrelated local session is not dispatch's concern).
3. **Two new nullable timestamp columns** on `tasks`: `last_peer_message_sent_at`
   (stamped on the *sending* task's own row — the hook already knows `<id>`)
   and `last_peer_message_received_at` (stamped on the *resolved target's* row).
   Both writes happen from the one hook invocation, on the sending side —
   nothing on the receiving side needs to fire anything, which sidesteps an
   open question the research turned up (whether a receiver-side hook fires at
   all when a peer message is delivered into an idle session — undocumented
   and unconfirmed). We don't need it: the sender's own tool call already tells
   us definitively that a send targeting that task occurred.
4. **TUI diff-detection.** Extend the existing DB-refresh tick (which already
   re-fetches every task each cycle) to compare each task's new
   `last_peer_message_sent_at`/`last_peer_message_received_at` against the
   previously held in-memory value; a change inserts into the flash state,
   mirroring how `message_flash: HashMap<TaskId, Instant>` works today but
   keyed with a direction so rendering can distinguish them.
5. **Rendering.** A received-message flash keeps today's envelope glyph
   (✉). A sent-message flash gets its own glyph — **decided: a paper-plane /
   outgoing-arrow glyph (➤)** — with the same warm fill and neutral border —
   per `docs/specs/core.allium`'s existing "Message flash" reasoning, a third
   *border* vocabulary is explicitly the thing not to do, so the distinction
   stays glyph-only, consistent with how the existing flash already avoids
   colouring the border. Exact codepoint is an implementation-plan detail, not
   a design one.

## Audit trail reconstruction

`src/mcp/trajectory.rs`'s recording is automatic for every call that reaches
dispatch's own MCP server; a native `SendMessage` call never does, so it's
invisible to trajectory recording today. Since `dispatch hook`'s CLI process
can call `trajectory::append_entry` directly (confirmed — it's an async file
append, not exclusive to the long-running server), the new `peer-message` hook
handler should append a trajectory entry itself, keyed by the sending task's
`CallerIdentity::Task`, recording the target and body. This restores audit
parity without going through the MCP server at all.

## Documentation obligations

- `docs/specs/mcp-task-tools.allium::SendMessageViaMcp` — replaced (Option B)
  or rewritten to describe the thin-orchestrator instruction contract
  (Option A), depending on Decision 1.
- `docs/specs/agent-health.allium` — new `HookEventKind` variant/rule for the
  peer-message observation, following the `Notification(Option<NotificationKind>)`
  precedent for a hook event carrying structured data.
- `docs/specs/core.allium`'s "Message flash" section — extend for the
  sent/received distinction and the new glyph.
- `src/dispatch/prompts.rs` epic/agent guidance — if Option B, point agents at
  native `ListAgents`/`SendMessage` explicitly.

## Decisions locked

1. `send_message` MCP tool: **removed** (Option B).
2. Sent-flash glyph: **paper-plane/outgoing-arrow**, exact codepoint TBD at
   implementation time.
3. No fallback to the old file+tmux transport for `send_message` when native
   messaging is unavailable (platform/provider/env-var gating) — not carrying
   two code paths for an edge case. If this proves to matter in practice, it's
   a follow-up task, not scope here.

## Test strategy sketch (to firm up once decisions land)

- Unit: session-name construction/threading in `src/dispatch/agents.rs` (mock
  argv assertions, updated for the new `--name` substring).
- Unit: `peer-message` hook CLI parsing and target-name decoding (including the
  "not a dispatch session name" no-op case).
- DB: migration adds the two columns; a test writes both and reads them back.
- TUI: tick-diff detection inserts into the flash state on a changed
  timestamp, and renders the correct glyph per direction — likely alongside
  the existing `message_flash` tests in `src/tui/tests/rendering.rs` per
  `core.allium`'s "Where these rules are enforced" convention.
- Trajectory: hook-driven `peer-message` produces an entry equivalent to
  today's MCP-driven one.
- No real-tmux layer needed here — this design never touches `send-keys` for
  agent-initiated messages at all, which is the point.
