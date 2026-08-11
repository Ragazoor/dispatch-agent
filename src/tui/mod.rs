pub mod commands;
mod dispatcher;
pub mod input;
pub mod messages;
pub mod text_caret;
pub mod types;
pub mod ui;
pub mod update;

pub use types::*;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

#[cfg(test)]
use crate::models::ReviewDecision;
use crate::models::{
    epic_substatus, Epic, EpicId, EpicSubstatus, SubStatus, Task, TaskId, TaskStatus, VisualColumn,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// How long a transient status message stays visible before auto-clearing.
pub(in crate::tui) const STATUS_MESSAGE_TTL: Duration = Duration::from_secs(5);

/// Maximum gap between two `g` presses for them to count as the `gg` chord
/// (jump to top of column). A single `g` outside this window falls back to
/// its normal action (jump to tmux window / enter epic).
pub(in crate::tui) const GG_CHORD_TIMEOUT: Duration = Duration::from_millis(150);

/// Interval between PR status polls for tasks in review.
pub(in crate::tui) const PR_POLL_INTERVAL: Duration = Duration::from_secs(30);

/// Number of ticks between main-session liveness polls. At `TICK_INTERVAL` (2s)
/// this is 10s — mirrors config.main_session_poll_interval (see
/// docs/specs/core.allium config and dispatch.allium: MainSessionIndicator) and
/// the DB-refresh fallback cadence.
pub(in crate::tui) const MAIN_SESSION_POLL_TICKS: u64 = 5;

/// Number of ticks between budget-snapshot reads. At `TICK_INTERVAL` (2s) this
/// is 10s — mirrors config.budget_poll_interval (see docs/specs/core.allium
/// config and dispatch.allium: TokenBudgetIndicator).
pub(in crate::tui) const BUDGET_POLL_TICKS: u64 = 5;

/// Age after which the budget indicator dims and shows its age. Mirrors
/// config.budget_stale_after.
pub(in crate::tui) const BUDGET_STALE_AFTER: Duration = Duration::from_secs(600);

/// Whether the stale-learning cleanup background job runs.
/// Mirrors config.stale_learning_cleanup_enabled (see docs/specs/core.allium config).
pub(crate) const STALE_LEARNING_CLEANUP_ENABLED: bool = true;

/// Age after which an approved, non-positively-scored learning (upvote_count <= 0)
/// becomes eligible for auto-archival. Mirrors config.stale_learning_threshold
/// (90 days; see docs/specs/core.allium config and learnings.allium: ArchiveStaleLearning).
pub(crate) const STALE_LEARNING_THRESHOLD: Duration = Duration::from_secs(90 * 24 * 60 * 60);

/// Minimum wall-clock spacing between stale-learning cleanup sweeps, so the sweep
/// does not run on every 2s tick. Not a spec config value — an implementation-level
/// cadence for the tick-driven job (see learnings.allium: ArchiveStaleLearning).
pub(crate) const STALE_CLEANUP_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// Max character width for task titles shown in confirmation popups and status messages.
pub(in crate::tui) const TITLE_DISPLAY_LENGTH: usize = 30;

/// Maximum time a task may remain in the `dispatching` set before the watchdog
/// force-fails it. Defence-in-depth against a stuck dispatch worker.
pub(in crate::tui) const DISPATCH_WATCHDOG_TIMEOUT: Duration = Duration::from_secs(60);

/// Number of braille spinner frames for the per-card "dispatching…" indicator.
/// Must match the length of `DISPATCHING_SPINNER` in `kanban.rs`.
pub(in crate::tui) const DISPATCH_SPINNER_FRAMES: u8 = 10;

/// Returns true for the Archive edge column that doesn't hold regular task data
/// and must be excluded from task-operation hotkeys.
pub(in crate::tui) fn is_edge_column(col: usize) -> bool {
    col == TaskStatus::COLUMN_COUNT + 1
}

/// Sort priority below `SubStatus::Approved`, reserved for the detached
/// display override below (see `display_column_priority`).
const DETACHED_AWAITING_REVIEW_PRIORITY: u8 = 7;

/// Whether the detach-aware display override (see `display_column_priority`
/// / `display_header_label`) applies: a detached `awaiting_review` task
/// (agent session ended, PR not yet merged). `is_detached` is a presentation
/// concern (derived from `Task::is_detached`), not part of the `SubStatus`
/// domain model.
fn is_detached_awaiting_review(sub_status: SubStatus, is_detached: bool) -> bool {
    is_detached && sub_status == SubStatus::AwaitingReview
}

/// Column sort priority for a task's sub-status, with the detach-aware
/// display override applied: a detached `awaiting_review` task sinks below
/// `Approved` so it doesn't crowd out review items that still need
/// attention.
pub(in crate::tui) fn display_column_priority(sub_status: SubStatus, is_detached: bool) -> u8 {
    if is_detached_awaiting_review(sub_status, is_detached) {
        DETACHED_AWAITING_REVIEW_PRIORITY
    } else {
        sub_status.column_priority()
    }
}

/// Section-header label for a task's sub-status, with the detach-aware
/// display override applied: a detached `awaiting_review` task shows
/// "awaiting merge" instead, since the agent session has ended and the task
/// is just waiting on the PR to land.
pub(in crate::tui) fn display_header_label(
    sub_status: SubStatus,
    is_detached: bool,
) -> &'static str {
    if is_detached_awaiting_review(sub_status, is_detached) {
        "awaiting merge"
    } else {
        sub_status.header_label()
    }
}

#[cfg(test)]
mod display_priority_tests {
    use super::*;

    #[test]
    fn detached_awaiting_review_sinks_below_approved() {
        let priority = display_column_priority(SubStatus::AwaitingReview, true);
        assert!(priority > SubStatus::Approved.column_priority());
    }

    #[test]
    fn non_detached_awaiting_review_keeps_model_priority() {
        assert_eq!(
            display_column_priority(SubStatus::AwaitingReview, false),
            SubStatus::AwaitingReview.column_priority()
        );
    }

    #[test]
    fn detach_flag_is_a_no_op_for_other_sub_statuses() {
        for &ss in SubStatus::ALL {
            if ss == SubStatus::AwaitingReview {
                continue;
            }
            assert_eq!(
                display_column_priority(ss, true),
                ss.column_priority(),
                "detach flag should not affect {ss:?}"
            );
            assert_eq!(
                display_header_label(ss, true),
                ss.header_label(),
                "detach flag should not affect {ss:?}"
            );
        }
    }

    #[test]
    fn detached_awaiting_review_label_is_awaiting_merge() {
        assert_eq!(
            display_header_label(SubStatus::AwaitingReview, true),
            "awaiting merge"
        );
    }

    #[test]
    fn non_detached_awaiting_review_keeps_model_label() {
        assert_eq!(
            display_header_label(SubStatus::AwaitingReview, false),
            SubStatus::AwaitingReview.header_label()
        );
    }
}

// ---------------------------------------------------------------------------
// ReparentPickerState
// ---------------------------------------------------------------------------

/// State for the reparent-epic tree picker overlay.
/// Lives on `App` directly (not inside `InputState`) because `RefCell<TreeState>`
/// does not implement `Clone`, and `InputState` derives `Clone`.
pub(in crate::tui) struct ReparentPickerState {
    pub(in crate::tui) epic_id: EpicId,
    pub(in crate::tui) tree_state: std::cell::RefCell<tui_tree_widget::TreeState<String>>,
    /// Pre-built tree items. Computed once when the picker opens so the render
    /// path never calls `reparent_target_epics` or `build_reparent_tree` per frame.
    pub(in crate::tui) items: Vec<tui_tree_widget::TreeItem<'static, String>>,
}

/// State for the move-task-to-epic tree picker overlay (the `m` key on a task
/// card). Mirrors [`ReparentPickerState`] but targets a task instead of an epic.
pub(in crate::tui) struct MoveTaskPickerState {
    pub(in crate::tui) task_id: TaskId,
    pub(in crate::tui) tree_state: std::cell::RefCell<tui_tree_widget::TreeState<String>>,
    /// Pre-built tree items. Computed once when the picker opens so the render
    /// path never calls `move_task_target_epics` or `build_reparent_tree` per frame.
    pub(in crate::tui) items: Vec<tui_tree_widget::TreeItem<'static, String>>,
}

// ---------------------------------------------------------------------------
// InteractionState — transient overlay/picker state, one-at-a-time by construction
// ---------------------------------------------------------------------------

/// Transient overlay/picker UI state: at most one of these is meaningfully
/// active at a time (each is gated by a distinct `InputMode`, mirroring
/// [`PendingAction`]). Grouped so `App`'s own field list only carries genuine
/// board/session state, not this long tail of "is some popup open" flags.
/// Not `Clone` (mirrors [`ReparentPickerState`]/[`MoveTaskPickerState`]:
/// their `RefCell<TreeState>` fields don't implement it).
#[derive(Default)]
pub(in crate::tui) struct InteractionState {
    pub(in crate::tui) reparent_picker: Option<ReparentPickerState>,
    pub(in crate::tui) move_task_picker: Option<MoveTaskPickerState>,
    pub(in crate::tui) managed_feed_config: Option<ManagedFeedConfigState>,
    /// The single one-shot "remember this until the next message" action in
    /// flight. See [`PendingAction`].
    pub(in crate::tui) pending: PendingAction,
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

pub struct App {
    pub(in crate::tui) board: BoardState,
    pub(in crate::tui) status: StatusState,
    pub(in crate::tui) should_quit: bool,
    pub(in crate::tui) notifications_enabled: bool,
    pub(in crate::tui) input: InputState,
    pub(in crate::tui) agents: AgentTracking,
    pub(in crate::tui) archive: ArchiveState,
    pub(in crate::tui) select: SelectionState,
    pub(in crate::tui) filter: FilterState,
    pub(in crate::tui) search: SearchState,
    /// Task IDs with an in-flight dispatch, mapped to their start time.
    /// Membership prevents duplicate dispatches; start times drive the 60-second watchdog.
    pub(in crate::tui) dispatching: HashMap<TaskId, Instant>,
    /// Spinner frame index (0..DISPATCH_SPINNER_FRAMES) for the per-card "dispatching…" indicator.
    /// Advanced by `Tick` only while `dispatching` is non-empty.
    pub(in crate::tui) spinner_tick: u8,
    pub(in crate::tui) main_session_dir: Option<String>,
    /// Whether the fixed "dispatch-main" tmux window is currently alive, as of
    /// the last liveness poll. Drives the status-bar main-session badge. Derived
    /// purely from a live tmux check (never a persisted reference); refreshed on
    /// the tick loop every `MAIN_SESSION_POLL_TICKS`. See docs/specs/dispatch.allium:
    /// MainSessionIndicator.
    pub(in crate::tui) main_session_alive: bool,
    /// Ticks elapsed since the last main-session liveness poll. Reset to 0 on
    /// each poll; the poll fires when this reaches `MAIN_SESSION_POLL_TICKS`.
    pub(in crate::tui) ticks_since_main_session_poll: u64,
    /// Latest budget snapshot read from `<data_dir>/rate-limits.json`. `None`
    /// when absent or unreadable — the steady state for non-subscription auth.
    /// Derived live, never persisted (dispatch.allium: TokenBudgetIndicator).
    pub(in crate::tui) budget: Option<crate::models::budget::BudgetSnapshot>,
    pub(in crate::tui) ticks_since_budget_poll: u64,
    /// Derived layout state (epic stats, anchor cache, task index, and their
    /// fingerprints) computed from `board.tasks`/`board.epics`. See
    /// [`LayoutCache`] for coherence details.
    pub(in crate::tui) layout: LayoutCache,
    /// Set to `true` whenever state changes that should trigger a redraw.
    /// The runtime skips `terminal.draw` on consecutive events that leave
    /// `dirty` false (e.g. an idle tick whose DB refresh found no changes).
    pub dirty: bool,
    /// Set to `true` when a `Persist` or `BatchPatchSubStatus` command
    /// completes, cleared when `handle_tick` emits `RefreshFromDb`.
    /// Ensures the board re-reads from DB promptly after any write.
    pub dirty_since_refresh: bool,
    /// Ticks elapsed since the last `RefreshFromDb` was emitted. Reset to 0
    /// on each refresh; the fallback fires when this reaches 5 (= 10 s).
    pub(in crate::tui) ticks_since_last_refresh: u64,
    /// Persisted managed-feed settings, snapshotted so the config popup opens
    /// without a DB round-trip. Loaded at startup, refreshed after a save.
    pub(in crate::tui) managed_feed_settings: ManagedFeedSettings,
    /// Transient overlay/picker state (pickers, in-progress popup edits, the
    /// one-shot pending action). See [`InteractionState`].
    pub(in crate::tui) interaction: InteractionState,
    /// Paths in `board.repo_paths` that do not exist on disk (`is_dir()` → false).
    /// Recomputed once in `handle_repo_paths_updated` so the render path is
    /// never blocked by filesystem syscalls on every frame.
    pub(in crate::tui) broken_repo_paths: HashSet<String>,
    /// Per-repository drift measurements, keyed by repo path
    /// (docs/specs/repo-sync.allium: entity RepoSyncState). Purely in-memory:
    /// every refresh point re-establishes it, and nothing is persisted.
    pub(in crate::tui) repo_sync: crate::repo_sync::RepoSyncCache,
    /// Wall-clock of the last stale-learning cleanup sweep. `None` = never run.
    /// `handle_tick` emits `LearningCommand::ArchiveStale` at most once per
    /// [`STALE_CLEANUP_INTERVAL`] by consulting this. See
    /// docs/specs/learnings.allium: ArchiveStaleLearning.
    pub(crate) last_stale_cleanup_at: Option<Instant>,
}

/// A one-shot transient action awaiting its follow-up message. Collapses the
// allow-phantom-symbol: removed fields, cited as the history this enum collapses
/// former `pending_todo_edit` / `pending_todo_delete` / `pending_todo_link` /
// allow-phantom-symbol: removed field, cited as the history this enum collapses
/// `pending_g` fields into one matchable value — only one can be in flight at a
/// time (each is gated by a distinct [`InputMode`], and `GChord` is only armed
/// on the board), so a single field loses no information.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(in crate::tui) enum PendingAction {
    /// Nothing pending.
    #[default]
    None,
    /// A todo is being edited in `InputMode::TodoTitle`; holds its id. The add
    /// flow leaves this `None`-equivalent (variant `None`), so an empty submit
    /// creates a new item.
    TodoEdit(crate::models::TodoId),
    /// A todo is awaiting delete confirmation in `InputMode::ConfirmDeleteTodo`.
    TodoDelete(crate::models::TodoId),
    /// Link (task or epic) to attach to the next quick-add todo; set by the `[t]`
    /// key handler when a task/epic is selected, cleared after the submit.
    TodoLink(crate::models::TodoLink),
    /// A single `g` press is awaiting a possible second `g` (the `gg` chord,
    /// jump to top of column) within [`GG_CHORD_TIMEOUT`]. Resolved by the next
    /// keypress (`handle_key_board_normal`) or, if the user goes idle after a
    /// lone `g`, by `handle_tick` as a backstop. Holds the press instant.
    GChord(Instant),
}

/// FNV-1a offset basis, used as the seed for the layout-cache fingerprints
/// (`App::compute_layout_fingerprint`, `App::compute_task_ids_fingerprint`).
/// These are internal, non-adversarial fingerprints — a cheap fold is
/// plenty and much cheaper than `DefaultHasher` (SipHash) on the hot render
/// path.
fn fnv_seed() -> u64 {
    0xcbf29ce484222325
}

/// Fold one `u64` field into an FNV-1a-style accumulator.
fn fnv_fold(acc: u64, v: u64) -> u64 {
    const FNV_PRIME: u64 = 0x100000001b3;
    (acc ^ v).wrapping_mul(FNV_PRIME)
}

/// Format a title for display in confirmation prompts, truncating if longer than `max_len` chars.
pub(in crate::tui) fn truncate_title(title: &str, max_len: usize) -> String {
    if title.chars().count() <= max_len {
        format!("\"{title}\"")
    } else {
        let truncated: String = title.chars().take(max_len.saturating_sub(3)).collect();
        format!("\"{truncated}...\"")
    }
}

/// Returns true if every character in `query_lower` (already lowercased) appears in
/// `path` as a forward subsequence (case-insensitive on `path`).
/// An empty query matches everything.
pub(in crate::tui) fn fuzzy_matches_lower(path: &str, query_lower: &str) -> bool {
    if query_lower.is_empty() {
        return true;
    }
    let path_lower = path.to_lowercase();
    let mut path_chars = path_lower.chars();
    for qc in query_lower.chars() {
        if !path_chars.any(|pc| pc == qc) {
            return false;
        }
    }
    true
}

/// The digit payload of a board-search query, if it can address a task by id:
/// the query with one optional leading `#` stripped, provided the remainder is
/// non-empty and entirely ASCII digits. `None` for anything else (`"38a"`,
/// `"a38"`, a lone `"#"`, an empty query), which means title-only matching.
pub(in crate::tui) fn id_digits_query(query: &str) -> Option<&str> {
    let digits = query.strip_prefix('#').unwrap_or(query);
    (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).then_some(digits)
}

/// Returns true if the decimal spelling of `id` starts with `digits` (a payload
/// from [`id_digits_query`]). Prefix, not substring: `"38"` matches `3837` but
/// not `1385`.
pub(in crate::tui) fn id_prefix_matches(id: i64, digits: &str) -> bool {
    id.to_string().starts_with(digits)
}

/// Returns true if every character in `query` appears in `path` as a
/// forward subsequence (case-insensitive). An empty query matches everything.
pub(in crate::tui) fn fuzzy_matches(path: &str, query: &str) -> bool {
    fuzzy_matches_lower(path, &query.to_lowercase())
}

/// Returns the subset of `paths` that fuzzy-match `query`, preserving order.
pub(in crate::tui) fn filtered_repos(paths: &[String], query: &str) -> Vec<String> {
    paths
        .iter()
        .filter(|p| fuzzy_matches(p, query))
        .cloned()
        .collect()
}

/// Whether the epic (identified by `epic_ids` = epic + all descendants) should be shown
/// under the current repo filter.  A single pass over `tasks` tracks both "has any
/// non-archived subtask" and "has any repo-matching subtask", so the logic is O(tasks)
/// instead of two passes.
pub(in crate::tui) fn epic_repo_matches_for_ids(
    tasks: &[Task],
    filter: &FilterState,
    epic_ids: &HashSet<EpicId>,
) -> bool {
    if filter.repos.is_empty() {
        return true;
    }
    let (has_active, has_match) = tasks.iter().fold((false, false), |(active, matched), t| {
        if matches!(t.epic_id, Some(eid) if epic_ids.contains(&eid))
            && t.status != TaskStatus::Archived
        {
            (true, matched || filter.matches(&t.repo_path))
        } else {
            (active, matched)
        }
    });
    !has_active || has_match
}

/// Whether the epic has at least one subtask with an active tmux window.
/// `epic_ids` must include the epic itself and all its descendants.
pub(in crate::tui) fn epic_active_matches_for_ids(
    tasks: &[Task],
    epic_ids: &HashSet<EpicId>,
) -> bool {
    tasks.iter().any(|t| {
        matches!(t.epic_id, Some(eid) if epic_ids.contains(&eid)) && t.tmux_window.is_some()
    })
}

/// Whether `title`/`id` themselves satisfy the board-search query: a
/// case-insensitive forward-subsequence title match, or a decimal id-prefix
/// match against `id_digits` (see [`id_digits_query`]). Shared by the task
/// and epic own-match checks so the OR is expressed once. See
/// board_search_filter in `docs/specs/core.allium`.
pub(in crate::tui) fn own_search_match(
    title: &str,
    id: i64,
    query_lower: &str,
    id_digits: Option<&str>,
) -> bool {
    fuzzy_matches_lower(title, query_lower)
        || id_digits.is_some_and(|digits| id_prefix_matches(id, digits))
}

/// Whether some non-archived task in the epic subtree named by `epic_ids`
/// carries the epic's search match: the task has an own match (title or
/// id-prefix) AND the board would actually show that task under the repo and
/// only-active filters (`filter.matches` on its repo_path, and
/// `filter.task_matches`) — the same two predicates `tasks_for_current_view`
/// applies. A task the board would hide cannot keep an ancestor epic's card
/// alive: drilling into that card would be a dead end. See
/// board_search_filter in `docs/specs/core.allium`.
pub(in crate::tui) fn epic_search_matches_for_ids(
    tasks: &[Task],
    filter: &FilterState,
    epic_ids: &HashSet<EpicId>,
    query_lower: &str,
    id_digits: Option<&str>,
) -> bool {
    tasks.iter().any(|t| {
        matches!(t.epic_id, Some(eid) if epic_ids.contains(&eid))
            && t.status != TaskStatus::Archived
            && own_search_match(&t.title, t.id.0, query_lower, id_digits)
            && filter.matches(&t.repo_path)
            && filter.task_matches(t)
    })
}

/// Returns true when the buffer should be offered as a selectable "new path"
/// entry: the buffer is non-empty and is not already an exact member of
/// `filtered` (the user is typing a path that doesn't exist in the saved list).
pub(in crate::tui) fn has_new_repo_option(buffer: &str, filtered: &[String]) -> bool {
    !buffer.is_empty() && !filtered.iter().any(|p| p == buffer)
}

/// Resolve the item Enter selects in a picker (RepoPathPicker,
/// BaseBranchPicker, ...): `candidates` fuzzy-filtered by `buffer`, indexed at
/// `cursor` when that falls within the filtered list, otherwise the typed
/// `buffer` itself when it qualifies as a "new" entry. `None` when the
/// effective list is empty (buffer empty, no candidates).
pub(in crate::tui) fn resolve_picker_selection(
    candidates: &[String],
    buffer: &str,
    cursor: usize,
) -> Option<String> {
    let filtered = filtered_repos(candidates, buffer);
    if cursor < filtered.len() {
        Some(filtered[cursor].clone())
    } else if has_new_repo_option(buffer, &filtered) {
        Some(buffer.trim().to_string())
    } else {
        None
    }
}

impl App {
    pub fn new(tasks: Vec<Task>) -> Self {
        let mut app = App {
            board: BoardState {
                tasks,
                epics: Vec::new(),
                view_mode: ViewMode::default(),
                repo_paths: Vec::new(),
                repo_base_branches: HashMap::new(),
                split: SplitState::default(),
                flattened: false,
                todo_open_count: 0,
            },
            status: StatusState::default(),
            should_quit: false,
            notifications_enabled: false,
            input: InputState::default(),
            agents: AgentTracking::new(),
            archive: ArchiveState::default(),
            select: SelectionState::default(),
            filter: FilterState::default(),
            search: SearchState::default(),
            dispatching: HashMap::new(),
            spinner_tick: 0,
            main_session_dir: None,
            main_session_alive: false,
            ticks_since_main_session_poll: 0,
            budget: None,
            ticks_since_budget_poll: 0,
            layout: LayoutCache::default(),
            dirty: true,
            dirty_since_refresh: true,
            ticks_since_last_refresh: 0,
            managed_feed_settings: ManagedFeedSettings::default(),
            interaction: InteractionState::default(),
            broken_repo_paths: HashSet::new(),
            repo_sync: crate::repo_sync::RepoSyncCache::default(),
            last_stale_cleanup_at: None,
        };
        // Prime all caches so the first render is a cache hit instead of recomputing.
        let _ = app.cached_epic_stats();
        app.update_anchor_from_current();
        app
    }

    /// Returns true if the given task has an in-flight dispatch *started by
    /// this TUI process*. Not the whole picture — see
    /// [`Self::dispatch_may_be_in_flight`].
    pub fn is_dispatching(&self, id: TaskId) -> bool {
        self.dispatching.contains_key(&id)
    }

    /// Whether an unprovisioned task is unprovisioned because a dispatch is
    /// still running, rather than because one died.
    ///
    /// `dispatching` only holds dispatches this TUI started. The epic
    /// auto-dispatch chain claims its next subtask inside the MCP handler
    /// (`auto_dispatch_next`) and never enters that map, and a TUI restart
    /// mid-dispatch empties it — in both cases the row is `Running` with no
    /// worktree while an agent is genuinely being provisioned. So fall back to
    /// the row itself: every claim seeds `last_pre_tool_use_at`, and
    /// [`DISPATCH_WATCHDOG_TIMEOUT`] is already the line this codebase draws
    /// between "slow" and "dead" (see `DispatchingTimeout` in
    /// `docs/specs/dispatch.allium`).
    ///
    /// A missing stamp counts as not-in-flight, so an unstamped row surfaces
    /// immediately rather than hiding for a minute.
    ///
    /// Only meaningful for `task.is_unprovisioned()`; a provisioned task has
    /// its stamp refreshed by agent hooks and would always look "fresh".
    pub fn dispatch_may_be_in_flight(&self, task: &Task, now: DateTime<Utc>) -> bool {
        if self.is_dispatching(task.id) {
            return true;
        }
        task.last_pre_tool_use_at.is_some_and(|stamp| {
            now.signed_duration_since(stamp)
                .to_std()
                .is_ok_and(|elapsed| elapsed < DISPATCH_WATCHDOG_TIMEOUT)
        })
    }

    /// Get the current selection state (from whichever view mode is active).
    pub fn selection(&self) -> &BoardSelection {
        self.board.view_mode.selection()
    }

    /// Get mutable access to the current selection state.
    pub(in crate::tui) fn selection_mut(&mut self) -> &mut BoardSelection {
        self.board.view_mode.selection_mut()
    }

    /// When in an overlay (TaskDetail/Learnings/Todos), returns the board mode
    /// beneath (Board or Epic) by peeling away `previous` links. Returns
    /// [`BoardViewMode`] rather than `&ViewMode` so callers get an exhaustive
    /// 2-variant match with no `unreachable!` fallback for the overlay variants.
    pub(in crate::tui) fn effective_view_mode(&self) -> BoardViewMode<'_> {
        let mut current = &self.board.view_mode;
        loop {
            match current {
                ViewMode::Board(sel) => return BoardViewMode::Board(sel),
                ViewMode::Epic {
                    epic_id, selection, ..
                } => {
                    return BoardViewMode::Epic {
                        epic_id: *epic_id,
                        selection,
                    }
                }
                ViewMode::TaskDetail { previous, .. }
                | ViewMode::Learnings { previous, .. }
                | ViewMode::Todos { previous, .. } => current = previous,
            }
        }
    }

    // Read-only accessors for code outside the tui module
    pub fn tasks(&self) -> &[Task] {
        &self.board.tasks
    }
    pub fn should_quit(&self) -> bool {
        self.should_quit
    }
    pub fn selected_column(&self) -> usize {
        self.selection().column()
    }
    pub fn selected_row(&self) -> &[usize; TaskStatus::COLUMN_COUNT] {
        &self.selection().selected_row
    }
    pub fn view_mode(&self) -> &ViewMode {
        &self.board.view_mode
    }
    pub fn epics(&self) -> &[Epic] {
        &self.board.epics
    }
    pub fn mode(&self) -> &InputMode {
        &self.input.mode
    }
    pub fn input_buffer(&self) -> &str {
        &self.input.buffer
    }
    pub fn split_active(&self) -> bool {
        self.board.split.active
    }
    pub fn split_focused(&self) -> bool {
        self.board.split.focused
    }
    pub fn split_pinned_task_id(&self) -> Option<TaskId> {
        self.board.split.pinned_task_id
    }
    pub fn status_message(&self) -> Option<&str> {
        self.status.message.as_deref()
    }
    pub fn error_popup(&self) -> Option<&str> {
        self.status.error_popup.as_deref()
    }
    pub fn repo_paths(&self) -> &[String] {
        &self.board.repo_paths
    }
    /// The most-recently-used base_branch history for `repo_path`, ordered
    /// most-recent-first. Empty when the repo has no recorded history.
    pub fn base_branches_for(&self, repo_path: &str) -> &[String] {
        self.board
            .repo_base_branches
            .get(repo_path)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// The candidate slice for the picker rendered by the current
    /// `InputMode`, if any: `InputBaseBranch` scopes to the draft's
    /// repo_path's history; `InputMode::is_repo_picker()` modes use the
    /// global saved repo-path list. `None` when the current mode has no
    /// picker candidates (e.g. plain text fields).
    pub(in crate::tui) fn picker_candidates(&self) -> Option<&[String]> {
        if matches!(self.input.mode, InputMode::InputBaseBranch) {
            let repo_path = self
                .input
                .task_draft
                .as_ref()
                .map(|d| d.repo_path.as_str())
                .unwrap_or("");
            Some(self.base_branches_for(repo_path))
        } else if self.input.mode.is_repo_picker() {
            Some(&self.board.repo_paths)
        } else {
            None
        }
    }
    pub fn todo_open_count(&self) -> i64 {
        self.board.todo_open_count
    }
    pub fn task_draft(&self) -> Option<&TaskDraft> {
        self.input.task_draft.as_ref()
    }
    pub fn is_stale(&self, id: TaskId) -> bool {
        self.find_task(id)
            .is_some_and(|t| t.sub_status == SubStatus::Stale)
    }
    pub fn is_crashed(&self, id: TaskId) -> bool {
        self.find_task(id)
            .is_some_and(|t| t.sub_status == SubStatus::Crashed)
    }
    pub fn show_archived(&self) -> bool {
        self.selection().column() == TaskStatus::COLUMN_COUNT + 1
    }
    pub fn selected_archive_row(&self) -> usize {
        self.selection().row(TaskStatus::COLUMN_COUNT + 1)
    }
    pub fn selected_tasks(&self) -> &HashSet<TaskId> {
        &self.select.tasks
    }
    pub fn selected_epics(&self) -> &HashSet<EpicId> {
        &self.select.epics
    }
    pub fn on_select_all(&self) -> bool {
        self.selection().on_select_all
    }
    pub fn has_selection(&self) -> bool {
        self.select.has_selection()
    }

    pub fn notifications_enabled(&self) -> bool {
        self.notifications_enabled
    }
    pub fn repo_filter(&self) -> &HashSet<String> {
        &self.filter.repos
    }
    pub fn repo_filter_mode(&self) -> RepoFilterMode {
        self.filter.mode
    }
    pub fn filter_presets(&self) -> &[(String, HashSet<String>, RepoFilterMode)] {
        &self.filter.presets
    }

    pub fn filter_only_active(&self) -> bool {
        self.filter.only_active
    }

    /// Bootstrap-only carve-out: set during runtime startup from the saved
    /// `notifications_enabled` setting before the message loop begins. After
    /// bootstrap completes, this state is mutated only via Messages. See the
    /// "Visibility Convention" section in CLAUDE.md.
    pub fn set_notifications_enabled(&mut self, enabled: bool) {
        self.notifications_enabled = enabled;
    }

    pub fn main_session_dir(&self) -> Option<&str> {
        self.main_session_dir.as_deref()
    }

    /// Bootstrap-only carve-out: populated by the runtime loader from the four
    /// managed-feed settings at startup, and re-set in-memory after a save so
    /// the config popup re-opens with fresh values. See the config popup
    /// handlers in `update/managed_feeds.rs`.
    pub fn set_managed_feed_settings(&mut self, settings: ManagedFeedSettings) {
        self.managed_feed_settings = settings;
    }

    /// Read-only access to the in-progress config edit buffer (test/render use).
    pub fn managed_feed_config(&self) -> Option<&ManagedFeedConfigState> {
        self.interaction.managed_feed_config.as_ref()
    }

    /// Bootstrap-only carve-out: populated by the runtime loader from
    /// `main_session.dir` setting at startup. After bootstrap, set via Messages.
    pub fn set_main_session_dir(&mut self, dir: Option<String>) {
        self.main_session_dir = dir;
    }

    pub fn set_repo_filter(&mut self, filter: HashSet<String>) {
        self.filter.repos = filter;
        self.sync_board_selection();
    }

    pub fn set_repo_filter_mode(&mut self, mode: RepoFilterMode) {
        self.filter.mode = mode;
        self.sync_board_selection();
    }

    /// Set a transient status message with auto-clear timestamp.
    pub(in crate::tui) fn set_status(&mut self, msg: String) {
        self.status.message = Some(msg);
        self.status.message_set_at = Some(Instant::now());
        self.status.message_sticky = false;
    }

    /// Set a sticky status message that bypasses the 5-second TTL.
    /// The message persists until `clear_status` is called explicitly.
    pub(in crate::tui) fn set_status_sticky(&mut self, msg: String) {
        self.status.message = Some(msg);
        self.status.message_set_at = Some(Instant::now());
        self.status.message_sticky = true;
    }

    /// Clear the status message and its timestamp.
    pub(in crate::tui) fn clear_status(&mut self) {
        self.status.message = None;
        self.status.message_set_at = None;
        self.status.message_sticky = false;
    }

    /// Compute the sticky status text for the current `dispatching` set.
    /// Returns `None` when no dispatch is in flight.
    pub(in crate::tui) fn dispatching_status_text(&self) -> Option<String> {
        let count = self.dispatching.len();
        if count == 0 {
            return None;
        }
        if count == 1 {
            let (&id, _) = self.dispatching.iter().next()?;
            let label = self
                .find_task(id)
                .map(|t| {
                    let trimmed = t.title.trim();
                    if trimmed.is_empty() {
                        format!("task #{}", id.0)
                    } else if trimmed.chars().count() <= TITLE_DISPLAY_LENGTH {
                        format!("'{trimmed}'")
                    } else {
                        let truncated: String =
                            trimmed.chars().take(TITLE_DISPLAY_LENGTH - 1).collect();
                        format!("'{truncated}…'")
                    }
                })
                .unwrap_or_else(|| format!("task #{}", id.0));
            Some(format!("Dispatching {label}…"))
        } else {
            Some(format!("Dispatching {count} tasks…"))
        }
    }

    /// Mark a task as mid-dispatch and update the sticky status text.
    /// This is the single side-effect path for adding to `dispatching`.
    /// No-op if the task ID is not present in the task list.
    ///
    /// UI-only state update — does not perform dispatch. The caller (a
    /// `Command` handler) has already executed the side effect; this
    /// method only records the in-flight UI marker.
    /// Every production caller reaches this only for an unprovisioned task
    /// (`SpansTheClaim` in docs/specs/dispatch.allium) — the dispatch paths filter
    /// on Backlog, and retry-fresh clears the worktree first — but that is not
    /// asserted here. It is a property of the callers, not of this setter, and a
    /// `debug_assert` would fire on any test that drives the marker directly.
    pub(in crate::tui) fn mark_dispatching(&mut self, id: TaskId) {
        if self.find_task(id).is_none() {
            return;
        }
        self.dispatching.insert(id, Instant::now());
        // A retry is the resolution to a stalled chain, so starting one clears
        // the failure marker (PersistsUntilRedispatched in
        // docs/specs/epics.allium) — and keeps a stale marker from masking the
        // retry's own spinner.
        self.agents.auto_dispatch_failed.remove(&id);
        if let Some(msg) = self.dispatching_status_text() {
            self.set_status_sticky(msg);
        }
    }

    /// Remove a task from the dispatching map and recompute the sticky status.
    pub(in crate::tui) fn unmark_dispatching(&mut self, id: TaskId) {
        self.dispatching.remove(&id);
        self.refresh_dispatching_status();
    }

    /// Recompute the sticky status text after `dispatching` has been mutated.
    /// Clears the status if no dispatches remain.
    pub(in crate::tui) fn refresh_dispatching_status(&mut self) {
        match self.dispatching_status_text() {
            Some(msg) => self.set_status_sticky(msg),
            None => {
                if self.status.message_sticky {
                    self.clear_status();
                }
            }
        }
    }

    pub(in crate::tui) fn repo_matches(&self, repo_path: &str) -> bool {
        self.filter.matches(repo_path)
    }

    /// Returns whether the given epic should be shown under the current repo filter.
    /// An epic matches if:
    /// - No repo filter is active, OR
    /// - The epic has no non-archived subtasks (always show empty epics), OR
    /// - At least one non-archived subtask's repo_path matches the filter.
    ///
    pub(in crate::tui) fn epic_repo_matches(&self, epic_id: EpicId) -> bool {
        if let Some(ref cache) = self.layout.epic_filter_cache {
            if let Some(&(repo_matches, _)) = cache.get(&epic_id) {
                return repo_matches;
            }
        }
        let epic_ids = crate::models::descendant_epic_ids(epic_id, &self.board.epics);
        epic_repo_matches_for_ids(&self.board.tasks, &self.filter, &epic_ids)
    }

    pub(in crate::tui) fn epic_matches(&self, epic_id: EpicId) -> bool {
        if let Some(ref cache) = self.layout.epic_filter_cache {
            if let Some(&(_, active_matches)) = cache.get(&epic_id) {
                return active_matches;
            }
        }
        if !self.filter.only_active {
            return true;
        }
        let epic_ids = crate::models::descendant_epic_ids(epic_id, &self.board.epics);
        epic_active_matches_for_ids(&self.board.tasks, &epic_ids)
    }

    /// Whether the epic should be shown under the active board-search query.
    ///
    /// `E`'s own title/id match needs no extra gating: callers (see
    /// [`Self::visible_epics_for_effective_view`]) already require
    /// `epic_matches(E) && epic_repo_matches(E)` before this predicate runs.
    /// A descendant sub-epic or descendant task only counts toward `E`'s
    /// match when it would itself be visible under the repo and only-active
    /// filters — a descendant the board hides cannot keep `E`'s card alive,
    /// since the card would then be a dead end. See board_search_filter in
    /// `docs/specs/core.allium`.
    ///
    /// Deliberately uncached, unlike [`Self::epic_matches`] and
    /// [`Self::epic_repo_matches`]: `layout.epic_filter_cache` is guarded by
    /// `compute_layout_fingerprint()`, which folds ids, status, parent and sort
    /// order but neither titles nor the query — a cached verdict would go stale
    /// on a title edit or a keystroke in the search bar. The empty-query fast
    /// path keeps the non-searching render free.
    pub(in crate::tui) fn epic_search_matches(&self, epic_id: EpicId) -> bool {
        if !self.search_active() {
            return true;
        }
        let query_lower = self.search.query.to_lowercase();
        let id_digits = id_digits_query(&self.search.query);

        let own_match = self
            .board
            .epics
            .iter()
            .find(|e| e.id == epic_id)
            .is_some_and(|e| own_search_match(&e.title, e.id.0, &query_lower, id_digits));
        if own_match {
            return true;
        }

        let epic_ids = crate::models::descendant_epic_ids(epic_id, &self.board.epics);

        let sub_epic_matches = self.board.epics.iter().any(|e| {
            e.id != epic_id
                && epic_ids.contains(&e.id)
                && own_search_match(&e.title, e.id.0, &query_lower, id_digits)
                && self.epic_matches(e.id)
                && self.epic_repo_matches(e.id)
        });
        if sub_epic_matches {
            return true;
        }

        epic_search_matches_for_ids(
            &self.board.tasks,
            &self.filter,
            &epic_ids,
            &query_lower,
            id_digits,
        )
    }

    /// Epics visible in the current board/epic view, filtered by the active
    /// repo / only-active filters and the board-search query: root epics (no
    /// parent) in `Board` mode, direct children of the current epic in `Epic`
    /// mode. Shared by `column_items_for_status_with_view_tasks`,
    /// `column_item_count`, and `column_items_for_visual_column` so an
    /// epic-visibility rule change is made in one place instead of three.
    pub(in crate::tui) fn visible_epics_for_effective_view(&self) -> impl Iterator<Item = &Epic> {
        let parent = match self.effective_view_mode() {
            BoardViewMode::Board(_) => None,
            BoardViewMode::Epic { epic_id, .. } => Some(epic_id),
        };
        self.board
            .epics
            .iter()
            .filter(move |e| e.parent_epic_id == parent)
            .filter(|e| {
                self.epic_matches(e.id)
                    && self.epic_repo_matches(e.id)
                    && self.epic_search_matches(e.id)
            })
    }

    /// Epics eligible as reparent targets for `target`.
    ///
    /// Excludes the target epic and its descendants (cycle prevention), epics in
    /// `Done`/`Archived` status, and epics filtered out by the active repo /
    /// only-active filters (using the same predicates the board uses to decide
    /// epic visibility).
    pub(in crate::tui) fn reparent_target_epics(&self, target: EpicId) -> Vec<&Epic> {
        let excluded = crate::models::descendant_epic_ids(target, &self.board.epics);
        self.board
            .epics
            .iter()
            .filter(|e| {
                !excluded.contains(&e.id)
                    && !matches!(e.status, TaskStatus::Done | TaskStatus::Archived)
                    && self.epic_matches(e.id)
                    && self.epic_repo_matches(e.id)
            })
            .collect()
    }

    /// Epics eligible as move-to-epic targets for a task.
    ///
    /// Unlike [`Self::reparent_target_epics`], there is no descendant exclusion
    /// (a task can never be an ancestor of an epic, so no cycle is possible).
    /// Excludes epics in `Done`/`Archived` status and epics hidden by the
    /// active repo / only-active filters, using the same visibility predicates
    /// the board uses.
    pub(in crate::tui) fn move_task_target_epics(&self) -> Vec<&Epic> {
        self.board
            .epics
            .iter()
            .filter(|e| {
                !matches!(e.status, TaskStatus::Done | TaskStatus::Archived)
                    && self.epic_matches(e.id)
                    && self.epic_repo_matches(e.id)
            })
            .collect()
    }

    /// True when a board-search query is active (non-empty).
    pub(in crate::tui) fn search_active(&self) -> bool {
        !self.search.query.is_empty()
    }

    /// Whether flattened mode applies to `status`. Backlog is excluded from
    /// flattening so that epic cards remain visible in the backlog column.
    fn is_flattened_for_status(&self, status: TaskStatus) -> bool {
        self.board.flattened && status != TaskStatus::Backlog
    }

    /// Return tasks visible in the current view.
    /// Board view: standalone tasks only (epic_id is None).
    /// Epic view: only subtasks of the active epic.
    pub fn tasks_for_current_view(&self) -> Vec<&Task> {
        let repo_match = |t: &&Task| self.repo_matches(&t.repo_path);
        let active_match = |t: &&Task| self.filter.task_matches(t);
        let query_lower = self.search.query.to_lowercase();
        // Parsed once per call, not per task: this is the render hot path.
        let id_digits = id_digits_query(&self.search.query);
        let search_match = |t: &&Task| own_search_match(&t.title, t.id.0, &query_lower, id_digits);
        match self.effective_view_mode() {
            BoardViewMode::Board(_) => self
                .board
                .tasks
                .iter()
                .filter(|t| {
                    t.status != TaskStatus::Archived
                        && (self.is_flattened_for_status(t.status) || t.epic_id.is_none())
                })
                .filter(repo_match)
                .filter(active_match)
                .filter(search_match)
                .collect(),
            BoardViewMode::Epic { epic_id, .. } => {
                let current = epic_id;
                if self.board.flattened {
                    let subtree = crate::models::descendant_task_ids(
                        current,
                        &self.board.epics,
                        &self.board.tasks,
                    );
                    self.board
                        .tasks
                        .iter()
                        .filter(|t| {
                            t.status != TaskStatus::Archived
                                && if self.is_flattened_for_status(t.status) {
                                    subtree.contains(&t.id)
                                } else {
                                    // Backlog excluded from flattening: only direct children
                                    t.epic_id == Some(current)
                                }
                        })
                        .filter(repo_match)
                        .filter(active_match)
                        .filter(search_match)
                        .collect()
                } else {
                    self.board
                        .tasks
                        .iter()
                        .filter(|t| t.epic_id == Some(current) && t.status != TaskStatus::Archived)
                        .filter(repo_match)
                        .filter(active_match)
                        .filter(search_match)
                        .collect()
                }
            }
        }
    }

    /// Return tasks for a given status in the current view.
    pub fn tasks_by_status(&self, status: TaskStatus) -> Vec<&Task> {
        self.tasks_for_current_view()
            .into_iter()
            .filter(|t| t.status == status)
            .collect()
    }

    /// Return all archived tasks, ordered as they appear in self.board.tasks.
    pub fn archived_tasks(&self) -> Vec<&Task> {
        self.board
            .tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Archived)
            .filter(|t| self.repo_matches(&t.repo_path))
            .collect()
    }

    /// Return all archived epics, ordered as they appear in self.board.epics.
    pub fn archived_epics(&self) -> Vec<&Epic> {
        self.board
            .epics
            .iter()
            .filter(|e| e.status == TaskStatus::Archived)
            .collect()
    }

    /// Pre-compute subtask stats for all epics using a pre-built children map.
    /// The `children_map` argument avoids rebuilding the adjacency map per epic.
    fn compute_epic_stats_with_map(
        &self,
        children_map: &HashMap<EpicId, Vec<EpicId>>,
    ) -> EpicStatsMap {
        self.board
            .epics
            .iter()
            .map(|e| {
                (
                    e.id,
                    SubtaskStats::for_epic(e, &self.board.tasks, children_map),
                )
            })
            .collect()
    }

    /// Pre-compute subtask stats for all epics. Call once per render frame.
    pub fn compute_epic_stats(&self) -> EpicStatsMap {
        // Build the parent→children map once so each for_epic call is O(depth)
        // rather than O(epics) — total cost goes from O(epics²) to O(epics).
        let children_map = crate::models::build_children_map(&self.board.epics);
        self.compute_epic_stats_with_map(&children_map)
    }

    /// Return an `Arc`-wrapped `EpicStatsMap`, computing and caching on first call.
    ///
    /// Cloning the returned `Arc` is O(1) (atomic ref-count); the underlying
    /// `HashMap` is not copied.  Also populates `children_map_cache`,
    /// `column_anchor_cache`, and `epic_filter_cache` on first call so that
    /// rendering and navigation handlers can do O(1) lookups without re-scanning.
    ///
    /// Call `invalidate_layout_cache()` whenever `board.tasks` or `board.epics`
    /// are mutated to force a fresh computation on the next call. This is an
    /// optimization, not a correctness requirement: this method compares a
    /// fingerprint of the current board against the one captured when the
    /// cache was last populated, and self-heals (rebuilds) on mismatch even
    /// if invalidation was never called. See `compute_layout_fingerprint()`.
    pub(in crate::tui) fn cached_epic_stats(&mut self) -> Arc<EpicStatsMap> {
        let fingerprint = self.compute_layout_fingerprint();
        if self.layout.epic_stats_cache.is_some()
            && self.layout.layout_cache_fingerprint != Some(fingerprint)
        {
            self.invalidate_layout_cache();
        }
        if self.layout.epic_stats_cache.is_none() {
            // Build the children map once; store it so callers can reuse it.
            let children_map = crate::models::build_children_map(&self.board.epics);
            let stats = Arc::new(self.compute_epic_stats_with_map(&children_map));

            // Build epic_filter_cache: (epic_repo_matches, epic_matches) per epic,
            // using the already-built children_map so descendant traversal is O(1) per epic.
            // Computed before children_map is moved into children_map_cache.
            let filter_cache: HashMap<EpicId, (bool, bool)> = {
                let tasks = &self.board.tasks;
                let filter = &self.filter;
                self.board
                    .epics
                    .iter()
                    .map(|e| {
                        let epic_ids =
                            crate::models::descendant_epic_ids_with_map(e.id, &children_map);
                        let repo_matches = epic_repo_matches_for_ids(tasks, filter, &epic_ids);
                        let active_matches = if !filter.only_active {
                            true
                        } else {
                            epic_active_matches_for_ids(tasks, &epic_ids)
                        };
                        (e.id, (repo_matches, active_matches))
                    })
                    .collect()
            };
            self.layout.epic_filter_cache = Some(filter_cache);
            self.layout.children_map_cache = Some(children_map);

            // Build column_anchor_cache: sorted selectable items per status.
            // Hoist tasks_for_current_view() out of the loop so it's computed once,
            // not once per status.
            let view_tasks = self.tasks_for_current_view();
            let mut anchor_cache: HashMap<TaskStatus, Vec<ColumnAnchor>> = HashMap::new();
            for &status in TaskStatus::ALL.iter() {
                let anchors: Vec<ColumnAnchor> = self
                    .column_items_for_status_with_view_tasks(status, Some(&*stats), &view_tasks)
                    .into_iter()
                    .filter(|i| i.is_selectable())
                    .map(|item| match item {
                        ColumnItem::Task(t) => ColumnAnchor::Task(t.id),
                        ColumnItem::Epic(e) => ColumnAnchor::Epic(e.id),
                        ColumnItem::EpicHeader(_)
                        | ColumnItem::SubstatusLabel(_)
                        | ColumnItem::OrphanSeparator => {
                            unreachable!("is_selectable filters these out")
                        }
                    })
                    .collect();
                anchor_cache.insert(status, anchors);
            }
            self.layout.column_anchor_cache = Some(anchor_cache);

            self.layout.epic_stats_cache = Some(Arc::clone(&stats));
            self.layout.layout_cache_fingerprint = Some(fingerprint);
            return stats;
        }
        if let Some(ref arc) = self.layout.epic_stats_cache {
            Arc::clone(arc)
        } else {
            unreachable!("epic_stats_cache is set in the branch above")
        }
    }

    /// Fingerprint of the fields of `board.tasks`/`board.epics` that feed
    /// `epic_stats_cache`, `children_map_cache`, `column_anchor_cache`, and
    /// `epic_filter_cache`: task/epic id, status, epic membership
    /// (`epic_id`/`parent_epic_id`), and `sort_order`. Two boards with the
    /// same fingerprint necessarily derive the same cached views; a changed
    /// fingerprint means a rebuild is required regardless of whether
    /// `invalidate_layout_cache()` was called.
    ///
    /// Deliberately cheaper than a full rebuild (no allocation, no sorting,
    /// no `HashMap`s, and no cryptographic hashing — a plain FNV-1a fold is
    /// plenty for a non-adversarial in-memory fingerprint) so
    /// `cached_epic_stats()` can call it unconditionally on every
    /// invocation, including the cache-hit fast path.
    fn compute_layout_fingerprint(&self) -> u64 {
        let mut acc = fnv_seed();
        acc = fnv_fold(acc, self.board.tasks.len() as u64);
        for t in &self.board.tasks {
            acc = fnv_fold(acc, t.id.0 as u64);
            acc = fnv_fold(acc, t.status as u64);
            acc = fnv_fold(acc, t.epic_id.map_or(u64::MAX, |e| e.0 as u64));
            acc = fnv_fold(acc, t.sort_order.map_or(u64::MAX, |s| s as u64));
        }
        acc = fnv_fold(acc, self.board.epics.len() as u64);
        for e in &self.board.epics {
            acc = fnv_fold(acc, e.id.0 as u64);
            acc = fnv_fold(acc, e.status as u64);
            acc = fnv_fold(acc, e.parent_epic_id.map_or(u64::MAX, |p| p.0 as u64));
            acc = fnv_fold(acc, e.sort_order.map_or(u64::MAX, |s| s as u64));
        }
        acc
    }

    /// Fingerprint of `board.tasks` id/position only, used to self-heal
    /// `task_index` in `find_task_mut`. Cheaper than
    /// `compute_layout_fingerprint()` (no epics, no status/sort_order) since
    /// `task_index` only maps id → Vec position and doesn't care about
    /// anything else. Catches the case a plain length check misses: a
    /// same-length wholesale replacement of `board.tasks` with a different
    /// id set (a length-only check would wrongly consider the old index
    /// still valid).
    fn compute_task_ids_fingerprint(&self) -> u64 {
        let mut acc = fnv_seed();
        acc = fnv_fold(acc, self.board.tasks.len() as u64);
        for t in &self.board.tasks {
            acc = fnv_fold(acc, t.id.0 as u64);
        }
        acc
    }

    /// Discard all layout caches so the next `cached_epic_stats()` call
    /// recomputes from the current board state. Handlers that mutate
    /// `board.tasks`/`board.epics` should still call this (directly or via
    /// `sync_board_selection`) as a perf optimization — it forces an
    /// immediate rebuild rather than waiting for the next
    /// `cached_epic_stats()` call to detect the fingerprint mismatch — but it
    /// is no longer required for correctness.
    pub(in crate::tui) fn invalidate_layout_cache(&mut self) {
        self.layout.invalidate();
    }

    /// Build a list of items (tasks + epics) for a column in the current view.
    /// In board view, epics are included (positioned by derived status).
    /// In epic view, only subtasks are included (no epic cards).
    ///
    /// Passes `stats = None`: in non-flat mode with epics, epic sort order is derived
    /// by cloning all non-archived subtasks per epic. Prefer
    /// [`Self::column_items_for_status_with_stats`] with pre-computed stats whenever
    /// `compute_epic_stats()` can be called at the same site.
    #[cfg(test)]
    pub(crate) fn column_items_for_status(&self, status: TaskStatus) -> Vec<ColumnItem<'_>> {
        self.column_items_for_status_with_stats(status, None)
    }

    /// Like `column_items_for_status` but uses pre-computed epic stats for sorting.
    ///
    /// Shares the epic filter+sort pipeline with [`Self::column_items_for_visual_column`]:
    /// both apply view-mode epic filtering and sort by `(sort_order, id)`. This method
    /// handles the flat-board layout; the visual-column path handles split-pane layout.
    pub fn column_items_for_status_with_stats<'a>(
        &'a self,
        status: TaskStatus,
        stats: Option<&EpicStatsMap>,
    ) -> Vec<ColumnItem<'a>> {
        let view_tasks = self.tasks_for_current_view();
        self.column_items_for_status_with_view_tasks(status, stats, &view_tasks)
    }

    /// Like `column_items_for_status_with_stats` but accepts pre-computed view tasks,
    /// allowing `tasks_for_current_view()` to be called once and reused across all
    /// columns (e.g. in `ColumnLayout::build`).
    pub(in crate::tui) fn column_items_for_status_with_view_tasks<'a>(
        &'a self,
        status: TaskStatus,
        stats: Option<&EpicStatsMap>,
        view_tasks: &[&'a Task],
    ) -> Vec<ColumnItem<'a>> {
        let tasks: Vec<&'a Task> = view_tasks
            .iter()
            .filter(|t| t.status == status)
            .copied()
            .collect();

        if self.is_flattened_for_status(status) {
            let epic_lookup: HashMap<EpicId, &Epic> =
                self.board.epics.iter().map(|e| (e.id, e)).collect();

            // SubstatusLabel items only make sense in Running/Review columns.
            let show_substatus_labels = matches!(status, TaskStatus::Running | TaskStatus::Review);

            // Sort: (substatus_priority, epic_sort_key, task_sort_key, task_id).
            // Orphan tasks (epic not in board) sort last within each substatus group.
            let mut sorted_tasks = tasks;
            sorted_tasks.sort_by_key(|t| {
                let priority = display_column_priority(t.sub_status, t.is_detached());
                let epic_sk = match t.epic_id.and_then(|eid| epic_lookup.get(&eid)) {
                    Some(e) => e.sort_order.unwrap_or(e.id.0),
                    None => i64::MAX,
                };
                (priority, epic_sk, t.sort_order.unwrap_or(t.id.0), t.id.0)
            });

            // Single pass: emit SubstatusLabel on priority change (Running/Review only),
            // EpicHeader when (priority, epic_id) changes, then the task itself.
            // Tasks are sorted so all items in the same (priority, epic) group are
            // contiguous — no HashSet needed, just track the last-seen pair.
            let mut items: Vec<ColumnItem<'_>> = Vec::new();
            let mut current_priority: Option<u8> = None;
            let mut current_epic_id: Option<EpicId> = None;

            for t in sorted_tasks {
                let detached = t.is_detached();
                let priority = display_column_priority(t.sub_status, detached);
                let priority_changed = Some(priority) != current_priority;
                if priority_changed {
                    current_priority = Some(priority);
                    current_epic_id = None;
                    if show_substatus_labels {
                        items.push(ColumnItem::SubstatusLabel(display_header_label(
                            t.sub_status,
                            detached,
                        )));
                    }
                }

                // Emit OrphanSeparator when transitioning from an epic group to no-epic tasks.
                if t.epic_id.is_none() && current_epic_id.is_some() {
                    items.push(ColumnItem::OrphanSeparator);
                    current_epic_id = None;
                }

                if let Some(eid) = t.epic_id {
                    if let Some(&epic) = epic_lookup.get(&eid) {
                        if Some(eid) != current_epic_id {
                            current_epic_id = Some(eid);
                            items.push(ColumnItem::EpicHeader(epic));
                        }
                    }
                }

                items.push(ColumnItem::Task(t));
            }

            return items;
        }

        // --- Non-flat path (unchanged) ---
        let mut items: Vec<ColumnItem<'_>> = tasks.into_iter().map(ColumnItem::Task).collect();

        for epic in self.visible_epics_for_effective_view() {
            if epic.status == status {
                items.push(ColumnItem::Epic(epic));
            }
        }

        items.sort_by_key(|item| match item {
            ColumnItem::Task(t) => (
                display_column_priority(t.sub_status, t.is_detached()),
                t.sort_order.unwrap_or(t.id.0),
                t.id.0,
            ),
            ColumnItem::Epic(e) => {
                let priority = if let Some(s) = stats.and_then(|m| m.get(&e.id)) {
                    s.substatus.column_priority()
                } else {
                    let subtasks: Vec<&Task> = self
                        .board
                        .tasks
                        .iter()
                        .filter(|t| t.epic_id == Some(e.id) && t.status != TaskStatus::Archived)
                        .collect();
                    epic_substatus(e, &subtasks).column_priority()
                };
                (priority, e.sort_order.unwrap_or(e.id.0), e.id.0)
            }
            ColumnItem::EpicHeader(_) | ColumnItem::OrphanSeparator => {
                unreachable!("EpicHeader/OrphanSeparator never produced in non-flat mode")
            }
            ColumnItem::SubstatusLabel(_) => {
                unreachable!("SubstatusLabel never produced in non-flat mode")
            }
        });

        items
    }

    /// Count selectable column items (tasks + epics) for a status without sorting or
    /// allocating the full item list. Use this wherever only a count is needed —
    /// navigation bounds, clamp guards — rather than calling
    /// `column_items_for_status(s).len()`, which includes non-selectable decorators
    /// (`EpicHeader`, `SubstatusLabel`, `OrphanSeparator`) in flat mode and is O(n log n).
    /// Used by `clamp_selection()` and `handle_navigate_row()`.
    pub(in crate::tui) fn column_item_count(&self, status: TaskStatus) -> usize {
        let task_count = self.tasks_by_status(status).len();
        if self.is_flattened_for_status(status) {
            return task_count;
        }
        let epic_count = self
            .visible_epics_for_effective_view()
            .filter(|e| e.status == status)
            .count();
        task_count + epic_count
    }

    /// Build a list of items (tasks + epics) for a visual column.
    /// Tasks are filtered by parent_status and sub_status matching the visual column.
    /// Running epics are placed in Active or Blocked based on their substatus;
    /// other epics appear in the first visual column of their parent status group.
    ///
    /// Shares the epic filter+sort pipeline with [`Self::column_items_for_status_with_stats`]:
    /// both apply view-mode epic filtering and sort by `(sort_order, id)`. This method
    /// handles the split-pane layout; the status-based path handles the flat-board layout.
    pub fn column_items_for_visual_column(&self, vcol_idx: usize) -> Vec<ColumnItem<'_>> {
        let vcol = &VisualColumn::ALL[vcol_idx];
        let tasks: Vec<&Task> = self
            .tasks_for_current_view()
            .into_iter()
            .filter(|t| t.status == vcol.parent_status && vcol.contains(t.sub_status))
            .collect();

        let mut items: Vec<ColumnItem<'_>> = tasks.into_iter().map(ColumnItem::Task).collect();

        // Populated only for Running-parent epics, whose substatus is already
        // computed below to pick a target column — the sort key reuses that
        // result instead of recomputing epic_substatus per epic.
        let mut running_epic_priority: std::collections::HashMap<EpicId, u8> =
            std::collections::HashMap::new();

        for epic in self.visible_epics_for_effective_view() {
            let epic_parent = epic.status;
            if epic_parent != vcol.parent_status {
                continue;
            }
            if epic_parent == TaskStatus::Running {
                let subtasks: Vec<&Task> = self
                    .board
                    .tasks
                    .iter()
                    .filter(|t| t.epic_id == Some(epic.id) && t.status != TaskStatus::Archived)
                    .collect();
                let substatus = epic_substatus(epic, &subtasks);
                running_epic_priority.insert(epic.id, substatus.column_priority());
                let target_col = if matches!(substatus, EpicSubstatus::Blocked(_)) {
                    2
                } else {
                    1
                };
                if vcol_idx == target_col {
                    items.push(ColumnItem::Epic(epic));
                }
            } else if vcol_idx == VisualColumn::parent_group_start(epic_parent) {
                items.push(ColumnItem::Epic(epic));
            }
        }

        items.sort_by_key(|item| match item {
            ColumnItem::Task(t) => (
                display_column_priority(t.sub_status, t.is_detached()),
                t.sort_order.unwrap_or(t.id.0),
                t.id.0,
            ),
            ColumnItem::Epic(e) => {
                // A visual column already filters epics to a single substatus
                // bucket (see the Running-parent target_col split above), so
                // unlike the flat-board sort this priority never needs to
                // distinguish between epics within the same column — it only
                // has to share the Task arm's tuple shape.
                let priority = running_epic_priority.get(&e.id).copied().unwrap_or(0);
                (priority, e.sort_order.unwrap_or(e.id.0), e.id.0)
            }
            ColumnItem::EpicHeader(_) | ColumnItem::SubstatusLabel(_) | ColumnItem::OrphanSeparator => {
                unreachable!("EpicHeader/SubstatusLabel/OrphanSeparator never produced by column_items_for_visual_column")
            }
        });
        items
    }

    /// Get the statuses of all subtasks belonging to an epic.
    pub(in crate::tui) fn subtask_statuses(&self, epic_id: EpicId) -> Vec<TaskStatus> {
        self.board
            .tasks
            .iter()
            .filter(|t| t.epic_id == Some(epic_id) && t.status != TaskStatus::Archived)
            .map(|t| t.status)
            .collect()
    }

    /// Return the item (task or epic) currently under the cursor.
    ///
    /// Uses the cached `EpicStatsMap` when available (avoids the O(subtasks)
    /// clone that `column_items_for_status` incurs with `stats=None`).
    pub fn selected_column_item(&self) -> Option<ColumnItem<'_>> {
        if self.selection().on_select_all {
            return None;
        }
        let col = self.selection().column();
        if col == 0 || is_edge_column(col) {
            return None;
        }
        let status = TaskStatus::from_column_index(col - 1)?;
        let items = self
            .column_items_for_status_with_stats(status, self.layout.epic_stats_cache.as_deref());
        let row = self.selection().row(col);
        items.into_iter().filter(|i| i.is_selectable()).nth(row)
    }

    /// Look up the title of an epic by ID.
    pub fn epic_title(&self, id: EpicId) -> Option<&str> {
        self.board
            .epics
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.title.as_str())
    }

    /// Return the currently selected task (if the cursor is on a task), or None
    /// if the cursor is on an epic or the column is empty.
    pub fn selected_task(&self) -> Option<&Task> {
        match self.selected_column_item() {
            Some(ColumnItem::Task(task)) => Some(task),
            _ => None,
        }
    }

    /// Clamp all selected_row values to be within bounds for each column.
    pub fn clamp_selection(&mut self) {
        for (idx, &status) in TaskStatus::ALL.iter().enumerate() {
            let nav_col = idx + 1;
            let count = self.column_item_count(status);
            let sel = self.selection_mut();
            if count == 0 {
                sel.set_row(nav_col, 0);
            } else if sel.row(nav_col) >= count {
                sel.set_row(nav_col, count - 1);
            }
        }
    }

    /// Set the selection anchor to the item currently under the cursor.
    /// Called after every navigation keystroke so that subsequent data refreshes
    /// can restore the cursor to this item.
    /// Sets anchor to None when the cursor is on the select-all header.
    ///
    /// Warms the layout cache if needed, then reads from `column_anchor_cache`
    /// in O(1).
    pub(in crate::tui) fn update_anchor_from_current(&mut self) {
        let on_select_all = self.selection().on_select_all;
        if on_select_all {
            self.selection_mut().anchor = None;
            return;
        }
        let col = self.selection().column();
        if col == 0 || col > TaskStatus::COLUMN_COUNT {
            return;
        }
        let row = self.selection().row(col);
        let Some(status) = TaskStatus::from_column_index(col - 1) else {
            return;
        };

        let _ = self.cached_epic_stats(); // warms column_anchor_cache if cold
        let new_anchor = self
            .layout
            .column_anchor_cache
            .as_ref()
            .and_then(|m| m.get(&status))
            .and_then(|v| v.get(row))
            .copied();
        self.selection_mut().anchor = new_anchor;
    }

    /// Restore cursor position from the anchor after a data change.
    /// Scans all columns for the anchor item and moves the cursor to its new
    /// position (following it across columns if needed).
    /// Falls back to index clamping if the anchor is not found.
    pub fn sync_board_selection(&mut self) {
        // Board data has changed; discard stale stats and recompute below.
        self.invalidate_layout_cache();

        let current_col = self.selection().column();

        // If the cursor is on the Archive edge column, preserve the column and only clamp rows.
        if current_col == TaskStatus::COLUMN_COUNT + 1 {
            self.clamp_selection();
            let count = self.archived_tasks().len();
            let archive_col = TaskStatus::COLUMN_COUNT + 1;
            let row = self.selection().row(archive_col);
            let clamped = if count == 0 { 0 } else { row.min(count - 1) };
            self.selection_mut().set_row(archive_col, clamped);
            self.archive.list_state.select(Some(clamped));
            return;
        }

        let anchor = match self.effective_view_mode() {
            BoardViewMode::Board(sel) | BoardViewMode::Epic { selection: sel, .. } => sel.anchor,
        };

        let Some(anchor) = anchor else {
            // on_select_all or no anchor set yet — just clamp
            return self.clamp_selection();
        };

        // Rebuild all layout caches for the fresh board state.
        let _ = self.cached_epic_stats();
        // Search for the anchor in the pre-sorted anchor cache (avoids re-sorting each column).
        let mut found: Option<(usize, usize)> = None;
        if let Some(anchor_map) = &self.layout.column_anchor_cache {
            'outer: for (idx, &status) in TaskStatus::ALL.iter().enumerate() {
                let nav_col = idx + 1;
                if let Some(anchors) = anchor_map.get(&status) {
                    for (row, &item_anchor) in anchors.iter().enumerate() {
                        if item_anchor == anchor {
                            found = Some((nav_col, row));
                            break 'outer;
                        }
                    }
                }
            }
        }

        if let Some((found_col, found_row)) = found {
            for (idx, &status) in TaskStatus::ALL.iter().enumerate() {
                let nav_col = idx + 1;
                if nav_col == found_col {
                    continue;
                }
                let count = self.column_item_count(status);
                let sel = self.selection_mut();
                if count == 0 {
                    sel.set_row(nav_col, 0);
                } else if sel.row(nav_col) >= count {
                    sel.set_row(nav_col, count - 1);
                }
            }
            let sel = self.selection_mut();
            sel.set_column(found_col);
            sel.set_row(found_col, found_row);
            sel.on_select_all = false;
        } else {
            self.clamp_selection();
        }
    }

    pub(in crate::tui) fn reset_column_scroll(&mut self) {
        for state in &mut self.selection_mut().list_states {
            *state.offset_mut() = 0;
        }
    }

    pub(in crate::tui) fn find_task(&self, id: TaskId) -> Option<&Task> {
        self.board.tasks.iter().find(|t| t.id == id)
    }

    pub(in crate::tui) fn find_task_mut(&mut self, id: TaskId) -> Option<&mut Task> {
        // Rebuild index if missing or stale (e.g. direct board.tasks mutation in
        // tests, or a wholesale same-length replacement of board.tasks with a
        // different id set — a length-only check would miss that).
        let fingerprint = self.compute_task_ids_fingerprint();
        if self.layout.task_index.is_none()
            || self.layout.task_index_fingerprint != Some(fingerprint)
        {
            self.layout.task_index = Some(
                self.board
                    .tasks
                    .iter()
                    .enumerate()
                    .map(|(i, t)| (t.id, i))
                    .collect(),
            );
            self.layout.task_index_fingerprint = Some(fingerprint);
        }
        let i = self.layout.task_index.as_ref()?.get(&id).copied()?;
        self.board.tasks.get_mut(i)
    }

    pub(in crate::tui) fn find_epic(&self, id: EpicId) -> Option<&Epic> {
        self.board.epics.iter().find(|e| e.id == id)
    }

    /// Remove all in-memory agent tracking state for a task.
    pub(in crate::tui) fn clear_agent_tracking(&mut self, id: TaskId) {
        self.agents.clear(id);
    }

    /// Take worktree/tmux fields from a task and build a Cleanup command.
    /// Returns `None` if the task has no worktree (still clears tmux_window).
    pub(in crate::tui) fn take_cleanup(task: &mut Task) -> Option<Command> {
        match task.worktree.take() {
            Some(wt) => Some(Command::Task(crate::tui::commands::TaskCommand::Cleanup {
                id: task.id,
                repo_path: task.repo_path.clone(),
                worktree: wt,
                tmux_window: task.tmux_window.take(),
            })),
            None => {
                task.tmux_window.take();
                None
            }
        }
    }

    /// Move a task's status on the board, in step with what the service layer
    /// will write for the same transition: `sub_status` resets to the new
    /// status's default, and a deferred Stop is voided when the card leaves
    /// Running (`clears_pending_stop`, mirroring `PendingStopOnlyWhileRunning`
    /// in `docs/specs/core.allium`).
    ///
    /// The `stop_pending` half is board coherence rather than correctness — the
    /// DB write `Persist` carries is the authority, and the tick reconciler's
    /// own write is conditional on the row. It exists so the board cannot show
    /// a state the row does not have between a move and the next refresh.
    ///
    /// Every board mutation that lands a task in a new status should go through
    /// here; the alternative is remembering two derived fields at each site.
    pub(in crate::tui) fn set_local_status(task: &mut Task, next: TaskStatus) {
        if crate::models::clears_pending_stop(task.status, next) {
            task.stop_pending = false;
        }
        task.status = next;
        task.sub_status = SubStatus::default_for(next);
    }

    /// Take the tmux_window from a task and build a KillTmuxWindow command.
    /// Leaves the worktree intact so the task can be resumed later.
    pub(in crate::tui) fn take_detach(task: &mut Task) -> Option<Command> {
        task.tmux_window.take().map(|window| {
            Command::Task(crate::tui::commands::TaskCommand::KillTmuxWindow { window })
        })
    }

    /// Process a message and return a list of side-effect commands.
    ///
    /// The routing match lives in `dispatcher.rs`; this method is a thin
    /// delegate so adding a `Message` variant is a two-file edit.
    pub fn update(&mut self, msg: Message) -> Vec<Command> {
        dispatcher::dispatch(self, msg)
    }

    // -----------------------------------------------------------------------
    // Per-message handlers
    // -----------------------------------------------------------------------

    pub(in crate::tui) fn handle_detach_tmux(&mut self, ids: Vec<TaskId>) -> Vec<Command> {
        let detachable: Vec<TaskId> = ids
            .iter()
            .filter(|&&id| self.find_task(id).is_some_and(|t| t.tmux_window.is_some()))
            .copied()
            .collect();

        if detachable.is_empty() {
            return vec![];
        }

        let count = detachable.len();
        let msg = if count == 1 {
            "Detach tmux panel? [y/n]".to_string()
        } else {
            format!("Detach {count} tmux panels? [y/n]")
        };
        self.input.mode = InputMode::ConfirmDetachTmux(detachable);
        self.set_status(msg);
        vec![]
    }

    pub(in crate::tui) fn handle_confirm_detach_tmux(&mut self) -> Vec<Command> {
        let InputMode::ConfirmDetachTmux(ref ids) = self.input.mode else {
            return vec![];
        };
        let ids = ids.clone();
        self.input.mode = InputMode::Normal;
        self.clear_status();
        self.detach_tmux_panels(ids)
    }

    pub(in crate::tui) fn detach_tmux_panels(&mut self, ids: Vec<TaskId>) -> Vec<Command> {
        let mut cmds = Vec::new();
        for id in ids {
            self.clear_agent_tracking(id);
            if let Some(task) = self.find_task_mut(id) {
                if let Some(window) = task.tmux_window.take() {
                    cmds.push(Command::Task(
                        crate::tui::commands::TaskCommand::KillTmuxWindow { window },
                    ));
                }
                // Reset sub_status when detaching (e.g. Stale/Crashed -> default)
                if task.sub_status == SubStatus::Stale || task.sub_status == SubStatus::Crashed {
                    task.sub_status = SubStatus::default_for(task.status);
                }
                let task_clone = task.clone();
                cmds.push(Command::Task(crate::tui::commands::TaskCommand::Persist(
                    task_clone,
                )));
            }
            // Drain: the agent genuinely finished its turn, so a pending Stop
            // should land as the Review flip.
            cmds.push(Command::Task(
                crate::tui::commands::TaskCommand::ClearSubagents {
                    id,
                    mode: crate::models::DrainMode::Drain,
                },
            ));
        }
        cmds
    }

    pub(in crate::tui) fn finish_epic_creation(&mut self) -> Vec<Command> {
        let draft = self.input.epic_draft.take().unwrap_or_default();
        self.input.mode = InputMode::Normal;
        self.clear_status();
        vec![Command::Epic(crate::tui::commands::EpicCommand::Insert(
            draft,
        ))]
    }
}

#[cfg(test)]
mod tests;
