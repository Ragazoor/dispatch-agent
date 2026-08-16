//! Settings- and preference-persistence side-effect commands.
//!
//! Everything here writes a durable preference the board reloads at startup:
//! the `settings` table, or a repo's most-recently-used path/base-branch
//! history.

/// Wrapped by [`crate::tui::types::Command::Settings`] for runtime dispatch.
#[derive(Debug, Clone)]
pub enum SettingsCommand {
    /// Record a repo path into the most-recently-used repo-path history.
    SaveRepoPath(String),
    /// Record a base_branch into a repo's most-recently-used history (see
    /// docs/specs/dispatch.allium: rule RecordBaseBranch). Emitted only from
    /// `finish_task_creation` (the manual "new task" form) — never
    /// quick-dispatch or MCP `create_task`.
    SaveBaseBranch(String, String),
    /// Persist a boolean setting under `key`.
    PersistSetting { key: String, value: bool },
    /// Persist a string setting under `key`.
    PersistStringSetting { key: String, value: String },
}
