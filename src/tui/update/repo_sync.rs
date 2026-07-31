//! Local-first repo sync handlers (docs/specs/repo-sync.allium).
//!
//! The board keeps one drift measurement per repository in `App.repo_sync` and
//! nothing else: no persistence, no poll interval. Refreshes are event-driven,
//! and the only network *write* — the sync itself — is always operator-initiated
//! (`SyncNeverAutomatic`).

use crate::repo_sync::{RepoSyncMeasurement, RepoSyncState, SyncOutcome};
use crate::tui::commands::RepoSyncCommand;
use crate::tui::types::{Command, InputMode};
use crate::tui::App;

impl App {
    /// The drift measurement for the repository owning the currently selected
    /// task, or `None` when the cursor is not on a task (an epic row names no
    /// repository) or that repository has never been refreshed.
    pub(in crate::tui) fn selected_repo_sync_state(&self) -> Option<&RepoSyncState> {
        // `get` borrows the key as `&str`, so no clone is needed — this runs on
        // the status-bar render path, i.e. every tick and every keypress.
        self.repo_sync.get(self.selected_task()?.repo_path.as_str())
    }

    /// Fold one refresh observation into the per-repo cache (rule
    /// `RefreshRepoSyncState`).
    pub(in crate::tui) fn handle_repo_sync_measured(
        &mut self,
        m: RepoSyncMeasurement,
    ) -> Vec<Command> {
        self.repo_sync.apply(m);
        self.dirty = true;
        vec![]
    }

    /// A non-fetching refresh for `repo_path`. The refresh points other than
    /// startup all ride refs some other operation just refreshed, so this is a
    /// local ref read with no network cost.
    pub(in crate::tui) fn refresh_repo_sync_command(repo_path: String) -> Command {
        Command::RepoSync(RepoSyncCommand::Refresh {
            repo_path,
            fetch_first: false,
        })
    }

    /// `[o]`: open the sync confirmation for the selected task's repository
    /// (rule `PromptRepoSync`). With no selected task, an unmeasured repository
    /// or a clean one there is no drift to close and no prompt is shown — the
    /// same condition that hides the indicator.
    pub(in crate::tui) fn handle_open_repo_sync_prompt(&mut self) -> Vec<Command> {
        let Some(state) = self.selected_repo_sync_state() else {
            return vec![];
        };
        if !state.has_drift() {
            return vec![];
        }
        let repo_path = state.repo_path.clone();
        let prompt = crate::tui::ui::repo_sync_prompt_text(state);
        self.input.mode = InputMode::ConfirmRepoSync { repo_path };
        self.set_status(prompt);
        vec![]
    }

    /// A sync succeeded. The outcome goes to the status bar — the user does not
    /// have to act on it — and the repository is recounted so the indicator
    /// clears (rule `RefreshRepoSyncStateAfterSync`).
    pub(in crate::tui) fn handle_repo_sync_succeeded(
        &mut self,
        repo_path: String,
        outcome: SyncOutcome,
    ) -> Vec<Command> {
        let base = self
            .repo_sync
            .get(&repo_path)
            .map(|s| s.base_branch.clone())
            .unwrap_or_default();
        // AlreadyInSync is also returned when the post-fetch recount could not be
        // read at all, so it is worded as what the operation did rather than as a
        // claim that the repository is level, and quotes no counts.
        let msg = match outcome {
            SyncOutcome::AlreadyInSync => format!("{base}: nothing to do"),
            SyncOutcome::Synced { pulled, pushed } => {
                format!("Synced {base}: pulled {pulled}, pushed {pushed}")
            }
        };
        self.set_status(msg);
        vec![Self::refresh_repo_sync_command(repo_path)]
    }

    /// A sync failed (rule `ReportRepoSyncFailure`). Failures go to the error
    /// popup, never the status bar: every one of them needs a decision. No
    /// refresh follows — `RepoSyncFinished` is ensured by a completed `SyncRepo`,
    /// not by a failed one.
    pub(in crate::tui) fn handle_repo_sync_failed(
        &mut self,
        _repo_path: String,
        detail: String,
        retryable: bool,
    ) -> Vec<Command> {
        let msg = if retryable {
            format!("Repo sync failed: {detail}. Retrying the same action is the fix — try again.")
        } else {
            format!("Repo sync failed: {detail}")
        };
        self.status.error_popup = Some(msg);
        vec![]
    }

    /// Confirming the prompt syncs the repository (rule `AcceptRepoSyncPrompt`).
    /// The measurement is re-read here rather than captured when the prompt
    /// opened, so a refresh that landed meanwhile cannot be acted on stale.
    pub(in crate::tui) fn confirm_repo_sync(&mut self, repo_path: &str) -> Vec<Command> {
        let Some(state) = self.repo_sync.get(repo_path) else {
            return vec![];
        };
        if !state.has_drift() {
            return vec![];
        }
        vec![Command::RepoSync(RepoSyncCommand::Sync {
            repo_path: state.repo_path.clone(),
            base_branch: state.base_branch.clone(),
        })]
    }
}
