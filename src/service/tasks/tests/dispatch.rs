use super::*;

// ---------------------------------------------------------------------------
// The dispatch orchestration seam
// ---------------------------------------------------------------------------
//
// These are the invariants the three hand-written copies of this flow each
// asserted separately — `DispatchClaimExclusive` and the release-on-failure
// unwind in `docs/specs/dispatch.allium`. They are asserted once here, against
// the seam every entry point now goes through.
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod dispatch_seam {
    use super::*;
    use crate::dispatch::mock_sequence::{DispatchScript, Step};
    use crate::models::{DispatchMode, Task};
    use crate::process::MockProcessRunner;
    use crate::service::{DispatchClaim, DispatchOutcome, DispatchRequest};

    /// A `Backlog` task rooted at a fresh temp repo, plus a service wired to
    /// `runner`. The tempdir is returned so the caller keeps it alive.
    async fn fixture(
        db: &Arc<dyn db::TaskStore>,
        runner: Arc<dyn crate::process::ProcessRunner>,
    ) -> (TaskService, Task, tempfile::TempDir) {
        let bootstrap = task_svc(db);
        let id = bootstrap
            .create_task(make_task_params("/placeholder"))
            .await
            .unwrap();
        let mut task = bootstrap.get_task(id).await.unwrap();
        // Pre-creating `.worktrees/<id>-<slug>` is what puts provisioning on
        // the reuse branch `DispatchScript::dispatch` describes.
        let slug = format!("{}-{}", task.id.0, crate::models::slugify(&task.title));
        let (dir, repo_path, _) = crate::dispatch::tests::make_test_repo_with_worktree(&slug);
        bootstrap
            .update_task(UpdateTaskParams::for_task(id).repo_path(repo_path.clone()))
            .await
            .unwrap();
        task.repo_path = repo_path;
        (task_svc_with_runner(db, runner), task, dir)
    }

    fn request(task: Task, mode: DispatchMode, claim: DispatchClaim) -> DispatchRequest {
        DispatchRequest {
            task,
            mode,
            emb_svc: crate::service::embeddings::EmbeddingService::new_test(),
            epic_ctx: None,
            claim,
        }
    }

    /// The happy path: the seam claims, provisions, and records where the agent
    /// landed, so the caller never writes `worktree`/`tmux_window` itself.
    #[tokio::test]
    async fn dispatch_claims_provisions_and_records_the_agent_location() {
        let db = test_db().await;
        let runner = DispatchScript::dispatch().shared_runner();
        let (svc, task, _dir) = fixture(&db, runner.clone()).await;
        let id = task.id;

        let outcome = svc
            .dispatch(request(task, DispatchMode::Dispatch, DispatchClaim::Take))
            .await;

        let DispatchOutcome::Launched(result) = outcome else {
            panic!("expected Launched, got {outcome:?}");
        };
        let stored = svc.get_task(id).await.unwrap();
        assert_eq!(stored.status, TaskStatus::Running);
        assert_eq!(
            stored.worktree.as_deref(),
            Some(result.worktree_path.as_str())
        );
        assert_eq!(
            stored.tmux_window.as_deref(),
            Some(result.tmux_window.as_str())
        );
        // The `Dispatch` half of the mode routing: the standard agent is the
        // one that restricts nothing. Its `Research` twin is the next test.
        assert!(
            !runner
                .flattened_calls()
                .join("\n")
                .contains("--permission-mode"),
            "standard dispatch must not restrict permissions: {:?}",
            runner.recorded_calls()
        );
    }

    /// A dispatch that fails after winning the claim owes the release: the task
    /// must be dispatchable again, exactly as it was before the attempt.
    #[tokio::test]
    async fn dispatch_releases_the_claim_when_provisioning_fails() {
        let db = test_db().await;
        let runner = DispatchScript::dispatch()
            .fails_at(Step::NewWindow)
            .shared_runner();
        let (svc, task, _dir) = fixture(&db, runner).await;
        let id = task.id;

        let outcome = svc
            .dispatch(request(task, DispatchMode::Dispatch, DispatchClaim::Take))
            .await;

        assert!(
            matches!(outcome, DispatchOutcome::Failed(_)),
            "expected Failed, got {outcome:?}"
        );
        let stored = svc.get_task(id).await.unwrap();
        assert_eq!(stored.status, TaskStatus::Backlog);
        assert!(stored.worktree.is_none());
        assert!(stored.tmux_window.is_none());
    }

    /// `DispatchClaimExclusive`: a task that is no longer in backlog is reported
    /// as a lost claim and — the part that matters — nothing is provisioned for
    /// it, so the winner's worktree is never cut twice.
    #[tokio::test]
    async fn dispatch_reports_a_lost_claim_and_provisions_nothing() {
        let db = test_db().await;
        let runner = Arc::new(MockProcessRunner::new(vec![]));
        let (svc, task, _dir) = fixture(&db, runner.clone()).await;
        // Something else got there first.
        assert!(svc.claim_backlog_task(task.id).await.unwrap());

        let outcome = svc
            .dispatch(request(task, DispatchMode::Dispatch, DispatchClaim::Take))
            .await;

        assert!(
            matches!(outcome, DispatchOutcome::ClaimLost),
            "expected ClaimLost, got {outcome:?}"
        );
        assert!(
            runner.recorded_calls().is_empty(),
            "a lost claim must provision nothing: {:?}",
            runner.recorded_calls()
        );
    }

    /// The same exclusion under real concurrency: two callers race the seam and
    /// exactly one launches.
    #[tokio::test]
    async fn two_concurrent_dispatches_launch_exactly_one_agent() {
        let db = test_db().await;
        let runner = DispatchScript::dispatch().shared_runner();
        let (svc, task, _dir) = fixture(&db, runner).await;
        let svc = Arc::new(svc);

        let (s1, s2) = (svc.clone(), svc.clone());
        let (t1, t2) = (task.clone(), task);
        let h1 = tokio::spawn(async move {
            s1.dispatch(request(t1, DispatchMode::Dispatch, DispatchClaim::Take))
                .await
        });
        let h2 = tokio::spawn(async move {
            s2.dispatch(request(t2, DispatchMode::Dispatch, DispatchClaim::Take))
                .await
        });
        let (a, b) = (h1.await.unwrap(), h2.await.unwrap());

        let outcomes = [&a, &b];
        assert_eq!(
            outcomes
                .iter()
                .filter(|o| matches!(o, DispatchOutcome::Launched(_)))
                .count(),
            1,
            "exactly one caller may provision: {a:?} / {b:?}"
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|o| matches!(o, DispatchOutcome::ClaimLost))
                .count(),
            1,
            "the other must see a lost claim: {a:?} / {b:?}"
        );
    }

    /// `DispatchClaim::Held` is for the epic chain, whose
    /// `claim_next_backlog_task` both selected and claimed the row: the seam
    /// must not try to claim it a second time (which would lose, since the task
    /// is already Running) and must dispatch it.
    #[tokio::test]
    async fn dispatch_with_a_held_claim_does_not_reclaim() {
        let db = test_db().await;
        let runner = DispatchScript::dispatch().shared_runner();
        let (svc, task, _dir) = fixture(&db, runner).await;
        assert!(svc.claim_backlog_task(task.id).await.unwrap());

        let outcome = svc
            .dispatch(request(task, DispatchMode::Dispatch, DispatchClaim::Held))
            .await;

        assert!(
            matches!(outcome, DispatchOutcome::Launched(_)),
            "expected Launched, got {outcome:?}"
        );
    }

    /// Mode routing, the match that used to be written out twice: `Research`
    /// launches the read-only research agent, the only one that passes
    /// `--permission-mode plan`.
    #[tokio::test]
    async fn research_mode_launches_the_read_only_research_agent() {
        let db = test_db().await;
        let runner = DispatchScript::dispatch().shared_runner();
        let (svc, task, _dir) = fixture(&db, runner.clone()).await;

        let outcome = svc
            .dispatch(request(task, DispatchMode::Research, DispatchClaim::Take))
            .await;

        assert!(
            matches!(outcome, DispatchOutcome::Launched(_)),
            "expected Launched, got {outcome:?}"
        );
        assert!(
            runner
                .flattened_calls()
                .join("\n")
                .contains("--permission-mode plan"),
            "research mode must launch with plan permissions: {:?}",
            runner.recorded_calls()
        );
    }

    /// With `epic_ctx: None` the seam resolves the epic banner itself, through
    /// the service's own handle — the request carries no database to read from.
    ///
    /// What this asserts is the banner reaching the launched prompt via
    /// `TaskService::dispatch`; that a caller *cannot* aim the reads at some
    /// other database is enforced by `DispatchRequest` having no `db` field,
    /// not by anything observable here.
    #[tokio::test]
    async fn dispatch_reads_the_epic_banner_from_the_services_own_handle() {
        let db = test_db().await;
        let runner = DispatchScript::dispatch().shared_runner();
        let (svc, task, _dir) = fixture(&db, runner.clone()).await;
        let epic = make_epic(&epic_svc(&db), "Own-handle epic").await;
        svc.update_task(UpdateTaskParams::for_task(task.id).epic_id(epic.id))
            .await
            .unwrap();
        // Re-read so the row handed to the seam carries the epic link; with
        // `epic_ctx: None` the banner is the prologue's own read.
        let task = svc.get_task(task.id).await.unwrap();

        let outcome = svc
            .dispatch(request(task, DispatchMode::Dispatch, DispatchClaim::Take))
            .await;

        let DispatchOutcome::Launched(result) = outcome else {
            panic!("expected Launched, got {outcome:?}");
        };
        let prompt =
            std::fs::read_to_string(format!("{}/.claude-prompt", result.worktree_path)).unwrap();
        assert!(
            prompt.contains("Own-handle epic"),
            "prompt must carry the epic banner the prologue read for itself: {prompt}"
        );
    }
}
