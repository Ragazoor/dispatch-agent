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
    StaleAgent(TaskId),
    AgentCrashed(TaskId),
    KillAndRetry(TaskId),
    RetryResume(TaskId),
    RetryFresh(TaskId),
    ArchiveTask(TaskId),
    ToggleArchive,
    // Input routing messages
    DismissError,
    StartNewTask,
    CancelInput,
    ConfirmDeleteStart,
    ConfirmDeleteYes,
    CancelDelete,
    SubmitTitle(String),
    SubmitDescription(String),
    SubmitRepoPath(String),
    InputChar(char),
    InputBackspace,
    StartQuickDispatchSelection,
    SelectQuickDispatchRepo(usize),
    CancelRetry,
    StatusInfo(String),
}

// ---------------------------------------------------------------------------
// Command
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Command {
    PersistTask(Task),
    InsertTask(TaskDraft),
    DeleteTask(TaskId),
    Dispatch { task: Task },
    Brainstorm { task: Task },
    Cleanup { repo_path: String, worktree: String, tmux_window: Option<String> },
    CaptureTmux { id: TaskId, window: String },
    Resume { task: Task },
    JumpToTmux { window: String },
    KillTmuxWindow { window: String },
    EditTaskInEditor(Task),
    SaveRepoPath(String),
    RefreshFromDb,
    QuickDispatch(TaskDraft),
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
    ConfirmRetry(TaskId),
    ConfirmArchive,
}

// ---------------------------------------------------------------------------
// TaskDraft
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct TaskDraft {
    pub title: String,
    pub description: String,
    pub repo_path: String,
}
