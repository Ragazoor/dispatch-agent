//! Local-first repo sync side-effect commands (docs/specs/repo-sync.allium).

/// Commands the runtime executes off the event loop for repo sync. Both shell
/// out to git, so neither may run on the async or render path.
#[derive(Debug, Clone)]
pub enum RepoSyncCommand {
    /// Re-measure one repository's drift (rule `RefreshRepoSyncState`).
    /// `fetch_first` is true only for the startup refresh — every other refresh
    /// point rides refs some other operation already refreshed.
    Refresh {
        repo_path: String,
        fetch_first: bool,
    },
    /// Bring one repository into step with origin (rule `SyncRepo`). Emitted
    /// only from the confirmed `[o]` prompt — never automatically
    /// (`SyncNeverAutomatic`).
    Sync {
        repo_path: String,
        base_branch: String,
    },
}
