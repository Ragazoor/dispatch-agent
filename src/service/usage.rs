//! Feature-usage telemetry writes.
//!
//! Recording is fire-and-forget — callers spawn it and never await the result —
//! but "fire-and-forget" is not "silent". See `UsageWriteFailureIsSilent` in
//! `docs/specs/observability.allium`.

use crate::db::UsageStore;
use crate::models::UsageEvent;

/// Record a feature-usage event, warning on failure instead of discarding the
/// `Result`.
///
/// The write is best-effort: there is no retry and no propagation to the
/// caller. What there is, is a log line. Pruning passes read the *absence* of a
/// count as "unused", so a silently dropped write can make a used feature look
/// prunable — the warning is what keeps that from happening unobserved.
pub async fn record_usage_event_logged<D>(db: &D, event: &UsageEvent)
where
    D: UsageStore + ?Sized,
{
    if let Err(e) = db.record_usage_event(event).await {
        tracing::warn!(
            target: "usage",
            category = event.category.as_str(),
            action = %event.action,
            error = %e,
            "failed to record usage event"
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::db::{UsageCap, UsageQuery};
    use crate::models::{UsageActor, UsageCategory, UsageSummary};
    use crate::test_log::logged_during;

    struct FailingUsageStore;

    #[async_trait::async_trait]
    impl UsageStore for FailingUsageStore {
        async fn record_usage_event_with_cap(
            &self,
            _event: &UsageEvent,
            _cap: UsageCap,
        ) -> anyhow::Result<()> {
            Err(anyhow::anyhow!("disk on fire"))
        }

        async fn query_usage(&self, _q: &UsageQuery) -> anyhow::Result<Vec<UsageSummary>> {
            Ok(vec![])
        }
    }

    struct OkUsageStore;

    #[async_trait::async_trait]
    impl UsageStore for OkUsageStore {
        async fn record_usage_event_with_cap(
            &self,
            _event: &UsageEvent,
            _cap: UsageCap,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn query_usage(&self, _q: &UsageQuery) -> anyhow::Result<Vec<UsageSummary>> {
            Ok(vec![])
        }
    }

    fn event() -> UsageEvent {
        UsageEvent {
            category: UsageCategory::Keybinding,
            action: "move_task_right".to_string(),
            detail: Some("l".to_string()),
            actor: UsageActor::Human,
        }
    }

    #[tokio::test]
    async fn failed_usage_write_warns_instead_of_being_swallowed() {
        let logs = logged_during(|| async {
            record_usage_event_logged(&FailingUsageStore, &event()).await;
        })
        .await;

        assert!(
            logs.contains("WARN"),
            "expected a warn-level line, got: {logs}"
        );
        assert!(
            logs.contains("failed to record usage event"),
            "expected the failure message, got: {logs}"
        );
        assert!(
            logs.contains("move_task_right"),
            "warning must identify the event's action, got: {logs}"
        );
        assert!(
            logs.contains("disk on fire"),
            "warning must carry the underlying error, got: {logs}"
        );
    }

    #[tokio::test]
    async fn successful_usage_write_logs_nothing() {
        let logs = logged_during(|| async {
            record_usage_event_logged(&OkUsageStore, &event()).await;
        })
        .await;

        assert!(logs.is_empty(), "expected no log output, got: {logs}");
    }
}
