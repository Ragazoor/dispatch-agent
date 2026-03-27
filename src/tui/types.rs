use crate::models::{Note, Task, TaskStatus};

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
    Dispatched { id: i64, worktree: String, tmux_window: String },
    TaskCreated { task: Task },
    DeleteTask(i64),
    ToggleDetail,
    TmuxOutput { id: i64, output: String },
    WindowGone(i64),
    RefreshTasks(Vec<Task>),
    NotesLoaded { task_id: i64, notes: Vec<Note> },
    ResumeTask(i64),
    Resumed { id: i64, tmux_window: String },
    Error(String),
    TaskEdited { id: i64, title: String, description: String, repo_path: String, status: TaskStatus, plan: Option<String> },
    RepoPathsUpdated(Vec<String>),
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
    Cleanup { repo_path: String, worktree: String, tmux_window: Option<String> },
    CaptureTmux { id: i64, window: String },
    Resume { task: Task },
    JumpToTmux { window: String },
    EditTaskInEditor(Task),
    SaveRepoPath(String),
    LoadNotes(i64),
    RefreshFromDb,
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
}

// ---------------------------------------------------------------------------
// TaskDraft
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct TaskDraft {
    pub title: String,
    pub description: String,
}
