use chrono::{DateTime, Utc};

// ---------------------------------------------------------------------------
// Path utilities
// ---------------------------------------------------------------------------

/// Expand a leading `~` or `~/` to the user's home directory.
/// Returns the path unchanged if it doesn't start with `~` or `$HOME` is unset.
pub fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return format!("{}/{rest}", home.to_string_lossy());
        }
    } else if path == "~" {
        if let Some(home) = std::env::var_os("HOME") {
            return home.to_string_lossy().into_owned();
        }
    }
    path.to_string()
}

// ---------------------------------------------------------------------------
// TaskStatus
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Backlog,
    Running,
    Review,
    Done,
    Archived,
}

impl TaskStatus {
    pub const ALL: &'static [TaskStatus] = &[
        TaskStatus::Backlog,
        TaskStatus::Running,
        TaskStatus::Review,
        TaskStatus::Done,
    ];

    pub const COLUMN_COUNT: usize = Self::ALL.len();

    pub fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Backlog => "backlog",
            TaskStatus::Running => "running",
            TaskStatus::Review => "review",
            TaskStatus::Done => "done",
            TaskStatus::Archived => "archived",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "backlog" | "ready" => Some(TaskStatus::Backlog),
            "running" => Some(TaskStatus::Running),
            "review" => Some(TaskStatus::Review),
            "done" => Some(TaskStatus::Done),
            "archived" => Some(TaskStatus::Archived),
            _ => None,
        }
    }

    /// Advance to the next status (wraps at Done -> Done).
    pub fn next(self) -> Self {
        match self {
            TaskStatus::Backlog => TaskStatus::Running,
            TaskStatus::Running => TaskStatus::Review,
            TaskStatus::Review => TaskStatus::Done,
            TaskStatus::Done => TaskStatus::Done,
            TaskStatus::Archived => TaskStatus::Archived,
        }
    }

    /// Retreat to the previous status (wraps at Backlog -> Backlog).
    pub fn prev(self) -> Self {
        match self {
            TaskStatus::Backlog => TaskStatus::Backlog,
            TaskStatus::Running => TaskStatus::Backlog,
            TaskStatus::Review => TaskStatus::Running,
            TaskStatus::Done => TaskStatus::Review,
            TaskStatus::Archived => TaskStatus::Archived,
        }
    }

    /// Zero-based column index for kanban board layout.
    pub fn column_index(self) -> usize {
        match self {
            TaskStatus::Backlog => 0,
            TaskStatus::Running => 1,
            TaskStatus::Review => 2,
            TaskStatus::Done => 3,
            TaskStatus::Archived => 0, // Not displayed in kanban
        }
    }

    /// Construct from a column index; returns None if out of range.
    pub fn from_column_index(idx: usize) -> Option<Self> {
        match idx {
            0 => Some(TaskStatus::Backlog),
            1 => Some(TaskStatus::Running),
            2 => Some(TaskStatus::Review),
            3 => Some(TaskStatus::Done),
            _ => None,
        }
    }
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for TaskStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or_else(|| format!("unknown status: {s}"))
    }
}

// ---------------------------------------------------------------------------
// SubStatus
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubStatus {
    None,
    Active,
    NeedsInput,
    Stale,
    Crashed,
    Conflict,
    AwaitingReview,
    ChangesRequested,
    Approved,
}

impl SubStatus {
    pub const ALL: &'static [SubStatus] = &[
        SubStatus::None,
        SubStatus::Active,
        SubStatus::NeedsInput,
        SubStatus::Stale,
        SubStatus::Crashed,
        SubStatus::Conflict,
        SubStatus::AwaitingReview,
        SubStatus::ChangesRequested,
        SubStatus::Approved,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            SubStatus::None => "none",
            SubStatus::Active => "active",
            SubStatus::NeedsInput => "needs_input",
            SubStatus::Stale => "stale",
            SubStatus::Crashed => "crashed",
            SubStatus::Conflict => "conflict",
            SubStatus::AwaitingReview => "awaiting_review",
            SubStatus::ChangesRequested => "changes_requested",
            SubStatus::Approved => "approved",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "none" => Some(SubStatus::None),
            "active" => Some(SubStatus::Active),
            "needs_input" => Some(SubStatus::NeedsInput),
            "stale" => Some(SubStatus::Stale),
            "crashed" => Some(SubStatus::Crashed),
            "conflict" => Some(SubStatus::Conflict),
            "awaiting_review" => Some(SubStatus::AwaitingReview),
            "changes_requested" => Some(SubStatus::ChangesRequested),
            "approved" => Some(SubStatus::Approved),
            _ => None,
        }
    }

    /// Check whether this sub-status is valid for the given parent status.
    pub fn is_valid_for(&self, status: TaskStatus) -> bool {
        match status {
            TaskStatus::Backlog => matches!(self, SubStatus::None),
            TaskStatus::Running => matches!(
                self,
                SubStatus::Active
                    | SubStatus::NeedsInput
                    | SubStatus::Stale
                    | SubStatus::Crashed
                    | SubStatus::Conflict
            ),
            TaskStatus::Review => matches!(
                self,
                SubStatus::AwaitingReview | SubStatus::ChangesRequested | SubStatus::Approved
            ),
            TaskStatus::Done => matches!(self, SubStatus::None),
            TaskStatus::Archived => matches!(self, SubStatus::None),
        }
    }

    /// Return the default sub-status for a given parent status.
    pub fn default_for(status: TaskStatus) -> Self {
        match status {
            TaskStatus::Backlog => SubStatus::None,
            TaskStatus::Running => SubStatus::Active,
            TaskStatus::Review => SubStatus::AwaitingReview,
            TaskStatus::Done => SubStatus::None,
            TaskStatus::Archived => SubStatus::None,
        }
    }

    /// Sort priority for column grouping (lower = more urgent = top of column).
    pub fn column_priority(self) -> u8 {
        match self {
            SubStatus::Conflict  => 0,
            SubStatus::Crashed   => 1,
            SubStatus::Stale     => 2,
            SubStatus::NeedsInput => 3,
            SubStatus::ChangesRequested => 4,
            SubStatus::Active    => 5,
            SubStatus::AwaitingReview => 5,  // same slot as Active
            SubStatus::None      => 5,
            SubStatus::Approved  => 6,
        }
    }

    /// Label for section header lines within a column.
    pub fn header_label(self) -> &'static str {
        match self {
            SubStatus::None => "",
            SubStatus::Active => "active",
            SubStatus::NeedsInput => "needs input",
            SubStatus::Stale => "stale",
            SubStatus::Crashed => "crashed",
            SubStatus::Conflict => "conflict",
            SubStatus::AwaitingReview => "awaiting review",
            SubStatus::ChangesRequested => "changes requested",
            SubStatus::Approved => "approved",
        }
    }
}

impl std::fmt::Display for SubStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for SubStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or_else(|| format!("unknown sub-status: {s}"))
    }
}

// ---------------------------------------------------------------------------
// VisualColumn — the 8 visual columns for the kanban board
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct VisualColumn {
    pub label: &'static str,
    pub parent_status: TaskStatus,
    pub sub_statuses: &'static [SubStatus],
}

impl VisualColumn {
    pub const COUNT: usize = 8;
    pub const ALL: &'static [VisualColumn] = &[
        VisualColumn { label: "Backlog",    parent_status: TaskStatus::Backlog,  sub_statuses: &[SubStatus::None] },
        VisualColumn { label: "Active",     parent_status: TaskStatus::Running,  sub_statuses: &[SubStatus::Active] },
        VisualColumn { label: "Blocked",    parent_status: TaskStatus::Running,  sub_statuses: &[SubStatus::NeedsInput] },
        VisualColumn { label: "Stale",      parent_status: TaskStatus::Running,  sub_statuses: &[SubStatus::Stale, SubStatus::Crashed, SubStatus::Conflict] },
        VisualColumn { label: "PR Created", parent_status: TaskStatus::Review,   sub_statuses: &[SubStatus::AwaitingReview] },
        VisualColumn { label: "Revise",     parent_status: TaskStatus::Review,   sub_statuses: &[SubStatus::ChangesRequested] },
        VisualColumn { label: "Approved",   parent_status: TaskStatus::Review,   sub_statuses: &[SubStatus::Approved] },
        VisualColumn { label: "Done",       parent_status: TaskStatus::Done,     sub_statuses: &[SubStatus::None] },
    ];

    pub fn contains(&self, sub_status: SubStatus) -> bool {
        self.sub_statuses.contains(&sub_status)
    }

    pub fn parent_group_start(status: TaskStatus) -> usize {
        Self::ALL.iter().position(|vc| vc.parent_status == status).unwrap_or(0)
    }

    pub fn parent_group_span(status: TaskStatus) -> usize {
        Self::ALL.iter().filter(|vc| vc.parent_status == status).count()
    }
}

// ---------------------------------------------------------------------------
// TaskId
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskId(pub i64);

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// EpicId
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EpicId(pub i64);

impl std::fmt::Display for EpicId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// Epic
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Epic {
    pub id: EpicId,
    pub title: String,
    pub description: String,
    pub repo_path: String,
    pub status: TaskStatus,
    pub plan: Option<String>,
    pub sort_order: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Return the stored status for an epic. Previously this was derived from
/// subtask statuses, but now it is persisted and advanced forward by
/// `recalculate_epic_status` in the `TaskStore` trait.
pub fn epic_status(epic: &Epic) -> TaskStatus {
    epic.status
}

// ---------------------------------------------------------------------------
// EpicSubstatus — derived display state for epics
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpicSubstatus {
    // Backlog
    Unplanned,
    Planned,
    // Running
    Active,
    Blocked(usize),
    // Review
    InReview,
    WrappingUp,
    // Done
    Done,
}

impl EpicSubstatus {
    pub fn label(&self) -> String {
        match self {
            Self::Unplanned => "unplanned".into(),
            Self::Planned => "planned".into(),
            Self::Active => "active".into(),
            Self::Blocked(n) => format!("{n} blocked"),
            Self::InReview => "in review".into(),
            Self::WrappingUp => "wrapping up".into(),
            Self::Done => "done".into(),
        }
    }

    /// Priority for sorting within a column, unified with SubStatus priorities
    /// so that epics and tasks share the same section headers.
    pub fn column_priority(&self) -> u8 {
        match self {
            Self::Blocked(_) => 3,  // NeedsInput equivalent
            Self::Active => 5,     // Active equivalent
            Self::WrappingUp => 6, // Approved equivalent
            Self::InReview => 5,   // AwaitingReview equivalent
            Self::Unplanned => 5,
            Self::Planned => 5,
            Self::Done => 5,
        }
    }

    /// Header label for section grouping in the UI, unified with SubStatus header labels.
    pub fn header_label(&self) -> &'static str {
        match self {
            Self::Blocked(_) => "needs input",
            Self::Active => "active",
            Self::InReview => "awaiting review",
            Self::WrappingUp => "approved",
            Self::Unplanned | Self::Planned | Self::Done => "",
        }
    }
}

/// Derive epic substatus from current state. `active_merge_epic` is the epic_id
/// of the currently active merge queue, if any.
pub fn epic_substatus(
    epic: &Epic,
    subtasks: &[Task],
    active_merge_epic: Option<EpicId>,
) -> EpicSubstatus {
    match epic.status {
        TaskStatus::Done | TaskStatus::Archived => EpicSubstatus::Done,
        TaskStatus::Review => {
            if active_merge_epic == Some(epic.id) {
                EpicSubstatus::WrappingUp
            } else {
                EpicSubstatus::InReview
            }
        }
        TaskStatus::Running => {
            let blocked_count = subtasks.iter().filter(|t| {
                t.status == TaskStatus::Running
                    && matches!(
                        t.sub_status,
                        SubStatus::NeedsInput | SubStatus::Stale | SubStatus::Crashed | SubStatus::Conflict
                    )
            }).count();
            if blocked_count > 0 {
                EpicSubstatus::Blocked(blocked_count)
            } else {
                EpicSubstatus::Active
            }
        }
        TaskStatus::Backlog => {
            if epic.plan.is_some() {
                EpicSubstatus::Planned
            } else {
                EpicSubstatus::Unplanned
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ReviewDecision — review status for a PR
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewDecision {
    ReviewRequired,
    WaitingForResponse,
    ChangesRequested,
    Approved,
}

impl ReviewDecision {
    pub const ALL: [Self; 4] = [
        Self::ReviewRequired,
        Self::WaitingForResponse,
        Self::ChangesRequested,
        Self::Approved,
    ];

    pub const COLUMN_COUNT: usize = Self::ALL.len();

    pub fn column_index(self) -> usize {
        match self {
            Self::ReviewRequired => 0,
            Self::WaitingForResponse => 1,
            Self::ChangesRequested => 2,
            Self::Approved => 3,
        }
    }

    pub fn from_column_index(idx: usize) -> Option<Self> {
        match idx {
            0 => Some(Self::ReviewRequired),
            1 => Some(Self::WaitingForResponse),
            2 => Some(Self::ChangesRequested),
            3 => Some(Self::Approved),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ReviewRequired => "Needs Review",
            Self::WaitingForResponse => "Waiting for Response",
            Self::ChangesRequested => "Changes Requested",
            Self::Approved => "Approved",
        }
    }

    /// Stable string for database storage. Not the same as `as_str()` (display)
    /// or `parse()` (GitHub wire format).
    pub fn as_db_str(&self) -> &'static str {
        match self {
            Self::ReviewRequired => "ReviewRequired",
            Self::WaitingForResponse => "WaitingForResponse",
            Self::ChangesRequested => "ChangesRequested",
            Self::Approved => "Approved",
        }
    }

    /// Parse from database string. Inverse of `as_db_str`.
    pub fn from_db_str(s: &str) -> Option<Self> {
        match s {
            "ReviewRequired" => Some(Self::ReviewRequired),
            "WaitingForResponse" => Some(Self::WaitingForResponse),
            "ChangesRequested" => Some(Self::ChangesRequested),
            "Approved" => Some(Self::Approved),
            _ => None,
        }
    }

    /// Parse from GitHub GraphQL `reviewDecision` field value.
    /// Note: `WaitingForResponse` has no wire value — it is derived client-side.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "REVIEW_REQUIRED" => Some(Self::ReviewRequired),
            "CHANGES_REQUESTED" => Some(Self::ChangesRequested),
            "APPROVED" => Some(Self::Approved),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// CiStatus — CI check status for a PR
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiStatus {
    Pending,
    Success,
    Failure,
    None,
}

impl CiStatus {
    pub fn symbol(&self) -> &'static str {
        match self {
            Self::Pending => "\u{23f3}",  // ⏳
            Self::Success => "\u{2713}",  // ✓
            Self::Failure => "\u{2717}",  // ✗
            Self::None => "\u{00b7}",     // ·
        }
    }

    pub fn as_db_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Success => "success",
            Self::Failure => "failure",
            Self::None => "none",
        }
    }

    pub fn from_db_str(s: &str) -> Self {
        match s {
            "pending" => Self::Pending,
            "success" => Self::Success,
            "failure" => Self::Failure,
            _ => Self::None,
        }
    }

    pub fn from_github(s: Option<&str>) -> Self {
        match s {
            Some("SUCCESS") => Self::Success,
            Some("FAILURE") | Some("ERROR") => Self::Failure,
            Some("PENDING") | Some("EXPECTED") => Self::Pending,
            _ => Self::None,
        }
    }
}

// ---------------------------------------------------------------------------
// Reviewer — a reviewer on a PR
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reviewer {
    pub login: String,
    pub decision: Option<ReviewDecision>,
}

// ---------------------------------------------------------------------------
// ReviewPr — a PR the user is expected to review
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ReviewPr {
    pub number: i64,
    pub title: String,
    pub author: String,
    pub repo: String,
    pub url: String,
    pub is_draft: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub additions: i64,
    pub deletions: i64,
    pub review_decision: ReviewDecision,
    pub labels: Vec<String>,
    pub body: String,
    pub head_ref: String,
    pub ci_status: CiStatus,
    pub reviewers: Vec<Reviewer>,
}

// ---------------------------------------------------------------------------
// Task
// ---------------------------------------------------------------------------

pub const DEFAULT_QUICK_TASK_TITLE: &str = "Quick task";

#[derive(Debug, Clone)]
pub struct Task {
    pub id: TaskId,
    pub title: String,
    pub description: String,
    pub repo_path: String,
    pub status: TaskStatus,
    pub worktree: Option<String>,
    pub tmux_window: Option<String>,
    pub plan: Option<String>,
    pub epic_id: Option<EpicId>,
    pub sub_status: SubStatus,
    pub pr_url: Option<String>,
    pub tag: Option<TaskTag>,
    pub sort_order: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// DispatchMode
// ---------------------------------------------------------------------------

/// Determines how a backlog task should be dispatched based on whether it has
/// a plan and its tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchMode {
    Dispatch,
    Brainstorm,
    Plan,
}

impl DispatchMode {
    /// Select the dispatch mode for a task: tasks with a plan always get
    /// `Dispatch`; otherwise the tag drives the choice.
    pub fn for_task(task: &Task) -> Self {
        if task.plan.is_some() {
            DispatchMode::Dispatch
        } else {
            match task.tag {
                Some(TaskTag::Epic) => DispatchMode::Brainstorm,
                Some(TaskTag::Feature) => DispatchMode::Plan,
                _ => DispatchMode::Dispatch,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// TaskTag
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskTag {
    Bug,
    Feature,
    Chore,
    Epic,
}

impl TaskTag {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskTag::Bug => "bug",
            TaskTag::Feature => "feature",
            TaskTag::Chore => "chore",
            TaskTag::Epic => "epic",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "bug" => Some(TaskTag::Bug),
            "feature" => Some(TaskTag::Feature),
            "chore" => Some(TaskTag::Chore),
            "epic" => Some(TaskTag::Epic),
            _ => None,
        }
    }

    pub fn short_label(&self) -> &'static str {
        match self {
            TaskTag::Bug => "bug",
            TaskTag::Feature => "feat",
            TaskTag::Chore => "chore",
            TaskTag::Epic => "epic",
        }
    }
}

impl std::fmt::Display for TaskTag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for TaskTag {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or_else(|| format!("unknown tag: {s}"))
    }
}

// ---------------------------------------------------------------------------
// DispatchResult
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DispatchResult {
    pub worktree_path: String,
    pub tmux_window: String,
}

// ---------------------------------------------------------------------------
// ResumeResult
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ResumeResult {
    pub tmux_window: String,
}

// ---------------------------------------------------------------------------
// TaskUsage
// ---------------------------------------------------------------------------

/// Usage metrics for a single reporting interval (no task_id or timestamp).
#[derive(Debug, Clone)]
pub struct UsageReport {
    pub cost_usd: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
}

/// Accumulated usage stored in the database, keyed by task.
#[derive(Debug, Clone)]
pub struct TaskUsage {
    pub task_id: TaskId,
    pub cost_usd: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// slugify
// ---------------------------------------------------------------------------

/// Convert an arbitrary string into a URL/filesystem-safe slug.
/// - Lowercased
/// - Non-alphanumeric characters replaced with `-`
/// - Consecutive dashes collapsed to one
/// - Leading/trailing dashes trimmed
/// - Returns `"task"` if the result would be empty
pub fn slugify(input: &str) -> String {
    let lower = input.to_lowercase();
    let mut slug = String::with_capacity(lower.len());
    let mut last_was_dash = false;

    for ch in lower.chars() {
        if ch.is_alphanumeric() {
            slug.push(ch);
            last_was_dash = false;
        } else {
            if !last_was_dash && !slug.is_empty() {
                slug.push('-');
                last_was_dash = true;
            }
        }
    }

    // Trim trailing dash
    let slug = slug.trim_end_matches('-').to_string();

    if slug.is_empty() {
        "task".to_string()
    } else {
        slug
    }
}

// ---------------------------------------------------------------------------
// Staleness
// ---------------------------------------------------------------------------

/// Tasks updated within this many hours are considered fresh.
const FRESH_THRESHOLD_HOURS: i64 = 3 * 24; // 3 days
/// Tasks updated within this many hours are aging (not yet stale).
const AGING_THRESHOLD_HOURS: i64 = 7 * 24; // 7 days
/// Days threshold above which format_age switches to weeks.
const WEEKS_THRESHOLD_DAYS: i64 = 14;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Staleness {
    Fresh,  // < 3 days
    Aging,  // 3-7 days
    Stale,  // > 7 days
}

impl Staleness {
    /// Determine staleness tier from the age of `updated_at` relative to `now`.
    pub fn from_age(updated_at: DateTime<Utc>, now: DateTime<Utc>) -> Self {
        let age = now.signed_duration_since(updated_at);
        let hours = age.num_hours().max(0);
        if hours < FRESH_THRESHOLD_HOURS {
            Staleness::Fresh
        } else if hours < AGING_THRESHOLD_HOURS {
            Staleness::Aging
        } else {
            Staleness::Stale
        }
    }
}

// ---------------------------------------------------------------------------
// format_age
// ---------------------------------------------------------------------------

/// Format the age of `updated_at` relative to `now` as a compact label.
/// Returns strings like "<1h", "3h", "2d", "3w".
pub fn format_age(updated_at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let age = now.signed_duration_since(updated_at);
    let hours = age.num_hours().max(0);

    if hours < 1 {
        "<1h".to_string()
    } else if hours < 24 {
        format!("{hours}h")
    } else {
        let days = hours / 24;
        if days < WEEKS_THRESHOLD_DAYS {
            format!("{days}d")
        } else {
            format!("{}w", days / 7)
        }
    }
}

// ---------------------------------------------------------------------------
// format_detail_age
// ---------------------------------------------------------------------------

/// Format age for the detail panel — slightly more verbose than card labels.
/// Returns strings like "less than 1 hour", "1 hour", "5 hours", "1 day", "3 days".
pub fn format_detail_age(updated_at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let age = now.signed_duration_since(updated_at);
    let total_hours = age.num_hours().max(0);

    if total_hours < 1 {
        "less than 1 hour".to_string()
    } else if total_hours == 1 {
        "1 hour".to_string()
    } else if total_hours < 24 {
        format!("{total_hours} hours")
    } else {
        let days = total_hours / 24;
        if days == 1 {
            "1 day".to_string()
        } else {
            format!("{days} days")
        }
    }
}

// ---------------------------------------------------------------------------
// pr_number_from_url
// ---------------------------------------------------------------------------

/// Extract the PR number from a GitHub PR URL.
/// Handles trailing slashes, query parameters, and fragment identifiers.
pub fn pr_number_from_url(url: &str) -> Option<i64> {
    url.split(['?', '#'])
        .next()
        .and_then(|u| u.trim_end_matches('/').rsplit('/').next())
        .and_then(|s| s.parse::<i64>().ok())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- TaskStatus ---

    #[test]
    fn status_roundtrip() {
        for &status in TaskStatus::ALL {
            let s = status.as_str();
            let parsed = TaskStatus::parse(s).expect("roundtrip failed");
            assert_eq!(status, parsed, "roundtrip failed for {:?}", status);
        }
    }

    #[test]
    fn status_invalid_from_str() {
        assert!(TaskStatus::parse("").is_none());
        assert!(TaskStatus::parse("unknown").is_none());
        assert!(TaskStatus::parse("Backlog").is_none(), "should be case-sensitive");
    }

    #[test]
    fn parse_ready_maps_to_backlog() {
        assert_eq!(TaskStatus::parse("ready"), Some(TaskStatus::Backlog));
    }

    #[test]
    fn status_next() {
        assert_eq!(TaskStatus::Backlog.next(), TaskStatus::Running);
        assert_eq!(TaskStatus::Running.next(), TaskStatus::Review);
        assert_eq!(TaskStatus::Review.next(), TaskStatus::Done);
        assert_eq!(TaskStatus::Done.next(), TaskStatus::Done, "Done.next() should stay Done");
    }

    #[test]
    fn status_prev() {
        assert_eq!(TaskStatus::Done.prev(), TaskStatus::Review);
        assert_eq!(TaskStatus::Review.prev(), TaskStatus::Running);
        assert_eq!(TaskStatus::Running.prev(), TaskStatus::Backlog);
        assert_eq!(TaskStatus::Backlog.prev(), TaskStatus::Backlog, "Backlog.prev() should stay Backlog");
    }

    #[test]
    fn status_column_index_roundtrip() {
        for &status in TaskStatus::ALL {
            let idx = status.column_index();
            let back = TaskStatus::from_column_index(idx).expect("column roundtrip failed");
            assert_eq!(status, back);
        }
    }

    #[test]
    fn column_index_out_of_range() {
        assert!(TaskStatus::from_column_index(4).is_none());
        assert!(TaskStatus::from_column_index(999).is_none());
    }

    // --- SubStatus ---

    #[test]
    fn substatus_roundtrip() {
        for &sub in SubStatus::ALL {
            let s = sub.as_str();
            let parsed: SubStatus = s.parse().unwrap_or_else(|e| panic!("roundtrip failed for {s}: {e}"));
            assert_eq!(sub, parsed, "roundtrip failed for {s}");
        }
    }

    #[test]
    fn substatus_as_str_is_snake_case() {
        assert_eq!(SubStatus::None.as_str(), "none");
        assert_eq!(SubStatus::Active.as_str(), "active");
        assert_eq!(SubStatus::NeedsInput.as_str(), "needs_input");
        assert_eq!(SubStatus::Stale.as_str(), "stale");
        assert_eq!(SubStatus::Crashed.as_str(), "crashed");
        assert_eq!(SubStatus::Conflict.as_str(), "conflict");
        assert_eq!(SubStatus::AwaitingReview.as_str(), "awaiting_review");
        assert_eq!(SubStatus::ChangesRequested.as_str(), "changes_requested");
        assert_eq!(SubStatus::Approved.as_str(), "approved");
    }

    #[test]
    fn substatus_from_str_invalid() {
        assert!("bogus".parse::<SubStatus>().is_err());
        assert!("".parse::<SubStatus>().is_err());
        assert!("None".parse::<SubStatus>().is_err(), "should be case-sensitive");
    }

    #[test]
    fn substatus_display() {
        assert_eq!(format!("{}", SubStatus::NeedsInput), "needs_input");
        assert_eq!(format!("{}", SubStatus::AwaitingReview), "awaiting_review");
    }

    #[test]
    fn substatus_valid_combinations() {
        // Backlog: only None
        assert!(SubStatus::None.is_valid_for(TaskStatus::Backlog));
        assert!(!SubStatus::Active.is_valid_for(TaskStatus::Backlog));
        assert!(!SubStatus::NeedsInput.is_valid_for(TaskStatus::Backlog));
        assert!(!SubStatus::AwaitingReview.is_valid_for(TaskStatus::Backlog));

        // Running: Active, NeedsInput, Stale, Crashed
        assert!(!SubStatus::None.is_valid_for(TaskStatus::Running));
        assert!(SubStatus::Active.is_valid_for(TaskStatus::Running));
        assert!(SubStatus::NeedsInput.is_valid_for(TaskStatus::Running));
        assert!(SubStatus::Stale.is_valid_for(TaskStatus::Running));
        assert!(SubStatus::Crashed.is_valid_for(TaskStatus::Running));
        assert!(!SubStatus::AwaitingReview.is_valid_for(TaskStatus::Running));

        // Review: AwaitingReview, ChangesRequested, Approved
        assert!(!SubStatus::None.is_valid_for(TaskStatus::Review));
        assert!(!SubStatus::Active.is_valid_for(TaskStatus::Review));
        assert!(SubStatus::AwaitingReview.is_valid_for(TaskStatus::Review));
        assert!(SubStatus::ChangesRequested.is_valid_for(TaskStatus::Review));
        assert!(SubStatus::Approved.is_valid_for(TaskStatus::Review));

        // Done: only None
        assert!(SubStatus::None.is_valid_for(TaskStatus::Done));
        assert!(!SubStatus::Active.is_valid_for(TaskStatus::Done));

        // Archived: only None
        assert!(SubStatus::None.is_valid_for(TaskStatus::Archived));
        assert!(!SubStatus::Active.is_valid_for(TaskStatus::Archived));
    }

    #[test]
    fn substatus_default_for() {
        assert_eq!(SubStatus::default_for(TaskStatus::Backlog), SubStatus::None);
        assert_eq!(SubStatus::default_for(TaskStatus::Running), SubStatus::Active);
        assert_eq!(SubStatus::default_for(TaskStatus::Review), SubStatus::AwaitingReview);
        assert_eq!(SubStatus::default_for(TaskStatus::Done), SubStatus::None);
        assert_eq!(SubStatus::default_for(TaskStatus::Archived), SubStatus::None);
    }

    #[test]
    fn substatus_rules_consistent_with_visual_columns() {
        // Every SubStatus in a VisualColumn must be valid for that column's parent_status
        for vc in VisualColumn::ALL {
            for &sub in vc.sub_statuses {
                assert!(
                    sub.is_valid_for(vc.parent_status),
                    "{sub:?} in column {:?} but not valid for {:?}",
                    vc.label,
                    vc.parent_status
                );
            }
        }
        // Every valid (status, substatus) pair must appear in exactly one VisualColumn
        for &status in TaskStatus::ALL {
            for &sub in SubStatus::ALL {
                if sub.is_valid_for(status) {
                    let count = VisualColumn::ALL
                        .iter()
                        .filter(|vc| vc.parent_status == status && vc.contains(sub))
                        .count();
                    assert_eq!(
                        count, 1,
                        "{sub:?}/{status:?} is valid but appears in {count} VisualColumns"
                    );
                }
            }
        }
        // default_for() must be valid
        for &status in TaskStatus::ALL {
            let default = SubStatus::default_for(status);
            assert!(
                default.is_valid_for(status),
                "default_for({status:?}) = {default:?} is not valid"
            );
        }
    }

    // --- VisualColumn ---

    #[test]
    fn visual_columns_count_is_8() {
        assert_eq!(VisualColumn::ALL.len(), 8);
        assert_eq!(VisualColumn::COUNT, 8);
        assert_eq!(VisualColumn::ALL.len(), VisualColumn::COUNT);
    }

    #[test]
    fn visual_column_parent_status_mapping() {
        assert_eq!(VisualColumn::ALL[0].parent_status, TaskStatus::Backlog);
        assert_eq!(VisualColumn::ALL[1].parent_status, TaskStatus::Running);
        assert_eq!(VisualColumn::ALL[2].parent_status, TaskStatus::Running);
        assert_eq!(VisualColumn::ALL[3].parent_status, TaskStatus::Running);
        assert_eq!(VisualColumn::ALL[4].parent_status, TaskStatus::Review);
        assert_eq!(VisualColumn::ALL[5].parent_status, TaskStatus::Review);
        assert_eq!(VisualColumn::ALL[6].parent_status, TaskStatus::Review);
        assert_eq!(VisualColumn::ALL[7].parent_status, TaskStatus::Done);
    }

    #[test]
    fn visual_column_contains_substatus() {
        // Column 3 ("Stale") contains Stale and Crashed, but not Active
        let stale_col = &VisualColumn::ALL[3];
        assert!(stale_col.contains(SubStatus::Stale));
        assert!(stale_col.contains(SubStatus::Crashed));
        assert!(!stale_col.contains(SubStatus::Active));
    }

    #[test]
    fn visual_column_parent_group_start() {
        assert_eq!(VisualColumn::parent_group_start(TaskStatus::Backlog), 0);
        assert_eq!(VisualColumn::parent_group_start(TaskStatus::Running), 1);
        assert_eq!(VisualColumn::parent_group_start(TaskStatus::Review), 4);
        assert_eq!(VisualColumn::parent_group_start(TaskStatus::Done), 7);
    }

    #[test]
    fn visual_column_parent_group_span() {
        assert_eq!(VisualColumn::parent_group_span(TaskStatus::Backlog), 1);
        assert_eq!(VisualColumn::parent_group_span(TaskStatus::Running), 3);
        assert_eq!(VisualColumn::parent_group_span(TaskStatus::Review), 3);
        assert_eq!(VisualColumn::parent_group_span(TaskStatus::Done), 1);
    }

    // --- slugify ---

    #[test]
    fn slugify_normal() {
        assert_eq!(slugify("Hello World"), "hello-world");
    }

    #[test]
    fn slugify_special_chars() {
        assert_eq!(slugify("Foo & Bar! (baz)"), "foo-bar-baz");
    }

    #[test]
    fn slugify_empty() {
        assert_eq!(slugify(""), "task");
    }

    #[test]
    fn slugify_only_special() {
        assert_eq!(slugify("!!!"), "task");
    }

    #[test]
    fn slugify_collapsed_dashes() {
        assert_eq!(slugify("a---b"), "a-b");
        assert_eq!(slugify("a & & b"), "a-b");
    }

    #[test]
    fn slugify_leading_trailing_special() {
        assert_eq!(slugify("  hello  "), "hello");
        assert_eq!(slugify("---hello---"), "hello");
    }

    #[test]
    fn slugify_numbers() {
        assert_eq!(slugify("Task 42"), "task-42");
    }

    #[test]
    fn column_count_matches_all_len() {
        assert_eq!(TaskStatus::COLUMN_COUNT, TaskStatus::ALL.len());
        assert_eq!(TaskStatus::COLUMN_COUNT, 4);
    }

    #[test]
    fn task_status_display() {
        for &status in TaskStatus::ALL {
            assert_eq!(format!("{status}"), status.as_str());
        }
    }

    #[test]
    fn task_status_from_str_roundtrip() {
        for &status in TaskStatus::ALL {
            let parsed: TaskStatus = status.as_str().parse().unwrap();
            assert_eq!(parsed, status);
        }
    }

    #[test]
    fn task_status_from_str_error() {
        let result: Result<TaskStatus, _> = "bogus".parse();
        assert!(result.is_err());
    }

    #[test]
    fn status_archived_roundtrip() {
        let s = TaskStatus::Archived.as_str();
        assert_eq!(s, "archived");
        let parsed = TaskStatus::parse(s).expect("roundtrip failed");
        assert_eq!(parsed, TaskStatus::Archived);
    }

    #[test]
    fn status_archived_is_terminal() {
        assert_eq!(TaskStatus::Archived.next(), TaskStatus::Archived);
        assert_eq!(TaskStatus::Archived.prev(), TaskStatus::Archived);
    }

    #[test]
    fn status_archived_has_no_column() {
        // Archived is not a kanban column — COLUMN_COUNT stays 4
        assert_eq!(TaskStatus::COLUMN_COUNT, 4);
    }

    // --- Staleness ---

    #[test]
    fn staleness_fresh() {
        let now = Utc::now();
        let updated = now - chrono::Duration::hours(71);
        assert_eq!(Staleness::from_age(updated, now), Staleness::Fresh);
    }

    #[test]
    fn staleness_fresh_boundary() {
        let now = Utc::now();
        // Exactly 3 days minus 1 second => still Fresh
        let updated = now - chrono::Duration::seconds(3 * 24 * 3600 - 1);
        assert_eq!(Staleness::from_age(updated, now), Staleness::Fresh);
    }

    #[test]
    fn staleness_aging() {
        let now = Utc::now();
        let updated = now - chrono::Duration::days(3);
        assert_eq!(Staleness::from_age(updated, now), Staleness::Aging);
    }

    #[test]
    fn staleness_aging_boundary() {
        let now = Utc::now();
        // Exactly 7 days minus 1 second => still Aging
        let updated = now - chrono::Duration::seconds(7 * 24 * 3600 - 1);
        assert_eq!(Staleness::from_age(updated, now), Staleness::Aging);
    }

    #[test]
    fn staleness_stale() {
        let now = Utc::now();
        let updated = now - chrono::Duration::days(7);
        assert_eq!(Staleness::from_age(updated, now), Staleness::Stale);
    }

    #[test]
    fn staleness_very_stale() {
        let now = Utc::now();
        let updated = now - chrono::Duration::days(30);
        assert_eq!(Staleness::from_age(updated, now), Staleness::Stale);
    }

    #[test]
    fn staleness_future_is_fresh() {
        let now = Utc::now();
        let updated = now + chrono::Duration::hours(1);
        assert_eq!(Staleness::from_age(updated, now), Staleness::Fresh);
    }

    // --- format_age ---

    #[test]
    fn format_age_minutes() {
        let now = Utc::now();
        let updated = now - chrono::Duration::minutes(30);
        assert_eq!(format_age(updated, now), "<1h");
    }

    #[test]
    fn format_age_one_hour() {
        let now = Utc::now();
        let updated = now - chrono::Duration::hours(1);
        assert_eq!(format_age(updated, now), "1h");
    }

    #[test]
    fn format_age_hours() {
        let now = Utc::now();
        let updated = now - chrono::Duration::hours(23);
        assert_eq!(format_age(updated, now), "23h");
    }

    #[test]
    fn format_age_one_day() {
        let now = Utc::now();
        let updated = now - chrono::Duration::hours(24);
        assert_eq!(format_age(updated, now), "1d");
    }

    #[test]
    fn format_age_days() {
        let now = Utc::now();
        let updated = now - chrono::Duration::days(5);
        assert_eq!(format_age(updated, now), "5d");
    }

    #[test]
    fn format_age_thirteen_days() {
        let now = Utc::now();
        let updated = now - chrono::Duration::days(13);
        assert_eq!(format_age(updated, now), "13d");
    }

    #[test]
    fn format_age_two_weeks() {
        let now = Utc::now();
        let updated = now - chrono::Duration::days(14);
        assert_eq!(format_age(updated, now), "2w");
    }

    #[test]
    fn format_age_three_weeks() {
        let now = Utc::now();
        let updated = now - chrono::Duration::days(21);
        assert_eq!(format_age(updated, now), "3w");
    }

    #[test]
    fn format_age_future() {
        let now = Utc::now();
        let updated = now + chrono::Duration::hours(5);
        assert_eq!(format_age(updated, now), "<1h");
    }

    // --- format_detail_age ---

    #[test]
    fn format_detail_age_minutes() {
        let now = Utc::now();
        let updated = now - chrono::Duration::minutes(30);
        assert_eq!(format_detail_age(updated, now), "less than 1 hour");
    }

    #[test]
    fn format_detail_age_one_hour() {
        let now = Utc::now();
        let updated = now - chrono::Duration::hours(1);
        assert_eq!(format_detail_age(updated, now), "1 hour");
    }

    #[test]
    fn format_detail_age_hours() {
        let now = Utc::now();
        let updated = now - chrono::Duration::hours(5);
        assert_eq!(format_detail_age(updated, now), "5 hours");
    }

    #[test]
    fn format_detail_age_one_day() {
        let now = Utc::now();
        let updated = now - chrono::Duration::hours(24);
        assert_eq!(format_detail_age(updated, now), "1 day");
    }

    #[test]
    fn format_detail_age_days() {
        let now = Utc::now();
        let updated = now - chrono::Duration::days(10);
        assert_eq!(format_detail_age(updated, now), "10 days");
    }

    #[test]
    fn format_detail_age_future() {
        let now = Utc::now();
        let updated = now + chrono::Duration::hours(3);
        assert_eq!(format_detail_age(updated, now), "less than 1 hour");
    }

    // --- EpicId ---

    #[test]
    fn epic_id_display() {
        let id = EpicId(42);
        assert_eq!(format!("{id}"), "42");
    }

    #[test]
    fn epic_id_equality() {
        assert_eq!(EpicId(1), EpicId(1));
        assert_ne!(EpicId(1), EpicId(2));
    }

    #[test]
    fn task_epic_id_defaults_to_none() {
        let now = Utc::now();
        let task = Task {
            id: TaskId(1),
            title: "Test".to_string(),
            description: "Desc".to_string(),
            repo_path: "/repo".to_string(),
            status: TaskStatus::Backlog,
            worktree: None,
            tmux_window: None,
            plan: None,
            epic_id: None,
            sub_status: SubStatus::None,
            pr_url: None,
            tag: None,
            sort_order: None,
            created_at: now,
            updated_at: now,
        };
        assert!(task.epic_id.is_none());
    }

    #[test]
    fn task_with_epic_id() {
        let now = Utc::now();
        let task = Task {
            id: TaskId(1),
            title: "Test".to_string(),
            description: "Desc".to_string(),
            repo_path: "/repo".to_string(),
            status: TaskStatus::Backlog,
            worktree: None,
            tmux_window: None,
            plan: None,
            epic_id: Some(EpicId(5)),
            sub_status: SubStatus::None,
            pr_url: None,
            tag: None,
            sort_order: None,
            created_at: now,
            updated_at: now,
        };
        assert_eq!(task.epic_id, Some(EpicId(5)));
    }

    #[test]
    fn epic_struct_fields() {
        let now = Utc::now();
        let epic = Epic {
            id: EpicId(1),
            title: "Auth Rewrite".to_string(),
            description: "Rewrite auth system".to_string(),
            repo_path: "/repo".to_string(),
            status: TaskStatus::Backlog,
            plan: None,
            sort_order: None,
            created_at: now,
            updated_at: now,
        };
        assert_eq!(epic.id, EpicId(1));
        assert_eq!(epic.status, TaskStatus::Backlog);
    }

    // --- epic_status ---

    fn make_epic_for_status(status: TaskStatus) -> Epic {
        Epic {
            id: EpicId(1), title: String::new(), description: String::new(),
            repo_path: String::new(), status, plan: None, sort_order: None,
            created_at: Utc::now(), updated_at: Utc::now(),
        }
    }

    #[test]
    fn epic_status_returns_stored_status() {
        let epic = make_epic_for_status(TaskStatus::Done);
        assert_eq!(epic_status(&epic), TaskStatus::Done);

        let epic = make_epic_for_status(TaskStatus::Backlog);
        assert_eq!(epic_status(&epic), TaskStatus::Backlog);

        let epic = make_epic_for_status(TaskStatus::Running);
        assert_eq!(epic_status(&epic), TaskStatus::Running);

        let epic = make_epic_for_status(TaskStatus::Review);
        assert_eq!(epic_status(&epic), TaskStatus::Review);
    }

    // --- ReviewDecision ---

    #[test]
    fn review_decision_column_count() {
        assert_eq!(ReviewDecision::COLUMN_COUNT, 4);
        assert_eq!(ReviewDecision::ALL.len(), 4);
    }

    #[test]
    fn review_decision_column_index_roundtrip() {
        for (i, decision) in ReviewDecision::ALL.iter().enumerate() {
            assert_eq!(decision.column_index(), i);
        }
    }

    #[test]
    fn review_decision_from_column_index() {
        assert_eq!(ReviewDecision::from_column_index(0), Some(ReviewDecision::ReviewRequired));
        assert_eq!(ReviewDecision::from_column_index(1), Some(ReviewDecision::WaitingForResponse));
        assert_eq!(ReviewDecision::from_column_index(2), Some(ReviewDecision::ChangesRequested));
        assert_eq!(ReviewDecision::from_column_index(3), Some(ReviewDecision::Approved));
        assert_eq!(ReviewDecision::from_column_index(4), None);
    }

    #[test]
    fn review_decision_as_str() {
        assert_eq!(ReviewDecision::ReviewRequired.as_str(), "Needs Review");
        assert_eq!(ReviewDecision::ChangesRequested.as_str(), "Changes Requested");
        assert_eq!(ReviewDecision::Approved.as_str(), "Approved");
    }

    #[test]
    fn review_decision_parse() {
        assert_eq!(ReviewDecision::parse("REVIEW_REQUIRED"), Some(ReviewDecision::ReviewRequired));
        assert_eq!(ReviewDecision::parse("CHANGES_REQUESTED"), Some(ReviewDecision::ChangesRequested));
        assert_eq!(ReviewDecision::parse("APPROVED"), Some(ReviewDecision::Approved));
        assert_eq!(ReviewDecision::parse("bogus"), None);
        assert_eq!(ReviewDecision::parse(""), None);
    }

    // --- pr_number_from_url ---

    #[test]
    fn pr_number_from_standard_url() {
        assert_eq!(pr_number_from_url("https://github.com/org/repo/pull/42"), Some(42));
    }

    #[test]
    fn pr_number_from_url_with_trailing_slash() {
        assert_eq!(pr_number_from_url("https://github.com/org/repo/pull/42/"), Some(42));
    }

    #[test]
    fn pr_number_from_url_with_query_params() {
        assert_eq!(pr_number_from_url("https://github.com/org/repo/pull/42?diff=split"), Some(42));
    }

    #[test]
    fn pr_number_from_url_no_number() {
        assert_eq!(pr_number_from_url("https://github.com/org/repo"), None);
    }

    #[test]
    fn pr_number_from_empty_url() {
        assert_eq!(pr_number_from_url(""), None);
    }

    #[test]
    fn pr_number_from_url_with_fragment() {
        assert_eq!(pr_number_from_url("https://github.com/org/repo/pull/42#issuecomment-123"), Some(42));
    }

    #[test]
    fn review_decision_db_roundtrip() {
        for decision in ReviewDecision::ALL {
            let s = decision.as_db_str();
            let parsed = ReviewDecision::from_db_str(s)
                .unwrap_or_else(|| panic!("failed to parse db str: {s}"));
            assert_eq!(parsed, decision);
        }
    }

    // --- EpicSubstatus ---

    fn test_epic() -> Epic {
        Epic {
            id: EpicId(1),
            title: "Test".to_string(),
            description: "".to_string(),
            repo_path: "/repo".to_string(),
            status: TaskStatus::Backlog,
            plan: None,
            sort_order: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn test_task() -> Task {
        Task {
            id: TaskId(1),
            title: "T".to_string(),
            description: "".to_string(),
            repo_path: "/repo".to_string(),
            status: TaskStatus::Backlog,
            sub_status: SubStatus::None,
            worktree: None,
            tmux_window: None,
            plan: None,
            epic_id: None,
            pr_url: None,
            tag: None,
            sort_order: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn epic_substatus_unplanned() {
        let epic = Epic { plan: None, status: TaskStatus::Backlog, ..test_epic() };
        assert_eq!(epic_substatus(&epic, &[], None), EpicSubstatus::Unplanned);
    }

    #[test]
    fn epic_substatus_planned() {
        let epic = Epic { plan: Some("plan.md".into()), status: TaskStatus::Backlog, ..test_epic() };
        assert_eq!(epic_substatus(&epic, &[], None), EpicSubstatus::Planned);
    }

    #[test]
    fn epic_substatus_active_with_backlog() {
        let epic = Epic { status: TaskStatus::Running, ..test_epic() };
        let subtasks = vec![
            Task { status: TaskStatus::Running, sub_status: SubStatus::Active, ..test_task() },
            Task { status: TaskStatus::Backlog, ..test_task() },
        ];
        assert_eq!(epic_substatus(&epic, &subtasks, None), EpicSubstatus::Active);
    }

    #[test]
    fn epic_substatus_active_all_running() {
        let epic = Epic { status: TaskStatus::Running, ..test_epic() };
        let subtasks = vec![
            Task { status: TaskStatus::Running, sub_status: SubStatus::Active, ..test_task() },
            Task { status: TaskStatus::Done, sub_status: SubStatus::None, ..test_task() },
        ];
        assert_eq!(epic_substatus(&epic, &subtasks, None), EpicSubstatus::Active);
    }

    #[test]
    fn epic_substatus_blocked_stale() {
        let epic = Epic { status: TaskStatus::Running, ..test_epic() };
        let subtasks = vec![
            Task { status: TaskStatus::Running, sub_status: SubStatus::Stale, ..test_task() },
            Task { status: TaskStatus::Backlog, ..test_task() },
        ];
        assert_eq!(epic_substatus(&epic, &subtasks, None), EpicSubstatus::Blocked(1));
    }

    #[test]
    fn epic_substatus_blocked_needs_input() {
        let epic = Epic { status: TaskStatus::Running, ..test_epic() };
        let subtasks = vec![
            Task { status: TaskStatus::Running, sub_status: SubStatus::NeedsInput, ..test_task() },
            Task { status: TaskStatus::Running, sub_status: SubStatus::Active, ..test_task() },
        ];
        assert_eq!(epic_substatus(&epic, &subtasks, None), EpicSubstatus::Blocked(1));
    }

    #[test]
    fn epic_substatus_blocked_count() {
        let epic = Epic { status: TaskStatus::Running, ..test_epic() };
        let subtasks = vec![
            Task { status: TaskStatus::Running, sub_status: SubStatus::NeedsInput, ..test_task() },
            Task { status: TaskStatus::Running, sub_status: SubStatus::Stale, ..test_task() },
            Task { status: TaskStatus::Running, sub_status: SubStatus::Active, ..test_task() },
        ];
        assert_eq!(epic_substatus(&epic, &subtasks, None), EpicSubstatus::Blocked(2));
    }

    #[test]
    fn epic_substatus_in_review() {
        let epic = Epic { status: TaskStatus::Review, ..test_epic() };
        assert_eq!(epic_substatus(&epic, &[], None), EpicSubstatus::InReview);
    }

    #[test]
    fn epic_substatus_wrapping_up() {
        let epic = Epic { status: TaskStatus::Review, ..test_epic() };
        assert_eq!(epic_substatus(&epic, &[], Some(EpicId(1))), EpicSubstatus::WrappingUp);
    }

    #[test]
    fn epic_substatus_done() {
        let epic = Epic { status: TaskStatus::Done, ..test_epic() };
        assert_eq!(epic_substatus(&epic, &[], None), EpicSubstatus::Done);
    }

    // --- DispatchMode ---

    fn make_task_with(plan: Option<&str>, tag: Option<TaskTag>) -> Task {
        let now = chrono::Utc::now();
        Task {
            id: TaskId(1),
            title: String::new(),
            description: String::new(),
            repo_path: String::new(),
            status: TaskStatus::Backlog,
            worktree: None,
            tmux_window: None,
            plan: plan.map(String::from),
            epic_id: None,
            sub_status: SubStatus::None,
            pr_url: None,
            tag,
            sort_order: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn dispatch_mode_with_plan_always_dispatches() {
        assert_eq!(
            DispatchMode::for_task(&make_task_with(Some("a plan"), None)),
            DispatchMode::Dispatch
        );
        assert_eq!(
            DispatchMode::for_task(&make_task_with(Some("a plan"), Some(TaskTag::Epic))),
            DispatchMode::Dispatch
        );
        assert_eq!(
            DispatchMode::for_task(&make_task_with(Some("a plan"), Some(TaskTag::Feature))),
            DispatchMode::Dispatch
        );
    }

    #[test]
    fn dispatch_mode_without_plan_uses_tag() {
        assert_eq!(
            DispatchMode::for_task(&make_task_with(None, Some(TaskTag::Epic))),
            DispatchMode::Brainstorm
        );
        assert_eq!(
            DispatchMode::for_task(&make_task_with(None, Some(TaskTag::Feature))),
            DispatchMode::Plan
        );
        assert_eq!(
            DispatchMode::for_task(&make_task_with(None, Some(TaskTag::Chore))),
            DispatchMode::Dispatch
        );
        assert_eq!(
            DispatchMode::for_task(&make_task_with(None, None)),
            DispatchMode::Dispatch
        );
    }
}
