use crate::dispatch;
use crate::models::{EpicId, TaskId, TaskStatus};
use crate::tui::types::{Command, InputMode, MergeAction, MergeQueue, Message};
use crate::tui::App;

impl App {
    pub(in crate::tui) fn handle_finish_complete(&mut self, id: TaskId) -> Vec<Command> {
        let in_queue = self.merge_queue.as_ref().is_some_and(|q| q.current == Some(id));

        self.rebase_conflict_tasks.remove(&id);
        let mut cmds = if let Some(task) = self.find_task_mut(id) {
            task.tmux_window = None;
            task.status = TaskStatus::Done;
            let task_clone = task.clone();
            self.clear_agent_tracking(id);
            self.clamp_selection();
            if !in_queue {
                self.set_status(format!("Task {} finished", id));
            }
            vec![Command::PersistTask(task_clone)]
        } else {
            vec![]
        };

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
        if is_conflict {
            self.rebase_conflict_tasks.insert(id);
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
                return vec![];
            }
        }

        self.set_status(error);
        vec![]
    }

    pub(in crate::tui) fn handle_pr_created(
        &mut self,
        id: TaskId,
        pr_url: String,
    ) -> Vec<Command> {
        let in_queue = self.merge_queue.as_ref().is_some_and(|q| q.current == Some(id));

        let mut cmds = if let Some(task) = self.find_task_mut(id) {
            task.pr_url = Some(pr_url.clone());
            let task_clone = task.clone();
            if !in_queue {
                let pr_num = crate::models::pr_number_from_url(&pr_url);
                let label = pr_num.map_or("PR".to_string(), |n| format!("PR #{n}"));
                self.set_status(format!("{label} created: {pr_url}"));
            }
            vec![Command::PersistTask(task_clone)]
        } else {
            vec![]
        };

        if in_queue {
            if let Some(q) = &mut self.merge_queue {
                q.completed += 1;
                q.current = None;
            }
            cmds.extend(self.advance_merge_queue());
        }

        cmds
    }

    pub(in crate::tui) fn handle_pr_failed(
        &mut self,
        id: TaskId,
        error: String,
    ) -> Vec<Command> {
        if let Some(q) = &mut self.merge_queue {
            if q.current == Some(id) {
                q.current = None;
                q.failed = Some(id);
                let completed = q.completed;
                let total = q.task_ids.len();
                self.set_status(format!(
                    "Epic merge paused ({completed}/{total}): PR #{id} \u{2014} {error}"
                ));
                return vec![];
            }
        }

        self.set_status(error);
        vec![]
    }

    pub(in crate::tui) fn handle_pr_merged(&mut self, id: TaskId) -> Vec<Command> {
        let mut cmds = Vec::new();

        if let Some(task) = self.find_task_mut(id) {
            if task.status != TaskStatus::Review {
                return cmds;
            }

            let pr_label = task.pr_url.as_deref()
                .and_then(crate::models::pr_number_from_url)
                .map_or("PR".to_string(), |n| format!("PR #{n}"));
            let title = task.title.clone();

            // Detach: kill tmux window but preserve worktree
            if let Some(window) = task.tmux_window.take() {
                cmds.push(Command::KillTmuxWindow { window });
            }
            task.status = TaskStatus::Done;
            let task_clone = task.clone();

            self.clear_agent_tracking(id);
            self.clamp_selection();
            self.set_status(format!("{pr_label} merged \u{2014} task #{id} moved to Done"));

            cmds.push(Command::PersistTask(task_clone));

            if self.notifications_enabled {
                cmds.push(Command::SendNotification {
                    title: "PR merged".to_string(),
                    body: format!("{pr_label} merged: {title}"),
                    urgent: false,
                });
            }
        }

        cmds
    }

    pub(in crate::tui) fn handle_start_wrap_up(&mut self, id: TaskId) -> Vec<Command> {
        let branch = match self.find_task(id) {
            Some(t) if t.status == TaskStatus::Review => {
                match t.worktree.as_ref().and_then(dispatch::branch_from_worktree) {
                    Some(b) => b,
                    None => return vec![],
                }
            }
            _ => return vec![],
        };

        self.input.mode = InputMode::ConfirmWrapUp(id);
        self.set_status(format!(
            "Wrap up {}: (r) rebase onto main  (p) create PR  (Esc) cancel", branch
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
        self.rebase_conflict_tasks.remove(&id);

        if let Some(task) = self.find_task(id) {
            let worktree = match &task.worktree {
                Some(wt) => wt.clone(),
                None => return vec![],
            };
            let branch = match dispatch::branch_from_worktree(&worktree) {
                Some(b) => b,
                None => return vec![],
            };
            vec![Command::Finish {
                id,
                repo_path: task.repo_path.clone(),
                branch,
                worktree,
                tmux_window: task.tmux_window.clone(),
            }]
        } else {
            vec![]
        }
    }

    pub(in crate::tui) fn handle_wrap_up_pr(&mut self) -> Vec<Command> {
        let id = match self.input.mode {
            InputMode::ConfirmWrapUp(id) => id,
            _ => return vec![],
        };
        self.input.mode = InputMode::Normal;
        self.set_status("Creating PR...".to_string());

        if let Some(task) = self.find_task(id) {
            let worktree = match &task.worktree {
                Some(wt) => wt.clone(),
                None => return vec![],
            };
            let branch = match dispatch::branch_from_worktree(&worktree) {
                Some(b) => b,
                None => return vec![],
            };
            vec![Command::CreatePr {
                id,
                repo_path: task.repo_path.clone(),
                branch,
                title: task.title.clone(),
                description: task.description.clone(),
            }]
        } else {
            vec![]
        }
    }

    pub(in crate::tui) fn handle_cancel_wrap_up(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.clear_status();
        vec![]
    }

    pub(in crate::tui) fn handle_start_epic_wrap_up(&mut self, epic_id: EpicId) -> Vec<Command> {
        let review_count = self.tasks.iter()
            .filter(|t| {
                t.epic_id == Some(epic_id)
                    && t.status == TaskStatus::Review
                    && t.worktree.is_some()
            })
            .count();

        if review_count == 0 {
            return self.update(Message::StatusInfo(
                "No review tasks to wrap up".to_string(),
            ));
        }

        self.input.mode = InputMode::ConfirmEpicWrapUp(epic_id);
        self.set_status(format!(
            "Wrap up {} review task{}: (r) rebase all  (p) PR all  (Esc) cancel",
            review_count,
            if review_count == 1 { "" } else { "s" },
        ));
        vec![]
    }

    pub(in crate::tui) fn handle_epic_wrap_up(&mut self, action: MergeAction) -> Vec<Command> {
        let epic_id = match self.input.mode {
            InputMode::ConfirmEpicWrapUp(id) => id,
            _ => return vec![],
        };
        self.input.mode = InputMode::Normal;

        let mut review_tasks: Vec<&crate::models::Task> = self.tasks.iter()
            .filter(|t| {
                t.epic_id == Some(epic_id)
                    && t.status == TaskStatus::Review
                    && t.worktree.is_some()
            })
            .collect();
        review_tasks.sort_by_key(|t| t.sort_order.unwrap_or(t.id.0));

        let task_ids: Vec<TaskId> = review_tasks.iter().map(|t| t.id).collect();

        if task_ids.is_empty() {
            return vec![];
        }

        self.merge_queue = Some(MergeQueue {
            epic_id,
            action,
            task_ids,
            completed: 0,
            current: None,
            failed: None,
        });

        self.advance_merge_queue()
    }

    pub(in crate::tui) fn advance_merge_queue(&mut self) -> Vec<Command> {
        let (total, next_idx, action) = match &self.merge_queue {
            Some(q) => (q.task_ids.len(), q.completed, q.action.clone()),
            None => return vec![],
        };

        if next_idx >= total {
            self.merge_queue = None;
            self.set_status(format!("Epic merge complete: {total}/{total} done"));
            return vec![];
        }

        let next_id = self.merge_queue.as_ref().unwrap().task_ids[next_idx];

        // Validate the task is still eligible
        let task_data = match self.find_task(next_id) {
            Some(t) if t.status == TaskStatus::Review && t.worktree.is_some() => {
                let worktree = t.worktree.clone().unwrap();
                let branch = dispatch::branch_from_worktree(&worktree);
                let repo_path = t.repo_path.clone();
                let title = t.title.clone();
                let description = t.description.clone();
                let tmux_window = t.tmux_window.clone();
                branch.map(|b| (worktree, b, repo_path, title, description, tmux_window))
            }
            _ => None,
        };

        let Some((worktree, branch, repo_path, title, description, tmux_window)) = task_data else {
            // Skip this task — no longer eligible
            if let Some(q) = &mut self.merge_queue {
                q.completed += 1;
            }
            return self.advance_merge_queue();
        };

        if let Some(q) = &mut self.merge_queue {
            q.current = Some(next_id);
        }

        self.set_status(format!(
            "Epic merge: {next_idx}/{total} done \u{2014} processing #{}",
            next_id
        ));

        match action {
            MergeAction::Rebase => {
                self.rebase_conflict_tasks.remove(&next_id);
                vec![Command::Finish {
                    id: next_id,
                    repo_path,
                    branch,
                    worktree,
                    tmux_window,
                }]
            }
            MergeAction::Pr => vec![Command::CreatePr {
                id: next_id,
                repo_path,
                branch,
                title,
                description,
            }],
        }
    }

    pub(in crate::tui) fn handle_cancel_epic_wrap_up(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.clear_status();
        vec![]
    }

    pub(in crate::tui) fn handle_cancel_merge_queue(&mut self) -> Vec<Command> {
        self.merge_queue = None;
        self.set_status("Merge queue cancelled".to_string());
        vec![]
    }
}
