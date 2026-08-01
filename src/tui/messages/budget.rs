//! Budget-indicator messages.

use crate::models::budget::BudgetSnapshot;
use crate::tui::types::Command;
use crate::tui::App;

/// Wrapped by [`crate::tui::types::Message::Budget`] for dispatch.
#[derive(Debug, Clone)]
pub enum BudgetMessage {
    /// Result of a snapshot read. `None` when the file is absent or unreadable.
    Updated(Option<BudgetSnapshot>),
}

impl BudgetMessage {
    pub(in crate::tui) fn route(self, app: &mut App) -> Vec<Command> {
        match self {
            BudgetMessage::Updated(snapshot) => app.handle_budget_updated(snapshot),
        }
    }
}
