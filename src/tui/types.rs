use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

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
    TaskEdited(TaskEdit),
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

// ---------------------------------------------------------------------------
// AgentTracking — tmux output and health state for dispatched agents
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct AgentTracking {
    pub tmux_outputs: HashMap<TaskId, String>,
    pub last_output_change: HashMap<TaskId, Instant>,
    pub stale_tasks: HashSet<TaskId>,
    pub crashed_tasks: HashSet<TaskId>,
    pub inactivity_timeout: Duration,
}

impl AgentTracking {
    pub fn new(inactivity_timeout: Duration) -> Self {
        Self {
            tmux_outputs: HashMap::new(),
            last_output_change: HashMap::new(),
            stale_tasks: HashSet::new(),
            crashed_tasks: HashSet::new(),
            inactivity_timeout,
        }
    }

    /// Remove all tracking state for a task.
    pub fn clear(&mut self, id: TaskId) {
        self.last_output_change.remove(&id);
        self.stale_tasks.remove(&id);
        self.crashed_tasks.remove(&id);
        self.tmux_outputs.remove(&id);
    }
}

// ---------------------------------------------------------------------------
// InputState — current input mode and draft
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct InputState {
    pub mode: InputMode,
    pub buffer: String,
    pub task_draft: Option<TaskDraft>,
}

impl Default for InputState {
    fn default() -> Self {
        Self {
            mode: InputMode::Normal,
            buffer: String::new(),
            task_draft: None,
        }
    }
}

// ---------------------------------------------------------------------------
// ArchiveState — archive overlay state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct ArchiveState {
    pub visible: bool,
    pub selected_row: usize,
}

// ---------------------------------------------------------------------------
// TaskEdit — bundled fields for Message::TaskEdited
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct TaskEdit {
    pub id: TaskId,
    pub title: String,
    pub description: String,
    pub repo_path: String,
    pub status: TaskStatus,
    pub plan: Option<String>,
}
