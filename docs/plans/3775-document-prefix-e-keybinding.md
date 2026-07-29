# 3775 — Document `Prefix+e` in `docs/reference.md`

## Goal

`docs/reference.md`'s Tasks keybinding table documents the board's one tmux-global
binding (`Prefix+Space`, jump back to the TUI) but not the second one added by task
#3742 — `Prefix+e`, which toggles the agent-tree companion pane. Add a row for it.

## What `Prefix+e` actually does (verified against #3742's commit, `514bb66b` on `main`)

- Bound in `setup_tmux_for_tui` / unbound in `teardown_tmux_for_tui`
  (`src/runtime/mod.rs`), the same lifecycle as `Prefix+Space`: alive only while the
  board TUI process runs.
- tmux expands `#{window_name}` and invokes
  `dispatch toggle-agent-tree-pane <window>`; `toggle_agent_tree_pane`
  (`src/dispatch/agents.rs`) resolves the task id from the `task-<id>` window name and
  is a **no-op** for any window that doesn't match — so it only acts in agent windows.
- Toggle is kill-pane + re-split, not `resize-pane -Z`.
- Spec: `docs/specs/agent-tree.allium`, rules `BindAgentTreeToggle` /
  `UnbindAgentTreeToggle`.

## Change

One row in the Tasks table of `docs/reference.md`, immediately after the
`Prefix+Space` row, covering: what it toggles, that it applies to the agent window it
is pressed in, that it is inert in non-agent windows, and that its scope is the board
TUI's lifetime.

Documentation only — no code, no spec change (the spec already covers the behaviour),
so no new tests. Verification is the task's verify command:
`cargo test && ./scripts/check-doc-paths.sh`.
