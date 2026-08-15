//! Split pane mode handlers: toggle, swap, open/close, focus tracking.

use crate::models::TaskId;

use super::super::types::*;
use super::super::App;

impl App {
    pub(in crate::tui) fn handle_toggle_split_mode(&mut self) -> Vec<Command> {
        if self.board.split.active {
            self.exit_split_if_active()
        } else if let Some((task_id, window)) = self
            .selected_task()
            .and_then(|t| t.tmux_window.clone().map(|w| (t.id, w)))
        {
            vec![Command::Split(
                crate::tui::commands::SplitCommand::EnterWithTask { task_id, window },
            )]
        } else {
            vec![Command::Split(crate::tui::commands::SplitCommand::Enter)]
        }
    }

    /// Swap `task_id`'s tmux window into the split pane.
    ///
    /// The caller establishes the preconditions (see SwapSplitPane in
    /// docs/specs/split-pane.allium): the only producer of
    /// `SplitMessage::Swap` is `handle_key_activate`, which raises it solely
    /// for a task that has a live tmux window while split mode is active. A
    /// windowless task is routed by status there instead — it never reaches
    /// this handler — so there is no user-facing "no session" case to report.
    pub(in crate::tui) fn handle_swap_split_pane(&mut self, task_id: TaskId) -> Vec<Command> {
        // Already pinned — nothing to do. Reachable when the pane id is
        // missing, which makes the focus-the-pane branch upstream fall through.
        if self.board.split.pinned_task_id == Some(task_id) {
            return vec![];
        }

        let task = match self.find_task(task_id) {
            Some(t) => t,
            None => return vec![],
        };
        let new_window = match &task.tmux_window {
            Some(w) => w.clone(),
            None => return vec![],
        };
        let old_pane_id = self.board.split.right_pane_id.clone();
        let old_task = self
            .board
            .split
            .pinned_task_id
            .and_then(|id| self.find_task(id))
            .and_then(|t| t.tmux_window.clone().zip(t.worktree.clone()));
        vec![Command::Split(crate::tui::commands::SplitCommand::Swap {
            task_id,
            new_window,
            old_pane_id,
            old_task,
        })]
    }

    pub(in crate::tui) fn handle_split_pane_opened(
        &mut self,
        pane_id: String,
        task_id: Option<TaskId>,
    ) -> Vec<Command> {
        self.board.split.active = true;
        self.board.split.focused = true;
        self.board.split.right_pane_id = Some(pane_id);
        self.board.split.pinned_task_id = task_id;
        vec![]
    }

    pub(in crate::tui) fn handle_focus_changed(&mut self, focused: bool) -> Vec<Command> {
        if self.board.split.active {
            self.board.split.focused = focused;
        }
        vec![]
    }

    pub(in crate::tui) fn handle_split_pane_closed(&mut self) -> Vec<Command> {
        self.board.split.active = false;
        self.board.split.focused = true;
        self.board.split.right_pane_id = None;
        self.board.split.pinned_task_id = None;
        vec![]
    }

    /// If `task_id` is the split-pinned task, clear the pin and respawn the
    /// pane with a fresh shell.  Split mode stays active.
    pub(in crate::tui) fn maybe_respawn_split_pane(&mut self, task_id: TaskId) -> Vec<Command> {
        if self.board.split.active && self.board.split.pinned_task_id == Some(task_id) {
            self.board.split.pinned_task_id = None;
            if let Some(pane_id) = self.board.split.right_pane_id.clone() {
                return vec![Command::Split(
                    crate::tui::commands::SplitCommand::RespawnPane { pane_id },
                )];
            }
        }
        vec![]
    }
}
