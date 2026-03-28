use crossterm::event::{KeyCode, KeyEvent};

use super::{App, ColumnItem, Command, InputMode, Message, MoveDirection, ViewMode};
use crate::models::{TaskId, TaskStatus};

impl App {
    /// Translate a terminal key event into zero or more commands, depending on current mode.
    pub fn handle_key(&mut self, key: KeyEvent) -> Vec<Command> {
        if self.error_popup.is_some() {
            return self.update(Message::DismissError);
        }

        match self.input.mode.clone() {
            InputMode::Normal => self.handle_key_normal(key),
            InputMode::InputTitle
            | InputMode::InputDescription
            | InputMode::InputRepoPath => self.handle_key_text_input(key),
            InputMode::ConfirmDelete => self.handle_key_confirm_delete(key),
            InputMode::QuickDispatch => self.handle_key_quick_dispatch(key),
            InputMode::ConfirmRetry(id) => self.handle_key_confirm_retry(key, id),
            InputMode::ConfirmArchive => self.handle_key_confirm_archive(key),
            InputMode::InputEpicTitle
            | InputMode::InputEpicDescription
            | InputMode::InputEpicRepoPath => self.handle_key_epic_text_input(key),
            InputMode::ConfirmDeleteEpic => self.handle_key_confirm_delete_epic(key),
            InputMode::ConfirmArchiveEpic => self.handle_key_confirm_archive_epic(key),
            InputMode::ConfirmFinish(_) => self.handle_key_confirm_finish(key),
            InputMode::Help => self.handle_key_help(key),
        }
    }

    fn handle_key_normal(&mut self, key: KeyEvent) -> Vec<Command> {
        if self.archive.visible {
            return self.handle_key_archive(key);
        }

        match key.code {
            KeyCode::Char('q') => self.update(Message::Quit),

            KeyCode::Char('h') | KeyCode::Left => self.update(Message::NavigateColumn(-1)),
            KeyCode::Char('l') | KeyCode::Right => self.update(Message::NavigateColumn(1)),
            KeyCode::Char('j') | KeyCode::Down => self.update(Message::NavigateRow(1)),
            KeyCode::Char('k') | KeyCode::Up => self.update(Message::NavigateRow(-1)),

            KeyCode::Char('n') => self.update(Message::StartNewTask),
            KeyCode::Char('E') => self.update(Message::StartNewEpic),
            KeyCode::Char('d') => self.handle_key_dispatch(),
            KeyCode::Char('m') => {
                if matches!(self.selected_column_item(), Some(ColumnItem::Epic(_))) {
                    return self.update(Message::StatusInfo("Epic status is derived from subtasks".to_string()));
                }
                self.handle_key_move(MoveDirection::Forward)
            }
            KeyCode::Char('M') => {
                if matches!(self.selected_column_item(), Some(ColumnItem::Epic(_))) {
                    return self.update(Message::StatusInfo("Epic status is derived from subtasks".to_string()));
                }
                self.handle_key_move(MoveDirection::Backward)
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

            KeyCode::Char(' ') => {
                if let Some(task) = self.selected_task() {
                    let id = task.id;
                    self.update(Message::ToggleSelect(id))
                } else {
                    vec![]
                }
            }

            KeyCode::Enter => {
                match self.selected_column_item() {
                    Some(ColumnItem::Epic(epic)) => {
                        let id = epic.id;
                        self.update(Message::EnterEpic(id))
                    }
                    _ => self.update(Message::ToggleDetail),
                }
            }

            KeyCode::Char('e') => {
                if let ViewMode::Epic { epic_id, .. } = &self.view_mode {
                    let id = *epic_id;
                    return self.update(Message::EditEpic(id));
                }
                if let Some(task) = self.selected_task() {
                    vec![Command::EditTaskInEditor(task.clone())]
                } else {
                    vec![]
                }
            }

            KeyCode::Char('V') => {
                if let Some(ColumnItem::Epic(epic)) = self.selected_column_item() {
                    let id = epic.id;
                    self.update(Message::MarkEpicDone(id))
                } else {
                    vec![]
                }
            }

            KeyCode::Char('x') => {
                match self.selected_column_item() {
                    Some(ColumnItem::Epic(_)) => {
                        self.update(Message::ConfirmArchiveEpic)
                    }
                    _ => {
                        if !self.selected_tasks.is_empty() {
                            let count = self.selected_tasks.len();
                            self.input.mode = InputMode::ConfirmArchive;
                            self.status_message = Some(format!("Archive {} tasks? (y/n)", count));
                        } else if self.selected_task().is_some() {
                            self.input.mode = InputMode::ConfirmArchive;
                            self.status_message = Some("Archive task? (y/n)".to_string());
                        }
                        vec![]
                    }
                }
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

            KeyCode::Char('?') => self.update(Message::ToggleHelp),

            KeyCode::Esc => {
                if matches!(self.view_mode, ViewMode::Epic { .. }) {
                    self.update(Message::ExitEpic)
                } else if !self.selected_tasks.is_empty() {
                    self.update(Message::ClearSelection)
                } else {
                    vec![]
                }
            }

            _ => vec![],
        }
    }

    /// Handle keys when the archive overlay is visible.
    fn handle_key_archive(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                let count = self.archived_tasks().len();
                if count > 0 && self.archive.selected_row < count - 1 {
                    self.archive.selected_row += 1;
                }
                vec![]
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.archive.selected_row = self.archive.selected_row.saturating_sub(1);
                vec![]
            }
            KeyCode::Char('H') | KeyCode::Esc => self.update(Message::ToggleArchive),
            KeyCode::Char('x') => {
                let archived = self.archived_tasks();
                if archived.get(self.archive.selected_row).is_some() {
                    self.input.mode = InputMode::ConfirmDelete;
                    self.status_message = Some("Delete permanently? (y/n)".to_string());
                }
                vec![]
            }
            KeyCode::Char('e') => {
                let archived = self.archived_tasks();
                if let Some(task) = archived.get(self.archive.selected_row) {
                    vec![Command::EditTaskInEditor((*task).clone())]
                } else {
                    vec![]
                }
            }
            KeyCode::Char('q') => self.update(Message::Quit),
            _ => vec![],
        }
    }

    /// Handle the 'd' key: dispatch, brainstorm, resume, or retry depending on task status.
    fn handle_key_dispatch(&mut self) -> Vec<Command> {
        let Some(task) = self.selected_task() else {
            return vec![];
        };
        let id = task.id;
        let status = task.status;
        let has_window = task.tmux_window.is_some();
        let has_worktree = task.worktree.is_some();
        let is_problematic = self.agents.stale_tasks.contains(&id)
            || self.agents.crashed_tasks.contains(&id);

        match status {
            TaskStatus::Backlog => self.update(Message::BrainstormTask(id)),
            TaskStatus::Ready => self.update(Message::DispatchTask(id)),
            TaskStatus::Running | TaskStatus::Review => {
                if is_problematic {
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
    }

    /// Handle 'm'/'M' key: move selected task(s) forward or backward.
    fn handle_key_move(&mut self, direction: MoveDirection) -> Vec<Command> {
        if !self.selected_tasks.is_empty() {
            let ids: Vec<_> = self.selected_tasks.iter().copied().collect();
            self.update(Message::BatchMoveTasks { ids, direction })
        } else if let Some(task) = self.selected_task() {
            let id = task.id;
            self.update(Message::MoveTask { id, direction })
        } else {
            vec![]
        }
    }

    fn handle_key_text_input(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Esc => self.update(Message::CancelInput),
            KeyCode::Enter => {
                let value = self.input.buffer.trim().to_string();
                match self.input.mode.clone() {
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
                self.input.mode = InputMode::Normal;
                self.status_message = None;
                if self.archive.visible {
                    self.confirm_delete_archived()
                } else {
                    self.confirm_delete_selected()
                }
            }
            _ => {
                self.input.mode = InputMode::Normal;
                self.status_message = None;
                vec![]
            }
        }
    }

    fn confirm_delete_archived(&mut self) -> Vec<Command> {
        self.archived_tasks()
            .get(self.archive.selected_row)
            .map(|t| t.id)
            .map(|id| self.update(Message::DeleteTask(id)))
            .unwrap_or_default()
    }

    fn confirm_delete_selected(&mut self) -> Vec<Command> {
        self.selected_task()
            .map(|t| t.id)
            .map(|id| self.update(Message::DeleteTask(id)))
            .unwrap_or_default()
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
                self.input.mode = InputMode::Normal;
                self.status_message = None;
                if !self.selected_tasks.is_empty() {
                    let ids: Vec<_> = self.selected_tasks.iter().copied().collect();
                    self.update(Message::BatchArchiveTasks(ids))
                } else if let Some(task) = self.selected_task() {
                    let id = task.id;
                    self.update(Message::ArchiveTask(id))
                } else {
                    vec![]
                }
            }
            _ => {
                self.input.mode = InputMode::Normal;
                self.status_message = None;
                vec![]
            }
        }
    }

    fn handle_key_confirm_finish(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => self.update(Message::ConfirmFinish),
            _ => self.update(Message::CancelFinish),
        }
    }

    fn handle_key_epic_text_input(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Esc => self.update(Message::CancelInput),
            KeyCode::Enter => {
                let value = self.input.buffer.trim().to_string();
                match self.input.mode.clone() {
                    InputMode::InputEpicTitle => self.update(Message::SubmitEpicTitle(value)),
                    InputMode::InputEpicDescription => self.update(Message::SubmitEpicDescription(value)),
                    InputMode::InputEpicRepoPath => self.update(Message::SubmitEpicRepoPath(value)),
                    _ => vec![],
                }
            }
            KeyCode::Backspace => self.update(Message::InputBackspace),
            KeyCode::Char(c) => self.update(Message::InputChar(c)),
            _ => vec![],
        }
    }

    fn handle_key_confirm_delete_epic(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.input.mode = InputMode::Normal;
                self.status_message = None;
                if let Some(ColumnItem::Epic(epic)) = self.selected_column_item() {
                    let id = epic.id;
                    self.update(Message::DeleteEpic(id))
                } else {
                    vec![]
                }
            }
            _ => {
                self.input.mode = InputMode::Normal;
                self.status_message = None;
                vec![]
            }
        }
    }

    fn handle_key_confirm_archive_epic(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.input.mode = InputMode::Normal;
                self.status_message = None;
                if let Some(ColumnItem::Epic(epic)) = self.selected_column_item() {
                    let id = epic.id;
                    self.update(Message::ArchiveEpic(id))
                } else {
                    vec![]
                }
            }
            _ => {
                self.input.mode = InputMode::Normal;
                self.status_message = None;
                vec![]
            }
        }
    }

    fn handle_key_help(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Char('?') | KeyCode::Esc => self.update(Message::ToggleHelp),
            _ => vec![],
        }
    }
}
