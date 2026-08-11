pub mod scenarios;
pub mod snapshots;

mod archive;
mod budget;
mod dispatch;
mod epics;
mod helpers;
mod input_handlers;
mod layout_cache;
mod main_session;
mod move_task;
mod navigation;
mod render_dirty;
mod rendering;
mod repo_filter;
mod repo_sync;
mod search;
mod split_pane;
mod status_and_presets;
mod targeted_refresh;
mod task_detail;
mod tick_performance;
mod todos;
mod usage;
mod wrap_up;

// Re-exports: child test modules access these via `super::<item>`.
pub(in crate::tui) use super::*;
pub(in crate::tui) use helpers::*;
