use super::*;
use crate::db;
use crate::models::{LearningId, LearningStatus};
#[cfg(test)]
use crate::service::embeddings::EmbeddingService;

impl TuiRuntime {
    pub(super) async fn exec_load_learnings(&self, app: &mut App) {
        let db: Arc<dyn db::TaskReadStore> = self.database.clone();
        let approved = db
            .list_learnings(db::LearningFilter {
                status: Some(LearningStatus::Approved),
                ..Default::default()
            })
            .await;
        match approved {
            Ok(all) => {
                app.update(Message::Learning(LearningMessage::Show(all)));
            }
            Err(e) => {
                app.update(Message::System(
                    crate::tui::messages::SystemMessage::StatusInfo(format!(
                        "Failed to load learnings: {e}"
                    )),
                ));
            }
        }
    }

    pub(super) async fn exec_archive_learning(&self, app: &mut App, id: LearningId) {
        let result = self.learning_svc.archive_learning(id).await;
        Self::handle_action_result(app, id, "archive", result);
    }

    pub(super) async fn exec_reject_learning(&self, app: &mut App, id: LearningId) {
        let result = self.learning_svc.reject_learning(id).await;
        Self::handle_action_result(app, id, "reject", result);
    }

    /// Background stale-learning sweep. Computes the cutoff from
    /// `STALE_LEARNING_THRESHOLD` and archives approved, non-positively-scored
    /// entries untouched past it. See docs/specs/learnings.allium:
    /// ArchiveStaleLearning.
    pub(super) async fn exec_archive_stale_learnings(&self) {
        let cutoff = chrono::Utc::now()
            - chrono::Duration::from_std(crate::tui::STALE_LEARNING_THRESHOLD)
                .unwrap_or_else(|_| chrono::Duration::days(90));
        match self.learning_svc.archive_stale_learnings(cutoff).await {
            Ok(n) if n > 0 => {
                tracing::info!("Archived {n} stale learning(s)");
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("Stale-learning cleanup failed: {e}");
            }
        }
    }

    fn handle_action_result(
        app: &mut App,
        id: LearningId,
        verb: &str,
        result: Result<(), crate::service::ServiceError>,
    ) {
        match result {
            Ok(()) => {
                app.update(Message::Learning(LearningMessage::Actioned(id)));
                app.update(Message::System(
                    crate::tui::messages::SystemMessage::StatusInfo(format!(
                        "Learning {id} {verb}ed"
                    )),
                ));
            }
            Err(e) => {
                app.update(Message::System(
                    crate::tui::messages::SystemMessage::StatusInfo(format!(
                        "Could not {verb} learning: {e}"
                    )),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::db::{CreateLearningRow, Database, LearningRetrievalStore, LearningStore};
    use crate::models::{
        Learning, LearningId, LearningKind, LearningScope, LearningStatus, LearningVerdict,
    };
    use crate::tui::ViewMode;
    use chrono::Utc;
    use std::sync::Arc;

    fn make_runtime(db: Arc<Database>) -> TuiRuntime {
        let (tx, _rx) = mpsc::unbounded_channel();
        let (feed_tx, _) = mpsc::unbounded_channel();
        let db_arc: Arc<dyn crate::db::TaskStore> = db.clone();
        let runner: Arc<dyn crate::process::ProcessRunner> =
            Arc::new(crate::process::MockProcessRunner::new(vec![]));
        let emb_svc = EmbeddingService::new_noop();
        TuiRuntime {
            task_svc: Arc::new(crate::service::TaskService::new(
                db_arc.clone(),
                runner.clone(),
            )),
            epic_svc: Arc::new(crate::service::EpicService::new(db_arc.clone())),
            todo_svc: Arc::new(crate::service::TodoService::new(db.clone())),
            learning_svc: Arc::new(crate::service::LearningService::new(
                db_arc.clone(),
                emb_svc.clone(),
            )),
            feed_runner: Some(crate::feed::FeedRunner::new(
                db_arc.clone(),
                feed_tx,
                runner.clone(),
            )),
            feed_invalidate_tx: None,
            feed_db: db_arc.clone(),
            database: db_arc,
            msg_tx: tx,
            runner,
            editor_session: Arc::new(std::sync::Mutex::new(None)),
            emb_svc,
            last_change_count: std::sync::atomic::AtomicI64::new(-1),
        }
    }

    fn make_learning(id: LearningId) -> Learning {
        Learning {
            id,
            kind: LearningKind::Convention,
            summary: format!("Learning {id}"),
            detail: None,
            scope: LearningScope::Repo,
            scope_ref: Some("/repo".to_string()),
            tags: vec![],
            status: LearningStatus::Approved,
            source_task_id: None,
            upvote_count: 0,
            last_upvoted_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    async fn insert_learning(db: &Arc<Database>) -> LearningId {
        db.create_learning(CreateLearningRow {
            kind: LearningKind::Convention,
            summary: "test learning",
            detail: None,
            scope: LearningScope::User,
            scope_ref: None,
            tags: &[],
            source_task_id: None,
            embedding: None,
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn exec_archive_learning_updates_db_and_sends_actioned_message() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let id = insert_learning(&db).await;
        let rt = make_runtime(db.clone());
        let mut app = App::new(vec![]);
        // Put the app in Learnings view with the learning
        let learning = make_learning(id);
        app.update(Message::Learning(LearningMessage::Show(vec![learning])));

        rt.exec_archive_learning(&mut app, id).await;

        // Learning should be removed from the overlay list
        assert!(matches!(
            app.view_mode(),
            ViewMode::Learnings { learnings, .. } if learnings.is_empty()
        ));
        // DB should show archived
        let updated = db.get_learning(id).await.unwrap().unwrap();
        assert_eq!(updated.status, LearningStatus::Archived);
    }

    #[tokio::test]
    async fn exec_reject_learning_updates_db_and_sends_actioned_message() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let id = insert_learning(&db).await;
        let rt = make_runtime(db.clone());
        let mut app = App::new(vec![]);
        let learning = make_learning(id);
        app.update(Message::Learning(LearningMessage::Show(vec![learning])));

        rt.exec_reject_learning(&mut app, id).await;

        assert!(matches!(
            app.view_mode(),
            ViewMode::Learnings { learnings, .. } if learnings.is_empty()
        ));
        let updated = db.get_learning(id).await.unwrap().unwrap();
        assert_eq!(updated.status, LearningStatus::Rejected);
    }

    #[tokio::test]
    async fn exec_load_passes_learnings_to_show_learnings_sorted_by_upvote_count() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());

        // Insert two learnings; bump the second one's upvote_count via patch.
        let id1 = db
            .create_learning(CreateLearningRow {
                kind: LearningKind::Convention,
                summary: "learning 1",
                detail: None,
                scope: LearningScope::Repo,
                scope_ref: Some("/repo"),
                tags: &[],
                source_task_id: None,
                embedding: None,
            })
            .await
            .unwrap();
        let id2 = db
            .create_learning(CreateLearningRow {
                kind: LearningKind::Convention,
                summary: "learning 2",
                detail: None,
                scope: LearningScope::User,
                scope_ref: None,
                tags: &[],
                source_task_id: None,
                embedding: None,
            })
            .await
            .unwrap();
        // Seed upvote_count=3 on id2 via the production helped-verdict path.
        // Verdicts are no longer tied to a task, so no task setup is needed.
        db.apply_verdicts_tx(&[
            (id2, LearningVerdict::Helped),
            (id2, LearningVerdict::Helped),
            (id2, LearningVerdict::Helped),
        ])
        .await
        .unwrap();

        let rt = make_runtime(db.clone());
        let mut app = App::new(vec![]);

        rt.exec_load_learnings(&mut app).await;

        // TUI handler sorts by upvote_count DESC: id2 (count=3) before id1 (count=0).
        if let ViewMode::Learnings { learnings, .. } = app.view_mode() {
            assert_eq!(learnings.len(), 2);
            assert_eq!(
                learnings[0].id, id2,
                "higher upvote_count should sort first"
            );
            assert_eq!(
                learnings[1].id, id1,
                "lower upvote_count should sort second"
            );
        } else {
            panic!("expected Learnings view mode");
        }
    }

    /// Verify the `LearningServiceApi` seam is injectable without a real database.
    ///
    /// A mock that always returns `ServiceError::Validation` is wired in as
    /// `learning_svc`. `exec_archive_learning` must surface the error through
    /// the status bar — not panic or construct its own `LearningService`.
    #[tokio::test]
    async fn exec_archive_learning_uses_injected_learning_svc_not_ad_hoc_construction() {
        use crate::db::LearningFilter;
        use crate::models::{Learning, LearningVerdict, RetrievalSource};
        use crate::service::{LearningServiceApiStub, ServiceError};

        /// Only the methods this flow touches are mocked; the rest inherit the
        /// panicking defaults from `LearningServiceApiStub`.
        struct AlwaysFailLearningService;

        #[async_trait::async_trait]
        impl LearningServiceApiStub for AlwaysFailLearningService {
            async fn list_learnings(
                &self,
                _: LearningFilter,
            ) -> Result<Vec<Learning>, ServiceError> {
                Ok(vec![])
            }
            async fn archive_learning(&self, _: LearningId) -> Result<(), ServiceError> {
                Err(ServiceError::Validation("injected mock error".into()))
            }
            async fn record_retrieval(
                &self,
                _: crate::models::TaskId,
                _: LearningId,
                _: RetrievalSource,
            ) -> Result<(), ServiceError> {
                Ok(())
            }
            async fn apply_verdicts(
                &self,
                _: crate::models::TaskId,
                _: Vec<(LearningId, LearningVerdict)>,
            ) -> Result<(), ServiceError> {
                Ok(())
            }
            async fn archive_stale_learnings(
                &self,
                _: chrono::DateTime<chrono::Utc>,
            ) -> Result<u64, ServiceError> {
                Ok(0)
            }
        }

        crate::learning_service_api!(service_api_stub_bridge, AlwaysFailLearningService);

        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let db_arc: Arc<dyn crate::db::TaskStore> = db.clone();
        let (tx, _rx) = mpsc::unbounded_channel();
        let (feed_tx, _) = mpsc::unbounded_channel();
        let runner: Arc<dyn crate::process::ProcessRunner> =
            Arc::new(crate::process::MockProcessRunner::new(vec![]));
        let emb_svc = EmbeddingService::new_noop();
        let rt = TuiRuntime {
            task_svc: Arc::new(crate::service::TaskService::new(
                db_arc.clone(),
                runner.clone(),
            )),
            epic_svc: Arc::new(crate::service::EpicService::new(db_arc.clone())),
            todo_svc: Arc::new(crate::service::TodoService::new(db.clone())),
            learning_svc: Arc::new(AlwaysFailLearningService),
            feed_runner: Some(crate::feed::FeedRunner::new(
                db_arc.clone(),
                feed_tx,
                runner.clone(),
            )),
            feed_invalidate_tx: None,
            feed_db: db_arc.clone(),
            database: db_arc,
            msg_tx: tx,
            runner,
            editor_session: Arc::new(std::sync::Mutex::new(None)),
            emb_svc,
            last_change_count: std::sync::atomic::AtomicI64::new(-1),
        };
        let mut app = App::new(vec![]);

        rt.exec_archive_learning(&mut app, LearningId(99)).await;

        // The mock returns an error; it must surface in the status bar.
        let msg = app.status_message().unwrap_or_default();
        assert!(
            msg.contains("injected mock error"),
            "expected mock error in status bar, got: {msg:?}"
        );
    }
}
