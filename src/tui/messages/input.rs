//! Form input, text-entry, and confirmation-flow messages for the task
//! creation, copy, edit, and delete flows.

use crate::models::{TaskTag, WrapUpMode};

use crate::tui::types::Command;
use crate::tui::App;

/// Messages targeting the form-input flow.
///
/// Wrapped by [`crate::tui::types::Message::Input`] for dispatch.
#[derive(Debug, Clone)]
pub enum InputMessage {
    StartNewTask,
    CopyTask,
    CancelInput,
    SubmitTitle(String),
    SubmitDescription(String),
    SubmitRepoPath(String),
    SubmitTag(Option<TaskTag>),
    SubmitBaseBranch(String),
    SubmitWrapUpMode(Option<WrapUpMode>),
    SubmitPhoenix(bool),
    InputChar(char),
    InputBackspace,
    InputDeleteForward,
    CursorLeft,
    CursorRight,
    CursorWordLeft,
    CursorWordRight,
    CursorHome,
    CursorEnd,
    StartQuickDispatchSelection,
    SelectQuickDispatchRepo(usize),
    CancelRetry,
    ConfirmDone,
    CancelDone,
    ConfirmDetachTmux,
}

impl InputMessage {
    /// Route this message to its handler on [`App`]. See [`super::SplitMessage::route`].
    pub(in crate::tui) fn route(self, app: &mut App) -> Vec<Command> {
        match self {
            InputMessage::StartNewTask => app.handle_start_new_task(),
            InputMessage::CopyTask => app.handle_copy_task(),
            InputMessage::CancelInput => app.handle_cancel_input(),
            InputMessage::SubmitTitle(value) => app.handle_submit_title(value),
            InputMessage::SubmitDescription(value) => app.handle_submit_description(value),
            InputMessage::SubmitRepoPath(value) => app.handle_submit_repo_path(value),
            InputMessage::SubmitTag(tag) => app.handle_submit_tag(tag),
            InputMessage::SubmitBaseBranch(value) => app.handle_submit_base_branch(value),
            InputMessage::SubmitWrapUpMode(mode) => app.handle_submit_wrap_up_mode(mode),
            InputMessage::SubmitPhoenix(on) => app.handle_submit_phoenix(on),
            InputMessage::InputChar(c) => app.handle_input_char(c),
            InputMessage::InputBackspace => app.handle_input_backspace(),
            InputMessage::InputDeleteForward => app.handle_input_delete_forward(),
            InputMessage::CursorLeft => app.handle_cursor_left(),
            InputMessage::CursorRight => app.handle_cursor_right(),
            InputMessage::CursorWordLeft => app.handle_cursor_word_left(),
            InputMessage::CursorWordRight => app.handle_cursor_word_right(),
            InputMessage::CursorHome => app.handle_cursor_home(),
            InputMessage::CursorEnd => app.handle_cursor_end(),
            InputMessage::StartQuickDispatchSelection => {
                app.handle_start_quick_dispatch_selection()
            }
            InputMessage::SelectQuickDispatchRepo(idx) => {
                app.handle_select_quick_dispatch_repo(idx)
            }
            InputMessage::CancelRetry => app.handle_cancel_retry(),
            InputMessage::ConfirmDone => app.handle_confirm_done(),
            InputMessage::CancelDone => app.handle_cancel_done(),
            InputMessage::ConfirmDetachTmux => app.handle_confirm_detach_tmux(),
        }
    }
}
