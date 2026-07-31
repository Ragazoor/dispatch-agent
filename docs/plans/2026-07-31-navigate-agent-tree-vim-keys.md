# Vim navigation in the agent-tree companion pane (task #3791)

## Goal

`dispatch agent-tree <task_id>` (the companion pane renderer, `src/cli/agent_tree.rs`)
currently only accepts arrow keys for cursor movement and expand/collapse. Add the
vim motions `h`/`j`/`k`/`l` as equivalents.

## Decision: arrows stay

The task says "No need for the arrow-keys." Read as *"don't bother adding them"* —
they already exist. Removing them would be a regression for no gain and nothing in
the task asks for it, so `h/j/k/l` land **alongside** `Left/Down/Up/Right`.

| Key | Action |
|---|---|
| `j` / `Down` | move cursor down |
| `k` / `Up` | move cursor up |
| `h` / `Left` | collapse selected node (or move to parent) |
| `l` / `Right` | expand selected node |

`q` / `Ctrl-C` (exit) and `Space` / `Enter` (toggle) are untouched. No collision:
`h`, `j`, `k`, `l` are unbound in the renderer today.

## Testability seam

Key handling is inline in `run_loop`, which owns a `Terminal` and blocks on
`event::poll` — untestable. Extract a pure handler:

```rust
/// What the event loop should do after a key.
enum KeyAction { Continue, Exit }

fn handle_key(state: &mut RenderState, key: KeyEvent) -> KeyAction
```

`run_loop` calls it and returns `Ok(())` on `Exit`. Cursor movement
(`TreeState::key_up`/`key_down`) resolves against the identifiers captured by the
*last render*, so tests must render into a `TestBackend` once before sending keys.
A small `harness(jsonl) -> (TreeNode, RenderState, Terminal<TestBackend>)`-style
helper in the existing test module covers that.

## Steps (TDD — test first in each step)

1. **Spec first.** `docs/specs/agent-tree.allium`: the `AgentTreeCompanionPane`
   surface's `provides:` block annotates each action with its key
   (`MoveAgentTreeCursorUp(user, pane) -- Up`). Extend those four comments to name
   the vim key too (`-- k, Up`, `-- j, Down`, `-- h, Left`, `-- l, Right`). Comment-only
   change: the actions and rules are unchanged, since the vim keys are aliases that
   trigger exactly the same view-only actions. Run `allium check`.
2. **Extract the seam.** Add `KeyAction` + `handle_key` with the *existing* arrow/q/
   space behaviour moved verbatim; rewire `run_loop` to call it. Add tests asserting
   the current arrow/`q`/`Ctrl-C`/`Space` behaviour through the new function — these
   pass immediately and lock the refactor in as behaviour-preserving.
3. **Vim keys (red → green).** Add tests: `j` selects the next node, `k` the previous,
   `l` opens the selected directory, `h` closes it, and each pairs with its arrow
   equivalent. Then add the `KeyCode::Char('j' | 'k' | 'h' | 'l')` arms.
4. **Docs.** `docs/reference.md`: the key table documents `Prefix+e` (toggling the
   pane) but not the keys *inside* it. Add a short row/line covering
   `h/j/k/l` + arrows, `Space`/`Enter`, `q`. Update the `src/cli/agent_tree.rs` row in
   `docs/module-map.md` if the extraction changes its description materially.
5. **Verify.** `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`,
   plus `cargo clippy --all-targets -- -D warnings` (pre-push gate). Note learning
   #117: `TreeState` navigation methods return `bool` — discard with `{ }` blocks.

## Out of scope

`g`/`G`, `Ctrl-D`/`Ctrl-U`, search — the task names `h/j/k/l` only.
