//! Feed-trigger and feed-result handlers.

use crate::models::EpicId;

use super::super::types::*;
use super::super::App;

impl App {
    pub(in crate::tui) fn handle_trigger_epic_feed(&mut self, id: EpicId) -> Vec<Command> {
        let result = self.find_epic(id).and_then(|e| {
            e.feed_command
                .as_deref()
                .map(|cmd| (e.title.clone(), cmd.to_owned(), e.group_by_repo))
        });
        match result {
            Some((title, feed_command, group_by_repo)) => {
                self.set_status(format!("Fetching feed for '{title}'…"));
                vec![Command::Feed(
                    crate::tui::commands::FeedCommand::TriggerEpic {
                        epic_id: id,
                        epic_title: title,
                        feed_command,
                        group_by_repo,
                    },
                )]
            }
            None => {
                self.set_status("No feed command configured".to_string());
                vec![]
            }
        }
    }

    pub(in crate::tui) fn handle_feed_refreshed(
        &mut self,
        epic_title: String,
        count: usize,
        wrote_stderr: bool,
    ) -> Vec<Command> {
        // Only when nothing synced: a feed command that reported an error on
        // stderr and still exited 0 usually emitted a degraded array, and a
        // zero-item result is where that matters. Above zero, stderr is
        // chatter and the log line alone is enough.
        let hint = if count == 0 && wrote_stderr {
            " — command wrote to stderr (see app.log)"
        } else {
            ""
        };
        self.set_status(format!(
            "Feed for '{epic_title}': {count} task(s) synced{hint}"
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
}
