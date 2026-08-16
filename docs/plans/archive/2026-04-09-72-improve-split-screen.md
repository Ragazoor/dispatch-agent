# Plan: Improve Split Screen

## Context

When pressing `[S]` to enter split mode, the TUI always opens an empty right pane regardless of what's selected. The user must then press `[g]` to swap a task's tmux window into it. This adds an unnecessary step when the selected card already has a running agent.

Additionally, swapping panes with `[g]` causes visible flickering because the current implementation does `break_pane_to_window` → `join_pane` — between these two tmux commands, the TUI pane momentarily expands to full width (triggering a resize event and re-render) before shrinking back.

## Changes

### 1. [S] immediately shows selected task's tmux window

When pressing `[S]` to enter split mode, if the selected task has a `tmux_window`, use `join_pane` directly to show it in the right pane. If there's no selected task or no tmux window, fall back to the current behavior (empty split pane).

**Files:**
- `src/tui/types.rs` — Add `Command::EnterSplitModeWithTask { task_id: TaskId, window: String }`
- `src/tui/mod.rs` — Modify `handle_toggle_split_mode()` to check `selected_task()` for `tmux_window`; add handler for new command message result
- `src/runtime.rs` — Add `exec_enter_split_mode_with_task()` that calls `tmux::join_pane` directly, then emits `SplitPaneOpened { pane_id, task_id: Some(id) }`
- `src/tui/tests.rs` — New tests (see below)

**Logic in `handle_toggle_split_mode()` (entering):**
```
if selected_task has tmux_window:
    return Command::EnterSplitModeWithTask { task_id, window }
else:
    return Command::EnterSplitMode  // current behavior
```

### 2. Flicker-free pane swap using `tmux swap-pane`

Replace the `break_pane_to_window` → `join_pane` sequence with `tmux swap-pane`, which atomically swaps the contents of two panes without changing the layout. The TUI pane never resizes.

**Algorithm:**
1. Get new task's pane ID: `tmux display-message -p -t <new_window> "#{pane_id}"`
2. Swap contents: `tmux swap-pane -d -s <new_window>.0 -t <right_pane_id>`
3. If old pane had a task: rename the standalone window (now holding old content) to old task's window name via `tmux rename-window -t <window_containing_old> <old_window_name>`
4. If old pane had no task: kill the standalone window (now holding the empty shell) via `tmux kill-window`
5. Emit `SplitPaneOpened { pane_id: new_pane_id, task_id: Some(task_id) }`

**Files:**
- `src/tmux.rs` — Add `swap_pane(source: &str, target: &str)`, `rename_window(target: &str, new_name: &str)`, `kill_window_by_id(target: &str)` functions
- `src/runtime.rs` — Rewrite `exec_swap_split_pane()` to use `swap_pane` instead of `break_pane_to_window` + `join_pane`
- `src/tui/types.rs` — Update `Command::SwapSplitPane` fields: remove `old_pane_id` (available from `SplitState`), keep `old_window` for rename
- `src/tui/mod.rs` — Adjust `handle_swap_split_pane()` to pass updated fields

## Step-by-step (TDD)

### Step 1: Tests for [S] with selected task

Add tests in `src/tui/tests.rs`:

1. `toggle_split_with_selected_tmux_task_emits_enter_with_task` — selected task has tmux_window → `Command::EnterSplitModeWithTask { task_id, window }`
2. `toggle_split_without_tmux_task_emits_plain_enter` — selected task has no tmux_window → `Command::EnterSplitMode`
3. `toggle_split_no_selection_emits_plain_enter` — no task selected → `Command::EnterSplitMode`

### Step 2: Implement [S] with selected task

1. Add `Command::EnterSplitModeWithTask { task_id: TaskId, window: String }` to `src/tui/types.rs`
2. Update `handle_toggle_split_mode()` in `src/tui/mod.rs`
3. Add `exec_enter_split_mode_with_task()` in `src/runtime.rs`
4. Wire command in `execute_commands()` match arm in `src/runtime.rs`

### Step 3: Tests for swap-pane

Update existing test `g_in_split_mode_emits_swap_command` to match updated `Command::SwapSplitPane` fields.

Add MockProcessRunner tests or integration checks for the tmux command sequence if feasible.

### Step 4: Implement flicker-free swap

1. Add `swap_pane()`, `rename_window()` to `src/tmux.rs`
2. Rewrite `exec_swap_split_pane()` in `src/runtime.rs`
3. Update `Command::SwapSplitPane` in `src/tui/types.rs` and `handle_swap_split_pane()` in `src/tui/mod.rs`

### Step 5: Update existing test for toggle_split_mode_emits_enter_command

The existing test `toggle_split_mode_emits_enter_command` uses `make_app()` which has no tasks — verify it still passes (should emit `Command::EnterSplitMode` since no selected task has a tmux window).

## Verification

1. `cargo test` — all existing and new tests pass
2. `cargo clippy -- -D warnings` — no warnings
3. Manual testing:
   - Start TUI with running agents
   - Select a running task, press `[S]` → agent should appear immediately in right pane
   - Press `[g]` on another running task → swap should be instant with no flicker
   - Select a todo task, press `[S]` → empty split pane (fallback behavior)
   - Press `[S]` again to exit → pane restored/killed correctly
