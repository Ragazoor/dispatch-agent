# 3844 — Surface epic auto-dispatch failures on the board

## Problem

`auto_dispatch_next` (`src/mcp/handlers/tasks/dispatch.rs`) handles a failed
`do_dispatch` by logging `tracing::warn!` and releasing the claim. The subtask
returns to Backlog looking exactly as it did before the chain fired, so an epic
whose chain died on a transient `git fetch` failure simply stops progressing
with the only evidence in `app.log`.

The TUI dispatch path does better: `run_blocking_dispatch`
(`src/runtime/tasks.rs:29`) sends `DispatchFailed` **and** a
`SystemMessage::Error`, which reaches the status bar. The MCP chain sends
neither.

## Design

Three surfaces, in increasing durability, all fed by one new event:

1. **Status-bar error** — immediate, matches the TUI dispatch path.
2. **Desktop notification** (gated on `notifications_enabled`) — reaches a user
   who is not looking at the board, matching `NotifyNeedsInput`/`NotifyReview`.
3. **A persistent card indicator** on the reverted Backlog card — `⚠ auto-dispatch
   failed`, red — which survives until the subtask is dispatched again. This is
   the part that actually makes a stalled chain discoverable hours later.

State for (3) lives in memory (`AgentTracking`), not the database. That is
sufficient and not a shortcut: `auto_dispatch_next` runs inside the TUI process
(the MCP server is hosted by it), so the marker is exactly as durable as the
event that produced it. A TUI restart clears it, and after a restart the epic's
stall is a fresh problem to observe, not a stale one to replay.

No new `SubStatus` variant: `SubStatus::is_valid_for` allows only
`SubStatus::None` under `TaskStatus::Backlog`, and widening that would put a
transient dispatch-attempt outcome into a persisted field that the whole
agent-health classifier reads.

### Scope

Covered: the two arms of `auto_dispatch_next` that fail with a **known task** —
`Ok(Err(e))` (dispatch failed) and `Err(e)` (blocking task panicked). Both
already release the claim; both gain the event.

Not covered: the epic-fetch and claim-error arms. Those fail before any task is
selected, so there is no card to mark and no id to name. They remain
warn-and-skip, and the spec already documents them as fail-closed stops.

### Data flow

```
auto_dispatch_next (failure arm)
  └─ McpEvent::AutoDispatchFailed { task_id, epic_id, error }
       └─ runtime/mod.rs LoopEvent::Mcp arm
            └─ Message::Task(TaskMessage::AutoDispatchFailed { .. })
                 └─ App::handle_auto_dispatch_failed
                      ├─ agents.auto_dispatch_failed.insert(task_id, error)
                      ├─ set_status("auto-dispatch of #N failed: …")
                      └─ SendNotification (if notifications_enabled)
```

The existing `McpEvent::TaskChanged(next_id)` send stays as-is — the row still
needs reloading after the release.

### Clearing the marker

The marker is removed when:

- the task is dispatched again — `App::mark_dispatching(id)`;
- a board refresh brings the row back in a status other than `Backlog` (someone
  moved it, or another entry point dispatched it) — in
  `detect_task_transition_notifications`, alongside the existing notified-set
  cleanup;
- `AgentTracking::clear(id)` runs (task archived/removed).

## Implementation steps (TDD — test first in every step)

### Step 1 — spec

Update `docs/specs/epics.allium`:

- `AutoDispatchNextSubtask` guidance currently says a failed dispatch is
  "logged and nothing more". Amend to say it also raises an operator-visible
  failure signal, and keep the "no chain failure can fail the close" invariant
  explicit — the signal goes to the board, never to the closing agent's
  response.
- Add a rule (working name `SurfaceAutoDispatchFailure`) for the three
  surfaces and the clearing conditions, plus why the epic-fetch/claim arms are
  excluded.

Run `allium check` on the file. Use the `allium:tend` skill for the edit and
`allium:weed` afterwards.

### Step 2 — the event

*Test*: extend `exit_session_chain_reverts_claim_when_dispatch_fails`
(`src/mcp/handlers/tests/tasks/dispatch.rs`) to also await an
`McpEvent::AutoDispatchFailed` naming the reverted subtask and its epic, with a
non-empty error string. Add a helper mirroring `wait_for_task_changed`.

*Code*: add `McpEvent::AutoDispatchFailed { task_id, epic_id, error: String }`
(`src/mcp/mod.rs`) with a doc comment; send it from both failure arms of
`auto_dispatch_next`, before the existing `TaskChanged`/`EpicChanged` sends.

### Step 3 — runtime routing

*Test*: in `src/runtime/tests.rs`, drive the `LoopEvent::Mcp` arm with the new
event and assert the resulting `TaskMessage::AutoDispatchFailed`.

*Code*: add the arm in `src/runtime/mod.rs` (~line 683).

### Step 4 — TUI message + state

*Test* (`src/tui/tests/dispatch.rs`):

- handling the message records the marker and sets a status message naming the
  task id;
- with `notifications_enabled`, it emits a `SendNotification` command; without,
  it does not;
- `mark_dispatching` clears the marker;
- a refresh delivering the row in a non-Backlog status clears the marker.

*Code*: `TaskMessage::AutoDispatchFailed { task_id, epic_id, error }` +
`handle_auto_dispatch_failed`; `auto_dispatch_failed: HashMap<TaskId, String>`
on `AgentTracking` (cleared in `AgentTracking::clear`).

### Step 5 — card rendering

*Test*: a snapshot in `src/tui/tests/snapshots/` showing a Backlog card with the
marker set, plus a unit assertion that the indicator wins over the plain Idle
indicator and loses to `Dispatching` (a re-dispatch spinner must not be masked
by a stale failure marker).

*Code*: `CardIndicator::AutoDispatchFailed` in `src/tui/ui/kanban/cards.rs`,
rendered as `⚠ auto-dispatch failed` in `Color::Red`; classified after the
`is_dispatching` check and before the `Unprovisioned` check.

### Step 6 — verify

`cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`, then
`cargo clippy --all-targets -- -D warnings`. Clean up any `*.snap.new`.

## Risks

- Snapshot churn: a new indicator only renders when the marker is set, so
  existing snapshots are untouched. Confirm by diffing after the run.
- The `error` string is a formatted `{e:#}` chain and can be long. The status
  bar truncates by width; keep the notification body short and put the detail in
  the status message only.
