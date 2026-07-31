//! Task-domain side-effect commands.

use crate::models::{DispatchMode, EpicId, SubStatus, Task, TaskId};

use super::super::types::TaskDraft;

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
    Cleanup {
        id: TaskId,
        repo_path: String,
        worktree: String,
        tmux_window: Option<String>,
    },
    Finish {
        id: TaskId,
        repo_path: String,
        branch: String,
        base_branch: String,
        worktree: String,
    },
    /// Persist a finished task's terminal state, then — only if that write
    /// landed — kill its tmux window. The ordering is normative: a task whose
    /// Done write failed keeps both its live window and its `tmux_window`
    /// reference, so the two can never disagree (`FinishTaskSuccess` in
    /// `docs/specs/pr-workflow.allium`, matching `ExitSession`'s MCP path).
    CloseSession(Task),
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
}
