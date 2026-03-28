use crossterm::event::{KeyCode, KeyEvent};

use super::{App, Command, InputMode, Message, MoveDirection};
use crate::models::{TaskId, TaskStatus};

impl App {
    /// Translate a terminal key event into zero or more commands, depending on current mode.
    pub fn handle_key(&mut self, key: KeyEvent) -> Vec<Command> {
        if self.error_popup.is_some() {
            return self.update(Message::DismissError);
        }

        match self.mode.clone() {
            InputMode::Normal => self.handle_key_normal(key),
            InputMode::InputTitle
            | InputMode::InputDescription
            | InputMode::InputRepoPath => self.handle_key_text_input(key),
            InputMode::ConfirmDelete => self.handle_key_confirm_delete(key),
            InputMode::QuickDispatch => self.handle_key_quick_dispatch(key),
            InputMode::ConfirmRetry(id) => self.handle_key_confirm_retry(key, id),
            InputMode::ConfirmArchive => self.handle_key_confirm_archive(key),
        }
    }

    fn handle_key_normal(&mut self, key: KeyEvent) -> Vec<Command> {
        // Archive panel intercepts certain keys when visible
        if self.show_archived {
            match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    let count = self.archived_tasks().len();
                    if count > 0 && self.selected_archive_row < count - 1 {
                        self.selected_archive_row += 1;
                    }
                    return vec![];
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.selected_archive_row = self.selected_archive_row.saturating_sub(1);
                    return vec![];
                }
                KeyCode::Char('H') => {
                    return self.update(Message::ToggleArchive);
                }
                KeyCode::Char('x') => {
                    let archived = self.archived_tasks();
                    if archived.get(self.selected_archive_row).is_some() {
                        self.mode = InputMode::ConfirmDelete;
                        self.status_message = Some("Delete permanently? (y/n)".to_string());
                    }
                    return vec![];
                }
                KeyCode::Char('e') => {
                    let archived = self.archived_tasks();
                    if let Some(task) = archived.get(self.selected_archive_row) {
                        return vec![Command::EditTaskInEditor((*task).clone())];
                    }
                    return vec![];
                }
                KeyCode::Char('q') => return self.update(Message::Quit),
                KeyCode::Esc => {
                    return self.update(Message::ToggleArchive);
                }
                _ => return vec![],
            }
        }

        match key.code {
            KeyCode::Char('q') => self.update(Message::Quit),

            KeyCode::Char('h') | KeyCode::Left => self.update(Message::NavigateColumn(-1)),
            KeyCode::Char('l') | KeyCode::Right => self.update(Message::NavigateColumn(1)),
            KeyCode::Char('j') | KeyCode::Down => self.update(Message::NavigateRow(1)),
            KeyCode::Char('k') | KeyCode::Up => self.update(Message::NavigateRow(-1)),

            KeyCode::Char('n') => self.update(Message::StartNewTask),

            KeyCode::Char('d') => {
                if let Some(task) = self.selected_task() {
                    let id = task.id;
                    let status = task.status;
                    let has_window = task.tmux_window.is_some();
                    let has_worktree = task.worktree.is_some();
                    match status {
                        TaskStatus::Backlog => self.update(Message::BrainstormTask(id)),
                        TaskStatus::Ready => self.update(Message::DispatchTask(id)),
                        TaskStatus::Running | TaskStatus::Review => {
                            if self.stale_tasks.contains(&id) || self.crashed_tasks.contains(&id) {
                                self.update(Message::KillAndRetry(id))
                            } else if has_window {
                                self.update(Message::StatusInfo(
                                    "Agent already running, press g to jump".to_string(),
                                ))
                            } else if has_worktree {
                                self.update(Message::ResumeTask(id))
                            } else {
                                self.update(Message::StatusInfo(
                                    "No worktree to resume, move to Ready and re-dispatch".to_string(),
                                ))
                            }
                        }
                        TaskStatus::Done => self.update(Message::StatusInfo(
                            "Task is done".to_string(),
                        )),
                        TaskStatus::Archived => self.update(Message::StatusInfo(
                            "Task is archived".to_string(),
                        )),
                    }
                } else {
                    vec![]
                }
            }

            KeyCode::Char('g') => {
                if let Some(task) = self.selected_task() {
                    if let Some(window) = &task.tmux_window {
                        vec![Command::JumpToTmux { window: window.clone() }]
                    } else {
                        self.update(Message::StatusInfo("No active session".to_string()))
                    }
                } else {
                    vec![]
                }
            }

            KeyCode::Char('m') => {
                if let Some(task) = self.selected_task() {
                    let id = task.id;
                    self.update(Message::MoveTask { id, direction: MoveDirection::Forward })
                } else {
                    vec![]
                }
            }

            KeyCode::Char('M') => {
                if let Some(task) = self.selected_task() {
                    let id = task.id;
                    self.update(Message::MoveTask { id, direction: MoveDirection::Backward })
                } else {
                    vec![]
                }
            }

            KeyCode::Enter => self.update(Message::ToggleDetail),

            KeyCode::Char('e') => {
                if let Some(task) = self.selected_task() {
                    vec![Command::EditTaskInEditor(task.clone())]
                } else {
                    vec![]
                }
            }

            KeyCode::Char('x') => {
                if self.selected_task().is_some() {
                    self.mode = InputMode::ConfirmArchive;
                    self.status_message = Some("Archive task? (y/n)".to_string());
                }
                vec![]
            }

            KeyCode::Char('D') => {
                match self.repo_paths.len() {
                    0 => self.update(Message::StatusInfo(
                        "No saved repo paths — create a task first".to_string(),
                    )),
                    1 => {
                        let repo_path = self.repo_paths[0].clone();
                        self.update(Message::QuickDispatch { repo_path })
                    }
                    _ => self.update(Message::StartQuickDispatchSelection),
                }
            }

            KeyCode::Char('H') => self.update(Message::ToggleArchive),

            _ => vec![],
        }
    }

    fn handle_key_text_input(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Esc => self.update(Message::CancelInput),
            KeyCode::Enter => {
                let value = self.input_buffer.trim().to_string();
                match self.mode.clone() {
                    InputMode::InputTitle => self.update(Message::SubmitTitle(value)),
                    InputMode::InputDescription => self.update(Message::SubmitDescription(value)),
                    InputMode::InputRepoPath => self.update(Message::SubmitRepoPath(value)),
                    _ => vec![],
                }
            }
            KeyCode::Backspace => self.update(Message::InputBackspace),
            KeyCode::Char(c) => self.update(Message::InputChar(c)),
            _ => vec![],
        }
    }

    fn handle_key_confirm_delete(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.mode = InputMode::Normal;
                self.status_message = None;
                let task_id = if self.show_archived {
                    self.archived_tasks()
                        .get(self.selected_archive_row)
                        .map(|t| t.id)
                } else {
                    self.selected_task().map(|t| t.id)
                };
                if let Some(id) = task_id {
                    self.update(Message::DeleteTask(id))
                } else {
                    vec![]
                }
            }
            _ => {
                self.mode = InputMode::Normal;
                self.status_message = None;
                vec![]
            }
        }
    }

    fn handle_key_quick_dispatch(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Esc => self.update(Message::CancelInput),
            KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
                let idx = (c as usize) - ('1' as usize);
                self.update(Message::SelectQuickDispatchRepo(idx))
            }
            _ => vec![],
        }
    }

    fn handle_key_confirm_retry(&mut self, key: KeyEvent, id: TaskId) -> Vec<Command> {
        match key.code {
            KeyCode::Char('r') => self.update(Message::RetryResume(id)),
            KeyCode::Char('f') => self.update(Message::RetryFresh(id)),
            KeyCode::Esc => self.update(Message::CancelRetry),
            _ => vec![],
        }
    }

    fn handle_key_confirm_archive(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.mode = InputMode::Normal;
                self.status_message = None;
                if let Some(task) = self.selected_task() {
                    let id = task.id;
                    self.update(Message::ArchiveTask(id))
                } else {
                    vec![]
                }
            }
            _ => {
                self.mode = InputMode::Normal;
                self.status_message = None;
                vec![]
            }
        }
    }
}
