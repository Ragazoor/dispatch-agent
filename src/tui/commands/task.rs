//! Task-domain side-effect commands.

use crate::models::{DispatchMode, DrainMode, EpicId, SubStatus, Task, TaskId};

use super::super::types::TaskDraft;

/// What a **successful** worktree removal earns the requesting operation.
///
/// The operation that asked for the teardown cannot apply this itself: the
/// removal shells out to git in the background, so the write that acts on its
/// outcome has to happen on the teardown's completion path. See
/// [`TaskCommand::Cleanup`] and `WorktreeReleaseIsGated` in
/// docs/specs/tasks.allium.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupFollowUp {
    /// Clear the task's `worktree`/`tmux_window` columns (archive, retry-fresh).
    ClearPointer,
    /// Delete the task row (delete from the archive view). A failed removal
    /// therefore leaves the row in place, archived and still pointing at the
    /// directory on disk, so deleting again retries the removal.
    DeleteRow,
    /// Nothing to apply: the row is being removed by the operation that asked
    /// for the teardown, so there is no column left to clear and no pointer that
    /// could be retained. Used by the epic delete, which drops every subtask row
    /// in one operation — the documented exemption from the gate. The failure is
    /// still reported and logged.
    Nothing,
}

/// Side-effect commands for the task domain.
///
/// Wrapped by [`crate::tui::types::Command::Task`] for runtime dispatch.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum TaskCommand {
    Persist(Task),
    Insert {
        draft: TaskDraft,
        epic_id: Option<EpicId>,
    },
    Delete(TaskId),
    DispatchAgent {
        task: Task,
        mode: DispatchMode,
    },
    TrustAndDispatch {
        task: Task,
        mode: DispatchMode,
    },
    /// Check whether `repo_path` is trusted by Claude Code (reads
    /// `~/.claude.json` via `spawn_blocking`) and emit the message that routes
    /// to a dispatch or a trust-confirmation prompt based on the result.
    /// Keeps the file I/O off the render thread — see docs/conventions.md
    /// "No `std::fs` inside async handlers".
    CheckTrustAndDispatch {
        id: TaskId,
        repo_path: String,
        mode: DispatchMode,
    },
    /// Tear a task's live resources down (`TaskTeardown` in
    /// docs/specs/tasks.allium): kill the tmux window, remove the git worktree,
    /// best-effort delete the branch.
    ///
    /// `follow_up` is what a **successful** removal earns, and it is only ever
    /// applied on the removal's own completion path — never beside it. That is
    /// the gate: a failed removal leaves the task pointing at the directory that
    /// is still on disk, instead of forgetting it and stranding an orphan
    /// (`WorktreeReleaseIsGated`).
    Cleanup {
        id: TaskId,
        repo_path: String,
        worktree: String,
        tmux_window: Option<String>,
        follow_up: CleanupFollowUp,
    },
    /// Clear a task's `worktree` and `tmux_window` columns. Emitted by
    /// [`crate::tui::messages::TaskMessage::CleanupSucceeded`], and the only
    /// write that forgets a worktree path on the archive path.
    ClearWorktreePointer(TaskId),
    CheckWindow {
        id: TaskId,
        window: String,
    },
    /// Check all task windows in a single tmux list-windows call. Reduces N
    /// process forks per tick to 1.
    BatchCheckWindows {
        windows: Vec<(TaskId, String)>,
    },
    Resume {
        task: Task,
    },
    JumpToTmux {
        window: String,
    },
    QuickDispatch {
        draft: TaskDraft,
        epic_id: Option<EpicId>,
    },
    /// Confirmed-trust follow-up to `QuickDispatch`: `trust_repo` the draft's
    /// repo, then proceed exactly as `QuickDispatch` would have. Mirrors
    /// `TrustAndDispatch`.
    TrustAndQuickDispatch {
        draft: TaskDraft,
        epic_id: Option<EpicId>,
    },
    KillTmuxWindow {
        window: String,
    },
    PatchSubStatus {
        id: TaskId,
        sub_status: SubStatus,
    },
    /// Move a task to a different epic, or detach it (`new_epic = None`).
    MoveToEpic {
        id: TaskId,
        new_epic: Option<EpicId>,
    },
    /// Seed `last_pre_tool_use_at` on a Backlog→Running transition.
    ///
    /// Kept separate from [`Self::Persist`] so a generic in-memory persist
    /// (sort_order swaps, tick reclassification, etc.) cannot clobber a
    /// freshly hook-written timestamp with a stale in-memory value.
    SeedActivity {
        id: TaskId,
        at: chrono::DateTime<chrono::Utc>,
    },
    /// Return a claimed-but-unprovisioned task to `Backlog`.
    ///
    /// Emitted by `handle_dispatch_failed`, and only there: the caller must have
    /// *held* the claim. Paths that never held it use
    /// `TaskMessage::DispatchAbandoned`, whose doc explains why. The dispatch
    /// watchdog also deliberately does not release — see `tick_dispatching`.
    ReleaseClaim(TaskId),
    /// Update `sub_status` for multiple tasks in a single DB transaction.
    /// Emitted by the tick instead of N individual `Persist` commands so all
    /// reclassifications in one tick round-trip are batched together.
    BatchPatchSubStatus {
        updates: Vec<(TaskId, SubStatus)>,
    },
    RefreshFromDb,
    /// Drop every `task_subagents` entry for a task. See [`DrainMode`] for what
    /// the two modes mean and which callers pick which.
    ClearSubagents {
        id: TaskId,
        mode: DrainMode,
    },
}
