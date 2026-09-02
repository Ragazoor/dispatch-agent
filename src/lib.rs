#![recursion_limit = "256"]

/// Default port for the MCP server, used when `DISPATCH_PORT` is not set.
pub const DEFAULT_PORT: u16 = 3142;

pub mod agent_tree;
pub mod agent_tree_editor;
pub(crate) mod claude_paths;
pub mod cli;
pub mod db;
pub mod dispatch;
pub mod editor;
pub mod feed;
pub mod git;
pub mod mcp;
pub mod models;
pub mod notify;
pub mod plan;
pub mod process;
pub mod repo_sync;
pub mod runtime;
pub mod service;
pub mod setup;
#[cfg(test)]
mod test_log;
pub mod tmux;
pub mod tui;

pub fn default_db_path() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            home.join(".local").join("share")
        });
    base.join("dispatch").join("tasks.db")
}

/// The one budget-snapshot location on this machine.
///
/// Takes no database argument, deliberately. The Claude subscription windows it
/// holds are account-global, so publisher and reader must agree on a single
/// location that does not vary with whichever task database the current process
/// happens to have open — see `docs/specs/dispatch.allium`:
/// `SnapshotLocationIsFixedNotDerivedFromTheOpenDatabase`.
pub(crate) fn budget_snapshot_path() -> std::path::PathBuf {
    default_db_path().with_file_name(crate::setup::statusline::RATE_LIMITS_FILE_NAME)
}

#[cfg(test)]
mod budget_snapshot_path_tests {
    /// The snapshot sits beside the default database, under its own fixed name.
    /// The file name is spelled out rather than imported from the constant the
    /// code reads: an expectation derived from the code under test asserts
    /// nothing.
    #[test]
    fn sits_beside_the_default_database() {
        let path = super::budget_snapshot_path();

        assert_eq!(
            path.file_name(),
            Some(std::ffi::OsStr::new("rate-limits.json"))
        );
        assert_eq!(path.parent(), super::default_db_path().parent());
    }
}
