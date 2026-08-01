//! Budget-indicator update handlers.

use crate::models::budget::BudgetSnapshot;
use crate::tui::types::Command;
use crate::tui::App;

impl App {
    /// Record the latest budget snapshot. Marks the board dirty only when the
    /// value changed, so a no-op refresh forces no redraw (see
    /// docs/specs/dispatch.allium: TokenBudgetIndicator).
    pub(in crate::tui) fn handle_budget_updated(
        &mut self,
        snapshot: Option<BudgetSnapshot>,
    ) -> Vec<Command> {
        if self.budget != snapshot {
            self.budget = snapshot;
            // Invisible to the discriminant-based dirty detector in handle_key,
            // so mark dirty directly — but only on a real change.
            self.dirty = true;
        }
        vec![]
    }
}
