//! Repo filter mode + preset/path input handlers.

use crossterm::event::{KeyCode, KeyEvent};

use super::super::types::*;
use super::super::App;
use super::{key_event, key_label};

impl App {
    pub(in crate::tui) fn handle_key_repo_filter(&mut self, key: KeyEvent) -> Vec<Command> {
        let label = key_label(key);
        match key.code {
            KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q') => self.dispatch_keyed(
                Message::RepoFilter(crate::tui::messages::RepoFilterMessage::Close),
                "repo_filter_close",
                &label,
            ),
            KeyCode::Char('a') => self.dispatch_keyed(
                Message::RepoFilter(crate::tui::messages::RepoFilterMessage::ToggleAll),
                "repo_filter_toggle_all",
                &label,
            ),
            KeyCode::Char('j') | KeyCode::Down => self.dispatch_keyed(
                Message::RepoFilter(crate::tui::messages::RepoFilterMessage::MoveCursor(1)),
                "repo_filter_move_cursor",
                &label,
            ),
            KeyCode::Char('k') | KeyCode::Up => self.dispatch_keyed(
                Message::RepoFilter(crate::tui::messages::RepoFilterMessage::MoveCursor(-1)),
                "repo_filter_move_cursor",
                &label,
            ),
            KeyCode::Char(' ') => {
                let idx = self.input.repo_cursor;
                if idx == 0 {
                    self.dispatch_keyed(
                        Message::RepoFilter(
                            crate::tui::messages::RepoFilterMessage::ToggleOnlyActive,
                        ),
                        "repo_filter_toggle_only_active",
                        &label,
                    )
                } else if idx <= self.board.repo_paths.len() {
                    let path = self.board.repo_paths[idx - 1].clone();
                    self.dispatch_keyed(
                        Message::RepoFilter(crate::tui::messages::RepoFilterMessage::Toggle(path)),
                        "repo_filter_toggle_repo",
                        &label,
                    )
                } else {
                    vec![]
                }
            }
            KeyCode::Char(c @ '1'..='9') => {
                let idx = (c as usize) - ('1' as usize);
                if idx < self.board.repo_paths.len() {
                    let path = self.board.repo_paths[idx].clone();
                    self.dispatch_keyed(
                        Message::RepoFilter(crate::tui::messages::RepoFilterMessage::Toggle(path)),
                        "repo_filter_toggle_repo",
                        &label,
                    )
                } else {
                    vec![]
                }
            }
            KeyCode::Tab => self.dispatch_keyed(
                Message::RepoFilter(crate::tui::messages::RepoFilterMessage::ToggleMode),
                "repo_filter_toggle_mode",
                &label,
            ),
            KeyCode::Backspace | KeyCode::Delete => {
                if self.input.repo_cursor > 0 {
                    self.dispatch_keyed(
                        Message::RepoFilter(
                            crate::tui::messages::RepoFilterMessage::StartDeleteRepoPath,
                        ),
                        "repo_filter_delete_repo_path",
                        &label,
                    )
                } else {
                    vec![]
                }
            }
            KeyCode::Char('s') => self.dispatch_keyed(
                Message::RepoFilter(crate::tui::messages::RepoFilterMessage::StartSavePreset),
                "repo_filter_save_preset",
                &label,
            ),
            KeyCode::Char('x') => self.dispatch_keyed(
                Message::RepoFilter(crate::tui::messages::RepoFilterMessage::StartDeletePreset),
                "repo_filter_delete_preset",
                &label,
            ),
            KeyCode::Char(c @ 'A'..='Z') => {
                let idx = (c as usize) - ('A' as usize);
                if idx < self.filter.presets.len() {
                    let name = self.filter.presets[idx].0.clone();
                    self.dispatch_keyed(
                        Message::RepoFilter(crate::tui::messages::RepoFilterMessage::LoadPreset(
                            name,
                        )),
                        "repo_filter_load_preset",
                        &label,
                    )
                } else {
                    vec![]
                }
            }
            _ => vec![],
        }
    }

    pub(in crate::tui) fn handle_key_input_preset_name(&mut self, key: KeyEvent) -> Vec<Command> {
        // As in search mode, typing the name is not itself an action: only
        // committing or abandoning the preset is recorded.
        match key.code {
            KeyCode::Enter => {
                let name = self.input.buffer.clone();
                self.dispatch_keyed(
                    Message::RepoFilter(crate::tui::messages::RepoFilterMessage::SavePreset(name)),
                    "repo_filter_save_preset_submit",
                    "Enter",
                )
            }
            KeyCode::Esc => self.dispatch_keyed(
                Message::RepoFilter(crate::tui::messages::RepoFilterMessage::CancelPresetInput),
                "repo_filter_save_preset_cancel",
                "Esc",
            ),
            KeyCode::Backspace => self.update(Message::Input(
                crate::tui::messages::InputMessage::InputBackspace,
            )),
            KeyCode::Char(c) if !key.modifiers.contains(crossterm::event::KeyModifiers::ALT) => {
                self.update(Message::Input(
                    crate::tui::messages::InputMessage::InputChar(c),
                ))
            }
            _ => match super::text_edit_message(key) {
                Some(msg) => self.update(Message::Input(msg)),
                None => vec![],
            },
        }
    }

    pub(in crate::tui) fn handle_key_confirm_delete_preset(
        &mut self,
        key: KeyEvent,
    ) -> Vec<Command> {
        let label = key_label(key);
        match key.code {
            KeyCode::Char(c @ 'A'..='Z') => {
                let idx = (c as usize) - ('A' as usize);
                if idx < self.filter.presets.len() {
                    let name = self.filter.presets[idx].0.clone();
                    self.dispatch_keyed(
                        Message::RepoFilter(crate::tui::messages::RepoFilterMessage::DeletePreset(
                            name,
                        )),
                        "confirm_delete_preset_yes",
                        &label,
                    )
                } else {
                    vec![]
                }
            }
            KeyCode::Esc => self.dispatch_keyed(
                Message::RepoFilter(crate::tui::messages::RepoFilterMessage::CancelPresetInput),
                "confirm_delete_preset_no",
                &label,
            ),
            _ => vec![],
        }
    }

    pub(in crate::tui) fn handle_key_confirm_delete_repo_path(
        &mut self,
        key: KeyEvent,
    ) -> Vec<Command> {
        let label = key_label(key);
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                let idx = self.input.repo_cursor;
                if idx > 0 && idx <= self.board.repo_paths.len() {
                    let path = self.board.repo_paths[idx - 1].clone();
                    self.dispatch_keyed(
                        Message::RepoFilter(
                            crate::tui::messages::RepoFilterMessage::DeleteRepoPath(path),
                        ),
                        "confirm_delete_repo_path_yes",
                        &label,
                    )
                } else {
                    // Cursor no longer points at a deletable row — the prompt
                    // just closes, which is the same outcome as declining.
                    self.input.mode = InputMode::RepoFilter;
                    vec![key_event("confirm_delete_repo_path_no", &label)]
                }
            }
            _ => {
                self.input.mode = InputMode::RepoFilter;
                vec![key_event("confirm_delete_repo_path_no", &label)]
            }
        }
    }
}
