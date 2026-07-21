//! Managed-feed config popup handlers.
//!
//! The popup edits the four managed-feed settings. Opening copies the current
//! persisted snapshot into an edit buffer; saving validates the intervals,
//! emits a persist command and a provision-and-refresh command, and updates the
//! in-memory snapshot. Clearing a command does NOT tear down an already
//! provisioned subtree — that matches the `ProvisionManagedEpics` spec
//! (`docs/specs/epics.allium`); teardown is a separate explicit user action.

use super::super::messages::ManagedFeedField;
use super::super::types::*;
use super::super::App;

impl App {
    pub(in crate::tui) fn handle_open_managed_feed_config(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::ManagedFeedConfig;
        self.interaction.managed_feed_config = Some(ManagedFeedConfigState::from_settings(
            &self.managed_feed_settings,
        ));
        vec![]
    }

    pub(in crate::tui) fn handle_move_managed_feed_field(&mut self, delta: isize) -> Vec<Command> {
        if let Some(state) = self.interaction.managed_feed_config.as_mut() {
            let order = ManagedFeedField::ORDER;
            let cur = order.iter().position(|f| *f == state.field).unwrap_or(0);
            let len = order.len() as isize;
            let next = (cur as isize + delta).rem_euclid(len) as usize;
            state.field = order[next];
            self.dirty = true;
        }
        vec![]
    }

    pub(in crate::tui) fn handle_managed_feed_input(&mut self, c: char) -> Vec<Command> {
        if let Some(state) = self.interaction.managed_feed_config.as_mut() {
            // Interval fields accept digits only; everything else is ignored so
            // the buffer can never hold a non-numeric interval.
            if state.field.is_interval() && !c.is_ascii_digit() {
                return vec![];
            }
            state.focused_mut().push(c);
            self.dirty = true;
        }
        vec![]
    }

    pub(in crate::tui) fn handle_managed_feed_backspace(&mut self) -> Vec<Command> {
        if let Some(state) = self.interaction.managed_feed_config.as_mut() {
            state.focused_mut().pop();
            self.dirty = true;
        }
        vec![]
    }

    pub(in crate::tui) fn handle_close_managed_feed_config(&mut self, save: bool) -> Vec<Command> {
        if !save {
            self.input.mode = InputMode::Normal;
            self.interaction.managed_feed_config = None;
            return vec![];
        }

        let Some(state) = self.interaction.managed_feed_config.clone() else {
            self.input.mode = InputMode::Normal;
            return vec![];
        };

        // Validate intervals: empty = unset; otherwise a positive integer.
        let reviews_interval = match parse_interval(&state.reviews_interval) {
            Ok(v) => v,
            Err(()) => {
                self.set_status("Reviews interval must be a positive number".to_string());
                return vec![];
            }
        };
        let cve_interval = match parse_interval(&state.cve_interval) {
            Ok(v) => v,
            Err(()) => {
                self.set_status("CVE interval must be a positive number".to_string());
                return vec![];
            }
        };

        let reviews_command = trim_to_option(&state.reviews_command);
        let cve_command = trim_to_option(&state.cve_command);

        // Update the in-memory snapshot so a re-open shows the saved values.
        self.managed_feed_settings = ManagedFeedSettings {
            reviews_command: reviews_command.clone(),
            reviews_interval_secs: reviews_interval,
            cve_command: cve_command.clone(),
            cve_interval_secs: cve_interval,
        };

        self.input.mode = InputMode::Normal;
        self.interaction.managed_feed_config = None;
        self.set_status("Managed feed config saved".to_string());

        vec![
            Command::ManagedFeed(crate::tui::commands::ManagedFeedCommand::PersistConfig {
                reviews_command,
                reviews_interval_secs: reviews_interval,
                cve_command,
                cve_interval_secs: cve_interval,
            }),
            Command::ManagedFeed(crate::tui::commands::ManagedFeedCommand::ProvisionAndRefresh),
        ]
    }
}

/// Parse an interval field: empty → `Ok(None)` (unset); a positive integer →
/// `Ok(Some(n))`; anything else → `Err(())`.
fn parse_interval(s: &str) -> Result<Option<i64>, ()> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    match s.parse::<i64>() {
        Ok(n) if n > 0 => Ok(Some(n)),
        _ => Err(()),
    }
}

/// Trim a command field; empty → `None` (clears the setting).
fn trim_to_option(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}
