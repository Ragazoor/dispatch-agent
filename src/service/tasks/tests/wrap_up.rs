use super::*;

// ---------------------------------------------------------------------------
// The wrap-up rebase seam
// ---------------------------------------------------------------------------
//
// Only the two arms the MCP handler cannot reach. `WrapUpRebase`'s `Conflict`
// sub_status maintenance (docs/specs/pr-workflow.allium) already has end-to-end
// coverage through the handler — `wrap_up_rebase_conflict_sets_conflict_substatus`
// and `wrap_up_rebase_clears_conflict_substatus_on_non_conflict_error` in
// `src/mcp/handlers/tests/tasks/dispatch.rs` — and moving that logic behind the
// seam did not change what they assert, so they are not duplicated here.
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod wrap_up_rebase_seam {
    use super::*;
    use crate::process::MockProcessRunner;
    use crate::service::WrapUpRebaseOutcome;

    /// A worktree path that names no branch is rejected before any git command
    /// runs — the request is unanswerable, not a failed rebase.
    #[tokio::test]
    async fn an_underivable_branch_is_reported_without_running_git() {
        let db = test_db().await;
        let runner = Arc::new(MockProcessRunner::new(vec![]));
        let svc = task_svc_with_runner(&db, runner.clone());
        let id = svc.create_task(make_task_params("/repo")).await.unwrap();
        svc.update_task(
            UpdateTaskParams::for_task(id)
                .status(TaskStatus::Running)
                // A root path has no final component, so no branch name.
                .worktree(FieldUpdate::Set("/".to_string())),
        )
        .await
        .unwrap();
        let task = svc.get_task(id).await.unwrap();

        let outcome = svc.wrap_up_rebase(task).await;

        assert!(
            matches!(outcome, WrapUpRebaseOutcome::UnderivableBranch { .. }),
            "expected UnderivableBranch, got {outcome:?}"
        );
        assert!(runner.recorded_calls().is_empty());
    }

    /// Defence in depth: a task with no worktree must be reported, not panic.
    #[tokio::test]
    async fn a_task_without_a_worktree_is_reported_not_panicked() {
        let db = test_db().await;
        let svc = task_svc(&db);
        let id = svc.create_task(make_task_params("/repo")).await.unwrap();
        let task = svc.get_task(id).await.unwrap();

        assert!(matches!(
            svc.wrap_up_rebase(task).await,
            WrapUpRebaseOutcome::MissingWorktree
        ));
    }
}
