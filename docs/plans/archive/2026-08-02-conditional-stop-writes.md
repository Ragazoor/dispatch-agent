# Make subagent Stop/drain writes conditional, retiring the tick reconciler

Task #3849. Follow-up to #3755.

## Problem

Three writes decide the same thing — "should this Running task flip to Review?" —
and only one of them decides it against the row's committed state.

`try_apply_pending_stop` (`src/db/queries/tasks.rs:591`) is the correct shape: a
single `UPDATE … WHERE id = ? AND status = 'running' AND stop_pending = 1 AND
live_subagents = 0`. The predicate is part of the statement, so no other process
can move the row between the decision and the write.

The two upstream paths are read-then-write, across process boundaries (every
Claude Code hook is its own `dispatch` process, learning #355):

1. **`record_hook_event`, `Stop` arm** (`src/service/tasks/crud.rs:711-723`) —
   reads the task, branches on the snapshot's `task.live_subagents`, writes that
   decision as a `TaskPatch`.
2. **`record_subagent_event`, drain check** (`src/service/tasks/crud.rs:796`) —
   reads the task *before* `subagent_stop`, then gates the flip on the pre-read
   `task.stop_pending` / `task.status`.

Interleaved, both can miss: the `Stop` sets `stop_pending` after the count has
already reached zero, while the drain saw `stop_pending = false`. That leaves
`Running + stop_pending + live_subagents = 0` — a task that never leaves Running
without a human. `ReconcileStrandedPendingStop` / `tick_stranded_pending_stop`
exists solely to mop that up on the next tick.

## Approach

Reshape both writes as conditional statements evaluated at write time. Then
whichever process commits second observes the other's committed value and
computes the correct outcome, the stranded state becomes unreachable, and the
backstop is dead code.

### Why the stranded state becomes unreachable

The argument does not rest on enumerating interleavings — that only ever covers
the participants you thought of. It rests on an invariant over the *single*
transaction that can create the state and the *single* transaction that must
clear it:

- `stop_pending = 1` is written in exactly one place: statement B of
  `try_record_stop`, whose `WHERE` requires `live_subagents > 0`. So the instant
  the bit is set, the count is non-zero — the triple cannot be created directly.
- The count can only reach zero via `subagent_stop` / `subagent_clear`, and both
  now apply the deferred Stop **in the same transaction** that writes the count.
  So the triple cannot be reached by the count falling either.

Every other writer of these columns (`clear_subagents_no_drain`,
`UserPromptSubmit`) sets `stop_pending = 0`, which moves *away* from the triple.

That leaves no transaction that commits into `Running + stop_pending +
live_subagents = 0`. Because each is one transaction rather than a
read-decide-write pair, this holds regardless of how many processes participate,
in what order they commit, and whether any of them dies partway — a killed
process either committed its whole transaction or none of it.

Two processes still make a useful sanity check. **S** = the main agent's `Stop`,
**D** = the last `SubagentStop`:

| Commit order | S | D | Result |
|---|---|---|---|
| S, then D | `live > 0` → defers | drains to 0, sees the bit → flips | Review |
| D, then S | `live = 0` → flips immediately | drains to 0, no bit → no-op | Review |

WAL plus SQLite's single-writer lock (`src/db/mod.rs:1030-1036`,
`busy_timeout=5000`) serialises those commits across OS processes, and both
statements in `try_record_stop` are writes, so its transaction takes the write
lock at statement A — there is no read-then-upgrade snapshot hazard.

### Scope note: what this does *not* fix

`UserPromptSubmit` (`src/service/tasks/crud.rs:724-732`) still force-clears
`stop_pending` from a pre-read snapshot. Racing a concurrent deferred `Stop`, it
can void a legitimate pending bit, leaving a task Running after its subagents
drain. That is a **different** failure (bit ends up `false`, not the stranded
triple), it is pre-existing, and the reconciler being retired never caught it
either — so retiring the reconciler does not make it worse. Out of scope here;
worth a follow-up task.

## Design

### New: `StopOutcome` + `TaskCrud::try_record_stop`

```rust
pub enum StopOutcome { Flipped, Deferred, NoOp }
```

`try_record_stop(id) -> Result<StopOutcome>`, in one `unchecked_transaction()`
(load-bearing — `db_call` opens no transaction and only serialises within one
process; learning #355):

- **Statement A** — the flip's SET list, with
  `WHERE id = ?1 AND status = 'running' AND live_subagents = 0`. No
  `stop_pending` precondition — and note this is **not** a behaviour change:
  today's `Stop` arm (`src/service/tasks/crud.rs:711-723`) already flips
  whenever the snapshot's `live_subagents == 0`, without consulting
  `stop_pending`. The SQL is a faithful translation. Adding the precondition
  would be the regression: on a row already carrying a stale bit, statement A
  would fail and statement B would fail too (`live` is not `> 0`), leaving it
  stranded. `rows == 1` → `Flipped`.
- **Statement B** — only when A wrote nothing:
  `UPDATE tasks SET stop_pending = 1, updated_at = datetime('now')
   WHERE id = ?1 AND status = 'running' AND live_subagents > 0`.
  `rows == 1` → `Deferred`, else `NoOp` (task left Running, or vanished).

`live_subagents > 0` is written explicitly rather than left implied by A's
failure, so each statement is independently readable.

### The drain moves *inside* the subagent transaction

The obvious version of this change — have `record_subagent_event` call
`try_apply_pending_stop` after `subagent_stop` returns — is not enough. Those are
two separate `db_call`s, so there is an open window between the count reaching
zero and the flip being applied. A `dispatch hook-subagent stop` process killed
in that window (the user quits `claude`, the pane dies, the machine sleeps)
leaves the row stranded with **no further hook coming to fix it** — and the tick
reconciler being deleted is exactly what used to sweep that up.

So the drain becomes part of the same transaction as the count mutation.
`src/db/queries/subagents.rs` gains:

```rust
/// Apply a deferred Stop when the count has just reached zero. Caller must
/// hold the transaction that wrote that count.
fn apply_pending_stop_if_drained(tx: &Connection, task_id: i64) -> Result<bool>
```

which runs the conditional `UPDATE … WHERE id = ? AND status = 'running' AND
stop_pending = 1 AND live_subagents = 0`, called from `subagent_stop` and
`subagent_clear` after `sync_count`, inside their existing
`unchecked_transaction()`. Both then return `SubagentDrain { live,
applied_pending_stop }` so the service layer still knows whether to recalculate
the epic.

This makes fence → mutate → count → flip a single atomic step. There is no
window for a process death to strand a row, and the `live == 0` short-circuit
question disappears — the count is checked in the same transaction that wrote it.

**`subagent_clear` must split in two.** It is currently shared by the draining
caller (detach) *and* the non-draining one (`clear_subagents_no_drain`, used by
crash, dispatch-claim and `SessionStart`). Folding the drain into it would flip
a task to Review on `SessionStart`, which `ClearSubagentsOnSessionStart`
explicitly forbids — a Stop deferred by the previous turn is stale and must be
voided, not applied. So:

- `subagent_clear` — drains (detach only).
- `subagent_clear_no_drain` — clears entries, zeroes the count, and sets
  `stop_pending = 0`, all in one transaction.

The second also fixes a pre-existing seam: `clear_subagents_no_drain` used to
clear the entries and then void the bit in a *separate* `patch_task`, so a
`SubagentStart` landing between the two was counted against a task whose bit was
about to be wiped.

### `try_apply_pending_stop` is then deleted too

With the drain folded in, the standalone DB method has no callers left. Its SQL
survives as `apply_pending_stop_if_drained`; its five tests in
`src/db/tests/subagents.rs:268-362` move to driving `subagent_stop` /
`subagent_clear` and asserting the same outcomes, so the coverage of "the
conditional write does nothing when a subagent is live / when there is no
pending bit / when the task left Running" is kept, not dropped.

### `record_hook_event` — `Stop` arm

The `Stop` case leaves the `TaskPatch` match entirely and takes the new write
path. The `get_task` up front stays (the `NotFound` contract and the other arms
need it), but the `Stop` *decision* no longer consults the snapshot — including
the `task.status == Running` guard, which moves into the `WHERE`.

`recalc` becomes `matches!(outcome, StopOutcome::Flipped)` — more precise than
today's `patch.status.is_some()`, and derived from what the DB actually wrote.

### `record_subagent_event` — drain check

The service-layer condition disappears entirely. `subagent_stop` /
`subagent_clear` now report whether they applied the flip, and the only thing
left to do is the epic recalculation:

```rust
if flipped {
    self.recalculate_epic_for_task(id).await;
}
```

`task.stop_pending`, `task.status` and the `live == 0` check all drop out — the
DB transaction owns the decision now. The `get_task` at the top of the function
stays only for the `NotFound` contract.

### Retired

| Thing | Location |
|---|---|
| `TaskService::apply_pending_stop` | `src/service/tasks/crud.rs:836-856` |
| `apply_pending_stop` trait seam | `src/service/api.rs:269-278` |
| `TaskCrud::try_apply_pending_stop` | `src/db/mod.rs:211-221`, `src/db/queries/tasks.rs:591-613` |
| `TaskCommand::ApplyPendingStop` | `src/tui/commands/task.rs:119-124` |
| dispatch arm | `src/runtime/commands.rs:213-215` |
| `exec_apply_pending_stop` | `src/runtime/tasks.rs:236` |
| `tick_stranded_pending_stop` + its call | `src/tui/update/agent.rs:339-357`, `:166` |
| tick tests + `apply_pending_stop_ids` helper | `src/tui/tests/dispatch.rs:2426-2496` |
| `apply_pending_stop_resolves_a_stranded_task` | `src/service/tasks/tests.rs:3047-3082` |
| `ReconcileStrandedPendingStop` rule | `docs/specs/agent-health.allium:406-437` |

## Steps

Spec first, then tests, then code.

### 1. Spec (`allium:tend`)

- `HookStop` — state that the branch is evaluated against the task's committed
  state in the same write, not a prior read; a Stop arriving with
  `live_subagents = 0` flips even when `stop_pending` was already set.
- `HookSubagentStop` — the drain condition is unchanged semantically; update the
  guidance to say the count and the flip are one atomic step, so a process death
  between them cannot strand the task. Remove the trailing paragraph (`:349-352`)
  pointing at the backstop.
- Delete `ReconcileStrandedPendingStop` (`:406-437`).
- Add guidance recording *why* the backstop is gone — the invariant argument
  above (the bit is only ever set with `live > 0`; the count only ever reaches
  zero in a transaction that also applies the flip), not an interleaving
  enumeration.
- Run `allium:weed` at the end to confirm alignment.

### 2. Tests — DB (`src/db/tests/subagents.rs`)

New, all red before step 4:

- `record_stop_flips_immediately_when_no_subagents_are_live` → `Flipped`;
  Review, both timestamps null, `stop_pending` false.
- `record_stop_defers_when_a_subagent_is_live` → `Deferred`; still Running,
  `stop_pending` true, timestamps untouched.
- `record_stop_is_a_noop_for_a_task_that_is_not_running` → `NoOp`, row
  unchanged.
- `record_stop_flips_a_task_that_already_carries_a_pending_stop` → `Flipped`.
  This is the re-fired-Stop case that statement A's missing `stop_pending`
  precondition exists for.

Rework the five existing `try_apply_pending_stop` tests (`:268-362`) to drive
`subagent_stop` / `subagent_clear` instead of the deleted method, keeping every
assertion: flips a drained pending stop; does nothing while a subagent is still
live; does nothing without the pending bit; does nothing once the task left
Running; idempotent on a second call.

Plus one the old design could not express:

- `a_killed_process_cannot_strand_a_drained_task` — assert `subagent_stop`
  leaves no state in which the count is zero, the bit is set and the status is
  Running, i.e. the flip is not a separate follow-up write. The existing
  abort-trigger harness at `:113-121` is the tool: it installs a trigger that
  aborts a late statement in the transaction, so it can prove the flip and the
  count commit or roll back together.

### 3. Tests — service (`src/service/tasks/tests.rs`)

The race tests are the point of the task:

- `a_stop_landing_after_the_last_subagent_drained_flips_immediately` — the
  previously-stranding interleave. Start a subagent, `record_subagent_event`
  (Stop) drains to 0 with no pending bit, then `record_hook_event(Stop)`.
  Assert Review, not `Running + stop_pending`.
- `a_deferred_stop_is_applied_by_the_last_drain` — the opposite order; asserts
  the existing behaviour still holds through the new path.
- `neither_hook_order_can_strand_a_task` — run both orders and assert the
  `Running + stop_pending + live_subagents = 0` triple never holds afterwards.
  This is the assertion that replaces the deleted reconciler test.

Existing tests that must stay green unmodified:
`stop_with_live_subagents_defers_the_review_flip` (`:2814`), the drain test at
`:2877`, and the SessionStart-voids-a-stale-Stop coverage in `tests/cli.rs:727`.

### 4. One-shot migration for already-stranded rows

Retiring the reconciler removes the only thing that fixes a task **already**
stranded in a user's existing database — written by the current racy code before
they upgrade. Nothing else would ever resolve it: no subagent is left to drain
it, `Stop` does not re-fire, and `PreToolUse` never touches status. The task
would sit in Running forever.

So the retirement needs a migration (v82) doing once what the tick did every 2s:

```sql
UPDATE tasks
   SET status = 'review', sub_status = <default_for(review)>,
       last_pre_tool_use_at = NULL, last_notification_at = NULL,
       stop_pending = 0, updated_at = datetime('now')
 WHERE status = 'running' AND stop_pending = 1 AND live_subagents = 0
```

Same predicate and same SET list as `apply_pending_stop_if_drained` —
deliberately, so "what resolving a strand writes" keeps one definition.

This is the *only* thing the migration is for. Post-fix the triple is
unreachable (see the invariant above), so this sweeps pre-fix rows once and is
never needed again — which is exactly why a one-shot migration is the right
shape rather than a reduced-frequency tick.

Epic status is derived from task status and this bypasses
`recalculate_epic_status` (no startup recalc exists — the call sites are the
service layer and `src/feed/`). A flipped task can leave its epic's status stale
until the next mutation in that epic recalculates it. Rather than reach into
`recalculate_epic_status_inner` from a migration, accept the staleness and note
it: the window is one migration, the rows affected are already-broken tasks, and
any subsequent write to the epic self-heals it.

Test in `src/db/tests/migrations.rs`: seed a stranded row against the pre-v82
schema, run migrations, assert Review + cleared bit; and assert the migration
does *not* touch a Running task with `live_subagents > 0` or one without the
pending bit.

### 5. Implement

`StopOutcome` in `src/models/tasks.rs`; `try_record_stop` on the `TaskCrud`
trait (`src/db/mod.rs`) and its impl (`src/db/queries/tasks.rs`);
`apply_pending_stop_if_drained` in `src/db/queries/subagents.rs` plus the
`(count, flipped)` return change; rewire the two service paths; then delete the
retired items in the order listed above (TUI test → tick → command → runtime →
service → seam → DB method), so the tree compiles at each step.

### 6. Verify

`cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`, plus
`cargo clippy --all-targets -- -D warnings` (the pre-push gate; a plain build
will not catch it). `./scripts/check-doc-symbols.sh` will flag the retired
identifiers if any doc still names them — that is the cross-check that the
retirement is complete. Baseline to beat: 4080 passed, 1 ignored.

## Risks

- **`recalculate_epic_status` coverage.** The flip now happens in two places
  (`try_record_stop`, `apply_pending_stop_if_drained`); both callers still call
  `recalculate_epic_for_task`. Note an epic-status *assertion* would be vacuous:
  `recalculate_epic_status_inner` only moves an epic when every child is Done,
  so a Running → Review flip cannot change it. The call is kept for the
  mutation-boundary convention, not for an observable effect here.
- **Migration flips task status.** v82 moves tasks to Review without user
  action. Scoped to the stranded triple, which is by definition a task whose
  agent already signalled Stop, so Review is where it belonged — but it is still
  a status write on upgrade and should be called out in the commit message.
- **Optimistic TUI state.** `tick_stranded_pending_stop` cleared `stop_pending`
  locally to avoid re-submitting each tick. Nothing replaces it, and nothing
  needs to — the board picks up the flip on the next `RefreshFromDb`, the same
  way it does for every other hook-driven status change.
- **`docs/superpowers/specs/2026-08-01-subagent-count-design.md`** describes the
  tick reconciler. It is a dated design artifact (excluded from the doc-path and
  doc-symbol checkers) and stays as the historical record of #3755; the living
  spec is `agent-health.allium`.
