use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use super::{SubStatus, Task, TaskId, TaskStatus};
use crate::define_id_newtype;

define_id_newtype!(EpicId, epic_id_tests);

// ---------------------------------------------------------------------------
// Epic
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Epic {
    pub id: EpicId,
    pub title: String,
    pub description: String,
    pub status: TaskStatus,
    pub plan_path: Option<String>,
    pub sort_order: Option<i64>,
    pub auto_dispatch: bool,
    pub parent_epic_id: Option<EpicId>,
    pub feed_command: Option<String>,
    pub feed_interval_secs: Option<i64>,
    pub group_by_repo: bool,
    pub feed_role: FeedRole,
    pub origin: EpicOrigin,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Epic {
    /// True when this epic participates in MANUAL repo-grouping: it has
    /// group_by_repo enabled and is not a feed epic (feed epics group via
    /// ingestion, not the manual router).
    pub fn can_auto_group(&self) -> bool {
        self.group_by_repo && self.feed_command.is_none()
    }
}

// ---------------------------------------------------------------------------
// FeedRole — routing role of an epic within a PR-review feed hierarchy
// ---------------------------------------------------------------------------

/// The routing role an epic plays within a feed hierarchy. `None` is the
/// default for ordinary epics; the other variants tag a feed sub-epic so PR
/// items can be routed to the right bucket (reviews requested of me, my team,
/// bots, CVEs, etc.). Stored in the SQLite `epics.feed_role` TEXT column as the
/// kebab-case string (mirrors `TaskTag`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FeedRole {
    #[default]
    None,
    ReviewsParent,
    MyReviews,
    TeamReviews,
    Bots,
    Cve,
}

impl FeedRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            FeedRole::None => "none",
            FeedRole::ReviewsParent => "reviews-parent",
            FeedRole::MyReviews => "my-reviews",
            FeedRole::TeamReviews => "team-reviews",
            FeedRole::Bots => "bots",
            FeedRole::Cve => "cve",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "none" => Some(FeedRole::None),
            "reviews-parent" => Some(FeedRole::ReviewsParent),
            "my-reviews" => Some(FeedRole::MyReviews),
            "team-reviews" => Some(FeedRole::TeamReviews),
            "bots" => Some(FeedRole::Bots),
            "cve" => Some(FeedRole::Cve),
            _ => None,
        }
    }
}

impl std::fmt::Display for FeedRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for FeedRole {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        FeedRole::parse(s).ok_or(())
    }
}

// ---------------------------------------------------------------------------
// EpicOrigin — how an epic came to exist
// ---------------------------------------------------------------------------

/// How an epic was created. `Manual` is any user-created epic (TUI / MCP).
/// `RepoGroup` is an auto-created per-repo sub-epic produced by repo grouping.
/// Stored in the SQLite `epics.origin` TEXT column as the kebab-case string.
/// This is the safety boundary for grouping: routing, flatten, and reroute only
/// ever touch `RepoGroup` sub-epics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EpicOrigin {
    #[default]
    Manual,
    RepoGroup,
}

impl EpicOrigin {
    pub fn as_str(&self) -> &'static str {
        match self {
            EpicOrigin::Manual => "manual",
            EpicOrigin::RepoGroup => "repo-group",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "manual" => Some(EpicOrigin::Manual),
            "repo-group" => Some(EpicOrigin::RepoGroup),
            _ => None,
        }
    }
}

impl std::fmt::Display for EpicOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for EpicOrigin {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        EpicOrigin::parse(s).ok_or(())
    }
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
            Self::Done => "done".into(),
        }
    }

    /// Priority for sorting within a column, unified with SubStatus priorities
    /// so that epics and tasks share the same section headers.
    pub fn column_priority(&self) -> u8 {
        match self {
            Self::Blocked(_) => SubStatus::NeedsInput.column_priority(),
            Self::Active => SubStatus::Active.column_priority(),
            Self::InReview => SubStatus::AwaitingReview.column_priority(),
            Self::Unplanned | Self::Planned | Self::Done => SubStatus::None.column_priority(),
        }
    }

    /// Header label for section grouping in the UI, unified with SubStatus header labels.
    pub fn header_label(&self) -> &'static str {
        match self {
            Self::Blocked(_) => "needs input",
            Self::Active => "active",
            Self::InReview => "awaiting review",
            Self::Unplanned | Self::Planned | Self::Done => "",
        }
    }
}

/// Derive epic substatus from current state.
pub fn epic_substatus(epic: &Epic, subtasks: &[&Task]) -> EpicSubstatus {
    match epic.status {
        TaskStatus::Done | TaskStatus::Archived => EpicSubstatus::Done,
        TaskStatus::Review => EpicSubstatus::InReview,
        TaskStatus::Running => {
            let blocked_count = subtasks
                .iter()
                .filter(|t| {
                    t.status == TaskStatus::Running
                        && matches!(
                            t.sub_status,
                            SubStatus::NeedsInput
                                | SubStatus::Stale
                                | SubStatus::Crashed
                                | SubStatus::Conflict
                        )
                })
                .count();
            if blocked_count > 0 {
                EpicSubstatus::Blocked(blocked_count)
            } else {
                EpicSubstatus::Active
            }
        }
        TaskStatus::Backlog => {
            if epic.plan_path.is_some() {
                EpicSubstatus::Planned
            } else {
                EpicSubstatus::Unplanned
            }
        }
    }
}

/// Build a parent→children adjacency map over `epics`.
///
/// Calling this once and passing the result to [`descendant_epic_ids_with_map`]
/// for each epic in a loop reduces the cost from O(epics²) to O(epics).
pub fn build_children_map(epics: &[Epic]) -> std::collections::HashMap<EpicId, Vec<EpicId>> {
    let mut children: std::collections::HashMap<EpicId, Vec<EpicId>> =
        std::collections::HashMap::new();
    for epic in epics {
        if let Some(parent) = epic.parent_epic_id {
            children.entry(parent).or_default().push(epic.id);
        }
    }
    children
}

/// Collect all descendant epic IDs of `root` using a prebuilt children map.
///
/// Equivalent to [`descendant_epic_ids`] but skips rebuilding the map.
/// Use when calling for multiple roots over the same epic list.
pub fn descendant_epic_ids_with_map(
    root: EpicId,
    children: &std::collections::HashMap<EpicId, Vec<EpicId>>,
) -> HashSet<EpicId> {
    let mut out = HashSet::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if out.insert(id) {
            if let Some(kids) = children.get(&id) {
                stack.extend_from_slice(kids);
            }
        }
    }
    out
}

/// Collect all descendant epic IDs of `root`, inclusive of `root` itself.
///
/// Uses a DFS stack over a children map for O(N) traversal. Cycle-safe: each
/// node is visited at most once via the `out` visited set.
///
/// When computing descendants for multiple roots over the same epic list,
/// prefer [`build_children_map`] + [`descendant_epic_ids_with_map`] to avoid
/// rebuilding the adjacency map on every call.
pub fn descendant_epic_ids(root: EpicId, epics: &[Epic]) -> HashSet<EpicId> {
    let children = build_children_map(epics);
    descendant_epic_ids_with_map(root, &children)
}

/// Titles of `epic`'s ancestor chain, root first, ending with `epic` itself.
///
/// Walks `parent_epic_id` upward through `epics`. A visited-set guards against
/// malformed cycles (which the service layer prevents, but the renderer must
/// not hang on). A root epic returns `vec![epic.title]`.
pub fn ancestor_titles<'a>(epic: &'a Epic, epics: &'a [Epic]) -> Vec<&'a str> {
    let by_id: std::collections::HashMap<EpicId, &Epic> = epics.iter().map(|e| (e.id, e)).collect();
    let mut seen: HashSet<EpicId> = HashSet::new();
    let mut chain: Vec<&str> = Vec::new();
    let mut cursor = epic;
    loop {
        if !seen.insert(cursor.id) {
            break; // cycle guard
        }
        chain.push(cursor.title.as_str());
        match cursor.parent_epic_id.and_then(|pid| by_id.get(&pid)) {
            Some(parent) => cursor = parent,
            None => break,
        }
    }
    chain.reverse();
    chain
}

/// Collect all tasks whose `epic_id` is in the subtree rooted at `root`.
///
/// Returns every task directly under `root` or under any of its descendant
/// sub-epics, recursively. Cycle-safe: malformed epic parent cycles terminate.
pub fn descendant_task_ids(root: EpicId, epics: &[Epic], tasks: &[Task]) -> HashSet<TaskId> {
    let epic_ids = descendant_epic_ids(root, epics);
    tasks
        .iter()
        .filter(|t| matches!(t.epic_id, Some(eid) if epic_ids.contains(&eid)))
        .map(|t| t.id)
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::models::{SubStatus, Task, TaskId, TaskStatus};
    use chrono::Utc;

    fn make_epic(
        id: i64,
        status: TaskStatus,
        plan_path: Option<&str>,
        parent: Option<i64>,
    ) -> Epic {
        Epic {
            id: EpicId(id),
            title: format!("Epic {id}"),
            description: String::new(),
            status,
            plan_path: plan_path.map(String::from),
            sort_order: None,
            auto_dispatch: false,
            parent_epic_id: parent.map(EpicId),
            feed_command: None,
            feed_interval_secs: None,
            group_by_repo: false,
            feed_role: FeedRole::None,
            origin: EpicOrigin::Manual,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn make_task(id: i64, status: TaskStatus, sub_status: SubStatus, epic: Option<i64>) -> Task {
        Task {
            id: TaskId(id),
            title: format!("Task {id}"),
            description: String::new(),
            repo_path: "/repo".to_string(),
            status,
            sub_status,
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
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_pre_tool_use_at: None,
            last_notification_at: None,
            wrap_up_mode: None,
            auto_run_plan: false,
            live_subagents: 0,
            stop_pending: false,
        }
    }

    #[test]
    fn feed_role_roundtrips_kebab_case() {
        assert_eq!(
            serde_json::to_string(&FeedRole::MyReviews).unwrap(),
            "\"my-reviews\""
        );
        assert_eq!(
            serde_json::from_str::<FeedRole>("\"reviews-parent\"").unwrap(),
            FeedRole::ReviewsParent
        );
        assert_eq!(FeedRole::default(), FeedRole::None);
    }

    #[test]
    fn epic_substatus_label_all_variants() {
        assert_eq!(EpicSubstatus::Unplanned.label(), "unplanned");
        assert_eq!(EpicSubstatus::Planned.label(), "planned");
        assert_eq!(EpicSubstatus::Active.label(), "active");
        assert_eq!(EpicSubstatus::Blocked(1).label(), "1 blocked");
        assert_eq!(EpicSubstatus::Blocked(5).label(), "5 blocked");
        assert_eq!(EpicSubstatus::InReview.label(), "in review");
        assert_eq!(EpicSubstatus::Done.label(), "done");
    }

    #[test]
    fn epic_substatus_header_label_active_states() {
        assert_eq!(EpicSubstatus::Blocked(2).header_label(), "needs input");
        assert_eq!(EpicSubstatus::Active.header_label(), "active");
        assert_eq!(EpicSubstatus::InReview.header_label(), "awaiting review");
    }

    #[test]
    fn epic_substatus_header_label_terminal_states_are_empty() {
        assert_eq!(EpicSubstatus::Unplanned.header_label(), "");
        assert_eq!(EpicSubstatus::Planned.header_label(), "");
        assert_eq!(EpicSubstatus::Done.header_label(), "");
    }

    #[test]
    fn epic_substatus_column_priority_aligns_with_substatus() {
        assert_eq!(
            EpicSubstatus::Blocked(1).column_priority(),
            SubStatus::NeedsInput.column_priority()
        );
        assert_eq!(
            EpicSubstatus::Active.column_priority(),
            SubStatus::Active.column_priority()
        );
        assert_eq!(
            EpicSubstatus::InReview.column_priority(),
            SubStatus::AwaitingReview.column_priority()
        );
        assert_eq!(
            EpicSubstatus::Unplanned.column_priority(),
            SubStatus::None.column_priority()
        );
        assert_eq!(
            EpicSubstatus::Done.column_priority(),
            SubStatus::None.column_priority()
        );
    }

    #[test]
    fn epic_substatus_archived_yields_done() {
        let epic = make_epic(1, TaskStatus::Archived, None, None);
        assert_eq!(epic_substatus(&epic, &[]), EpicSubstatus::Done);
    }

    #[test]
    fn epic_substatus_blocked_counts_conflict_and_crashed() {
        for sub in [SubStatus::Conflict, SubStatus::Crashed] {
            let epic = make_epic(1, TaskStatus::Running, None, None);
            let t = make_task(1, TaskStatus::Running, sub, None);
            assert_eq!(
                epic_substatus(&epic, &[&t]),
                EpicSubstatus::Blocked(1),
                "{sub:?}"
            );
        }
    }

    #[test]
    fn descendant_epic_ids_includes_root_itself() {
        let epics = vec![make_epic(1, TaskStatus::Backlog, None, None)];
        let ids = descendant_epic_ids(EpicId(1), &epics);
        assert!(ids.contains(&EpicId(1)));
        assert_eq!(ids.len(), 1);
    }

    #[test]
    fn descendant_epic_ids_includes_direct_children() {
        let epics = vec![
            make_epic(1, TaskStatus::Backlog, None, None),
            make_epic(2, TaskStatus::Backlog, None, Some(1)),
            make_epic(3, TaskStatus::Backlog, None, Some(1)),
        ];
        let ids = descendant_epic_ids(EpicId(1), &epics);
        assert!(ids.contains(&EpicId(1)));
        assert!(ids.contains(&EpicId(2)));
        assert!(ids.contains(&EpicId(3)));
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn descendant_epic_ids_is_recursive() {
        // root(1) → child(2) → grandchild(3)
        let epics = vec![
            make_epic(1, TaskStatus::Backlog, None, None),
            make_epic(2, TaskStatus::Backlog, None, Some(1)),
            make_epic(3, TaskStatus::Backlog, None, Some(2)),
        ];
        let ids = descendant_epic_ids(EpicId(1), &epics);
        assert!(ids.contains(&EpicId(1)));
        assert!(ids.contains(&EpicId(2)));
        assert!(ids.contains(&EpicId(3)));
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn descendant_epic_ids_excludes_unrelated_subtree() {
        let epics = vec![
            make_epic(1, TaskStatus::Backlog, None, None),
            make_epic(2, TaskStatus::Backlog, None, None),
            make_epic(3, TaskStatus::Backlog, None, Some(2)),
        ];
        let ids = descendant_epic_ids(EpicId(1), &epics);
        assert!(ids.contains(&EpicId(1)));
        assert!(!ids.contains(&EpicId(2)));
        assert!(!ids.contains(&EpicId(3)));
    }

    #[test]
    fn descendant_epic_ids_is_cycle_safe() {
        // Malformed: epic 1 → parent 2, epic 2 → parent 1
        let epics = vec![
            make_epic(1, TaskStatus::Backlog, None, Some(2)),
            make_epic(2, TaskStatus::Backlog, None, Some(1)),
        ];
        let ids = descendant_epic_ids(EpicId(1), &epics);
        assert!(ids.contains(&EpicId(1)));
        assert!(ids.contains(&EpicId(2)));
    }

    // ---------------------------------------------------------------------------
    // build_children_map / descendant_epic_ids_with_map
    // ---------------------------------------------------------------------------

    #[test]
    fn descendant_epic_ids_with_map_matches_original_for_deep_tree() {
        // root(1) → child(2) → grandchild(3), unrelated(4) → child(5)
        let epics = vec![
            make_epic(1, TaskStatus::Backlog, None, None),
            make_epic(2, TaskStatus::Backlog, None, Some(1)),
            make_epic(3, TaskStatus::Backlog, None, Some(2)),
            make_epic(4, TaskStatus::Backlog, None, None),
            make_epic(5, TaskStatus::Backlog, None, Some(4)),
        ];
        let children = build_children_map(&epics);

        for root in [EpicId(1), EpicId(2), EpicId(3), EpicId(4), EpicId(5)] {
            let original = descendant_epic_ids(root, &epics);
            let with_map = descendant_epic_ids_with_map(root, &children);
            assert_eq!(
                original, with_map,
                "descendant_epic_ids_with_map must match original for root {root:?}"
            );
        }
    }

    #[test]
    fn build_children_map_returns_empty_for_flat_epics() {
        let epics = vec![
            make_epic(1, TaskStatus::Backlog, None, None),
            make_epic(2, TaskStatus::Backlog, None, None),
        ];
        let children = build_children_map(&epics);
        assert!(
            children.is_empty(),
            "no parent-child relationships → empty map"
        );
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
            plan_path: None,
            epic_id: None,
            sub_status: SubStatus::None,
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
            wrap_up_mode: None,
            auto_run_plan: false,
            live_subagents: 0,
            stop_pending: false,
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
            plan_path: None,
            epic_id: Some(EpicId(5)),
            sub_status: SubStatus::None,
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
            wrap_up_mode: None,
            auto_run_plan: false,
            live_subagents: 0,
            stop_pending: false,
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
            status: TaskStatus::Backlog,
            plan_path: None,
            sort_order: None,
            auto_dispatch: true,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
            group_by_repo: false,
            feed_role: FeedRole::None,
            origin: EpicOrigin::Manual,
            created_at: now,
            updated_at: now,
        };
        assert_eq!(epic.id, EpicId(1));
        assert_eq!(epic.status, TaskStatus::Backlog);
    }

    // --- Epic.status direct access ---
    // epic_status() was a wrapper that once derived status from subtasks.
    // It was deleted; callers should access epic.status directly.

    #[test]
    fn epic_has_status_field_directly_accessible() {
        // Regression guard: epic.status is public and accessible directly.
        // Previously callers used epic_status(&epic) — that wrapper no longer exists.
        for status in [
            TaskStatus::Done,
            TaskStatus::Backlog,
            TaskStatus::Running,
            TaskStatus::Review,
        ] {
            let epic = make_epic(1, status, None, None);
            assert_eq!(epic.status, status);
        }
    }

    // --- EpicSubstatus / epic_substatus ---

    fn test_epic() -> Epic {
        Epic {
            id: EpicId(1),
            title: "Test".to_string(),
            description: "".to_string(),
            status: TaskStatus::Backlog,
            plan_path: None,
            sort_order: None,
            auto_dispatch: true,
            parent_epic_id: None,
            feed_command: None,
            feed_interval_secs: None,
            group_by_repo: false,
            feed_role: FeedRole::None,
            origin: EpicOrigin::Manual,
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
            plan_path: None,
            epic_id: None,
            url: None,
            tag: None,
            sort_order: None,
            base_branch: "main".into(),
            external_id: None,
            labels: Vec::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_pre_tool_use_at: None,
            last_notification_at: None,
            wrap_up_mode: None,
            auto_run_plan: false,
            live_subagents: 0,
            stop_pending: false,
        }
    }

    #[test]
    fn epic_substatus_unplanned() {
        let epic = Epic {
            plan_path: None,
            status: TaskStatus::Backlog,
            ..test_epic()
        };
        assert_eq!(epic_substatus(&epic, &[]), EpicSubstatus::Unplanned);
    }

    #[test]
    fn epic_substatus_planned() {
        let epic = Epic {
            plan_path: Some("plan.md".into()),
            status: TaskStatus::Backlog,
            ..test_epic()
        };
        assert_eq!(epic_substatus(&epic, &[]), EpicSubstatus::Planned);
    }

    #[test]
    fn epic_substatus_active_with_backlog() {
        let epic = Epic {
            status: TaskStatus::Running,
            ..test_epic()
        };
        let t1 = Task {
            status: TaskStatus::Running,
            sub_status: SubStatus::Active,
            ..test_task()
        };
        let t2 = Task {
            status: TaskStatus::Backlog,
            ..test_task()
        };
        assert_eq!(epic_substatus(&epic, &[&t1, &t2]), EpicSubstatus::Active);
    }

    #[test]
    fn epic_substatus_active_all_running() {
        let epic = Epic {
            status: TaskStatus::Running,
            ..test_epic()
        };
        let t1 = Task {
            status: TaskStatus::Running,
            sub_status: SubStatus::Active,
            ..test_task()
        };
        let t2 = Task {
            status: TaskStatus::Done,
            sub_status: SubStatus::None,
            ..test_task()
        };
        assert_eq!(epic_substatus(&epic, &[&t1, &t2]), EpicSubstatus::Active);
    }

    #[test]
    fn epic_substatus_blocked_stale() {
        let epic = Epic {
            status: TaskStatus::Running,
            ..test_epic()
        };
        let t1 = Task {
            status: TaskStatus::Running,
            sub_status: SubStatus::Stale,
            ..test_task()
        };
        let t2 = Task {
            status: TaskStatus::Backlog,
            ..test_task()
        };
        assert_eq!(
            epic_substatus(&epic, &[&t1, &t2]),
            EpicSubstatus::Blocked(1)
        );
    }

    #[test]
    fn epic_substatus_blocked_needs_input() {
        let epic = Epic {
            status: TaskStatus::Running,
            ..test_epic()
        };
        let t1 = Task {
            status: TaskStatus::Running,
            sub_status: SubStatus::NeedsInput,
            ..test_task()
        };
        let t2 = Task {
            status: TaskStatus::Running,
            sub_status: SubStatus::Active,
            ..test_task()
        };
        assert_eq!(
            epic_substatus(&epic, &[&t1, &t2]),
            EpicSubstatus::Blocked(1)
        );
    }

    #[test]
    fn epic_substatus_blocked_count() {
        let epic = Epic {
            status: TaskStatus::Running,
            ..test_epic()
        };
        let t1 = Task {
            status: TaskStatus::Running,
            sub_status: SubStatus::NeedsInput,
            ..test_task()
        };
        let t2 = Task {
            status: TaskStatus::Running,
            sub_status: SubStatus::Stale,
            ..test_task()
        };
        let t3 = Task {
            status: TaskStatus::Running,
            sub_status: SubStatus::Active,
            ..test_task()
        };
        assert_eq!(
            epic_substatus(&epic, &[&t1, &t2, &t3]),
            EpicSubstatus::Blocked(2)
        );
    }

    #[test]
    fn epic_substatus_in_review() {
        let epic = Epic {
            status: TaskStatus::Review,
            ..test_epic()
        };
        assert_eq!(epic_substatus(&epic, &[]), EpicSubstatus::InReview);
    }

    #[test]
    fn epic_substatus_done() {
        let epic = Epic {
            status: TaskStatus::Done,
            ..test_epic()
        };
        assert_eq!(epic_substatus(&epic, &[]), EpicSubstatus::Done);
    }

    // --- descendant_task_ids ---
    // These reuse the module's `make_epic`/`make_task` builders: a backlog epic
    // with the given parent, and a backlog task under the given epic.

    fn epic_with(id: i64, parent: Option<i64>) -> Epic {
        make_epic(id, TaskStatus::Backlog, None, parent)
    }

    fn task_under(id: i64, epic: Option<i64>) -> Task {
        make_task(id, TaskStatus::Backlog, SubStatus::None, epic)
    }

    #[test]
    fn descendant_task_ids_includes_direct_children() {
        let epics = vec![epic_with(1, None)];
        let tasks = vec![task_under(10, Some(1)), task_under(11, None)];
        let ids = descendant_task_ids(EpicId(1), &epics, &tasks);
        assert!(ids.contains(&TaskId(10)));
        assert!(!ids.contains(&TaskId(11)));
    }

    #[test]
    fn descendant_task_ids_is_recursive() {
        // root(1) -> mid(2) -> leaf(3)
        let epics = vec![
            epic_with(1, None),
            epic_with(2, Some(1)),
            epic_with(3, Some(2)),
        ];
        let tasks = vec![
            task_under(10, Some(1)),
            task_under(20, Some(2)),
            task_under(30, Some(3)),
        ];
        let ids = descendant_task_ids(EpicId(1), &epics, &tasks);
        assert!(ids.contains(&TaskId(10)));
        assert!(ids.contains(&TaskId(20)));
        assert!(ids.contains(&TaskId(30)));
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn descendant_task_ids_excludes_sibling_subtree() {
        // root_a(1) with child 10; root_b(2) with child 20
        let epics = vec![epic_with(1, None), epic_with(2, None)];
        let tasks = vec![task_under(10, Some(1)), task_under(20, Some(2))];
        let ids = descendant_task_ids(EpicId(1), &epics, &tasks);
        assert!(ids.contains(&TaskId(10)));
        assert!(!ids.contains(&TaskId(20)));
    }

    #[test]
    fn descendant_task_ids_is_cycle_safe() {
        // Malformed: epic 1 points to epic 2, epic 2 points back to epic 1.
        let epics = vec![epic_with(1, Some(2)), epic_with(2, Some(1))];
        let tasks = vec![task_under(10, Some(1)), task_under(20, Some(2))];
        // Must terminate. From root=1, descendants include {1, 2}, so both tasks.
        let ids = descendant_task_ids(EpicId(1), &epics, &tasks);
        assert!(ids.contains(&TaskId(10)));
        assert!(ids.contains(&TaskId(20)));
    }

    #[test]
    fn ancestor_titles_root_returns_self_only() {
        let mut root = epic_with(1, None);
        root.title = "Root".into();
        let epics = vec![root.clone()];
        assert_eq!(ancestor_titles(&root, &epics), vec!["Root"]);
    }

    #[test]
    fn ancestor_titles_orders_root_to_self() {
        // root(1) -> mid(2) -> leaf(3)
        let mut root = epic_with(1, None);
        root.title = "Root".into();
        let mut mid = epic_with(2, Some(1));
        mid.title = "Mid".into();
        let mut leaf = epic_with(3, Some(2));
        leaf.title = "Leaf".into();
        let epics = vec![root, mid, leaf.clone()];
        assert_eq!(ancestor_titles(&leaf, &epics), vec!["Root", "Mid", "Leaf"]);
    }

    #[test]
    fn ancestor_titles_is_cycle_safe() {
        // 1 -> 2 -> 1 (malformed cycle); must terminate.
        let mut a = epic_with(1, Some(2));
        a.title = "A".into();
        let mut b = epic_with(2, Some(1));
        b.title = "B".into();
        let epics = vec![a.clone(), b];
        let out = ancestor_titles(&a, &epics);
        // Terminates and includes A's own title as the last segment.
        assert_eq!(out.last(), Some(&"A"));
        assert!(out.len() <= 2);
    }

    #[test]
    fn epic_origin_roundtrips_kebab_case() {
        assert_eq!(EpicOrigin::Manual.as_str(), "manual");
        assert_eq!(EpicOrigin::RepoGroup.as_str(), "repo-group");
        assert_eq!(EpicOrigin::parse("manual"), Some(EpicOrigin::Manual));
        assert_eq!(EpicOrigin::parse("repo-group"), Some(EpicOrigin::RepoGroup));
        assert_eq!(EpicOrigin::parse("bogus"), None);
        assert_eq!(EpicOrigin::default(), EpicOrigin::Manual);
    }
}
