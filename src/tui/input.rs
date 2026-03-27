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
        }
    }

    fn handle_key_normal(&mut self, key: KeyEvent) -> Vec<Command> {
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

            KeyCode::Char('x') => self.update(Message::ConfirmDeleteStart),

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
            KeyCode::Char('y') | KeyCode::Char('Y') => self.update(Message::ConfirmDeleteYes),
            _ => self.update(Message::CancelDelete),
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
}
