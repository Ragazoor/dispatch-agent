//! Main session setup messages.

use crate::tui::types::Command;
use crate::tui::App;

/// Messages targeting the main session setup flow.
///
/// Wrapped by [`crate::tui::types::Message::MainSession`] for dispatch.
#[derive(Debug, Clone)]
pub enum MainSessionMessage {
    /// Open the repo picker (MainSessionDir mode) to (re)select the directory.
    /// Emitted by the runtime when `:` is pressed and no main-session window is
    /// alive.
    Configure,
    /// The user confirmed a directory in the picker.
    SubmitDir(String),
    /// Result of a main-session liveness poll: whether the "dispatch-main"
    /// tmux window is currently alive. Emitted by the runtime's periodic check.
    LivenessChanged(bool),
}

impl MainSessionMessage {
    /// Route this message to its handler on [`App`]. See [`super::SplitMessage::route`].
    pub(in crate::tui) fn route(self, app: &mut App) -> Vec<Command> {
        match self {
            MainSessionMessage::Configure => app.handle_configure_main_session(),
            MainSessionMessage::SubmitDir(dir) => app.handle_submit_main_session_dir(dir),
            MainSessionMessage::LivenessChanged(alive) => app.handle_main_session_liveness(alive),
        }
    }
}
