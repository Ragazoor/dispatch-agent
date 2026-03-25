use crate::models::{Note, Task};

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
    CreateTask { title: String, description: String, repo_path: String },
    DeleteTask(i64),
    ToggleDetail,
    TmuxOutput { id: i64, output: String },
    WindowGone(i64),
    RefreshTasks(Vec<Task>),
    NotesLoaded { task_id: i64, notes: Vec<Note> },
    Error(String),
}

// ---------------------------------------------------------------------------
// Command
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Command {
    PersistTask(Task),
    DeleteTask(i64),
    Dispatch { task: Task },
    Cleanup { repo_path: String, worktree: String, tmux_window: String },
    CaptureTmux { id: i64, window: String },
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
    InputDescription { title: String },
    InputRepoPath { title: String, description: String },
    ConfirmDelete,
}
