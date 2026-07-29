# 3758 — `x` moves tasks to Done, archives only from Done

## Problem

Today `x` archives the selected task, with one special case: a Review task (or an
all-Review multi-selection) is moved forward to Done instead. That means `x` on a
Backlog or Running task skips straight to Archived, which is almost never what the
user wants — archiving is the exception, completing is the norm.

## Target behaviour

`x` on a **task** (single or multi-selected):

| Selection | New behaviour |
|---|---|
| Single task, status ≠ Done | Prompt `Move "<title>" to Done? [y/n]` → moves to Done |
| Single task, status = Done | Prompt `Archive task? [y/n]` → archives (unchanged) |
| Tasks only, all Done | Prompt `Archive N items? [y/n]` → batch archive (unchanged) |
| Tasks only, at least one not Done | Prompt `Move N tasks to Done? [y/n]` → moves the non-Done ones to Done; already-Done tasks in the selection are left alone |
| Selection contains any epic | Archive prompt (unchanged) |
| Single epic | Epic archive confirmation (unchanged) |
| Archive panel (`x` on an archived task) | Delete confirmation (unchanged) |

The `Shift+L` forward-move path is unchanged: Review → Done still routes through the
same `ConfirmDone` prompt, and Backlog → Running / Running → Review still move one
step without confirmation.

Moving to Done via `x` reuses the existing `ConfirmDone` machinery, so it keeps the
existing semantics: kill the tmux window (`take_detach`), clear agent tracking,
reset `sub_status`, respawn the split pane — and, unlike archive, **does not** remove
the worktree.

## Design

Two focused changes plus a spec update.

### 1. `handle_confirm_done` (`src/tui/update/lifecycle.rs:74`)

The per-task guard `if task.status != TaskStatus::Review { continue; }` becomes
"skip tasks that are already Done or Archived". Everything else is untouched.

### 2. New helper `prompt_move_to_done(ids)` (`src/tui/update/lifecycle.rs`)

Sets `select.pending_done = ids`, `input.mode = ConfirmDone(ids[0])`, and the status
message — `Move "<title>" to Done? [y/n]` for one id, `Move N tasks to Done? [y/n]`
for several. No-ops on an empty id list. This unifies the single-task and batch
prompts, which `handle_confirm_done` already handles via `pending_done`.

### 3. `handle_key_archive_item` (`src/tui/input/normal.rs:687`)

Rewritten to the table above:

- selection with epics → `ConfirmArchive(None)` (unchanged)
- tasks-only selection: partition on `status == Done`; all Done → `ConfirmArchive(None)`,
  otherwise → `prompt_move_to_done(non_done_ids)`
- no selection, epic → `EpicMessage::ConfirmArchive` (unchanged)
- no selection, task: `Done` → `ConfirmArchive(Some(id))`, else `prompt_move_to_done(vec![id])`

The current Review special-cases (both the `BatchMove { Forward }` route and the
`handle_key_move(Forward)` route) are removed — they're subsumed by the general rule.

### 4. Docs

- `docs/specs/tasks.allium`: relax `ConfirmDone`'s `requires: task.status = review`
  to `!= done` / `!= archived`, rewrite its `ArchiveKeyOnReviewTask` guidance to the
  new rule, and update the `ArchiveTask` / `BatchArchive` guidance that references it.
- `docs/reference.md:33`: update the `x` key row.

## TDD steps

Each step: write/adjust the test, watch it fail, then implement.

1. **Spec first** — update `docs/specs/tasks.allium` (`ConfirmDone`, `ArchiveTask`,
   `BatchArchive` guidance) via `allium:tend`.
2. `src/tui/tests/archive.rs` — new tests:
   - `x_key_on_backlog_task_enters_confirm_done` (mode is `ConfirmDone`, task unchanged
     until `y`)
   - `confirm_x_on_backlog_task_moves_to_done` (`y` → status `Done`, not `Archived`)
   - `x_key_on_running_task_moves_to_done_and_kills_window` (asserts a detach/kill
     command is emitted and the worktree is preserved)
   - `x_key_on_done_task_enters_confirm_archive`
   - `x_key_on_mixed_status_selection_moves_non_done_to_done` (Review + Done selection:
     Review → Done, the Done one untouched)
   - `x_key_on_all_done_selection_archives`
3. Adjust the existing tests whose premise changed:
   - `x_key_enters_confirm_archive_mode` → retarget to a Done task
   - `confirm_archive_y_emits_archive_task` → retarget to a Done task
   - `confirm_archive_n_cancels` → assert the cancel path from the Done prompt
   - `x_key_on_mixed_status_selection_still_archives_all` → replaced by the new
     mixed-status test
   - `x_key_on_review_task_...` / `x_key_on_all_review_selection_...` still pass
     unchanged (Review is just one of the "not Done" statuses now)
4. Implement 1–3 above; run `cargo test`.
5. Refresh snapshots if any status-bar snapshot changed
   (`INSTA_UPDATE=always cargo test tui::tests::snapshots`, then remove `*.snap.new`).
6. `allium:weed` to confirm spec/code alignment.
7. Verify: `cargo test && ./scripts/check-doc-paths.sh`.

## Out of scope

- Epic behaviour under `x` (still archive).
- The archive panel's `x` = delete binding.
- Any change to `Shift+L` / `Shift+H` movement.
