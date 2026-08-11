use super::*;

impl TuiRuntime {
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
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::service::{LearningServiceApiStub, ServiceError};
    use std::sync::Mutex;

    /// Records the cutoff it was handed so the test can assert the sweep derives
    /// it from `STALE_LEARNING_THRESHOLD` rather than inventing one. Only the
    /// swept method is mocked; every other seam method keeps the panicking
    /// default, which is what proves nothing else is called.
    struct RecordingSweep {
        cutoff: Mutex<Option<chrono::DateTime<chrono::Utc>>>,
        result: Result<u64, ()>,
    }

    #[async_trait::async_trait]
    impl LearningServiceApiStub for RecordingSweep {
        async fn archive_stale_learnings(
            &self,
            cutoff: chrono::DateTime<chrono::Utc>,
        ) -> Result<u64, ServiceError> {
            *self.cutoff.lock().unwrap() = Some(cutoff);
            self.result
                .map_err(|()| ServiceError::Validation("injected sweep failure".into()))
        }
    }

    crate::learning_service_api!(service_api_stub_bridge, RecordingSweep);

    async fn runtime_with(svc: Arc<RecordingSweep>) -> TuiRuntime {
        let db = crate::runtime::tests::test_db().await;
        let (tx, _rx) = mpsc::unbounded_channel();
        let runner: Arc<dyn crate::process::ProcessRunner> =
            Arc::new(crate::process::MockProcessRunner::new(vec![]));
        let mut rt = crate::runtime::tests::make_runtime(db, tx, runner).await;
        rt.learning_svc = svc;
        rt
    }

    /// The sweep must go through the injected `learning_svc` seam — not build its
    /// own `LearningService` — and must pass a cutoff derived from
    /// `STALE_LEARNING_THRESHOLD`. This is now the only runtime-level guard on
    /// that seam; the overlay tests that used to cover it are gone.
    #[tokio::test]
    async fn exec_archive_stale_learnings_sweeps_through_the_injected_seam() {
        let svc = Arc::new(RecordingSweep {
            cutoff: Mutex::new(None),
            result: Ok(3),
        });
        let before = chrono::Utc::now();

        runtime_with(svc.clone())
            .await
            .exec_archive_stale_learnings()
            .await;

        let cutoff = svc.cutoff.lock().unwrap().expect("seam must be called");
        let threshold = chrono::Duration::from_std(crate::tui::STALE_LEARNING_THRESHOLD).unwrap();
        // The cutoff is `now - threshold`, computed inside the call, so it lands
        // between `before - threshold` and `after - threshold`.
        assert!(cutoff >= before - threshold - chrono::Duration::seconds(5));
        assert!(cutoff <= chrono::Utc::now() - threshold + chrono::Duration::seconds(5));
    }

    /// A failing sweep is logged and swallowed: it is a background job, so it
    /// must not propagate or panic into the tick loop.
    #[tokio::test]
    async fn exec_archive_stale_learnings_swallows_a_service_error() {
        let svc = Arc::new(RecordingSweep {
            cutoff: Mutex::new(None),
            result: Err(()),
        });

        runtime_with(svc.clone())
            .await
            .exec_archive_stale_learnings()
            .await;

        assert!(
            svc.cutoff.lock().unwrap().is_some(),
            "the seam was still called"
        );
    }
}
