//! Runtime execution of the repo-sync commands (docs/specs/repo-sync.allium).
//!
//! Both operations shell out to git, so both run inside `spawn_blocking` and
//! report their results back as messages — a slow network never delays TUI
//! startup or blocks a frame.

use super::*;

impl TuiRuntime {
    /// Measure one repository's drift off the event loop and report the result
    /// as [`crate::tui::messages::RepoSyncMessage::Measured`] (rule
    /// `RefreshRepoSyncState`).
    pub(super) fn exec_refresh_repo_sync(
        &self,
        repo_path: String,
        fetch_first: bool,
    ) -> tokio::task::JoinHandle<()> {
        let tx = self.msg_tx.clone();
        let runner = Arc::clone(&self.runner);
        tokio::task::spawn_blocking(move || {
            let measurement = crate::repo_sync::measure_repo(&repo_path, fetch_first, &*runner);
            let _ = tx.send(Message::RepoSync(
                crate::tui::messages::RepoSyncMessage::Measured(measurement),
            ));
        })
    }

    /// One fetching refresh per saved repo path (rule
    /// `RefreshRepoSyncStateOnStartup`). Fire-and-forget: results arrive as they
    /// land, and an offline machine simply keeps unmeasured repositories, which
    /// show no indicator.
    pub(super) fn exec_refresh_all_repo_sync(
        &self,
        repo_paths: &[String],
    ) -> Vec<tokio::task::JoinHandle<()>> {
        repo_paths
            .iter()
            .map(|p| self.exec_refresh_repo_sync(p.clone(), true))
            .collect()
    }

    /// Run one sync off the event loop (rule `SyncRepo`), routing the success to
    /// the status bar and the failure to the error popup.
    pub(super) fn exec_sync_repo(
        &self,
        repo_path: String,
        base_branch: String,
    ) -> tokio::task::JoinHandle<()> {
        let tx = self.msg_tx.clone();
        let runner = Arc::clone(&self.runner);
        tokio::task::spawn_blocking(move || {
            let msg = match crate::repo_sync::sync_repo(&repo_path, &base_branch, &*runner) {
                Ok(outcome) => {
                    crate::tui::messages::RepoSyncMessage::Succeeded { repo_path, outcome }
                }
                Err(e) => crate::tui::messages::RepoSyncMessage::Failed {
                    repo_path,
                    detail: e.to_string(),
                    retryable: e.retryable(),
                },
            };
            let _ = tx.send(Message::RepoSync(msg));
        })
    }
}
