use super::*;

mod exec_open_main_session {
    use super::*;

    #[tokio::test]
    async fn exec_open_jumps_when_window_alive() {
        let db = test_db().await;
        let (tx, _rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"dispatch-main\n"), // has_window → true
            MockProcessRunner::ok(),                               // select-window
        ]));
        let rt = make_runtime(db.clone(), tx, mock.clone()).await;
        let mut app = empty_app();

        rt.exec_open_main_session(&mut app).await;

        let calls = mock.recorded_calls();
        // Jumped to the live window — never created one, never opened the picker.
        assert!(!calls
            .iter()
            .any(|(_, args)| args.contains(&"new-window".to_string())));
        assert_ne!(app.mode(), &crate::tui::InputMode::MainSessionDir);
        assert!(app.error_popup().is_none());
    }

    #[tokio::test]
    async fn exec_open_enters_picker_when_no_window() {
        let db = test_db().await;
        let (tx, _rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // has_window → false (empty list)
        ]));
        let rt = make_runtime(db.clone(), tx, mock.clone()).await;
        let mut app = empty_app();
        // A previously-configured dir does not stop the picker from re-prompting.
        app.set_main_session_dir(Some("/home/user".to_string()));

        rt.exec_open_main_session(&mut app).await;

        // No live window — opened the picker to (re)select the directory.
        assert_eq!(app.mode(), &crate::tui::InputMode::MainSessionDir);
        let calls = mock.recorded_calls();
        assert!(!calls
            .iter()
            .any(|(_, args)| args.contains(&"new-window".to_string())));
        assert!(app.error_popup().is_none());
    }
}

/// MainSessionIndicator poll
mod exec_check_main_session_liveness {
    use super::*;

    // @guarantee LivenessFromLiveTmuxCheck: the poll derives liveness from a live
    // tmux has-window check and reports true when the window is present.
    #[tokio::test]
    async fn exec_check_liveness_emits_alive_when_window_present() {
        let db = test_db().await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"dispatch-main\n"), // has_window → true
        ]));
        let rt = make_runtime(db.clone(), tx, mock).await;

        rt.exec_check_main_session_liveness().await.unwrap();

        let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(
                &msg,
                Message::MainSession(crate::tui::messages::MainSessionMessage::LivenessChanged(
                    true
                ))
            ),
            "expected LivenessChanged(true), got: {msg:?}"
        );
    }

    // @guarantee LivenessFromLiveTmuxCheck: reports false when the window is absent.
    #[tokio::test]
    async fn exec_check_liveness_emits_not_alive_when_window_absent() {
        let db = test_db().await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // has_window → false (empty list)
        ]));
        let rt = make_runtime(db.clone(), tx, mock).await;

        rt.exec_check_main_session_liveness().await.unwrap();

        let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(
                &msg,
                Message::MainSession(crate::tui::messages::MainSessionMessage::LivenessChanged(
                    false
                ))
            ),
            "expected LivenessChanged(false), got: {msg:?}"
        );
    }
}

mod exec_create_main_session {
    use super::*;

    #[tokio::test]
    async fn exec_create_makes_window_and_jumps_without_persisting_window() {
        let db = test_db().await;
        let (tx, _rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // new-window
            MockProcessRunner::ok(), // send-keys -l
            MockProcessRunner::ok(), // send-keys Enter
            MockProcessRunner::ok(), // select-window
        ]));
        let rt = make_runtime(db.clone(), tx, mock.clone()).await;
        let mut app = empty_app();
        app.set_main_session_dir(Some("/home/user".to_string()));

        rt.exec_create_main_session(&mut app).await;

        let calls = mock.recorded_calls();
        assert!(calls
            .iter()
            .any(|(_, args)| args.contains(&"new-window".to_string())));
        assert!(app.error_popup().is_none());
        // The window identity is never persisted.
        let stored = db.get_setting_string("main_session.window").await.unwrap();
        assert!(stored.as_deref().unwrap_or("").is_empty());
    }

    #[tokio::test]
    async fn exec_create_with_no_dir_errors() {
        let (rt, mut app) = test_runtime().await;
        rt.exec_create_main_session(&mut app).await;
        assert!(app.error_popup().is_some());
    }
}

mod load_main_session {
    use super::*;

    #[tokio::test]
    async fn load_main_session_sets_dir_from_db() {
        let db = Database::open_in_memory().await.unwrap();
        db.set_setting_string("main_session.dir", "/home/user/code")
            .await
            .unwrap();
        let mut app = empty_app();

        load_main_session(&db, &mut app).await;

        assert_eq!(app.main_session_dir(), Some("/home/user/code"));
    }

    #[tokio::test]
    async fn load_main_session_ignores_empty_dir() {
        let db = Database::open_in_memory().await.unwrap();
        db.set_setting_string("main_session.dir", "").await.unwrap();
        let mut app = empty_app();

        load_main_session(&db, &mut app).await;

        assert_eq!(app.main_session_dir(), None);
    }

    #[tokio::test]
    async fn build_learning_injections_partitions_and_records_retrievals() {
        use crate::models::{LearningKind, LearningScope, RetrievalSource};
        use crate::service::embeddings::{serialize_embedding, EmbeddingService};

        let (rt, _app) = test_runtime().await;
        // Seed a task in the default project.
        let task = create_task_returning(
            &**rt.db_write(),
            "title",
            "desc",
            "/repo/a",
            None,
            models::TaskStatus::Backlog,
        )
        .await
        .unwrap();

        // RAG pipeline requires stored embeddings. Seed fake BLOB bytes so both
        // learnings survive the `embedding IS NULL` filter.
        let fake_emb = serialize_embedding(&[0.1f32; 384]);

        // Seed two approved learnings: one repo-scoped non-procedural, one
        // user-scoped procedural. Both should land in the dispatch list for
        // a task in /repo/a.
        let proc_id = rt
            .database
            .create_learning(CreateLearningRow {
                kind: LearningKind::Procedural,
                summary: "Always run tests before committing.",
                detail: None,
                scope: LearningScope::User,
                scope_ref: None,
                tags: &[],
                source_task_id: None,
                embedding: Some(&fake_emb),
            })
            .await
            .unwrap();
        let repo_id = rt
            .database
            .create_learning(CreateLearningRow {
                kind: LearningKind::Convention,
                summary: "Use Arc for shared state.",
                detail: None,
                scope: LearningScope::Repo,
                scope_ref: Some("/repo/a"),
                tags: &[],
                source_task_id: None,
                embedding: Some(&fake_emb),
            })
            .await
            .unwrap();

        let emb_svc = EmbeddingService::new_test();
        let injected =
            crate::dispatch::build_and_record_injections(&*rt.database, &task, &emb_svc).await;
        assert_eq!(injected.len(), 2);
        let ids: Vec<_> = injected.iter().map(|l| l.id).collect();
        assert!(ids.contains(&proc_id));
        assert!(ids.contains(&repo_id));

        let rows = rt.database.list_retrievals_for_task(task.id).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows
            .iter()
            .all(|r| matches!(r.source, RetrievalSource::PromptInjection)));
    }
}
