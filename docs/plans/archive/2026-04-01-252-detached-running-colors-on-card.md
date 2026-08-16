# Plan: Better Colors for Running/Detached on Card

## Context

Running tasks currently show `◉ running` in MUTED gray (rgb 86,95,137) regardless of whether
they have an active tmux session or are detached (worktree exists, tmux window gone). This makes
active agents feel invisible and gives no visual distinction for detached tasks that need resuming.

The fix adds a proper "detached" branch and brightens the active-running color.

## Target File

`src/tui/ui.rs` — specifically `build_task_list_item()` lines 379–386.

## Changes

### 1. Add a detached branch (before the generic running branch)

A running task is **detached** when `task.tmux_window.is_none()` (no active session) and
`task.worktree.is_some()` (worktree still exists). Insert a new branch into the if-else chain:

```rust
} else if status == TaskStatus::Running
    && task.tmux_window.is_none()
    && task.worktree.is_some()
{
    Line::from(vec![
        Span::raw("   "),
        Span::styled(
            "◌ detached",
            Style::default().fg(MUTED_LIGHT),
        ),
    ])
} else if status == TaskStatus::Running {
    Line::from(vec![
        Span::raw("   "),
        Span::styled(
            format!("{} running", status_icon(status)),
            Style::default().fg(CYAN),    // was MUTED
        ),
    ])
```

`MUTED_LIGHT` (rgb 120,124,153) reads as "paused/inactive" — lighter than MUTED but clearly
not alive. `CYAN` (rgb 86,182,194) feels active and alive, matching the lively column YELLOW
accent without competing with it.

### 2. No new constants needed

Both `MUTED_LIGHT` and `CYAN` already exist in the palette at lines 16 and 22.

## Verification

- `cargo clippy` — no warnings
- `cargo test` — all tests pass (no test changes expected; this is purely visual)
- Manual: launch TUI, observe a Running task with active tmux shows `◉ running` in cyan; a
  detached task (close its tmux window) shows `◌ detached` in muted-light.
