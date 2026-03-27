use crossterm::event::{KeyCode, KeyEvent};

use super::{App, Command, InputMode, Message, MoveDirection, TaskDraft};
use crate::models::TaskStatus;

impl App {
    /// Translate a terminal key event into zero or more commands, depending on current mode.
    pub fn handle_key(&mut self, key: KeyEvent) -> Vec<Command> {
        if self.error_popup.is_some() {
            self.error_popup = None;
            return vec![];
        }

        match self.mode.clone() {
            InputMode::Normal => self.handle_key_normal(key),
            InputMode::InputTitle => self.handle_key_text_input(key),
            InputMode::InputDescription => self.handle_key_text_input(key),
            InputMode::InputRepoPath => self.handle_key_text_input(key),
            InputMode::ConfirmDelete => self.handle_key_confirm_delete(key),
        }
    }

    fn handle_key_normal(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Char('q') => self.update(Message::Quit),

            KeyCode::Char('h') | KeyCode::Left => self.update(Message::NavigateColumn(-1)),
            KeyCode::Char('l') | KeyCode::Right => self.update(Message::NavigateColumn(1)),
            KeyCode::Char('j') | KeyCode::Down => self.update(Message::NavigateRow(1)),
            KeyCode::Char('k') | KeyCode::Up => self.update(Message::NavigateRow(-1)),

            KeyCode::Char('n') => {
                self.mode = InputMode::InputTitle;
                self.input_buffer.clear();
                self.task_draft = None;
                self.status_message = Some("Enter title: ".to_string());
                vec![]
            }

            KeyCode::Char('d') => {
                if let Some(task) = self.selected_task() {
                    let id = task.id;
                    let status = task.status;
                    let has_window = task.tmux_window.is_some();
                    let has_worktree = task.worktree.is_some();
                    match status {
                        TaskStatus::Ready => {
                            self.update(Message::DispatchTask(id))
                        }
                        TaskStatus::Running | TaskStatus::Review => {
                            if has_window {
                                self.status_message = Some(
                                    "Agent already running, press g to jump".to_string(),
                                );
                                vec![]
                            } else if has_worktree {
                                self.update(Message::ResumeTask(id))
                            } else {
                                self.status_message = Some(
                                    "No worktree to resume, move to Ready and re-dispatch".to_string(),
                                );
                                vec![]
                            }
                        }
                        _ => {
                            self.status_message = Some(
                                "Move task to Ready before dispatching (press m)".to_string(),
                            );
                            vec![]
                        }
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
                        self.status_message = Some("No active session".to_string());
                        vec![]
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
                // Delete selected task — enter confirm mode
                if self.selected_task().is_some() {
                    self.mode = InputMode::ConfirmDelete;
                    self.status_message = Some("Delete task? (y/n)".to_string());
                }
                vec![]
            }

            _ => vec![],
        }
    }

    fn handle_key_text_input(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Esc => {
                self.mode = InputMode::Normal;
                self.input_buffer.clear();
                self.task_draft = None;
                self.status_message = None;
                vec![]
            }

            KeyCode::Enter => {
                let value = self.input_buffer.trim().to_string();
                self.input_buffer.clear();

                match self.mode.clone() {
                    InputMode::InputTitle => {
                        if value.is_empty() {
                            self.mode = InputMode::Normal;
                            self.task_draft = None;
                            self.status_message = None;
                            vec![]
                        } else {
                            self.task_draft = Some(TaskDraft {
                                title: value,
                                description: String::new(),
                            });
                            self.mode = InputMode::InputDescription;
                            self.status_message = Some("Enter description: ".to_string());
                            vec![]
                        }
                    }
                    InputMode::InputDescription => {
                        if let Some(ref mut draft) = self.task_draft {
                            draft.description = value;
                        }
                        self.mode = InputMode::InputRepoPath;
                        self.status_message = Some("Enter repo path: ".to_string());
                        vec![]
                    }
                    InputMode::InputRepoPath => {
                        let draft = self.task_draft.take().unwrap_or_default();
                        let repo_path = if value.is_empty() {
                            if let Some(first) = self.repo_paths.first() {
                                first.clone()
                            } else {
                                self.task_draft = Some(draft);
                                self.status_message =
                                    Some("Repo path required (no saved paths available)".to_string());
                                return vec![];
                            }
                        } else {
                            value
                        };
                        self.mode = InputMode::Normal;
                        self.status_message = None;
                        vec![
                            Command::InsertTask {
                                title: draft.title,
                                description: draft.description,
                                repo_path: repo_path.clone(),
                            },
                            Command::SaveRepoPath(repo_path),
                        ]
                    }
                    _ => vec![],
                }
            }

            KeyCode::Backspace => {
                self.input_buffer.pop();
                vec![]
            }

            KeyCode::Char(c) => {
                // In repo path mode with empty buffer, 1-9 selects a saved path
                if self.mode == InputMode::InputRepoPath
                    && self.input_buffer.is_empty() && c.is_ascii_digit() && c != '0'
                {
                    let idx = (c as usize) - ('1' as usize);
                    if idx < self.repo_paths.len() {
                        let draft = self.task_draft.take().unwrap_or_default();
                        let repo_path = self.repo_paths[idx].clone();
                        self.mode = InputMode::Normal;
                        self.status_message = None;
                        return vec![
                            Command::InsertTask {
                                title: draft.title,
                                description: draft.description,
                                repo_path: repo_path.clone(),
                            },
                            Command::SaveRepoPath(repo_path),
                        ];
                    }
                }
                self.input_buffer.push(c);
                vec![]
            }

            _ => vec![],
        }
    }

    fn handle_key_confirm_delete(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.mode = InputMode::Normal;
                self.status_message = None;
                if let Some(task) = self.selected_task() {
                    let id = task.id;
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
}
