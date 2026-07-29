# Done column default ordering by completion time

## Goal

The Done column (and, by extension, any Done-status epic group) currently
falls back to sorting by task/epic id when no explicit `sort_order` is set —
i.e. default order is creation order, not completion order. Change the
default so that a task/epic that has just been marked Done sorts before
older completions, most-recent-first, while leaving every other column's
default ordering (by id) untouched.

## Non-goals

- No change to Backlog/Running/Review/Archived default ordering.
- No change to the manual reorder feature (`Shift+J`/`Shift+K`) — it must
  keep working inside Done exactly as it does today.
- No new "recently completed" view, filter, or UI surface — this is purely
  a default-ordering change within the existing Done column.
- No change to how Archived tasks are ordered (a task leaving Done into
  Archived reverts to Archived's normal id-based fallback, unchanged).

## Approach: reuse `sort_order`, populate it explicitly on transition

Rather than special-casing the render/sort comparators (which would mean the
Done column's ordering depends on a hidden fallback that's invisible at the
call site), the transition into/out of Done becomes the write-time trigger
for an ordinary, always-explicit `sort_order` value. No sort/render code
changes at all — `column_items_for_status_with_view_tasks`,
`column_items_for_visual_column`, and `handle_reorder_item` keep their
existing `sort_order.unwrap_or(id)` logic unmodified, because after this
change a Done task's `sort_order` is (almost) always populated.

**Core rule** — one pure function, the single source of truth, used by every
call site below:

```rust
/// Decides what a status transition should do to `sort_order`, expressed
/// as an instruction for `TaskPatch`/`EpicPatch`'s nullable `.sort_order()`
/// setter: `None` = don't touch it, `Some(v)` = write `v` (where `v` may
/// itself be `None` to clear, or `Some(ts)` to set).
fn sort_order_for_status_transition(
    prior: TaskStatus,
    next: TaskStatus,
    now: DateTime<Utc>,
) -> Option<Option<i64>> {
    match (prior == TaskStatus::Done, next == TaskStatus::Done) {
        (false, true) => Some(Some(-now.timestamp_millis())), // entering Done
        (true, false) => Some(None),                          // leaving Done
        _ => None,                                            // no transition
    }
}
```

The value is the *negated* Unix timestamp in **milliseconds** (not seconds)
so that the existing ascending `sort_by_key` comparators used everywhere
already put the most negative (= most recent) value first, with no
comparator changes. Millisecond precision is a deliberate choice, not the
obvious default: two tasks marked Done in the same wall-clock *second* are a
real, reachable case (bulk multi-select "confirm done", or the PR-poller
detecting several merges in the same 30s tick) and would otherwise tie and
silently fall back to the id-order tie-break — exactly the default this
feature replaces. Millisecond precision shrinks that window enormously
without any schema change (still a plain `i64`). A true tie is still
possible in principle and is an accepted residual gap: `column_items_for_*`
already tie-breaks on id as a second sort key, so a same-millisecond tie
degrades gracefully to id order rather than crashing or panicking.

Wherever the rule returns `Some(v)`, the caller **must apply it
unconditionally** — overwriting any `sort_order` already present in the
patch being built, not just filling in an unset field. This is not
defensive plumbing; see the `exec_persist_task` finding below for why it's
load-bearing.

This function is the one piece of new domain logic; everything else is
plumbing it into the places that already decide a task's/epic's next
status.

## Call sites

### Tasks

Per the mutation-boundary inventory, task status changes funnel through
exactly two service methods (everything else — TUI handlers, the task
editor, MCP tools — ultimately calls one of these):

- `TaskService::update_task` (`src/service/tasks/crud.rs:83-162`)
- `TaskService::cli_update_task` (`src/service/tasks/crud.rs:318-361`)

**Correction from the adversarial review**: neither method fetches the
prior task unconditionally today. Both compute
`is_finishing_status = matches!(status, Some(Done | Archived))` and only
fetch `prior` when the *target* is Done/Archived (for the existing
PR-finalisation check) — which covers *entering* Done but never *leaving*
it. Detecting "was this task Done a moment ago" requires the prior task
regardless of what the target status is. The fix: widen the fetch
condition in both methods to fire whenever `params.status.is_some()`, not
only when the target is a finishing status. This is a genuine new
always-on-status-change DB read (via `db_call_read`, the read-pool
connection — not serialized behind the single writer, so it doesn't
contend with the mutation-boundary performance model in
`docs/conventions.md`), not free reuse of existing logic.

Given `prior.status` and the requested `next_status`, call
`sort_order_for_status_transition` and **unconditionally** overwrite
`sort_order` on the `TaskPatch` being built whenever it returns `Some(v)` —
this must win over anything the caller-supplied params already set for that
field. This is not a defensive no-op: `exec_persist_task`
(`src/runtime/tasks.rs:163-192`), the funnel every TUI status-change
handler goes through, unconditionally forwards whatever `sort_order`
happens to be sitting on the in-memory `Task` struct alongside the status
change (`if let Some(so) = task.sort_order { p = p.sort_order(so); }`).
Concretely: `handle_move_task`'s backward branch
(`src/tui/update/lifecycle.rs:9-72`) moves a task Done→Review without
touching `sort_order` on the in-memory struct; if the "leaving Done" clear
isn't applied unconditionally, the stale large-negative `sort_order` value
rides along into Review/Backlog/Running and permanently pins the task to
the top of whatever column it lands in (since `sort_order.unwrap_or(id)`
always prefers a very negative number over any id). The implementation
must apply the transition rule *after* building the patch from params, as
a final override step, not merged into the params-to-patch translation.

`UpdateTaskParams`/`build_task_patch` has no way to express "clear
`sort_order`" today (it only supports "set"). Rather than add
`FieldUpdate`-style nullability to a param that's otherwise a plain setter
everywhere else, `update_task`/`cli_update_task` apply the transition rule
directly against the `TaskPatch` right before calling `db.patch_task`,
independent of the params builder.

**Feed-inserted tasks** (`upsert_feed_tasks`, `src/db/queries/tasks.rs`) are
a sanctioned bypass of `TaskService` and are explicitly out of scope for the
insert path — a feed task that starts life directly in Done keeps whatever
`sort_order` the feed supplied (e.g. a severity rank). No built-in feed does
this today. The **re-poll** path does need a guard (see Feed mitigation
below), because it can silently clobber a real completion-order value.

### Epics

Epic status transitions are **not** funneled through one place — two
independent chokepoints both need the rule:

- `EpicService::update_epic` (`src/service/epics.rs:275-341`) — manual
  TUI column-move and MCP `update_epic` path. **Correction**: this method
  has *no* existing prior-fetch to reuse (its only conditional
  `self.db.get_epic(...)` call is for an unrelated RepoGroup-reparent
  guard) — a prior-fetch gated on `params.status.is_some()` needs to be
  added from scratch here, then the same unconditional-override logic as
  the task side applied to the `EpicPatch` before calling `db.patch_epic`.
- `recalculate_epic_status_inner` (`src/db/queries/epics.rs:376-457`) — the
  automatic "all subtasks done → epic done" (and regression back to
  Backlog) rollup. This runs synchronously against a raw `rusqlite`
  connection and writes via hand-rolled SQL rather than `EpicPatch`. It
  already knows the prior status (`current`) and the computed `target`
  before issuing its `UPDATE`; call the same pure function and bind the
  result as an extra parameter in the same `UPDATE ... SET status = ?,
  sort_order = ?, updated_at = datetime('now') WHERE id = ?` (only touching
  `sort_order` in the SQL when the rule returns `Some(...)`, to avoid
  clobbering a manually-reordered value on every no-op recalculation).

### Feed mitigation

`upsert_feed_tasks`'s `ON CONFLICT DO UPDATE SET` unconditionally rewrites
`sort_order = excluded.sort_order` on every re-poll (used for feed severity
ranking, e.g. the CVE feed). Left unguarded, a feed re-poll of a task that
has since been completed would silently overwrite its completion-order
value back to a severity rank. Fix: change that one `SET` clause to a
`CASE` that only applies `excluded.sort_order` when the row's current
`status != 'done'`, leaving a Done task's `sort_order` alone on re-poll —
qualified as `tasks.status` in the `CASE`, matching the existing qualified
`CASE` style already used two lines above for `tasks.url`/`tasks.url_type`
in the same statement (`src/db/queries/tasks.rs`), rather than an
unqualified `status` reference.

### Migration

New migration `v79` (append-only, following the `v58`/`v73` backfill
pattern): for every task and every epic currently in `Done` status with
`sort_order IS NULL`, set `sort_order = -CAST(strftime('%s', updated_at) AS
INTEGER)` — an approximation using the existing `updated_at`, since no real
completion timestamp exists for historical data. Rows that already have a
non-null `sort_order` (e.g. previously manually reordered) are left
untouched.

Note the backfill is deliberately **seconds**-scale (matching `updated_at`'s
storage precision) while live transitions going forward are
**milliseconds**-scale (per the core rule above) — these two scales are not
directly comparable magnitude-for-magnitude, but this is not a bug: any
live (post-migration) completion is ~1000x more negative than any
backfilled value regardless of how soon after the migration it happens,
which correctly places the entire "completed after this feature shipped"
set above the entire "completed before, approximated" set. Ordering within
each set independently is correct (same units throughout).

## Testing plan (TDD — tests before implementation)

- **Core rule**: table-driven unit test enumerating all four
  `(prior_is_done, next_is_done)` combinations for
  `sort_order_for_status_transition`, including the "both Done" (no-op
  recalculation) and "neither Done" cases returning `None`.
- **`TaskService::update_task`**: entering Done from Review/Running/Backlog
  sets a negative-timestamp `sort_order`; leaving Done to any other status
  clears it to `None` **even when the caller's params carry a stale
  `sort_order` alongside the status change** (the `exec_persist_task`
  scenario — construct params exactly as that funnel does, with both
  `status` and a non-`None` `sort_order` set, and assert the clear still
  wins); Done→Done (e.g. an unrelated field edit while already Done) leaves
  `sort_order` untouched.
- **`TaskService::cli_update_task`**: same three cases via the CLI funnel.
- **Task editor un-archive path**: an Archived task has `sort_order = None`
  (already cleared when it left Done); editing its STATUS field back to
  `backlog` goes through `update_task` and must not error or produce a
  surprising `sort_order` — regression test confirming this reachable
  (if unvalidated) transition behaves correctly, since the design's
  reasoning here relies on `sort_order` already being `None` by the time a
  task leaves Done, not on restore-from-archive being unreachable.
- **`recalculate_epic_status_inner`**: all-subtasks-done transition sets
  `sort_order`; regression back to Backlog (per the existing
  `..._done_regresses_to_backlog_when_running_task_added` test) clears it;
  already-Done no-op recalculation (per
  `..._all_done_stays_done_when_already_done`) leaves it untouched.
- **`EpicService::update_epic`**: manual status set to Done sets
  `sort_order`; manual status set away from Done clears it (this method has
  no prior-fetch today — the added fetch, gated on `params.status.is_some()`,
  needs its own dedicated test coverage, not just reuse of an existing one).
- **Feed upsert**: re-poll of an existing Done-status feed task does not
  change its `sort_order`; re-poll of a non-Done feed task still updates
  `sort_order` from the feed item as before.
- **Migration v79**: backfills null `sort_order` for existing Done tasks/
  epics from `updated_at`; does not touch already-set `sort_order`; no-ops
  for non-Done rows.
- **Rendering regression**: existing `column_items_for_status`/reorder tests
  continue to pass unchanged (no comparator code touched); add one test
  confirming Done column tasks with auto-populated `sort_order` render
  most-recent-first, and one confirming `Shift+J`/`Shift+K` still swaps
  correctly between two Done tasks that both have auto-populated values.

## Allium spec updates

- `docs/specs/tasks.allium`: `ConfirmDone`, `MoveTaskForward`/
  `MoveTaskBackward` (the transition-into/out-of-Done paths), and
  `ArchiveTask` need `ensures: task.sort_order` clauses reflecting the new
  rule. The existing `ReorderItem` rule's `sort_order ?? id` fallback
  description stays accurate and unchanged.
- `docs/specs/epics.allium`: the epic status-recalculation rule and
  `update_epic`'s manual status transition need the equivalent `ensures`
  clauses.
- `docs/specs/feeds.allium`: note the re-poll guard (never overwrites
  `sort_order` on a Done-status task).

## Known limitations (accepted)

- A feed task that is *inserted* directly into Done status keeps the feed's
  own `sort_order` (e.g. severity rank) rather than a completion-order
  value. No built-in feed does this today; fixing it is out of scope.
- Historical Done tasks' backfilled `sort_order` is approximated from
  `updated_at`, which may have been bumped by edits after actual
  completion — accepted as good-enough for pre-existing data.
