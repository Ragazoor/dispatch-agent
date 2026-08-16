//! Scheduled (re)dispatch — `DispatchScheduledTask` in `docs/specs/dispatch.allium`.
//!
//! A background poll loop structurally parallel to [`crate::feed::FeedRunner`]:
//! it walks tasks with `schedule_interval_secs` set, applies an elapsed-time
//! gate per task, and hands the actual work to a spawned tokio task so a slow
//! `git fetch` or a live dispatch never stalls the loop.
//!
//! The point of the whole thing is the *skip*. A pinned-branch task compares
//! `origin/<pinned_branch>`'s tip against `last_processed_sha` and, when they
//! match, does nothing but stamp `last_scheduled_check_at`. An idle pipeline
//! therefore costs one `git ls-remote` per interval — no fetch, no worktree, no
//! tmux window, no agent.
//!
//! Retry falls out of that for free, because `last_processed_sha` is written
//! only on a *successful* promotion (subtask #4205's `wrap_up(action="merge")`,
//! not yet landed). A tick whose agent got stuck leaves the SHA stale, so the
//! next tick still sees the branch as unprocessed and runs again. There is no
//! backoff state to keep.
//!
//! What this module deliberately does *not* own is the dispatch sequence
//! itself. Claim → provision → record where the agent landed → release on
//! failure lives once, in [`crate::service::TaskService::dispatch`]; the
//! scheduler reaches it through [`DispatchClaim::TakeScheduled`] rather than
//! re-deriving it. Only the gate, the branch probe, and the
//! `last_scheduled_check_at` stamp are this module's own.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::sync::mpsc;

use crate::db::TaskRead;
use crate::mcp::McpEvent;
use crate::models::{DispatchMode, Task};
use crate::process::ProcessRunner;
use crate::service::embeddings::EmbeddingService;
use crate::service::{DispatchClaim, DispatchRequest, TaskService};

/// Poll interval for the background scheduler task.
///
/// Its own constant rather than a reuse of `FEED_POLL_INTERVAL` or the runtime's
/// `TICK_INTERVAL`, for the same reason those two are separate: the three
/// concerns have no reason to move together. Note this is only how often the
/// loop *looks* — the per-task gate is `schedule_interval_secs`, so a task set
/// to 600s is examined every couple of seconds and acted on every ten minutes.
const SCHEDULER_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Background poll loop for scheduled tasks.
pub struct SchedulerRunner {
    /// Read handle for the eligibility query. Every *write* goes through
    /// `task_svc`, so this stays the narrow read surface.
    db: Arc<dyn TaskRead>,
    task_svc: Arc<TaskService>,
    emb_svc: Arc<EmbeddingService>,
    notify: mpsc::UnboundedSender<McpEvent>,
    runner: Arc<dyn ProcessRunner>,
    /// Test-only join handles for the jobs spawned by `tick`, mirroring
    /// `FeedRunner::spawned`. Production fires and forgets; tests need a
    /// deterministic completion signal because sleeping is banned by
    /// `./scripts/check-no-test-sleep.sh`.
    #[cfg(test)]
    spawned: Vec<tokio::task::JoinHandle<()>>,
}

impl SchedulerRunner {
    pub fn new(
        db: Arc<dyn TaskRead>,
        task_svc: Arc<TaskService>,
        emb_svc: Arc<EmbeddingService>,
        notify: mpsc::UnboundedSender<McpEvent>,
        runner: Arc<dyn ProcessRunner>,
    ) -> Self {
        Self {
            db,
            task_svc,
            emb_svc,
            notify,
            runner,
            #[cfg(test)]
            spawned: Vec::new(),
        }
    }

    /// Spawns as an independent background task so a slow git probe or a live
    /// dispatch can't freeze the UI.
    pub fn start(self) {
        tokio::spawn(async move {
            let mut runner = self;
            let mut interval = tokio::time::interval(SCHEDULER_POLL_INTERVAL);
            loop {
                interval.tick().await;
                runner.tick().await;
            }
        });
    }

    /// Await every job spawned by the ticks run so far, draining the handle
    /// list. Deterministic replacement for a sleep in tests.
    #[cfg(test)]
    pub(crate) async fn join_spawned_jobs(&mut self) {
        for handle in std::mem::take(&mut self.spawned) {
            let _ = handle.await;
        }
    }

    pub async fn tick(&mut self) {
        let tasks = match self.db.list_scheduled_tasks().await {
            Ok(t) => t,
            Err(err) => {
                tracing::warn!("SchedulerRunner: failed to list scheduled tasks: {err:#}");
                return;
            }
        };

        for task in tasks {
            // `list_scheduled_tasks` already filtered on NOT NULL, so this is
            // just the unwrap. A non-positive interval means "every tick".
            let Some(secs) = task.schedule_interval_secs else {
                continue;
            };
            if !is_due(&task, Duration::from_secs(secs.max(0) as u64)) {
                continue;
            }

            // Stamped here, before the work is spawned, which is what stops two
            // ticks two seconds apart from both probing the same task: the
            // persisted stamp is the only gate, so it has to be written while
            // the loop still owns the decision. (The claim inside `dispatch` is
            // what makes a double *dispatch* impossible regardless; this just
            // avoids the wasted probe.) Awaited in the loop deliberately — it
            // only runs for a task that is actually due, which is rare by
            // construction.
            if let Err(err) = self.task_svc.stamp_scheduled_check(task.id).await {
                tracing::warn!(
                    task_id = task.id.0,
                    "scheduler: failed to stamp last_scheduled_check_at: {err:#}"
                );
                // Skip rather than press on: without the stamp the next tick
                // would look again immediately, so dispatching here risks a
                // tight redispatch loop against a database that is already
                // failing.
                continue;
            }

            let task_svc = Arc::clone(&self.task_svc);
            let emb_svc = Arc::clone(&self.emb_svc);
            let runner = Arc::clone(&self.runner);
            let notify = self.notify.clone();
            let _handle = tokio::spawn(async move {
                Self::check_and_dispatch(task_svc, emb_svc, runner, notify, task).await;
            });
            #[cfg(test)]
            self.spawned.push(_handle);
        }
    }

    /// One scheduled task's turn: probe the pinned branch, then either skip or
    /// hand the task to the dispatch seam.
    async fn check_and_dispatch(
        task_svc: Arc<TaskService>,
        emb_svc: Arc<EmbeddingService>,
        runner: Arc<dyn ProcessRunner>,
        notify: mpsc::UnboundedSender<McpEvent>,
        task: Task,
    ) {
        let task_id = task.id;

        if let (Some(pinned), Some(last)) = (
            task.pinned_branch.as_deref(),
            task.last_processed_sha.as_deref(),
        ) {
            let repo_path = task.repo_path.clone();
            let pinned = pinned.to_string();
            let probe_runner = Arc::clone(&runner);
            let current = tokio::task::spawn_blocking(move || {
                crate::git::remote_branch_sha(&repo_path, &pinned, &*probe_runner)
            })
            .await
            .unwrap_or(None);

            if current.as_deref() == Some(last) {
                tracing::debug!(
                    task_id = task_id.0,
                    "scheduler: pinned branch unchanged; skipping dispatch"
                );
                return;
            }
        }
        // No pinned branch (a plain cron-like task), never processed, or the
        // branch moved — all three dispatch.

        let outcome = task_svc
            .dispatch(DispatchRequest {
                task,
                mode: DispatchMode::Pipeline,
                emb_svc,
                epic_ctx: None,
                claim: DispatchClaim::TakeScheduled,
            })
            .await;

        match outcome {
            crate::service::DispatchOutcome::Launched(_) => {
                tracing::info!(task_id = task_id.0, "scheduler: dispatched");
                let _ = notify.send(McpEvent::TaskChanged(task_id));
            }
            // The task stopped being idle between the listing and the claim.
            // Nothing was written and nothing is owed.
            crate::service::DispatchOutcome::ClaimLost => {}
            crate::service::DispatchOutcome::ClaimFailed(err) => {
                tracing::warn!(task_id = task_id.0, "scheduler: claim failed: {err}");
            }
            // `dispatch` has already released the claim.
            crate::service::DispatchOutcome::Failed(reason) => {
                tracing::warn!(task_id = task_id.0, "scheduler: dispatch failed: {reason}");
                let _ = notify.send(McpEvent::TaskChanged(task_id));
            }
        }
    }
}

/// Whether `task` is due, given its interval.
///
/// Reads the persisted stamp only — `tick` writes it before spawning any work,
/// so it paces this process as well as surviving a restart. A stamp in the
/// future (clock skew) reads as not-yet-due rather than overdue: waiting is
/// recoverable, a redispatch storm is not.
fn is_due(task: &Task, interval: Duration) -> bool {
    match task.last_scheduled_check_at {
        Some(at) => (Utc::now() - at)
            .to_std()
            .is_ok_and(|elapsed| elapsed >= interval),
        // Never checked: due immediately.
        None => true,
    }
}

#[cfg(test)]
mod tests;
