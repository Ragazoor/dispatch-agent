//! System-level messages: lifecycle, ticks, focus, errors, status, notifications,
//! help/notification toggles, browser-opens.

use crate::tui::types::Command;
use crate::tui::App;

/// System-level messages.
///
/// Wrapped by [`crate::tui::types::Message::System`] for dispatch.
#[derive(Debug, Clone)]
pub enum SystemMessage {
    /// Periodic tick (see `TICK_INTERVAL` in `runtime/mod.rs`).
    Tick,
    /// Terminal was resized.
    TerminalResized,
    /// Window focus changed.
    FocusChanged(bool),
    /// User requested quit.
    Quit,
    /// Error popup.
    Error(String),
    /// Dismiss the active error popup.
    DismissError,
    /// Transient status-bar info message.
    StatusInfo(String),
    /// Toggle the help overlay.
    ToggleHelp,
    /// Toggle desktop notifications setting.
    ToggleNotifications,
    /// Open a URL in the user's browser.
    OpenInBrowser { url: String },
}

impl SystemMessage {
    /// Route this message to its handler on [`App`]. See [`super::SplitMessage::route`].
    pub(in crate::tui) fn route(self, app: &mut App) -> Vec<Command> {
        match self {
            SystemMessage::Tick => app.handle_tick(),
            SystemMessage::TerminalResized => vec![],
            SystemMessage::FocusChanged(focused) => app.handle_focus_changed(focused),
            SystemMessage::Quit => app.handle_quit(),
            SystemMessage::Error(text) => app.handle_error(text),
            SystemMessage::DismissError => app.handle_dismiss_error(),
            SystemMessage::StatusInfo(text) => app.handle_status_info(text),
            SystemMessage::ToggleHelp => app.handle_toggle_help(),
            SystemMessage::ToggleNotifications => app.handle_toggle_notifications(),
            SystemMessage::OpenInBrowser { url } => app.handle_open_in_browser(url),
        }
    }
}
