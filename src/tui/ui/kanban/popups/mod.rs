//! Confirmation overlays and popup helpers (error, help, repo filter, task detail).

mod error;
mod help;
mod managed_feeds;
mod reparent_epic;
mod repo_filter;
mod task_detail;

pub(super) use error::render_error_popup;
pub(super) use help::render_help_overlay;
pub(super) use managed_feeds::render_managed_feed_config_overlay;
pub(in crate::tui) use reparent_epic::build_reparent_tree;
pub(super) use reparent_epic::{render_move_task_overlay, render_reparent_epic_overlay};
pub(super) use repo_filter::render_repo_filter_overlay;
pub(super) use task_detail::render_task_detail_overlay;
