//! Task wrap-up and finish handlers (rebase + cleanup, PR creation flow).

use crate::dispatch;
use crate::models::{SubStatus, TaskId, TaskStatus};

use super::super::types::*;
use super::super::App;

impl App {
    pub(in crate::tui) fn handle_finish_complete(&mut self, id: TaskId) -> Vec<Command> {
        let in_queue = self
            .merge_queue
            .as_ref()
            .is_some_and(|q| q.current == Some(id));

        let mut cmds = if let Some(task) = self.find_task_mut(id) {
            task.tmux_window = None;
            task.status = TaskStatus::Done;
            task.sub_status = SubStatus::None;
            let task_clone = task.clone();
            self.clear_agent_tracking(id);
            self.sync_board_selection();
            if !in_queue {
                self.set_status(format!("Task {} finished", id));
            }
            vec![Command::Task(crate::tui::commands::TaskCommand::Persist(
                task_clone,
            ))]
        } else {
            vec![]
        };

        cmds.extend(self.maybe_respawn_split_pane(id));

        if in_queue {
            if let Some(q) = &mut self.merge_queue {
                q.completed += 1;
                q.current = None;
            }
            cmds.extend(self.advance_merge_queue());
        }

        cmds
    }

    pub(in crate::tui) fn handle_finish_failed(
        &mut self,
        id: TaskId,
        error: String,
        is_conflict: bool,
    ) -> Vec<Command> {
        let mut cmds = Vec::new();

        if is_conflict {
            if let Some(task) = self.find_task_mut(id) {
                task.sub_status = SubStatus::Conflict;
            }
            cmds.push(Command::Task(
                crate::tui::commands::TaskCommand::PatchSubStatus {
                    id,
                    sub_status: SubStatus::Conflict,
                },
            ));
        }

        if let Some(q) = &mut self.merge_queue {
            if q.current == Some(id) {
                q.current = None;
                q.failed = Some(id);
                let completed = q.completed;
                let total = q.task_ids.len();
                self.set_status(format!(
                    "Epic merge paused ({completed}/{total}): #{id} \u{2014} {error}"
                ));
                return cmds;
            }
        }

        self.set_status(error);
        cmds
    }

    pub(in crate::tui) fn handle_start_wrap_up(&mut self, id: TaskId) -> Vec<Command> {
        let branch = match self.find_task(id) {
            Some(t) if dispatch::is_wrappable(t) => {
                match t
                    .worktree
                    .as_deref()
                    .and_then(dispatch::branch_from_worktree)
                {
                    Some(b) => b,
                    None => return vec![],
                }
            }
            _ => return vec![],
        };

        self.input.mode = InputMode::ConfirmWrapUp(id);
        self.set_status(format!(
            "Wrap up {}: [r] rebase onto main  [d] done (no git)  [Esc] cancel",
            branch
        ));
        vec![]
    }

    pub(in crate::tui) fn handle_wrap_up_rebase(&mut self) -> Vec<Command> {
        let id = match self.input.mode {
            InputMode::ConfirmWrapUp(id) => id,
            _ => return vec![],
        };
        self.input.mode = InputMode::Normal;
        self.set_status("Rebasing...".to_string());
        // Optimistically clear conflict substatus — FinishComplete will persist it.
        if let Some(task) = self.find_task_mut(id) {
            if task.sub_status == SubStatus::Conflict {
                task.sub_status = SubStatus::default_for(task.status);
            }
        }

        if let Some(task) = self.find_task(id) {
            let worktree = match &task.worktree {
                Some(wt) => wt.clone(),
                None => return vec![],
            };
            let branch = match dispatch::branch_from_worktree(&worktree) {
                Some(b) => b,
                None => return vec![],
            };
            vec![Command::Task(crate::tui::commands::TaskCommand::Finish {
                id,
                repo_path: task.repo_path.clone(),
                branch,
                base_branch: task.base_branch.clone(),
                worktree,
                tmux_window: task.tmux_window.clone(),
            })]
        } else {
            vec![]
        }
    }

    pub(in crate::tui) fn handle_wrap_up_done(&mut self) -> Vec<Command> {
        let id = match self.input.mode {
            InputMode::ConfirmWrapUp(id) => id,
            _ => return vec![],
        };
        self.input.mode = InputMode::Normal;
        if let Some(task) = self.find_task_mut(id) {
            let detach = Self::take_detach(task);
            task.status = TaskStatus::Done;
            task.sub_status = SubStatus::default_for(TaskStatus::Done);
            let task_clone = task.clone();
            self.clear_agent_tracking(id);
            self.sync_board_selection();
            self.set_status(format!("Task {} marked done", id));
            let mut cmds = Vec::new();
            if let Some(c) = detach {
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

    pub(in crate::tui) fn handle_cancel_wrap_up(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.clear_status();
        vec![]
    }
}
