//! Retry, kill-and-retry, archive task handlers.

use crate::models::{DispatchMode, SubStatus, TaskId, TaskStatus};

use super::super::types::*;
use super::super::App;

impl App {
    pub(in crate::tui) fn handle_kill_and_retry(&mut self, id: TaskId) -> Vec<Command> {
        self.input.mode = InputMode::ConfirmRetry(id);
        let task = self.find_task(id);

        // An unprovisioned task has no worktree to resume into, so [r] would
        // dead-end on "Cannot resume: task has no worktree". Offer only the
        // fresh start, and name the state it is actually in — it is neither
        // stale nor crashed. See RetryReachableInPlace in
        // docs/specs/dispatch.allium.
        if task.is_some_and(|t| t.is_unprovisioned()) {
            self.set_status("Agent never started - [f] Fresh start  [Esc] Cancel".to_string());
            return vec![];
        }

        let state = if task.is_some_and(|t| t.sub_status == SubStatus::Crashed) {
            "crashed"
        } else {
            "stale"
        };
        self.set_status(format!(
            "Agent {state} - [r] Resume  [f] Fresh start  [Esc] Cancel"
        ));
        vec![]
    }

    pub(in crate::tui) fn handle_retry_resume(&mut self, id: TaskId) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.clear_status();
        self.clear_agent_tracking(id);

        if let Some(task) = self.find_task_mut(id) {
            if task.status != TaskStatus::Running {
                return vec![];
            }
            if task.worktree.is_none() {
                self.set_status("Cannot resume: task has no worktree".to_string());
                return vec![];
            }
            task.sub_status = SubStatus::Active;
            // Seed in-memory so a tick that fires between this handler and the
            // arriving Resumed message does not reclassify the task back to
            // Stale from an old timestamp. The DB row is updated by the
            // subsequent Resumed → handle_resumed path.
            task.last_pre_tool_use_at = Some(chrono::Utc::now());
            let old_window = task.tmux_window.take();
            let task_clone = task.clone();

            let mut cmds = Vec::new();
            if let Some(window) = old_window {
                cmds.push(Command::Task(
                    crate::tui::commands::TaskCommand::KillTmuxWindow { window },
                ));
            }
            cmds.push(Command::Task(crate::tui::commands::TaskCommand::Resume {
                task: task_clone,
            }));
            cmds.extend(self.maybe_respawn_split_pane(id));
            cmds
        } else {
            vec![]
        }
    }

    pub(in crate::tui) fn handle_retry_fresh(&mut self, id: TaskId) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.clear_status();
        self.clear_agent_tracking(id);

        if let Some(task) = self.find_task_mut(id) {
            if task.status != TaskStatus::Running {
                return vec![];
            }
            let cleanup = Self::take_cleanup(task);
            task.status = TaskStatus::Backlog;
            task.sub_status = SubStatus::None;
            let task_clone = task.clone();
            self.sync_board_selection();

            let mut cmds = Vec::new();
            if let Some(c) = cleanup {
                cmds.push(c);
            }
            cmds.push(Command::Task(crate::tui::commands::TaskCommand::Persist(
                task_clone.clone(),
            )));
            self.mark_dispatching(id);
            cmds.push(Command::Task(
                crate::tui::commands::TaskCommand::DispatchAgent {
                    task: task_clone,
                    mode: DispatchMode::Dispatch,
                },
            ));
            cmds
        } else {
            vec![]
        }
    }

    pub(in crate::tui) fn handle_archive_task(&mut self, id: TaskId) -> Vec<Command> {
        if let Some(task) = self.find_task_mut(id) {
            if task.status == TaskStatus::Archived {
                return vec![];
            }
            let cleanup = Self::take_cleanup(task);
            task.status = TaskStatus::Archived;
            task.sub_status = SubStatus::default_for(TaskStatus::Archived);
            let task_clone = task.clone();
            self.clear_agent_tracking(id);
            self.sync_board_selection();

            let mut cmds = Vec::new();
            if let Some(c) = cleanup {
                cmds.push(c);
            }
            cmds.push(Command::Task(crate::tui::commands::TaskCommand::Persist(
                task_clone,
            )));
            cmds.extend(self.maybe_respawn_split_pane(id));
            cmds
        } else {
            vec![]
        }
    }
}
