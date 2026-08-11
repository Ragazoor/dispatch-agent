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
