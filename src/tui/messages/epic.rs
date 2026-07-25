//! Epic CRUD, lifecycle, batch ops, and creation-flow messages.

use crate::models::{Epic, EpicId};

use super::super::types::{Command, MoveDirection, TreeNav};
use crate::tui::App;

/// Messages targeting the epic domain.
///
/// Wrapped by [`crate::tui::types::Message::Epic`] for dispatch.
#[derive(Debug, Clone)]
pub enum EpicMessage {
    Enter(EpicId),
    Exit,
    Refresh(Vec<Epic>),
    /// Splice a single fresh epic into `app.board.epics`.
    Updated(Epic),
    Created(Epic),
    Edit(EpicId),
    Edited(Epic),
    Delete(EpicId),
    ToggleAutoDispatch(EpicId),
    ToggleGroupByRepo(EpicId),
    ConfirmDelete,
    MoveStatus(EpicId, MoveDirection),
    Archive(EpicId),
    ConfirmArchive,
    StartNew,
    SubmitTitle(String),
    SubmitDescription(String),
    ToggleSelect(EpicId),
    BatchArchive(Vec<EpicId>),
    StartReparent(EpicId),
    ReparentNavigate(TreeNav),
    ReparentConfirm,
    ReparentExecute,
    ReparentCancel,
    ReparentCancelAll,
}

impl EpicMessage {
    /// Route this message to its handler on [`App`]. See [`super::SplitMessage::route`].
    pub(in crate::tui) fn route(self, app: &mut App) -> Vec<Command> {
        match self {
            EpicMessage::Enter(id) => app.handle_enter_epic(id),
            EpicMessage::Exit => app.handle_exit_epic(),
            EpicMessage::Refresh(epics) => app.handle_refresh_epics(epics),
            EpicMessage::Updated(epic) => app.handle_epic_updated(epic),
            EpicMessage::Created(epic) => app.handle_epic_created(epic),
            EpicMessage::Edit(id) => app.handle_edit_epic(id),
            EpicMessage::Edited(epic) => app.handle_epic_edited(epic),
            EpicMessage::Delete(id) => app.handle_delete_epic(id),
            EpicMessage::ToggleAutoDispatch(id) => app.handle_toggle_epic_auto_dispatch(id),
            EpicMessage::ToggleGroupByRepo(id) => app.handle_toggle_epic_group_by_repo(id),
            EpicMessage::ConfirmDelete => app.handle_confirm_delete_epic(),
            EpicMessage::MoveStatus(id, dir) => app.handle_move_epic_status(id, dir),
            EpicMessage::Archive(id) => app.handle_archive_epic(id),
            EpicMessage::ConfirmArchive => app.handle_confirm_archive_epic(),
            EpicMessage::StartNew => app.handle_start_new_epic(),
            EpicMessage::SubmitTitle(v) => app.handle_submit_epic_title(v),
            EpicMessage::SubmitDescription(v) => app.handle_submit_epic_description(v),
            EpicMessage::ToggleSelect(id) => app.handle_toggle_select_epic(id),
            EpicMessage::BatchArchive(ids) => app.handle_batch_archive_epics(ids),
            EpicMessage::StartReparent(id) => app.handle_start_reparent(id),
            EpicMessage::ReparentNavigate(nav) => app.handle_reparent_navigate(nav),
            EpicMessage::ReparentConfirm => app.handle_reparent_confirm(),
            EpicMessage::ReparentExecute => app.handle_reparent_execute(),
            EpicMessage::ReparentCancel => app.handle_reparent_cancel(),
            EpicMessage::ReparentCancelAll => app.handle_reparent_cancel_all(),
        }
    }
}
