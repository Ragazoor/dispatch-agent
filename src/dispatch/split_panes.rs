//! tmux composition for the board's split-pane feature: pulling an agent's
//! window into the board window as a pane ("pin"), and swapping a different
//! task's window into an already-pinned pane.
//!
//! These live here rather than in `src/runtime/split.rs` because they are domain
//! logic — multi-step sequences carrying policy, not raw tmux verbs — while the
//! runtime's job is `spawn_blocking` and message emission (the Message→Command
//! split in docs/architecture.md). And here rather than in `src/tmux.rs` because
//! the swap path calls `resync_agent_tree_pane`; putting them in `tmux.rs` would
//! invert that dependency.
//!
//! Being reachable from tests/tmux_lifecycle.rs falls out of the same placement,
//! and matters because the correctness of these sequences is entirely about
//! tmux's own pane semantics, which the mock layer cannot observe.
//!
//! See docs/specs/split-pane.allium and docs/specs/agent-tree.allium's
//! `ToggleVsSplitPaneInteraction`.

use anyhow::{Context, Result};

use crate::process::ProcessRunner;
use crate::tmux;

/// Move `window`'s agent pane into `target_pane`'s window as a right-hand pane,
/// discarding the agent-tree companion pane left behind.
///
/// Returns the joined pane's id (tmux preserves pane ids across a move).
pub fn join_task_window_into_pane(
    window: &str,
    target_pane: &str,
    runner: &dyn ProcessRunner,
) -> Result<String> {
    // Capture any companion pane *before* the join. `join_pane` moves only the
    // agent's own (active) pane out of the window, so a companion left behind
    // becomes the window's sole remaining pane — at which point it is
    // indistinguishable from "hidden" to the agent-tree toggle (see
    // docs/specs/agent-tree.allium: ToggleVsSplitPaneInteraction). Checking
    // after the join would be too late to tell the two apart.
    //
    // Best-effort: a failed check degrades to "no companion" rather than
    // failing the pin, which is the user's actual intent.
    let companion_pane_id = match tmux::inactive_pane_id(window, runner) {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!(
                %window,
                error = %e,
                "failed to check for companion pane before join-pane"
            );
            None
        }
    };

    let pane_id = tmux::join_pane(window, target_pane, runner)?;

    if let Some(companion_id) = companion_pane_id {
        if let Err(e) = tmux::kill_pane(&companion_id, runner) {
            tracing::warn!(
                %window,
                error = %e,
                "failed to kill leftover companion pane after join-pane"
            );
        }
    }

    Ok(pane_id)
}

/// Swap `new_window`'s agent pane into `right_pane` (the currently pinned pane),
/// then dispose of the standalone window now holding the previous occupant:
/// renamed back to `old_window` when the outgoing pane belonged to a task,
/// killed outright when it did not.
///
/// Returns the pane id of the task swapped in.
pub fn swap_task_window_into_pane(
    new_window: &str,
    right_pane: &str,
    old_window: Option<&str>,
    runner: &dyn ProcessRunner,
) -> Result<String> {
    // 1. Resolve the incoming task's pane before swapping.
    let new_pane_id = tmux::pane_id_for_window(new_window, runner).context("cannot get pane ID")?;

    // 2. Atomically swap pane contents, addressing the source by pane id.
    //
    // Not `<window>.0`: that form is an *index*, and no pane has index 0 when the
    // user sets `pane-base-index 1` — tmux fails with "can't find pane: 0" and
    // the swap silently does nothing. A `-b` split also renumbers indices, so the
    // index of "the window's own pane" is not stable even at the default base
    // index. The pane id resolved above is exactly the pane meant here and is
    // immune to both. See learning #324 and tests/tmux_lifecycle.rs's
    // `swap_works_when_pane_base_index_is_1`.
    tmux::swap_pane(&new_pane_id, right_pane, runner).context("swap pane failed")?;

    // 3. Rename or kill the standalone window that now holds the old content.
    match old_window {
        Some(old_name) => {
            tmux::rename_window(new_window, old_name, runner).context("rename window failed")?;
            // swap-pane exchanged only the agent panes — the renamed window's
            // companion pane (if any) still renders the previous occupant's
            // tree. Resync it to the task the window's new name implies.
            super::resync_agent_tree_pane(old_name, runner);
        }
        None => {
            tmux::kill_window(new_window, runner).context("kill window failed")?;
        }
    }

    Ok(new_pane_id)
}
