/// Default port for the MCP server, used when `DISPATCH_PORT` is not set.
pub const DEFAULT_PORT: u16 = 3142;

pub mod db;
pub mod dispatch;
pub mod editor;
pub mod github;
pub mod mcp;
pub mod models;
pub mod plan;
pub mod process;
pub mod runtime;
pub mod service;
pub mod setup;
pub mod tmux;
pub mod tui;
