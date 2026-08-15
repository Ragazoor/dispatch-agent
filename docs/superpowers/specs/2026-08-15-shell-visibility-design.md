# Shell Visibility Design

**Task:** #4187 — "Possible to see shell running?"
**Date:** 2026-08-15
**Status:** revised after adversarial review AND after verifying against the
real current code (several assumptions in both the original draft and the
review turned out to be wrong — see inline notes); pending implementation
plan

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

**The bug:** `HookStop`'s real implementation, `try_record_stop`
(`src/db/queries/tasks.rs:559-629`), runs two separate conditional `UPDATE`s
in one transaction: a **flip** statement (`WHERE status = 'running' AND
live_subagents = 0` → sets status = Review) and a **defer** statement (`WHERE
status = 'running' AND live_subagents > 0` → sets `stop_pending = true`). A
live background shell is invisible to both — it isn't a subagent — so a task
with a background shell still running (e.g. a dev server, a long build) gets
flipped straight to Review the moment the agent's foreground turn ends
(`live_subagents = 0` is still true). The task then sits in Review with a
process still alive underneath it, and nothing on the board reflects that.

## Goals

1. Make a live background shell visible on the card (a new label, additive to
   the existing "running · N agents" convention).
2. Fix the correctness bug: defer the Running → Review transition while a
   background shell is presumably still alive, the same way it's already
   deferred for live subagents.
3. Exempt a task with a live background shell from the *normal* 10-minute
   stale classification, but not indefinitely — see "`ClassifyAgentActivity`
   change" below.

## Non-goals

- Tracking *foreground* (synchronous) Bash calls. A synchronous call blocks
  the turn until it completes, and `Stop` cannot fire mid-blocking-call, so
  the correctness bug this design targets cannot occur for foreground calls.
  (A long foreground call can still render "stale" mid-call today, since
  `last_pre_tool_use_at` isn't refreshed until the blocking call's
  `PostToolUse` fires — that's a cosmetic staleness quirk, not the
  correctness bug, and is left alone here.)
- Perfectly detecting every background shell's exit. See "Known limitation"
  below.
- Any UI to actually "jump into" the shell's live output. Out of scope; the
  ask here is visibility on the board, not an interactive attach.

## Design

### New entity: mirrors `task_subagents` exactly, including its name shape

The real subagent machinery (verified against `src/db/queries/subagents.rs`,
`src/db/migrations.rs`, `src/models/tasks.rs`) is **not** what the first
draft assumed. Corrections:

- The table is `task_subagents(task_id, agent_id, session_id, started_at)`,
  `PRIMARY KEY (task_id, agent_id)` — not `subagent_entries`. The shell
  equivalent is **`task_shells(task_id, shell_id, session_id, started_at)`**,
  `PRIMARY KEY (task_id, shell_id)`.
- New `task.live_shells: i64` column (matching `live_subagents`'s real type,
  `i64` not `u32`), recomputed via a `sync_count`-shaped helper reading
  `COUNT(*) FROM task_shells WHERE task_id = ?`.
- Subagent lifecycle does **not** go through the generic `dispatch hook <id>
  <kind>` CLI path (that's `HookEventKind` — PreToolUse/Notification/Stop/
  UserPromptSubmit only). It's a **separate subcommand**:
  `dispatch hook-subagent <id> <start|stop|clear> [--agent-id X]
  [--session-id Y]`. The shell equivalent must mirror this shape exactly:
  `dispatch hook-shell <id> <start|stop> --shell-id <id> --session-id <id>`
  (no `clear` action — see below for why shells don't need one).

### Session fencing — reuse the existing mechanism, don't invent a new one

**This section replaces the first and second drafts' reasoning entirely.**
Both earlier drafts assumed shells need some flavor of "clear on
`SessionStart`" and argued about which sources should trigger it. That
framing is wrong, because it's solving the wrong problem with a mechanism
that doesn't exist yet (the hook script doesn't even parse `SessionStart`'s
`.source` field today — that would have been new, unverified surface area).

The real subagent machinery already solves the "stale rows from a dead
session" problem a different way, and it transfers cleanly: **`fence_session`**
(`src/db/queries/subagents.rs`, called from both `subagent_start` and
`subagent_stop`) deletes any `task_subagents` row for the task whose
`session_id` differs from the *incoming* event's `session_id`, every time a
new subagent event arrives. It doesn't wait for a `SessionStart` hook at
all — the moment any fresh subagent event shows up carrying a new session
id, every row from the old session is swept, because a new `claude` session
id proves the old rows can no longer be validly referenced.

`shell_start`/`shell_stop` should apply the identical fence on `task_shells`,
using the same top-level `session_id` field the hook payload already carries
on every event (confirmed already extracted today for the `SubagentStart`/
`SubagentStop` branches in `plugin/hooks/scripts/task-status-hook` — this is
not a new, unverified field).

**Consequence: no `SessionStart`-triggered shell clearing at all — and no
`ShellEvent::Clear` action tied to session boundaries.** This is a
deliberate, accepted asymmetry from subagents (which *do* get an
unconditional clear on every `SessionStart`, via `hook-subagent clear` →
`clear_subagents_no_drain`, precisely because if no subagent event ever
arrives in the new session, fencing alone would never fire and the stale
count would sit forever). For shells, the equivalent gap — a task that had
live shells before a resume/clear, where no new shell event ever arrives in
the new session — is treated the same way as the rest of this design's
"Known limitation": self-correcting through the other four structural clear
points, or eventually surfaced via the shell-specific staleness threshold.
This is the safer default (per the same asymmetry-of-consequences argument
used in `HookUserPromptSubmit`'s tie-breaking rule,
`docs/specs/agent-health.allium:519-525`): under-clearing resolves itself
later; a speculative `SessionStart`-driven clear risks reproducing the
original bug if it guesses wrong about whether a background shell survived
the boundary.

### Detecting shell start

Fired from `PostToolUse` for `tool_name == "Bash"` with
`tool_input.run_in_background == true` — has to be `PostToolUse` because the
shell ID is only assigned once the call returns.

`plugin/hooks/scripts/task-status-hook` gains a branch in its existing
`PostToolUse` tool_name switch (alongside the current
`Read|Write|Edit|NotebookEdit` branch) that forwards:
`dispatch hook-shell "$ID" start --shell-id "$SHELL_ID" --session-id
"$SESSION_ID"` (the latter read the same way the existing `SubagentStart`
branch already reads `.session_id`).

Service layer: `record_shell_event(id, ShellEvent::Start { shell_id,
session_id })` — mirrors `record_subagent_event`'s `Start` arm exactly,
calling a new `shell_start` query function shaped like `subagent_start`
(fence, insert-or-replace, recompute count, commit).

### Detecting shell stop

Two distinct signals, both from `PostToolUse`:

- **`KillBash`** — `tool_input.shell_id` names the shell being killed.
  Always a definitive stop signal.
- **`BashOutput`** — `tool_input.shell_id` names the shell being polled;
  `tool_response` reports its status. When that status is not "running", this
  is also a definitive stop signal for that shell_id.

Both forward `dispatch hook-shell "$ID" stop --shell-id "$SHELL_ID"
--session-id "$SESSION_ID"`. Service layer: `record_shell_event(id,
ShellEvent::Stop { shell_id, session_id })` → a new `shell_stop` query
function shaped like `subagent_stop` (fence, delete, then the shared drain
check — see next section).

**Implementation-time risks to resolve first, before writing any parsing
logic or its tests** (unchanged from the prior revision, still open):

- The exact field names for `tool_input`/`tool_response` on `Bash`,
  `KillBash`, and `BashOutput` — capture real hook payloads first.
- **Fallback if `BashOutput`'s response doesn't expose a clean, structured
  running/not-running status:** `shell_stop` can then only be driven by
  explicit `KillBash`, and a background shell that finishes on its own
  (arguably the *common* case) would never produce a stop signal. This is a
  go/no-go question to answer during implementation — if the field isn't
  usable, come back to this design before proceeding.

### `HookStop` change

`try_record_stop` (`src/db/queries/tasks.rs:559-629`) is two separate
conditional `UPDATE` statements in one transaction:

- the **flip** statement, currently `WHERE status = 'running' AND
  live_subagents = 0` — becomes `AND live_subagents = 0 AND live_shells = 0`.
- the **defer** statement, currently `WHERE status = 'running' AND
  live_subagents > 0` — becomes `AND (live_subagents > 0 OR live_shells > 0)`.

Both statements must change together — widening only the defer statement
leaves the flip statement blind to `live_shells`, and every task with
`live_subagents = 0` and a live background shell (this design's headline
scenario) keeps flipping to Review unchanged.

### The shared drain predicate

`apply_pending_stop_if_drained` (`src/db/queries/subagents.rs`) is a single
function, its `WHERE` clause `... AND stop_pending = 1 AND live_subagents =
0`. It's called from `finish_drain`, which both `subagent_stop` and
`subagent_clear` (DetachTmux's path) route through — one shared function,
specifically so a later edit can't fix one caller and silently leave the
other racy.

This function widens to `... AND live_subagents = 0 AND live_shells = 0`,
and **every caller that currently reaches it — the subagent-stop drain, and
`DetachTmux`'s subagent-clear drain — plus the new shell-stop/shell-clear
paths, all call through this same widened function.** No parallel
shell-specific copy of this predicate; that would reproduce the exact race
it exists to prevent (a subagent-drain-to-zero flipping the task while
`live_shells > 0` is still true, or vice versa).

### `ClassifyAgentActivity` / `classify_agent_activity` change

Real signature (`src/models/tasks.rs:944-963`):

```rust
pub fn classify_agent_activity(
    last_pre_tool_use_at: Option<DateTime<Utc>>,
    last_notification_at: Option<DateTime<Utc>>,
    live_subagents: i64,
    now: DateTime<Utc>,
) -> AgentActivity
```

with `AgentActivity` currently exactly `Active | Waiting | Stale`, mapped
1:1 to `SubStatus` via `to_sub_status()`.

This needs two new parameters (`live_shells: i64`, and either
`shell_started_at: Option<DateTime<Utc>>` or an oldest-shell timestamp — the
plan should decide which is simpler given how `task_shells` rows are
queried) and a **new `AgentActivity` variant** for the shell-stale tier,
which maps to a **new `SubStatus::StaleShell`** variant. `SubStatus`
(`src/models/tasks.rs:138-148`) touches four places for any new variant: the
enum itself, its `ALL` const, `is_valid_for`'s `Running` arm, and
`properties()` (needs a new priority constant alongside the existing
staleness priority).

**Revised after review — unconditional, indefinite exemption was wrong.**
`live_shells > 0` exempts from the *normal* `ACTIVE_THRESHOLD` (10 minutes,
`src/models/tasks.rs:914`), but a **separate, much longer** threshold
applies — `SHELL_STALE_THRESHOLD: Duration = 4.hours` (new constant, same
idiom as `ACTIVE_THRESHOLD`, right beside it) — measured from the oldest
live `task_shells` row's `started_at`. Past that threshold, classify as
`StaleShell`, rendered distinctly from plain `Stale` so it's clear the task
has a shell that's been running unusually long, not that the agent has gone
idle. Without this, a genuinely healthy long-running dev server and a
silently-abandoned one would render identically forever — see "Known
limitation."

### Cleanup invariants

**Revised after review to correct the actual set of structural clear
points.** The first draft's list included `ExitSession`, manual moves, and
MCP `update_task` as required clear points — **verified against the real
code, this is wrong.** None of them clear `SubagentEntry`/`live_subagents`
today. `ExitSession`'s own guidance (`docs/specs/pr-workflow.allium:483-497`)
is explicit and deliberate about this: leftover subagent rows after a normal
session close are harmless (only a `running` task's count is ever read; the
rows get swept by the next `DispatchTask` claim or the task's delete
cascade), so this was never a structural clear point to begin with. The
adversarial review's "Good Practices Observed" section affirmed the original
(wrong) claim without checking the real code — worth noting as a reminder
that even a review pass needs its factual claims checked against source, not
just its reasoning.

The **real, verified** list of structural `SubagentEntry` clear points —
which `task_shells` should mirror exactly, nothing more:

- **`DetectCrashedAgent`** (`src/tui/update/agent.rs:489-494`) — dispatches
  `TaskCommand::ClearSubagents { id, mode: DrainMode::NoDrain }` →
  `clear_subagents_no_drain`. Extend this same command/function to also
  clear `task_shells` + `live_shells` in the same transaction (the window's
  gone, so is anything it was running) — not a second command, not a second
  DB round-trip.
- **`DetachTmux`** (`src/tui/mod.rs:2007-2012`) — dispatches
  `TaskCommand::ClearSubagents { id, mode: DrainMode::Drain }` →
  `subagent_clear` → `finish_drain` (the draining path, which is what makes
  this the fix for the original bug's detach-shaped reproduction). Extend
  the same function to also clear `task_shells` in the same transaction,
  before the shared (now-widened) drain predicate runs.
- **`DispatchTask`'s two claim functions** — `claim_backlog_task`
  (`src/service/tasks/crud.rs:945`) and `claim_next_backlog_task` (`:907`) —
  both call `clear_subagents_no_drain` inline (not via a `TaskCommand`).
  Both need an equivalent `clear_shells_no_drain` call alongside it, guarding
  against shell entries left over from a prior run of the same task.
- **The shell's own stop/clear drain** — the normal path, not a special
  cleanup case: `shell_stop` draining `live_shells` to 0 with `stop_pending`
  set applies the deferred Stop via the shared predicate above.

**Not** required (correcting the first draft): `ExitSession`, manual status
moves, MCP `update_task`. `task_shells` rows left behind by any of these
follow the identical accepted precedent already established for
`task_subagents` — harmless leftover, swept by the next `DispatchTask` claim
or the task's delete cascade.

### Card rendering

`CardIndicator` (`src/tui/ui/kanban/cards.rs:45-81`), `Running { subagents:
u32 }` at lines 65-67. `classify_card_indicator` (83-181) checks
`SubStatus::Stale` (129-139) *before* reaching the `Running` branch
(143-147) — a new `StaleShell` check needs its own branch inserted at that
same tier, before the plain `Running` branch, or `Running` will shadow it.
`render_card_indicator` (190-248), `Running` label arm at 211-219.

Extend `CardIndicator::Running` with a `shells: u32` field alongside
`subagents: u32`; add a `StaleShell` (or similarly named) variant for the
new sub-status. Label composes additively:

- `running` (both zero)
- `running · 1 agent` (existing)
- `running · 2 shells` (new)
- `running · 1 agent · 2 shells` (both present)

`CardIndicator` is private to `cards.rs` (confirmed via repo-wide grep — no
other call sites to update).

## Known limitation

This only catches a background shell dying when the agent later runs
`BashOutput` or `KillBash` against it, or when session fencing happens to
sweep it (a new shell event arriving in a new session). If an agent starts a
background shell and never checks on it again, and no new shell event ever
arrives in a later session for that task, the task stays in Running past the
normal 10-minute threshold. `SHELL_STALE_THRESHOLD` (4 hours) gives an
operator a distinct, actionable signal once that's gone on long enough that
abandonment is the more likely explanation — the task doesn't flip to Review
(that would reproduce the original bug), but it does stop pretending
everything is fine.

## Testing plan (TDD order)

1. Allium spec update to `docs/specs/agent-health.allium` (`HookStop`'s two
   `UPDATE` conditions, the shared drain predicate, `ClassifyAgentActivity`'s
   new `StaleShell` tier, new `HookShellStart`/`HookShellStop` rules,
   `task_shells` entity in `core.allium`), `docs/specs/split-pane.allium`
   (`DetachTmux`), and `docs/specs/dispatch.allium` (`DispatchTask`'s claim
   functions) — spec first, across every rule this design touches.
2. Migration (next version confirmed as 85, `src/db/migrations.rs`) +
   `task_shells` CRUD tests (`src/db/tests/migrations.rs`), following the
   `migrate_v81_create_task_subagents` pattern exactly (`CREATE TABLE IF NOT
   EXISTS` + `column_exists`-guarded `ALTER TABLE`).
3. Query-layer tests for `shell_start`/`shell_stop` (fencing behavior,
   count sync) mirroring the existing (currently untested-in-isolation,
   verify) `subagent_start`/`subagent_stop` shape.
4. `apply_pending_stop_if_drained` widening test — a subagent-drain-to-zero
   with `live_shells > 0` must NOT flip to Review, and vice versa; only
   both-zero flips. Regression test for the shared-drain-predicate finding.
5. `try_record_stop` tests — both widened conditions (flip AND defer).
6. Hook-script tests (`src/setup/hooks.rs`, using the existing
   `spawn_hook_harness`/`invoke_hook` helpers) — Bash-with-background-true
   forwards `hook-shell ... start`; Bash without it does not; `KillBash`
   forwards `hook-shell ... stop`; `BashOutput` forwards `hook-shell ...
   stop` only when status is non-running.
7. CLI tests for the new `hook-shell` subcommand (mirroring however
   `hook-subagent`'s CLI parsing is tested today).
8. `DetectCrashedAgent`/`DetachTmux`/`DispatchTask`-claim tests — each clears
   `task_shells`/`live_shells` in the same transaction as its existing
   subagent clear.
9. `classify_agent_activity` unit tests — `live_shells > 0` stays active past
   `ACTIVE_THRESHOLD` but crosses into `StaleShell` past
   `SHELL_STALE_THRESHOLD`.
10. Card indicator tests/snapshots (`src/tui/tests/snapshots.rs`, following
    `snapshot_card_running_with_subagents` as the exact template) — new
    label composition, plus the `StaleShell` rendering.

## Open questions

- Whether `BashOutput`'s `tool_response` exposes a usable structured status
  at all — see the fallback discussion under "Detecting shell stop." If not,
  return to this design before implementing.
- Whether `live_shells`/staleness should read from the oldest live
  `task_shells` row or track a separate timestamp — left for the
  implementation plan to decide based on how the query is most naturally
  shaped.
