use crate::models::{Task, TaskId, TaskStatus};

// ---------------------------------------------------------------------------
// MoveDirection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveDirection {
    Forward,
    Backward,
}

// ---------------------------------------------------------------------------
// Message
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Message {
    Tick,
    Quit,
    NavigateColumn(isize),
    NavigateRow(isize),
    MoveTask { id: TaskId, direction: MoveDirection },
    DispatchTask(TaskId),
    BrainstormTask(TaskId),
    Dispatched { id: TaskId, worktree: String, tmux_window: String, switch_focus: bool },
    TaskCreated { task: Task },
    DeleteTask(TaskId),
    ToggleDetail,
    TmuxOutput { id: TaskId, output: String },
    WindowGone(TaskId),
    RefreshTasks(Vec<Task>),
    ResumeTask(TaskId),
    Resumed { id: TaskId, tmux_window: String },
    Error(String),
    TaskEdited { id: TaskId, title: String, description: String, repo_path: String, status: TaskStatus, plan: Option<String> },
    RepoPathsUpdated(Vec<String>),
    QuickDispatch { repo_path: String },
}

// ---------------------------------------------------------------------------
// Command
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Command {
    PersistTask(Task),
    InsertTask { title: String, description: String, repo_path: String },
    DeleteTask(TaskId),
    Dispatch { task: Task },
    Brainstorm { task: Task },
    Cleanup { repo_path: String, worktree: String, tmux_window: Option<String> },
    CaptureTmux { id: TaskId, window: String },
    Resume { task: Task },
    JumpToTmux { window: String },
    EditTaskInEditor(Task),
    SaveRepoPath(String),
    RefreshFromDb,
    QuickDispatch { title: String, description: String, repo_path: String },
}

// ---------------------------------------------------------------------------
// InputMode
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    InputTitle,
    InputDescription,
    InputRepoPath,
    ConfirmDelete,
    QuickDispatch,
}

// ---------------------------------------------------------------------------
// TaskDraft
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct TaskDraft {
    pub title: String,
    pub description: String,
}
