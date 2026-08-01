//! Confirmation dialog handlers (delete, archive, retry, done, etc).

use crossterm::event::{KeyCode, KeyEvent};

use crate::models::{DispatchMode, EpicId, TaskId};

use super::super::types::*;
use super::super::{App, PendingAction};

impl App {
    pub(in crate::tui) fn confirm_dialog(
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

    pub(in crate::tui) fn handle_key_confirm_quit(&mut self, key: KeyEvent) -> Vec<Command> {
        self.confirm_dialog(key, |s| {
            s.should_quit = true;
            s.exit_split_if_active()
        })
    }

    pub(in crate::tui) fn handle_key_confirm_delete(&mut self, key: KeyEvent) -> Vec<Command> {
        self.confirm_dialog(key, |s| {
            if s.show_archived() {
                s.confirm_delete_archived()
            } else {
                s.confirm_delete_selected()
            }
        })
    }

    pub(in crate::tui) fn confirm_delete_archived(&mut self) -> Vec<Command> {
        self.archived_tasks()
            .get(self.selected_archive_row())
            .map(|t| t.id)
            .map(|id| self.update(Message::Task(crate::tui::messages::TaskMessage::Delete(id))))
            .unwrap_or_default()
    }

    pub(in crate::tui) fn confirm_delete_selected(&mut self) -> Vec<Command> {
        self.selected_task()
            .map(|t| t.id)
            .map(|id| self.update(Message::Task(crate::tui::messages::TaskMessage::Delete(id))))
            .unwrap_or_default()
    }

    pub(in crate::tui) fn handle_key_confirm_retry(
        &mut self,
        key: KeyEvent,
        id: TaskId,
    ) -> Vec<Command> {
        match key.code {
            KeyCode::Char('r') => self.update(Message::Task(
                crate::tui::messages::TaskMessage::RetryResume(id),
            )),
            KeyCode::Char('f') => self.update(Message::Task(
                crate::tui::messages::TaskMessage::RetryFresh(id),
            )),
            KeyCode::Esc => self.update(Message::Input(
                crate::tui::messages::InputMessage::CancelRetry,
            )),
            _ => vec![],
        }
    }

    pub(in crate::tui) fn handle_key_confirm_archive(
        &mut self,
        key: KeyEvent,
        task_id: Option<TaskId>,
    ) -> Vec<Command> {
        self.confirm_dialog(key, |s| {
            if s.has_selection() {
                let mut cmds = Vec::new();
                if !s.select.tasks.is_empty() {
                    let ids: Vec<_> = s.select.tasks.iter().copied().collect();
                    cmds.extend(s.update(Message::Task(
                        crate::tui::messages::TaskMessage::BatchArchive(ids),
                    )));
                }
                if !s.select.epics.is_empty() {
                    let ids: Vec<_> = s.select.epics.iter().copied().collect();
                    cmds.extend(s.update(Message::Epic(
                        crate::tui::messages::EpicMessage::BatchArchive(ids),
                    )));
                }
                cmds
            } else if let Some(id) = task_id {
                s.update(Message::Task(crate::tui::messages::TaskMessage::Archive(
                    id,
                )))
            } else {
                vec![]
            }
        })
    }

    pub(in crate::tui) fn handle_key_confirm_done(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => self.update(Message::Input(
                crate::tui::messages::InputMessage::ConfirmDone,
            )),
            _ => self.update(Message::Input(
                crate::tui::messages::InputMessage::CancelDone,
            )),
        }
    }

    pub(in crate::tui) fn handle_key_confirm_delete_epic(&mut self, key: KeyEvent) -> Vec<Command> {
        self.confirm_dialog(key, |s| {
            if let Some(id) = s.selected_epic_id() {
                s.update(Message::Epic(crate::tui::messages::EpicMessage::Delete(id)))
            } else {
                vec![]
            }
        })
    }

    pub(in crate::tui) fn handle_key_confirm_archive_epic(
        &mut self,
        key: KeyEvent,
    ) -> Vec<Command> {
        self.confirm_dialog(key, |s| {
            if let Some(id) = s.selected_epic_id() {
                s.update(Message::Epic(crate::tui::messages::EpicMessage::Archive(
                    id,
                )))
            } else {
                vec![]
            }
        })
    }

    pub(in crate::tui) fn handle_key_confirm_detach_tmux(&mut self, key: KeyEvent) -> Vec<Command> {
        let ids = match &self.input.mode {
            InputMode::ConfirmDetachTmux(ids) => ids.clone(),
            _ => return vec![],
        };
        self.confirm_dialog(key, |s| s.detach_tmux_panels(ids))
    }

    pub(in crate::tui) fn handle_key_confirm_delete_todo(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Enter => {
                self.input.mode = InputMode::Normal;
                match std::mem::take(&mut self.interaction.pending) {
                    PendingAction::TodoDelete(id) => {
                        self.update(Message::Todo(crate::tui::messages::TodoMessage::Delete(id)))
                    }
                    _ => vec![],
                }
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                self.input.mode = InputMode::Normal;
                self.interaction.pending = PendingAction::None;
                vec![]
            }
            _ => vec![],
        }
    }

    pub(in crate::tui) fn handle_key_confirm_trust_repo(
        &mut self,
        key: KeyEvent,
        task_id: TaskId,
        mode: DispatchMode,
    ) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.clear_status();
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => self.update(Message::Task(
                crate::tui::messages::TaskMessage::TrustAndDispatch { id: task_id, mode },
            )),
            _ => vec![],
        }
    }

    /// The sync confirmation (docs/specs/repo-sync.allium: surface
    /// RepoSyncConfirmation). Nothing is fetched, merged or pushed until it is
    /// confirmed; dismissing leaves the repository untouched.
    pub(in crate::tui) fn handle_key_confirm_repo_sync(
        &mut self,
        key: KeyEvent,
        repo_path: String,
    ) -> Vec<Command> {
        self.confirm_dialog(key, |s| s.confirm_repo_sync(&repo_path))
    }

    pub(in crate::tui) fn handle_key_confirm_trust_repo_quick_dispatch(
        &mut self,
        key: KeyEvent,
        draft: TaskDraft,
        epic_id: Option<EpicId>,
    ) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.clear_status();
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                vec![Command::Task(
                    crate::tui::commands::TaskCommand::TrustAndQuickDispatch { draft, epic_id },
                )]
            }
            _ => vec![],
        }
    }
}
