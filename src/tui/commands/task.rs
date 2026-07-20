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
        tmux_window: Option<String>,
    },
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
    /// Update `sub_status` for multiple tasks in a single DB transaction.
    /// Emitted by the tick instead of N individual `Persist` commands so all
    /// reclassifications in one tick round-trip are batched together.
    BatchPatchSubStatus {
        updates: Vec<(TaskId, SubStatus)>,
    },
    RefreshFromDb,
}
