// Declared first and `#[macro_use]`d: `macro_rules!` scope is textual, so
// `mcp_args!` must be in scope before the modules that invoke it.
#[macro_use]
mod args;

mod dispatch;
mod epics;
mod learnings;
mod managed_feeds;
mod tasks;
mod types;

#[cfg(test)]
mod tests;

pub use dispatch::handle_mcp;
pub use dispatch::TOOL_NAMES;
