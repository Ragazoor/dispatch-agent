//! Pop-out `$EDITOR` flow messages.

use super::super::types::{Command, EditKind, EditorOutcome};
use crate::tui::App;

/// Messages produced by the pop-out editor flow.
///
/// Wrapped by [`crate::tui::types::Message::Editor`] for dispatch.
#[derive(Debug, Clone)]
pub enum EditorMessage {
    /// Editor closed for a description-only edit during task/epic creation.
    DescriptionResult(String),
    /// Editor closed for any other [`EditKind`] (full task/epic edit).
    Result {
        kind: EditKind,
        outcome: EditorOutcome,
    },
}

impl EditorMessage {
    /// Route this message to its handler on [`App`]. See [`super::SplitMessage::route`].
    pub(in crate::tui) fn route(self, app: &mut App) -> Vec<Command> {
        match self {
            EditorMessage::DescriptionResult(value) => app.handle_description_editor_result(value),
            EditorMessage::Result { kind, outcome } => app.handle_editor_result(kind, outcome),
        }
    }
}
