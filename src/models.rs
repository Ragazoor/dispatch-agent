use chrono::{DateTime, Utc};

// ---------------------------------------------------------------------------
// TaskStatus
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Backlog,
    Ready,
    Running,
    Review,
    Done,
}

impl TaskStatus {
    pub const ALL: &'static [TaskStatus] = &[
        TaskStatus::Backlog,
        TaskStatus::Ready,
        TaskStatus::Running,
        TaskStatus::Review,
        TaskStatus::Done,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Backlog => "backlog",
            TaskStatus::Ready => "ready",
            TaskStatus::Running => "running",
            TaskStatus::Review => "review",
            TaskStatus::Done => "done",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "backlog" => Some(TaskStatus::Backlog),
            "ready" => Some(TaskStatus::Ready),
            "running" => Some(TaskStatus::Running),
            "review" => Some(TaskStatus::Review),
            "done" => Some(TaskStatus::Done),
            _ => None,
        }
    }

    /// Advance to the next status (wraps at Done -> Done).
    pub fn next(self) -> Self {
        match self {
            TaskStatus::Backlog => TaskStatus::Ready,
            TaskStatus::Ready => TaskStatus::Running,
            TaskStatus::Running => TaskStatus::Review,
            TaskStatus::Review => TaskStatus::Done,
            TaskStatus::Done => TaskStatus::Done,
        }
    }

    /// Retreat to the previous status (wraps at Backlog -> Backlog).
    pub fn prev(self) -> Self {
        match self {
            TaskStatus::Backlog => TaskStatus::Backlog,
            TaskStatus::Ready => TaskStatus::Backlog,
            TaskStatus::Running => TaskStatus::Ready,
            TaskStatus::Review => TaskStatus::Running,
            TaskStatus::Done => TaskStatus::Review,
        }
    }

    /// Zero-based column index for kanban board layout.
    pub fn column_index(self) -> usize {
        match self {
            TaskStatus::Backlog => 0,
            TaskStatus::Ready => 1,
            TaskStatus::Running => 2,
            TaskStatus::Review => 3,
            TaskStatus::Done => 4,
        }
    }

    /// Construct from a column index; returns None if out of range.
    pub fn from_column_index(idx: usize) -> Option<Self> {
        match idx {
            0 => Some(TaskStatus::Backlog),
            1 => Some(TaskStatus::Ready),
            2 => Some(TaskStatus::Running),
            3 => Some(TaskStatus::Review),
            4 => Some(TaskStatus::Done),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Task
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Task {
    pub id: i64,
    pub title: String,
    pub description: String,
    pub repo_path: String,
    pub status: TaskStatus,
    pub worktree: Option<String>,
    pub tmux_window: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// NoteSource
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoteSource {
    User,
    Agent,
    System,
}

impl NoteSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            NoteSource::User => "user",
            NoteSource::Agent => "agent",
            NoteSource::System => "system",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "user" => Some(NoteSource::User),
            "agent" => Some(NoteSource::Agent),
            "system" => Some(NoteSource::System),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Note
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Note {
    pub id: i64,
    pub task_id: i64,
    pub content: String,
    pub source: NoteSource,
    pub created_at: DateTime<Utc>,
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
    fn status_next() {
        assert_eq!(TaskStatus::Backlog.next(), TaskStatus::Ready);
        assert_eq!(TaskStatus::Ready.next(), TaskStatus::Running);
        assert_eq!(TaskStatus::Running.next(), TaskStatus::Review);
        assert_eq!(TaskStatus::Review.next(), TaskStatus::Done);
        assert_eq!(TaskStatus::Done.next(), TaskStatus::Done, "Done.next() should stay Done");
    }

    #[test]
    fn status_prev() {
        assert_eq!(TaskStatus::Done.prev(), TaskStatus::Review);
        assert_eq!(TaskStatus::Review.prev(), TaskStatus::Running);
        assert_eq!(TaskStatus::Running.prev(), TaskStatus::Ready);
        assert_eq!(TaskStatus::Ready.prev(), TaskStatus::Backlog);
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
        assert!(TaskStatus::from_column_index(5).is_none());
        assert!(TaskStatus::from_column_index(999).is_none());
    }

    // --- NoteSource ---

    #[test]
    fn note_source_roundtrip() {
        for (src, s) in [
            (NoteSource::User, "user"),
            (NoteSource::Agent, "agent"),
            (NoteSource::System, "system"),
        ] {
            assert_eq!(src.as_str(), s);
            assert_eq!(NoteSource::parse(s), Some(src));
        }
    }

    #[test]
    fn note_source_invalid() {
        assert!(NoteSource::parse("unknown").is_none());
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
}
