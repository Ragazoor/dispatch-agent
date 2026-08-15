# Design: harden watcher-notice delivery against non-prompt panes

Task: #4098 (follow-up from #3983, `docs/plans/3983-cross-session-messaging-investigation.md`)
Date: 2026-08-15

**Scope note (revised):** after this doc was first drafted, the direction for
agent-initiated `send_message` changed — see
`2026-08-15-send-message-native-relay-design.md` for that redesign (agents call
Claude Code's native `SendMessage` tool directly instead of dispatch injecting
keystrokes). **This document now covers only task-watcher completion/deletion
notices** (`docs/specs/task-watchers.allium::DeliverWatcherNotification`),
which have no live agent turn to piggyback a native tool call on — a watcher
notice is fired by dispatch's own background service when a *different* task
reaches Done/Archived, not by the watching agent choosing to do anything. That
path has no alternative to the file+tmux-send-keys transport, so the
pane-content-probe hardening below still applies to it. Everything below that
originally described `send_message` should be read as describing the shared
`notify::deliver`/`notify_tmux` primitive, now exercised only by
`deliver_watch_notification` (`src/service/tasks/watchers.rs`).

## Problem

`src/notify.rs::deliver` (shared by `send_message` and task-watcher completion
notices) writes the message body to a file, then unconditionally
`tmux send-keys -l <nudge text>` + `send-keys Enter` into the target window. Two
weaknesses, per the task:

1. **Keystroke injection is state-dependent.** `send-keys` types into whatever
   the pane is currently rendering. If the target isn't at its normal chat
   input — a permission prompt, a plan-mode/elicitation dialog, a pager — the
   text and Enter land there instead.
2. **No delivery confirmation.** A zero exit from `send-keys` only means tmux
   accepted keystrokes, not that the agent read or acted on them. The sender
   gets an unconditional success back from `send_message` regardless.

The task's own instruction: reproduce (1) before building anything, and close
the task if it doesn't reproduce.

## Reproduction

Private tmux socket (`-L dispatch-repro-4098`), a real `claude` process (not a
simulation), scratch git repo. Two scenarios:

**First-run trust dialog.** Launching `claude` in a fresh directory shows:
```
 ❯ 1. Yes, I trust this folder
   2. No, exit

 Enter to confirm · Esc to cancel
```
Sending a bare `Enter` (mirroring `notify_tmux`'s second `send-keys` call)
confirmed "trust this folder" — a security-relevant dialog answered by a
keystroke nobody aimed at it.

**Plan-mode elicitation dialog (the faithful repro).** In `--permission-mode
plan`, asked the agent to draft a one-line plan. It opened a multiple-choice
dialog:
```
❯ 1. MIT
     Permissive, simple, most common for open-source projects.
  2. Apache 2.0
  3. GPL-3.0
  4. Type something.
────────────────────────────────────────────────────────────────────────────
  5. Chat about this

Enter to select · ↑/↓ to navigate · Esc to cancel
```
From outside, sent the **exact** `notify_tmux` sequence — `send-keys -l` with
the real nudge text (`"You received a message from task 42. Read
.claude-messages/42-1234-0.md for the full content, then delete the file."`)
followed by `send-keys Enter` — the same two calls `notify_tmux` issues, same
flags. Result:
```
● User answered Claude's questions:
  ⎿  · Which license should the LICENSE file use? → MIT
```
The agent never saw the message. The literal text was swallowed by the dialog
(it doesn't accept free text except via the explicit "Type something" option)
and the trailing Enter silently confirmed the pre-highlighted choice. This is a
real, reproduced failure, not a hypothetical — confirmed with the identical
send-keys invocations `notify_tmux` makes today.

This also demonstrates (2) implicitly: `tmux send-keys` returned exit 0 both
times. Nothing in the transport would have told the sender delivery failed.

## Root cause

tmux has no concept of application state. `send-keys` writes raw bytes to the
pane's pty; whatever is reading that pty (a dialog's option list, a pager, the
chat input box) decides what they mean. A modal dialog reads Enter as "confirm
the highlighted option," not as "submit the preceding line of text."

## Alternatives considered

**(a) Gate on the existing `task.sub_status` (`needs_input`).** Already
computed from Claude Code hook `Notification` events per
`docs/specs/agent-health.allium::HookNotification`. Rejected: `needs_input` is
a single bucket covering three distinct notification kinds —
`permission_prompt`, `elicitation_dialog` (unsafe to inject into, per the repro
above), *and* `idle_prompt` (Claude idle at its own normal input box — exactly
the state a live nudge is supposed to reach). Gating on `needs_input` as-is
would silently stop nudging the single most common and useful case — an idle
sibling agent waiting to be told about a message. Disambiguating would require
persisting the raw notification kind (a schema migration) and still wouldn't
cover a manually-opened pager, which fires no hook at all.

**(b) Swap the transport for Claude Code's native cross-session messaging.**
Already investigated and rejected in #3983 / learning #396: the inbox-socket
wire format and session registry are undocumented, and dispatch's MCP server
sits outside the sender-verification model that makes native delivery
unsupervised-safe. Not revisited here.

**(c) Probe the pane's actual rendered content immediately before injecting.**
Chosen. `tmux capture-pane -p` returns exactly what's on screen right now — the
same view a human sees. Claude Code's idle chat view always ends in a status
line containing `shift+tab to cycle` (confirmed across auto/plan-mode states in
the repro above); every modal dialog observed (trust prompt, plan-mode
question) replaces that entire footer region with dialog-specific text instead
(`Enter to select`, `Enter to confirm`, `Esc to cancel`/`Esc to reject`), and a
pager or dead window shows neither. So: last non-blank line of the capture
contains the marker → safe to nudge; anything else (marker absent, capture
fails, window vanished) → withhold the nudge.

This is the only option that (i) needs no schema change, (ii) doesn't
conflate idle-and-safe with blocked-and-unsafe, and (iii) covers the pager
case the task names, for which no hook signal exists at all.

**Known limitation, accepted:** the marker string is coupled to Claude Code's
current UI text. If a future release changes it, the check fails *closed* —
nudges stop firing for the idle case too, exactly like a false negative today,
never fails *open* into re-injecting blindly. That direction of failure is the
acceptable one: a missed live nudge (message still sits in the file) is far
cheaper than a corrupted dialog answer.

## Fix

1. **`src/tmux.rs`**: add `capture_pane(window, runner) -> Result<String>` —
   resolves `window` via the existing `window_target` (so a genuinely absent
   window still fails loudly, matching today's error path), then
   `tmux capture-pane -p -t <target>`.

2. **`src/notify.rs`**:
   - `pub enum DeliveryOutcome { Notified, QueuedNoNudge }`.
   - `notify_tmux` / `deliver` return `Result<DeliveryOutcome, String>` instead
     of `Result<(), String>`.
   - Before the existing two `send_keys` calls, capture the pane and check the
     last non-blank line for the marker constant (named and comment-linked to
     this design doc's reproduction, so a future reader knows it's empirical,
     not arbitrary).
   - Window/capture resolution failure → same hard-error path as today (file
     removed, `Err` returned) — this is a real "can't reach the target" case,
     not a "target is busy" case, so it must not be downgraded silently.
   - Marker absent → **do not call `send_keys` at all**. Message file is kept
     (it's queued, not failed). Return `Ok(DeliveryOutcome::QueuedNoNudge)`.

3. **`src/service/tasks/watchers.rs::deliver_watch_notification`**: match on
   the outcome. `Notified` is today's behaviour. `QueuedNoNudge` is not an
   error — no caller is waiting on this fire-and-forget call — but log it
   (`tracing::debug!`) instead of only logging on hard error, so a silently
   withheld nudge is at least visible in logs. The message file is kept either
   way (it's queued, not failed).

## Documentation obligations

- `docs/specs/task-watchers.allium::DeliverWatcherNotification` — describe the
  pane readiness probe and the two outcomes (`TmuxNotificationSent` only on
  `Notified`; a queued-no-nudge case still counts as the file having been
  written, per the existing "cleanup does not wait on... whether delivery
  actually [succeeded]" framing already in that spec).
- `docs/specs/mcp-task-tools.allium::SendMessageViaMcp` is superseded, not
  amended here — see the native-relay design doc.

## Test strategy

1. **Unit (mock)** — `src/notify.rs`: the marker-matching predicate as a pure
   function; `notify_tmux`/`deliver` with a mocked `capture-pane` response
   containing the marker (sends both keys, `Notified`), one without it (no
   `send-keys` calls at all, file kept, `QueuedNoNudge`), and window
   resolution failure (existing hard-error path, file removed).
2. **Real-tmux integration** — new `tests/tmux_send_message_pane_state.rs`,
   following the private-socket pattern in `tests/tmux_harness/mod.rs`: a pane
   running a small controllable script that prints a "ready" screen or a
   "dialog" screen on command, and a second pane recording keystrokes via the
   `cat > file` technique used in `tests/tmux_split_hook.rs`. Asserts real
   `tmux capture-pane` + real `send-keys` behave as the mock predicts — no
   keystrokes recorded in the dialog case, both keystrokes recorded in the
   ready case. Does not require a real `claude` process (no network/API
   dependency in CI).

## Open question for review

The marker constant (`shift+tab to cycle`) is the load-bearing piece of this
design. I'm confident it's real (reproduced against `claude` v2.1.233) but it
is inherently version-coupled. Flagging before implementation in case there's
a preference for a different signal or a belt-and-suspenders combination with
`task.sub_status` as a cheap pre-filter (skip the capture-pane call entirely
when `sub_status` is already `active`/`stale`, only probe when `needs_input`) —
that would cut the capture-pane call count roughly to only the ambiguous
cases, at the cost of one more moving part.
