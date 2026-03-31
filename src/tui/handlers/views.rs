use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::models::{ReviewPr, Task, TaskId, TaskStatus};
use crate::tui::types::{Command, InputMode, ReviewBoardSelection, ViewMode};
use crate::tui::App;

impl App {
    pub(in crate::tui) fn handle_switch_to_review_board(&mut self) -> Vec<Command> {
        if matches!(self.view_mode, ViewMode::ReviewBoard { .. }) {
            return vec![];
        }
        let saved_board = match &self.view_mode {
            ViewMode::Board(sel) => sel.clone(),
            ViewMode::Epic { saved_board, .. } => saved_board.clone(),
            ViewMode::ReviewBoard { saved_board, .. } => saved_board.clone(),
        };
        self.view_mode = ViewMode::ReviewBoard {
            selection: ReviewBoardSelection::new(),
            saved_board,
        };
        self.review_board_loading = true;
        let needs_fetch = self.last_review_fetch
            .map(|t| t.elapsed() > Duration::from_secs(60))
            .unwrap_or(true);
        if needs_fetch {
            vec![Command::FetchReviewPrs]
        } else {
            self.review_board_loading = false;
            vec![]
        }
    }

    pub(in crate::tui) fn handle_switch_to_task_board(&mut self) -> Vec<Command> {
        if let ViewMode::ReviewBoard { saved_board, .. } = &self.view_mode {
            self.view_mode = ViewMode::Board(saved_board.clone());
        }
        vec![]
    }

    pub(in crate::tui) fn handle_review_prs_loaded(&mut self, prs: Vec<ReviewPr>) -> Vec<Command> {
        let cmds = vec![Command::PersistReviewPrs(prs.clone())];
        self.review_prs = prs;
        self.review_board_loading = false;
        self.last_review_fetch = Some(Instant::now());
        self.clamp_review_selection();
        cmds
    }

    pub(in crate::tui) fn clamp_review_selection(&mut self) {
        let counts: [usize; 3] = std::array::from_fn(|col| {
            self.review_prs.iter()
                .filter(|pr| pr.review_decision.column_index() == col)
                .count()
        });
        if let Some(sel) = self.review_selection_mut() {
            for (col, &count) in counts.iter().enumerate() {
                if count == 0 {
                    sel.selected_row[col] = 0;
                } else if sel.selected_row[col] >= count {
                    sel.selected_row[col] = count - 1;
                }
            }
        }
    }

    pub(in crate::tui) fn handle_review_prs_fetch_failed(&mut self, error: String) -> Vec<Command> {
        self.review_board_loading = false;
        self.set_status(format!("Failed to fetch review PRs: {error}"));
        vec![]
    }

    pub(in crate::tui) fn handle_start_repo_filter(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::RepoFilter;
        vec![]
    }

    pub(in crate::tui) fn handle_close_repo_filter(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.clamp_selection();
        let mut paths: Vec<_> = self.repo_filter.iter().cloned().collect();
        paths.sort();
        let value = paths.join("\n");
        vec![Command::PersistStringSetting {
            key: "repo_filter".to_string(),
            value,
        }]
    }

    pub(in crate::tui) fn handle_toggle_repo_filter(&mut self, path: String) -> Vec<Command> {
        if self.repo_filter.contains(&path) {
            self.repo_filter.remove(&path);
        } else {
            self.repo_filter.insert(path);
        }
        self.clamp_selection();
        vec![]
    }

    pub(in crate::tui) fn handle_toggle_all_repo_filter(&mut self) -> Vec<Command> {
        if self.repo_filter.len() == self.repo_paths.len() {
            self.repo_filter.clear();
        } else {
            self.repo_filter = self.repo_paths.iter().cloned().collect();
        }
        self.clamp_selection();
        vec![]
    }

    pub(in crate::tui) fn handle_start_save_preset(&mut self) -> Vec<Command> {
        self.input.buffer.clear();
        self.input.mode = InputMode::InputPresetName;
        vec![]
    }

    pub(in crate::tui) fn handle_save_filter_preset(&mut self, name: String) -> Vec<Command> {
        let name = name.trim().to_string();
        if name.is_empty() {
            self.input.mode = InputMode::RepoFilter;
            return vec![];
        }
        let repos: HashSet<String> = self.repo_filter.clone();
        // Update or insert in the presets list
        if let Some(existing) = self.filter_presets.iter_mut().find(|(n, _)| *n == name) {
            existing.1.clone_from(&repos);
        } else {
            self.filter_presets.push((name.clone(), repos));
            self.filter_presets.sort_by(|a, b| a.0.cmp(&b.0));
        }
        self.input.buffer.clear();
        self.input.mode = InputMode::RepoFilter;
        self.set_status(format!("Saved preset \"{name}\""));
        let mut paths: Vec<_> = self.repo_filter.iter().cloned().collect();
        paths.sort();
        vec![Command::PersistFilterPreset {
            name,
            repo_paths: paths.join("\n"),
        }]
    }

    pub(in crate::tui) fn handle_load_filter_preset(&mut self, name: String) -> Vec<Command> {
        if let Some((_, repos)) = self.filter_presets.iter().find(|(n, _)| *n == name) {
            // Intersect with known repo_paths to skip stale entries
            let known: HashSet<&String> = self.repo_paths.iter().collect();
            self.repo_filter = repos.iter().filter(|p| known.contains(p)).cloned().collect();
            self.clamp_selection();
            self.set_status(format!("Loaded preset \"{name}\""));
        }
        vec![]
    }

    pub(in crate::tui) fn handle_start_delete_preset(&mut self) -> Vec<Command> {
        if self.filter_presets.is_empty() {
            return vec![];
        }
        self.input.mode = InputMode::ConfirmDeletePreset;
        vec![]
    }

    pub(in crate::tui) fn handle_delete_filter_preset(&mut self, name: String) -> Vec<Command> {
        self.filter_presets.retain(|(n, _)| *n != name);
        self.input.mode = InputMode::RepoFilter;
        self.set_status(format!("Deleted preset \"{name}\""));
        vec![Command::DeleteFilterPreset(name)]
    }

    pub(in crate::tui) fn handle_cancel_preset_input(&mut self) -> Vec<Command> {
        self.input.buffer.clear();
        self.input.mode = InputMode::RepoFilter;
        vec![]
    }

    pub(in crate::tui) fn handle_filter_presets_loaded(&mut self, presets: Vec<(String, HashSet<String>)>) -> Vec<Command> {
        self.filter_presets = presets;
        vec![]
    }

    pub(in crate::tui) fn handle_repo_paths_updated(&mut self, paths: Vec<String>) -> Vec<Command> {
        self.repo_paths = paths;
        vec![]
    }

    pub(in crate::tui) fn handle_refresh_tasks(&mut self, new_tasks: Vec<Task>) -> Vec<Command> {
        let mut cmds = Vec::new();

        for new_task in &new_tasks {
            if self.notifications_enabled {
                // Extract old state before any mutable borrows
                let old_task = self.find_task(new_task.id);
                let was_needs_input = old_task.is_some_and(|t| t.needs_input);
                let was_review = old_task.is_some_and(|t| t.status == TaskStatus::Review);

                // Detect needs_input transition: false → true (running tasks only)
                if new_task.needs_input && !was_needs_input
                    && new_task.status == TaskStatus::Running
                    && !self.agents.notified_needs_input.contains(&new_task.id)
                {
                    self.agents.notified_needs_input.insert(new_task.id);
                    cmds.push(Command::SendNotification {
                        title: format!("Task #{}: {}", new_task.id.0, new_task.title),
                        body: "Agent needs your input".to_string(),
                        urgent: true,
                    });
                }

                // Detect review transition
                if new_task.status == TaskStatus::Review && !was_review
                    && !self.agents.notified_review.contains(&new_task.id)
                {
                    self.agents.notified_review.insert(new_task.id);
                    cmds.push(Command::SendNotification {
                        title: format!("Task #{}: {}", new_task.id.0, new_task.title),
                        body: "Ready for review".to_string(),
                        urgent: false,
                    });
                }
            }

            // Always clear notified state when task leaves the triggering state,
            // even when notifications are disabled. This prevents stale entries from
            // suppressing notifications after re-enabling.
            if new_task.status != TaskStatus::Review {
                self.agents.notified_review.remove(&new_task.id);
            }
            if !new_task.needs_input {
                self.agents.notified_needs_input.remove(&new_task.id);
            }
        }

        // Merge DB state into in-memory state, preserving tmux_outputs
        // Prune selections for tasks that no longer exist
        let valid_ids: HashSet<TaskId> = new_tasks.iter().map(|t| t.id).collect();
        self.selected_tasks.retain(|id| valid_ids.contains(id));
        self.rebase_conflict_tasks.retain(|id| valid_ids.contains(id));
        self.tasks = new_tasks;
        self.clamp_selection();
        cmds
    }

    pub(in crate::tui) fn handle_error(&mut self, msg: String) -> Vec<Command> {
        self.error_popup = Some(msg);
        vec![]
    }

    pub(in crate::tui) fn handle_dismiss_error(&mut self) -> Vec<Command> {
        self.error_popup = None;
        vec![]
    }

    pub(in crate::tui) fn handle_status_info(&mut self, msg: String) -> Vec<Command> {
        self.set_status(msg);
        vec![]
    }
}
