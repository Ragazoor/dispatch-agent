# 3793 — Make `InputMode::ConfirmDone` a unit variant

## Note on sequencing

Work started before #3758 had landed on `main`. At that point `prompt_move_to_done`
did not exist and the `TaskId` payload was still live: `handle_move_task` set
`ConfirmDone(id)` without writing `select.pending_done`, so `handle_confirm_done`'s
`match self.input.mode` fallback was reachable. The first implementation therefore
introduced the helper itself.

#3758 landed mid-task and brought its own `prompt_move_to_done` (plus the
`mem::take` in `handle_confirm_done` and a stale-state guard for
`Done | Archived`). The rebase resolution keeps #3758's version of all of that and
narrows this task to what it actually owns:

1. the unit variant, and
2. `prompt_move_to_done`'s `ids.first()` guard → `is_empty()`.

## Behaviour

No user-visible change. Pure internal refactor:

- Invariant: `input.mode == InputMode::ConfirmDone` implies `select.pending_done`
  is non-empty, and it holds *every* task awaiting confirmation (1 for the single
  path, N for a batch). Established at the single point that enters the mode.
- `handle_confirm_done` reads only `pending_done`.

No Allium spec change: `docs/specs/tasks.allium`'s `ConfirmDone` rule already
describes the prompt as shared between single-task forward moves and
`BatchMoveForward`, and the pending-set is an implementation detail below the
spec's altitude.

## Steps (TDD)

### Step 1 — failing test for the invariant

Added to `src/tui/tests/navigation.rs`, next to
`move_forward_to_done_enters_confirm_mode`:

```rust
#[test]
fn single_move_to_done_records_the_task_in_pending_done() {
    let mut app = App::new(vec![make_task(5, TaskStatus::Review)]);

    app.update(Message::Task(crate::tui::messages::TaskMessage::Move {
        id: TaskId(5),
        direction: MoveDirection::Forward,
    }));

    assert_eq!(app.select.pending_done, vec![TaskId(5)]);
}
```

Red against the pre-#3758 tree (`left: []`, `right: [TaskId(5)]`); green under
#3758's `prompt_move_to_done`, where it now serves as a regression guard for the
invariant the unit variant depends on.

### Step 2 — production change

1. `src/tui/types.rs` — `ConfirmDone(TaskId)` → `ConfirmDone`, with a doc comment
   pointing at `select.pending_done`.
2. `src/tui/update/lifecycle.rs` — `prompt_move_to_done` guards with
   `ids.is_empty()` instead of destructuring `ids.first()`, and sets the
   payload-free mode. The single-vs-batch status message is chosen by matching
   `ids.as_slice()` (`[single]` vs `_`), which avoids both the discarded `first`
   binding and any indexing.
3. Match sites drop the binding: `src/tui/input.rs`,
   `src/tui/ui/kanban/status_bar.rs`.

### Step 3 — test fixture updates

- Tests that need the confirm state call `app.prompt_move_to_done(vec![id])`
  rather than hand-setting `input.mode`, so they cannot construct the
  now-impossible "mode set, pending_done empty" state:
  `src/tui/tests/wrap_up.rs`, `src/tui/tests/split_pane.rs`.
- Pattern-match assertions become `assert_eq!(…, InputMode::ConfirmDone)`. Where
  an assertion previously pinned the id (`ConfirmDone(TaskId(5))`), the id
  assertion moves to `select.pending_done` so coverage is not lost:
  `src/tui/tests/navigation.rs`, `archive.rs`, `input_handlers.rs`,
  `rendering.rs`.
- The status-bar render test keeps a direct `input.mode` assignment — the mode
  alone is the input under test there.

### Step 4 — verify

```
cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh
```

Plus `cargo clippy --all-targets -- -D warnings` (pre-push gate). All green:
3644 tests passed.
