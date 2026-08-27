//! `VisualColumn` — the 8 visual columns for the kanban board.

use super::{SubStatus, TaskStatus};

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

    pub fn parent_group_span(status: TaskStatus) -> usize {
        Self::ALL
            .iter()
            .filter(|vc| vc.parent_status == status)
            .count()
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
    fn visual_column_parent_group_span() {
        assert_eq!(VisualColumn::parent_group_span(TaskStatus::Backlog), 1);
        assert_eq!(VisualColumn::parent_group_span(TaskStatus::Running), 3);
        assert_eq!(VisualColumn::parent_group_span(TaskStatus::Review), 3);
        assert_eq!(VisualColumn::parent_group_span(TaskStatus::Done), 1);
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
