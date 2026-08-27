//! PR-related message handlers: review state polling.

use crate::models::{ReviewDecision, SubStatus, TaskId, TaskStatus};

use super::super::types::*;
use super::super::App;

impl App {
    /// Builds the `Persist` command, plus a `SendNotification` when
    /// notifications are enabled, for a PR-status task update. Shared by
    /// `handle_pr_merged` and `handle_pr_closed` — they differ in what they
    /// change on the task, not in how the resulting write/notification pair
    /// is emitted.
    fn persist_and_notify(
        &self,
        fields: crate::tui::commands::PersistFields,
        title: String,
        body: String,
    ) -> Vec<Command> {
        let mut cmds = vec![Command::Task(crate::tui::commands::TaskCommand::Persist(
            fields,
        ))];
        if self.notifications_enabled {
            cmds.push(Command::System(
                crate::tui::commands::SystemCommand::SendNotification {
                    title,
                    body,
                    urgent: false,
                },
            ));
        }
        cmds
    }

    pub(in crate::tui) fn handle_pr_merged(&mut self, id: TaskId) -> Vec<Command> {
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
                "{pr_label} merged \u{2014} task #{id} moved to Done"
            ));

            cmds.extend(self.persist_and_notify(
                fields,
                "PR merged".to_string(),
                format!("{pr_label} merged: {task_title}"),
            ));
        }

        cmds.extend(self.maybe_respawn_split_pane(id));

        cmds
    }

    /// A closed-without-merge PR is NOT a terminal event: the task stays in
    /// review — only `sub_status` changes, to `pr_closed`, so the user
    /// notices and decides what to do (reopen the PR, push a new one,
    /// archive the task). No tmux/worktree teardown, unlike `PrMerged`.
    ///
    /// `PollPrStatus` re-fires this event every tick the PR stays closed, so
    /// this is guarded on `sub_status != PrClosed` to avoid re-persisting and
    /// re-notifying on every tick. Overrides `sub_status = Conflict`
    /// unconditionally — a closed PR is a stronger, more definitive GitHub
    /// signal than a local rebase conflict.
    pub(in crate::tui) fn handle_pr_closed(&mut self, id: TaskId) -> Vec<Command> {
        let mut cmds = Vec::new();

        if let Some(task) = self.find_task_mut(id) {
            if task.status != TaskStatus::Review || task.sub_status == SubStatus::PrClosed {
                return cmds;
            }

            let pr_label = task.url.as_ref().map_or("PR".to_string(), |u| u.label());
            let task_title = task.title.clone();

            task.sub_status = SubStatus::PrClosed;
            let fields = crate::tui::commands::PersistFields::from_task(task);

            self.set_status(format!(
                "{pr_label} closed \u{2014} task #{id} marked \"PR closed\""
            ));

            cmds = self.persist_and_notify(
                fields,
                "PR closed".to_string(),
                format!("{pr_label} closed: {task_title}"),
            );
        }

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
