# agent-tree: guard expand/toggle on directories (#3834)

## Problem

`handle_key` in `src/cli/agent_tree.rs` maps `Space`/`Enter` to
`TreeState::toggle_selected()` and `l`/`Right` to `TreeState::key_right()`.
Neither `tui_tree_widget` method has a leaf guard — `open()` only rejects an
empty identifier — so pressing either while a *file* node is selected inserts
that file's identifier path into `TreeState::opened`.

Nothing renders differently (a leaf has no children to reveal), but the phantom
open is observable through `key_left`: it removes the selection from `opened`
first and only falls back to popping the last path segment when nothing was
removed. So the next `h`/`Left` on that file consumes the keypress "closing"
the phantom open instead of stepping out to the parent — step-out silently
needs two presses.

## Intended behaviour

Expand and toggle act on directories only. On a file node they are no-ops:
`opened` is unchanged, and a single `h`/`Left` still steps out to the parent.

Collapse (`h`/`Left`) keeps its current dual behaviour, which the spec already
documents: close the node if open, otherwise move the cursor to the parent.
For a file that means it always steps out — no guard needed there.

## Spec

`docs/specs/agent-tree.allium`'s `AgentTreeCompanionPane` surface already speaks
of opening/closing *directories*, and `CollapseSelectedAgentTreeNode` explicitly
covers the file case ("when the selected node is already closed (or is a file),
the cursor moves to its parent instead"). What it does not say is what
`ExpandSelectedAgentTreeNode` / `ToggleSelectedAgentTreeNode` do on a file. That
silence is why the code drifted, so the spec gets one clarifying comment on
those two actions: they are no-ops on a file node. This is code catching up to
intent, not a behaviour change to the spec's model.

Spec change first (via `allium:tend`), then tests, then code.

## Implementation

### 1. Spec (tend)

In the `provides:` block of `AgentTreeCompanionPane`, annotate the expand and
toggle actions: on a file node both are no-ops, because a file has nothing to
open — leaving the pane's expansion state untouched so a following collapse
steps out on the first press.

### 2. Tests (TDD — write these first, watch them fail)

All in the existing `mod tests` of `src/cli/agent_tree.rs`, using the existing
`KeyRig` (which draws before each press — `TreeState` resolves keys against the
last render's identifiers) and `three_node_log()` (flattened view: `a.rs`,
`src`, `src/lib.rs`, `z.rs`).

- `space_on_a_file_records_no_open` — `j` to `a.rs`, press Space, assert
  `state.tree_state.opened()` does not contain `["a.rs"]`. Repeat for `Enter`.
- `l_on_a_file_records_no_open` — same, for `l` and `Right`.
- `h_after_space_on_a_file_steps_out_to_the_parent` — `j`×3 to
  `["src", "lib.rs"]`, press Space, then a single `h`; assert the selection is
  `["src"]`. This is the user-visible regression test.
- `l_on_a_file_leaves_the_whole_open_set_untouched` — capture `opened()` before
  and after pressing `l` on `a.rs`, assert equal. Guards against a fix that
  merely special-cases one path.

Existing directory tests (`right_and_l_both_expand_the_selected_directory`,
`space_and_enter_both_toggle_the_selected_directory`,
`left_and_h_both_collapse_the_selected_directory`,
`h_on_a_child_moves_the_cursor_to_its_parent`) must stay green unchanged apart
from the call-site update in step 3.

### 3. Code

`handle_key` cannot see the tree today, so this is a signature change:

```rust
pub fn handle_key(state: &mut RenderState, root: &TreeNode, key: KeyEvent) -> KeyAction
```

Add a private lookup helper. A node's widget identifier path is exactly its
chain of name segments below the root (see `build_tree_items` and
`sync_expansion_at`), so resolving a selection against the tree is a walk:

```rust
fn node_at<'a>(root: &'a TreeNode, path: &[String]) -> Option<&'a TreeNode>
fn selected_is_directory(root: &TreeNode, selected: &[String]) -> bool
```

`selected_is_directory` returns `false` for an empty path (nothing selected —
the widget rejects an empty identifier anyway) and `false` for a path that
resolves to nothing (a stale selection after a rebuild), so the guard fails
closed: no phantom opens on anything we cannot confirm is a directory.

Then gate the two arms:

```rust
KeyCode::Char('l') | KeyCode::Right => {
    if selected_is_directory(root, state.tree_state.selected()) {
        state.tree_state.key_right();
    }
}
KeyCode::Char(' ') | KeyCode::Enter => {
    if selected_is_directory(root, state.tree_state.selected()) {
        state.tree_state.toggle_selected();
    }
}
```

`k`/`j`/`h`/arrows and `q`/`Ctrl-C` are unchanged.

Call sites:

- `run_loop` already owns `tree`; pass `&tree`. Borrow order matters — the
  `terminal.draw` closure borrows `tree` immutably and `state` mutably, and
  `handle_key` does the same after the draw returns, so no conflict.
- `KeyRig::press` passes `&self.tree`; the `ctrl_c_exits_the_renderer` test that
  builds a bare `RenderState` passes `&build_tree(&root(), "")`.

### 4. Docs

Update the `src/cli/agent_tree.rs` row in `docs/module-map.md`: `handle_key` now
takes the tree so expand/toggle can be guarded on directories.

Also update the `handle_key` doc comment to say expand/toggle are
directory-only, and note the tree parameter is what makes that possible.

## Verification

```
cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh
```

Plus `cargo clippy --all-targets -- -D warnings` (the pre-push gate) — the
discarded `bool` returns stay inside `{ }` blocks as today.
