//! `VisualColumn` — the 8 visual columns for the kanban board — and
//! [`DerivedSection`], the Review-column section headers that are derived from a
//! task rather than stored on it.

use super::{SubStatus, Task, TaskStatus};

#[derive(Debug, Clone)]
pub struct VisualColumn {
    pub label: &'static str,
    pub parent_status: TaskStatus,
    pub sub_statuses: &'static [SubStatus],
}

impl VisualColumn {
    pub const COUNT: usize = 8;
    pub const ALL: &'static [VisualColumn] = &[
        VisualColumn {
            label: "Backlog",
            parent_status: TaskStatus::Backlog,
            sub_statuses: &[SubStatus::None],
        },
        VisualColumn {
            label: "Active",
            parent_status: TaskStatus::Running,
            sub_statuses: &[SubStatus::Active],
        },
        VisualColumn {
            label: "Blocked",
            parent_status: TaskStatus::Running,
            sub_statuses: &[SubStatus::NeedsInput],
        },
        VisualColumn {
            label: "Stale",
            parent_status: TaskStatus::Running,
            sub_statuses: &[
                SubStatus::Stale,
                SubStatus::StaleShell,
                SubStatus::Crashed,
                SubStatus::Conflict,
            ],
        },
        VisualColumn {
            label: "PR Created",
            parent_status: TaskStatus::Review,
            sub_statuses: &[
                SubStatus::AwaitingReview,
                SubStatus::Conflict,
                SubStatus::PrClosed,
                SubStatus::PrUnreachable,
            ],
        },
        VisualColumn {
            label: "Revise",
            parent_status: TaskStatus::Review,
            sub_statuses: &[SubStatus::ChangesRequested],
        },
        VisualColumn {
            label: "Approved",
            parent_status: TaskStatus::Review,
            sub_statuses: &[SubStatus::Approved],
        },
        VisualColumn {
            label: "Done",
            parent_status: TaskStatus::Done,
            sub_statuses: &[SubStatus::None],
        },
    ];

    pub fn contains(&self, sub_status: SubStatus) -> bool {
        self.sub_statuses.contains(&sub_status)
    }

    pub fn parent_group_start(status: TaskStatus) -> usize {
        Self::ALL
            .iter()
            .position(|vc| vc.parent_status == status)
            .unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// DerivedSection
// ---------------------------------------------------------------------------

/// A Review-column section header that is *derived* from a task rather than
/// stored as a [`SubStatus`].
///
/// Deriving is what makes these self-correcting: a parked task leaves the parked
/// section the moment its tmux window reappears, with nothing to write back and
/// no migration to run. See "Derived review sections" in
/// `docs/specs/core.allium` for the full section order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DerivedSection {
    /// Review, provisioned, agent session ended, and no PR to review or merge.
    /// Sub-status is deliberately not part of the condition: with no PR there is
    /// nothing for any review decision to be about.
    Parked,
    /// Changes *the user* requested on someone else's PR. The `pr-review` and
    /// `dependabot` tags mean the task is reviewing a PR rather than authoring
    /// one, so the ball is with the other author.
    ChangesRequestedByMe,
    /// An approval *the user* gave on someone else's PR — same tags, same
    /// direction. Unlike an approval on the user's own PR, there is no merge
    /// left to perform.
    ApprovedByMe,
}

impl DerivedSection {
    /// The section a task belongs to, or `None` when its own [`SubStatus`]
    /// already names the section.
    ///
    /// `Parked` is tested first: it means there is no PR at all, which dominates
    /// either review decision that was somehow recorded without one.
    pub fn for_task(task: &Task) -> Option<Self> {
        let has_pr = task.url.as_ref().is_some_and(|u| u.is_pr());
        if task.status == TaskStatus::Review && task.is_detached() && !has_pr {
            return Some(Self::Parked);
        }
        if !task.tag.is_some_and(|t| t.is_review()) {
            return None;
        }
        // The two sub-statuses that record a review *decision*. On a task that
        // reviews someone else's PR, that decision was the user's own.
        match task.sub_status {
            SubStatus::ChangesRequested => Some(Self::ChangesRequestedByMe),
            SubStatus::Approved => Some(Self::ApprovedByMe),
            _ => None,
        }
    }

    /// Sort priority for column grouping (lower = more urgent = top of column),
    /// on the same scale as [`SubStatus::column_priority`].
    pub const fn column_priority(self) -> u8 {
        self.properties().priority
    }

    /// Label for the section header line within a column.
    pub const fn header_label(self) -> &'static str {
        self.properties().header_label
    }

    /// Per-variant display properties in a single match, mirroring
    /// `SubStatus::properties` — a new variant touches this one table rather
    /// than two parallel ones that can drift.
    const fn properties(self) -> DerivedSectionProperties {
        match self {
            Self::Parked => DerivedSectionProperties {
                priority: PRIORITY_PARKED,
                header_label: "parked",
            },
            Self::ChangesRequestedByMe => DerivedSectionProperties {
                priority: PRIORITY_CHANGES_REQUESTED_BY_ME,
                header_label: "changes requested by me",
            },
            Self::ApprovedByMe => DerivedSectionProperties {
                priority: PRIORITY_APPROVED_BY_ME,
                header_label: "approved by me",
            },
        }
    }
}

/// Per-variant properties returned by [`DerivedSection::properties`].
struct DerivedSectionProperties {
    priority: u8,
    header_label: &'static str,
}

// Sort slots for the derived sections. All three sit under every named
// `SubStatus` slot — none of these sections is waiting on the user — and all are
// derived from the model's lowest slot rather than hardcoded, so inserting a new
// `SubStatus` priority tier can't silently desync them.
const PRIORITY_PARKED: u8 = SubStatus::AwaitingReview.column_priority() + 1;
const PRIORITY_CHANGES_REQUESTED_BY_ME: u8 = PRIORITY_PARKED + 1;
const PRIORITY_APPROVED_BY_ME: u8 = PRIORITY_CHANGES_REQUESTED_BY_ME + 1;

/// Column sort priority for a task: the [`DerivedSection`] slot when one
/// applies, else the task's own [`SubStatus`] slot.
pub fn task_column_priority(task: &Task) -> u8 {
    match DerivedSection::for_task(task) {
        Some(section) => section.column_priority(),
        None => task.sub_status.column_priority(),
    }
}

/// Section-header label for a task: the [`DerivedSection`] label when one
/// applies, else the task's own [`SubStatus`] label.
pub fn task_header_label(task: &Task) -> &'static str {
    match DerivedSection::for_task(task) {
        Some(section) => section.header_label(),
        None => task.sub_status.header_label(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

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
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod derived_section_tests {
    use super::*;
    use crate::models::tasks::model_tests::make_task_with;
    use crate::models::{test_tmux_window, TaskTag, TaskUrl, UrlType};

    /// A Review task, provisioned and live (worktree + tmux window), no url.
    fn review_task(sub_status: SubStatus, tag: Option<TaskTag>) -> Task {
        let mut t = make_task_with(None, tag);
        t.status = TaskStatus::Review;
        t.sub_status = sub_status;
        t.worktree = Some("/repo/.worktrees/1-task".to_string());
        t.tmux_window = Some(test_tmux_window("task-1"));
        t
    }

    /// A detached Review task: worktree present, tmux window gone, no url.
    fn detached_review_task() -> Task {
        let mut t = review_task(SubStatus::AwaitingReview, None);
        t.tmux_window = None;
        t
    }

    fn pr_url() -> Option<TaskUrl> {
        Some(TaskUrl::new(
            "https://github.com/org/repo/pull/10",
            UrlType::Pr,
        ))
    }

    /// A live Review task that has a PR — the common case for every section
    /// except `parked`.
    fn with_pr(sub_status: SubStatus, tag: Option<TaskTag>) -> Task {
        let mut t = review_task(sub_status, tag);
        t.url = pr_url();
        t
    }

    // --- parked ---

    #[test]
    fn detached_review_task_without_a_pr_is_parked() {
        let t = detached_review_task();
        assert_eq!(DerivedSection::for_task(&t), Some(DerivedSection::Parked));
        assert_eq!(task_header_label(&t), "parked");
        assert!(
            task_column_priority(&t) > SubStatus::AwaitingReview.column_priority(),
            "parked should sort below awaiting review"
        );
    }

    /// The awaiting-merge removal: once a PR exists, detach state stops
    /// mattering and only the review decision does.
    #[test]
    fn detached_review_task_with_a_pr_is_plain_awaiting_review() {
        let mut t = detached_review_task();
        t.url = pr_url();
        assert_eq!(DerivedSection::for_task(&t), None);
        assert_eq!(task_header_label(&t), "awaiting review");
        assert_eq!(
            task_column_priority(&t),
            SubStatus::AwaitingReview.column_priority()
        );
    }

    /// Only a *pr*-typed url takes a task out of parked — an issue or
    /// security-alert link is not something that can be reviewed or merged.
    #[test]
    fn detached_review_task_with_a_non_pr_url_is_still_parked() {
        for ty in [UrlType::Issue, UrlType::SecurityAlert, UrlType::Other] {
            let mut t = detached_review_task();
            t.url = Some(TaskUrl::new("https://example.com/1", ty));
            assert_eq!(task_header_label(&t), "parked", "url_type {ty:?}");
        }
    }

    #[test]
    fn live_review_task_without_a_pr_is_not_parked() {
        let t = review_task(SubStatus::AwaitingReview, None);
        assert_eq!(task_header_label(&t), "awaiting review");
    }

    #[test]
    fn running_detached_task_is_not_parked() {
        let mut t = detached_review_task();
        t.status = TaskStatus::Running;
        t.sub_status = SubStatus::Active;
        assert_eq!(task_header_label(&t), SubStatus::Active.header_label());
    }

    #[test]
    fn unprovisioned_review_task_is_not_parked() {
        let mut t = detached_review_task();
        t.worktree = None;
        assert_eq!(task_header_label(&t), "awaiting review");
    }

    /// Sub-status is deliberately not part of the parked condition: parked
    /// means there is no PR at all, so it dominates every sub-status that could
    /// only be about one — a conflict, or either review decision somehow
    /// recorded without a PR.
    #[test]
    fn parked_wins_over_every_pr_bearing_sub_status() {
        for ss in [
            SubStatus::Conflict,
            SubStatus::ChangesRequested,
            SubStatus::Approved,
        ] {
            let mut t = detached_review_task();
            t.sub_status = ss;
            t.tag = Some(TaskTag::PrReview);
            assert_eq!(
                DerivedSection::for_task(&t),
                Some(DerivedSection::Parked),
                "{ss:?}"
            );
            assert_eq!(task_header_label(&t), "parked", "{ss:?}");
        }
    }

    /// A closed-without-merge PR still *has* a url, so it keeps its own
    /// "pr closed" section rather than falling into parked when the agent
    /// session ends.
    #[test]
    fn detached_pr_closed_task_is_not_parked() {
        let mut t = detached_review_task();
        t.sub_status = SubStatus::PrClosed;
        t.url = pr_url();
        assert_eq!(task_header_label(&t), SubStatus::PrClosed.header_label());
    }

    // --- review decisions the user made themselves ---

    /// The two sub-statuses that record a review decision, and the section each
    /// becomes on a task that reviews someone else's PR. One table so a third
    /// decision is one row, not another pair of cloned tests.
    const BY_ME: [(SubStatus, DerivedSection, &str); 2] = [
        (
            SubStatus::ChangesRequested,
            DerivedSection::ChangesRequestedByMe,
            "changes requested by me",
        ),
        (
            SubStatus::Approved,
            DerivedSection::ApprovedByMe,
            "approved by me",
        ),
    ];

    #[test]
    fn a_review_tagged_decision_is_by_me() {
        for (ss, section, label) in BY_ME {
            for tag in [TaskTag::PrReview, TaskTag::Dependabot] {
                let t = with_pr(ss, Some(tag));
                assert_eq!(
                    DerivedSection::for_task(&t),
                    Some(section),
                    "{ss:?}/{tag:?}"
                );
                assert_eq!(task_header_label(&t), label, "{ss:?}/{tag:?}");
                assert!(
                    task_column_priority(&t) > task_column_priority(&detached_review_task()),
                    "{ss:?}/{tag:?} should sort below parked"
                );
            }
        }
    }

    #[test]
    fn a_non_review_tagged_decision_keeps_the_model_label() {
        for (ss, _, _) in BY_ME {
            for tag in [None, Some(TaskTag::Feature), Some(TaskTag::Bug)] {
                let t = with_pr(ss, tag);
                assert_eq!(DerivedSection::for_task(&t), None, "{ss:?}/{tag:?}");
                assert_eq!(task_header_label(&t), ss.header_label(), "{ss:?}/{tag:?}");
                assert_eq!(
                    task_column_priority(&t),
                    ss.column_priority(),
                    "{ss:?}/{tag:?}"
                );
            }
        }
    }

    /// The override is scoped to the two review *decisions*: a review-tagged
    /// task in any other sub-status keeps the model's own label.
    #[test]
    fn review_tag_does_not_affect_other_sub_statuses() {
        for &ss in SubStatus::ALL {
            if BY_ME.iter().any(|&(decision, _, _)| decision == ss) {
                continue;
            }
            let t = with_pr(ss, Some(TaskTag::PrReview));
            assert_eq!(task_header_label(&t), ss.header_label(), "{ss:?}");
            assert_eq!(task_column_priority(&t), ss.column_priority(), "{ss:?}");
        }
    }

    // --- ordering ---

    /// The full Review-column section order, asserted end to end.
    #[test]
    fn review_section_order_is_urgent_first() {
        let sections = [
            with_pr(SubStatus::Conflict, None),
            with_pr(SubStatus::PrClosed, None),
            with_pr(SubStatus::ChangesRequested, None),
            with_pr(SubStatus::Approved, None),
            with_pr(SubStatus::AwaitingReview, None),
            detached_review_task(),
            with_pr(SubStatus::ChangesRequested, Some(TaskTag::PrReview)),
            with_pr(SubStatus::Approved, Some(TaskTag::PrReview)),
        ];

        for pair in sections.windows(2) {
            let (above, below) = (&pair[0], &pair[1]);
            assert!(
                task_column_priority(above) < task_column_priority(below),
                "{:?} should sort above {:?}",
                task_header_label(above),
                task_header_label(below)
            );
        }
    }
}
