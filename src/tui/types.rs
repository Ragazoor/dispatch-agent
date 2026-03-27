use crate::models::{Task, TaskStatus};

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
    MoveTask { id: i64, direction: MoveDirection },
    DispatchTask(i64),
    BrainstormTask(i64),
    Dispatched { id: i64, worktree: String, tmux_window: String, switch_focus: bool },
    TaskCreated { task: Task },
    DeleteTask(i64),
    ToggleDetail,
    TmuxOutput { id: i64, output: String },
    WindowGone(i64),
    RefreshTasks(Vec<Task>),
    ResumeTask(i64),
    Resumed { id: i64, tmux_window: String },
    Error(String),
    TaskEdited { id: i64, title: String, description: String, repo_path: String, status: TaskStatus, plan: Option<String> },
    RepoPathsUpdated(Vec<String>),
    QuickDispatch { repo_path: String },
    StaleAgent(i64),
    AgentCrashed(i64),
    KillAndRetry(i64),
    RetryResume(i64),
    RetryFresh(i64),
}

// ---------------------------------------------------------------------------
// Command
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Command {
    PersistTask(Task),
    InsertTask { title: String, description: String, repo_path: String },
    DeleteTask(i64),
    Dispatch { task: Task },
    Brainstorm { task: Task },
    Cleanup { repo_path: String, worktree: String, tmux_window: Option<String> },
    CaptureTmux { id: i64, window: String },
    Resume { task: Task },
    JumpToTmux { window: String },
    KillTmuxWindow { window: String },
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
    ConfirmRetry(i64),
}

// ---------------------------------------------------------------------------
// TaskDraft
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct TaskDraft {
    pub title: String,
    pub description: String,
}
