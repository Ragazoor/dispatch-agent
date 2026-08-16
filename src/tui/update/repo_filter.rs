//! Repo filter and filter preset handlers.

use std::collections::HashSet;

use super::super::types::*;
use super::super::{filtered_repos, has_new_repo_option, App};

impl App {
    pub(in crate::tui) fn handle_start_repo_filter(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::RepoFilter;
        self.input.repo_cursor = 0;
        vec![]
    }

    pub(in crate::tui) fn handle_move_repo_cursor(&mut self, delta: isize) -> Vec<Command> {
        let count = if let Some(candidates) = self.picker_candidates() {
            let filtered = filtered_repos(candidates, &self.input.buffer);
            let extra = has_new_repo_option(&self.input.buffer, &filtered) as usize;
            filtered.len() + extra
        } else if matches!(self.input.mode, InputMode::RepoFilter) {
            self.board.repo_paths.len() + 1 // +1 for the "Active sessions only" toggle at cursor 0
        } else {
            self.board.repo_paths.len()
        };
        if count == 0 {
            return vec![];
        }
        self.input.repo_cursor =
            (self.input.repo_cursor as isize + delta).rem_euclid(count as isize) as usize;
        self.dirty = true;
        vec![]
    }

    pub(in crate::tui) fn handle_close_repo_filter(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.sync_board_selection();
        self.reset_column_scroll();
        let mut paths: Vec<_> = self.filter.repos.iter().cloned().collect();
        paths.sort();
        let value = serde_json::to_string(&paths).unwrap_or_else(|_| "[]".to_string());
        let mode_value = self.filter.mode.as_str();
        vec![
            Command::Settings(
                crate::tui::commands::SettingsCommand::PersistStringSetting {
                    key: "repo_filter".to_string(),
                    value,
                },
            ),
            Command::Settings(
                crate::tui::commands::SettingsCommand::PersistStringSetting {
                    key: "repo_filter_mode".to_string(),
                    value: mode_value.to_string(),
                },
            ),
        ]
    }

    pub(in crate::tui) fn handle_toggle_repo_filter(&mut self, path: String) -> Vec<Command> {
        if self.filter.repos.contains(&path) {
            self.filter.repos.remove(&path);
        } else {
            self.filter.repos.insert(path);
        }
        self.sync_board_selection();
        self.reset_column_scroll();
        self.dirty = true;
        vec![]
    }

    pub(in crate::tui) fn handle_toggle_repo_filter_mode(&mut self) -> Vec<Command> {
        self.filter.mode = match self.filter.mode {
            RepoFilterMode::Include => RepoFilterMode::Exclude,
            RepoFilterMode::Exclude => RepoFilterMode::Include,
        };
        self.sync_board_selection();
        self.reset_column_scroll();
        self.dirty = true;
        vec![]
    }

    pub(in crate::tui) fn handle_toggle_only_active(&mut self) -> Vec<Command> {
        self.filter.only_active = !self.filter.only_active;
        self.sync_board_selection();
        self.reset_column_scroll();
        self.dirty = true;
        vec![]
    }

    pub(in crate::tui) fn handle_toggle_all_repo_filter(&mut self) -> Vec<Command> {
        if self.filter.repos.len() == self.board.repo_paths.len() {
            self.filter.repos.clear();
        } else {
            self.filter.repos = self.board.repo_paths.iter().cloned().collect();
        }
        self.sync_board_selection();
        self.reset_column_scroll();
        self.dirty = true;
        vec![]
    }

    pub(in crate::tui) fn handle_save_filter_preset(&mut self, name: String) -> Vec<Command> {
        let name = name.trim().to_string();
        if name.is_empty() {
            self.input.mode = InputMode::RepoFilter;
            return vec![];
        }
        let repos: HashSet<String> = self.filter.repos.clone();
        let mode = self.filter.mode;
        // Update or insert in the presets list
        if let Some(existing) = self.filter.presets.iter_mut().find(|(n, _, _)| *n == name) {
            existing.1.clone_from(&repos);
            existing.2 = mode;
        } else {
            self.filter.presets.push((name.clone(), repos, mode));
            self.filter.presets.sort_by(|a, b| a.0.cmp(&b.0));
        }
        self.input.clear_buffer();
        self.input.mode = InputMode::RepoFilter;
        self.set_status(format!("Saved preset \"{name}\""));
        let mut paths: Vec<_> = self.filter.repos.iter().cloned().collect();
        paths.sort();
        vec![Command::RepoFilter(
            crate::tui::commands::RepoFilterCommand::PersistFilterPreset {
                name,
                repo_paths: paths,
                mode,
            },
        )]
    }

    pub(in crate::tui) fn handle_load_filter_preset(&mut self, name: String) -> Vec<Command> {
        if let Some((_, repos, mode)) = self.filter.presets.iter().find(|(n, _, _)| *n == name) {
            // Intersect with known repo_paths to skip stale entries
            let known: HashSet<&String> = self.board.repo_paths.iter().collect();
            self.filter.repos = repos
                .iter()
                .filter(|p| known.contains(p))
                .cloned()
                .collect();
            self.filter.mode = *mode;
            self.sync_board_selection();
            self.reset_column_scroll();
            self.set_status(format!("Loaded preset \"{name}\""));
            self.dirty = true;
        }
        vec![]
    }

    pub(in crate::tui) fn handle_start_save_preset(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::InputPresetName;
        vec![]
    }

    pub(in crate::tui) fn handle_start_delete_preset(&mut self) -> Vec<Command> {
        if self.filter.presets.is_empty() {
            return vec![];
        }
        self.input.mode = InputMode::ConfirmDeletePreset;
        vec![]
    }

    pub(in crate::tui) fn handle_delete_filter_preset(&mut self, name: String) -> Vec<Command> {
        self.filter.presets.retain(|(n, _, _)| *n != name);
        self.input.mode = InputMode::RepoFilter;
        self.set_status(format!("Deleted preset \"{name}\""));
        vec![Command::RepoFilter(
            crate::tui::commands::RepoFilterCommand::DeleteFilterPreset(name),
        )]
    }

    pub(in crate::tui) fn handle_start_delete_repo_path(&mut self) -> Vec<Command> {
        if self.board.repo_paths.is_empty() {
            return vec![];
        }
        self.input.mode = InputMode::ConfirmDeleteRepoPath;
        vec![]
    }

    pub(in crate::tui) fn handle_delete_repo_path(&mut self, path: String) -> Vec<Command> {
        self.filter.repos.remove(&path);
        self.input.mode = InputMode::RepoFilter;
        self.set_status("Deleted repo path".to_string());
        vec![Command::RepoFilter(
            crate::tui::commands::RepoFilterCommand::DeleteRepoPath(path),
        )]
    }

    pub(in crate::tui) fn handle_cancel_preset_input(&mut self) -> Vec<Command> {
        self.input.clear_buffer();
        self.input.mode = InputMode::RepoFilter;
        vec![]
    }

    pub(in crate::tui) fn handle_filter_presets_loaded(
        &mut self,
        presets: Vec<(String, HashSet<String>, RepoFilterMode)>,
    ) -> Vec<Command> {
        self.filter.presets = presets;
        vec![]
    }
}
