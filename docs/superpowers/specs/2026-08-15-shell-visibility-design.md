# Shell Visibility Design

**Task:** #4187 — "Possible to see shell running?"
**Date:** 2026-08-15
**Status:** approved, pending implementation plan

## Problem

Dispatch shows agent activity as labels on kanban cards ("running", "running ·
N agents", "stale · Xm", "blocked", etc.), all derived from Claude Code hook
timestamps and the existing subagent-counting machinery
(`docs/specs/agent-health.allium`). None of that machinery has any concept of
a **backgrounded shell** — a process started via the Bash tool's
`run_in_background: true` option, which returns control to the agent
immediately while the process keeps running detached.

Claude Code's own CLI renders a native "N shell" footer badge for these, with
its own UI affordance to inspect them. Dispatch has no visibility into this at
all today, and — more concretely — has a correctness bug because of it:

**The bug:** `HookStop` (`docs/specs/agent-health.allium:255`) flips a
Running task to Review once the agent's turn ends, *unless* `live_subagents >
0`, in which case the transition is deferred (`stop_pending`) until the last
subagent drains (`HookSubagentStop`'s drain path). A live background shell is
invisible to this check — it isn't a subagent — so a task with a background
shell still running (e.g. a dev server, a long build) gets flipped straight to
Review the moment the agent's foreground turn ends. The task then sits in
Review with a process still alive underneath it, and nothing on the board
reflects that.

## Goals

1. Make a live background shell visible on the card (a new label, additive to
   the existing "running · N agents" convention).
2. Fix the correctness bug: defer the Running → Review transition while a
   background shell is presumably still alive, the same way it's already
   deferred for live subagents.
3. Exempt a task with a live background shell from the stale-after-10-minutes
   classification, for the same reason subagent activity does today.

## Non-goals

- Tracking *foreground* (synchronous) Bash calls. A synchronous call blocks
  the turn until it completes, so ordinary `PreToolUse`/`PostToolUse`
  timestamps already keep the task "active" for its duration — no gap exists
  there today.
- Perfectly detecting every background shell's exit. See "Known limitation"
  below — this design accepts a specific, narrow gap rather than trying to
  close it with something fragile (transcript scraping, polling).
- Any UI to actually "jump into" the shell's live output (mirroring Claude
  Code's own affordance). Out of scope for this task; the ask here is
  visibility on the board, not an interactive attach.

## Design

### New entity: `ShellEntry`

Mirrors the existing `SubagentEntry` pattern
(`docs/specs/agent-health.allium:302-435`) structurally:

- New table `shell_entries(task_id, shell_id, started_at)`.
- New `task.live_shells: u32` column, recomputed from the row count for that
  task whenever a row is added or removed — same shape as `live_subagents`.

Unlike `SubagentEntry`, no session-id fencing rule is needed: every
`SessionStart` unconditionally clears all `ShellEntry` rows for the task (see
"Cleanup invariants" below), so a stale entry from a dead process can never
survive into a new one. `SubagentSessionFence` exists because subagents *can*
legitimately survive a session boundary in some flows; background shells
cannot (a new `claude` OS process cannot reference a shell ID that belonged to
a prior process), so the simpler unconditional-clear approach is correct here
and avoids reproducing that fencing rule for no benefit.

### Detecting shell start

Fired from `PostToolUse`, not `PreToolUse`, for `tool_name == "Bash"` with
`tool_input.run_in_background == true`. This has to be `PostToolUse` because
the shell ID is only assigned once the call returns — at `PreToolUse` we know
a backgrounded Bash is *about to* start but have no ID to key the entry on
yet.

New hook kind: `shell_start --shell-id <id>`. The hook script
(`plugin/hooks/scripts/task-status-hook`) gains a branch in its existing
`PostToolUse` tool_name switch (alongside the current
`Read|Write|Edit|NotebookEdit` branch) that additionally forwards this call
when the Bash response indicates backgrounding.

Service layer: `record_hook_event` creates a `ShellEntry { task, shell_id,
started_at: now }` (idempotent on replay, same join-or-create shape as
`HookSubagentStart`) and recomputes `task.live_shells`.

### Detecting shell stop

Two distinct signals, both from `PostToolUse`:

- **`KillBash`** — `tool_input.shell_id` names the shell being killed. Always
  a definitive stop signal.
- **`BashOutput`** — `tool_input.shell_id` names the shell being polled;
  `tool_response` reports its status. When that status is not "running"
  (e.g. completed, failed, killed), this is also a definitive stop signal for
  that shell_id.

New hook kind: `shell_stop --shell-id <id>`. Service layer deletes the
matching `ShellEntry` (a stop for an unrecognized shell_id is a no-op, mirror
of `HookSubagentStop`'s guidance) and recomputes `live_shells`. **This is also
the drain point**: if this brings `live_shells` to 0, `stop_pending` is set,
and `status == running`, apply the deferred Stop (flip to Review) in the same
atomic write — exactly the `HookSubagentStop` drain path, extended to check
both counters.

**Implementation-time risk to resolve first:** the exact field names for
`tool_input`/`tool_response` on `Bash`, `KillBash`, and `BashOutput` aren't
pinned down from documentation alone. Before writing the parsing logic or its
tests, capture real hook payloads (temporary logging in a scratch session is
enough) to confirm the actual JSON shape.

### `HookStop` change

Extend the existing defer condition
(`docs/specs/agent-health.allium:255-300`) from:

```
if task.live_subagents > 0:
    task.stop_pending = true
```

to:

```
if task.live_subagents > 0 or task.live_shells > 0:
    task.stop_pending = true
```

Everything else about `HookStop` (the atomic-write-order reasoning, the
`stop_pending_at` bookkeeping) is unaffected — this only widens the guard.

### `ClassifyAgentActivity` change

Add `live_shells > 0` at the same precedence tier as the existing
`live_subagents > 0` check (`docs/specs/agent-health.allium:32-33`) — both
force `sub_status = active`, ahead of the 10-minute staleness window, behind
`needs_input`. A task with a live background shell can never go stale, same
reasoning as for live subagents: we have a positive, structural signal that
something is still happening.

### Cleanup invariants

Same discipline as `stop_pending`/`SubagentEntry` today — every point that
already clears subagent state gets the shell-entry equivalent:

- `DetectCrashedAgent` — clears all `ShellEntry` rows and `live_shells = 0`
  alongside its existing subagent clear (the window's gone, so is anything it
  was running).
- Every `SessionStart` (startup/resume/clear — the same three sources
  `ClearSubagentsOnSessionStart` is registered for, not compact/fork, for the
  same reasons already documented there) — unconditionally clears all
  `ShellEntry` rows and `live_shells = 0`. No drain path runs, same as
  `ClearSubagentsOnSessionStart` — a deferred Stop from the ended turn is
  stale, not to be applied here.
- Any write that moves a task out of `running` (manual moves, MCP
  `update_task`, archive, `ExitSession`) clears `stop_pending` today; it
  should also clear `ShellEntry` rows and `live_shells` in the same write, so
  a human's explicit status choice can't later be undone by a delayed
  `shell_stop` drain.

### Card rendering

Extend `CardIndicator::Running` (`src/tui/ui/kanban/cards.rs:65-67`) with a
`shells: u32` field alongside the existing `subagents: u32`. Label composes
additively onto the existing convention:

- `running` (both zero)
- `running · 1 agent` (existing)
- `running · 2 shells` (new)
- `running · 1 agent · 2 shells` (both present)

Singular/plural follows the same pattern as the existing subagent label.

## Known limitation

This only catches a background shell dying when the agent later runs
`BashOutput` or `KillBash` against it. If an agent starts a background shell
and never checks on it again before its turn ends, the task stays in Running
indefinitely — there is no hook event for "a detached shell exited on its
own." This is an accepted tradeoff: per the discussion that produced this
design, an agent backgrounds a shell specifically because it plans to act on
the result later (check output, kill it, or use its output to inform the next
step) — that follow-up action is what supplies the stop signal. A task
stranded in Running because that never happened is a visible, debuggable
symptom (the "· N shells" label stays lit), not a silent one.

## Testing plan (TDD order)

1. Allium spec update to `docs/specs/agent-health.allium` (`HookStop`,
   `ClassifyAgentActivity`, new `HookShellStart`/`HookShellStop` rules,
   `ShellEntry` entity in `core.allium`) — spec first, per this repo's
   convention.
2. Migration + `ShellEntry` CRUD tests (`src/db/tests/migrations.rs`,
   alongside the `SubagentEntry` precedent).
3. Hook-script tests (`src/setup/hooks.rs`) — Bash-with-background-true
   forwards `shell_start`; Bash without it does not; `KillBash` forwards
   `shell_stop`; `BashOutput` forwards `shell_stop` only when status is
   non-running.
4. Service-layer tests (`src/service/tasks/` or wherever
   `record_hook_event` is tested) — `shell_start`/`shell_stop` handling,
   the drain path, and the `HookStop` defer-widening.
5. `classify_agent_activity` unit tests — `live_shells > 0` stays active past
   the 10-minute threshold.
6. Cleanup-invariant tests — crash, session start, and manual-move all clear
   `ShellEntry`/`live_shells`.
7. Card indicator tests/snapshots — new label composition.

## Open questions

- None blocking; the field-name risk under "Detecting shell stop" is the one
  item to resolve empirically before implementation, not a design ambiguity.
