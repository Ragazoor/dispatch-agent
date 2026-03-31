use crate::models::{EpicId, RepoPath};
use crate::tui::truncate_title;
use crate::tui::types::{Command, InputMode, TaskDraft, ViewMode};
use crate::tui::App;

impl App {
    pub(in crate::tui) fn handle_start_new_task(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::InputTitle;
        self.input.buffer.clear();
        self.input.task_draft = None;
        self.set_status("Enter title: ".to_string());
        vec![]
    }

    pub(in crate::tui) fn handle_cancel_input(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.input.buffer.clear();
        self.input.task_draft = None;
        self.clear_status();
        vec![]
    }

    pub(in crate::tui) fn handle_confirm_delete_start(&mut self) -> Vec<Command> {
        if let Some(task) = self.selected_task() {
            let title = truncate_title(&task.title, 30);
            let status = task.status.as_str();
            let warning = if task.worktree.is_some() { " (has worktree)" } else { "" };
            self.input.mode = InputMode::ConfirmDelete;
            self.set_status(format!("Delete {title} [{status}]{warning}? (y/n)"));
        }
        vec![]
    }

    pub(in crate::tui) fn handle_confirm_delete_yes(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.clear_status();
        if let Some(task) = self.selected_task() {
            let id = task.id;
            self.handle_delete_task(id)
        } else {
            vec![]
        }
    }

    pub(in crate::tui) fn handle_cancel_delete(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.clear_status();
        vec![]
    }

    pub(in crate::tui) fn handle_submit_title(&mut self, value: String) -> Vec<Command> {
        self.input.buffer.clear();
        if value.is_empty() {
            self.input.mode = InputMode::Normal;
            self.input.task_draft = None;
            self.clear_status();
        } else {
            self.input.task_draft = Some(TaskDraft {
                title: value,
                description: String::new(),
                repo_path: RepoPath::default(),
                tag: None,
            });
            self.input.mode = InputMode::InputDescription;
            self.set_status("Enter description: ".to_string());
        }
        vec![]
    }

    pub(in crate::tui) fn handle_submit_description(&mut self, value: String) -> Vec<Command> {
        self.input.buffer.clear();
        if let Some(ref mut draft) = self.input.task_draft {
            draft.description = value;
        }
        self.input.mode = InputMode::InputRepoPath;
        self.set_status("Enter repo path: ".to_string());
        vec![]
    }

    pub(in crate::tui) fn handle_submit_repo_path(&mut self, value: String) -> Vec<Command> {
        self.input.buffer.clear();
        let repo_path = if value.is_empty() {
            if let Some(first) = self.repo_paths.first() {
                RepoPath(first.clone())
            } else {
                self.set_status("Repo path required (no saved paths available)".to_string());
                return vec![];
            }
        } else {
            RepoPath(value)
        };
        if let Some(ref mut draft) = self.input.task_draft {
            draft.repo_path = repo_path;
        }
        self.input.mode = InputMode::InputTag;
        self.set_status("Tag: (b)ug (f)eature (c)hore (e)pic (Enter=none)".to_string());
        vec![]
    }

    pub(in crate::tui) fn handle_submit_tag(&mut self, tag: Option<String>) -> Vec<Command> {
        if let Some(ref mut draft) = self.input.task_draft {
            draft.tag = tag;
        }
        let repo_path = self.input.task_draft.as_ref()
            .map(|d| d.repo_path.clone())
            .unwrap_or_default();
        self.finish_task_creation(repo_path)
    }

    pub(in crate::tui) fn handle_input_char(&mut self, c: char) -> Vec<Command> {
        // In repo path mode with empty buffer, 1-9 selects a saved path
        if (self.input.mode == InputMode::InputRepoPath
            || self.input.mode == InputMode::InputEpicRepoPath)
            && self.input.buffer.is_empty()
            && c.is_ascii_digit()
            && c != '0'
        {
            let idx = (c as usize) - ('1' as usize);
            if idx < self.repo_paths.len() {
                let repo_path = RepoPath(self.repo_paths[idx].clone());
                if self.input.mode == InputMode::InputEpicRepoPath {
                    return self.finish_epic_creation(repo_path);
                }
                // For tasks, go through the tag selection step
                if let Some(ref mut draft) = self.input.task_draft {
                    draft.repo_path = repo_path;
                }
                self.input.mode = InputMode::InputTag;
                self.set_status("Tag: (b)ug (f)eature (c)hore (e)pic (Enter=none)".to_string());
                return vec![];
            }
        }
        self.input.buffer.push(c);
        vec![]
    }

    pub(in crate::tui) fn handle_input_backspace(&mut self) -> Vec<Command> {
        self.input.buffer.pop();
        vec![]
    }

    pub(in crate::tui) fn handle_start_quick_dispatch_selection(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::QuickDispatch;
        self.set_status("Select repo path (1-9) or Esc to cancel".to_string());
        vec![]
    }

    pub(in crate::tui) fn handle_select_quick_dispatch_repo(&mut self, idx: usize) -> Vec<Command> {
        if idx < self.repo_paths.len() {
            let repo_path = RepoPath(self.repo_paths[idx].clone());
            self.input.mode = InputMode::Normal;
            self.clear_status();
            self.handle_quick_dispatch(repo_path, None)
        } else {
            vec![]
        }
    }

    pub(in crate::tui) fn handle_quick_dispatch(&mut self, repo_path: RepoPath, epic_id: Option<EpicId>) -> Vec<Command> {
        vec![Command::QuickDispatch {
            draft: TaskDraft {
                title: "Quick task".to_string(),
                description: String::new(),
                repo_path,
                tag: None,
            },
            epic_id,
        }]
    }

    pub(in crate::tui) fn finish_task_creation(&mut self, repo_path: RepoPath) -> Vec<Command> {
        let mut draft = self.input.task_draft.take().unwrap_or_default();
        let path_str = repo_path.0.clone();
        draft.repo_path = repo_path;
        self.input.mode = InputMode::Normal;
        self.clear_status();
        let epic_id = match &self.view_mode {
            ViewMode::Epic { epic_id, .. } => Some(*epic_id),
            _ => None,
        };
        vec![
            Command::InsertTask { draft, epic_id },
            Command::SaveRepoPath(path_str),
        ]
    }
}
