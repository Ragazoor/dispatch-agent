use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::{Duration, Instant};

use ratatui::widgets::ListState;

use crate::models::{
    AlertKind, AlertSeverity, Epic, EpicId, ReviewDecision, SecurityAlert, SubStatus, Task, TaskId,
    TaskStatus, TaskTag, TaskUsage,
};

// ---------------------------------------------------------------------------
// MoveDirection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveDirection {
    Forward,
    Backward,
}

// ---------------------------------------------------------------------------
// ReviewBoardMode
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewBoardMode {
    Reviewer,
    Author,
    Dependabot,
}

impl ReviewBoardMode {
    pub fn column_count(&self) -> usize {
        4
    }

    pub fn column_label(&self, col: usize) -> &'static str {
        match self {
            Self::Reviewer | Self::Author => match col {
                0 => "Needs Review",
                1 => "Waiting for Response",
                2 => "Changes Requested",
                3 => "Approved",
                _ => "",
            },
            Self::Dependabot => match col {
                0 => "CI Passing",
                1 => "CI Failing",
                2 => "CI Pending",
                3 => "Approved",
                _ => "",
            },
        }
    }

    pub fn pr_column(&self, pr: &crate::models::ReviewPr) -> usize {
        match self {
            Self::Reviewer | Self::Author => pr.review_decision.column_index(),
            Self::Dependabot => {
                if pr.review_decision == crate::models::ReviewDecision::Approved {
                    3
                } else {
                    pr.ci_status.column_index()
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ReviewAgentRequest
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ReviewAgentRequest {
    pub repo: String,
    pub number: i64,
    pub title: String,
    pub body: String,
    pub head_ref: String,
    pub is_dependabot: bool,
}

// ---------------------------------------------------------------------------
// RepoFilterMode
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RepoFilterMode {
    #[default]
    Include,
    Exclude,
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
    MoveTask {
        id: TaskId,
        direction: MoveDirection,
    },
    ReorderItem(isize), // +1 = down, -1 = up
    DispatchTask(TaskId),
    BrainstormTask(TaskId),
    PlanTask(TaskId),
    Dispatched {
        id: TaskId,
        worktree: String,
        tmux_window: String,
        switch_focus: bool,
    },
    TaskCreated {
        task: Task,
    },
    DeleteTask(TaskId),
    ToggleDetail,
    TmuxOutput {
        id: TaskId,
        output: String,
        activity_ts: u64,
    },
    WindowGone(TaskId),
    RefreshTasks(Vec<Task>),
    ResumeTask(TaskId),
    Resumed {
        id: TaskId,
        tmux_window: String,
    },
    Error(String),
    TaskEdited(TaskEdit),
    RepoPathsUpdated(Vec<String>),
    QuickDispatch {
        repo_path: String,
        epic_id: Option<EpicId>,
    },
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
    BatchMoveTasks {
        ids: Vec<TaskId>,
        direction: MoveDirection,
    },
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
    DescriptionEditorResult(String),
    SubmitRepoPath(String),
    SubmitTag(Option<TaskTag>),
    InputChar(char),
    InputBackspace,
    StartQuickDispatchSelection,
    SelectQuickDispatchRepo(usize),
    CancelRetry,
    StatusInfo(String),
    ToggleHelp,
    // Epic messages
    DispatchEpic(EpicId),
    AutoDispatchEpic(EpicId),
    EnterEpic(EpicId),
    ExitEpic,
    RefreshEpics(Vec<Epic>),
    RefreshUsage(Vec<TaskUsage>),
    EpicCreated(Epic),
    EditEpic(EpicId),
    EpicEdited(Epic),
    DeleteEpic(EpicId),
    ConfirmDeleteEpic,
    MoveEpicStatus(EpicId, MoveDirection),
    ArchiveEpic(EpicId),
    ConfirmArchiveEpic,
    StartNewEpic,
    SubmitEpicTitle(String),
    SubmitEpicDescription(String),
    SubmitEpicRepoPath(String),
    // Finish (rebase + cleanup)
    FinishComplete(TaskId),
    FinishFailed {
        id: TaskId,
        error: String,
        is_conflict: bool,
    },
    // PR flow
    PrCreated {
        id: TaskId,
        pr_url: String,
    },
    PrFailed {
        id: TaskId,
        error: String,
    },
    PrMerged(TaskId),
    StartMergePr(TaskId),
    ConfirmMergePr,
    CancelMergePr,
    MergePrFailed {
        id: TaskId,
        error: String,
    },
    PrReviewState {
        id: TaskId,
        review_decision: Option<crate::dispatch::PrReviewDecision>,
    },
    // Done confirmation (no cleanup, just status change)
    ConfirmDone,
    CancelDone,
    ToggleNotifications,
    // Review board
    SwitchToReviewBoard,
    SwitchToTaskBoard,
    ToggleReviewBoardMode,
    ReviewPrsLoaded(Vec<crate::models::ReviewPr>),
    ReviewPrsFetchFailed(String),
    MyPrsLoaded(Vec<crate::models::ReviewPr>),
    MyPrsFetchFailed(String),
    OpenInBrowser {
        url: String,
    },
    ToggleReviewDetail,
    RefreshReviewPrs,
    DispatchReviewAgent(ReviewAgentRequest),
    ReviewAgentDispatched {
        repo: String,
        number: i64,
        tmux_window: String,
    },
    ReviewAgentFailed {
        error: String,
    },
    // Repo filter
    StartRepoFilter,
    CloseRepoFilter,
    ToggleRepoFilter(String),
    ToggleAllRepoFilter,
    MoveRepoCursor(isize),
    ToggleRepoFilterMode,
    // Review repo filter
    StartReviewRepoFilter,
    CloseReviewRepoFilter,
    ToggleReviewRepoFilter(String),
    ToggleAllReviewRepoFilter,
    ToggleReviewRepoFilterMode,
    // Dispatch PR filter (My PRs tab)
    ToggleDispatchPrFilter,
    // Bot PRs (dependabot/renovate)
    BotPrsLoaded(Vec<crate::models::ReviewPr>),
    BotPrsFetchFailed(String),
    RefreshBotPrs,
    ToggleSelectBotPr(String),
    SelectAllBotPrColumn,
    ClearBotPrSelection,
    StartBatchApprove,
    StartBatchMerge,
    ConfirmBatchApprove,
    ConfirmBatchMerge,
    CancelBatchOperation,
    // Security board
    SwitchToSecurityBoard,
    SecurityAlertsLoaded(Vec<SecurityAlert>),
    SecurityAlertsFetchFailed(String),
    RefreshSecurityAlerts,
    ToggleSecurityDetail,
    ToggleSecurityKindFilter,
    StartSecurityRepoFilter,
    CloseSecurityRepoFilter,
    ToggleSecurityRepoFilter(String),
    ToggleAllSecurityRepoFilter,
    ToggleSecurityRepoFilterMode,
    DispatchFixAgent {
        repo: String,
        number: i64,
        kind: crate::models::AlertKind,
        title: String,
        description: String,
        package: Option<String>,
        fixed_version: Option<String>,
    },
    FixAgentDispatched {
        repo: String,
        number: i64,
        tmux_window: String,
    },
    FixAgentFailed {
        error: String,
    },
    // Filter presets
    StartSavePreset,
    SaveFilterPreset(String),
    LoadFilterPreset(String),
    StartDeletePreset,
    DeleteFilterPreset(String),
    CancelPresetInput,
    FilterPresetsLoaded(Vec<(String, HashSet<String>, RepoFilterMode)>),
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
    // Detach tmux panel (Review tasks only)
    DetachTmux(TaskId),
    BatchDetachTmux(Vec<TaskId>),
    ConfirmDetachTmux,
    // Inter-agent messaging
    MessageReceived(TaskId),
}

// ---------------------------------------------------------------------------
// Command
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Command {
    PersistTask(Task),
    InsertTask {
        draft: TaskDraft,
        epic_id: Option<EpicId>,
    },
    DeleteTask(TaskId),
    Dispatch {
        task: Task,
    },
    Brainstorm {
        task: Task,
    },
    Plan {
        task: Task,
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
        worktree: String,
        tmux_window: Option<String>,
    },
    CaptureTmux {
        id: TaskId,
        window: String,
    },
    Resume {
        task: Task,
    },
    JumpToTmux {
        window: String,
    },
    KillTmuxWindow {
        window: String,
    },
    EditTaskInEditor(Task),
    OpenDescriptionEditor { is_epic: bool },
    SaveRepoPath(String),
    RefreshFromDb,
    QuickDispatch {
        draft: TaskDraft,
        epic_id: Option<EpicId>,
    },
    // Epic commands
    DispatchEpic {
        epic: Epic,
    },
    InsertEpic(EpicDraft),
    EditEpicInEditor(Epic),
    DeleteEpic(EpicId),
    PersistEpic {
        id: EpicId,
        status: Option<TaskStatus>,
        sort_order: Option<i64>,
    },
    RefreshEpicsFromDb,
    SendNotification {
        title: String,
        body: String,
        urgent: bool,
    },
    PersistSetting {
        key: String,
        value: bool,
    },
    PersistStringSetting {
        key: String,
        value: String,
    },
    PersistFilterPreset {
        name: String,
        repo_paths: String,
        mode: RepoFilterMode,
    },
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
    MergePr {
        id: TaskId,
        pr_url: String,
    },
    FetchReviewPrs,
    PersistReviewPrs(Vec<crate::models::ReviewPr>),
    FetchMyPrs,
    PersistMyPrs(Vec<crate::models::ReviewPr>),
    FetchBotPrs,
    PersistBotPrs(Vec<crate::models::ReviewPr>),
    BatchApprovePrs(Vec<String>),
    BatchMergePrs(Vec<String>),
    OpenInBrowser {
        url: String,
    },
    PatchSubStatus {
        id: TaskId,
        sub_status: SubStatus,
    },
    DispatchReviewAgent(ReviewAgentRequest),
    FetchSecurityAlerts,
    PersistSecurityAlerts(Vec<SecurityAlert>),
    DispatchFixAgent {
        repo: String,
        number: i64,
        kind: crate::models::AlertKind,
        title: String,
        description: String,
        package: Option<String>,
        fixed_version: Option<String>,
    },
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
    ConfirmMergePr(TaskId),
    ConfirmWrapUp(TaskId),
    ConfirmDetachTmux(Vec<TaskId>),
    // Epic input modes
    InputEpicTitle,
    InputEpicDescription,
    InputEpicRepoPath,
    ConfirmDeleteEpic,
    ConfirmArchiveEpic,
    ConfirmEpicWrapUp(EpicId),
    // Overlay modes
    Help,
    RepoFilter,
    ReviewRepoFilter,
    SecurityRepoFilter,
    InputPresetName,
    ConfirmDeletePreset,
    ConfirmEditTask(TaskId),
    // Dependabot batch operations
    ConfirmBatchApprove(Vec<String>),
    ConfirmBatchMerge(Vec<String>),
    ConfirmQuit,
}

// ---------------------------------------------------------------------------
// TaskDraft
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct TaskDraft {
    pub title: String,
    pub description: String,
    pub repo_path: String,
    pub tag: Option<TaskTag>,
}

// ---------------------------------------------------------------------------
// AgentTracking — tmux output and health state for dispatched agents
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct AgentTracking {
    pub tmux_outputs: HashMap<TaskId, String>,
    pub last_output_change: HashMap<TaskId, Instant>,
    pub last_activity: HashMap<TaskId, u64>,
    pub inactivity_timeout: Duration,
    pub notified_review: HashSet<TaskId>,
    pub notified_needs_input: HashSet<TaskId>,
    pub auto_dispatched_epics: HashSet<EpicId>,
    pub last_pr_poll: HashMap<TaskId, Instant>,
    pub message_flash: HashMap<TaskId, Instant>,
}

impl AgentTracking {
    pub fn new(inactivity_timeout: Duration) -> Self {
        Self {
            tmux_outputs: HashMap::new(),
            last_output_change: HashMap::new(),
            last_activity: HashMap::new(),
            inactivity_timeout,
            notified_review: HashSet::new(),
            notified_needs_input: HashSet::new(),
            auto_dispatched_epics: HashSet::new(),
            last_pr_poll: HashMap::new(),
            message_flash: HashMap::new(),
        }
    }

    /// Remove all tracking state for a task.
    pub fn clear(&mut self, id: TaskId) {
        self.last_output_change.remove(&id);
        self.last_activity.remove(&id);
        self.tmux_outputs.remove(&id);
        self.notified_review.remove(&id);
        self.notified_needs_input.remove(&id);
        self.last_pr_poll.remove(&id);
        self.message_flash.remove(&id);
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
    pub repo_cursor: usize,
    /// Tracks epic_id during quick-dispatch repo selection in epic view.
    pub pending_epic_id: Option<EpicId>,
}

impl Default for InputState {
    fn default() -> Self {
        Self {
            mode: InputMode::Normal,
            buffer: String::new(),
            task_draft: None,
            epic_draft: None,
            repo_cursor: 0,
            pending_epic_id: None,
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
// SelectionState — multi-select state for batch operations
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct SelectionState {
    pub tasks: HashSet<TaskId>,
    pub epics: HashSet<EpicId>,
    pub bot_prs: HashSet<String>,
    pub pending_done: Vec<TaskId>,
}

impl SelectionState {
    pub fn has_selection(&self) -> bool {
        !self.tasks.is_empty() || !self.epics.is_empty()
    }

    pub fn has_bot_pr_selection(&self) -> bool {
        !self.bot_prs.is_empty()
    }

    pub fn clear(&mut self) {
        self.tasks.clear();
        self.epics.clear();
    }
}

// ---------------------------------------------------------------------------
// FilterState — repo filter and presets for the task board
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct FilterState {
    pub repos: HashSet<String>,
    pub mode: RepoFilterMode,
    pub presets: Vec<(String, HashSet<String>, RepoFilterMode)>,
}

impl FilterState {
    pub fn matches(&self, repo_path: &str) -> bool {
        if self.repos.is_empty() {
            return true;
        }
        match self.mode {
            RepoFilterMode::Include => self.repos.contains(repo_path),
            RepoFilterMode::Exclude => !self.repos.contains(repo_path),
        }
    }
}

// ---------------------------------------------------------------------------
// ReviewBoardState — review board data and loading state
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct ReviewBoardState {
    pub prs: Vec<crate::models::ReviewPr>,
    pub repos: Vec<String>,
    pub loading: bool,
    pub last_fetch: Option<Instant>,
    pub last_error: Option<String>,
    pub detail_visible: bool,
    pub repo_filter: HashSet<String>,
    pub repo_filter_mode: RepoFilterMode,
    pub my_prs: Vec<crate::models::ReviewPr>,
    pub my_prs_repos: Vec<String>,
    pub my_prs_loading: bool,
    pub last_my_prs_fetch: Option<Instant>,
    pub my_prs_repo_filter: HashSet<String>,
    pub my_prs_repo_filter_mode: RepoFilterMode,
    pub dispatch_pr_filter: bool,
    // Bot (dependabot/renovate) PRs
    pub bot_prs: Vec<crate::models::ReviewPr>,
    pub bot_prs_repos: Vec<String>,
    pub bot_prs_loading: bool,
    pub last_bot_prs_fetch: Option<Instant>,
    pub bot_prs_repo_filter: HashSet<String>,
    pub bot_prs_repo_filter_mode: RepoFilterMode,
}

impl ReviewBoardState {
    /// Set review PRs and rebuild the cached distinct repos list.
    pub fn set_prs(&mut self, prs: Vec<crate::models::ReviewPr>) {
        self.repos = distinct_repos(&prs);
        self.prs = prs;
    }

    /// Set author PRs and rebuild the cached distinct repos list.
    pub fn set_my_prs(&mut self, prs: Vec<crate::models::ReviewPr>) {
        self.my_prs_repos = distinct_repos(&prs);
        self.my_prs = prs;
    }

    /// Return review PRs filtered by repo filter. Empty filter means all PRs.
    pub fn filtered_prs(&self) -> Vec<&crate::models::ReviewPr> {
        self.prs
            .iter()
            .filter(|pr| self.repo_matches(&pr.repo))
            .collect()
    }

    /// Return author's PRs filtered by repo filter. Empty filter means all PRs.
    pub fn filtered_my_prs(&self) -> Vec<&crate::models::ReviewPr> {
        self.my_prs
            .iter()
            .filter(|pr| self.my_prs_repo_matches(&pr.repo))
            .collect()
    }

    pub fn repo_matches(&self, repo: &str) -> bool {
        if self.repo_filter.is_empty() {
            return true;
        }
        match self.repo_filter_mode {
            RepoFilterMode::Include => self.repo_filter.contains(repo),
            RepoFilterMode::Exclude => !self.repo_filter.contains(repo),
        }
    }

    pub fn my_prs_repo_matches(&self, repo: &str) -> bool {
        if self.my_prs_repo_filter.is_empty() {
            return true;
        }
        match self.my_prs_repo_filter_mode {
            RepoFilterMode::Include => self.my_prs_repo_filter.contains(repo),
            RepoFilterMode::Exclude => !self.my_prs_repo_filter.contains(repo),
        }
    }

    /// Whether review PRs need a refresh given the interval.
    pub fn needs_fetch(&self, interval: Duration) -> bool {
        self.last_fetch
            .map(|t| t.elapsed() > interval)
            .unwrap_or(true)
    }

    /// Whether author PRs need a refresh given the interval.
    pub fn needs_my_prs_fetch(&self, interval: Duration) -> bool {
        self.last_my_prs_fetch
            .map(|t| t.elapsed() > interval)
            .unwrap_or(true)
    }

    /// Set bot PRs and rebuild the cached distinct repos list.
    pub fn set_bot_prs(&mut self, prs: Vec<crate::models::ReviewPr>) {
        self.bot_prs_repos = distinct_repos(&prs);
        self.bot_prs = prs;
    }

    /// Return bot PRs filtered by repo filter. Empty filter means all PRs.
    pub fn filtered_bot_prs(&self) -> Vec<&crate::models::ReviewPr> {
        self.bot_prs
            .iter()
            .filter(|pr| self.bot_prs_repo_matches(&pr.repo))
            .collect()
    }

    pub fn bot_prs_repo_matches(&self, repo: &str) -> bool {
        if self.bot_prs_repo_filter.is_empty() {
            return true;
        }
        match self.bot_prs_repo_filter_mode {
            RepoFilterMode::Include => self.bot_prs_repo_filter.contains(repo),
            RepoFilterMode::Exclude => !self.bot_prs_repo_filter.contains(repo),
        }
    }

    /// Whether bot PRs need a refresh given the interval.
    pub fn needs_bot_prs_fetch(&self, interval: Duration) -> bool {
        self.last_bot_prs_fetch
            .map(|t| t.elapsed() > interval)
            .unwrap_or(true)
    }
}

/// Compute a sorted, deduplicated list of repo names from a slice of review PRs.
fn distinct_repos(prs: &[crate::models::ReviewPr]) -> Vec<String> {
    prs.iter()
        .map(|pr| pr.repo.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
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
    pub tag: Option<TaskTag>,
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
        if self.on_select_all {
            None
        } else {
            Some(self.selected_row[col])
        }
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
// SecurityBoardSelection — column + row selection state for security board
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SecurityBoardSelection {
    pub(in crate::tui) selected_column: usize,
    pub(in crate::tui) selected_row: [usize; AlertSeverity::COLUMN_COUNT],
    pub(in crate::tui) list_states: [ListState; AlertSeverity::COLUMN_COUNT],
}

impl SecurityBoardSelection {
    pub fn new() -> Self {
        Self {
            selected_column: 0,
            selected_row: [0; AlertSeverity::COLUMN_COUNT],
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

impl Default for SecurityBoardSelection {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// SecurityBoardState — security board data and loading state
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct SecurityBoardState {
    pub alerts: Vec<SecurityAlert>,
    pub repos: Vec<String>,
    pub loading: bool,
    pub last_fetch: Option<Instant>,
    pub last_error: Option<String>,
    pub detail_visible: bool,
    pub repo_filter: HashSet<String>,
    pub repo_filter_mode: RepoFilterMode,
    pub kind_filter: Option<AlertKind>,
}

impl SecurityBoardState {
    /// Set alerts and rebuild the cached distinct repos list.
    pub fn set_alerts(&mut self, alerts: Vec<SecurityAlert>) {
        self.repos = {
            let mut set = BTreeSet::new();
            for a in &alerts {
                set.insert(a.repo.clone());
            }
            set.into_iter().collect()
        };
        self.alerts = alerts;
    }

    /// Return alerts filtered by repo filter and kind filter.
    pub fn filtered_alerts(&self) -> Vec<&SecurityAlert> {
        self.alerts
            .iter()
            .filter(|a| self.repo_matches(&a.repo))
            .filter(|a| self.kind_filter.is_none() || self.kind_filter == Some(a.kind))
            .collect()
    }

    pub fn repo_matches(&self, repo: &str) -> bool {
        if self.repo_filter.is_empty() {
            return true;
        }
        match self.repo_filter_mode {
            RepoFilterMode::Include => self.repo_filter.contains(repo),
            RepoFilterMode::Exclude => !self.repo_filter.contains(repo),
        }
    }

    /// Whether alerts need a refresh given the interval.
    pub fn needs_fetch(&self, interval: Duration) -> bool {
        self.last_fetch
            .map(|t| t.elapsed() > interval)
            .unwrap_or(true)
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
        mode: ReviewBoardMode,
        selection: ReviewBoardSelection,
        saved_board: BoardSelection,
    },
    SecurityBoard {
        selection: SecurityBoardSelection,
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
// ColumnLayout — pre-computed column items for one render frame
// ---------------------------------------------------------------------------

/// Pre-computed column items for one render frame.
/// Built once at the top of `render()` to avoid recomputing per widget.
pub struct ColumnLayout<'a> {
    columns: [Vec<ColumnItem<'a>>; TaskStatus::COLUMN_COUNT],
}

impl<'a> ColumnLayout<'a> {
    pub fn build(app: &'a super::App) -> Self {
        let columns = std::array::from_fn(|i| {
            let status = TaskStatus::ALL[i];
            app.column_items_for_status(status)
        });
        ColumnLayout { columns }
    }

    pub fn get(&self, status: TaskStatus) -> &[ColumnItem<'a>] {
        &self.columns[status.column_index()]
    }

    pub fn count(&self, status: TaskStatus) -> usize {
        self.columns[status.column_index()].len()
    }
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
