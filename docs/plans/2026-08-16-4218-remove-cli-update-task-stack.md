# 4218 — Remove the dead `cli_update_task` / `update_status_if` / `list_by_status` stack

Follow-up from #4212 (WP-5). Deleting `dispatch update` and `dispatch list` left three
methods alive only through their own tests.

## Decision: the `only_if` compare-and-set goes too

The task asks for an explicit call. **Delete it.** Reasons:

- No production caller, and no pending design that wants one. Restoring it is one
  `git revert` away if a future CLI/hook needs a conditional status write.
- It is not free capacity — it is a *hazard* kept warm. `update_status_if` writes
  `status`/`sub_status` only and does **not** apply the prior-status-derived rules
  (Done sort_order rank, leaving-Running `stop_pending` clear). Its own doc comment
  warns that a second caller must follow up with a patch or `PendingStopOnlyWhileRunning`
  (`docs/specs/core.allium`) can be violated. A dead method with a "read this before
  calling me" footgun is worse than no method.
- Keeping it means keeping the `TaskServiceApi` seam entry, the trait method on
  `TaskCrud`, and ~12 tests, for zero production behaviour.

## Scope

Delete:

1. `TaskService::cli_update_task` (`src/service/tasks/crud.rs`) + its `task_service_api!`
   entry in `src/service/api.rs` (macro regenerates trait/impl/stub — nothing hand-written
   to chase).
2. `TaskCrud::update_status_if` (`src/db/mod.rs` decl, `src/db/queries/tasks.rs` impl).
3. `TaskRead::list_by_status` (`src/db/mod.rs` decl, `src/db/queries/tasks.rs` impl).

No production behaviour changes — nothing calls any of them.

## TDD shape

This is a deletion, so "test first" means *porting the tests first*: rewrite each surviving
test onto the surface that remains, watch it pass against the current code (proving the port
is faithful, not that the deletion happened to compile), then delete the dead code and the
tests that only tested it.

### Step 1 — port the tests, run green against unchanged code

`src/db/tests/epics.rs` (4 tests, all epic-recalculation coverage — the behaviour under test
is the recalc, not the CLI funnel). Drive `TaskService::update_task` with
`UpdateTaskParams::for_task(id).status(..)`; the `only_if` argument in each is satisfied by
the task's actual current status, so the conditional adds nothing:

- `cli_update_conditional_task_to_review_leaves_epic_in_backlog` → rename to
  `update_task_to_review_leaves_epic_in_backlog`
- `cli_update_unconditional_task_to_running_leaves_epic_in_backlog` →
  `update_task_to_running_leaves_epic_in_backlog`
- `cli_update_epic_stays_backlog_when_review_task_completes` →
  `epic_stays_backlog_when_review_task_completes`
- `cli_update_with_substatus_keeps_task_running_and_epic_in_backlog` →
  `update_task_with_substatus_keeps_task_running_and_epic_in_backlog`
  (uses `.sub_status(SubStatus::NeedsInput)` on the params builder)

`src/service/tasks/tests.rs`:

- Keep, ported to `update_task`: `..._updates_status_unconditionally` (→
  `update_task_updates_status`), `..._entering_done_sets_sort_order`,
  `..._leaving_done_clears_sort_order`, `..._unconditional_sets_sub_status`,
  `..._recalculates_parent_epic`, `cli_update_task_to_done_notifies_watcher`.
  **Check first** whether an equivalent `update_task` test already exists — the
  sort_order and watcher rules are shared code (`with_status_transition`,
  `notify_watchers_after_status_write`); where a duplicate already covers it, drop the
  `cli_` one rather than porting a second copy.
- `cli_update_task_moving_out_of_running_clears_stop_pending`: the first half ports to
  `update_task`; the second half (the "conditional branch takes a different write path"
  block) dies with `only_if`. Verify the `update_task` twin
  (`..._moving_out_of_running_clears_stop_pending`) already exists — if so, delete rather
  than port.
- Delete outright (only_if-specific, nothing left to assert):
  `cli_update_task_skipped_by_only_if_keeps_stop_pending`,
  `..._only_if_not_matching_does_not_touch_sort_order`,
  `..._only_if_matching_entering_done_sets_sort_order`,
  `..._with_only_if_matching_returns_true_and_updates`,
  `..._with_only_if_not_matching_returns_false_and_preserves_status`,
  `..._conditional_sets_sub_status_when_matching`,
  `..._conditional_does_not_apply_sub_status_when_not_matching`.

`src/db/tests/tasks.rs`:

- Delete `update_status_if_matching`, `update_status_if_not_matching`,
  `update_status_if_nonexistent`, `update_status_if_resets_sub_status_to_default`,
  `update_status_if_leaves_sub_status_unchanged_when_condition_fails`. The sub_status-reset
  invariant they guard is independently covered by the `TaskPatch::status()` auto-reset
  tests and the migration-16 CHECK constraint test
  (`check_constraint_rejects_review_with_active_substatus`) — confirm before deleting.
- Delete `list_by_status` (the query test); `list_all` already covers ordering and decode.
- Fold `list_by_status_skips_row_with_unrecognised_status` into
  `list_all_skips_row_with_unrecognised_status` — same `db_with_undecodable_status_row`
  fixture, same `collect_decodable` row decoder. Nothing to add: the `list_all` test
  already asserts exactly the same thing plus the fallback counter. Delete the
  `list_by_status` variant and note the shared decoder in a comment on the survivor.

Run `cargo test` here — everything green with the old code still present.

### Step 2 — delete the code

`src/service/api.rs` seam entry, `src/service/tasks/crud.rs::cli_update_task`,
`src/db/mod.rs` two trait decls, `src/db/queries/tasks.rs` two impls.

Compiler + `cargo clippy --all-targets -- -D warnings` prove nothing else called them.

Also fix the now-orphaned doc references (the `check-doc-symbols.sh` gate will reject them):

- `docs/conventions.md:84` — drop `list_by_status` from the bulk-reads-skip-and-warn list.
- `docs/conventions.md:147` — the `TaskPatch<'a>` paragraph cites `cli_update_task` as the
  "build one patch up front" example. Repoint at a live example or drop the clause,
  keeping the `with_status_transition` half.
- `src/service/tasks/crud.rs:448` — `notify_watchers_after_status_write` doc says "Shared by
  `update_task` and `cli_update_task`". Now one caller; reword.

### Step 3 — spec

`docs/specs/task-watchers.allium`:

- `NotifyWatchersOnFinish` `@guidance` (~line 145): drop the
  `TaskServiceApi::cli_update_task` hook point, leaving `update_task` as the sole one.
- Known-limitation note (~line 158): "bypassing TaskService::update_task and
  cli_update_task entirely" → just `update_task`.

Pure editorial — the observable rule (watchers notified on a finishing transition through
TaskService) is unchanged, because `cli_update_task` had no production caller producing
those transitions. Run `allium check`.

### Step 4 — verify

`cargo test` (full, redirected to a file — never piped), `cargo clippy --all-targets --
-D warnings`, `cargo fmt`, `./scripts/check-doc-paths.sh`, `./scripts/check-doc-symbols.sh`.
