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
//! therefore costs one `git fetch` per interval — no worktree, no tmux window,
//! no agent.
//!
//! Retry falls out of that for free, because `last_processed_sha` is written
//! only on a *successful* promotion (subtask #4205's `wrap_up(action="merge")`,
//! not yet landed). A tick whose agent got stuck leaves the SHA stale, so the
//! next tick still sees the branch as unprocessed and runs again. There is no
//! backoff state to keep.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use tokio::sync::mpsc;

use crate::db::{TaskPatch, TaskStore};
use crate::mcp::McpEvent;
use crate::models::{Task, TaskId};
use crate::process::{ProcessRunner, SUBPROCESS_TIMEOUT};

/// Poll interval for the background scheduler task.
///
/// Its own constant rather than a reuse of `FEED_POLL_INTERVAL` or the runtime's
/// `TICK_INTERVAL`, for the same reason those two are separate: the three
/// concerns have no reason to move together. Note this is only how often the
/// loop *looks* — the per-task gate is `schedule_interval_secs`, so a task set
/// to 600s is examined every couple of seconds and acted on every ten minutes.
const SCHEDULER_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// `origin/<branch>`'s tip, with a fetch first so the answer is current.
///
/// Both subprocesses are bounded, like every other git call on a poll path: a
/// hung fetch would otherwise pin a tokio task forever. Returns `None` on any
/// failure — unreachable origin, missing branch, unparseable output — and a
/// `None` deliberately reads as "cannot prove nothing changed", so the caller
/// dispatches rather than skipping. Skipping on a failed measurement is the one
/// outcome that would silently stall a pipeline.
fn fetch_and_resolve_sha(
    repo_path: &str,
    branch: &str,
    runner: &dyn ProcessRunner,
) -> Option<String> {
    let repo_path = crate::models::expand_tilde(repo_path);
    let fetch = runner
        .run_with_timeout(
            "git",
            &["-C", &repo_path, "fetch", "origin", branch],
            SUBPROCESS_TIMEOUT,
        )
        .ok()?;
    if !fetch.status.success() {
        tracing::warn!(
            repo_path,
            branch,
            "scheduler: could not fetch origin; treating the branch as changed"
        );
        return None;
    }

    let remote_ref = format!("origin/{branch}");
    let output = runner
        .run_with_timeout(
            "git",
            &["-C", &repo_path, "rev-parse", &remote_ref],
            SUBPROCESS_TIMEOUT,
        )
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if sha.is_empty() {
        return None;
    }
    Some(sha)
}

/// Background poll loop for scheduled tasks.
///
/// Holds a write-capable [`TaskStore`], which makes it a sanctioned
/// direct-mutation consumer alongside `FeedRunner` — see "Sanctioned
/// direct-mutation consumers" in `docs/conventions.md`. It owns its own
/// invariant (a scheduled task is claimed before it is provisioned and released
/// if provisioning fails) and has no service handle to route through.
pub struct SchedulerRunner {
    db: Arc<dyn TaskStore>,
    notify: mpsc::UnboundedSender<McpEvent>,
    runner: Arc<dyn ProcessRunner>,
    /// When each task was last *looked at* by this process.
    ///
    /// Inserted before the per-task work is spawned, which is what keeps two
    /// ticks two seconds apart from both dispatching: the DB stamp is written
    /// inside the spawned task, far too late to gate the next tick. The
    /// persisted `last_scheduled_check_at` is the cold-start fallback (see
    /// [`Self::is_due`]) so a restart does not redispatch every scheduled task
    /// at once.
    last_check: HashMap<TaskId, Instant>,
    /// Test-only join handles for the jobs spawned by `tick`, mirroring
    /// `FeedRunner::spawned`. Production fires and forgets; tests need a
    /// deterministic completion signal because sleeping is banned by
    /// `./scripts/check-no-test-sleep.sh`.
    #[cfg(test)]
    spawned: Vec<tokio::task::JoinHandle<()>>,
}

impl SchedulerRunner {
    pub fn new(
        db: Arc<dyn TaskStore>,
        notify: mpsc::UnboundedSender<McpEvent>,
        runner: Arc<dyn ProcessRunner>,
    ) -> Self {
        Self {
            db,
            notify,
            runner,
            last_check: HashMap::new(),
            #[cfg(test)]
            spawned: Vec::new(),
        }
    }

    /// Spawns as an independent background task so a slow git fetch or a live
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

    /// Whether `task` is due, given its interval.
    ///
    /// Prefers this process's own observation; falls back to the persisted
    /// stamp on the first look after a restart. A stamp in the future (clock
    /// skew) reads as not-due rather than as overdue — waiting is recoverable,
    /// a redispatch storm is not.
    fn is_due(&self, task: &Task, interval: Duration) -> bool {
        match self.last_check.get(&task.id) {
            Some(seen) => seen.elapsed() >= interval,
            None => match task.last_scheduled_check_at {
                Some(at) => (Utc::now() - at)
                    .to_std()
                    .is_ok_and(|elapsed| elapsed >= interval),
                // Never checked: due immediately.
                None => true,
            },
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

        // Forget tasks that are no longer schedulable, so the map cannot grow
        // without bound across a long-running session. A task that becomes
        // eligible again is simply due on its next appearance.
        let eligible: std::collections::HashSet<TaskId> = tasks.iter().map(|t| t.id).collect();
        self.last_check.retain(|id, _| eligible.contains(id));

        for task in tasks {
            // `list_scheduled_tasks` already filtered on NOT NULL, so this is
            // just the unwrap. A non-positive interval means "every tick".
            let Some(secs) = task.schedule_interval_secs else {
                continue;
            };
            let interval = Duration::from_secs(secs.max(0) as u64);

            if !self.is_due(&task, interval) {
                continue;
            }
            self.last_check.insert(task.id, Instant::now());

            let db = Arc::clone(&self.db);
            let runner = Arc::clone(&self.runner);
            let notify = self.notify.clone();
            // Spawned, so neither the fetch nor the dispatch blocks the loop.
            let _handle = tokio::spawn(async move {
                Self::check_and_dispatch(db, runner, notify, task).await;
            });
            #[cfg(test)]
            self.spawned.push(_handle);
        }
    }

    /// One scheduled task's turn: measure, then either skip or dispatch.
    async fn check_and_dispatch(
        db: Arc<dyn TaskStore>,
        runner: Arc<dyn ProcessRunner>,
        notify: mpsc::UnboundedSender<McpEvent>,
        task: Task,
    ) {
        let task_id = task.id;

        if let (Some(pinned), Some(last)) =
            (task.pinned_branch.clone(), task.last_processed_sha.clone())
        {
            let repo_path = task.repo_path.clone();
            let probe_runner = Arc::clone(&runner);
            let current = tokio::task::spawn_blocking(move || {
                fetch_and_resolve_sha(&repo_path, &pinned, &*probe_runner)
            })
            .await
            .unwrap_or(None);

            if current.as_deref() == Some(last.as_str()) {
                tracing::debug!(
                    task_id = task_id.0,
                    "scheduler: pinned branch unchanged; skipping dispatch"
                );
                Self::stamp_checked(&*db, task_id).await;
                return;
            }
        }
        // No pinned branch (a plain cron-like task), never processed, or the
        // branch moved — all three dispatch.

        match db.try_claim_scheduled_task(task_id, Utc::now()).await {
            Ok(true) => {}
            Ok(false) => {
                // Someone else took it, or it stopped being idle between the
                // listing and now. Nothing was written, so nothing is owed.
                Self::stamp_checked(&*db, task_id).await;
                return;
            }
            Err(err) => {
                tracing::warn!(task_id = task_id.0, "scheduler: claim failed: {err:#}");
                return;
            }
        }

        let dispatch_runner = Arc::clone(&runner);
        let launched = tokio::task::spawn_blocking(move || {
            crate::dispatch::pipeline_agent(&task, &*dispatch_runner)
        })
        .await;

        match launched {
            Ok(Ok(result)) => {
                // Record where the agent landed. Without this the next tick
                // would see `tmux_window IS NULL` and dispatch a second agent
                // into the same worktree.
                let patch = TaskPatch::new()
                    .worktree(Some(result.worktree_path.as_str()))
                    .tmux_window(Some(result.tmux_window.as_str()))
                    .last_scheduled_check_at(Some(Utc::now()));
                if let Err(err) = db.patch_task(task_id, &patch).await {
                    tracing::warn!(
                        task_id = task_id.0,
                        "scheduler: failed to record worktree/tmux_window: {err:#}"
                    );
                }
                tracing::info!(task_id = task_id.0, "scheduler: dispatched");
            }
            Ok(Err(err)) => {
                tracing::warn!(task_id = task_id.0, "scheduler: dispatch failed: {err:#}");
                Self::release_claim(&*db, task_id).await;
                Self::stamp_checked(&*db, task_id).await;
            }
            Err(err) => {
                tracing::warn!(
                    task_id = task_id.0,
                    "scheduler: dispatch worker died: {err}"
                );
                Self::release_claim(&*db, task_id).await;
                Self::stamp_checked(&*db, task_id).await;
            }
        }

        let _ = notify.send(McpEvent::TaskChanged(task_id));
    }

    async fn stamp_checked(db: &dyn TaskStore, task_id: TaskId) {
        let patch = TaskPatch::new().last_scheduled_check_at(Some(Utc::now()));
        if let Err(err) = db.patch_task(task_id, &patch).await {
            tracing::warn!(
                task_id = task_id.0,
                "scheduler: failed to stamp last_scheduled_check_at: {err:#}"
            );
        }
    }

    async fn release_claim(db: &dyn TaskStore, task_id: TaskId) {
        match db.try_release_backlog_claim(task_id).await {
            Ok(true) | Ok(false) => {}
            Err(err) => tracing::warn!(
                task_id = task_id.0,
                "scheduler: failed to release claim: {err:#}"
            ),
        }
    }
}

#[cfg(test)]
mod tests;
