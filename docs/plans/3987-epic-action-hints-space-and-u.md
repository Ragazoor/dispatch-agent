# 3987 — `epic_action_hints`: wrong `Space` label, inert `[U]`

## Problem

`epic_action_hints` (`src/tui/ui/kanban/mod.rs::epic_action_hints`) builds the Normal-mode
footer for a **selected epic card** (reached from `normal_status_line` in
`src/tui/ui/kanban/status_bar.rs`). Two hints are wrong:

1. `[Space] board` — `handle_key_activate` (`src/tui/input.rs`) on a `ColumnItem::Epic`
   dispatches `EpicMessage::Enter`, i.e. it *enters* the epic. `docs/specs/split-pane.allium`
   ("Space on an epic row (board view) is unchanged: it enters the epic") and
   `docs/reference.md` ("`Space` | Enter epic view") both already say so; only the footer
   still carries the pre-unification `board` label.
2. `[U] auto dispatch` — the `U` arm (`src/tui/input/normal.rs`) requires
   `current_epic_id()`, which is `Some` only inside `ViewMode::Epic`. On a board epic card
   the key does nothing. `docs/specs/epics.allium::ToggleAutoDispatch` guidance is explicit:
   "Triggered by pressing U in the epic detail view … The key is a no-op outside that view."
   Inside an epic view a selected *sub-epic* card would also render this footer, and there
   `U` targets the enclosing epic rather than the card — so the hint is wrong in both
   positions.

Nothing here is a domain-behaviour change: the specs are already right and the code
behaviour is unchanged. Only footer copy plus tests move. No `allium:tend` pass needed.

## Fix

In `epic_action_hints`:

- `push_hint("Space", "board")` → `push_hint("Space", "enter")`.
- Drop `push_hint("U", "auto dispatch")`. Discoverability of `U` is preserved where the key
  actually works: the epic-view header badge (`src/tui/ui/shared.rs`, `auto dispatch [U]`)
  and the help overlay's `[U] auto-dispatch` row.

## TDD steps

1. **Test first**, in `src/tui/tests/epics.rs` next to the existing `epic_action_hints_*`
   unit tests:
   - `epic_action_hints_labels_space_as_enter` — the joined hint text contains
     `[Space] enter` and does not contain `board`.
   - `epic_action_hints_omits_auto_dispatch` — hint keys do not contain `[U]`, and the text
     does not contain `auto dispatch`.
2. **Footer-render test** for the selected-epic-card case (rendered footer, not just the
   helper): build an `App` with one epic on the board, select the epic card, render to a
   buffer, assert the status row contains `[Space] enter` and not `[U] auto dispatch`.
3. Implement the two label changes.
4. Accept snapshot churn — three footer snapshots carry the old text
   (`snapshot_top_indicators_in_board_mode`, `flat_view_backlog_shows_epic_card`,
   `snapshot_board_search_narrows_epic_cards`): `INSTA_UPDATE=always cargo test
   tui::tests::snapshots`, then `rm src/tui/tests/snapshots/*.snap.new`.
5. Verify: `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`.

## Out of scope

- Making `U` work on a board epic card (would be a behaviour change, and epic #273 is about
  pruning unused bindings, not adding reach).
- The sub-epic-card / ancestor-epic targeting of `U` inside an epic view.
