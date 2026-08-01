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
| `SubagentStart` | `dispatch hook-subagent "$ID" start --agent-id "$AGENT_ID"` |
| `SubagentStop` | `dispatch hook-subagent "$ID" stop --agent-id "$AGENT_ID"` |
| `SessionStart` | `dispatch hook-subagent "$ID" clear` |

Task-ID extraction reuses the existing branch-name logic
(`plugin/hooks/scripts/task-status-hook:20`) unchanged. An empty `agent_id` on
the start/stop arms is a silent `exit 0`.

The existing `PreToolUse|PostToolUse` arm is deliberately left alone. Subagent
tool calls reaching it is correct behaviour — it is what keeps the parent task's
activity timestamp fresh — and filtering on `agent_id` there would reintroduce
the staleness problem this design is trying to remove.

`agent_type` is captured by neither arm. Nothing in this design consumes it: the
render is a count, the classifier is a `> 0` test, and the TTL sweep keys off
`started_at`. Adding the column later, if the label ever grows to name agent
types, is a one-line migration against a table that already exists.

### Storage

Migration 81:

```sql
CREATE TABLE task_subagents (
  task_id    INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  agent_id   TEXT    NOT NULL,
  started_at TEXT    NOT NULL,
  PRIMARY KEY (task_id, agent_id)
);
CREATE INDEX idx_task_subagents_task ON task_subagents(task_id);

ALTER TABLE tasks ADD COLUMN live_subagents INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tasks ADD COLUMN stop_pending   INTEGER NOT NULL DEFAULT 0;
```

The table is the authority. `INSERT OR REPLACE` makes a duplicate `SubagentStart`
idempotent; `DELETE` by `(task_id, agent_id)` makes an unrecognised
`SubagentStop` a no-op rather than a count underflow. Per-row `started_at` is
what the TTL sweep needs.

`tasks.live_subagents` is a denormalised `COUNT(*)`, rewritten from the table
inside the same transaction as every mutation. It exists so the classifier and
the card render read one integer off the task row instead of forcing a join into
every task query. The sweep recomputes it, so the two cannot drift for long.

### Health semantics

All changes land in `docs/specs/agent-health.allium`.

New config constant alongside `active_threshold`:

```
subagent_ttl: Duration = 60.minutes
```

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
when the count reaches zero *and* `stop_pending` is set, applies the deferred
`HookStop` effects (status → review, sub_status → default, timestamps cleared,
`stop_pending` → false).

**New `SweepStaleSubagents`**: on Tick, deletes entries older than
`subagent_ttl`, recomputes the count, and runs the identical drain path as
`HookSubagentStop`. This is not optional. Without it a leaked entry pins the task
to Running *and* strands a `stop_pending` task that can never reach Review —
the one way this feature could permanently break a task.

**Clear points** — reset both the entries and `stop_pending`:

- `SessionStart` (a fresh Claude session provably has zero live subagents)
- `DispatchTask` and crash-retry
- `HookUserPromptSubmit` (a human resuming voids any pending stop)

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

1. **Spec** (`allium:tend` on `agent-health.allium`): `subagent_ttl`, the new
   `ClassifyAgentActivity` branch, conditional `HookStop`, and the three new
   rules `HookSubagentStart` / `HookSubagentStop` / `SweepStaleSubagents`.
2. **DB** (`src/db/tests/`): migration 81 applies and bumps
   `LATEST_SCHEMA_VERSION`; duplicate start is idempotent; unknown stop is a
   no-op; `live_subagents` equals `COUNT(*)` after every mutation; the sweep
   drops only entries past `subagent_ttl`.
3. **Service**: classifier precedence (subagents beat stale, lose to
   `needs_input`); `Stop` with live subagents sets `stop_pending` without moving
   status; the last `SubagentStop` performs the deferred flip; the TTL sweep
   performs the same deferred flip; each clear point resets entries and
   `stop_pending`.
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
  idempotency (a replayed hook double-counts) and has no `started_at`, so the
  TTL sweep the drift bound depends on becomes impossible.
- **Letting `Stop` flip to Review and pulling the task back on the next
  `SubagentStart`.** Visible flicker through Review, and it fires the epic
  auto-dispatch chain, which reacts to review transitions.
- **Reclaiming leaks from tmux liveness alone**, with no TTL. Misses the case
  where the `claude` process dies but the tmux window survives.
