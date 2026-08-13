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

        let output =
            match super::exec_feed_command(&feed_command, self.epic_id.0, &self.epic_title).await {
                Ok(output) => output,
                // exec_feed_command logs spawn failures, non-zero exits and
                // stderr-on-success itself, so this is already in app.log.
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
