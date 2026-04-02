use crossterm::event::{KeyCode, KeyEvent};

use super::{App, ColumnItem, Command, InputMode, Message, MoveDirection, ReviewAgentRequest, ViewMode};
use crate::models::{DispatchMode, ReviewDecision, SubStatus, TaskId, TaskStatus, TaskTag};

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
            | InputMode::InputRepoPath
            | InputMode::InputEpicTitle
            | InputMode::InputEpicDescription
            | InputMode::InputEpicRepoPath => self.handle_key_text_input(key),
            InputMode::ConfirmDelete => self.handle_key_confirm_delete(key),
            InputMode::InputTag => self.handle_key_tag(key),
            InputMode::QuickDispatch => self.handle_key_quick_dispatch(key),
            InputMode::ConfirmRetry(id) => self.handle_key_confirm_retry(key, id),
            InputMode::ConfirmArchive => self.handle_key_confirm_archive(key),
            InputMode::ConfirmDeleteEpic => self.handle_key_confirm_delete_epic(key),
            InputMode::ConfirmArchiveEpic => self.handle_key_confirm_archive_epic(key),

            InputMode::ConfirmDone(_) => self.handle_key_confirm_done(key),
            InputMode::ConfirmWrapUp(_) => self.handle_key_confirm_wrap_up(key),
            InputMode::ConfirmEpicWrapUp(_) => self.handle_key_confirm_epic_wrap_up(key),
            InputMode::ConfirmDetachTmux(_) => self.handle_key_confirm_detach_tmux(key),
            InputMode::Help => self.handle_key_help(key),
            InputMode::RepoFilter => self.handle_key_repo_filter(key),
            InputMode::ReviewRepoFilter => self.handle_key_review_repo_filter(key),
            InputMode::InputPresetName => self.handle_key_input_preset_name(key),
            InputMode::ConfirmDeletePreset => self.handle_key_confirm_delete_preset(key),
        }
    }

    fn handle_key_normal(&mut self, key: KeyEvent) -> Vec<Command> {
        if self.archive.visible {
            return self.handle_key_archive(key);
        }

        if matches!(self.view_mode, ViewMode::ReviewBoard { .. }) {
            return self.handle_key_review_board(key);
        }

        match key.code {
            KeyCode::Tab => {
                self.update(Message::SwitchToReviewBoard)
            }

            KeyCode::Char('q') => {
                if matches!(self.view_mode, ViewMode::Epic { .. }) {
                    self.update(Message::ExitEpic)
                } else {
                    self.update(Message::Quit)
                }
            }

            KeyCode::Char('h') | KeyCode::Left => self.update(Message::NavigateColumn(-1)),
            KeyCode::Char('l') | KeyCode::Right => self.update(Message::NavigateColumn(1)),
            KeyCode::Char('j') | KeyCode::Down => self.update(Message::NavigateRow(1)),
            KeyCode::Char('k') | KeyCode::Up => self.update(Message::NavigateRow(-1)),
            KeyCode::Char('J') => self.update(Message::ReorderItem(1)),
            KeyCode::Char('K') => self.update(Message::ReorderItem(-1)),

            KeyCode::Char('n') => self.update(Message::StartNewTask),
            KeyCode::Char('N') => self.update(Message::ToggleNotifications),
            KeyCode::Char('E') => self.update(Message::StartNewEpic),
            KeyCode::Char('d') => self.handle_key_dispatch(),
            KeyCode::Char('f') => self.update(Message::StartRepoFilter),
            KeyCode::Char('W') => {
                match self.selected_column_item() {
                    Some(ColumnItem::Task(task)) => {
                        let id = task.id;
                        self.update(Message::StartWrapUp(id))
                    }
                    Some(ColumnItem::Epic(epic)) => {
                        let id = epic.id;
                        self.update(Message::StartEpicWrapUp(id))
                    }
                    None => vec![],
                }
            }
            KeyCode::Char('m') => {
                if let Some(ColumnItem::Epic(epic)) = self.selected_column_item() {
                    let id = epic.id;
                    return self.update(Message::MoveEpicStatus(id, MoveDirection::Forward));
                }
                self.handle_key_move(MoveDirection::Forward)
            }
            KeyCode::Char('M') => {
                if let Some(ColumnItem::Epic(epic)) = self.selected_column_item() {
                    let id = epic.id;
                    return self.update(Message::MoveEpicStatus(id, MoveDirection::Backward));
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
                } else if let Some(ColumnItem::Epic(epic)) = self.selected_column_item() {
                    // Prefer Running subtasks, then Review, by sort_order
                    let window = self.tasks.iter()
                        .filter(|t| {
                            t.epic_id == Some(epic.id)
                                && t.status == TaskStatus::Running
                                && t.tmux_window.is_some()
                        })
                        .min_by_key(|t| (t.sort_order.unwrap_or(t.id.0), t.id.0))
                        .or_else(|| {
                            self.tasks.iter()
                                .filter(|t| {
                                    t.epic_id == Some(epic.id)
                                        && t.status == TaskStatus::Review
                                        && t.tmux_window.is_some()
                                })
                                .min_by_key(|t| (t.sort_order.unwrap_or(t.id.0), t.id.0))
                        })
                        .and_then(|t| t.tmux_window.clone());

                    if let Some(window) = window {
                        vec![Command::JumpToTmux { window }]
                    } else {
                        // Fallback: enter epic view
                        let id = epic.id;
                        self.update(Message::EnterEpic(id))
                    }
                } else {
                    vec![]
                }
            }

            KeyCode::Char('p') => {
                if let Some(task) = self.selected_task() {
                    if let Some(url) = &task.pr_url {
                        vec![Command::OpenInBrowser { url: url.clone() }]
                    } else {
                        self.update(Message::StatusInfo("No PR URL".to_string()))
                    }
                } else {
                    vec![]
                }
            }

            KeyCode::Char('a') => self.update(Message::SelectAllColumn),

            KeyCode::Char(' ') => {
                match self.selected_column_item() {
                    Some(ColumnItem::Task(task)) => {
                        let id = task.id;
                        self.update(Message::ToggleSelect(id))
                    }
                    Some(ColumnItem::Epic(epic)) => {
                        let id = epic.id;
                        self.update(Message::ToggleSelectEpic(id))
                    }
                    None => vec![],
                }
            }

            KeyCode::Enter => {
                if self.selection().on_select_all {
                    return self.update(Message::SelectAllColumn);
                }
                self.update(Message::ToggleDetail)
            }

            KeyCode::Char('e') => {
                match self.selected_column_item() {
                    Some(ColumnItem::Task(task)) => vec![Command::EditTaskInEditor(task.clone())],
                    Some(ColumnItem::Epic(epic)) => {
                        let id = epic.id;
                        self.update(Message::EnterEpic(id))
                    }
                    None => {
                        if let ViewMode::Epic { epic_id, .. } = &self.view_mode {
                            let id = *epic_id;
                            self.update(Message::EditEpic(id))
                        } else {
                            vec![]
                        }
                    }
                }
            }

            KeyCode::Char('x') => {
                if self.has_selection() {
                    let count = self.selected_tasks.len() + self.selected_epics.len();
                    self.input.mode = InputMode::ConfirmArchive;
                    self.set_status(format!("Archive {} items? (y/n)", count));
                    vec![]
                } else {
                    match self.selected_column_item() {
                        Some(ColumnItem::Epic(_)) => {
                            self.update(Message::ConfirmArchiveEpic)
                        }
                        _ => {
                            if self.selected_task().is_some() {
                                self.input.mode = InputMode::ConfirmArchive;
                                self.set_status("Archive task? (y/n)".to_string());
                            }
                            vec![]
                        }
                    }
                }
            }

            KeyCode::Char('D') => {
                // In epic view, quick-dispatch a subtask for the current epic
                if let ViewMode::Epic { epic_id, .. } = &self.view_mode {
                    let eid = *epic_id;
                    if let Some(epic) = self.epics.iter().find(|e| e.id == eid) {
                        let repo_path = epic.repo_path.clone();
                        return self.update(Message::QuickDispatch { repo_path, epic_id: Some(eid) });
                    }
                }
                match self.repo_paths.len() {
                    0 => self.update(Message::StatusInfo(
                        "No saved repo paths — create a task first".to_string(),
                    )),
                    1 => {
                        let repo_path = self.repo_paths[0].clone();
                        self.update(Message::QuickDispatch { repo_path, epic_id: None })
                    }
                    _ => self.update(Message::StartQuickDispatchSelection),
                }
            }

            KeyCode::Char('H') => self.update(Message::ToggleArchive),

            KeyCode::Char('?') => self.update(Message::ToggleHelp),

            KeyCode::Char('T') => {
                if !self.selected_tasks.is_empty() {
                    let ids: Vec<_> = self.selected_tasks.iter().copied().collect();
                    self.update(Message::BatchDetachTmux(ids))
                } else if let Some(task) = self.selected_task() {
                    if task.status == TaskStatus::Review && task.tmux_window.is_some() {
                        let id = task.id;
                        self.update(Message::DetachTmux(id))
                    } else {
                        vec![]
                    }
                } else {
                    vec![]
                }
            }

            KeyCode::Esc => {
                if matches!(self.view_mode, ViewMode::Epic { .. }) {
                    self.update(Message::ExitEpic)
                } else if self.has_selection() || self.selection().on_select_all {
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
                *self.archive.list_state.selected_mut() = Some(self.archive.selected_row);
                vec![]
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.archive.selected_row = self.archive.selected_row.saturating_sub(1);
                *self.archive.list_state.selected_mut() = Some(self.archive.selected_row);
                vec![]
            }
            KeyCode::Char('H') | KeyCode::Esc => self.update(Message::ToggleArchive),
            KeyCode::Char('x') => {
                let archived = self.archived_tasks();
                if let Some(task) = archived.get(self.archive.selected_row) {
                    let title = super::truncate_title(&task.title, 30);
                    self.input.mode = InputMode::ConfirmDelete;
                    self.set_status(format!("Delete {title}? (y/n)"));
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

    /// Handle the 'd' key: dispatch, brainstorm, resume, or retry depending on item type/status.
    fn handle_key_dispatch(&mut self) -> Vec<Command> {
        match self.selected_column_item() {
            Some(ColumnItem::Epic(epic)) => {
                let id = epic.id;
                self.update(Message::DispatchEpic(id))
            }
            Some(ColumnItem::Task(task)) => {
                let id = task.id;
                let status = task.status;
                let has_window = task.tmux_window.is_some();
                let has_worktree = task.worktree.is_some();
                let is_problematic = self.find_task(id).is_some_and(|t| {
                    t.sub_status == SubStatus::Stale || t.sub_status == SubStatus::Crashed
                });

                match status {
                    TaskStatus::Backlog => {
                        let msg = match DispatchMode::for_task(task) {
                            DispatchMode::Dispatch => Message::DispatchTask(id),
                            DispatchMode::Brainstorm => Message::BrainstormTask(id),
                            DispatchMode::Plan => Message::PlanTask(id),
                        };
                        self.update(msg)
                    }
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
                                "No worktree to resume, move to Backlog and re-dispatch".to_string(),
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
            None => {
                if let ViewMode::Epic { epic_id, .. } = self.view_mode {
                    self.update(Message::DispatchEpic(epic_id))
                } else {
                    vec![]
                }
            }
        }
    }

    /// Handle 'm'/'M' key: move selected task(s) forward or backward.
    fn handle_key_move(&mut self, direction: MoveDirection) -> Vec<Command> {
        if self.has_selection() {
            if self.selected_tasks.is_empty() {
                // Only epics selected — can't move since status is derived
                return self.update(Message::StatusInfo(
                    "Epic status is derived from subtasks".to_string(),
                ));
            }
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
        // In repo path modes, j/k navigate saved repo paths when the buffer is empty
        let is_repo_mode = matches!(
            self.input.mode,
            InputMode::InputRepoPath | InputMode::InputEpicRepoPath
        );
        if is_repo_mode && self.input.buffer.is_empty() {
            match key.code {
                KeyCode::Char('j') | KeyCode::Down => return self.update(Message::MoveRepoCursor(1)),
                KeyCode::Char('k') | KeyCode::Up => return self.update(Message::MoveRepoCursor(-1)),
                _ => {}
            }
        }
        match key.code {
            KeyCode::Esc => self.update(Message::CancelInput),
            KeyCode::Enter => {
                // In repo path modes with empty buffer, Enter selects the cursor repo
                if is_repo_mode && self.input.buffer.is_empty() {
                    let idx = self.input.repo_cursor;
                    if let Some(path) = self.repo_paths.get(idx) {
                        let msg = match self.input.mode {
                            InputMode::InputEpicRepoPath => Message::SubmitEpicRepoPath(path.clone()),
                            _ => Message::SubmitRepoPath(path.clone()),
                        };
                        return self.update(msg);
                    }
                }
                let value = self.input.buffer.trim().to_string();
                match self.input.mode.clone() {
                    InputMode::InputTitle => self.update(Message::SubmitTitle(value)),
                    InputMode::InputDescription => self.update(Message::SubmitDescription(value)),
                    InputMode::InputRepoPath => self.update(Message::SubmitRepoPath(value)),
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

    fn handle_key_tag(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Char('b') => self.update(Message::SubmitTag(Some(TaskTag::Bug))),
            KeyCode::Char('f') => self.update(Message::SubmitTag(Some(TaskTag::Feature))),
            KeyCode::Char('c') => self.update(Message::SubmitTag(Some(TaskTag::Chore))),
            KeyCode::Char('e') => self.update(Message::SubmitTag(Some(TaskTag::Epic))),
            KeyCode::Enter => self.update(Message::SubmitTag(None)),
            KeyCode::Esc => self.update(Message::CancelInput),
            _ => vec![],
        }
    }

    /// Generic y/n confirm dialog: on y/Y resets mode, clears status, and runs `on_confirm`;
    /// on any other key just resets mode and clears status.
    fn confirm_dialog(
        &mut self,
        key: KeyEvent,
        on_confirm: impl FnOnce(&mut Self) -> Vec<Command>,
    ) -> Vec<Command> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.input.mode = InputMode::Normal;
                self.clear_status();
                on_confirm(self)
            }
            _ => {
                self.input.mode = InputMode::Normal;
                self.clear_status();
                vec![]
            }
        }
    }

    fn handle_key_confirm_delete(&mut self, key: KeyEvent) -> Vec<Command> {
        self.confirm_dialog(key, |s| {
            if s.archive.visible {
                s.confirm_delete_archived()
            } else {
                s.confirm_delete_selected()
            }
        })
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
            KeyCode::Char('j') | KeyCode::Down => self.update(Message::MoveRepoCursor(1)),
            KeyCode::Char('k') | KeyCode::Up => self.update(Message::MoveRepoCursor(-1)),
            KeyCode::Enter => {
                let idx = self.input.repo_cursor;
                self.update(Message::SelectQuickDispatchRepo(idx))
            }
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
        self.confirm_dialog(key, |s| {
            if s.has_selection() {
                let mut cmds = Vec::new();
                if !s.selected_tasks.is_empty() {
                    let ids: Vec<_> = s.selected_tasks.iter().copied().collect();
                    cmds.extend(s.update(Message::BatchArchiveTasks(ids)));
                }
                if !s.selected_epics.is_empty() {
                    let ids: Vec<_> = s.selected_epics.iter().copied().collect();
                    cmds.extend(s.update(Message::BatchArchiveEpics(ids)));
                }
                cmds
            } else if let Some(task) = s.selected_task() {
                let id = task.id;
                s.update(Message::ArchiveTask(id))
            } else {
                vec![]
            }
        })
    }

    fn handle_key_confirm_done(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => self.update(Message::ConfirmDone),
            _ => self.update(Message::CancelDone),
        }
    }

    fn handle_key_confirm_delete_epic(&mut self, key: KeyEvent) -> Vec<Command> {
        self.confirm_dialog(key, |s| {
            if let Some(ColumnItem::Epic(epic)) = s.selected_column_item() {
                let id = epic.id;
                s.update(Message::DeleteEpic(id))
            } else {
                vec![]
            }
        })
    }

    fn handle_key_confirm_archive_epic(&mut self, key: KeyEvent) -> Vec<Command> {
        self.confirm_dialog(key, |s| {
            if let Some(ColumnItem::Epic(epic)) = s.selected_column_item() {
                let id = epic.id;
                s.update(Message::ArchiveEpic(id))
            } else {
                vec![]
            }
        })
    }

    fn handle_key_help(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Char('?') | KeyCode::Esc => self.update(Message::ToggleHelp),
            _ => vec![],
        }
    }

    fn handle_key_repo_filter(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q') => {
                self.update(Message::CloseRepoFilter)
            }
            KeyCode::Tab => self.update(Message::ToggleRepoFilterMode),
            KeyCode::Char('a') => self.update(Message::ToggleAllRepoFilter),
            KeyCode::Char('j') | KeyCode::Down => self.update(Message::MoveRepoCursor(1)),
            KeyCode::Char('k') | KeyCode::Up => self.update(Message::MoveRepoCursor(-1)),
            KeyCode::Char(' ') => {
                let idx = self.input.repo_cursor;
                if idx < self.repo_paths.len() {
                    let path = self.repo_paths[idx].clone();
                    self.update(Message::ToggleRepoFilter(path))
                } else {
                    vec![]
                }
            }
            KeyCode::Char(c @ '1'..='9') => {
                let idx = (c as usize) - ('1' as usize);
                if idx < self.repo_paths.len() {
                    let path = self.repo_paths[idx].clone();
                    self.update(Message::ToggleRepoFilter(path))
                } else {
                    vec![]
                }
            }
            KeyCode::Char('s') => self.update(Message::StartSavePreset),
            KeyCode::Char('x') => self.update(Message::StartDeletePreset),
            KeyCode::Char(c @ 'A'..='Z') => {
                let idx = (c as usize) - ('A' as usize);
                if idx < self.filter_presets.len() {
                    let name = self.filter_presets[idx].0.clone();
                    self.update(Message::LoadFilterPreset(name))
                } else {
                    vec![]
                }
            }
            _ => vec![],
        }
    }

    fn handle_key_review_repo_filter(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Enter | KeyCode::Esc => self.update(Message::CloseReviewRepoFilter),
            KeyCode::Tab => self.update(Message::ToggleReviewRepoFilterMode),
            KeyCode::Char('a') => self.update(Message::ToggleAllReviewRepoFilter),
            KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
                let idx = (c as usize) - ('1' as usize);
                let repos: Vec<String> = self.review.prs.iter()
                    .map(|pr| pr.repo.clone())
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect();
                if let Some(repo) = repos.get(idx) {
                    self.update(Message::ToggleReviewRepoFilter(repo.clone()))
                } else {
                    vec![]
                }
            }
            _ => vec![],
        }
    }

    fn handle_key_input_preset_name(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Enter => {
                let name = self.input.buffer.clone();
                self.update(Message::SaveFilterPreset(name))
            }
            KeyCode::Esc => self.update(Message::CancelPresetInput),
            KeyCode::Backspace => self.update(Message::InputBackspace),
            KeyCode::Char(c) => self.update(Message::InputChar(c)),
            _ => vec![],
        }
    }

    fn handle_key_confirm_delete_preset(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Char(c @ 'A'..='Z') => {
                let idx = (c as usize) - ('A' as usize);
                if idx < self.filter_presets.len() {
                    let name = self.filter_presets[idx].0.clone();
                    self.update(Message::DeleteFilterPreset(name))
                } else {
                    vec![]
                }
            }
            KeyCode::Esc => self.update(Message::CancelPresetInput),
            _ => vec![],
        }
    }

    fn handle_key_confirm_detach_tmux(&mut self, key: KeyEvent) -> Vec<Command> {
        let ids = match &self.input.mode {
            InputMode::ConfirmDetachTmux(ids) => ids.clone(),
            _ => return vec![],
        };
        self.confirm_dialog(key, |s| s.detach_tmux_panels(ids))
    }

    fn handle_key_confirm_wrap_up(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Char('r') => self.update(Message::WrapUpRebase),
            KeyCode::Char('p') => self.update(Message::WrapUpPr),
            KeyCode::Esc => self.update(Message::CancelWrapUp),
            _ => vec![],
        }
    }

    fn handle_key_confirm_epic_wrap_up(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Char('r') => self.update(Message::EpicWrapUpRebase),
            KeyCode::Char('p') => self.update(Message::EpicWrapUpPr),
            KeyCode::Esc => self.update(Message::CancelEpicWrapUp),
            _ => vec![],
        }
    }

    fn handle_key_review_board(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Char('q') => self.update(Message::Quit),
            KeyCode::Tab | KeyCode::Esc => self.update(Message::SwitchToTaskBoard),

            KeyCode::Char('h') | KeyCode::Left => {
                if let Some(sel) = self.review_selection_mut() {
                    let col = sel.selected_column;
                    sel.selected_column = col.saturating_sub(1);
                }
                vec![]
            }
            KeyCode::Char('l') | KeyCode::Right => {
                if let Some(sel) = self.review_selection_mut() {
                    let col = sel.selected_column;
                    sel.selected_column = (col + 1).min(ReviewDecision::COLUMN_COUNT - 1);
                }
                vec![]
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.navigate_review_row(1);
                vec![]
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.navigate_review_row(-1);
                vec![]
            }

            KeyCode::Enter => self.update(Message::ToggleReviewDetail),

            KeyCode::Char('p') => {
                if let Some(pr) = self.selected_review_pr() {
                    let url = pr.url.clone();
                    vec![Command::OpenInBrowser { url }]
                } else {
                    vec![]
                }
            }

            KeyCode::Char('r') => self.update(Message::RefreshReviewPrs),
            KeyCode::Char('f') => self.update(Message::StartReviewRepoFilter),

            KeyCode::Char('d') => {
                if let Some(pr) = self.selected_review_pr() {
                    self.update(Message::DispatchReviewAgent(ReviewAgentRequest {
                        repo: pr.repo.clone(),
                        number: pr.number,
                        title: pr.title.clone(),
                        body: pr.body.clone(),
                        head_ref: pr.head_ref.clone(),
                    }))
                } else {
                    vec![]
                }
            }

            KeyCode::Char('?') => self.update(Message::ToggleHelp),

            KeyCode::BackTab => self.update(Message::ToggleReviewBoardMode),

            _ => vec![],
        }
    }
}
