# 3811 — Agent tree: expand every folder by default

## Problem

In the agent-tree companion pane, only top-level directories open on their own.
Every nested directory renders collapsed (`▶`), so reaching a badge means
manually opening each level.

The spec already promises the opposite. `docs/specs/agent-tree.allium`'s
`RefreshAgentTree` rule, EXPANSION clause:

> Every directory node is, by construction, an ancestor of a touched path, so
> every directory that appears is expanded — there is no untouched, collapsed
> directory to reveal.

So this is a **bug against the existing spec**, not a new feature. Scope is
unchanged: the tree still shows only what the agent read or modified — no
filesystem traversal (the deferred `worktree_tree` work stays deferred).

## Root cause

`tui_tree_widget` identifies an open node by the chain of each ancestor item's
**own** `identifier` value, root-first (`flatten.rs:36` in tui-tree-widget
0.23.1: `child_identifier = current + [item.identifier]`).

`node_to_item` (`src/cli/agent_tree.rs`) gives each item a slash-joined
cumulative identifier: `"a"`, `"a/b"`, `"a/b/c.rs"`. The widget therefore
expects the open-set entry for `a/b` to be `["a", "a/b"]`.

`RenderState::sync_expansion_at` instead builds a vector of bare path
*segments* — `["a", "b"]` — and calls `tree_state.open()` with it. `open()`
inserts blindly and returns success, so nothing errors; the entry simply never
matches a real node. Depth-1 directories work only because for them the two
representations coincide (`["a"]`).

The existing unit test `sync_expansion_opens_nested_ancestor_directories`
asserts against the same wrong representation, so it passes while the pane is
visibly broken. The snapshot
`…__snapshot_nested_directories_auto_expanded.snap` has the defect locked in:
it renders `▼ a` / `▶ b`.

## Approach

Two fixes were available:

1. Teach `sync_expansion_at` to build the slash-joined identifier chain the
   widget expects (`["a", "a/b"]`).
2. Stop slash-joining. Identify each node by its own name segment, which is
   all the widget requires — `TreeItem`'s docs say identifiers "need to be
   unique among siblings" and its own example key is `vec!["src", "main.rs"]`.

(1) was implemented first, then replaced by (2) after review. The join
re-encodes ancestry the widget already tracks (`flatten` scopes every lookup by
the ancestor chain), so it buys nothing and creates a second representation
that the expansion side must mirror by hand. Under (2) a node's widget key and
its path segments are the same vector and the mismatch stops being expressible
— `sync_expansion_at` reverts to its original segment walk, which was correct
all along against segment identifiers.

Sibling-uniqueness holds by construction: `build_tree` dedups directories by
name and files by full path, so `TreeItem::new`'s duplicate check fires in
exactly the same cases as before. Nothing outside `src/cli/agent_tree.rs`
consumes these identifiers. The duplicate-identifier warn log keeps its full
path by logging the accumulator as a separate `path` field.

The "auto-expand exactly once, so a manual collapse survives" behaviour is
unchanged throughout.

## Steps (TDD)

1. **Spec** — add a note to `RefreshAgentTree`'s guidance recording the test
   obligation this bug exposed: auto-expansion must be covered by *rendering*,
   because asking the widget to open a directory succeeds even when the key
   identifies no node, so an open-set assertion can encode the wrong key and
   pass. The behavioural clause (EXPANSION) already says the right thing and
   does not change. Deliberately says nothing about the key's representation —
   that is a vendored-library detail, and it belongs next to the code.
2. **Failing test (render level)** — render a deeply nested touched path and
   assert the leaf is visible with no collapsed directory anywhere. This is
   the test that would have caught the bug, because it goes through `flatten`.
   Assert on the leaf name alone: the pane title is a real source of false
   positives for short segment names.
3. **Failing test (unit level)** — `sync_expansion_opens_nested_ancestor_directories`
   and a new nested manual-collapse case, both keyed on segment vectors.
4. **Fix** — identify nodes by name segment in `node_to_item`; keep
   `sync_expansion_at`'s segment walk.
5. **Re-accept** `…__snapshot_nested_directories_auto_expanded.snap` (it now
   shows `▼ a` / `▼ b` / `c.rs [Modified]`), and delete any `*.snap.new`.
6. **Verify** — `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`.

## Out of scope

- Any filesystem scan of the worktree (untouched files stay invisible; the
  `TreeScanExclusions` open question stays open).
- The `AgentTreeViewState` open question — cursor/expansion state remains
  unmodelled per-process state.
