use super::*;
use crate::db::{CreateTaskRequest, EpicPatch};
use proptest::prelude::*;

/// The non-archived task statuses an epic rolls up over, plus `Archived`,
/// which the recalc query filters out (so it must never influence the roll-up).
const RECALC_STATUSES: &[TaskStatus] = &[
    TaskStatus::Backlog,
    TaskStatus::Running,
    TaskStatus::Review,
    TaskStatus::Done,
    TaskStatus::Archived,
];

/// Epic baseline statuses — `recalculate_epic_status` reads the epic's
/// current status as the fallback/regression pivot. Archived epics are
/// excluded (they are not recalculated as live epics here).
const EPIC_BASELINES: &[TaskStatus] = &[
    TaskStatus::Backlog,
    TaskStatus::Running,
    TaskStatus::Review,
    TaskStatus::Done,
];

/// Re-derive the expected rolled-up epic status from the same rule the DB
/// recalc applies: archived subtasks are ignored; an all-`Done` active set
/// rolls the epic to `Done`; a `Done` epic with any active non-`Done` child
/// regresses to `Backlog`; otherwise the baseline status is preserved.
fn expected_rollup(baseline: TaskStatus, task_statuses: &[TaskStatus]) -> TaskStatus {
    let active: Vec<TaskStatus> = task_statuses
        .iter()
        .copied()
        .filter(|s| *s != TaskStatus::Archived)
        .collect();
    if active.is_empty() {
        baseline
    } else if active.iter().all(|s| *s == TaskStatus::Done) {
        TaskStatus::Done
    } else if baseline == TaskStatus::Done {
        TaskStatus::Backlog
    } else {
        baseline
    }
}

/// Mirror of the `FieldUpdate ↔ Option<Option<T>>` mapping documented in
/// docs/conventions.md and applied by `validators::build_task_patch`:
///   `Some(Set(v))` → `Some(Some(v))`
///   `Some(Clear)`  → `Some(None)`
///   `None`         → `None`
fn map_field_update(fu: Option<FieldUpdate>) -> Option<Option<String>> {
    match fu {
        Some(FieldUpdate::Set(v)) => Some(Some(v)),
        Some(FieldUpdate::Clear) => Some(None),
        None => None,
    }
}

fn field_update_strategy() -> impl Strategy<Value = Option<FieldUpdate>> {
    prop_oneof![
        Just(None),
        Just(Some(FieldUpdate::Clear)),
        "[a-zA-Z0-9/]{0,32}".prop_map(|s| Some(FieldUpdate::Set(s))),
    ]
}

proptest! {
    /// `FieldUpdate` round-trips through the canonical mapping cleanly.
    #[test]
    fn field_update_roundtrip(fu in field_update_strategy()) {
        let mapped = map_field_update(fu.clone());
        let back: Option<FieldUpdate> = match mapped {
            None              => None,
            Some(None)        => Some(FieldUpdate::Clear),
            Some(Some(v))     => Some(FieldUpdate::Set(v)),
        };
        prop_assert_eq!(back, fu);
    }

    /// `build_task_patch` applies the mapping to `worktree` and
    /// `tmux_window`. For all input combinations, the resulting `TaskPatch`
    /// must carry the canonical `Option<Option<&str>>` shape.
    #[test]
    fn build_task_patch_maps_field_updates(
        worktree in field_update_strategy(),
        tmux_window in field_update_strategy(),
    ) {
        let mut params = UpdateTaskParams::for_task(TaskId(1));
        if let Some(ref w) = worktree    { params = params.worktree(w.clone()); }
        if let Some(ref t) = tmux_window { params = params.tmux_window(t.clone()); }

        let patch = super::super::validators::build_task_patch(&params, None, None);

        let expect = |fu: &Option<FieldUpdate>| -> Option<Option<String>> {
            fu.as_ref().map(|x| match x {
                FieldUpdate::Set(v) => Some(v.clone()),
                FieldUpdate::Clear  => None,
            })
        };
        prop_assert_eq!(
            patch.worktree.map(|o| o.map(|s| s.to_string())),
            expect(&worktree)
        );
        prop_assert_eq!(
            patch.tmux_window.map(|o| o.map(|s| s.to_string())),
            expect(&tmux_window)
        );
    }

    /// Epic sub-status recalculation over random subtask-status combinations:
    /// for any baseline epic status and any multiset of subtask statuses,
    /// `recalculate_epic_status` must produce the rolled-up status given by
    /// `expected_rollup` (archived subtasks ignored; all-Done → Done;
    /// Done-epic regression → Backlog; otherwise baseline preserved).
    #[test]
    fn epic_recalc_rolls_up_subtask_statuses(
        baseline_idx in 0..EPIC_BASELINES.len(),
        status_idxs in proptest::collection::vec(0..RECALC_STATUSES.len(), 0..6),
    ) {
        let baseline = EPIC_BASELINES[baseline_idx];
        let task_statuses: Vec<TaskStatus> =
            status_idxs.iter().map(|&i| RECALC_STATUSES[i]).collect();

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let actual = rt.block_on(async {
            let db = test_db().await;
            let epic = db.create_epic("E", "", None).await.unwrap();
            // Seed the baseline status the recalc pivots on.
            db.patch_epic(epic.id, &EpicPatch::new().status(baseline))
                .await
                .unwrap();
            for status in &task_statuses {
                db.create_task(CreateTaskRequest {
                    title: "t",
                    description: "",
                    repo_path: "/r",
                    plan: None,
                    status: *status,
                    base_branch: "main",
                    epic_id: Some(epic.id),
                    sort_order: None,
                    tag: None,
                    wrap_up_mode: None,
                    auto_run_plan: false,
                    phoenix: false,
                })
                .await
                .unwrap();
            }
            db.recalculate_epic_status(epic.id).await.unwrap();
            db.get_epic(epic.id).await.unwrap().unwrap().status
        });

        prop_assert_eq!(
            actual,
            expected_rollup(baseline, &task_statuses),
            "baseline={:?} tasks={:?}",
            baseline,
            task_statuses
        );
    }
}
