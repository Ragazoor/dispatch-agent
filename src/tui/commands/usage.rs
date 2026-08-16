//! Usage-telemetry side-effect commands.

/// Wrapped by [`crate::tui::types::Command::Usage`] for runtime dispatch.
#[derive(Debug, Clone)]
pub enum UsageCommand {
    /// Append-only telemetry: record a feature-usage event. The runtime spawns
    /// a fire-and-forget DB write; failures are intentionally swallowed.
    Record(crate::models::UsageEvent),
}
