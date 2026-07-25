#![recursion_limit = "256"]

/// Default port for the MCP server, used when `DISPATCH_PORT` is not set.
pub const DEFAULT_PORT: u16 = 3142;

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
pub mod runtime;
pub mod service;
pub mod setup;
pub mod tips;
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
