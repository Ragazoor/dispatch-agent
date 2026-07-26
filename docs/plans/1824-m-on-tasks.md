# Plan: `[m]` on tasks — move a task to another epic

Task #1824. Pressing `m` on a **task** card opens a tree picker to move the task
into another epic, or detach it to standalone ("— no parent —"). Pressing `m` on
an **epic** card keeps the existing `ReparentEpic` flow unchanged.

## Confirmed design

- Detach option **included**: picker offers "— no parent —" → clears `epic_id`.
- Acts on **any non-archived task**, from **both** board view and epic view.
- Target epics: active (not Done/Archived) + visible under the board filter; no
  cycle/descendant exclusion (tasks can't be ancestors). Mirrors the epic flow's
  re-rooting of status-filtered orphans.

## Spec (done)

`MoveTaskToEpic(task, new_epic?)` rule added to `docs/specs/tasks.allium`
(`-- == Move To Epic ==` section). `allium check` is green.

## Approach

Keep the epic reparent flow untouched (it has snapshot/scenario tests). Add a
**parallel, minimal** task-move flow that **reuses** the tree-building helpers and
a shared overlay renderer. New state field rather than generalizing
`ReparentPickerState`, to avoid churning existing epic tests.

## Layers & TDD steps

Each step: write/extend tests first, then implement to green.

### 1. Service layer

- **Test** (`src/service/tasks/tests.rs`): `move_task_to_epic`
  - moves a standalone task into an epic (epic_id set, epic recalculated);
  - moves a task between epics (both old + new epics recalculated);
  - detaches to `None` (epic_id cleared, old epic recalculated);
  - errors `NotFound` when target epic id does not exist;
  - errors when task does not exist.
- **Impl**: add `move_task_to_epic(&self, task_id, new_epic: Option<EpicId>)` to
  `TaskServiceApi` (`src/service/api.rs`) + impl in `src/service/tasks/crud.rs`:
  read prior task → old epic; if `Some(eid)` verify `get_epic` exists; call
  `db.set_task_epic_id(task_id, new_epic)`; recalc old (if any) and new (if any).
  Reuse the existing `recalculate_epic` helper.

### 2. Command + runtime

- **Impl**: add `TaskCommand::MoveToEpic { id: TaskId, new_epic: Option<EpicId> }`
  (`src/tui/commands/task.rs`).
- `dispatch_task` arm (`src/runtime/commands.rs`):
  `MoveToEpic { id, new_epic } => rt.exec_move_task_to_epic(app, id, new_epic).await`.
- **Impl** `exec_move_task_to_epic` (`src/runtime/tasks.rs`): call
  `task_svc.move_task_to_epic`; on error surface a status error; on success call
  `exec_refresh_from_db(app)` (syncs tasks + epics + review count) and return its
  commands.
- **Test** (`src/runtime/tests.rs`): exec moves task to epic and refreshes board;
  exec to `None` detaches; exec on missing target shows error.

### 3. TUI state, input, messages, handlers

- **State** (`src/tui/mod.rs`): add `move_task_picker: Option<MoveTaskPickerState>`
  (`{ task_id: TaskId, tree_state: RefCell<TreeState<String>> }`), init `None`.
- **InputMode** (`src/tui/types.rs`): add `MoveTaskToEpic(TaskId)` and
  `ConfirmMoveTaskToEpic { task_id: TaskId, new_epic: Option<EpicId> }`.
- **Messages** (`src/tui/messages/task.rs`): add `StartMoveToEpic(TaskId)`,
  `MoveToEpicNavigate(TreeNav)`, `MoveToEpicConfirm`, `MoveToEpicExecute`,
  `MoveToEpicCancel`, `MoveToEpicCancelAll`.
- **`m` key** (`src/tui/input/normal.rs`): the existing `Char('m')` arm gains an
  `else if let Some(task) = self.selected_task()` branch guarded on
  `status != Archived` → dispatch `TaskMessage::StartMoveToEpic(task.id)` +
  `key_event("move_task_to_epic", "m")`.
- **Key routing** (`src/tui/input.rs`): route `InputMode::MoveTaskToEpic` and
  `ConfirmMoveTaskToEpic` to handlers mirroring `handle_key_reparent_epic` /
  `handle_key_confirm_reparent_epic`.
- **Handlers** (new `src/tui/update/move_task.rs` or in `update/tasks` area):
  `handle_start_move_to_epic`, `handle_move_to_epic_navigate`,
  `handle_move_to_epic_confirm` (build `[y/n]` prompt, set Confirm mode),
  `handle_move_to_epic_execute` (emit `TaskCommand::MoveToEpic`, clear state),
  `handle_move_to_epic_cancel` (Confirm → picker; picker → Normal),
  `handle_move_to_epic_cancel_all`. Mirror the epic handlers in
  `src/tui/update/epics.rs`.
- **Dispatcher** (`src/tui/dispatcher.rs`): route the new TaskMessages.
- **Tests** (`src/tui/tests/`): key-sequence scenario — `m` on a task opens the
  picker; `Enter` → confirm prompt; `y` emits `MoveToEpic`; `n` returns to picker;
  `Esc` cancels. Unit tests for the cancel/confirm state transitions.

### 4. Overlay

- **Refactor** (`src/tui/ui/kanban/popups/reparent_epic.rs`): extract a shared
  `render_tree_picker(frame, area, title, eligible, tree_state)` used by both the
  epic overlay and the new task overlay; keep `build_reparent_tree` reusable.
- **Impl** new `render_move_task_overlay` (same module or sibling) keyed on
  `InputMode::MoveTaskToEpic | ConfirmMoveTaskToEpic` and `app.move_task_picker`.
  Title: `Move task: "<title>"`. Eligible epics from a new
  `App::move_task_target_epics()` helper (active + board-visible, no exclusion).
- **Register** in `src/tui/ui/kanban/mod.rs` render path + `popups/mod.rs` export.
- **Test**: snapshot of the move-task overlay (120×40 backend).

### 5. Help overlay + docs

- `src/tui/ui/kanban/popups/help.rs`: add `[m] move task to epic` entry near the
  existing epic reparent entry (clarify `m` is selection-sensitive).
- Update snapshot(s) for help overlay if present.

## Verification

`cargo test && ./scripts/check-doc-paths.sh`. Accept intentional snapshot changes
via `cargo insta review` and clean up `*.snap.new`.
