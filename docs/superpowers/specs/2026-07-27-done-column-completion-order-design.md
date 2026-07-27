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
        (false, true) => Some(Some(-now.timestamp())), // entering Done
        (true, false) => Some(None),                    // leaving Done
        _ => None,                                      // no transition
    }
}
```

The value is the *negated* Unix timestamp (seconds) so that the existing
ascending `sort_by_key` comparators used everywhere already put the most
negative (= most recent) value first, with no comparator changes.

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

Both already read the prior task via `self.db.get_task(...)` when the
target status needs inspecting (the existing PR-finalisation check is the
precedent for this shape). In both methods, after resolving `prior.status`
and the requested `next_status`, call `sort_order_for_status_transition` and
fold its result into the `TaskPatch` being built — overriding whatever
`sort_order` the caller-supplied params already set for that field (no
existing caller sets both `status` and `sort_order` in the same call, so
there's no real conflict to arbitrate).

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
  TUI column-move and MCP `update_epic` path. Same shape as the task
  version: read prior epic, compute the transition, fold into the
  `EpicPatch` alongside `status`.
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
`status != 'done'`, leaving a Done task's `sort_order` alone on re-poll.

### Migration

New migration `v79` (append-only, following the `v58`/`v73` backfill
pattern): for every task and every epic currently in `Done` status with
`sort_order IS NULL`, set `sort_order = -CAST(strftime('%s', updated_at) AS
INTEGER)` — an approximation using the existing `updated_at`, since no real
completion timestamp exists for historical data. Rows that already have a
non-null `sort_order` (e.g. previously manually reordered) are left
untouched.

## Testing plan (TDD — tests before implementation)

- **Core rule**: table-driven unit test enumerating all four
  `(prior_is_done, next_is_done)` combinations for
  `sort_order_for_status_transition`, including the "both Done" (no-op
  recalculation) and "neither Done" cases returning `None`.
- **`TaskService::update_task`**: entering Done from Review/Running/Backlog
  sets a negative-timestamp `sort_order`; leaving Done to any other status
  clears it to `None`; Done→Done (e.g. an unrelated field edit while
  already Done) leaves `sort_order` untouched.
- **`TaskService::cli_update_task`**: same three cases via the CLI funnel.
- **`recalculate_epic_status_inner`**: all-subtasks-done transition sets
  `sort_order`; regression back to Backlog (per the existing
  `..._done_regresses_to_backlog_when_running_task_added` test) clears it;
  already-Done no-op recalculation (per
  `..._all_done_stays_done_when_already_done`) leaves it untouched.
- **`EpicService::update_epic`**: manual status set to Done sets
  `sort_order`; manual status set away from Done clears it.
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
