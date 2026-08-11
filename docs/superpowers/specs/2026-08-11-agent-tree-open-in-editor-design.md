# Agent tree: open the selected file in an editor

**Task**: #3856
**Date**: 2026-08-11
**Status**: design approved, awaiting implementation plan

## Problem

The agent-tree companion pane shows which files an agent read and modified, but
it is a dead end: seeing `src/service/tasks.rs [Modified]` gives you no way to
look at what changed. Space and Enter on a file node are deliberate no-ops
(#3834 — an unguarded `open()` recorded a phantom expansion that the next `h`
silently consumed), so the two keys a file browser would use for "open this" are
free.

The user wants Space/Enter on a file to split the agent's tmux window and open
that file in their default editor.

## Decisions

Every one of these was chosen explicitly; none is a default that fell out of the
implementation.

1. **Placement**: bottom of the agent window, spanning the full window width,
   60% of its height. The tree and the agent's own pane keep the top 40%.
2. **Focus stays in the tree** (`split-window -d`). The intended flow is
   browsing: `j`/`k` to move, Enter to show the file below, Enter again on the
   next one. Typing in the editor costs one `prefix`+arrow.
3. **One editor pane per agent window.** A second Enter replaces the existing
   pane's contents rather than stacking another pane. This *does* kill a running
   editor — accepted, with the swap-file and unsaved-edit consequences that
   implies, in exchange for a layout that does not degrade as you browse.
4. **Editor resolution**: `$VISUAL`, then `$EDITOR`, then `vi`. POSIX order.
   There is no user-facing setting for the editor — the `vi` fallback is a spec
   config constant so it has a name, not a knob to turn.
5. **Failures are visible in the pane**, not just in a log — a one-line notice
   in the tree pane's bottom border.
6. **Pane identity is explicit, not positional.** In scope for this task, and
   the reason is a pre-existing bug (see below), not the new feature alone.

## The pane-identity bug this has to fix

`prefix+e` (`toggle_agent_tree_pane`, src/dispatch/agents.rs) finds the tree
pane via `tmux::inactive_pane_id`: "the window's single inactive pane". That
holds only because every split helper passes `-d`, leaving the agent's own pane
active at split time. It is not an invariant of the window — it is a property of
the moment the pane was created.

Two consequences, both live:

- **Today**: focus the tree pane and press `prefix+e`. The single inactive pane
  is now *claude*, so dispatch kills the agent's session. The spec's claim that
  "the killed pane is the companion pane, never the agent's own pane"
  (`HideAgentTreePane` guidance) does not hold once the user moves focus.
- **With this feature**: the user *must* focus the tree pane to press Enter, so
  the above stops being obscure. And a third pane makes two panes inactive, so
  `inactive_pane_id` returns `None`, which `toggle_agent_tree_pane` reads as
  "hidden" and answers by splitting a *second* tree pane.

So both panes get identified by what they are, not by where focus happens to be:

| Pane | Identified by |
|---|---|
| Tree | `#{pane_start_command}`: argv0's basename equals `ProcessRunner::agent_binaries().dispatch` **and** argv1 is `agent-tree` |
| Editor | `@dispatch_editor_pane` tmux pane option, set immediately after the split |

The tree pane's marker is free and retroactive: tmux already reports
`pane_start_command` for panes running right now (verified against tmux 3.7b),
so windows open at upgrade time are covered without a migration or a fallback
heuristic. A *substring* match on `agent-tree` would be wrong — an editor pane
opened on `docs/specs/agent-tree.allium` contains that substring — hence
matching argv0 and argv1 as separate tokens.

The editor pane cannot use the same trick: its start command is whatever
`$EDITOR` resolves to, which may change between presses and may carry
arguments. An explicit pane option costs one `set-option -p` per split and
survives `respawn-pane`, since the pane object persists.

## Mechanics

### The two tmux calls

Open (no editor pane in this window yet):

```
split-window -v -f -d -l 60% -t $TMUX_PANE -c <worktree> -- <editor argv...> <abs path>
set-option -p -t <new pane id> @dispatch_editor_pane 1
```

`-f` makes the new pane span the full window rather than subdividing the tree
pane's own column, so the geometry does not depend on which pane the split is
targeted at. `-d` keeps focus where it is. `--` plus separate argv elements
means no shell and no quoting layer, matching
`tmux::split_window_horizontal_running`.

Replace (an editor pane exists):

```
respawn-pane -k -c <worktree> -t <editor pane id> -- <editor argv...> <abs path>
```

`respawn-pane` does not move focus and preserves the pane's options, so the
`@dispatch_editor_pane` marker does not need re-setting.

### Resolving panes

Both lookups mirror `tmux::inactive_pane_id`: resolve the target through
`window_target`, then `list-panes -t <resolved target>` with the format field
the predicate needs. Never a pane index — `pane-base-index` shifts those, which
is the reasoning already recorded on `inactive_pane_id`.

The renderer never needs a window name: `$TMUX_PANE` is a `%N` pane id, which
`is_resolved_target` passes through untouched, and `list-panes -t %N` lists
every pane in *that pane's window* (verified against tmux 3.7b). `prefix+e`
still starts from a window name and resolves it the way it does today.

If `$TMUX_PANE` is unset — the renderer run outside tmux — opening is a visible
failure, not a panic.

### Where the code goes

- `src/agent_tree_editor.rs` (new, sibling of `src/agent_tree.rs`, which owns
  tree building): editor resolution from environment values, and the two tmux
  operations behind one `open_in_editor(root, rel_path, my_pane, runner)`
  entry point. Editor
  resolution takes the env values as parameters rather than reading the process
  environment, so it is testable without `set_var`.
- `src/tmux.rs`: the two pane lookups, the `-f`/`-v` split, `respawn-pane` with
  a command, and `set-option -p`. Existing helpers are left alone —
  `split_window_horizontal` (board split pane) and
  `split_window_horizontal_running` (tree pane) both encode geometry this needs
  to differ from.
- `src/cli/agent_tree.rs`: `handle_key` gains a `KeyAction::OpenInEditor(PathBuf)`
  variant carrying the selection **relative** to the root. `handle_key` stays
  pure — it does not join paths, read the environment, or touch tmux.
  `run_loop` performs the effect through an injected `&dyn ProcessRunner` and
  records the outcome.
- `src/dispatch/agents.rs`: `toggle_agent_tree_pane` and
  `resync_agent_tree_pane` switch from `inactive_pane_id` to the tree-pane
  lookup.
- `src/dispatch/split_panes.rs`: `join_task_window_into_pane` drains the
  companion pane before pinning an agent window into the board. It finds it with
  `inactive_pane_id`, so with an editor pane open the lookup is ambiguous,
  returns `None`, and *both* panes are orphaned in a window the toggle can no
  longer make sense of. It switches to draining every dispatch-created companion
  pane — tree and editor — which is a superset of what it does today.
- `src/main.rs`: `cmd_agent_tree` installs the app-log subscriber, which it does
  not do today — so every `tracing::warn!` the renderer already contains
  currently goes nowhere. It writes to a file, so it is safe under the alternate
  screen.

### Error notice

`RenderState` carries an `Option<String>` notice. It is set when opening fails
(file absent, `$TMUX_PANE` missing, tmux command failed), rendered in the tree
pane's bottom border, and cleared by the next keypress. Failure never ends the
renderer's loop.

The file's existence is checked before splitting, so "the agent deleted it after
touching it" reads as a message rather than an editor on an empty buffer.

## Spec changes (`docs/specs/agent-tree.allium`)

- `AgentTreeCompanionPane` gains `OpenSelectedAgentTreeFile(user, pane)` — Space
  and Enter, files only. `ToggleSelectedAgentTreeNode` is narrowed to
  directories: its current "on a file both are no-ops" guidance becomes wrong.
- `AgentTreePane` gains `editor_pane: core/TmuxPane?` and the error notice.
- New rules: open (split), replace (respawn), and a failure rule in the shape of
  the existing `FileEventWriteFailureIsSilent` — except this one is deliberately
  *not* silent.
- New config: `agent_tree_editor_pane_percent = 60`,
  `agent_tree_editor_fallback = "vi"`.
- New invariant `OneEditorPanePerAgentWindow`, alongside the existing
  `OneCompanionPanePerAgentWindow`.
- `HideAgentTreePane` / `ShowAgentTreePane` guidance: the active/inactive
  derivation is replaced by the start-command marker, and the resolved
  `ToggleVsSplitPaneInteraction` note needs revisiting — its conclusion was that
  the *mutation* side (`join_pane`, `exec_swap_split_pane`) was the thing to fix
  because the state-reading "was already correct". It was not.
- `@guarantee ReadOnlyObservation` is amended, not deleted: the renderer still
  never writes to the worktree, mutates a task, or issues an MCP call. It now
  launches a process, at the user's explicit keypress, that can write.

## Test plan (TDD, in this order)

**Pure, no process and no terminal**

- `handle_key` returns `OpenInEditor` with the relative selection path for both
  Space and Enter on a file; still `Continue` (and toggles) on a directory;
  `Continue` on an empty or stale selection.
- Editor resolution: `$VISUAL` wins over `$EDITOR`; `$EDITOR` alone; neither set
  → `vi`; an env var set to the empty string counts as unset; a multi-word value
  splits into argv.

**`MockProcessRunner` — argv shape only**

- Split argv exactly, including `-f`, `-d`, `-l 60%`, `-c <worktree>` and the
  `--` boundary, with the editor's arguments as separate elements.
- `set-option -p @dispatch_editor_pane` follows the split, against the pane id
  the split printed.
- Second open issues `respawn-pane -k` against the marked pane and no
  `split-window`.
- Tree-pane lookup picks the `dispatch agent-tree <id>` pane and is *not* fooled
  by an editor pane whose start command is `vim docs/specs/agent-tree.allium`.
- A failing split leaves the renderer's loop intact and populates the notice.

**Real tmux — `tests/tmux_editor_pane.rs`, new target**

The mock cannot see any of these (learning #327: a mock pins the command string,
not tmux's behaviour):

- Two consecutive opens leave the window at three panes, not four.
- Focus is still on the tree pane after an open.
- The editor pane's cwd is the worktree, not the parent repo.
- `prefix+e`'s target resolution kills the *tree* pane while the tree pane is
  the active one — the kill-claude regression, which is the one assertion that
  would have caught the pre-existing bug.
- `prefix+e` with an editor pane present kills the tree pane rather than
  spawning a second one.
- Pinning an agent window that has an editor pane open leaves no orphaned pane
  behind.

**Snapshot**

- The error notice in the bottom border, at the existing 50×12 renderer test
  size (not the 120×40 board size — this is the companion pane's own harness).

## Out of scope

- Any editor integration beyond argv: no jump-to-line, no diff view, no reuse of
  an already-running editor's server (`nvim --remote`).
- Making the *board* TUI able to open files. This is the companion pane only.
- The `AgentTreeViewState` open question (whether per-process cursor/expansion
  state belongs in the spec at all). The error notice adds to that pile; the
  question stays open.
- `EventLogRetention` and `OutOfWorktreeTouches`, the spec's other open
  questions.
