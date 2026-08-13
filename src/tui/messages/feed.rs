//! Feed-epic refresh messages.

use crate::models::EpicId;

use crate::tui::types::Command;
use crate::tui::App;

/// Messages produced by the feed-epic refresh flow.
///
/// Wrapped by [`crate::tui::types::Message::Feed`] for dispatch.
#[derive(Debug, Clone)]
pub enum FeedMessage {
    /// User-triggered refresh of a feed epic.
    TriggerEpic(EpicId),
    /// Feed refresh succeeded with at least one item, or a genuinely empty,
    /// clean (no-stderr) zero-item run — see feeds.allium
    /// DegradedEmptyEmission for the zero-item-with-stderr case, which fails
    /// instead of reaching this variant.
    Refreshed { epic_title: String, count: usize },
    /// Feed refresh failed.
    Failed { epic_title: String, error: String },
    /// The refresh ran nothing: a feed cycle for this epic was already in
    /// flight, so the request was dropped (feeds.allium: SerialisedFeedCycle).
    /// Deliberately distinct from [`FeedMessage::Failed`] — it is neither a
    /// success nor a failure, and reporting it as a failure would blame the
    /// user's feed command for a serialisation decision.
    AlreadyRefreshing { epic_title: String },
}

impl FeedMessage {
    /// Route this message to its handler on [`App`]. See [`super::SplitMessage::route`].
    pub(in crate::tui) fn route(self, app: &mut App) -> Vec<Command> {
        match self {
            FeedMessage::TriggerEpic(id) => app.handle_trigger_epic_feed(id),
            FeedMessage::Refreshed { epic_title, count } => {
                app.handle_feed_refreshed(epic_title, count)
            }
            FeedMessage::Failed { epic_title, error } => app.handle_feed_failed(epic_title, error),
            FeedMessage::AlreadyRefreshing { epic_title } => {
                app.handle_feed_already_refreshing(epic_title)
            }
        }
    }
}
