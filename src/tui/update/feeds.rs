//! Feed-trigger and feed-result handlers.

use crate::models::EpicId;

use super::super::types::*;
use super::super::App;

impl App {
    pub(in crate::tui) fn handle_trigger_epic_feed(&mut self, id: EpicId) -> Vec<Command> {
        // The cached board answers only "is this a feed epic at all?" — this
        // rule's requires clause, for which stale data is fine. The values the
        // cycle ACTS on (feed_command, feed_role, group_by_repo,
        // feed_append_only) are re-read from the epic inside FeedCycle::run, so
        // a refresh can never run a command, a grouping mode or a removal
        // policy the user has since changed.
        let title = self
            .find_epic(id)
            .filter(|e| e.feed_command.is_some())
            .map(|e| e.title.clone());
        match title {
            Some(title) => {
                self.set_status(format!("Fetching feed for '{title}'…"));
                vec![Command::Feed(
                    crate::tui::commands::FeedCommand::TriggerEpic {
                        epic_id: id,
                        epic_title: title,
                    },
                )]
            }
            None => {
                self.set_status("No feed command configured".to_string());
                vec![]
            }
        }
    }

    /// `degraded` carries the reason a partially degraded emission ran
    /// additively (feeds.allium: `DegradedNonEmptyEmission`); when present it
    /// becomes a suffix naming why nothing was removed, so the user does not
    /// read a withheld reconcile as a completed one.
    pub(in crate::tui) fn handle_feed_refreshed(
        &mut self,
        epic_title: String,
        count: usize,
        degraded: Option<String>,
    ) -> Vec<Command> {
        let suffix = match degraded {
            Some(reason) => format!(" (additive, no removals: {reason})"),
            None => String::new(),
        };
        self.set_status(format!(
            "Feed for '{epic_title}': {count} task(s) synced{suffix}"
        ));
        vec![Command::Task(
            crate::tui::commands::TaskCommand::RefreshFromDb,
        )]
    }

    pub(in crate::tui) fn handle_feed_failed(
        &mut self,
        epic_title: String,
        error: String,
    ) -> Vec<Command> {
        self.set_status(format!("Feed for '{epic_title}' failed: {error}"));
        vec![]
    }

    /// The refresh was dropped because a cycle for this epic was already
    /// running (feeds.allium: SerialisedFeedCycle). No `RefreshFromDb`: nothing
    /// was written, so there is nothing to reload.
    pub(in crate::tui) fn handle_feed_already_refreshing(
        &mut self,
        epic_title: String,
    ) -> Vec<Command> {
        self.set_status(format!("Feed for '{epic_title}' is already refreshing…"));
        vec![]
    }
}
