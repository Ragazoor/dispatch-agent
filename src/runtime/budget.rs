//! Budget-snapshot refresh executor.

use crate::models::budget::BudgetSnapshot;
use crate::tui::types::Message;

impl super::TuiRuntime {
    /// Read the budget snapshot file off the event loop and report it via
    /// `BudgetMessage::Updated`. Drives the top-row budget indicator
    /// (docs/specs/dispatch.allium: TokenBudgetIndicator).
    ///
    /// `std::fs` is forbidden in async handlers (docs/conventions.md), hence
    /// `spawn_blocking`.
    pub(super) fn exec_refresh_budget(&self) -> tokio::task::JoinHandle<()> {
        let tx = self.msg_tx.clone();
        let path = self.budget_snapshot_path.clone();
        tokio::task::spawn_blocking(move || {
            let snapshot = std::fs::read_to_string(&path)
                .ok()
                .and_then(|text| serde_json::from_str::<BudgetSnapshot>(&text).ok());
            let _ = tx.send(Message::Budget(
                crate::tui::messages::BudgetMessage::Updated(snapshot),
            ));
        })
    }
}
