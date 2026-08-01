# Subagent counting — design

Task #3755. Show how many subagents a dispatched agent is currently running, and
stop subagent activity from being misread as a stalled or finished agent.

## Problem

Two observable defects today, both rooted in dispatch having no idea that
subagents exist:

1. **No visibility.** A card in Running looks identical whether the agent is
   typing one edit or fanning out across six parallel subagents.
2. **A task with live subagents lands in Review.** The main agent spawns
   background subagents, finishes its own turn, and Claude Code fires `Stop`.
   `HookStop` (`docs/specs/agent-health.allium:227`) moves the task to Review
   while the subagents are still working.

Defect 2 is the load-bearing one. Defect 1 is the surface that makes it legible.

## Research: what Claude Code actually emits

From the official hooks documentation (`https://code.claude.com/docs/en/hooks`):

- **`SubagentStart`** fires when a subagent is spawned. Payload carries the
  common fields (`session_id`, `cwd`, `transcript_path`, `permission_mode`,
  `hook_event_name`) plus `agent_id` (unique per subagent) and `agent_type`.
- **`SubagentStop`** fires when a subagent finishes. Same fields, plus
  `last_assistant_message` and `effort.level`.
- Matchers on both events filter by agent type. Omitting the matcher catches
  every agent type, which is what we want.
- Hooks from plugins **also run inside subagents**. A subagent's own tool calls
  therefore already fire `PreToolUse`/`PostToolUse` against the task's worktree,
  and those events carry `agent_id` to distinguish them from main-thread calls.
- **`Stop` does not fire inside subagents** — the docs state this explicitly and
  direct you to `SubagentStop` for that context.

That last point rules out the tempting explanation for defect 2 (that `Stop` was
leaking out of subagents). The real cause is background subagents outliving the
turn that spawned them, which makes the `Stop` legitimate and the *reaction* to
it wrong.

It also means we get staleness resistance mostly for free: a working subagent
refreshes `last_pre_tool_use_at` on every tool call. The gap this design still
has to close is a subagent that runs a single long operation with no tool calls
for longer than `active_threshold`.

## Design

### Capture

Register three new events in `plugin/hooks/hooks.json`, all pointing at the
existing `task-status-hook` script with no matcher:

| Event | New arm in `task-status-hook` |
|---|---|
| `SubagentStart` | `dispatch hook-subagent "$ID" start --agent-id "$AGENT_ID" --session-id "$SESSION_ID"` |
| `SubagentStop` | `dispatch hook-subagent "$ID" stop --agent-id "$AGENT_ID" --session-id "$SESSION_ID"` |
| `SessionStart` | `dispatch hook-subagent "$ID" clear` |

Task-ID extraction reuses the existing branch-name logic
(`plugin/hooks/scripts/task-status-hook:20`) unchanged. An empty `agent_id` on
the start/stop arms is a silent `exit 0`.

The existing `PreToolUse|PostToolUse` arm is deliberately left alone. Subagent
tool calls reaching it is correct behaviour — it is what keeps the parent task's
activity timestamp fresh — and filtering on `agent_id` there would reintroduce
the staleness problem this design is trying to remove.

`agent_type` is captured by neither arm. (`session_id` is, and is load-bearing —
see "Drift".) Nothing in this design consumes it: the
render is a count, the classifier is a `> 0` test, and drift reclamation keys off
`session_id`. Adding the column later, if the label ever grows to name agent
types, is a one-line migration against a table that already exists.

### Storage

Migration 81:

```sql
CREATE TABLE task_subagents (
  task_id    INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  agent_id   TEXT    NOT NULL,
  session_id TEXT    NOT NULL,
  started_at TEXT    NOT NULL,
  PRIMARY KEY (task_id, agent_id)
);
CREATE INDEX idx_task_subagents_task ON task_subagents(task_id);

ALTER TABLE tasks ADD COLUMN live_subagents INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tasks ADD COLUMN stop_pending   INTEGER NOT NULL DEFAULT 0;
```

The table is the authority. `INSERT OR REPLACE` makes a duplicate `SubagentStart`
idempotent; `DELETE` by `(task_id, agent_id)` makes an unrecognised
`SubagentStop` a no-op rather than a count underflow. `started_at` is retained
for diagnostics only — nothing in the design branches on it.

`tasks.live_subagents` is a denormalised `COUNT(*)`, rewritten from the table
inside the same transaction as every mutation. It exists so the classifier and
the card render read one integer off the task row instead of forcing a join into
every task query. It is rewritten from the table by every mutation and every
clear point, so the two have no window in which to disagree.

### Health semantics

All changes land in `docs/specs/agent-health.allium`. No new config constant —
see "Drift" below for why there is no tuned threshold anywhere in this design.

**`ClassifyAgentActivity`** gains one branch, below `needs_input` and above the
threshold check:

```
if last_notification_at is newer than last_pre_tool_use_at:  needs_input
else if live_subagents > 0:                                  active     <- new
else if now - last_pre_tool_use_at <= active_threshold:      active
else:                                                        stale
```

Live subagents beat staleness but lose to `needs_input`. A permission prompt
genuinely needs a human even while subagents churn, so hiding it behind a
subagent count would trade one misreport for a worse one.

**`HookStop`** becomes conditional:

```
if live_subagents > 0:  stop_pending = true          -- suppress; stay running
else:                   status = review, ...         -- unchanged
```

**New `HookSubagentStart`**: upserts the entry, recomputes `live_subagents`.

**New `HookSubagentStop`**: removes the entry, recomputes `live_subagents`, and
runs the *drain path* — when the count reaches zero and `stop_pending` is set,
apply the deferred `HookStop` effects (status → review, sub_status → default,
timestamps cleared, `stop_pending` → false).

### Drift

`SubagentStop` is reliable in normal operation, so the design trusts it rather
than second-guessing it on a timer. Reclaiming a leaked entry uses two
mechanisms, neither of which has a tunable constant.

**Session fencing.** Every hook payload carries `session_id`, stored on the row.
Any subagent hook write first deletes rows for that task whose `session_id`
differs from the incoming one. A new `claude` process means a new `session_id`,
so entries from a dead session are provably dead — no threshold and no judgement
call. This holds even if `SessionStart` never fires or the hook is not installed.

**Structural clear points**, each an event already detected. They split by
whether the triggering rule already owns the task's resulting status:

| Signal | Rule | Clears entries | Then |
|---|---|---|---|
| Session starts, resumes, or is cleared | new `SessionStart` arm | yes | drain path |
| Detach | `DetachTmux` / `BatchDetachTmux` (`docs/specs/split-pane.allium:21`) | yes | drain path |
| tmux window gone | `DetectCrashedAgent` (`docs/specs/agent-health.allium:92`) | yes | clear `stop_pending`, no flip |
| Dispatch or retry | `DispatchTask` | yes | clear `stop_pending`, no flip |

*Drain path* means the same effect as `HookSubagentStop`: if the clear drops the
count to zero while `stop_pending` is set, apply the deferred `HookStop` effects.
This is the load-bearing property — a reset never strands a task in Running with
an unresolved `stop_pending`, it resolves it.

The bottom two rows deliberately skip the flip. `DetectCrashedAgent` sets
`sub_status = crashed` and `DispatchTask` moves the task into Running; draining
to Review in the same breath would produce a task that is Crashed *and* in Review,
or freshly dispatched *and* in Review. In both cases the triggering rule's status
is the more informative one and wins. They still clear `stop_pending`, so the bit
cannot survive into the next session and fire a spurious flip later.

**Known uncovered case**: the `claude` process dies but its tmux window survives.
No hook arrives and no window-gone signal fires, so the task stays pinned in
Running with a phantom count until it is detached, retried, or a new session
starts in that window. This is accepted rather than fixed. The scenario already
misbehaves today for the same root cause — a dead process emits no `PreToolUse`
either — and it fails toward a visible Running rather than a wrong Review.
Closing it properly means probing pane liveness on the tick, which changes crash
detection for every task and belongs in its own task.

An earlier draft of this design used a per-entry TTL sweep instead. It was
dropped: the threshold is unfalsifiable — short enough to reclaim a phantom
promptly is short enough to sweep a legitimately long-running subagent.

### Render

The `CardIndicator::Running` variant (enum at `src/tui/ui/kanban/cards.rs:45`,
render arm at `src/tui/ui/kanban/cards.rs:197`) gains a `subagents: u32` field. At zero the label is unchanged; above zero a suffix is
appended:

```
◉ running · 3 agents
◉ running · 1 agent      -- singular
◉ running                -- zero, unchanged
```

This follows the existing suffix convention exactly — status glyph in front,
plain text after the middot, as in `◉ stale · 3m`. Card interior is 27 columns
with a 3-space indent; the longest form uses 20 of the 24 usable columns.

Only the `Running` variant carries the count. `Stale` can no longer co-occur with
live subagents by construction, and `Blocked` is left plain to keep the change to
a single variant.

## Testing

TDD order. Spec first, then tests, then code, per the repo convention.

1. **Spec** (`allium:tend` on `agent-health.allium`): the new
   `ClassifyAgentActivity` branch, conditional `HookStop`, and the two new rules
   `HookSubagentStart` / `HookSubagentStop`, plus the clear-point amendments to
   `DetectCrashedAgent`, `DetachTmux` and `DispatchTask`.
2. **DB** (`src/db/tests/`): migration 81 applies and bumps
   `LATEST_SCHEMA_VERSION`; duplicate start is idempotent; unknown stop is a
   no-op; `live_subagents` equals `COUNT(*)` after every mutation; a write
   bearing a new `session_id` evicts the previous session's rows.
3. **Service**: classifier precedence (subagents beat stale, lose to
   `needs_input`); `Stop` with live subagents sets `stop_pending` without moving
   status; the last `SubagentStop` performs the deferred flip; the session fence
   and detach also perform it when they drain the last entry; **crash and
   dispatch clear `stop_pending` without flipping** — assert the task is not
   left simultaneously Crashed-and-Review or freshly-dispatched-and-Review.
4. **Hook script** (`src/setup/hooks.rs`, stub-`dispatch` with JSON on stdin,
   matching the existing arm tests): each new arm invokes the right command;
   missing `agent_id` is a silent no-op; `hooks.json` registers all three events.
5. **Card snapshot** (`src/tui/tests/snapshots/`): `◉ running · 3 agents`, the
   singular `1 agent`, and unchanged output at zero.

Then `allium:weed` to confirm spec and code agree.

Verification gate: `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`

## Rejected alternatives

- **Counting via `PreToolUse` on the `Task` tool** rather than the dedicated
  events. Gives you spawns but no reliable completion signal for background
  agents, so the count only ever grows.
- **A bare integer column** with increment/decrement instead of a table. Loses
  idempotency (a replayed hook double-counts) and has nowhere to record
  `session_id`, so session fencing becomes impossible.
- **A per-entry TTL sweep** as the drift bound. Any threshold short enough to
  reclaim a phantom promptly is short enough to sweep a legitimately long-running
  subagent, and the constant cannot be validated against anything.
- **Letting `Stop` flip to Review and pulling the task back on the next
  `SubagentStart`.** Visible flicker through Review, and it fires the epic
  auto-dispatch chain, which reacts to review transitions.
- **Reclaiming leaks from tmux liveness alone.** Window-gone is one of the four
  clear points, but on its own it misses a `/clear`ed or restarted session inside
  a window that never died — which session fencing catches exactly.
