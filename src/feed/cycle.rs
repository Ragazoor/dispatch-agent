//! The one feed cycle, shared by both surfaces that can start one.
//!
//! A FEED CYCLE is the whole sequence a feed request performs for one epic:
//! claim the epic, read its current feed configuration, exec the command, parse
//! stdout, apply the degraded-emission guard, dispatch the sync, and tear down
//! the worktrees of every task the sync removed. [`FeedCycle::run`] owns all of
//! it; its two callers — [`crate::feed::FeedRunner`]'s auto-poll job and the
//! manual "r" refresh in `src/runtime/epics.rs::exec_trigger_epic_feed` —
//! differ only in how they present the [`FeedCycleOutcome`].
//!
//! That single-function discipline is deliberate and load-bearing: the feed
//! pipeline has repeatedly grown a step that had to behave identically on both
//! paths (the shared exec, the shared parse, the role dispatcher, the teardown,
//! and now the serialisation claim), and every time it was added twice it drifted.
//! A new step belongs here, not in the two callers.
//!
//! See feeds.allium `SerialisedFeedCycle`.

use std::sync::Arc;
use std::time::Duration;

use super::guard::FeedSyncGuard;
use crate::db::TaskStore;
use crate::dispatch::resolve_feed_item_repo_paths;
use crate::models::EpicId;
use crate::process::ProcessRunner;

/// What a feed cycle did, for its caller to present.
pub(crate) enum FeedCycleOutcome {
    /// The sync ran. `count` is the number of items the command emitted (not
    /// the number of tasks written), and `affected_epics` is every epic whose
    /// contents may have changed, for notification.
    Synced {
        count: usize,
        affected_epics: Vec<EpicId>,
        /// `Some(reason)` when the sync ran ADDITIVELY because the command
        /// wrote to stderr — it removed nothing, so a caller that presents this
        /// outcome must say so rather than let it read as a full reconcile.
        /// See feeds.allium: `DegradedNonEmptyEmission`.
        degraded: Option<String>,
    },
    /// A cycle for this epic was already in flight, so this request did nothing
    /// at all — no exec, no sync, no teardown.
    Busy,
    /// The cycle stopped early. The string is already logged by the time it is
    /// returned; it is carried so the manual path can put it in the status bar.
    Failed(String),
}

/// One feed cycle for one epic.
///
/// `feed_command`, `feed_role` and `group_by_repo` are deliberately NOT fields:
/// they are read from the epic inside [`run`](Self::run), after the claim, so
/// neither caller can act on a stale snapshot of them.
pub(crate) struct FeedCycle {
    pub(crate) db: Arc<dyn TaskStore>,
    pub(crate) runner: Arc<dyn ProcessRunner>,
    pub(crate) guard: Arc<FeedSyncGuard>,
    pub(crate) epic_id: EpicId,
    /// Presentation only — status-bar strings and log fields. Never decides
    /// behaviour, so a stale title is harmless.
    pub(crate) epic_title: String,
    /// `Some` from the auto-poll path, which fetches the repo-path list once
    /// per tick and shares it across every epic's job. `None` from the manual
    /// path, resolved inside after the claim so a dropped request does no DB
    /// work.
    pub(crate) known_paths: Option<Arc<Vec<String>>>,
    /// Deadline passed to [`super::exec_feed_command`]. Both production
    /// callers pass `exec::FEED_COMMAND_TIMEOUT`; tests inject a short value
    /// so a deliberately-hung command times out fast instead of making the
    /// suite wait out the real production deadline.
    pub(crate) command_timeout: Duration,
}

impl FeedCycle {
    /// Run the cycle. Logs every failure itself — it holds the epic id and
    /// title, and logging here rather than in the callers is what stops a
    /// failure being logged twice or not at all.
    pub(crate) async fn run(self) -> FeedCycleOutcome {
        // Claim first: everything below, including the exec, is inside the
        // critical section. `_claim` releases on drop, so every `return` path
        // below — and a panic — frees the epic.
        let Some(_claim) = self.guard.try_claim(self.epic_id) else {
            tracing::debug!(
                epic_id = self.epic_id.0,
                epic_title = %self.epic_title,
                "feed: a cycle for this epic is already in flight; dropping this request"
            );
            return FeedCycleOutcome::Busy;
        };

        let mut epic = match self.db.get_epic(self.epic_id).await {
            Ok(Some(epic)) => epic,
            Ok(None) => return self.fail("epic no longer exists"),
            Err(err) => return self.fail(format!("failed to read epic: {err:#}")),
        };
        let Some(feed_command) = epic.feed_command.take() else {
            return self.fail("epic has no feed command");
        };

        let output = match super::exec_feed_command(
            &feed_command,
            self.epic_id.0,
            &self.epic_title,
            self.command_timeout,
        )
        .await
        {
            Ok(output) => output,
            // exec_feed_command logs spawn failures, timeouts, non-zero exits
            // and stderr-on-success itself, so this is already in app.log.
            Err(err) => return FeedCycleOutcome::Failed(err),
        };

        let items = match super::parse_feed_items(&output.stdout) {
            Ok(items) => items,
            Err(err) => return self.fail(format!("failed to parse JSON output: {err:#}")),
        };

        // A zero-item emission that also wrote to stderr is a degraded run, not
        // an empty one: syncing it would delete every feed task in this epic's
        // subtree. feeds.allium: DegradedEmptyEmission.
        if let Some(reason) = super::degraded_empty_emission(items.len(), &output.stderr) {
            return self.fail(reason);
        }

        // A NON-empty emission that also wrote to stderr is partially degraded:
        // trustworthy about what it contains, not about what it omits. It syncs,
        // but additively — no stale delete, and so no teardown of a live review
        // agent whose PR one soft-failed sub-query happened to drop.
        // feeds.allium: DegradedNonEmptyEmission.
        let degraded = super::degraded_partial_emission(items.len(), &output.stderr);
        let mode = match &degraded {
            Some(reason) => {
                tracing::warn!(
                    epic_id = self.epic_id.0,
                    epic_title = %self.epic_title,
                    "feed: syncing additively, no removals this cycle: {reason}"
                );
                super::SyncMode::Additive
            }
            None => super::SyncMode::Reconcile,
        };

        let count = items.len();
        let known_paths = match &self.known_paths {
            Some(paths) => Arc::clone(paths),
            None => Arc::new(self.db.list_repo_paths().await.unwrap_or_default()),
        };
        let repo_paths = resolve_feed_item_repo_paths(&items, &known_paths);
        let base_branches = super::resolve_base_branches(&repo_paths, &*self.runner);
        let entries = super::FeedItemWithTarget::zip(items, repo_paths, base_branches);

        let outcome = match super::run_feed_sync_by_role(
            &*self.db,
            self.epic_id,
            epic.feed_role,
            epic.group_by_repo,
            entries,
            mode,
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(err) => return self.fail(format!("{err:#}")),
        };

        super::recalculate_epic_status_after_feed(&*self.db, self.epic_id, "FeedCycle::run").await;

        // Teardown before the caller notifies, on BOTH surfaces: a notification
        // means reconciled AND cleaned up, so the board never shows a row gone
        // while its worktree is still being removed. feeds.allium
        // RoleRoutedFeedSync, teardown-vs-notification.
        super::cleanup_removed_feed_tasks(self.runner.clone(), outcome.removed).await;

        FeedCycleOutcome::Synced {
            count,
            affected_epics: outcome.affected_epics,
            degraded,
        }
    }

    /// Log a failure against this cycle's epic and return it for the caller to
    /// present. Every `Failed` outcome except the exec's (which
    /// `exec_feed_command` has already logged) goes through here.
    fn fail(&self, reason: impl Into<String>) -> FeedCycleOutcome {
        let reason = reason.into();
        tracing::warn!(
            epic_id = self.epic_id.0,
            epic_title = %self.epic_title,
            "feed cycle failed: {reason}"
        );
        FeedCycleOutcome::Failed(reason)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use std::path::Path;

    use super::super::exec::AlwaysFailRunner;
    use super::*;
    use crate::db::{Database, EpicCrud, EpicPatch, EpicRead};
    use crate::models::FeedRole;

    /// One PR, enough to be routed and therefore enough to be stranded.
    const EMISSION: &str = r#"[{"external_id":"pr-1","title":"PR 1","description":"","status":"backlog","tag":"pr-review"}]"#;

    /// A `reviews_parent` feed epic whose command records that it ran by
    /// creating `sentinel`, then emits one item.
    ///
    /// The role matters: `reviews_parent` is the flavour the bucket-5 failure
    /// arms used to endanger. A cycle that fell back to `FeedRole::None` on a
    /// failed epic read would route this emission through the FLAT upsert and
    /// strand the task directly on the parent, violating
    /// `NoFlatFeedTasksOnReviewsParent`.
    async fn reviews_parent_with_sentinel_command(db: &Database, sentinel: &Path) -> EpicId {
        let epic = db.create_epic("Reviews", "", None).await.unwrap();
        let cmd = format!("touch {}; echo '{EMISSION}'", sentinel.display());
        db.patch_epic(
            epic.id,
            &EpicPatch::new()
                .feed_role(FeedRole::ReviewsParent)
                .feed_command(Some(cmd.as_str())),
        )
        .await
        .unwrap();
        epic.id
    }

    fn cycle(db: Arc<Database>, epic_id: EpicId) -> FeedCycle {
        FeedCycle {
            db,
            runner: Arc::new(AlwaysFailRunner),
            guard: Arc::new(FeedSyncGuard::default()),
            epic_id,
            epic_title: "Reviews".to_string(),
            known_paths: None,
            command_timeout: Duration::from_secs(5),
        }
    }

    fn failure(outcome: FeedCycleOutcome) -> String {
        match outcome {
            FeedCycleOutcome::Failed(err) => err,
            FeedCycleOutcome::Synced { count, .. } => {
                panic!("a failed epic read must not sync, but {count} item(s) were synced")
            }
            FeedCycleOutcome::Busy => panic!("nothing else claimed the epic"),
        }
    }

    /// feeds.allium `FeedCommandFailure` bucket 5: the epic is gone by the time
    /// the cycle claims it. Nothing may be spawned and nothing may be synced —
    /// in particular the cycle must not proceed with a defaulted `FeedRole`.
    #[tokio::test]
    async fn a_cycle_whose_epic_is_gone_fails_without_running_the_command() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let dir = tempfile::tempdir().unwrap();
        let sentinel = dir.path().join("command-ran");
        let epic_id = reviews_parent_with_sentinel_command(&db, &sentinel).await;

        db.delete_epic(epic_id).await.unwrap();

        let err = failure(cycle(db, epic_id).run().await);

        assert!(
            err.contains("epic no longer exists"),
            "a missing epic must fail as such, got: {err}"
        );
        assert!(
            !sentinel.exists(),
            "the failure precedes the exec, so the feed command must never run"
        );
    }

    /// The other half of bucket 5: the epic row is there but cannot be READ.
    ///
    /// Fault-injected by renaming the `epics` table out from under the open
    /// `Database` through a second connection to the same file — the arm is
    /// unreachable otherwise. The `tasks` table is left intact so the
    /// no-stranded-tasks assertion can still run.
    #[tokio::test]
    async fn a_cycle_whose_epic_read_errors_fails_without_syncing_anything() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("tasks.db");
        let db = Arc::new(Database::open(&db_path).await.unwrap());
        let sentinel = dir.path().join("command-ran");
        let epic_id = reviews_parent_with_sentinel_command(&db, &sentinel).await;

        rusqlite::Connection::open(&db_path)
            .unwrap()
            .execute_batch("ALTER TABLE epics RENAME TO epics_unreadable")
            .unwrap();

        let err = failure(cycle(db.clone(), epic_id).run().await);

        assert!(
            err.contains("failed to read epic"),
            "an unreadable epic must fail as such, got: {err}"
        );
        assert!(
            !sentinel.exists(),
            "the failure precedes the exec, so the feed command must never run"
        );
        assert!(
            db.list_tasks_for_epic(epic_id).await.unwrap().is_empty(),
            "no sync may run, so nothing may be stranded flat on the \
             reviews_parent epic"
        );
    }

    /// #4150: exec_feed_command has a deadline. A command that never exits
    /// must fail within it AND release the epic's claim so the NEXT cycle
    /// proceeds — proving only that one call returned is not enough (see
    /// feeds.allium SerialisedFeedCycle's bounded-cost note). Reuses the
    /// FIFO shape from
    /// `src/runtime/tests.rs::manual_refresh_is_dropped_while_a_real_auto_poll_cycle_is_in_flight`:
    /// `cat <fifo>` blocks forever because nothing ever opens the write end,
    /// so the hang is genuine, not a timed sleep that would end on its own.
    #[tokio::test]
    async fn a_hung_command_times_out_and_the_epic_recovers_on_the_next_cycle() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("Feed", "", None).await.unwrap();

        let fifo = std::env::temp_dir().join(format!("dispatch_feed_timeout_{}", epic.id.0));
        let _ = std::fs::remove_file(&fifo);
        let mkfifo = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(mkfifo.success(), "mkfifo failed for {}", fifo.display());

        // Never written to: this command hangs forever until killed.
        db.patch_epic(
            epic.id,
            &EpicPatch::new().feed_command(Some(format!("cat {}", fifo.display()).as_str())),
        )
        .await
        .unwrap();

        let guard = Arc::new(FeedSyncGuard::default());
        let hung_cycle = FeedCycle {
            db: db.clone(),
            runner: Arc::new(AlwaysFailRunner),
            guard: guard.clone(),
            epic_id: epic.id,
            epic_title: "Feed".to_string(),
            known_paths: None,
            command_timeout: Duration::from_millis(100),
        };

        let err = match hung_cycle.run().await {
            FeedCycleOutcome::Failed(err) => err,
            FeedCycleOutcome::Synced { count, .. } => {
                panic!("a hung command must not sync, but {count} item(s) were synced")
            }
            FeedCycleOutcome::Busy => panic!("nothing else claimed the epic"),
        };
        assert!(
            err.contains("timed out"),
            "a hung command must fail as a timeout, got: {err}"
        );

        // The real proof: point the epic at a fast, successful command and
        // run a FRESH cycle. If the claim (or anything else) were still
        // wedged, this would come back Busy or Failed instead of Synced.
        db.patch_epic(epic.id, &EpicPatch::new().feed_command(Some("echo '[]'")))
            .await
            .unwrap();
        let recovered_cycle = FeedCycle {
            db,
            runner: Arc::new(AlwaysFailRunner),
            guard,
            epic_id: epic.id,
            epic_title: "Feed".to_string(),
            known_paths: None,
            command_timeout: Duration::from_secs(5),
        };
        let outcome = recovered_cycle.run().await;
        assert!(
            matches!(outcome, FeedCycleOutcome::Synced { .. }),
            "the epic must recover and sync on the next cycle after a timeout, got a non-Synced outcome"
        );

        let _ = std::fs::remove_file(&fifo);
    }
}
