use std::collections::{HashMap, HashSet};
use std::time::Instant;

/// Sentinel identifier for the "no parent" option in the reparent tree picker.
pub(in crate::tui) const REPARENT_NO_PARENT_SENTINEL: &str = "__no_parent__";

use ratatui::widgets::ListState;

use crate::models::{
    DispatchMode, Epic, EpicId, EpicSubstatus, Task, TaskId, TaskStatus, TaskTag, TodoId,
    WrapUpMode, DEFAULT_BASE_BRANCH,
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
// RepoFilterMode
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RepoFilterMode {
    #[default]
    Include,
    Exclude,
}

impl RepoFilterMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            RepoFilterMode::Include => "include",
            RepoFilterMode::Exclude => "exclude",
        }
    }
}

impl std::str::FromStr for RepoFilterMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "include" => Ok(RepoFilterMode::Include),
            "exclude" => Ok(RepoFilterMode::Exclude),
            _ => Err(format!("unknown filter mode: {s}")),
        }
    }
}

// ---------------------------------------------------------------------------
// EditKind / EditorOutcome — tags for the pop-out editor flow
// ---------------------------------------------------------------------------

/// Identifies what the user is editing and how to finalize the edit when
/// the pop-out editor closes. One variant per existing $EDITOR call-site.
#[derive(Debug, Clone)]
pub enum EditKind {
    /// Full task editor (title/description/repo_path/status/plan/tag/base_branch).
    /// Boxed: `Task` has grown large enough (peer-message + live-shell
    /// tracking fields) that clippy's `large_enum_variant` flags the
    /// unboxed size difference against `EpicEdit`/`Description`.
    TaskEdit(Box<Task>),
    /// Full epic editor (title/description/repo_path).
    EpicEdit(Epic),
    /// Description-only editor used during task/epic creation.
    /// `is_epic` distinguishes the epic-create flow from the task-create flow.
    Description { is_epic: bool },
}

/// Result of a pop-out editor session. `Saved` carries the final tempfile
/// contents; `Cancelled` means the editor closed without a readable result
/// (e.g. the tempfile disappeared, or the tmux window was killed while the
/// editor buffer was empty).
#[derive(Debug, Clone)]
pub enum EditorOutcome {
    Saved(String),
    Cancelled,
}

// ---------------------------------------------------------------------------
// TreeNav — directional navigation within the tree view
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub enum TreeNav {
    Up,
    Down,
    Left,
    Right,
}

/// Apply a `TreeNav` direction to a `TreeState`. Used by the reparent-epic
/// picker and the move-to-epic tree picker.
pub(in crate::tui) fn apply_tree_nav<Id: Clone + PartialEq + Eq + std::hash::Hash>(
    state: &mut tui_tree_widget::TreeState<Id>,
    nav: TreeNav,
) {
    match nav {
        TreeNav::Up => {
            state.key_up();
        }
        TreeNav::Down => {
            state.key_down();
        }
        TreeNav::Left => {
            state.key_left();
        }
        TreeNav::Right => {
            state.key_right();
        }
    }
}

// ---------------------------------------------------------------------------
// Message
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Message {
    /// System-level messages — see [`crate::tui::messages::SystemMessage`].
    System(crate::tui::messages::SystemMessage),
    /// Task-domain messages — see [`crate::tui::messages::TaskMessage`].
    Task(crate::tui::messages::TaskMessage),
    NavigateColumn(isize),
    NavigateRow(isize),
    NavigateRowFirst,
    NavigateRowLast,
    RepoPathsUpdated(Vec<String>),
    /// Full-board reload of per-repo base_branch history, keyed by repo_path
    /// (see docs/specs/dispatch.allium: surface BaseBranchPicker).
    BaseBranchesUpdated(std::collections::HashMap<String, Vec<String>>),
    ClearSelection,
    SelectAllColumn,
    /// Form-input flow messages — see [`crate::tui::messages::InputMessage`].
    Input(crate::tui::messages::InputMessage),
    /// Pop-out `$EDITOR` flow messages — see
    /// [`crate::tui::messages::EditorMessage`].
    Editor(crate::tui::messages::EditorMessage),
    /// Split-pane mode messages — see [`crate::tui::messages::SplitMessage`].
    Split(crate::tui::messages::SplitMessage),
    /// Epic-domain messages — see [`crate::tui::messages::EpicMessage`].
    Epic(crate::tui::messages::EpicMessage),
    /// PR flow messages — see [`crate::tui::messages::PrMessage`].
    Pr(crate::tui::messages::PrMessage),
    /// Repo-filter overlay messages — see [`crate::tui::messages::RepoFilterMessage`].
    RepoFilter(crate::tui::messages::RepoFilterMessage),
    /// Local-first repo sync messages — see
    /// [`crate::tui::messages::RepoSyncMessage`].
    RepoSync(crate::tui::messages::RepoSyncMessage),
    /// Personal TODO overlay messages — see [`crate::tui::messages::TodoMessage`].
    Todo(crate::tui::messages::TodoMessage),
    /// Feed-epic refresh messages — see [`crate::tui::messages::FeedMessage`].
    Feed(crate::tui::messages::FeedMessage),
    /// Main session setup messages — see [`crate::tui::messages::MainSessionMessage`].
    MainSession(crate::tui::messages::MainSessionMessage),
    /// Budget-indicator messages — see [`crate::tui::messages::BudgetMessage`].
    Budget(crate::tui::messages::BudgetMessage),
}

// ---------------------------------------------------------------------------
// Command
// ---------------------------------------------------------------------------

/// Side effects the runtime executes on the update loop's behalf.
///
/// A pure router: every variant wraps exactly one per-domain inner enum from
/// [`crate::tui::commands`], with no payload of its own. The migration that
/// established that shape is complete — adding a new inline variant here
/// instead of a variant on (or a new module beside) one of the inner enums
/// reintroduces the half-done split it was done to remove.
#[derive(Debug, Clone)]
pub enum Command {
    /// Task-domain side-effect commands — see
    /// [`crate::tui::commands::TaskCommand`].
    Task(crate::tui::commands::TaskCommand),
    /// Split-pane mode side-effect commands — see [`crate::tui::commands::SplitCommand`].
    Split(crate::tui::commands::SplitCommand),
    /// Pop-out `$EDITOR` flow side-effect commands — see
    /// [`crate::tui::commands::EditorCommand`].
    Editor(crate::tui::commands::EditorCommand),
    /// Settings/preference-persistence side-effect commands — see
    /// [`crate::tui::commands::SettingsCommand`].
    Settings(crate::tui::commands::SettingsCommand),
    /// Epic-domain side-effect commands — see
    /// [`crate::tui::commands::EpicCommand`].
    Epic(crate::tui::commands::EpicCommand),
    /// Feed-epic refresh side-effect commands — see
    /// [`crate::tui::commands::FeedCommand`].
    Feed(crate::tui::commands::FeedCommand),
    /// System-level side-effect commands — see
    /// [`crate::tui::commands::SystemCommand`].
    System(crate::tui::commands::SystemCommand),
    /// Repo-filter overlay side-effect commands — see [`crate::tui::commands::RepoFilterCommand`].
    RepoFilter(crate::tui::commands::RepoFilterCommand),
    /// Local-first repo sync side-effect commands — see
    /// [`crate::tui::commands::RepoSyncCommand`].
    RepoSync(crate::tui::commands::RepoSyncCommand),
    /// PR flow side-effect commands — see [`crate::tui::commands::PrCommand`].
    Pr(crate::tui::commands::PrCommand),
    /// Main session side-effect commands — see [`crate::tui::commands::MainSessionCommand`].
    MainSession(crate::tui::commands::MainSessionCommand),
    /// Background learning-maintenance side-effect commands — see
    /// [`crate::tui::commands::LearningCommand`].
    Learning(crate::tui::commands::LearningCommand),
    /// Personal TODO overlay side-effect commands — see
    /// [`crate::tui::commands::TodoCommand`].
    Todo(crate::tui::commands::TodoCommand),
    /// Usage-telemetry side-effect commands — see
    /// [`crate::tui::commands::UsageCommand`].
    Usage(crate::tui::commands::UsageCommand),
    /// Budget-indicator side-effect commands — see
    /// [`crate::tui::commands::BudgetCommand`].
    Budget(crate::tui::commands::BudgetCommand),
}

// ---------------------------------------------------------------------------
// InputMode
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    /// Live task-search bar (title or task id). The board filters in place as
    /// the user types.
    SearchTasks,
    InputTitle,
    InputDescription,
    InputRepoPath,
    InputTag,
    ConfirmDelete,
    QuickDispatch,
    ConfirmRetry(TaskId),
    /// `Some(id)` = single-task archive (ID captured when 'x' was pressed).
    /// `None` = batch archive (uses the current multi-selection set).
    ConfirmArchive(Option<TaskId>),
    /// Review → Done confirmation. The tasks awaiting confirmation live in
    /// `select.pending_done` (one entry for a single move, N for a batch),
    /// which is why the variant carries no payload.
    ConfirmDone,
    ConfirmDetachTmux(Vec<TaskId>),
    // Epic input modes
    InputEpicTitle,
    InputEpicDescription,
    ConfirmDeleteEpic,
    ConfirmArchiveEpic,
    ReparentEpic(EpicId),
    ConfirmReparentEpic {
        epic_id: EpicId,
        new_parent: Option<EpicId>,
    },
    // Move-task-to-epic tree picker (the `m` key on a task card)
    MoveTaskToEpic(TaskId),
    ConfirmMoveTaskToEpic {
        task_id: TaskId,
        new_epic: Option<EpicId>,
    },
    // Overlay modes
    Help,
    RepoFilter,
    InputPresetName,
    ConfirmDeletePreset,
    ConfirmDeleteRepoPath,
    ConfirmQuit,
    ConfirmTrustRepo {
        task_id: TaskId,
        mode: DispatchMode,
    },
    /// Quick-dispatch's equivalent of `ConfirmTrustRepo`: entered when
    /// `TaskCommand::QuickDispatch`'s trust check finds the repo untrusted.
    /// No `Task`/`TaskId` exists yet at this point, so the pending draft is
    /// carried directly instead.
    ConfirmTrustRepoQuickDispatch {
        draft: TaskDraft,
        epic_id: Option<EpicId>,
    },
    InputBaseBranch,
    InputWrapUpMode,
    MainSessionDir,
    /// In-view title input for adding or editing a personal TODO item.
    TodoTitle,
    /// Board quick-add input for personal TODOs.
    TodoQuickAdd,
    /// Confirmation prompt for deleting a personal TODO item.
    ConfirmDeleteTodo,
    /// Board-pick mode: user browses the board to link this todo to a task/epic.
    LinkTodoToTask(TodoId),
    /// Sync confirmation for one repository (docs/specs/repo-sync.allium:
    /// surface RepoSyncConfirmation). Carries only the repo path: the
    /// measurement itself is re-read from `App.repo_sync` at confirm time, so a
    /// refresh that lands while the prompt is open cannot be acted on stale.
    ConfirmRepoSync {
        repo_path: String,
    },
}

impl InputMode {
    /// The repo-picker modes whose filtered list depends on the query, so any
    /// query edit must reset the list cursor to 0 (per RepoPathPicker in
    /// dispatch.allium). `InputBaseBranch` shares this cursor-reset-on-type
    /// contract (per BaseBranchPicker) even though its candidate list is a
    /// per-repo branch history rather than the global repo-path set — see
    /// `handle_move_repo_cursor` and the Enter-selection branch in
    /// `handle_key_text_input`, which special-case it for candidate lookup.
    pub fn is_repo_picker(&self) -> bool {
        matches!(
            self,
            InputMode::InputRepoPath
                | InputMode::MainSessionDir
                | InputMode::QuickDispatch
                | InputMode::InputBaseBranch
        )
    }
}

// ---------------------------------------------------------------------------
// TaskDraft
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDraft {
    pub title: String,
    pub description: String,
    pub repo_path: String,
    pub tag: Option<TaskTag>,
    pub base_branch: String,
    pub wrap_up_mode: Option<WrapUpMode>,
}

impl Default for TaskDraft {
    fn default() -> Self {
        Self {
            title: String::new(),
            description: String::new(),
            repo_path: String::new(),
            tag: None,
            base_branch: DEFAULT_BASE_BRANCH.to_string(),
            wrap_up_mode: None,
        }
    }
}

// ---------------------------------------------------------------------------
// BoardState — tasks, epics, view mode, and related board data
// ---------------------------------------------------------------------------

pub struct BoardState {
    pub(in crate::tui) tasks: Vec<Task>,
    pub(in crate::tui) epics: Vec<Epic>,
    pub(in crate::tui) view_mode: ViewMode,
    pub(in crate::tui) repo_paths: Vec<String>,
    /// Per-repo most-recently-used base_branch history, keyed by repo_path,
    /// each list ordered most-recent-first (see docs/specs/dispatch.allium:
    /// surface BaseBranchPicker).
    pub(in crate::tui) repo_base_branches: std::collections::HashMap<String, Vec<String>>,
    pub(in crate::tui) split: SplitState,
    /// Flattened rendering mode: when true, epic cards are hidden and every
    /// descendant task of the current view surfaces directly in its status
    /// column. Preserved across navigation, session-scoped.
    pub(in crate::tui) flattened: bool,
    /// Count of open (not-done) personal TODO items, shown in the board footer.
    /// Updated whenever the Todos view is opened or mutated.
    pub(in crate::tui) todo_open_count: i64,
}

// ---------------------------------------------------------------------------
// StatusState — transient status messages and error popups
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct StatusState {
    pub(in crate::tui) message: Option<String>,
    pub(in crate::tui) message_set_at: Option<Instant>,
    pub(in crate::tui) error_popup: Option<String>,
    /// When true, the status message survives the [`STATUS_MESSAGE_TTL`]
    /// auto-clear in `handle_tick`. Used for in-flight dispatch feedback —
    /// the message must persist for the multi-second `git fetch` window
    /// rather than vanish mid-flight.
    pub(in crate::tui) message_sticky: bool,
}

// ---------------------------------------------------------------------------
// AgentTracking — agent health state for dispatched agents
// ---------------------------------------------------------------------------

/// Per-agent health tracking for dispatched agents. Stale detection is derived
/// from `task.last_pre_tool_use_at` by `ClassifyAgentActivity` on each tick;
/// this struct retains state the classifier cannot reconstruct from the
/// database — notification de-duplication, PR poll cadence, and message-flash
/// decay.
#[derive(Debug, Default)]
pub struct AgentTracking {
    pub notified_review: HashSet<TaskId>,
    pub notified_needs_input: HashSet<TaskId>,
    pub last_pr_poll: HashMap<TaskId, Instant>,
    /// A task that just *received* a native peer message — envelope glyph.
    pub message_flash: HashMap<TaskId, Instant>,
    /// A task that just *sent* one — its own glyph, same TTL and fill as
    /// [`Self::message_flash`]. See `docs/specs/core.allium`'s "Message
    /// flash".
    pub message_flash_sent: HashMap<TaskId, Instant>,
    /// Subtasks whose epic auto-dispatch chain claimed them and then failed to
    /// provision them, mapped to why (`AutoDispatchFailureIndicator` in
    /// docs/specs/epics.allium). Unlike every other entry here this one carries
    /// no timestamp: a stalled chain stays stalled until a human acts, so the
    /// marker decays on re-dispatch rather than on a clock.
    pub auto_dispatch_failed: HashMap<TaskId, String>,
}

impl AgentTracking {
    pub fn new() -> Self {
        Self::default()
    }

    /// Remove all tracking state for a task.
    pub fn clear(&mut self, id: TaskId) {
        self.notified_review.remove(&id);
        self.notified_needs_input.remove(&id);
        self.last_pr_poll.remove(&id);
        self.message_flash.remove(&id);
        self.message_flash_sent.remove(&id);
        self.auto_dispatch_failed.remove(&id);
    }
}

// ---------------------------------------------------------------------------
// InputState — current input mode and draft
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct InputState {
    pub mode: InputMode,
    pub buffer: String,
    /// Text caret: a **character** index into `buffer` (count of chars left of
    /// the caret), invariant `0..=buffer.chars().count()`. All edits go through
    /// `crate::tui::text_caret` and all buffer writes through
    /// [`InputState::set_buffer`] / [`InputState::clear_buffer`] so the caret
    /// never drifts out of range or onto a non-char boundary.
    pub caret: usize,
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
            caret: 0,
            task_draft: None,
            epic_draft: None,
            repo_cursor: 0,
            pending_epic_id: None,
        }
    }
}

impl InputState {
    /// Replace the buffer and land the caret at the end (natural for editing an
    /// existing value, e.g. a prefilled todo title or the default base branch).
    pub fn set_buffer(&mut self, s: String) {
        self.buffer = s;
        self.caret = self.buffer.chars().count();
    }

    /// Clear the buffer and reset the caret to the start.
    pub fn clear_buffer(&mut self) {
        self.buffer.clear();
        self.caret = 0;
    }
}

// ---------------------------------------------------------------------------
// ArchiveState — archive overlay state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct ArchiveState {
    pub list_state: ListState,
}

// ---------------------------------------------------------------------------
// SplitState — tmux split mode state
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct SplitState {
    pub(in crate::tui) active: bool,
    pub(in crate::tui) focused: bool,
    pub(in crate::tui) right_pane_id: Option<String>,
    pub(in crate::tui) pinned_task_id: Option<TaskId>,
}

impl Default for SplitState {
    fn default() -> Self {
        Self {
            active: false,
            focused: true,
            right_pane_id: None,
            pinned_task_id: None,
        }
    }
}

// ---------------------------------------------------------------------------
// SelectionState — multi-select state for batch operations
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct SelectionState {
    pub tasks: HashSet<TaskId>,
    pub epics: HashSet<EpicId>,
    pub pending_done: Vec<TaskId>,
}

impl SelectionState {
    pub fn has_selection(&self) -> bool {
        !self.tasks.is_empty() || !self.epics.is_empty()
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
    pub only_active: bool,
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

    /// Returns false when `only_active` is set and the task has no tmux window.
    pub fn task_matches(&self, task: &crate::models::Task) -> bool {
        !self.only_active || task.tmux_window.is_some()
    }
}

// ---------------------------------------------------------------------------
// SearchState — live title/id search over the task board
// ---------------------------------------------------------------------------

/// Task-search state: the query matches a task title (fuzzy subsequence) or a
/// task id (digit prefix, optional leading `#`). `query` empty = no filtering.
/// `saved` holds the query to restore if the user cancels the search bar with Esc.
#[derive(Debug, Clone, Default)]
pub struct SearchState {
    pub query: String,
    pub(in crate::tui) saved: Option<String>,
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
    pub plan_path: Option<String>,
    pub tag: Option<TaskTag>,
    pub base_branch: Option<String>,
    pub wrap_up_mode: Option<crate::models::WrapUpMode>,
    /// Resolved post-edit url value (not a delta): `Some` to set, `None` to
    /// clear or leave absent. Applied directly to the in-memory task snapshot.
    pub url: Option<crate::models::TaskUrl>,
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
    pub(in crate::tui) anchor: Option<ColumnAnchor>,
    pub(in crate::tui) archive_row: usize,
}

impl BoardSelection {
    pub fn new() -> Self {
        Self {
            selected_column: 1,
            selected_row: [0; TaskStatus::COLUMN_COUNT],
            on_select_all: false,
            list_states: std::array::from_fn(|_| ListState::default()),
            anchor: None,
            archive_row: 0,
        }
    }

    pub fn new_for_board() -> Self {
        Self::new()
    }

    pub fn new_for_epic() -> Self {
        Self::new()
    }

    pub fn column(&self) -> usize {
        self.selected_column
    }

    /// Row cursor for the given navigation column.
    /// nav col 1–4 → selected_row[nav_col-1], nav col 5 → archive_row.
    pub fn row(&self, col: usize) -> usize {
        match col {
            1..=4 => self.selected_row[col - 1],
            5 => self.archive_row,
            _ => 0,
        }
    }

    pub fn set_column(&mut self, col: usize) {
        self.selected_column = col;
    }

    pub fn set_row(&mut self, col: usize, row: usize) {
        match col {
            1..=4 => self.selected_row[col - 1] = row,
            5 => self.archive_row = row,
            _ => {}
        }
    }

    /// Reset the cursor for `col` to the top row, clear the select-all
    /// toggle, and (for task columns) scroll that column's list back to the
    /// top. Used when the cursor enters a column and should never land on a
    /// row remembered from a prior visit.
    pub fn reset_to_top(&mut self, col: usize) {
        self.set_row(col, 0);
        self.on_select_all = false;
        if let 1..=4 = col {
            *self.list_states[col - 1].offset_mut() = 0;
        }
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

#[derive(Debug)]
pub enum ViewMode {
    Board(BoardSelection),
    Epic {
        epic_id: EpicId,
        selection: BoardSelection,
        /// The view to restore when exiting this epic.
        /// For a root epic entered from the board, this is `ViewMode::Board(...)`.
        /// For a nested sub-epic, this is `ViewMode::Epic { ... }` of the parent.
        parent: Box<ViewMode>,
    },
    TaskDetail {
        task_id: TaskId,
        scroll: u16,
        zoomed: bool,
        /// Scroll limit — updated by the renderer each frame from the actual wrapped line count.
        /// Do not treat this as authoritative input state; it is renderer-managed.
        max_scroll: u16,
        previous: Box<ViewMode>,
    },
    Todos {
        todos: Vec<crate::models::Todo>,
        selected: usize,
        previous: Box<ViewMode>,
    },
}

impl Clone for ViewMode {
    fn clone(&self) -> Self {
        match self {
            ViewMode::Board(sel) => ViewMode::Board(sel.clone()),
            ViewMode::Epic {
                epic_id,
                selection,
                parent,
            } => ViewMode::Epic {
                epic_id: *epic_id,
                selection: selection.clone(),
                parent: parent.clone(),
            },
            ViewMode::TaskDetail {
                task_id,
                scroll,
                zoomed,
                max_scroll,
                previous,
            } => ViewMode::TaskDetail {
                task_id: *task_id,
                scroll: *scroll,
                zoomed: *zoomed,
                max_scroll: *max_scroll,
                previous: previous.clone(),
            },
            ViewMode::Todos {
                todos,
                selected,
                previous,
            } => ViewMode::Todos {
                todos: todos.clone(),
                selected: *selected,
                previous: previous.clone(),
            },
        }
    }
}

impl ViewMode {
    pub(in crate::tui) fn selection(&self) -> &BoardSelection {
        match self {
            ViewMode::Board(sel) => sel,
            ViewMode::Epic { selection, .. } => selection,
            ViewMode::TaskDetail { previous, .. } => previous.selection(),
            ViewMode::Todos { previous, .. } => previous.selection(),
        }
    }

    pub(in crate::tui) fn selection_mut(&mut self) -> &mut BoardSelection {
        match self {
            ViewMode::Board(sel) => sel,
            ViewMode::Epic { selection, .. } => selection,
            ViewMode::TaskDetail { previous, .. } => previous.selection_mut(),
            ViewMode::Todos { previous, .. } => previous.selection_mut(),
        }
    }
}

impl Default for ViewMode {
    fn default() -> Self {
        ViewMode::Board(BoardSelection::new_for_board())
    }
}

// ---------------------------------------------------------------------------
// BoardViewMode — the board-column-relevant subset of ViewMode
// ---------------------------------------------------------------------------

/// `ViewMode` narrowed to the two variants that carry board-column layout:
/// `Board` and `Epic`. Returned by `App::effective_view_mode()`, which peels
/// away the `TaskDetail`/`Todos` overlay variants. Column-builder
/// callers match exhaustively on this with no `unreachable!` fallback.
pub(in crate::tui) enum BoardViewMode<'a> {
    Board(&'a BoardSelection),
    Epic {
        epic_id: EpicId,
        selection: &'a BoardSelection,
    },
}

// ---------------------------------------------------------------------------
// ColumnItem — resolves whether cursor is on a task or an epic
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum ColumnItem<'a> {
    Task(&'a Task),
    Epic(&'a Epic),
    /// Non-selectable group header in flat view. Carries the epic so the renderer
    /// can read its title without an extra lookup.
    EpicHeader(&'a Epic),
    /// Non-selectable substatus section header, pre-built by the flat-view path of
    /// `column_items_for_status_with_stats`. Only produced in flat view for
    /// Running and Review columns. The renderer must not also inject its own
    /// substatus header for the same group transition.
    SubstatusLabel(&'static str),
    /// Non-selectable separator inserted in flat view between the last epic-grouped
    /// task and the first orphan task (a task with no epic). Signals the visual
    /// boundary so the renderer can draw a divider line.
    OrphanSeparator,
}

impl ColumnItem<'_> {
    /// Returns `true` for `Task` and `Epic` items that can hold the cursor.
    /// `EpicHeader`, `SubstatusLabel`, and `OrphanSeparator` are decorative and non-selectable.
    pub fn is_selectable(&self) -> bool {
        matches!(self, ColumnItem::Task(_) | ColumnItem::Epic(_))
    }
}

// ---------------------------------------------------------------------------
// ColumnAnchor — identity of the currently-selected task-board item
// ---------------------------------------------------------------------------

/// Identifies which item the cursor is anchored to across column refreshes.
/// Task and Epic IDs come from separate SQLite sequences and can overlap,
/// so we use a discriminated enum rather than a bare i64.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnAnchor {
    Task(crate::models::TaskId),
    Epic(crate::models::EpicId),
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
    pub fn build(app: &'a super::App, stats: &EpicStatsMap) -> Self {
        // Call tasks_for_current_view() and epic_search_pass() once each and share
        // them across all column builds instead of recomputing them per-status
        // inside column_items_for_status_with_stats.
        let view_tasks = app.tasks_for_current_view();
        let pass = app.epic_search_pass();
        let columns = std::array::from_fn(|i| {
            let status = TaskStatus::ALL[i];
            app.column_items_for_status_with_view_tasks(status, Some(stats), &view_tasks, &pass)
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
    pub parent_epic_id: Option<EpicId>,
}

// ---------------------------------------------------------------------------
// SubtaskStats — pre-computed per-epic subtask status counts
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SubtaskStats {
    pub backlog: usize,
    pub running: usize,
    pub review: usize,
    pub done: usize,
    pub total: usize,
    pub substatus: EpicSubstatus,
}

impl SubtaskStats {
    /// Compute stats for a single epic from its non-archived subtasks,
    /// including tasks owned by any descendant sub-epics. The `substatus`
    /// field also reflects the full subtree: a blocked task anywhere in the
    /// descendant hierarchy contributes to the `Blocked(N)` indicator.
    ///
    /// `children_map` is the parent→children adjacency map produced by
    /// [`crate::models::build_children_map`]. Build it once per stats
    /// computation and pass it here to avoid O(epics²) rebuilds.
    pub fn for_epic(
        epic: &Epic,
        all_tasks: &[Task],
        children_map: &HashMap<EpicId, Vec<EpicId>>,
    ) -> Self {
        let epic_ids = crate::models::descendant_epic_ids_with_map(epic.id, children_map);

        let mut backlog = 0;
        let mut running = 0;
        let mut review = 0;
        let mut done = 0;
        let mut owned: Vec<&Task> = Vec::new();

        for t in all_tasks {
            if t.status == TaskStatus::Archived {
                continue;
            }
            if matches!(t.epic_id, Some(eid) if epic_ids.contains(&eid)) {
                match t.status {
                    TaskStatus::Backlog => backlog += 1,
                    TaskStatus::Running => running += 1,
                    TaskStatus::Review => review += 1,
                    TaskStatus::Done => done += 1,
                    TaskStatus::Archived => {}
                }
                owned.push(t);
            }
        }

        let substatus = crate::models::epic_substatus(epic, &owned);

        SubtaskStats {
            backlog,
            running,
            review,
            done,
            total: backlog + running + review + done,
            substatus,
        }
    }
}

/// Pre-computed subtask stats for all epics, keyed by EpicId.
pub type EpicStatsMap = HashMap<EpicId, SubtaskStats>;

// ---------------------------------------------------------------------------
// LayoutCache — derived per-frame layout state, invalidated as a unit
// ---------------------------------------------------------------------------

/// Derived layout state computed from `board.tasks`/`board.epics`, populated
/// together by `App::cached_epic_stats()` and cleared together by
/// `App::invalidate_layout_cache()`. Grouped into one struct so the fields
/// that must stay coherent with each other (and with the board) can only be
/// invalidated as a unit — see `LayoutCache::invalidate()`. `cached_epic_stats()`
/// also self-heals on a fingerprint mismatch even if invalidation was
/// forgotten; see `App::compute_layout_fingerprint()`.
#[derive(Debug, Default)]
pub(in crate::tui) struct LayoutCache {
    /// Cached result of `compute_epic_stats()`, wrapped in an `Arc` so that
    /// `cached_epic_stats()` returns a reference-counted handle (O(1) clone)
    /// rather than cloning the full `HashMap` on every call.
    pub(in crate::tui) epic_stats_cache: Option<std::sync::Arc<EpicStatsMap>>,
    /// Parent→children adjacency map over `board.epics`. Built once alongside
    /// `epic_stats_cache` in `cached_epic_stats()`; passed into
    /// `compute_epic_stats()` so the map is not rebuilt for each epic.
    pub(in crate::tui) children_map_cache: Option<HashMap<EpicId, Vec<EpicId>>>,
    /// Pre-sorted selectable items (tasks + epics) per status in display order.
    /// Built once alongside `epic_stats_cache`; `update_anchor_from_current`
    /// reads from this (O(1) per nav event) instead of re-sorting the column.
    pub(in crate::tui) column_anchor_cache: Option<HashMap<TaskStatus, Vec<ColumnAnchor>>>,
    /// Per-epic `(epic_repo_matches, epic_matches)` results, built once per render frame
    /// inside `cached_epic_stats()` using a single shared `build_children_map()` call.
    pub(in crate::tui) epic_filter_cache: Option<HashMap<EpicId, (bool, bool)>>,
    /// Fingerprint of the cache-relevant fields of `board.tasks`/`board.epics`
    /// (id, status, epic_id/parent_epic_id, sort_order) captured when
    /// `epic_stats_cache` and friends were last populated. `cached_epic_stats()`
    /// recomputes this fingerprint on every call and self-heals (discards and
    /// rebuilds) if it no longer matches — so a handler that forgets to call
    /// `invalidate_layout_cache()` cannot serve stale data, it only pays for
    /// an extra rebuild. See `App::compute_layout_fingerprint()`.
    pub(in crate::tui) layout_cache_fingerprint: Option<u64>,
    /// TaskId → Vec index for O(1) lookups in `find_task_mut`. Not primed in
    /// `App::new()` to avoid staleness when tests mutate `board.tasks` directly.
    /// Rebuilt lazily in `find_task_mut` whenever `task_index_fingerprint`
    /// no longer matches `App::compute_task_ids_fingerprint()` (covers both
    /// length changes and same-length id-set replacement).
    pub(in crate::tui) task_index: Option<HashMap<TaskId, usize>>,
    /// Fingerprint of `board.tasks` ids captured when `task_index` was last
    /// built. See `App::compute_task_ids_fingerprint()`.
    pub(in crate::tui) task_index_fingerprint: Option<u64>,
}

impl LayoutCache {
    /// Clear every cache field as a unit. Called whenever `board.tasks` or
    /// `board.epics` are mutated; also a no-op-safe fallback since
    /// `cached_epic_stats()` self-heals on a fingerprint mismatch regardless.
    pub(in crate::tui) fn invalidate(&mut self) {
        *self = Self::default();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::models::{SubStatus, TaskId};
    use chrono::Utc;

    fn make_test_epic(id: i64, parent: Option<i64>) -> Epic {
        let now = Utc::now();
        Epic {
            id: EpicId(id),
            title: format!("Epic {id}"),
            description: String::new(),
            status: TaskStatus::Backlog,
            plan_path: None,
            sort_order: None,
            auto_dispatch: false,
            parent_epic_id: parent.map(EpicId),
            feed_command: None,
            feed_interval_secs: None,
            group_by_repo: false,
            feed_role: crate::models::FeedRole::None,
            origin: crate::models::EpicOrigin::Manual,
            created_at: now,
            updated_at: now,
        }
    }

    fn make_test_task(id: i64, status: TaskStatus, epic: Option<i64>) -> Task {
        let now = Utc::now();
        Task {
            id: TaskId(id),
            title: format!("Task {id}"),
            description: String::new(),
            repo_path: "/repo".to_string(),
            status,
            sub_status: SubStatus::None,
            worktree: None,
            tmux_window: None,
            plan_path: None,
            epic_id: epic.map(EpicId),
            url: None,
            tag: None,
            sort_order: None,
            base_branch: "main".into(),
            external_id: None,
            labels: Vec::new(),
            created_at: now,
            updated_at: now,
            last_pre_tool_use_at: None,
            last_notification_at: None,
            last_peer_message_sent_at: None,
            last_peer_message_received_at: None,
            wrap_up_mode: None,
            auto_run_plan: false,
            live_subagents: 0,
            stop_pending: false,
            live_shells: 0,
            oldest_live_shell_started_at: None,
            schedule_interval_secs: None,
            pinned_branch: None,
            last_processed_sha: None,
            last_scheduled_check_at: None,
        }
    }

    // -- SubtaskStats --

    #[test]
    fn subtask_stats_counts_direct_tasks_only_without_nested_epics() {
        let epics = vec![make_test_epic(1, None)];
        let tasks = vec![
            make_test_task(1, TaskStatus::Running, Some(1)),
            make_test_task(2, TaskStatus::Done, Some(1)),
        ];
        let cm = crate::models::build_children_map(&epics);
        let stats = SubtaskStats::for_epic(&epics[0], &tasks, &cm);
        assert_eq!(stats.running, 1);
        assert_eq!(stats.done, 1);
        assert_eq!(stats.total, 2);
    }

    #[test]
    fn subtask_stats_includes_tasks_from_nested_sub_epics() {
        let epics = vec![make_test_epic(1, None), make_test_epic(2, Some(1))];
        let tasks = vec![
            make_test_task(1, TaskStatus::Backlog, Some(1)),
            make_test_task(2, TaskStatus::Running, Some(2)),
            make_test_task(3, TaskStatus::Done, Some(2)),
        ];
        let cm = crate::models::build_children_map(&epics);
        let stats = SubtaskStats::for_epic(&epics[0], &tasks, &cm);
        assert_eq!(stats.backlog, 1);
        assert_eq!(stats.running, 1);
        assert_eq!(stats.done, 1);
        assert_eq!(stats.total, 3);
    }

    #[test]
    fn subtask_stats_includes_tasks_from_deeply_nested_epics() {
        let epics = vec![
            make_test_epic(1, None),
            make_test_epic(2, Some(1)),
            make_test_epic(3, Some(2)),
        ];
        let tasks = vec![make_test_task(1, TaskStatus::Running, Some(3))];
        let cm = crate::models::build_children_map(&epics);
        let stats = SubtaskStats::for_epic(&epics[0], &tasks, &cm);
        assert_eq!(stats.running, 1);
        assert_eq!(stats.total, 1);
    }

    #[test]
    fn subtask_stats_excludes_archived_tasks_from_nested_epics() {
        let epics = vec![make_test_epic(1, None), make_test_epic(2, Some(1))];
        let tasks = vec![
            make_test_task(1, TaskStatus::Running, Some(1)),
            make_test_task(2, TaskStatus::Archived, Some(2)),
        ];
        let cm = crate::models::build_children_map(&epics);
        let stats = SubtaskStats::for_epic(&epics[0], &tasks, &cm);
        assert_eq!(stats.running, 1);
        assert_eq!(stats.total, 1);
    }

    #[test]
    fn subtask_stats_ignores_tasks_with_no_epic_id() {
        let epics = vec![make_test_epic(1, None)];
        let tasks = vec![
            make_test_task(1, TaskStatus::Running, Some(1)),
            make_test_task(2, TaskStatus::Running, None), // unowned — must not count
        ];
        let cm = crate::models::build_children_map(&epics);
        let stats = SubtaskStats::for_epic(&epics[0], &tasks, &cm);
        assert_eq!(stats.running, 1);
        assert_eq!(stats.total, 1);
    }

    #[test]
    fn subtask_stats_blocked_substatus_includes_nested_blocked_tasks() {
        use crate::models::{EpicSubstatus, SubStatus};

        let mut parent = make_test_epic(1, None);
        parent.status = TaskStatus::Running;
        let child_epic = make_test_epic(2, Some(1));
        let epics = vec![parent.clone(), child_epic];

        // A blocked task lives on the child epic, not directly on parent.
        let mut blocked_task = make_test_task(1, TaskStatus::Running, Some(2));
        blocked_task.sub_status = SubStatus::Crashed;
        let tasks = vec![blocked_task];

        let cm = crate::models::build_children_map(&epics);
        let stats = SubtaskStats::for_epic(&parent, &tasks, &cm);
        assert_eq!(stats.substatus, EpicSubstatus::Blocked(1));
    }

    // -- RepoFilterMode --

    #[test]
    fn repo_filter_mode_as_str() {
        assert_eq!(RepoFilterMode::Include.as_str(), "include");
        assert_eq!(RepoFilterMode::Exclude.as_str(), "exclude");
    }

    #[test]
    fn repo_filter_mode_from_str_roundtrip() {
        for mode in [RepoFilterMode::Include, RepoFilterMode::Exclude] {
            let s = mode.as_str();
            let parsed: RepoFilterMode = s.parse().unwrap();
            assert_eq!(parsed, mode);
        }
    }

    #[test]
    fn repo_filter_mode_from_str_invalid() {
        assert!("bogus".parse::<RepoFilterMode>().is_err());
        assert!("".parse::<RepoFilterMode>().is_err());
        assert!("Include".parse::<RepoFilterMode>().is_err());
    }

    #[test]
    fn repo_filter_mode_default_is_include() {
        assert_eq!(RepoFilterMode::default(), RepoFilterMode::Include);
    }

    // -- repo_filter_matches --

    /// Test-only mirror of the repo-filter predicate. Lives here (not in the
    /// production region) because only these tests exercise it.
    fn repo_filter_matches(filter: &HashSet<String>, mode: RepoFilterMode, repo: &str) -> bool {
        if filter.is_empty() {
            return true;
        }
        match mode {
            RepoFilterMode::Include => filter.contains(repo),
            RepoFilterMode::Exclude => !filter.contains(repo),
        }
    }

    #[test]
    fn repo_filter_matches_empty_filter_matches_any_repo() {
        let filter = HashSet::new();
        assert!(repo_filter_matches(
            &filter,
            RepoFilterMode::Include,
            "org/any"
        ));
        assert!(repo_filter_matches(
            &filter,
            RepoFilterMode::Exclude,
            "org/any"
        ));
    }

    #[test]
    fn repo_filter_matches_include_mode() {
        let filter: HashSet<String> = ["org/a".to_string()].into();
        assert!(repo_filter_matches(
            &filter,
            RepoFilterMode::Include,
            "org/a"
        ));
        assert!(!repo_filter_matches(
            &filter,
            RepoFilterMode::Include,
            "org/b"
        ));
    }

    #[test]
    fn repo_filter_matches_exclude_mode() {
        let filter: HashSet<String> = ["org/a".to_string()].into();
        assert!(!repo_filter_matches(
            &filter,
            RepoFilterMode::Exclude,
            "org/a"
        ));
        assert!(repo_filter_matches(
            &filter,
            RepoFilterMode::Exclude,
            "org/b"
        ));
    }
}
