//! Task lifecycle, dispatch, retry, selection, finish, detach messages.

use crate::models::{DispatchMode, EpicId, Task, TaskId};

use super::super::types::{Command, MoveDirection, TaskDraft, TaskEdit, TreeNav};
use crate::tui::App;

/// Messages targeting the task domain.
///
/// Wrapped by [`crate::tui::types::Message::Task`] for dispatch.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum TaskMessage {
    Move {
        id: TaskId,
        direction: MoveDirection,
    },
    /// +1 = down, -1 = up
    ReorderItem(isize),
    Dispatch(TaskId, DispatchMode),
    Dispatched {
        id: TaskId,
        worktree: String,
        tmux_window: String,
        switch_focus: bool,
    },
    Created {
        task: Task,
    },
    Delete(TaskId),
    OpenDetail(TaskId),
    CloseDetail,
    ToggleFlattened,
    WindowGone(TaskId),
    Refresh(Vec<Task>),
    /// Splice a single fresh task into `app.board.tasks`.
    Updated(Task),
    Resume(TaskId),
    Resumed {
        id: TaskId,
        tmux_window: String,
    },
    /// A dispatch that **held** the claim failed or panicked. Drains the spinner
    /// *and* releases the claim.
    DispatchFailed(TaskId),
    /// A dispatch ended without ever holding the claim: it lost the claim to
    /// another entry point, or gave up before claiming (a failed repo-trust
    /// grant). Drains the spinner and nothing else.
    ///
    /// Deliberately not `DispatchFailed`. Releasing here would target a claim
    /// belonging to someone else: the winner of a contested claim is itself
    /// Running-with-no-worktree for as long as its provisioning takes, which is
    /// the exact state `release_claim` fires on, so the release would hand the
    /// winner's task back to Backlog mid-provision.
    DispatchAbandoned(TaskId),
    MarkDispatching(TaskId),
    Edited(TaskEdit),
    QuickDispatch {
        repo_path: String,
        epic_id: Option<EpicId>,
    },
    AgentCrashed(TaskId),
    KillAndRetry(TaskId),
    TrustAndDispatch {
        id: TaskId,
        mode: DispatchMode,
    },
    /// Emitted by the runtime once `CheckTrustAndDispatch` finds the repo
    /// untrusted: enters the trust-confirmation prompt.
    TrustCheckUntrusted {
        id: TaskId,
        mode: DispatchMode,
        repo_path: String,
    },
    /// Emitted by the runtime once `QuickDispatch`'s trust check finds the
    /// repo untrusted: enters the quick-dispatch trust-confirmation prompt.
    TrustCheckUntrustedForQuickDispatch {
        draft: TaskDraft,
        epic_id: Option<EpicId>,
    },
    RetryResume(TaskId),
    RetryFresh(TaskId),
    Archive(TaskId),
    ToggleSelect(TaskId),
    BatchMove {
        ids: Vec<TaskId>,
        direction: MoveDirection,
    },
    BatchArchive(Vec<TaskId>),
    DetachTmux(TaskId),
    BatchDetachTmux(Vec<TaskId>),
    // Move-to-epic tree picker (the `m` key on a task card).
    StartMoveToEpic(TaskId),
    MoveToEpicNavigate(TreeNav),
    MoveToEpicConfirm,
    MoveToEpicExecute,
    MoveToEpicCancel,
    MoveToEpicCancelAll,
}

impl TaskMessage {
    /// Route this message to its handler on [`App`]. See [`super::SplitMessage::route`].
    pub(in crate::tui) fn route(self, app: &mut App) -> Vec<Command> {
        match self {
            TaskMessage::Move { id, direction } => app.handle_move_task(id, direction),
            TaskMessage::ReorderItem(dir) => app.handle_reorder_item(dir),
            TaskMessage::Dispatch(id, mode) => app.handle_dispatch_task(id, mode),
            TaskMessage::Dispatched {
                id,
                worktree,
                tmux_window,
                switch_focus,
            } => app.handle_dispatched(id, worktree, tmux_window, switch_focus),
            TaskMessage::Created { task } => app.handle_task_created(task),
            TaskMessage::Delete(id) => app.handle_delete_task(id),
            TaskMessage::OpenDetail(task_id) => app.handle_open_task_detail(task_id),
            TaskMessage::CloseDetail => app.handle_close_task_detail(),
            TaskMessage::ToggleFlattened => app.handle_toggle_flattened(),
            TaskMessage::WindowGone(id) => app.handle_window_gone(id),
            TaskMessage::Refresh(tasks) => app.handle_refresh_tasks(tasks),
            TaskMessage::Updated(task) => app.handle_task_updated(task),
            TaskMessage::Resume(id) => app.handle_resume_task(id),
            TaskMessage::Resumed { id, tmux_window } => app.handle_resumed(id, tmux_window),
            TaskMessage::DispatchFailed(id) => app.handle_dispatch_failed(id),
            TaskMessage::DispatchAbandoned(id) => app.handle_dispatch_abandoned(id),
            TaskMessage::MarkDispatching(id) => app.handle_mark_dispatching(id),
            TaskMessage::Edited(edit) => app.handle_task_edited(edit),
            TaskMessage::QuickDispatch { repo_path, epic_id } => {
                app.handle_quick_dispatch(repo_path, epic_id)
            }
            TaskMessage::AgentCrashed(id) => app.handle_agent_crashed(id),
            TaskMessage::KillAndRetry(id) => app.handle_kill_and_retry(id),
            TaskMessage::TrustAndDispatch { id, mode } => app.handle_trust_and_dispatch(id, mode),
            TaskMessage::TrustCheckUntrusted {
                id,
                mode,
                repo_path,
            } => app.handle_trust_check_untrusted(id, mode, repo_path),
            TaskMessage::TrustCheckUntrustedForQuickDispatch { draft, epic_id } => {
                app.handle_trust_check_untrusted_for_quick_dispatch(draft, epic_id)
            }
            TaskMessage::RetryResume(id) => app.handle_retry_resume(id),
            TaskMessage::RetryFresh(id) => app.handle_retry_fresh(id),
            TaskMessage::Archive(id) => app.handle_archive_task(id),
            TaskMessage::ToggleSelect(id) => app.handle_toggle_select(id),
            TaskMessage::BatchMove { ids, direction } => {
                app.handle_batch_move_tasks(ids, direction)
            }
            TaskMessage::BatchArchive(ids) => app.handle_batch_archive_tasks(ids),
            TaskMessage::DetachTmux(id) => app.handle_detach_tmux(vec![id]),
            TaskMessage::BatchDetachTmux(ids) => app.handle_detach_tmux(ids),
            TaskMessage::StartMoveToEpic(id) => app.handle_start_move_to_epic(id),
            TaskMessage::MoveToEpicNavigate(nav) => app.handle_move_to_epic_navigate(nav),
            TaskMessage::MoveToEpicConfirm => app.handle_move_to_epic_confirm(),
            TaskMessage::MoveToEpicExecute => app.handle_move_to_epic_execute(),
            TaskMessage::MoveToEpicCancel => app.handle_move_to_epic_cancel(),
            TaskMessage::MoveToEpicCancelAll => app.handle_move_to_epic_cancel_all(),
        }
    }
}
