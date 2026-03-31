use std::time::{Duration, Instant};

use crate::models::{TaskId, TaskStatus};
use crate::tui::types::{Command, InputMode};
use crate::tui::App;

impl App {
    pub(in crate::tui) fn handle_tick(&mut self) -> Vec<Command> {
        // Auto-clear transient status messages after 5 seconds (only in Normal mode)
        if self.input.mode == InputMode::Normal {
            if let Some(set_at) = self.status_message_set_at {
                if set_at.elapsed() > Duration::from_secs(5) {
                    self.clear_status();
                }
            }
        }

        let mut cmds: Vec<Command> = self
            .tasks
            .iter()
            .filter(|t| t.tmux_window.is_some())
            .filter_map(|t| {
                t.tmux_window.clone().map(|window| Command::CaptureTmux {
                    id: t.id,
                    window,
                })
            })
            .collect();

        // Check for stale agents
        let timeout = self.agents.inactivity_timeout;
        let newly_stale: Vec<TaskId> = self
            .tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Running && t.tmux_window.is_some())
            .filter(|t| !self.agents.stale_tasks.contains(&t.id))
            .filter(|t| {
                self.agents.last_output_change
                    .get(&t.id)
                    .is_some_and(|instant| instant.elapsed() > timeout)
            })
            .map(|t| t.id)
            .collect();

        for id in newly_stale {
            let stale_cmds = self.handle_stale_agent(id);
            cmds.extend(stale_cmds);
        }

        // Poll PR status for review tasks with open PRs
        let pr_poll_interval = Duration::from_secs(30);
        let pr_tasks: Vec<(TaskId, String)> = self
            .tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Review && t.pr_url.is_some())
            .filter(|t| {
                self.agents
                    .last_pr_poll
                    .get(&t.id)
                    .is_none_or(|last| last.elapsed() > pr_poll_interval)
            })
            .map(|t| (t.id, t.pr_url.clone().unwrap()))
            .collect();

        for (id, pr_url) in pr_tasks {
            self.agents.last_pr_poll.insert(id, Instant::now());
            cmds.push(Command::CheckPrStatus { id, pr_url });
        }

        // Refresh review board data periodically (regardless of active tab)
        let needs_fetch = self.last_review_fetch
            .map(|t| t.elapsed() > Duration::from_secs(60))
            .unwrap_or(true);
        if needs_fetch && !self.review_board_loading {
            self.review_board_loading = true;
            cmds.push(Command::FetchReviewPrs);
        }

        cmds.push(Command::RefreshFromDb);
        cmds
    }

    pub(in crate::tui) fn handle_tmux_output(&mut self, id: TaskId, output: String, activity_ts: u64) -> Vec<Command> {
        let activity_changed = self.agents.last_activity
            .get(&id)
            .is_none_or(|&prev| prev != activity_ts);
        if activity_changed {
            self.agents.last_output_change.insert(id, Instant::now());
            self.agents.stale_tasks.remove(&id);
            self.agents.last_activity.insert(id, activity_ts);
        }
        self.agents.tmux_outputs.insert(id, output);
        vec![]
    }

    pub(in crate::tui) fn handle_window_gone(&mut self, id: TaskId) -> Vec<Command> {
        if let Some(task) = self.find_task(id) {
            if task.status == TaskStatus::Running {
                // Running task lost its window — likely crashed
                return self.handle_agent_crashed(id);
            }
        }
        // Non-running task: existing behavior
        if let Some(task) = self.find_task_mut(id) {
            task.tmux_window = None;
            let task_clone = task.clone();
            vec![Command::PersistTask(task_clone)]
        } else {
            vec![]
        }
    }

    pub(in crate::tui) fn handle_stale_agent(&mut self, id: TaskId) -> Vec<Command> {
        self.agents.stale_tasks.insert(id);
        if let Some(task) = self.find_task(id) {
            let elapsed = self.agents.last_output_change
                .get(&id)
                .map(|t| t.elapsed().as_secs() / 60)
                .unwrap_or(0);
            self.set_status(format!(
                "Task {} inactive for {}m - press d to retry",
                task.id, elapsed
            ));
        }
        vec![]
    }

    pub(in crate::tui) fn handle_agent_crashed(&mut self, id: TaskId) -> Vec<Command> {
        self.agents.crashed_tasks.insert(id);
        if let Some(task) = self.find_task(id) {
            self.set_status(format!(
                "Task {} agent crashed - press d to retry", task.id
            ));
        }
        vec![]
    }

    pub(in crate::tui) fn handle_kill_and_retry(&mut self, id: TaskId) -> Vec<Command> {
        self.input.mode = InputMode::ConfirmRetry(id);
        let label = if self.agents.crashed_tasks.contains(&id) {
            "crashed"
        } else {
            "stale"
        };
        self.set_status(format!(
            "Agent {} - [r] Resume  [f] Fresh start  [Esc] Cancel", label
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
            let old_window = task.tmux_window.take();
            let task_clone = task.clone();

            let mut cmds = Vec::new();
            if let Some(window) = old_window {
                cmds.push(Command::KillTmuxWindow { window });
            }
            cmds.push(Command::Resume { task: task_clone });
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
            let task_clone = task.clone();

            let mut cmds = Vec::new();
            if let Some(c) = cleanup {
                cmds.push(c);
            }
            cmds.push(Command::PersistTask(task_clone.clone()));
            cmds.push(Command::Dispatch { task: task_clone });
            cmds
        } else {
            vec![]
        }
    }

    pub(in crate::tui) fn handle_cancel_retry(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.clear_status();
        vec![]
    }
}
