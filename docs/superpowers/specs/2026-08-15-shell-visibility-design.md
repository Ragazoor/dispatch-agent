# Shell Visibility Design

**Task:** #4187 — "Possible to see shell running?"
**Date:** 2026-08-15
**Status:** revised after adversarial review, pending implementation plan

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
  the turn until it completes, and `Stop` cannot fire mid-blocking-call, so the
  correctness bug this design targets (a premature Running → Review flip)
  cannot occur for foreground calls. (Correction from the first draft: a
  long foreground call *can* still render "stale" mid-call today, since
  `last_pre_tool_use_at` isn't refreshed until the blocking call's
  `PostToolUse` fires — there is no heartbeat during the wait. That's a
  cosmetic staleness quirk, not the correctness bug, and is left alone here.)
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

**Revised after review — the first draft's session-fencing reasoning was
wrong.** It claimed no fencing was needed because "a new `claude` OS process
cannot reference a shell ID that belonged to a prior process," and proposed
unconditionally clearing `ShellEntry` on every `SessionStart`, including
`source = clear`. But `docs/specs/agent-health.allium:447` is explicit that
`clear` means the **transcript** is cleared, not that the OS process restarts.
A backgrounded shell (a dev server, a long build) is exactly the kind of
process a user keeps running *across* a `/clear` — that's the point of
backgrounding it. Unconditionally clearing on `clear` silently drops tracking
of a shell that's still alive, reproducing the exact bug this design exists to
fix, via a completely ordinary workflow. The same doubt applies to `resume`
with less certainty (unclear whether Claude Code's background-shell registry
survives a resumed process).

Given that doubt, this design takes the conservative direction, by the same
asymmetry-of-consequences reasoning `HookUserPromptSubmit`'s tie-breaking rule
already uses in this codebase (`docs/specs/agent-health.allium:519-525`): "a
bit wrongly kept is resolved by the next drain... whereas one wrongly voided
strands the task." Under-clearing (leaving a `ShellEntry` marked live after
the process actually died) is self-correcting — it resolves the next time
`ExitSession`, a manual move, or `DispatchTask` sweeps it (see "Cleanup
invariants" below), or it just sits as an accepted, visible "Known
limitation" case. Over-clearing (dropping live tracking for a shell that
survived `clear`/`resume`) directly reproduces the bug with no way back.

So: **`ShellEntry` rows are cleared only on `SessionStart(source = startup)`**
— a definite fresh process, where nothing from a prior process could still be
referenced — never on `resume` or `clear`. This is a deliberate asymmetry from
`ClearSubagentsOnSessionStart` (which clears on all three), because subagents
and background shells differ in exactly the property that matters here:
subagents are part of the same conversational/process lifecycle Claude Code
itself owns and cannot survive a resume or clear, while a backgrounded shell
is an independent OS-level process that can. Whether `resume` in practice ever
needs the conservative treatment (i.e. whether a resumed process really can
still reference a prior shell ID) is exactly the kind of thing to confirm
empirically alongside the field-name risk below — if it turns out `resume`
always spawns a registry-reset process, this can be revisited to clear on
`resume` too. Until confirmed, default to not clearing there.

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
a drain point** — see "The shared drain predicate" below for why this must
not become its own independent copy of the drain logic.

**Implementation-time risks to resolve first, before writing any parsing
logic or its tests:**

- The exact field names for `tool_input`/`tool_response` on `Bash`,
  `KillBash`, and `BashOutput` aren't pinned down from documentation alone —
  capture real hook payloads (temporary logging in a scratch session is
  enough) to confirm the actual JSON shape.
- **Fallback if `BashOutput`'s response doesn't expose a clean, structured
  running/not-running status** (e.g. if it's closer to free-form transcript
  text than a status enum): `shell_stop` can then only be driven by explicit
  `KillBash`, and a background shell that finishes on its own — arguably the
  *common* case (a build completes, a script finishes), not a rare one — would
  never produce a stop signal at all. That would turn "Known limitation"
  below from a narrow accepted edge case into the typical outcome. This is a
  go/no-go question to answer during implementation, not something to
  discover silently after shipping: if the field isn't usable, come back to
  this design before proceeding, since the risk profile changes materially.

### `HookStop` change

**Revised after review — the design's original single if/else was too
abstract.** The real implementation, `try_record_stop`
(`src/db/queries/tasks.rs:580-594` and `:608-616`), is **two separate
conditional `UPDATE` statements**, each gated at write time for race-safety
(so whichever hook process commits second observes the other's committed
value):

- the **flip** statement, guarded by `WHERE ... AND live_subagents = 0`
  (line 584) — must become `AND live_subagents = 0 AND live_shells = 0`.
- the **defer** statement, guarded by `WHERE ... AND live_subagents > 0`
  (line 613) — must become `AND (live_subagents > 0 OR live_shells > 0)`.

Both statements need to change. Widening only the defer statement (as the
first draft implied) leaves the flip statement blind to `live_shells`, and
every task with `live_subagents = 0` and a live background shell — this
design's headline scenario — keeps flipping to Review unchanged, with no
error to signal the fix didn't take.

### The shared drain predicate

`apply_pending_stop_if_drained` (`src/db/queries/subagents.rs:33-52`) is a
single function deliberately shared by `subagent_stop`'s drain and
`subagent_clear`'s drain (`finish_drain`, lines 178-187) specifically so a
later edit can't fix one caller and silently leave the other racy. Its
`WHERE` clause currently reads `... AND live_subagents = 0`.

This function — not a new, parallel one written for shells — must widen to
`... AND live_subagents = 0 AND live_shells = 0`, and both the subagent-drain
call site and the new shell-stop call site must call *this same* function.
Writing a separate `shell`-side drain helper (which "mirrors `SubagentEntry`
structurally" invites) reproduces the exact race this function exists to
prevent: a subagent-count drain to zero could flip the task while
`live_shells > 0` is still true, or vice versa, because neither independent
copy would know about the other counter. Whoever implements this must locate
and widen the one function, not add a second.

**Every place that already applies this drain — `HookSubagentStop`'s drain
path, `DetachTmux` (see "Cleanup invariants" below) — must call through the
same widened predicate**, not reimplement the `live_subagents = 0` check
inline.

### `ClassifyAgentActivity` change

**Revised after review — unconditional, indefinite exemption was wrong.** The
first draft gave `live_shells > 0` the same unconditional-forever exemption as
`live_subagents > 0`. Combined with "Known limitation" below (a task can be
stuck in Running if the agent never re-checks its background shell), this
means the *one* class of task most likely to need operator attention — an
abandoned dispatch with a dangling shell entry — is exactly the class exempted
from ever surfacing as needing attention. A genuinely healthy long-running
dev server and a silently-abandoned one would render identically forever.

Instead: `live_shells > 0` exempts from the *normal* 10-minute
`active_threshold`, but a **separate, much longer** threshold applies —
`shell_stale_threshold: Duration = 4.hours` (config value, tunable) — measured
from `ShellEntry.started_at`. Past that threshold, sub_status classifies as a
distinct `stale_shell` (or equivalent), rendered distinctly from plain
`stale` so it's clear the task has a shell that's been running unusually
long, not that the agent has gone idle. This keeps ordinary builds and dev
servers (minutes to low hours) from ever falling into the original premature
staleness/Review-flip bug, while still giving an operator a signal after a
duration long enough that "abandoned" is the more likely explanation than
"legitimately still running."

### Cleanup invariants

Same discipline as `stop_pending`/`SubagentEntry` today — every point that
already clears subagent state gets the shell-entry equivalent. **Revised after
review to add two structural points the first draft missed**, both found by
checking every existing writer of `SubagentEntry`/`stop_pending`, not just the
ones the first draft happened to remember:

- `DetectCrashedAgent` — clears all `ShellEntry` rows and `live_shells = 0`
  alongside its existing subagent clear (the window's gone, so is anything it
  was running).
- **`DetachTmux`** (`docs/specs/split-pane.allium:21-36`) — **missed
  entirely in the first draft.** This rule clears `SubagentEntry`/
  `live_subagents` and then flips Running → Review itself, checking only
  `live_subagents = 0 and stop_pending and status = running`. Left unchanged,
  this reproduces the exact bug the design fixes: detaching a split pane
  (`split-pane.allium`'s ordinary detach flow) on a task with a live shell and
  no subagents would flip it to Review, live shell still running underneath.
  Its flip condition must widen through the same shared drain predicate
  described above, and it must clear `ShellEntry`/`live_shells` in its own
  write, exactly as it does for `SubagentEntry` today.
- **`DispatchTask`** (`docs/specs/dispatch.allium:121-158`) — **missed
  entirely in the first draft.** It explicitly clears
  `SubagentEntry`/`live_subagents`/`stop_pending` in its own write, guarding
  against entries left over from a prior run of the same task, rather than
  relying on the redispatched task's later `SessionStart(startup)` to sweep
  them. `ShellEntry`/`live_shells` need the same explicit clear here for
  parity, rather than trusting a downstream `SessionStart` to catch it.
- `SessionStart(source = startup)` only — see the revised session-fencing
  reasoning under "New entity: `ShellEntry`" above for why `resume`/`clear`
  are deliberately excluded here, unlike the subagent equivalent.
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
past the normal 10-minute threshold — there is no hook event for "a detached
shell exited on its own." This is an accepted tradeoff: per the discussion
that produced this design, an agent backgrounds a shell specifically because
it plans to act on the result later (check output, kill it, or use its output
to inform the next step) — that follow-up action is what supplies the stop
signal. Unlike the first draft, this is no longer an *indefinite* silent
stranding: the `shell_stale_threshold` (4 hours, see "`ClassifyAgentActivity`
change") gives an operator a distinct, actionable signal once a shell has
been "running" long enough that abandonment is the more likely explanation —
the task doesn't flip to Review (that would reproduce the original bug), but
it does stop pretending everything is fine.

## Testing plan (TDD order)

1. Allium spec update to `docs/specs/agent-health.allium` (`HookStop`'s two
   `UPDATE` conditions, `ClassifyAgentActivity`'s new `shell_stale_threshold`
   tier, new `HookShellStart`/`HookShellStop` rules, `ShellEntry` entity in
   `core.allium`) and `docs/specs/split-pane.allium` (`DetachTmux`) and
   `docs/specs/dispatch.allium` (`DispatchTask`) — spec first, per this
   repo's convention, across every rule this design touches, not just the
   ones in `agent-health.allium`.
2. Migration + `ShellEntry` CRUD tests (`src/db/tests/migrations.rs`,
   alongside the `SubagentEntry` precedent).
3. Hook-script tests (`src/setup/hooks.rs`) — Bash-with-background-true
   forwards `shell_start`; Bash without it does not; `KillBash` forwards
   `shell_stop`; `BashOutput` forwards `shell_stop` only when status is
   non-running.
4. `apply_pending_stop_if_drained` widening test — a subagent-drain-to-zero
   with `live_shells > 0` must NOT flip to Review, and vice versa; only
   both-zero flips. This is the regression test for the shared-drain-predicate
   finding.
5. Service-layer tests (`src/service/tasks/` or wherever
   `record_hook_event` is tested) — `shell_start`/`shell_stop` handling, and
   both of `try_record_stop`'s widened conditions (flip AND defer).
6. `DetachTmux` test — detaching with `live_shells > 0` and no subagents must
   NOT flip the task to Review.
7. `DispatchTask` test — redispatching a task with leftover `ShellEntry` rows
   clears them in the same write.
8. `classify_agent_activity` unit tests — `live_shells > 0` stays active past
   the 10-minute threshold but crosses into `stale_shell` past
   `shell_stale_threshold`.
9. Cleanup-invariant tests — crash and manual-move clear `ShellEntry`/
   `live_shells`; `SessionStart(startup)` clears them; `SessionStart(resume)`
   and `SessionStart(clear)` do NOT.
10. Card indicator tests/snapshots — new label composition, plus the
    `stale_shell` rendering.

## Open questions

- Whether `SessionStart(resume)` ever needs to clear `ShellEntry` after all —
  depends on whether a resumed process can still reference a prior
  background-shell registry. Confirm empirically; default (per this design)
  is not to clear on resume unless that's confirmed safe.
- Whether `BashOutput`'s `tool_response` exposes a usable structured status
  at all — see the fallback discussion under "Detecting shell stop." If not,
  return to this design before implementing, since it changes how common the
  "Known limitation" case is.
