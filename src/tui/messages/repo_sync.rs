//! Local-first repo sync messages (docs/specs/repo-sync.allium).

use crate::repo_sync::{RepoSyncMeasurement, SyncOutcome};
use crate::tui::types::Command;
use crate::tui::App;

/// Messages targeting the board's repo-sync state.
///
/// Wrapped by [`crate::tui::types::Message::RepoSync`] for dispatch.
#[derive(Debug, Clone)]
pub enum RepoSyncMessage {
    /// A refresh observation landed (rule `RefreshRepoSyncState`). Sent by the
    /// runtime's off-event-loop measurement worker.
    Measured(RepoSyncMeasurement),
    /// `[o]`: open the sync confirmation for the selected task's repository
    /// (rule `PromptRepoSync`).
    OpenPrompt,
    /// A sync succeeded (`RepoSynced` / `RepoAlreadyInSync`).
    Succeeded {
        repo_path: String,
        outcome: SyncOutcome,
    },
    /// A sync failed (rule `ReportRepoSyncFailure`). `detail` is the
    /// `SyncFailureReport` detail that makes the cause actionable.
    Failed {
        repo_path: String,
        detail: String,
        retryable: bool,
    },
}

impl RepoSyncMessage {
    /// Route this message to its handler on [`App`]. See [`super::SplitMessage::route`].
    pub(in crate::tui) fn route(self, app: &mut App) -> Vec<Command> {
        match self {
            RepoSyncMessage::Measured(m) => app.handle_repo_sync_measured(m),
            RepoSyncMessage::OpenPrompt => app.handle_open_repo_sync_prompt(),
            RepoSyncMessage::Succeeded { repo_path, outcome } => {
                app.handle_repo_sync_succeeded(repo_path, outcome)
            }
            RepoSyncMessage::Failed {
                repo_path,
                detail,
                retryable,
            } => app.handle_repo_sync_failed(repo_path, detail, retryable),
        }
    }
}
