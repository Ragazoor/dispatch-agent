use crate::models::{Epic, EpicId, RepoPath, Task, TaskId, TaskStatus, TaskUsage, epic_status};
use crate::tui::types::{
    BoardSelection, ColumnItem, Command, EpicDraft, InputMode, ViewMode,
};
use crate::tui::truncate_title;
use crate::tui::App;

impl App {
    pub(in crate::tui) fn handle_dispatch_epic(&mut self, id: EpicId) -> Vec<Command> {
        let Some(epic) = self.epics.iter().find(|e| e.id == id) else {
            return vec![];
        };
        let subtask_statuses = self.subtask_statuses(id);
        let status = epic_status(epic, &subtask_statuses);

        if status != TaskStatus::Backlog {
            self.set_status("No backlog tasks in epic".to_string());
            return vec![];
        }

        if epic.plan.is_some() {
            // Epic has a plan — dispatch the next backlog subtask sorted by sort_order
            let mut backlog_subtasks: Vec<&Task> = self
                .tasks
                .iter()
                .filter(|t| t.epic_id == Some(id) && t.status == TaskStatus::Backlog)
                .collect();
            backlog_subtasks.sort_by_key(|t| (t.sort_order.unwrap_or(t.id.0), t.id.0));

            match backlog_subtasks.first() {
                Some(task) if task.plan.is_some() => {
                    vec![Command::Dispatch { task: (*task).clone() }]
                }
                Some(task) => {
                    match task.tag.as_deref() {
                        Some("epic") => vec![Command::Brainstorm { task: (*task).clone() }],
                        Some("feature") => vec![Command::Plan { task: (*task).clone() }],
                        _ => vec![Command::Dispatch { task: (*task).clone() }],
                    }
                }
                None => {
                    vec![Command::DispatchEpic { epic: epic.clone() }]
                }
            }
        } else {
            // No plan — spawn planning subtask
            vec![Command::DispatchEpic { epic: epic.clone() }]
        }
    }

    pub(in crate::tui) fn handle_enter_epic(&mut self, epic_id: EpicId) -> Vec<Command> {
        let saved_board = match &self.view_mode {
            ViewMode::Board(sel) => sel.clone(),
            ViewMode::Epic { saved_board, .. } => saved_board.clone(),
            ViewMode::ReviewBoard { saved_board, .. } => saved_board.clone(),
        };
        self.view_mode = ViewMode::Epic {
            epic_id,
            selection: BoardSelection::new(),
            saved_board,
        };
        self.detail_visible = false;
        vec![]
    }

    pub(in crate::tui) fn handle_exit_epic(&mut self) -> Vec<Command> {
        if let ViewMode::Epic { saved_board, .. } = &self.view_mode {
            self.view_mode = ViewMode::Board(saved_board.clone());
        }
        self.detail_visible = false;
        vec![]
    }

    pub(in crate::tui) fn handle_refresh_epics(&mut self, epics: Vec<Epic>) -> Vec<Command> {
        self.epics = epics;
        let valid_ids: std::collections::HashSet<EpicId> = self.epics.iter().map(|e| e.id).collect();
        self.selected_epics.retain(|id| valid_ids.contains(id));
        vec![]
    }

    pub(in crate::tui) fn handle_refresh_usage(&mut self, usage: Vec<TaskUsage>) -> Vec<Command> {
        self.usage = usage.into_iter().map(|u| (u.task_id, u)).collect();
        vec![]
    }

    pub(in crate::tui) fn handle_epic_created(&mut self, epic: Epic) -> Vec<Command> {
        self.epics.push(epic);
        vec![]
    }

    pub(in crate::tui) fn handle_edit_epic(&mut self, id: EpicId) -> Vec<Command> {
        if let Some(epic) = self.epics.iter().find(|e| e.id == id) {
            vec![Command::EditEpicInEditor(epic.clone())]
        } else {
            vec![]
        }
    }

    pub(in crate::tui) fn handle_epic_edited(&mut self, epic: Epic) -> Vec<Command> {
        if let Some(e) = self.epics.iter_mut().find(|e| e.id == epic.id) {
            e.title = epic.title;
            e.description = epic.description;
            e.updated_at = chrono::Utc::now();
        }
        vec![]
    }

    pub(in crate::tui) fn handle_delete_epic(&mut self, id: EpicId) -> Vec<Command> {
        let mut cmds = Vec::new();
        // Clean up worktrees/tmux for subtasks before deleting
        let subtask_ids: Vec<TaskId> = self.tasks
            .iter()
            .filter(|t| t.epic_id == Some(id))
            .map(|t| t.id)
            .collect();
        for task_id in subtask_ids {
            if let Some(task) = self.find_task_mut(task_id) {
                let cleanup = Self::take_cleanup(task);
                if let Some(c) = cleanup {
                    cmds.push(c);
                }
                self.clear_agent_tracking(task_id);
            }
        }
        self.epics.retain(|e| e.id != id);
        self.tasks.retain(|t| t.epic_id != Some(id));
        // If we were viewing this epic, exit
        if matches!(&self.view_mode, ViewMode::Epic { epic_id, .. } if *epic_id == id) {
            self.handle_exit_epic();
        }
        self.clamp_selection();
        cmds.push(Command::DeleteEpic(id));
        cmds
    }

    pub(in crate::tui) fn handle_confirm_delete_epic(&mut self) -> Vec<Command> {
        if let Some(ColumnItem::Epic(epic)) = self.selected_column_item() {
            let title = truncate_title(&epic.title, 30);
            self.input.mode = InputMode::ConfirmDeleteEpic;
            self.set_status(format!("Delete epic {title} and subtasks? (y/n)"));
        }
        vec![]
    }

    pub(in crate::tui) fn handle_mark_epic_done(&mut self, id: EpicId) -> Vec<Command> {
        if let Some(epic) = self.epics.iter_mut().find(|e| e.id == id) {
            epic.done = true;
        }
        vec![Command::PersistEpic { id, done: Some(true), sort_order: None }]
    }

    pub(in crate::tui) fn handle_mark_epic_undone(&mut self, id: EpicId) -> Vec<Command> {
        if let Some(epic) = self.epics.iter_mut().find(|e| e.id == id) {
            epic.done = false;
        }
        vec![Command::PersistEpic { id, done: Some(false), sort_order: None }]
    }

    pub(in crate::tui) fn handle_confirm_epic_done(&mut self) -> Vec<Command> {
        let id = match self.input.mode {
            InputMode::ConfirmEpicDone(id) => id,
            _ => return vec![],
        };
        self.input.mode = InputMode::Normal;
        self.clear_status();
        self.handle_mark_epic_done(id)
    }

    pub(in crate::tui) fn handle_cancel_epic_done(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.clear_status();
        vec![]
    }

    pub(in crate::tui) fn handle_archive_epic(&mut self, id: EpicId) -> Vec<Command> {
        let mut cmds = Vec::new();
        let subtask_ids: Vec<TaskId> = self.tasks
            .iter()
            .filter(|t| t.epic_id == Some(id) && t.status != TaskStatus::Archived)
            .map(|t| t.id)
            .collect();
        for task_id in subtask_ids {
            cmds.extend(self.handle_archive_task(task_id));
        }
        self.epics.retain(|e| e.id != id);
        if matches!(&self.view_mode, ViewMode::Epic { epic_id, .. } if *epic_id == id) {
            self.handle_exit_epic();
        }
        self.clamp_selection();
        cmds.push(Command::DeleteEpic(id));
        cmds
    }

    pub(in crate::tui) fn handle_confirm_archive_epic(&mut self) -> Vec<Command> {
        if let Some(ColumnItem::Epic(epic)) = self.selected_column_item() {
            let id = epic.id;
            let not_done_count = self.subtask_statuses(id)
                .iter()
                .filter(|s| **s != TaskStatus::Done)
                .count();
            if not_done_count > 0 {
                let noun = if not_done_count == 1 { "subtask" } else { "subtasks" };
                self.set_status(format!(
                    "Cannot archive epic: {} {} not done", not_done_count, noun
                ));
                return vec![];
            }
            self.input.mode = InputMode::ConfirmArchiveEpic;
            self.set_status("Archive epic and all subtasks? (y/n)".to_string());
        }
        vec![]
    }

    pub(in crate::tui) fn handle_start_new_epic(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::InputEpicTitle;
        self.input.buffer.clear();
        self.input.epic_draft = None;
        self.set_status("Epic title: ".to_string());
        vec![]
    }

    pub(in crate::tui) fn handle_submit_epic_title(&mut self, value: String) -> Vec<Command> {
        self.input.buffer.clear();
        if value.is_empty() {
            self.input.mode = InputMode::Normal;
            self.clear_status();
        } else {
            self.input.epic_draft = Some(EpicDraft {
                title: value,
                description: String::new(),
                repo_path: RepoPath::default(),
            });
            self.input.mode = InputMode::InputEpicDescription;
            self.set_status("Epic description: ".to_string());
        }
        vec![]
    }

    pub(in crate::tui) fn handle_submit_epic_description(&mut self, value: String) -> Vec<Command> {
        self.input.buffer.clear();
        if let Some(ref mut draft) = self.input.epic_draft {
            draft.description = value;
        }
        self.input.mode = InputMode::InputEpicRepoPath;
        self.set_status("Epic repo path: ".to_string());
        vec![]
    }

    pub(in crate::tui) fn handle_submit_epic_repo_path(&mut self, value: String) -> Vec<Command> {
        self.input.buffer.clear();
        let repo_path = if value.is_empty() {
            if let Some(first) = self.repo_paths.first() {
                RepoPath(first.clone())
            } else {
                self.set_status("Repo path required".to_string());
                return vec![];
            }
        } else {
            RepoPath(value)
        };

        self.finish_epic_creation(repo_path)
    }

    pub(in crate::tui) fn finish_epic_creation(&mut self, repo_path: RepoPath) -> Vec<Command> {
        let mut draft = self.input.epic_draft.take().unwrap_or_default();
        let path_str = repo_path.0.clone();
        draft.repo_path = repo_path;
        self.input.mode = InputMode::Normal;
        self.clear_status();
        vec![
            Command::InsertEpic(draft),
            Command::SaveRepoPath(path_str),
        ]
    }
}
