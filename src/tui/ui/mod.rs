pub(in crate::tui::ui) mod budget;
mod input_form;
mod kanban;
pub(crate) mod palette;
mod shared;
pub mod todos;

pub(in crate::tui) use kanban::build_reparent_tree;
pub use kanban::render;
pub(in crate::tui) use kanban::repo_sync_prompt_text;
// Only the status bar itself renders the drift segment; the re-export exists so
// the surface's guarantees can be asserted directly.
#[cfg(test)]
pub(in crate::tui) use kanban::repo_drift_segment;
// The prompt's path shortening and its budget, re-exported so
// PromptNamesTheRepository can be asserted directly.
#[cfg(test)]
pub(in crate::tui) use kanban::{repo_path_for_prompt, REPO_PATH_DISPLAY_BUDGET};
pub(in crate::tui) use shared::caret_field_line;
pub use shared::{refresh_status, truncate};

#[cfg(test)]
pub(in crate::tui) use kanban::{action_hints, column_color, epic_action_hints};
