use chrono::{DateTime, Utc};

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
    pub done: bool,
    pub plan: Option<String>,
    pub sort_order: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Compute the derived kanban status for an epic based on its subtask statuses.
/// The `done` flag on the epic is the only stored state; everything else is derived.
pub fn epic_status(epic: &Epic, subtask_statuses: &[TaskStatus]) -> TaskStatus {
    if epic.done {
        return TaskStatus::Done;
    }
    if subtask_statuses.is_empty() {
        return TaskStatus::Backlog;
    }

    let all_done = subtask_statuses.iter().all(|s| *s == TaskStatus::Done);
    if all_done {
        return TaskStatus::Review;
    }

    let any_review = subtask_statuses.contains(&TaskStatus::Review);
    if any_review {
        return TaskStatus::Review;
    }

    let any_running = subtask_statuses.contains(&TaskStatus::Running);
    if any_running {
        return TaskStatus::Running;
    }

    TaskStatus::Backlog
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
}

// ---------------------------------------------------------------------------
// Task
// ---------------------------------------------------------------------------

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
    pub needs_input: bool,
    pub pr_url: Option<String>,
    pub tag: Option<String>,
    pub sort_order: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
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
            needs_input: false,
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
            needs_input: false,
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
            done: false,
            plan: None,
            sort_order: None,
            created_at: now,
            updated_at: now,
        };
        assert_eq!(epic.id, EpicId(1));
        assert!(!epic.done);
    }

    // --- epic_status ---

    fn make_epic_for_status(done: bool) -> Epic {
        Epic {
            id: EpicId(1), title: String::new(), description: String::new(),
            repo_path: String::new(), done, plan: None, sort_order: None,
            created_at: Utc::now(), updated_at: Utc::now(),
        }
    }

    #[test]
    fn epic_status_done_flag_overrides() {
        let epic = make_epic_for_status(true);
        assert_eq!(epic_status(&epic, &[]), TaskStatus::Done);
    }

    #[test]
    fn epic_status_no_subtasks_is_backlog() {
        let epic = make_epic_for_status(false);
        assert_eq!(epic_status(&epic, &[]), TaskStatus::Backlog);
    }

    #[test]
    fn epic_status_all_backlog() {
        let epic = make_epic_for_status(false);
        let statuses = [TaskStatus::Backlog, TaskStatus::Backlog];
        assert_eq!(epic_status(&epic, &statuses), TaskStatus::Backlog);
    }

    #[test]
    fn epic_status_some_running() {
        let epic = make_epic_for_status(false);
        let statuses = [TaskStatus::Backlog, TaskStatus::Running];
        assert_eq!(epic_status(&epic, &statuses), TaskStatus::Running);
    }

    #[test]
    fn epic_status_done_and_backlog_is_backlog() {
        let epic = make_epic_for_status(false);
        let statuses = [TaskStatus::Backlog, TaskStatus::Done];
        assert_eq!(epic_status(&epic, &statuses), TaskStatus::Backlog);
    }

    #[test]
    fn epic_status_done_and_running_is_running() {
        let epic = make_epic_for_status(false);
        let statuses = [TaskStatus::Done, TaskStatus::Running];
        assert_eq!(epic_status(&epic, &statuses), TaskStatus::Running);
    }

    #[test]
    fn epic_status_all_done_is_review() {
        let epic = make_epic_for_status(false);
        let statuses = [TaskStatus::Done, TaskStatus::Done];
        assert_eq!(epic_status(&epic, &statuses), TaskStatus::Review);
    }

    #[test]
    fn epic_status_review_beats_running() {
        let epic = make_epic_for_status(false);
        let statuses = [TaskStatus::Running, TaskStatus::Review];
        assert_eq!(epic_status(&epic, &statuses), TaskStatus::Review);
    }

    #[test]
    fn epic_status_some_review() {
        let epic = make_epic_for_status(false);
        let statuses = [TaskStatus::Backlog, TaskStatus::Review];
        assert_eq!(epic_status(&epic, &statuses), TaskStatus::Review);
    }

    #[test]
    fn epic_status_review_with_done() {
        let epic = make_epic_for_status(false);
        let statuses = [TaskStatus::Review, TaskStatus::Done];
        assert_eq!(epic_status(&epic, &statuses), TaskStatus::Review);
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

}
