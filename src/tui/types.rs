use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use ratatui::widgets::ListState;

use crate::models::{Epic, EpicId, ReviewDecision, Task, TaskId, TaskStatus, TaskUsage};

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
    ReorderItem(isize),  // +1 = down, -1 = up
    DispatchTask(TaskId),
    BrainstormTask(TaskId),
    PlanTask(TaskId),
    Dispatched { id: TaskId, worktree: String, tmux_window: String, switch_focus: bool },
    TaskCreated { task: Task },
    DeleteTask(TaskId),
    ToggleDetail,
    TmuxOutput { id: TaskId, output: String, activity_ts: u64 },
    WindowGone(TaskId),
    RefreshTasks(Vec<Task>),
    ResumeTask(TaskId),
    Resumed { id: TaskId, tmux_window: String },
    Error(String),
    TaskEdited(TaskEdit),
    RepoPathsUpdated(Vec<String>),
    QuickDispatch { repo_path: String, epic_id: Option<EpicId> },
    StaleAgent(TaskId),
    AgentCrashed(TaskId),
    KillAndRetry(TaskId),
    RetryResume(TaskId),
    RetryFresh(TaskId),
    ArchiveTask(TaskId),
    ToggleArchive,
    ToggleSelect(TaskId),
    ToggleSelectEpic(EpicId),
    ClearSelection,
    SelectAllColumn,
    BatchMoveTasks { ids: Vec<TaskId>, direction: MoveDirection },
    BatchArchiveTasks(Vec<TaskId>),
    BatchArchiveEpics(Vec<EpicId>),
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
    SubmitTag(Option<String>),
    InputChar(char),
    InputBackspace,
    StartQuickDispatchSelection,
    SelectQuickDispatchRepo(usize),
    CancelRetry,
    StatusInfo(String),
    ToggleHelp,
    // Epic messages
    DispatchEpic(EpicId),
    EnterEpic(EpicId),
    ExitEpic,
    RefreshEpics(Vec<Epic>),
    RefreshUsage(Vec<TaskUsage>),
    EpicCreated(Epic),
    EditEpic(EpicId),
    EpicEdited(Epic),
    DeleteEpic(EpicId),
    ConfirmDeleteEpic,
    MarkEpicDone(EpicId),
    MarkEpicUndone(EpicId),
    ConfirmEpicDone,
    CancelEpicDone,
    ArchiveEpic(EpicId),
    ConfirmArchiveEpic,
    StartNewEpic,
    SubmitEpicTitle(String),
    SubmitEpicDescription(String),
    SubmitEpicRepoPath(String),
    // Finish (rebase + cleanup)
    FinishComplete(TaskId),
    FinishFailed { id: TaskId, error: String, is_conflict: bool },
    // PR flow
    PrCreated { id: TaskId, pr_url: String },
    PrFailed { id: TaskId, error: String },
    PrMerged(TaskId),
    // Done confirmation (no cleanup, just status change)
    ConfirmDone,
    CancelDone,
    ToggleNotifications,
    // Review board
    SwitchToReviewBoard,
    SwitchToTaskBoard,
    ReviewPrsLoaded(Vec<crate::models::ReviewPr>),
    ReviewPrsFetchFailed(String),
    OpenInBrowser { url: String },
    RefreshReviewPrs,
    // Repo filter
    StartRepoFilter,
    CloseRepoFilter,
    ToggleRepoFilter(String),
    ToggleAllRepoFilter,
    // Filter presets
    StartSavePreset,
    SaveFilterPreset(String),
    LoadFilterPreset(String),
    StartDeletePreset,
    DeleteFilterPreset(String),
    CancelPresetInput,
    FilterPresetsLoaded(Vec<(String, HashSet<String>)>),
    // Wrap up (replaces finish + PR)
    StartWrapUp(TaskId),
    WrapUpRebase,
    WrapUpPr,
    CancelWrapUp,
    // Epic batch wrap-up
    StartEpicWrapUp(EpicId),
    EpicWrapUpRebase,
    EpicWrapUpPr,
    CancelEpicWrapUp,
    CancelMergeQueue,
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
    Plan { task: Task },
    Cleanup { id: TaskId, repo_path: String, worktree: String, tmux_window: Option<String> },
    Finish {
        id: TaskId,
        repo_path: String,
        branch: String,
        worktree: String,
        tmux_window: Option<String>,
    },
    CaptureTmux { id: TaskId, window: String },
    Resume { task: Task },
    JumpToTmux { window: String },
    KillTmuxWindow { window: String },
    EditTaskInEditor(Task),
    SaveRepoPath(String),
    RefreshFromDb,
    QuickDispatch { draft: TaskDraft, epic_id: Option<EpicId> },
    // Epic commands
    DispatchEpic { epic: Epic },
    InsertEpic(EpicDraft),
    EditEpicInEditor(Epic),
    DeleteEpic(EpicId),
    PersistEpic { id: EpicId, done: Option<bool>, sort_order: Option<i64> },
    RefreshEpicsFromDb,
    SendNotification { title: String, body: String, urgent: bool },
    PersistSetting { key: String, value: bool },
    PersistStringSetting { key: String, value: String },
    PersistFilterPreset { name: String, repo_paths: String },
    DeleteFilterPreset(String),
    CreatePr {
        id: TaskId,
        repo_path: String,
        branch: String,
        title: String,
        description: String,
    },
    CheckPrStatus {
        id: TaskId,
        pr_url: String,
    },
    FetchReviewPrs,
    OpenInBrowser { url: String },
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
    InputTag,
    ConfirmDelete,
    QuickDispatch,
    ConfirmRetry(TaskId),
    ConfirmArchive,
    ConfirmDone(TaskId),
    ConfirmWrapUp(TaskId),
    // Epic input modes
    InputEpicTitle,
    InputEpicDescription,
    InputEpicRepoPath,
    ConfirmDeleteEpic,
    ConfirmArchiveEpic,
    ConfirmEpicDone(EpicId),
    ConfirmEpicWrapUp(EpicId),
    // Overlay modes
    Help,
    RepoFilter,
    InputPresetName,
    ConfirmDeletePreset,
}

// ---------------------------------------------------------------------------
// TaskDraft
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct TaskDraft {
    pub title: String,
    pub description: String,
    pub repo_path: String,
    pub tag: Option<String>,
}

// ---------------------------------------------------------------------------
// AgentTracking — tmux output and health state for dispatched agents
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct AgentTracking {
    pub tmux_outputs: HashMap<TaskId, String>,
    pub last_output_change: HashMap<TaskId, Instant>,
    pub last_activity: HashMap<TaskId, u64>,
    pub stale_tasks: HashSet<TaskId>,
    pub crashed_tasks: HashSet<TaskId>,
    pub inactivity_timeout: Duration,
    pub notified_review: HashSet<TaskId>,
    pub notified_needs_input: HashSet<TaskId>,
    pub last_pr_poll: HashMap<TaskId, Instant>,
}

impl AgentTracking {
    pub fn new(inactivity_timeout: Duration) -> Self {
        Self {
            tmux_outputs: HashMap::new(),
            last_output_change: HashMap::new(),
            last_activity: HashMap::new(),
            stale_tasks: HashSet::new(),
            crashed_tasks: HashSet::new(),
            inactivity_timeout,
            notified_review: HashSet::new(),
            notified_needs_input: HashSet::new(),
            last_pr_poll: HashMap::new(),
        }
    }

    /// Remove all tracking state for a task.
    pub fn clear(&mut self, id: TaskId) {
        self.last_output_change.remove(&id);
        self.last_activity.remove(&id);
        self.stale_tasks.remove(&id);
        self.crashed_tasks.remove(&id);
        self.tmux_outputs.remove(&id);
        self.notified_review.remove(&id);
        self.notified_needs_input.remove(&id);
        self.last_pr_poll.remove(&id);
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
    pub list_state: ListState,
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
    pub tag: Option<String>,
}

// ---------------------------------------------------------------------------
// BoardSelection — column + row selection state for a kanban view
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct BoardSelection {
    pub(in crate::tui) selected_column: usize,
    pub(in crate::tui) selected_row: [usize; TaskStatus::COLUMN_COUNT],
    pub(in crate::tui) on_select_all: bool,
    pub(in crate::tui) list_states: [ListState; TaskStatus::COLUMN_COUNT],
}

impl BoardSelection {
    pub fn new() -> Self {
        Self {
            selected_column: 0,
            selected_row: [0; TaskStatus::COLUMN_COUNT],
            on_select_all: false,
            list_states: std::array::from_fn(|_| ListState::default()),
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

    /// List item index for `ListState` scrolling.
    /// Returns `None` when the cursor is on the select-all toggle (header),
    /// since no list item should be selected in that case.
    pub fn list_state_index(&self, col: usize) -> Option<usize> {
        if self.on_select_all { None } else { Some(self.selected_row[col]) }
    }
}

impl Default for BoardSelection {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// ReviewBoardSelection — column + row selection state for review board
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ReviewBoardSelection {
    pub(in crate::tui) selected_column: usize,
    pub(in crate::tui) selected_row: [usize; ReviewDecision::COLUMN_COUNT],
    pub(in crate::tui) list_states: [ListState; ReviewDecision::COLUMN_COUNT],
}

impl ReviewBoardSelection {
    pub fn new() -> Self {
        Self {
            selected_column: 0,
            selected_row: [0; ReviewDecision::COLUMN_COUNT],
            list_states: std::array::from_fn(|_| ListState::default()),
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

    pub fn list_state_index(&self, col: usize) -> Option<usize> {
        Some(self.selected_row[col])
    }
}

impl Default for ReviewBoardSelection {
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
    ReviewBoard {
        selection: ReviewBoardSelection,
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

// ---------------------------------------------------------------------------
// MergeQueue — state for batch epic wrap-up
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeAction {
    Rebase,
    Pr,
}

#[derive(Debug, Clone)]
pub struct MergeQueue {
    pub epic_id: EpicId,
    pub action: MergeAction,
    pub task_ids: Vec<TaskId>,
    pub completed: usize,
    pub current: Option<TaskId>,
    pub failed: Option<TaskId>,
}
