use super::*;
use crate::models::test_tmux_window;

mod filter_presets {
    use super::*;

    #[tokio::test]
    async fn exec_persist_filter_preset_saves_to_db() {
        let (rt, mut app) = test_runtime().await;
        rt.exec_persist_filter_preset(
            &mut app,
            "my-preset",
            &["/repo1".into(), "/repo2".into()],
            "include",
        )
        .await;
        let presets = rt.database.list_filter_presets().await.unwrap();
        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].0, "my-preset");
        assert_eq!(presets[0].2, "include");
        assert!(app.error_popup().is_none());
    }

    #[tokio::test]
    async fn exec_delete_filter_preset_removes_from_db() {
        let (rt, mut app) = test_runtime().await;
        rt.database
            .save_filter_preset("doomed", &["/repo".into()], "include")
            .await
            .unwrap();
        rt.exec_delete_filter_preset(&mut app, "doomed").await;
        assert!(rt.database.list_filter_presets().await.unwrap().is_empty());
        assert!(app.error_popup().is_none());
    }
}

mod parse_raw_presets {
    use super::*;

    #[tokio::test]
    async fn parse_raw_presets_converts_all_paths() {
        let raw = vec![(
            "backend".to_string(),
            vec!["/a".to_string(), "/b".to_string()],
            "include".to_string(),
        )];
        let result = parse_raw_presets(raw, None);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, "backend");
        assert_eq!(
            result[0].1,
            HashSet::from(["/a".to_string(), "/b".to_string()])
        );
        assert_eq!(result[0].2, RepoFilterMode::Include);
    }

    #[tokio::test]
    async fn parse_raw_presets_filters_against_known_repos() {
        let raw = vec![(
            "backend".to_string(),
            vec!["/a".to_string(), "/b".to_string(), "/gone".to_string()],
            "exclude".to_string(),
        )];
        let known = HashSet::from(["/a".to_string(), "/b".to_string()]);
        let result = parse_raw_presets(raw, Some(&known));
        assert_eq!(
            result[0].1,
            HashSet::from(["/a".to_string(), "/b".to_string()])
        );
        assert_eq!(result[0].2, RepoFilterMode::Exclude);
    }

    #[tokio::test]
    async fn parse_raw_presets_defaults_invalid_mode() {
        let raw = vec![("x".to_string(), vec![], "bogus".to_string())];
        let result = parse_raw_presets(raw, None);
        assert_eq!(result[0].2, RepoFilterMode::Include);
    }

    #[tokio::test]
    async fn parse_raw_presets_empty_input() {
        let result = parse_raw_presets(vec![], None);
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn parse_raw_presets_multiple_presets() {
        let raw = vec![
            (
                "a".to_string(),
                vec!["/x".to_string()],
                "include".to_string(),
            ),
            (
                "b".to_string(),
                vec!["/y".to_string()],
                "exclude".to_string(),
            ),
        ];
        let result = parse_raw_presets(raw, None);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].2, RepoFilterMode::Include);
        assert_eq!(result[1].2, RepoFilterMode::Exclude);
    }
}

mod repo_path {
    use super::*;

    #[tokio::test]
    async fn exec_delete_repo_path_removes_and_refreshes() {
        let (rt, mut app) = test_runtime().await;
        rt.exec_save_repo_path(&mut app, "/repo1".into()).await;
        rt.exec_save_repo_path(&mut app, "/repo2".into()).await;
        assert_eq!(app.repo_paths().len(), 2);

        rt.exec_delete_repo_path(&mut app, "/repo1").await;
        assert_eq!(app.repo_paths().len(), 1);
        assert!(app.repo_paths().contains(&"/repo2".to_string()));
        assert!(app.error_popup().is_none());
    }
}

mod browser_and_tmux_window {
    use super::*;

    #[tokio::test]
    async fn exec_open_in_browser_calls_xdg_open() {
        let db = test_db().await;
        let (tx, _rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // xdg-open
        ]));
        let rt = make_runtime(db, tx, mock.clone()).await;

        rt.exec_open_in_browser("https://github.com/org/repo/pull/1".into())
            .await
            .unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "xdg-open");
        assert!(calls[0]
            .1
            .contains(&"https://github.com/org/repo/pull/1".to_string()));
    }

    #[tokio::test]
    async fn exec_kill_tmux_window_calls_kill() {
        let db = test_db().await;
        let (tx, _rx) = mpsc::unbounded_channel();
        let mock = Arc::new(
            MockProcessRunner::new(vec![
                MockProcessRunner::ok(), // tmux kill-window
            ])
            .with_windows(&["task-1"]),
        );
        let rt = make_runtime(db, tx, mock.clone()).await;

        rt.exec_kill_tmux_window(test_tmux_window("task-1"))
            .await
            .unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert!(calls[0].1.contains(&"kill-window".to_string()));
        // Targeted by resolved pane ID, not by name — see `tmux::window_target`.
        assert!(calls[0].1.contains(&mock.pane_id_of("task-1")));
    }

    /// Clearing subagent entries for a task that no longer exists is moot, not
    /// a fault: the task was deleted while its hook was still in flight. Only
    /// the typed `ServiceError::NotFound` is demoted, so a real DB failure on
    /// the same path still warns.
    #[tokio::test]
    async fn exec_clear_subagents_is_silent_when_the_task_is_gone() {
        let log = crate::test_log::logged_during(|| async {
            let db = test_db().await;
            let (tx, _rx) = mpsc::unbounded_channel();
            let mock = Arc::new(MockProcessRunner::new(vec![]));
            let rt = make_runtime(db, tx, mock).await;

            // Never created, so the service reports NotFound.
            rt.exec_clear_subagents(crate::models::TaskId(4242), crate::models::DrainMode::Drain)
                .await;
        })
        .await;

        assert!(
            !log.contains("failed to clear subagent/shell entries"),
            "a task that no longer exists must not warn, got log: {log}"
        );
    }

    #[tokio::test]
    async fn exec_kill_tmux_window_failure_is_best_effort() {
        let db = test_db().await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![MockProcessRunner::fail(
            "no such window",
        )]));
        let rt = make_runtime(db, tx, mock).await;

        rt.exec_kill_tmux_window(test_tmux_window("gone-window"))
            .await
            .unwrap();

        // Kill-window failure is best-effort — no error message sent
        assert!(rx.try_recv().is_err(), "Expected no message, but got one");
    }
}

mod load_init_helpers {
    use super::*;

    #[tokio::test]
    async fn load_notifications_pref_defaults_to_false_when_not_set() {
        let db = Database::open_in_memory().await.unwrap();
        let mut app = empty_app();
        load_notifications_pref(&db, &mut app).await;
        assert!(!app.notifications_enabled());
    }

    #[tokio::test]
    async fn load_notifications_pref_sets_true_when_enabled() {
        let db = Database::open_in_memory().await.unwrap();
        db.set_setting_bool("notifications_enabled", true)
            .await
            .unwrap();
        let mut app = empty_app();
        load_notifications_pref(&db, &mut app).await;
        assert!(app.notifications_enabled());
    }

    #[tokio::test]
    async fn load_repo_filter_loads_paths_and_mode() {
        let db = Database::open_in_memory().await.unwrap();
        db.set_setting_string(
            "repo_filter",
            &serde_json::to_string(&vec!["/repo/a".to_string(), "/repo/b".to_string()]).unwrap(),
        )
        .await
        .unwrap();
        db.set_setting_string("repo_filter_mode", RepoFilterMode::Exclude.as_str())
            .await
            .unwrap();
        let mut app = empty_app();

        load_repo_filter(&db, &mut app).await;

        assert_eq!(
            app.repo_filter(),
            &std::collections::HashSet::from(["/repo/a".to_string(), "/repo/b".to_string()])
        );
        assert_eq!(app.repo_filter_mode(), RepoFilterMode::Exclude);
    }

    #[tokio::test]
    async fn load_repo_filter_leaves_defaults_when_nothing_saved() {
        let db = Database::open_in_memory().await.unwrap();
        let mut app = empty_app();

        load_repo_filter(&db, &mut app).await;

        assert!(app.repo_filter().is_empty());
        assert_eq!(app.repo_filter_mode(), RepoFilterMode::Include);
    }

    #[tokio::test]
    async fn load_repo_filter_ignores_an_unparseable_saved_mode() {
        let db = Database::open_in_memory().await.unwrap();
        db.set_setting_string("repo_filter_mode", "bogus")
            .await
            .unwrap();
        let mut app = empty_app();

        load_repo_filter(&db, &mut app).await;

        assert_eq!(
            app.repo_filter_mode(),
            RepoFilterMode::Include,
            "an unparseable saved mode must leave the default in place"
        );
    }

    #[tokio::test]
    async fn load_filter_presets_returns_none_on_success() {
        let db = Database::open_in_memory().await.unwrap();
        let mut app = empty_app();
        let result = load_filter_presets(&db, &mut app);
        assert!(result.await.is_none());
    }

    #[tokio::test]
    async fn load_filter_presets_loads_saved_presets() {
        let db = Database::open_in_memory().await.unwrap();
        db.save_filter_preset("backend", &["/repo/a".into()], "include")
            .await
            .unwrap();
        let mut app = empty_app();
        load_filter_presets(&db, &mut app).await;
        assert_eq!(app.filter_presets().len(), 1);
        assert_eq!(app.filter_presets()[0].0, "backend");
    }

    #[tokio::test]
    async fn apply_tmux_focus_warning_returns_none_when_enabled() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"on\n")]);
        let result = apply_tmux_focus_warning(&mock);
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn apply_tmux_focus_warning_returns_status_info_when_disabled() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"off\n")]);
        let result = apply_tmux_focus_warning(&mock);
        assert!(matches!(
            result,
            Some(Message::System(
                crate::tui::messages::SystemMessage::StatusInfo(_)
            ))
        ));
    }
}

/// Finding 1: bootstrap safety net for the dispatch-owned statusline settings file
/// (see src/setup/statusline.rs).
mod ensure_statusline_settings_file {
    use super::*;

    #[test]
    fn ensure_statusline_settings_file_creates_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let claude_dir = dir.path().join("claude");
        let snapshot_path = dir.path().join("data").join("rate-limits.json");

        ensure_statusline_settings_file(&claude_dir, &snapshot_path).unwrap();

        let settings_path = claude_dir.join(crate::setup::statusline::SETTINGS_FILE_NAME);
        assert!(settings_path.exists(), "settings file must be created");
        let content = std::fs::read_to_string(&settings_path).unwrap();
        assert!(content.contains("dispatch statusline"));
    }

    #[test]
    fn ensure_statusline_settings_file_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let claude_dir = dir.path().join("claude");
        let snapshot_path = dir.path().join("data").join("rate-limits.json");

        ensure_statusline_settings_file(&claude_dir, &snapshot_path).unwrap();
        let settings_path = claude_dir.join(crate::setup::statusline::SETTINGS_FILE_NAME);
        let first = std::fs::read_to_string(&settings_path).unwrap();

        // A normal TUI start on an already-configured machine must not rewrite
        // the file (setup's write_settings_file already guarantees this; this
        // asserts bootstrap doesn't bypass that guarantee).
        ensure_statusline_settings_file(&claude_dir, &snapshot_path).unwrap();
        let second = std::fs::read_to_string(&settings_path).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn ensure_statusline_settings_file_errors_when_directory_unwritable() {
        // Point `claude_dir` at a path whose parent is a *file*, not a directory
        // — `create_dir_all` fails deterministically without touching real
        // permission bits (which vary by OS/CI and can be blocked by sandboxing).
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, b"not a directory").unwrap();
        let claude_dir = blocker.join("claude");
        let snapshot_path = dir.path().join("rate-limits.json");

        let result = ensure_statusline_settings_file(&claude_dir, &snapshot_path);

        assert!(
            result.is_err(),
            "must surface an error rather than silently doing nothing"
        );
    }
}

/// The shared dispatch prologue. Four launch sites (dispatch_task and the epic chain
/// in src/mcp/handlers/tasks/dispatch.rs, exec_quick_dispatch and exec_dispatch_agent
/// in src/runtime/tasks.rs) run it; their own end-to-end tests cover the wiring,
/// these pin the prologue itself.
mod prepare_inputs {
    use super::*;

    #[tokio::test]
    async fn prepare_inputs_reads_epic_context_and_injections() {
        use crate::db::CreateLearningRow;
        use crate::models::{LearningKind, LearningScope, RetrievalSource};
        use crate::service::embeddings::{serialize_embedding, EmbeddingService};

        let (rt, _app) = test_runtime().await;
        let db = rt.db_write().clone();
        let epic = db.create_epic("Chained Epic", "desc", None).await.unwrap();
        let task_id = db
            .create_task(CreateTaskRequest {
                title: "title",
                description: "desc",
                repo_path: "/repo/a",
                plan: None,
                status: models::TaskStatus::Backlog,
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
        let task = db.get_task(task_id).await.unwrap().unwrap();
        let learning_id = db
            .create_learning(CreateLearningRow {
                kind: LearningKind::Convention,
                summary: "Use Arc for shared state.",
                detail: None,
                scope: LearningScope::Repo,
                scope_ref: Some("/repo/a"),
                tags: &[],
                source_task_id: None,
                embedding: Some(&serialize_embedding(&[0.1f32; 384])),
            })
            .await
            .unwrap();

        let inputs =
            crate::dispatch::prepare_inputs(&*db, &task, &EmbeddingService::new_test()).await;

        let epic_ctx = inputs.epic_ctx.expect("epic context read from the DB");
        assert_eq!(epic_ctx.epic_id, epic.id);
        assert_eq!(epic_ctx.epic_title, "Chained Epic");
        assert_eq!(
            inputs.injected.iter().map(|l| l.id).collect::<Vec<_>>(),
            vec![learning_id]
        );

        // The prologue's side effect: each injection is recorded as a retrieval.
        let rows = db.list_retrievals_for_task(task.id).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert!(matches!(rows[0].source, RetrievalSource::PromptInjection));
    }

    #[tokio::test]
    async fn prepare_inputs_with_epic_ctx_uses_the_supplied_context() {
        use crate::service::embeddings::EmbeddingService;

        let (rt, _app) = test_runtime().await;
        let db = rt.db_write().clone();
        // Deliberately epic-less: a from_db read would yield None, so seeing the
        // supplied context proves it was not re-read.
        let task = create_task_returning(
            &*db,
            "title",
            "desc",
            "/repo/a",
            None,
            models::TaskStatus::Backlog,
        )
        .await
        .unwrap();
        let supplied = crate::dispatch::EpicContext {
            epic_id: models::EpicId(7),
            epic_title: "Already in hand".to_string(),
        };

        let inputs = crate::dispatch::prepare_inputs_with_epic_ctx(
            &*db,
            &task,
            &EmbeddingService::new_test(),
            Some(supplied),
        )
        .await;

        let epic_ctx = inputs.epic_ctx.expect("the supplied context is returned");
        assert_eq!(epic_ctx.epic_id, models::EpicId(7));
        assert_eq!(epic_ctx.epic_title, "Already in hand");
        assert!(inputs.injected.is_empty());
    }
}

mod backfill_embeddings {
    use super::*;

    #[tokio::test]
    async fn backfill_fills_missing_embeddings() {
        use crate::db::{CreateLearningRow, LearningStore};
        use crate::models::{LearningKind, LearningScope};
        use crate::service::embeddings::EmbeddingService;

        let db = Arc::new(Database::open_in_memory().await.unwrap());

        // Insert two learnings without embeddings.
        let id1 = db
            .create_learning(CreateLearningRow {
                kind: LearningKind::Convention,
                summary: "always use snake_case",
                detail: None,
                scope: LearningScope::User,
                scope_ref: None,
                tags: &[],
                source_task_id: None,
                embedding: None,
            })
            .await
            .unwrap();
        let id2 = db
            .create_learning(CreateLearningRow {
                kind: LearningKind::Pitfall,
                summary: "avoid unwrap in production",
                detail: Some("use ? or expect with a message"),
                scope: LearningScope::User,
                scope_ref: None,
                tags: &["rust".to_string()],
                source_task_id: None,
                embedding: None,
            })
            .await
            .unwrap();

        // Confirm both are missing embeddings before backfill.
        let missing_before = db.list_learnings_missing_embedding().await.unwrap();
        assert_eq!(
            missing_before.len(),
            2,
            "expected 2 learnings missing embeddings"
        );

        // Run the backfill using the test stub service.
        let emb_svc = EmbeddingService::new_noop();
        let db_for_backfill: Arc<dyn crate::db::LearningStore + Send + Sync> = db.clone();
        super::backfill_embeddings(db_for_backfill, emb_svc)
            .await
            .unwrap();

        // After backfill, no learnings should be missing embeddings.
        let missing_after = db.list_learnings_missing_embedding().await.unwrap();
        assert!(
            missing_after.is_empty(),
            "expected 0 learnings missing embeddings after backfill, got {}",
            missing_after.len()
        );

        // Both learnings should now have non-empty embeddings stored.
        let l1 = db.get_learning(id1).await.unwrap().unwrap();
        let l2 = db.get_learning(id2).await.unwrap().unwrap();
        // Verify via list_all_approved_non_task_learnings which returns embeddings
        let all = db.list_all_approved_non_task_learnings().await.unwrap();
        let emb1 = all.iter().find(|(l, _)| l.id == l1.id).map(|(_, e)| e);
        let emb2 = all.iter().find(|(l, _)| l.id == l2.id).map(|(_, e)| e);
        assert!(
            emb1.is_some_and(|e| !e.is_empty()),
            "learning 1 should have embedding"
        );
        assert!(
            emb2.is_some_and(|e| !e.is_empty()),
            "learning 2 should have embedding"
        );
    }

    #[tokio::test]
    async fn backfill_is_noop_when_no_missing_embeddings() {
        use crate::db::{CreateLearningRow, LearningStore};
        use crate::models::{LearningKind, LearningScope};
        use crate::service::embeddings::{serialize_embedding, EmbeddingService};

        let db = Arc::new(Database::open_in_memory().await.unwrap());

        // Insert a learning that already has an embedding.
        let sentinel = serialize_embedding(&vec![0.1f32; 384]);
        db.create_learning(CreateLearningRow {
            kind: LearningKind::Convention,
            summary: "already embedded",
            detail: None,
            scope: LearningScope::User,
            scope_ref: None,
            tags: &[],
            source_task_id: None,
            embedding: Some(&sentinel),
        })
        .await
        .unwrap();

        let missing_before = db.list_learnings_missing_embedding().await.unwrap();
        assert!(
            missing_before.is_empty(),
            "precondition: no missing embeddings"
        );

        // Backfill should succeed without doing any work.
        let emb_svc = EmbeddingService::new_noop();
        let db_for_backfill: Arc<dyn crate::db::LearningStore + Send + Sync> = db.clone();
        super::backfill_embeddings(db_for_backfill, emb_svc)
            .await
            .unwrap();

        let missing_after = db.list_learnings_missing_embedding().await.unwrap();
        assert!(
            missing_after.is_empty(),
            "still no missing embeddings after no-op backfill"
        );
    }
}

/// Local-first repo sync (docs/specs/repo-sync.allium)
mod repo_sync {
    use super::*;

    /// The three responses one fetching refresh consumes: symbolic-ref, fetch,
    /// rev-list.
    fn refresh_responses_fetching(counts: &[u8]) -> Vec<anyhow::Result<std::process::Output>> {
        vec![
            MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/main\n"),
            MockProcessRunner::ok(),
            MockProcessRunner::ok_with_stdout(counts),
        ]
    }

    async fn expect_measurement(
        rx: &mut mpsc::UnboundedReceiver<Message>,
    ) -> crate::repo_sync::RepoSyncMeasurement {
        match recv_msg(rx).await {
            Message::RepoSync(crate::tui::messages::RepoSyncMessage::Measured(m)) => m,
            other => panic!("expected a repo-sync measurement, got {other:?}"),
        }
    }

    // rule-success.RefreshRepoSyncState: the refresh runs off the event loop and
    // reports its measurement back as a message.
    #[tokio::test]
    async fn exec_refresh_repo_sync_reports_the_measurement() {
        let db = test_db().await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(refresh_responses_fetching(
            b"3\t1\n",
        )));
        let rt = make_runtime(db, tx, mock.clone()).await;

        rt.exec_refresh_repo_sync("/repo".to_string(), true)
            .await
            .unwrap();

        let m = expect_measurement(&mut rx).await;
        assert_eq!(m.repo_path, "/repo");
        assert_eq!(m.base_branch, "main");
        assert_eq!(
            m.counts,
            Some(crate::repo_sync::AheadBehind {
                ahead: 3,
                behind: 1
            })
        );
        assert!(mock
            .recorded_calls()
            .iter()
            .any(|(_, a)| a.contains(&"fetch".to_string())));
    }

    // Only the fetching refresh points perform a fetch; every other caller rides
    // refs some other operation already refreshed.
    #[tokio::test]
    async fn exec_refresh_repo_sync_without_fetch_touches_no_network() {
        let db = test_db().await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/main\n"),
            MockProcessRunner::ok_with_stdout(b"0\t2\n"),
        ]));
        let rt = make_runtime(db, tx, mock.clone()).await;

        rt.exec_refresh_repo_sync("/repo".to_string(), false)
            .await
            .unwrap();

        let m = expect_measurement(&mut rx).await;
        assert_eq!(
            m.counts,
            Some(crate::repo_sync::AheadBehind {
                ahead: 0,
                behind: 2
            })
        );
        assert!(
            !mock
                .recorded_calls()
                .iter()
                .any(|(_, a)| a.contains(&"fetch".to_string())),
            "a non-fetching refresh must be a pure local ref read"
        );
    }

    // rule-success.RefreshRepoSyncStateOnStartup + OneRepoSetForDriftMeasurement:
    // one fetching refresh per saved repo path, and no other repository.
    #[tokio::test]
    async fn exec_refresh_all_repo_sync_fetches_once_per_saved_repo_path() {
        let db = test_db().await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut responses = refresh_responses_fetching(b"1\t0\n");
        responses.extend(refresh_responses_fetching(b"0\t1\n"));
        let mock = Arc::new(MockProcessRunner::new(responses));
        let rt = make_runtime(db, tx, mock.clone()).await;

        let paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
        for handle in rt.exec_refresh_all_repo_sync(&paths) {
            handle.await.unwrap();
        }

        let mut seen = vec![
            expect_measurement(&mut rx).await.repo_path,
            expect_measurement(&mut rx).await.repo_path,
        ];
        seen.sort();
        assert_eq!(seen, paths);
        assert_eq!(
            mock.recorded_calls()
                .iter()
                .filter(|(_, a)| a.contains(&"fetch".to_string()))
                .count(),
            2,
            "exactly one fetch per saved repo path"
        );
    }

    #[tokio::test]
    async fn exec_refresh_all_repo_sync_does_nothing_without_saved_paths() {
        let db = test_db().await;
        let (tx, _rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![]));
        let rt = make_runtime(db, tx, mock.clone()).await;

        assert!(rt.exec_refresh_all_repo_sync(&[]).is_empty());
        assert!(mock.recorded_calls().is_empty());
    }

    // rule-success.SyncRepo, reported back through the success channel.
    #[tokio::test]
    async fn exec_sync_repo_reports_the_counts_it_moved() {
        let db = test_db().await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"git@github.com:org/repo.git\n"), // remote
            MockProcessRunner::ok_with_stdout(b"main\n"),                        // branch
            MockProcessRunner::ok_with_stdout(b""),                              // clean
            MockProcessRunner::ok(),                                             // fetch
            MockProcessRunner::ok_with_stdout(b"3\t1\n"),                        // rev-list
            MockProcessRunner::ok(),                                             // merge
            MockProcessRunner::ok_with_stdout(b"4\t0\n"),                        // recount
            MockProcessRunner::ok(),                                             // push
        ]));
        let rt = make_runtime(db, tx, mock).await;

        rt.exec_sync_repo("/repo".to_string(), "main".to_string())
            .await
            .unwrap();

        match recv_msg(&mut rx).await {
            Message::RepoSync(crate::tui::messages::RepoSyncMessage::Succeeded {
                repo_path,
                outcome,
            }) => {
                assert_eq!(repo_path, "/repo");
                assert_eq!(
                    outcome,
                    crate::repo_sync::SyncOutcome::Synced {
                        pulled: 1,
                        pushed: 4
                    }
                );
            }
            other => panic!("expected a sync success, got {other:?}"),
        }
    }

    // rule-success.ReportRepoSyncFailure: the failure channel carries the detail
    // that makes the cause actionable, plus whether retrying is the fix.
    #[tokio::test]
    async fn exec_sync_repo_reports_a_failure_with_its_detail() {
        let db = test_db().await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"git@github.com:org/repo.git\n"),
            MockProcessRunner::ok_with_stdout(b"feature\n"), // not on base branch
        ]));
        let rt = make_runtime(db, tx, mock).await;

        rt.exec_sync_repo("/repo".to_string(), "main".to_string())
            .await
            .unwrap();

        match recv_msg(&mut rx).await {
            Message::RepoSync(crate::tui::messages::RepoSyncMessage::Failed {
                repo_path,
                detail,
                retryable,
            }) => {
                assert_eq!(repo_path, "/repo");
                assert!(
                    detail.contains("feature") && detail.contains("main"),
                    "the branch found and the one expected: {detail}"
                );
                assert!(!retryable, "the operator must checkout main first");
            }
            other => panic!("expected a sync failure, got {other:?}"),
        }
    }

    // rule-success.RefreshRepoSyncStateAfterRebase: a rebase that moved the repo's
    // base branch triggers a non-fetching refresh.
    #[tokio::test]
    async fn apply_loop_event_branch_rebased_refreshes_the_repo() {
        let db = test_db().await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/main\n"),
            MockProcessRunner::ok_with_stdout(b"2\t0\n"),
        ]));
        let rt = make_runtime(db, tx, mock.clone()).await;
        let mut app = App::new(vec![]);

        let cmds = apply_loop_event(
            &mut app,
            LoopEvent::Mcp(mcp::McpEvent::BranchRebased {
                repo_path: "/repo".to_string(),
            }),
            &rt,
        );

        assert!(cmds.is_empty(), "the refresh is spawned, not queued");
        let m = expect_measurement(&mut rx).await;
        assert_eq!(m.repo_path, "/repo");
        assert!(
            !mock
                .recorded_calls()
                .iter()
                .any(|(_, a)| a.contains(&"fetch".to_string())),
            "the rebase already refreshed the refs"
        );
    }

    // rule-success.RefreshRepoSyncStateAfterDispatch: an agent launched off-board
    // (the dispatch_task tool, or epic auto-dispatch chaining) refreshes the
    // repository's drift too, without a fetch — provisioning already fetched
    // origin/<base>.
    #[tokio::test]
    async fn apply_loop_event_agent_launched_refreshes_the_repo() {
        let db = test_db().await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/main\n"),
            MockProcessRunner::ok_with_stdout(b"1\t0\n"),
        ]));
        let rt = make_runtime(db, tx, mock.clone()).await;
        let mut app = App::new(vec![]);

        let cmds = apply_loop_event(
            &mut app,
            LoopEvent::Mcp(mcp::McpEvent::AgentLaunched {
                repo_path: "/repo".to_string(),
            }),
            &rt,
        );

        assert!(cmds.is_empty(), "the refresh is spawned, not queued");
        let m = expect_measurement(&mut rx).await;
        assert_eq!(m.repo_path, "/repo");
        assert!(
            !mock
                .recorded_calls()
                .iter()
                .any(|(_, a)| a.contains(&"fetch".to_string())),
            "provisioning already fetched origin/<base>"
        );
    }

    // rule-failure.RefreshRepoSyncStateAfterRebase.1: no repository could be
    // resolved from the rebased branch, so nothing is refreshed.
    #[tokio::test]
    async fn apply_loop_event_branch_rebased_without_a_repo_refreshes_nothing() {
        let db = test_db().await;
        let (tx, _rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![]));
        let rt = make_runtime(db, tx, mock.clone()).await;
        let mut app = App::new(vec![]);

        apply_loop_event(
            &mut app,
            LoopEvent::Mcp(mcp::McpEvent::BranchRebased {
                repo_path: String::new(),
            }),
            &rt,
        );

        assert!(
            mock.recorded_calls().is_empty(),
            "an unresolvable repository must not be measured"
        );
    }

    /// `SurfaceAutoDispatchFailure` (docs/specs/epics.allium): the chain's failure
    /// event reaches the board as a message, so the marker, the status line and the
    /// notification are all decided by the app rather than by the loop.
    #[tokio::test]
    async fn apply_loop_event_auto_dispatch_failed_marks_the_subtask() {
        let (rt, mut app) = test_runtime().await;

        let cmds = apply_loop_event(
            &mut app,
            LoopEvent::Mcp(mcp::McpEvent::AutoDispatchFailed {
                task_id: TaskId(1),
                epic_id: crate::models::EpicId(9),
                reason: "no such repo".to_string(),
            }),
            &rt,
        );

        assert!(
            app.auto_dispatch_failed(TaskId(1)),
            "the failure must reach the board's marker, got commands: {cmds:?}"
        );
        let status = app.status_message().unwrap_or_default();
        assert!(
            status.contains("no such repo"),
            "the reason must reach the status line, got: {status}"
        );
    }
}

mod invalidate_feed_cache {
    use super::*;

    /// A live receiver observes the invalidate signal.
    #[tokio::test]
    async fn notifies_the_feed_runner_of_a_change() {
        let (rt, _app) = test_runtime().await;
        let mut watch_rx = rt
            .feed_invalidate_tx
            .as_ref()
            .expect("make_runtime always wires up a live feed runner")
            .subscribe();

        rt.invalidate_feed_cache();

        tokio::time::timeout(TEST_TIMEOUT, watch_rx.changed())
            .await
            .expect("invalidate_feed_cache must notify the feed runner within the timeout")
            .expect("the sender must still be alive");
    }

    /// Best-effort: no live receiver (e.g. the feed runner was never
    /// started) must not panic.
    #[tokio::test]
    async fn is_a_noop_without_a_receiver() {
        let (mut rt, _app) = test_runtime().await;
        rt.feed_invalidate_tx = None;

        rt.invalidate_feed_cache();
    }
}

mod bootstrap {
    use super::*;

    /// Temp-backed `StartupPaths` plus a database path, the fixture every
    /// test here needs. A bootstrap test must never be handed the operator's
    /// real locations — see docs/specs/dispatch.allium: StatusLineDecorator,
    /// `SettingsLocationIsAnExplicitStartupInput` — so this is the only shape
    /// a new one should use.
    fn fixture() -> (tempfile::TempDir, std::path::PathBuf, StartupPaths) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("bootstrap.db");
        let paths = StartupPaths {
            claude_dir: dir.path().join("claude"),
            claude_json_path: dir.path().join(".claude.json"),
        };
        (dir, db_path, paths)
    }

    /// The happy path: opens a real (temp-file-backed) database, spawns the
    /// MCP server and feed runner in the background, and hydrates the
    /// returned `App`/`TuiRuntime` from persisted settings. Binds port 0 so
    /// the OS picks a free ephemeral port — this pins the startup wiring,
    /// not the MCP server's own behaviour.
    #[tokio::test]
    async fn wires_up_a_working_app_and_runtime() {
        let (_dir, db_path, paths) = fixture();

        let bootstrap = TuiRuntime::bootstrap(&db_path, 0, &paths)
            .await
            .expect("bootstrap must succeed against a fresh, writable db path");

        assert!(
            bootstrap.app.tasks().is_empty(),
            "a fresh database has no tasks to hydrate"
        );
        assert!(
            bootstrap
                .runtime
                .database
                .list_all()
                .await
                .unwrap()
                .is_empty(),
            "the returned runtime must be backed by the same freshly-opened database"
        );
        assert!(
            bootstrap.runtime.feed_runner.is_some(),
            "bootstrap must wire up a feed runner for the runtime to own"
        );
    }

    /// The budget snapshot location is account-global and fixed per machine, so
    /// booting against a throwaway database must not move it into that
    /// database's directory (docs/specs/dispatch.allium:
    /// SnapshotLocationIsFixedNotDerivedFromTheOpenDatabase). Deriving it from
    /// the open database is what let a single `cargo test` run silently
    /// repoint every later Claude session at a temp directory.
    #[tokio::test]
    async fn budget_snapshot_path_ignores_the_open_database() {
        let (dir, db_path, paths) = fixture();

        let bootstrap = TuiRuntime::bootstrap(&db_path, 0, &paths)
            .await
            .expect("bootstrap must succeed against a fresh, writable db path");

        assert!(
            !bootstrap
                .runtime
                .budget_snapshot_path
                .starts_with(dir.path()),
            "the snapshot location must not follow the throwaway database"
        );
    }

    /// docs/specs/dispatch.allium: StatusLineDecorator,
    /// `SettingsLocationIsAnExplicitStartupInput`. Startup is *handed* the
    /// operator's locations; it resolves none of them itself. Both halves of
    /// the statusline write are covered here:
    ///
    /// - the file lands under the `claude_dir` it was given, and
    /// - the chain is discovered from that same directory's `settings.json`,
    ///   so reads are scoped to it too, not just writes.
    ///
    /// `HOME` is deliberately not repointed to prove the same thing:
    /// `std::env::set_var` is `unsafe` and races every other test in the
    /// process (see the note in src/editor.rs).
    #[tokio::test]
    async fn statusline_settings_file_is_scoped_to_the_supplied_claude_dir() {
        let (_dir, db_path, paths) = fixture();
        std::fs::create_dir_all(&paths.claude_dir).unwrap();
        std::fs::write(
            paths.claude_dir.join("settings.json"),
            r#"{"statusLine":{"command":"my-own-statusline"}}"#,
        )
        .unwrap();

        TuiRuntime::bootstrap(&db_path, 0, &paths)
            .await
            .expect("bootstrap must succeed against a fresh, writable db path");

        let written =
            std::fs::read_to_string(crate::setup::statusline::settings_path(&paths.claude_dir))
                .expect("the settings file must land in the directory startup was handed");
        assert!(
            written.contains("my-own-statusline"),
            "bootstrap must discover the chain from the directory it was \
             handed, not from one it resolved itself: {written}"
        );
    }

    /// The trust store is the other operator-owned file bootstrap wires up.
    /// It comes from the same `StartupPaths`, so a run that is not the
    /// operator's session cannot reach `$HOME/.claude.json` either.
    #[tokio::test]
    async fn trust_store_path_comes_from_the_supplied_paths() {
        let (_dir, db_path, paths) = fixture();

        let bootstrap = TuiRuntime::bootstrap(&db_path, 0, &paths)
            .await
            .expect("bootstrap must succeed against a fresh, writable db path");

        assert_eq!(
            bootstrap.runtime.claude_json_path, paths.claude_json_path,
            "the trust store path must be the one startup was handed"
        );
    }
}

/// `StartupPaths::resolve` itself — deliberately outside `mod bootstrap`, whose
/// fixture rule is that a bootstrap test is never handed the operator's real
/// locations. These resolve and compare only: nothing boots, and nothing is
/// written anywhere.
mod startup_paths {
    use super::*;

    /// docs/specs/dispatch.allium: StatusLineDecorator,
    /// `SpawnSitesAndStartupNameTheSameConfigurationDirectory`. The writer-side
    /// link for the settings file: `src/dispatch/tests.rs` binds the spawn
    /// constant's literal to the configuration-directory layout, and this binds
    /// startup to the same lookup. Neither alone rules out a `resolve` that
    /// quietly hands startup somewhere else.
    ///
    /// This mirrors `resolve`'s composition on purpose. It cannot fail for a
    /// change in what those lookups *return* — that is the dispatch-side test's
    /// job — only if `resolve` stops delegating to them.
    ///
    /// `src/main.rs::cmd_tui` needs no test of its own: `StartupPaths`' fields
    /// are private to this module, so `resolve` is the only way a caller
    /// outside it can obtain one. That link is compiler-enforced, not merely
    /// asserted.
    #[test]
    fn resolve_delegates_to_the_shared_configuration_directory_lookup() {
        let paths = StartupPaths::resolve().expect("$HOME must be set");

        assert_eq!(
            paths.claude_dir,
            crate::setup::claude_dir().expect("$HOME must be set"),
            "startup must be handed the same configuration directory setup \
             resolves — that is the directory the spawn constant's literals \
             are checked against"
        );
    }

    /// docs/specs/dispatch.allium: StatusLineDecorator,
    /// `AnUnavailableHomeDirectoryIsAFailureNotAPath`. The trust store's
    /// writer-side link.
    ///
    /// `resolve` returning a `Result` is what carries the absent-home failure
    /// out to `src/main.rs::cmd_tui`, and this pins that the trust store is
    /// one of the locations that failure covers — that `resolve` gets it from
    /// the same lookup as the configuration directory rather than deriving it
    /// a second way, which is how the two came to disagree about an absent
    /// `$HOME` in the first place.
    #[test]
    fn resolve_delegates_to_the_shared_trust_store_lookup() {
        let paths = StartupPaths::resolve().expect("$HOME must be set");

        assert_eq!(
            paths.claude_json_path,
            crate::setup::user_global_config_path().expect("$HOME must be set"),
            "startup must be handed the trust store the one $HOME reading \
             resolves, not a path derived independently of it"
        );
    }
}
