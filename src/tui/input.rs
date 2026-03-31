use crossterm::event::{KeyCode, KeyEvent};

use super::{App, ColumnItem, Command, InputMode, Message, MoveDirection, ViewMode};
use crate::models::{ReviewDecision, TaskId, TaskStatus};

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
            InputMode::InputTag => self.handle_key_tag(key),
            InputMode::QuickDispatch => self.handle_key_quick_dispatch(key),
            InputMode::ConfirmRetry(id) => self.handle_key_confirm_retry(key, id),
            InputMode::ConfirmArchive => self.handle_key_confirm_archive(key),
            InputMode::InputEpicTitle
            | InputMode::InputEpicDescription
            | InputMode::InputEpicRepoPath => self.handle_key_epic_text_input(key),
            InputMode::ConfirmDeleteEpic => self.handle_key_confirm_delete_epic(key),
            InputMode::ConfirmArchiveEpic => self.handle_key_confirm_archive_epic(key),
            InputMode::ConfirmEpicDone(_) => self.handle_key_confirm_epic_done(key),
            InputMode::ConfirmDone(_) => self.handle_key_confirm_done(key),
            InputMode::ConfirmWrapUp(_) => self.handle_key_confirm_wrap_up(key),
            InputMode::ConfirmEpicWrapUp(_) => self.handle_key_confirm_epic_wrap_up(key),
            InputMode::ConfirmDetachTmux(_) => self.handle_key_confirm_detach_tmux(key),
            InputMode::Help => self.handle_key_help(key),
            InputMode::RepoFilter => self.handle_key_repo_filter(key),
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
                    let statuses = self.subtask_statuses(id);
                    let all_done = !statuses.is_empty()
                        && statuses.iter().all(|s| *s == TaskStatus::Done);
                    if all_done && !epic.done {
                        let title = crate::tui::truncate_title(&epic.title, 30);
                        self.input.mode = InputMode::ConfirmEpicDone(id);
                        self.set_status(format!("Move epic {title} to Done? (y/n)"));
                        return vec![];
                    }
                    return self.update(Message::StatusInfo(
                        "Epic status is derived from subtasks".to_string(),
                    ));
                }
                self.handle_key_move(MoveDirection::Forward)
            }
            KeyCode::Char('M') => {
                if let Some(ColumnItem::Epic(epic)) = self.selected_column_item() {
                    let id = epic.id;
                    if epic.done {
                        return self.update(Message::MarkEpicUndone(id));
                    }
                    return self.update(Message::StatusInfo(
                        "Epic status is derived from subtasks".to_string(),
                    ));
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
                    let review_window = self.tasks.iter()
                        .filter(|t| t.epic_id == Some(epic.id) && t.status == TaskStatus::Review)
                        .find_map(|t| t.tmux_window.clone());
                    if let Some(window) = review_window {
                        vec![Command::JumpToTmux { window }]
                    } else {
                        self.update(Message::StatusInfo("No active review session".to_string()))
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
                if let ViewMode::Epic { epic_id, .. } = &self.view_mode {
                    let id = *epic_id;
                    return self.update(Message::EditEpic(id));
                }
                match self.selected_column_item() {
                    Some(ColumnItem::Epic(epic)) => {
                        let id = epic.id;
                        self.update(Message::EnterEpic(id))
                    }
                    Some(ColumnItem::Task(task)) => vec![Command::EditTaskInEditor(task.clone())],
                    None => vec![],
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
                        let repo_path = crate::models::RepoPath(self.repo_paths[0].clone());
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
                let has_plan = task.plan.is_some();
                let tag = task.tag.as_deref();
                let is_problematic = self.agents.stale_tasks.contains(&id)
                    || self.agents.crashed_tasks.contains(&id);

                match status {
                    TaskStatus::Backlog => {
                        if has_plan {
                            self.update(Message::DispatchTask(id))
                        } else {
                            match tag {
                                Some("epic") => self.update(Message::BrainstormTask(id)),
                                Some("feature") => self.update(Message::PlanTask(id)),
                                _ => self.update(Message::DispatchTask(id)),
                            }
                        }
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

    fn handle_key_tag(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Char('b') => self.update(Message::SubmitTag(Some("bug".to_string()))),
            KeyCode::Char('f') => self.update(Message::SubmitTag(Some("feature".to_string()))),
            KeyCode::Char('c') => self.update(Message::SubmitTag(Some("chore".to_string()))),
            KeyCode::Char('e') => self.update(Message::SubmitTag(Some("epic".to_string()))),
            KeyCode::Enter => self.update(Message::SubmitTag(None)),
            KeyCode::Esc => self.update(Message::CancelInput),
            _ => vec![],
        }
    }

    fn handle_key_confirm_delete(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.input.mode = InputMode::Normal;
                self.clear_status();
                if self.archive.visible {
                    self.confirm_delete_archived()
                } else {
                    self.confirm_delete_selected()
                }
            }
            _ => {
                self.input.mode = InputMode::Normal;
                self.clear_status();
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
                self.clear_status();
                if self.has_selection() {
                    let mut cmds = Vec::new();
                    if !self.selected_tasks.is_empty() {
                        let ids: Vec<_> = self.selected_tasks.iter().copied().collect();
                        cmds.extend(self.update(Message::BatchArchiveTasks(ids)));
                    }
                    if !self.selected_epics.is_empty() {
                        let ids: Vec<_> = self.selected_epics.iter().copied().collect();
                        cmds.extend(self.update(Message::BatchArchiveEpics(ids)));
                    }
                    cmds
                } else if let Some(task) = self.selected_task() {
                    let id = task.id;
                    self.update(Message::ArchiveTask(id))
                } else {
                    vec![]
                }
            }
            _ => {
                self.input.mode = InputMode::Normal;
                self.clear_status();
                vec![]
            }
        }
    }

    fn handle_key_confirm_done(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => self.update(Message::ConfirmDone),
            _ => self.update(Message::CancelDone),
        }
    }

    fn handle_key_confirm_epic_done(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => self.update(Message::ConfirmEpicDone),
            _ => self.update(Message::CancelEpicDone),
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
                self.clear_status();
                if let Some(ColumnItem::Epic(epic)) = self.selected_column_item() {
                    let id = epic.id;
                    self.update(Message::DeleteEpic(id))
                } else {
                    vec![]
                }
            }
            _ => {
                self.input.mode = InputMode::Normal;
                self.clear_status();
                vec![]
            }
        }
    }

    fn handle_key_confirm_archive_epic(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.input.mode = InputMode::Normal;
                self.clear_status();
                if let Some(ColumnItem::Epic(epic)) = self.selected_column_item() {
                    let id = epic.id;
                    self.update(Message::ArchiveEpic(id))
                } else {
                    vec![]
                }
            }
            _ => {
                self.input.mode = InputMode::Normal;
                self.clear_status();
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

    fn handle_key_repo_filter(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Enter | KeyCode::Esc => self.update(Message::CloseRepoFilter),
            KeyCode::Char('a') => self.update(Message::ToggleAllRepoFilter),
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
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => self.update(Message::ConfirmDetachTmux),
            _ => {
                self.input.mode = InputMode::Normal;
                self.clear_status();
                vec![]
            }
        }
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

            KeyCode::Enter => {
                if let Some(pr) = self.selected_review_pr() {
                    let url = pr.url.clone();
                    vec![Command::OpenInBrowser { url }]
                } else {
                    vec![]
                }
            }

            KeyCode::Char('r') => self.update(Message::RefreshReviewPrs),

            KeyCode::Char('?') => self.update(Message::ToggleHelp),

            _ => vec![],
        }
    }
}
