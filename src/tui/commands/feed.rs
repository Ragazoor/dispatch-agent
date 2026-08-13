//! Feed-epic refresh side-effect commands.

use crate::models::EpicId;

/// Side-effect commands for the feed-epic refresh flow.
///
/// Wrapped by [`crate::tui::types::Command::Feed`] for runtime dispatch.
#[derive(Debug, Clone)]
pub enum FeedCommand {
    /// Run the configured shell command for a feed epic and upsert results.
    TriggerEpic {
        epic_id: EpicId,
        /// Presentation only — the status-bar lines. The feed command, role and
        /// grouping flag are read from the epic inside the cycle, never carried
        /// on this command, so a refresh cannot act on a stale board snapshot.
        epic_title: String,
    },
}
