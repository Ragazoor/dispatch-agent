//! PR-related message handlers: review state polling.

use crate::models::{ReviewDecision, SubStatus, TaskId, TaskStatus};

use super::super::types::*;
use super::super::App;

impl App {
    pub(in crate::tui) fn handle_pr_merged(&mut self, id: TaskId) -> Vec<Command> {
        self.handle_pr_terminal(id, "merged")
    }

    pub(in crate::tui) fn handle_pr_closed(&mut self, id: TaskId) -> Vec<Command> {
        self.handle_pr_terminal(id, "closed")
    }

    /// Shared terminal-state handler for PRs that have reached a final GitHub
    /// state (merged or closed). `verb` is "merged" or "closed" and drives the
    /// status bar and notification copy.
    fn handle_pr_terminal(&mut self, id: TaskId, verb: &str) -> Vec<Command> {
        let mut cmds = Vec::new();

        if let Some(task) = self.find_task_mut(id) {
            if task.status != TaskStatus::Review {
                return cmds;
            }

            let pr_label = task.url.as_ref().map_or("PR".to_string(), |u| u.label());
            let task_title = task.title.clone();

            // Detach: kill tmux window but preserve worktree
            if let Some(window) = task.tmux_window.take() {
                cmds.push(Command::Task(
                    crate::tui::commands::TaskCommand::KillTmuxWindow { window },
                ));
            }
            task.status = TaskStatus::Done;
            task.sub_status = SubStatus::default_for(TaskStatus::Done);
            let fields = crate::tui::commands::PersistFields::from_task(task);

            self.clear_agent_tracking(id);
            self.sync_board_selection();
            self.set_status(format!(
                "{pr_label} {verb} \u{2014} task #{id} moved to Done"
            ));

            cmds.push(Command::Task(crate::tui::commands::TaskCommand::Persist(
                fields,
            )));

            if self.notifications_enabled {
                cmds.push(Command::System(
                    crate::tui::commands::SystemCommand::SendNotification {
                        title: format!("PR {verb}"),
                        body: format!("{pr_label} {verb}: {task_title}"),
                        urgent: false,
                    },
                ));
            }
        }

        cmds.extend(self.maybe_respawn_split_pane(id));

        cmds
    }

    pub(in crate::tui) fn handle_pr_review_state(
        &mut self,
        id: TaskId,
        review_decision: Option<ReviewDecision>,
    ) -> Vec<Command> {
        if let Some(task) = self.find_task_mut(id) {
            if task.status != TaskStatus::Review {
                return vec![];
            }
            // Don't overwrite attention-requiring substatuses
            if task.sub_status == SubStatus::Conflict {
                return vec![];
            }
            let new_sub = match review_decision {
                Some(ReviewDecision::Approved) => SubStatus::Approved,
                Some(ReviewDecision::ChangesRequested) => SubStatus::ChangesRequested,
                _ => SubStatus::AwaitingReview,
            };
            if task.sub_status != new_sub {
                task.sub_status = new_sub;
                let fields = crate::tui::commands::PersistFields::from_task(task);
                return vec![Command::Task(crate::tui::commands::TaskCommand::Persist(
                    fields,
                ))];
            }
        }
        vec![]
    }
}
