# Task #3983 — Claude Code cross-session messaging vs. dispatch `send_message`

Investigation only. No behaviour change, no spec change. Recommendation at the bottom.

## 1. What the native feature is

Claude Code shipped **cross-session messaging** in **v2.1.224 (2026-08-07)**, macOS + Linux only.
Two model-facing tools: `ListAgents` (discover reachable agents) and `SendMessage` (deliver plain
text to one by name). Docs: <https://code.claude.com/docs/en/cross-session-messaging>.

Mechanics that matter here:

| Property | Behaviour |
|---|---|
| Discovery | Each session registers itself in files under the Claude config dir and binds a **per-session Unix inbox socket**. `ListAgents` / `/list-agents` reads those files. Same-filesystem only (a container can't see the host's sessions). |
| Addressing | By session **name** — `--name`/`/rename`, else auto-derived from the cwd folder name plus a 2-char suffix (e.g. `3989-an-empty-feed-…-d0`). Colliding names get a short `[ref]`. |
| Delivery | Queued to the receiver's inbox and read **between tool calls**; a running tool is never interrupted. Idle receivers start a new turn. |
| Payload | Plain text only. No files, no history, no structured payload. |
| Inbound control | `crossSessionInbound` = `accept` / `hold` / `refuse`. With no value set, the default is derived from both sides' permission classes: a receiver that *prompts* for permissions (dispatch agents — they run default/auto, no `--permission-mode` flag) is **delivered** to; a receiver that bypasses prompts **holds** for approval. |
| Sender feedback | Sender is told when a message is held, and later whether it was delivered / denied / expired. Failed writes are errors, not silent. |
| Safety | A peer message never counts as user consent, can't change config, slash commands arrive as inert text, receiver's own permission prompts still fire. Loops are rate-limited; unread queue caps at 50. |
| Gating | Off on native Windows; off on Bedrock / GCP Agent Platform / Microsoft Foundry; **off when `DISABLE_TELEMETRY`, `DO_NOT_TRACK`, `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` or `DISABLE_GROWTHBOOK` disable feature-flag evaluation**; off in `--bare` sessions. |

**Confirmed live on this machine.** `ListAgents` from this dispatch worktree listed the three other
running dispatch agent sessions, each with its tmux pane, an `interactive` kind, and a
`busy`/`waiting` state. So dispatch-launched agents already bind inbox sockets and are already
mutually reachable — with no dispatch involvement at all.

## 2. What dispatch does today

`send_message` (MCP) → `src/service/tasks/crud.rs::validate_send_message` → `src/notify.rs::deliver`:

1. Write the body to `<target_worktree>/.claude-messages/<from_id>-<ms>-<seq>.md`.
2. `tmux send-keys` a one-line nudge into the target window telling the agent to read that file and
   delete it.

Spec: `SendMessageViaMcp` in `docs/specs/mcp-task-tools.allium`. The same `notify::deliver`
transport also carries task-watcher completion notices (`docs/specs/task-watchers.allium`), and
`send_message` is recorded in the trajectory log (`docs/specs/observability.allium`) and flashes the
target card in the TUI via `McpEvent::MessageSent`.

Where the tmux transport is genuinely weaker than the native one:

- **Keystroke injection is state-dependent.** `send-keys` types into whatever the pane is showing.
  If the target agent has a permission prompt, a plan-mode dialog, or a pager open, the text and the
  Enter land there instead of the prompt box.
- **No delivery confirmation.** `send-keys` returning 0 means tmux accepted keystrokes, nothing more.
- **Two extra tool round-trips** on the receiving side (Read the file, then delete it), and the
  delete is prompt-dependent, so `.claude-messages/` accumulates litter.
- **No loop/rate protection.**

What dispatch has that the native feature does not: **task-ID addressing** validated against the
board, the **TUI flash badge**, the **trajectory record**, and a transport that is version- and
provider-independent.

## 3. Can dispatch piggyback? Three candidate seams

**(a) Swap the transport — deliver over the target session's inbox socket.**
This is the attractive one: keep `send_message`, its task-ID addressing, the flash badge and the
trajectory record, and only replace `notify::deliver`'s tmux leg with a socket write. The docs
explicitly bless external posting — the inbox-socket section is to be read "when you want a script
or hook to post into a session", and the path is exported to hooks and Bash as
`CLAUDE_CODE_MESSAGING_SOCKET`.

**Blocked today**, on two counts:
- The **wire format is not documented** anywhere, and neither is the on-disk session-registry format
  dispatch would need to map `task_id`/worktree → socket path. Both are internal formats; the docs
  already warn that comparable internals (transcript JSONL) "change between versions". Dispatch
  would be reverse-engineering a private protocol into a load-bearing delivery path.
- `CLAUDE_CODE_MESSAGING_SOCKET` is only exported **to a session's own children**, and own-child
  verification is what makes unapproved delivery work. Dispatch's MCP server is not a child of the
  target agent's session, so it would post as an unverified external sender.
- There is no CLI surface to lean on instead: `claude --help` has no message/send subcommand
  (`agents`, `auth`, `mcp`, `plugin`, `project`, … only).

*Caveat on evidence:* I could not probe the socket or the registry empirically — this sandbox's
command classifier blocked every read under the Claude config dir and every `env`/`printenv` call.
The protocol conclusion rests on the public docs, which is the part that matters for depending on it.

**(b) Delegate agent↔agent chat to native `SendMessage`.**
Change the epic prompt guidance (`src/dispatch/prompts.rs:54`) to point agents at native
`SendMessage`/`ListAgents` instead of `send_message`. Costs: dispatch loses the trajectory record and
the TUI flash for all sibling coordination (a real observability regression — `observability.allium`
treats `send_message` as an audited call), loses board-validated task-ID addressing, and inherits the
gating in §1 — a `DO_NOT_TRACK` in someone's shell silently removes the channel.

**(c) Channels** (<https://code.claude.com/docs/en/channels.md>) is the *documented* way to push
external events into a running session, and is conceptually the right fit for board→agent notices.
But it is a **research preview**: it needs `--channels plugin:<name>@<marketplace>` at launch, the
plugin must be on Anthropic's (or the org's) allowlist — a local plugin like dispatch's needs
`--dangerously-load-development-channels` — and the flag syntax and protocol contract are declared
subject to change.

## 4. Recommendation

**Do not incorporate it now.** Keep `send_message` on the file + tmux transport.

Reasoning: the only integration that would actually improve dispatch — (a), swapping the transport
while keeping the board semantics — depends on an undocumented socket protocol and an undocumented
session registry, plus a sender-verification model that dispatch's out-of-process MCP server sits
outside of. That is a lot of reverse-engineered surface in the path every watcher notice also travels,
in exchange for robustness dispatch can improve on its own terms. (b) trades audited, board-addressed
messaging for unaudited messaging plus new gating failure modes. (c) is a preview API.

Two things worth noting rather than doing:

1. **It's already on.** Dispatch agents can discover and message each other natively today. If
   invisible-to-board coordination is a concern, the lever is a `SendMessage`/`ListAgents` deny rule
   in settings — my recommendation is to leave it on: it's a useful escape hatch and it is subject to
   the receiver's own permission prompts.
2. **The real defect is dispatch-side and fixable without any of this.** If `send_message` is to get
   better, the valuable work is (i) not injecting keystrokes into a pane that isn't at the prompt,
   and (ii) reporting something stronger than "tmux accepted the keys" back to the sender. Worth a
   separate task if the failure mode has actually been observed.

**Revisit when** either the inbox-socket payload format becomes documented and stable, or channels
leaves research preview — at which point (a) becomes a contained, genuinely worthwhile change.
