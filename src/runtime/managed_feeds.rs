//! Runtime handlers for the managed-feed config popup.
//!
//! `PersistConfig` writes the four settings; `ProvisionAndRefresh` re-fires
//! `provision_managed_feeds_from_settings` and syncs the resulting epic tree
//! into the board, so enabling a feed provisions it without a restart. Both are
//! best-effort: a DB/provisioning failure surfaces as an error popup but never
//! wedges the TUI (matching the startup provisioning posture).

use super::*;

impl TuiRuntime {
    pub(super) async fn exec_persist_managed_feed_config(
        &self,
        app: &mut App,
        reviews_command: Option<String>,
        reviews_interval_secs: Option<i64>,
        cve_command: Option<String>,
        cve_interval_secs: Option<i64>,
    ) {
        let result = crate::service::write_managed_feed_settings(
            &*self.database,
            crate::service::ManagedFeedSettingsPatch {
                reviews_command: Some(reviews_command),
                reviews_interval_secs: Some(reviews_interval_secs),
                cve_command: Some(cve_command),
                cve_interval_secs: Some(cve_interval_secs),
            },
        )
        .await;
        if let Err(e) = result {
            app.update(Message::System(crate::tui::messages::SystemMessage::Error(
                Self::db_error("persisting managed feed config", e),
            )));
        }
    }

    pub(super) async fn exec_provision_and_refresh(&self, app: &mut App) {
        let settings = match crate::service::read_managed_feed_settings(&*self.database).await {
            Ok(s) => s,
            Err(e) => {
                app.update(Message::System(crate::tui::messages::SystemMessage::Error(
                    Self::db_error("reading managed feed settings", e),
                )));
                return;
            }
        };
        if let Err(e) = self.epic_svc.provision_managed_feeds(settings).await {
            app.update(Message::System(crate::tui::messages::SystemMessage::Error(
                Self::db_error("provisioning managed feeds", e),
            )));
            return;
        }
        // Invalidate the FeedRunner's cache so the next tick re-queries for feed
        // commands. Without this, enabling the first feed on a previously
        // feed-less instance leaves the runner short-circuiting on a stale
        // `any_feed_cmds == Some(false)` until an unrelated EpicChanged/Refresh
        // or a restart — the [C] save path does not go through McpEvent, so it
        // must invalidate the cache itself. Mirrors the MCP Refresh path.
        self.invalidate_feed_cache();
        // Surface any newly-created managed epics in the board.
        self.exec_refresh_epics_from_db(app).await;
    }
}
