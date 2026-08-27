//! Domain model.
//!
//! Types are split per concern into submodules and re-exported here, so
//! external code continues to use flat paths (`models::Task`, `models::Epic`,
//! `models::expand_tilde`, …) regardless of which submodule owns a type.
//!
//! - [`ids`] — the `define_id_newtype!` macro behind `TaskId`/`EpicId`/`LearningId`/`TodoId`
//! - [`string_enum`] — the `define_str_enum!` macro behind status/tag/mode string conversions
//! - [`paths`] — path utilities (`expand_tilde`) and the repo-grouping family
//!   (`repo_name_from_path`/`repo_name_from_url`/`extract_github_repo`)
//! - [`tmux_window`] — the `task-<id>` window/session naming convention
//! - [`tasks`] — tasks, statuses, tags, dispatch mode, slugify, age formatting
//! - [`epics`] — epics, epic sub-status, descendant traversal
//! - [`review`] — review decisions, PR-URL parsing
//! - [`learnings`] — knowledge-base entries
//! - [`todos`] — personal TODO list items
//! - [`usage`] — usage events
//! - [`budget`] — Claude subscription rate-limit windows
//! - [`columns`] — `VisualColumn` kanban board layout
//! - [`interval`] — the interval literal (`10m`, `600`) every cadence field takes
//! - [`url`] — typed task URLs

// `define_id_newtype!` is `#[macro_export]`ed (crate root); consuming modules
// bring it into scope with `use crate::define_id_newtype;`.
mod ids;

// `define_str_enum!` is `#[macro_export]`ed (crate root); consuming modules
// bring it into scope with `use crate::define_str_enum;`.
mod string_enum;

mod paths;
pub use paths::{
    expand_tilde, extract_github_repo, repo_name_from_path, repo_name_from_url, UNKNOWN_REPO_GROUP,
};

mod tmux_window;
pub use tmux_window::{build_tmux_window_name, parse_tmux_window_task_id};

pub mod learnings;
pub use learnings::*;

pub mod todos;
pub use todos::*;

pub mod review;
pub use review::*;
pub mod tasks;
pub use tasks::*;

pub mod epics;
pub use epics::*;

pub mod usage;
pub use usage::*;

pub mod budget;
pub use budget::*;

mod columns;
pub use columns::{task_column_priority, task_header_label, DerivedSection, VisualColumn};

mod interval;
pub use interval::{format_interval_secs, parse_interval_secs, INTERVAL_EXAMPLES};

mod url;
pub use url::{TaskUrl, UrlType};
