//! Budget-indicator side-effect commands.

/// Wrapped by [`crate::tui::types::Command::Budget`] for runtime dispatch.
#[derive(Debug, Clone)]
pub enum BudgetCommand {
    /// Read the budget snapshot file off the event loop and report the result
    /// via [`crate::tui::messages::BudgetMessage::Updated`]. Emitted by the tick
    /// loop every `BUDGET_POLL_TICKS`.
    Refresh,
}
