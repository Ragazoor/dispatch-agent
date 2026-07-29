//! Confirmation dialog handlers (delete, archive, retry, done, merge, wrap-up, etc).

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

    pub(in crate::tui) fn handle_key_confirm_wrap_up(&mut self, key: KeyEvent) -> Vec<Command> {
        match key.code {
            KeyCode::Char('r') => {
                self.update(Message::WrapUp(crate::tui::messages::WrapUpMessage::Rebase))
            }
            KeyCode::Char('d') => {
                self.update(Message::WrapUp(crate::tui::messages::WrapUpMessage::Done))
            }
            KeyCode::Char('p') => {
                // PR creation moved to the agent /wrap-up skill so the
                // body actually reflects the diff rather than the stale
                // task description. Exit the prompt and point the user
                // at the skill.
                self.input.mode = InputMode::Normal;
                self.set_status(
                    "PR creation is agent-driven \
\u{2014} run the /wrap-up skill from the agent session"
                        .to_string(),
                );
                vec![]
            }
            KeyCode::Esc => {
                self.update(Message::WrapUp(crate::tui::messages::WrapUpMessage::Cancel))
            }
            _ => vec![],
        }
    }

    pub(in crate::tui) fn handle_key_confirm_epic_wrap_up(
        &mut self,
        key: KeyEvent,
    ) -> Vec<Command> {
        match key.code {
            KeyCode::Char('r') => self.update(Message::WrapUp(
                crate::tui::messages::WrapUpMessage::EpicRebase,
            )),
            KeyCode::Char('p') => {
                // Epic-merge batched PR creation had the same defect as
                // W+p (auto-generated bodies). Removed; the user can
                // PR each subtask via its own agent /wrap-up.
                self.input.mode = InputMode::Normal;
                self.set_status(
                    "Epic batch PR removed \
\u{2014} PR each subtask via its agent /wrap-up skill"
                        .to_string(),
                );
                vec![]
            }
            KeyCode::Esc => self.update(Message::WrapUp(
                crate::tui::messages::WrapUpMessage::EpicCancel,
            )),
            _ => vec![],
        }
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
