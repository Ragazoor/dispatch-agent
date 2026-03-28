use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::models::{Epic, EpicId, Task, TaskId, TaskStatus};

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
    ToggleSelect(TaskId),
    ClearSelection,
    BatchMoveTasks { ids: Vec<TaskId>, direction: MoveDirection },
    BatchArchiveTasks(Vec<TaskId>),
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
    ToggleHelp,
    // Epic messages
    EnterEpic(EpicId),
    ExitEpic,
    RefreshEpics(Vec<Epic>),
    EpicCreated(Epic),
    EditEpic(EpicId),
    EpicEdited(Epic),
    DeleteEpic(EpicId),
    ConfirmDeleteEpic,
    MarkEpicDone(EpicId),
    ArchiveEpic(EpicId),
    ConfirmArchiveEpic,
    StartNewEpic,
    SubmitEpicTitle(String),
    SubmitEpicDescription(String),
    SubmitEpicRepoPath(String),
}

// ---------------------------------------------------------------------------
// Command
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Command {
    PersistTask(Task),
    InsertTask { draft: TaskDraft, epic_id: Option<EpicId> },
    DeleteTask(TaskId),
    Dispatch { task: Task },
    Brainstorm { task: Task },
    Cleanup { id: TaskId, repo_path: String, worktree: String, tmux_window: Option<String> },
    CaptureTmux { id: TaskId, window: String },
    Resume { task: Task },
    JumpToTmux { window: String },
    KillTmuxWindow { window: String },
    EditTaskInEditor(Task),
    SaveRepoPath(String),
    RefreshFromDb,
    QuickDispatch(TaskDraft),
    // Epic commands
    InsertEpic(EpicDraft),
    EditEpicInEditor(Epic),
    DeleteEpic(EpicId),
    PersistEpic { id: EpicId, done: Option<bool> },
    RefreshEpicsFromDb,
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
    // Epic input modes
    InputEpicTitle,
    InputEpicDescription,
    InputEpicRepoPath,
    ConfirmDeleteEpic,
    ConfirmArchiveEpic,
    // Overlay modes
    Help,
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
    pub epic_draft: Option<EpicDraft>,
}

impl Default for InputState {
    fn default() -> Self {
        Self {
            mode: InputMode::Normal,
            buffer: String::new(),
            task_draft: None,
            epic_draft: None,
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

// ---------------------------------------------------------------------------
// BoardSelection — column + row selection state for a kanban view
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct BoardSelection {
    pub(in crate::tui) selected_column: usize,
    pub(in crate::tui) selected_row: [usize; TaskStatus::COLUMN_COUNT],
}

impl BoardSelection {
    pub fn new() -> Self {
        Self {
            selected_column: 0,
            selected_row: [0; TaskStatus::COLUMN_COUNT],
        }
    }

    pub fn column(&self) -> usize {
        self.selected_column
    }

    pub fn row(&self, col: usize) -> usize {
        self.selected_row[col]
    }

    pub fn set_column(&mut self, col: usize) {
        self.selected_column = col;
    }

    pub fn set_row(&mut self, col: usize, row: usize) {
        self.selected_row[col] = row;
    }
}

impl Default for BoardSelection {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// ViewMode — board vs epic view with preserved selection state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum ViewMode {
    Board(BoardSelection),
    Epic {
        epic_id: EpicId,
        selection: BoardSelection,
        saved_board: BoardSelection,
    },
}

impl Default for ViewMode {
    fn default() -> Self {
        ViewMode::Board(BoardSelection::new())
    }
}

// ---------------------------------------------------------------------------
// ColumnItem — resolves whether cursor is on a task or an epic
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum ColumnItem<'a> {
    Task(&'a Task),
    Epic(&'a Epic),
}

// ---------------------------------------------------------------------------
// EpicDraft — fields collected during epic creation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct EpicDraft {
    pub title: String,
    pub description: String,
    pub repo_path: String,
}
