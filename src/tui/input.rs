use crossterm::event::{KeyCode, KeyEvent};

use super::{App, Command, InputMode, Message, MoveDirection};

impl App {
    /// Translate a terminal key event into zero or more commands, depending on current mode.
    pub fn handle_key(&mut self, key: KeyEvent) -> Vec<Command> {
        match &self.mode.clone() {
            InputMode::Normal => self.handle_key_normal(key),
            InputMode::InputTitle => self.handle_key_text_input(key),
            InputMode::InputDescription { .. } => self.handle_key_text_input(key),
            InputMode::InputRepoPath { .. } => self.handle_key_text_input(key),
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
                self.status_message = Some("Enter title: ".to_string());
                vec![Command::None]
            }

            KeyCode::Char('d') => {
                if let Some(task) = self.selected_task() {
                    let id = task.id;
                    self.update(Message::DispatchTask(id))
                } else {
                    vec![Command::None]
                }
            }

            KeyCode::Char('m') => {
                if let Some(task) = self.selected_task() {
                    let id = task.id;
                    self.update(Message::MoveTask { id, direction: MoveDirection::Forward })
                } else {
                    vec![Command::None]
                }
            }

            KeyCode::Char('M') => {
                if let Some(task) = self.selected_task() {
                    let id = task.id;
                    self.update(Message::MoveTask { id, direction: MoveDirection::Backward })
                } else {
                    vec![Command::None]
                }
            }

            KeyCode::Enter => self.update(Message::ToggleDetail),

            KeyCode::Char('x') => {
                // Delete selected task — enter confirm mode
                if self.selected_task().is_some() {
                    self.mode = InputMode::ConfirmDelete;
                    self.status_message = Some("Delete task? (y/n)".to_string());
                }
                vec![Command::None]
            }

            _ => vec![Command::None],
        }
    }

    fn handle_key_text_input(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Esc => {
                self.mode = InputMode::Normal;
                self.input_buffer.clear();
                self.status_message = None;
                vec![Command::None]
            }

            KeyCode::Enter => {
                let value = self.input_buffer.trim().to_string();
                self.input_buffer.clear();

                match self.mode.clone() {
                    InputMode::InputTitle => {
                        if value.is_empty() {
                            self.mode = InputMode::Normal;
                            self.status_message = None;
                            vec![Command::None]
                        } else {
                            self.mode = InputMode::InputDescription { title: value };
                            self.status_message = Some("Enter description: ".to_string());
                            vec![Command::None]
                        }
                    }
                    InputMode::InputDescription { title } => {
                        self.mode = InputMode::InputRepoPath {
                            title,
                            description: value,
                        };
                        self.status_message = Some("Enter repo path: ".to_string());
                        vec![Command::None]
                    }
                    InputMode::InputRepoPath { title, description } => {
                        self.mode = InputMode::Normal;
                        self.status_message = None;
                        let repo_path = if value.is_empty() {
                            "/".to_string()
                        } else {
                            value
                        };
                        self.update(Message::CreateTask {
                            title,
                            description,
                            repo_path,
                        })
                    }
                    _ => vec![Command::None],
                }
            }

            KeyCode::Backspace => {
                self.input_buffer.pop();
                vec![Command::None]
            }

            KeyCode::Char(c) => {
                // In repo path mode with empty buffer, 1-9 selects a saved path
                if let InputMode::InputRepoPath { ref title, ref description } = self.mode {
                    if self.input_buffer.is_empty() && c.is_ascii_digit() && c != '0' {
                        let idx = (c as usize) - ('1' as usize);
                        if idx < self.repo_paths.len() {
                            let title = title.clone();
                            let description = description.clone();
                            let repo_path = self.repo_paths[idx].clone();
                            self.mode = InputMode::Normal;
                            self.status_message = None;
                            return self.update(Message::CreateTask {
                                title,
                                description,
                                repo_path,
                            });
                        }
                    }
                }
                self.input_buffer.push(c);
                vec![Command::None]
            }

            _ => vec![Command::None],
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
                    vec![Command::None]
                }
            }
            _ => {
                self.mode = InputMode::Normal;
                self.status_message = None;
                vec![Command::None]
            }
        }
    }
}
