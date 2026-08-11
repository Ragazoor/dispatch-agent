//! Confirmation dialog handlers (delete, archive, retry, done, etc).

use crossterm::event::{KeyCode, KeyEvent};

use crate::models::{DispatchMode, EpicId, TaskId};

use super::super::types::*;
use super::super::{App, PendingAction};
use super::{key_event, key_label};

impl App {
    /// Every confirmation dialog records both outcomes as an
    /// `<action>_yes` / `<action>_no` pair. Dismissing is as much a use of the
    /// dialog as confirming is — a prompt that is nearly always declined is
    /// one worth removing, and only the pair makes that visible.
    pub(in crate::tui) fn confirm_dialog(
        &mut self,
        key: KeyEvent,
        action: &str,
        on_confirm: impl FnOnce(&mut Self) -> Vec<Command>,
    ) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.clear_status();
        let label = key_label(key);
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                let mut cmds = on_confirm(self);
                cmds.push(key_event(&format!("{action}_yes"), &label));
                cmds
            }
            _ => vec![key_event(&format!("{action}_no"), &label)],
        }
    }

    pub(in crate::tui) fn handle_key_confirm_quit(&mut self, key: KeyEvent) -> Vec<Command> {
        self.confirm_dialog(key, "confirm_quit", |s| {
            s.should_quit = true;
            s.exit_split_if_active()
        })
    }

    pub(in crate::tui) fn handle_key_confirm_delete(&mut self, key: KeyEvent) -> Vec<Command> {
        self.confirm_dialog(key, "confirm_delete", |s| {
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
            KeyCode::Char('r') => self.dispatch_keyed(
                Message::Task(crate::tui::messages::TaskMessage::RetryResume(id)),
                "confirm_retry_resume",
                "r",
            ),
            KeyCode::Char('f') => self.dispatch_keyed(
                Message::Task(crate::tui::messages::TaskMessage::RetryFresh(id)),
                "confirm_retry_fresh",
                "f",
            ),
            KeyCode::Esc => self.dispatch_keyed(
                Message::Input(crate::tui::messages::InputMessage::CancelRetry),
                "confirm_retry_no",
                "Esc",
            ),
            _ => vec![],
        }
    }

    pub(in crate::tui) fn handle_key_confirm_archive(
        &mut self,
        key: KeyEvent,
        task_id: Option<TaskId>,
    ) -> Vec<Command> {
        self.confirm_dialog(key, "confirm_archive", |s| {
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
        let label = key_label(key);
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => self.dispatch_keyed(
                Message::Input(crate::tui::messages::InputMessage::ConfirmDone),
                "confirm_done_yes",
                &label,
            ),
            _ => self.dispatch_keyed(
                Message::Input(crate::tui::messages::InputMessage::CancelDone),
                "confirm_done_no",
                &label,
            ),
        }
    }

    pub(in crate::tui) fn handle_key_confirm_delete_epic(&mut self, key: KeyEvent) -> Vec<Command> {
        self.confirm_dialog(key, "confirm_delete_epic", |s| {
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
        self.confirm_dialog(key, "confirm_archive_epic", |s| {
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
        self.confirm_dialog(key, "confirm_detach_tmux", |s| s.detach_tmux_panels(ids))
    }

    pub(in crate::tui) fn handle_key_confirm_delete_todo(&mut self, key: KeyEvent) -> Vec<Command> {
        let label = key_label(key);
        match key.code {
            KeyCode::Char('y') | KeyCode::Enter => {
                self.input.mode = InputMode::Normal;
                let mut cmds = match std::mem::take(&mut self.interaction.pending) {
                    PendingAction::TodoDelete(id) => {
                        self.update(Message::Todo(crate::tui::messages::TodoMessage::Delete(id)))
                    }
                    _ => vec![],
                };
                cmds.push(key_event("confirm_delete_todo_yes", &label));
                cmds
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                self.input.mode = InputMode::Normal;
                self.interaction.pending = PendingAction::None;
                vec![key_event("confirm_delete_todo_no", &label)]
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
        let label = key_label(key);
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => self.dispatch_keyed(
                Message::Task(crate::tui::messages::TaskMessage::TrustAndDispatch {
                    id: task_id,
                    mode,
                }),
                "confirm_trust_repo_yes",
                &label,
            ),
            _ => vec![key_event("confirm_trust_repo_no", &label)],
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
        self.confirm_dialog(key, "confirm_repo_sync", |s| {
            s.confirm_repo_sync(&repo_path)
        })
    }

    pub(in crate::tui) fn handle_key_confirm_trust_repo_quick_dispatch(
        &mut self,
        key: KeyEvent,
        draft: TaskDraft,
        epic_id: Option<EpicId>,
    ) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.clear_status();
        let label = key_label(key);
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                vec![
                    Command::Task(crate::tui::commands::TaskCommand::TrustAndQuickDispatch {
                        draft,
                        epic_id,
                    }),
                    key_event("confirm_trust_repo_quick_dispatch_yes", &label),
                ]
            }
            _ => vec![key_event("confirm_trust_repo_quick_dispatch_no", &label)],
        }
    }
}
