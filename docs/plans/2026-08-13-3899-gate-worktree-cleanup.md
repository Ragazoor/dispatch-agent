# Task #3899 — A failed worktree cleanup must be logged and must not clear the column blindly

Date: 2026-08-13

Follows `docs/plans/2026-08-11-3897-worktree-cleanup-investigation.md`, "Confirmed
defects" §3, and implements its Step 3.

## Decision (user, this session)

**Gate the clear on both paths.** The strong postconditions on `ArchiveTask` and
`DeleteTask` stay, and the DB write that forgets the worktree is made conditional
on the removal actually succeeding:

- Archive: a failed removal leaves the `worktree` (and `tmux_window`) column set,
  so the archived card still points at what is on disk.
- Delete: a failed removal means **the row is not deleted**. The task stays
  archived with its pointer, and the failure is surfaced. Deleting it again
  retries the removal.

Unconditionally, and independent of the above: the failure is recorded at
`ERROR` in `app.log`.

## Why the current code cannot do this

`take_cleanup` (`src/tui/mod.rs::take_cleanup`) clears `worktree`/`tmux_window`
on the in-memory task and the caller emits two *independent* commands:

- `handle_archive_task` (`src/tui/update/retry.rs`) → `Cleanup` + `Persist(snapshot)`,
  and the snapshot's cleared fields become `FieldUpdate::Clear` in
  `exec_persist_task` (`src/runtime/tasks.rs::exec_persist_task`).
- `handle_delete_task` (`src/tui/update/lifecycle.rs`) → `Cleanup` +
  `TaskCommand::Delete(id)`, and `exec_delete_task` drops the row.

`exec_cleanup` (`src/runtime/tasks.rs::exec_cleanup`) runs `cleanup_task` in a
**detached** `spawn_blocking` and only sends a `SystemMessage::Error` on failure,
so neither follow-up write can depend on the outcome.

## Shape of the fix

Keep the cleanup detached — blocking the command loop on `git worktree remove`
would freeze the TUI on every archive — and instead make the **follow-up write
happen on the cleanup's completion path** rather than beside it.

1. `TaskCommand::Cleanup` gains `follow_up: CleanupFollowUp`
   (`ClearPointer` | `DeleteRow`), declaring what a *successful* removal earns.
2. `exec_cleanup` returns its `JoinHandle` (the `exec_check_window` pattern:
   `drop(...)` at the call site, `await` in tests) and on completion sends one of
   two new messages:
   - `TaskMessage::CleanupSucceeded { id, follow_up }`
   - `TaskMessage::CleanupFailed { id, worktree, error }`
3. `CleanupSucceeded` → `ClearPointer` emits `TaskCommand::DetachWorktree(id)`
   (the existing `detach_only` write); `DeleteRow` emits
   `TaskCommand::Delete(id)`.
4. `CleanupFailed` → status-bar error **and** `app.dirty_since_refresh = true`,
   so the next refresh restores the row/pointer the board optimistically dropped.
5. `handle_archive_task` persists a snapshot that **retains** `worktree` /
   `tmux_window` (board still clears them optimistically), so the column is only
   cleared by step 3.
6. `handle_delete_task` emits `Cleanup { follow_up: DeleteRow }` and **no longer
   emits `Delete`** when the task has a worktree; with no worktree it emits
   `Delete` exactly as today.
7. `cleanup_task` / `exec_cleanup`: `tracing::error!(task_id, worktree_path,
   error = %e, "worktree cleanup failed")` on the failure branch. One log site in
   `exec_cleanup` covers every way `cleanup_task` can bail (tmux kill, worktree
   remove), with the task id in scope.

### Deliberately out of scope

- **`RetryFresh`** keeps clearing eagerly: its own `Persist` carries
  `Backlog` and the immediate re-dispatch derives the same worktree path anyway,
  so a retained pointer changes nothing observable. It emits
  `follow_up: ClearPointer` and gets the ERROR log.
- **`DeleteEpic`** stays best-effort: `delete_epic_recursive` drops the subtask
  rows in one DB call, so there is nothing left to hold a pointer. Gating it
  requires the restructure that #3897 defect §1 covers. Stated as an explicit
  exception in the spec guidance rather than left implicit.

## Steps (TDD — test first, watch it fail, then implement)

### Step 1 — spec

`allium:tend` on `docs/specs/tasks.allium`:

- `ArchiveTask` / `DeleteTask`: make the `not exists task.worktree` ensures
  conditional on the removal succeeding, and add the failure postcondition
  (pointer retained; for `DeleteTask`, the row survives and stays archived).
- `TaskTeardown`: keep "best-effort" for the *branch* step, but state that a
  failed worktree removal is reported and recorded at ERROR, and that the
  requesting operation's column-clear / row-delete does not happen.
- Guidance: name the two exceptions above (`RetryFresh`, `DeleteEpic`).

Then `allium:weed`.

### Step 2 — the ERROR log (unconditional half)

Test: `src/runtime/tests.rs` — script a runner whose `git worktree remove`
fails, `await` the handle from `exec_cleanup`, assert a `SystemMessage::Error`
reached `msg_tx`. Implement the `tracing::error!`.

### Step 3 — archive retains the pointer

Tests:
- `src/tui/tests/archive.rs`: archiving a task with a worktree emits `Cleanup`
  **and** a `Persist` whose snapshot still carries `worktree`/`tmux_window`,
  while the board task has both cleared.
- `src/runtime/tests.rs`: `exec_cleanup` failure leaves the row's `worktree`
  set; success + `CleanupSucceeded(ClearPointer)` clears it.

Implement steps 1–5 of "Shape of the fix".

### Step 4 — delete is gated on removal

Tests (`src/runtime/tests.rs`):
- `git worktree remove` fails → the row still exists, still `archived`, still
  carries its `worktree`.
- removal succeeds → `CleanupSucceeded(DeleteRow)` → the row is gone.
- `src/tui/tests/`: deleting a task **with** a worktree emits no
  `TaskCommand::Delete`; deleting one **without** a worktree still does.

Implement step 6.

## As built — two things the plan missed

Both were found by weeding the spec against the code afterwards, and both are
places where routing the follow-up through the teardown changed a path the plan
had not considered.

1. **A shared worktree is a release.** `TaskTeardown` clause 2 detaches a shared
   worktree and leaves it on disk, and that *is* a release for this task — so it
   earns the follow-up. Without this, deleting a task that shares a worktree kept
   a row nothing could reach. `exec_cleanup` now reports `CleanupSucceeded` on
   the detach path, and the spec distinguishes `worktree_removed` (gone from
   disk, what `not exists task.worktree` asserts) from `released` (removed *or*
   detached, what earns the clear).
2. **A third follow-up, `Nothing`.** The epic delete drops every subtask row in
   one operation, so a successful teardown has nothing to write back — asking it
   to clear a column on a deleted row would surface a spurious "not found". That
   path now carries `CleanupFollowUp::Nothing`.

Also: a failed *shared-worktree check* now reports `CleanupFailed` rather than a
bare error, so an unmade check is treated like an unmade release — pointer kept,
row kept.

## Verification

`cargo fmt`, `cargo test`, `cargo clippy --all-targets -- -D warnings`,
`./scripts/check-doc-paths.sh`, `allium:weed` after the spec edit.
