mod cycle;
mod exec;
mod guard;
mod ingest;
mod parse;
mod routing;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::db::{RemovedFeedTask, TaskStore};
use crate::mcp::McpEvent;
use crate::models::{Epic, EpicId, MIN_FEED_INTERVAL_SECS};
use crate::process::ProcessRunner;

pub(crate) use cycle::{FeedCycle, FeedCycleOutcome};
pub(crate) use exec::degraded_partial_emission;
pub(crate) use exec::{
    degraded_empty_emission, exec_feed_command, resolve_base_branches, FEED_COMMAND_TIMEOUT,
};
pub(crate) use guard::FeedSyncGuard;
pub(crate) use ingest::{run_feed_sync_by_role, FeedItemWithTarget, SyncMode};
// `pub`, unlike the `pub(crate)` re-exports above: the `verify-feed` CLI in
// src/main.rs is a separate bin crate and is one of this function's three
// callers. See feeds.allium's FeedItemParse block.
pub use parse::parse_feed_items;
pub use routing::route;

/// Log-and-discard on `Err` for the feed writes that report what they deleted,
/// keeping the `Ok` payload so the removed rows reach
/// [`cleanup_removed_feed_tasks`]. An `Err` yields an empty vec — nothing was
/// reported, so there is nothing to tear down; the reconciliation pass
/// continues either way.
///
/// `context` labels the call site in the log line (e.g.
/// `"sync_grouped_feed: upsert_feed_tasks failed"`).
pub(crate) fn removed_or_warn(
    result: anyhow::Result<Vec<RemovedFeedTask>>,
    epic_id: EpicId,
    sub_epic_id: Option<EpicId>,
    context: &str,
) -> Vec<RemovedFeedTask> {
    match result {
        Ok(removed) => removed,
        Err(err) => {
            // `sub_epic_id` is attached when the write is scoped to a sub-epic
            // beneath `epic_id`, so a warning names the epic it actually wrote.
            match sub_epic_id {
                Some(sub_epic_id) => tracing::warn!(
                    epic_id = epic_id.0,
                    sub_epic_id = sub_epic_id.0,
                    "{context}: {err:#}"
                ),
                None => tracing::warn!(epic_id = epic_id.0, "{context}: {err:#}"),
            }
            Vec::new()
        }
    }
}

/// Recalculate an epic's status after feed tasks have been upserted, logging a
/// warning on failure. New non-done tasks can cause a done epic to regress to
/// backlog; the recalculation propagates upward to any parent epic.
///
/// `context` labels the call site in the log line (e.g. `"FeedRunner"`).
pub(crate) async fn recalculate_epic_status_after_feed(
    db: &dyn TaskStore,
    epic_id: EpicId,
    context: &str,
) {
    if let Err(err) = db.recalculate_epic_status(epic_id).await {
        tracing::warn!(
            epic_id = epic_id.0,
            "{context}: recalculate_epic_status failed: {err:#}"
        );
    }
}

/// Tear down the worktree and tmux window of every feed task a sync removed.
///
/// A feed-driven removal is a deletion like any other and owes the same
/// teardown `ArchiveTask` and `DeleteTask` perform — `TaskTeardown` at the head
/// of the archive section of `docs/specs/tasks.allium`: kill the tmux window,
/// remove the git worktree, and delete the branch best-effort.
/// `crate::dispatch::teardown_task` performs all three, and which of them a given
/// row owes is *its* decision, not this function's — see
/// `TeardownIsOwedWheneverThereIsSomethingToRelease` in that spec. This wrapper
/// contributes exactly two policies: per-repo serialisation, and warn-on-failure.
///
/// # Why there is no shared-worktree check here
///
/// `TaskTeardown`'s worktree clause is unconditional, on this path and every
/// other — see `WorktreeIsNeverShared` in `docs/specs/tasks.allium` for why no
/// two tasks can name one worktree.
///
/// Worth stating here because this function is the tempting place to "restore" a
/// safety net: do **not**. Beyond buying nothing, it would cost a store handle
/// this function does not currently take, a round-trip per removed task, and an
/// error path with no correct answer. The tripwire against a reinstated guard now
/// lives on the one wrapper that *does* hold a store handle — see that spec
/// block's coverage note.
///
/// # Per-repo serialisation
///
/// Removals are grouped by `repo_path` and run sequentially within a repo.
/// `teardown_task` shells `git -C <repo> worktree remove --force` and
/// `git branch -D` against the *shared* checkout, and a reviews epic's tasks
/// overwhelmingly share one repo — running those concurrently would contend on
/// that repo's index lock and fail spuriously. Different repos have no shared
/// lock, so they still proceed in parallel.
///
/// The whole of `TaskTeardown` is best-effort: failures are logged at warn and
/// never surfaced, because feed reconciliation is background work, and one
/// task's failure must not abort the rest of its repo's queue.
/// Called once, from [`cycle::FeedCycle::run`], with the `removed` half of the
/// [`ingest::FeedSyncOutcome`] the sync returned — so both feed paths get the
/// teardown by sharing that cycle rather than by each remembering to call this.
pub(crate) async fn cleanup_removed_feed_tasks(
    runner: Arc<dyn ProcessRunner>,
    removed: Vec<RemovedFeedTask>,
) {
    if removed.is_empty() {
        return;
    }

    let mut by_repo: HashMap<String, Vec<RemovedFeedTask>> = HashMap::new();
    for task in removed {
        by_repo
            .entry(task.repo_path.clone())
            .or_default()
            .push(task);
    }

    let mut handles = Vec::new();
    for (_repo, tasks) in by_repo {
        let runner = runner.clone();
        handles.push(tokio::task::spawn_blocking(move || {
            for task in tasks {
                if let Err(failure) = crate::dispatch::teardown_task(
                    &task.repo_path,
                    task.worktree.as_deref(),
                    task.tmux_window.as_ref(),
                    &*runner,
                ) {
                    tracing::warn!(
                        task_id = task.id.0,
                        "feed cleanup: teardown_task failed: {failure}"
                    );
                }
            }
        }));
    }
    for handle in handles {
        // tokio does not log a `spawn_blocking` panic, and the default panic
        // hook writes to stderr — which belongs to the TUI, so that output is
        // lost or garbles the display. Log it ourselves, to the app log.
        if let Err(err) = handle.await {
            tracing::warn!("feed cleanup: teardown thread did not complete: {err}");
        }
    }
}

/// Cadence for a feed epic with no explicit `feed_interval_secs` —
/// `config.default_feed_interval` in `docs/specs/core.allium`.
///
/// Equal to [`MIN_FEED_INTERVAL_SECS`] today and MUST NOT fall below it: the
/// floor binds the *resolved* cadence, so an unset field that polled faster
/// than the fastest value a user may enter would make the floor a lie.
/// `the_default_interval_clears_the_floor` asserts the relation
/// (feeds.allium: `DefaultFeedIntervalClearsTheFloor`). Raising this is safe;
/// lowering it past the floor is not.
///
/// Kept a separate constant rather than derived from the floor: a future
/// default of 120s must not drag the floor up with it.
const DEFAULT_FEED_INTERVAL: Duration = Duration::from_secs(60);

/// Poll interval for the background feed task.
/// Kept in `feed` (not reusing `TICK_INTERVAL` from `runtime`) so the two
/// concerns stay independent.
const FEED_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Whether `epic` is due to run this tick, given `last_run` and `now`.
///
/// The feed-cadence floor is enforced on read (feeds.allium: FeedTick).
/// Write-time validation is what normally keeps a sub-floor row from
/// existing, so reaching the below-floor arm means the value arrived by a
/// path that bypassed the service — a hand-edited database, or a bug — and
/// the runner must not make it work anyway. Skipped rather than clamped: a
/// clamped feed would run at a cadence nobody chose while looking healthy.
///
/// This also covers the negative case, which failed differently and worse.
/// The `as u64` this replaces wrapped a negative into an effectively
/// infinite cadence, so the feed went permanently silent with nothing
/// logged.
fn epic_due(epic: &Epic, last_run: &HashMap<EpicId, Instant>, now: Instant) -> bool {
    let interval = match epic.feed_interval_secs {
        Some(s) if s < MIN_FEED_INTERVAL_SECS => {
            tracing::warn!(
                epic_id = epic.id.0,
                epic_title = %epic.title,
                feed_interval_secs = s,
                min_feed_interval_secs = MIN_FEED_INTERVAL_SECS,
                "FeedRunner: feed_interval_secs is below the minimum; \
                 not polling this epic until it is corrected"
            );
            return false;
        }
        // The guard above proves `s >= MIN_FEED_INTERVAL_SECS`, which is
        // positive, so this conversion is lossless. `unsigned_abs`
        // rather than the `as u64` it replaces: `as` is what turned a
        // negative into a near-infinite cadence, and if a later edit
        // ever weakened the guard, `as` would silently do that again.
        Some(s) => Duration::from_secs(s.unsigned_abs()),
        None => DEFAULT_FEED_INTERVAL,
    };

    let elapsed = last_run
        .get(&epic.id)
        .map(|t| now.saturating_duration_since(*t))
        .unwrap_or(Duration::MAX);

    elapsed >= interval
}

pub struct FeedRunner {
    db: Arc<dyn TaskStore>,
    notify: mpsc::UnboundedSender<McpEvent>,
    runner: Arc<dyn ProcessRunner>,
    last_run: HashMap<EpicId, Instant>,
    /// Cached result of "does any epic have a feed command?".
    /// `None` means uninitialised or invalidated; `Some(false)` lets `tick()` skip
    /// all DB work when no epic needs polling.
    any_feed_cmds: Option<bool>,
    /// Watch receiver: when the sender fires, `any_feed_cmds` is reset to `None`
    /// so the next `tick()` re-queries.
    epic_changed_rx: tokio::sync::watch::Receiver<()>,
    /// Counterpart of `epic_changed_rx`.  Clone this before calling `start()` to
    /// retain a handle for external invalidation (e.g. on `EpicChanged` events).
    epic_changed_tx: tokio::sync::watch::Sender<()>,
    /// Per-epic single-flight claims, shared with the manual "r" refresh so the
    /// two surfaces serialise against each other. Take a handle with
    /// [`FeedRunner::sync_guard`] — the manual path holding a DIFFERENT
    /// `FeedSyncGuard` type-checks and silently serialises nothing.
    guard: Arc<FeedSyncGuard>,
    /// Test-only join handles for the jobs spawned by `tick`. Production keeps
    /// firing-and-forgetting: the field, and the push that fills it, exist only
    /// under `cfg(test)`. Tests need it because some feed-cycle outcomes
    /// deliberately send no `McpEvent` — the degraded-empty-emission guard
    /// (feeds.allium: DegradedEmptyEmission) returns before any sync — so
    /// awaiting `rx` is not a usable completion signal there, and sleeping is
    /// banned by `./scripts/check-no-test-sleep.sh`.
    #[cfg(test)]
    spawned: Vec<tokio::task::JoinHandle<()>>,
}

impl FeedRunner {
    pub fn new(
        db: Arc<dyn TaskStore>,
        notify: mpsc::UnboundedSender<McpEvent>,
        runner: Arc<dyn ProcessRunner>,
    ) -> Self {
        let (epic_changed_tx, epic_changed_rx) = tokio::sync::watch::channel(());
        Self {
            db,
            notify,
            runner,
            last_run: HashMap::new(),
            any_feed_cmds: None,
            epic_changed_rx,
            epic_changed_tx,
            guard: Arc::new(FeedSyncGuard::default()),
            #[cfg(test)]
            spawned: Vec::new(),
        }
    }

    /// Handle to the per-epic feed-cycle claims, for the manual "r" refresh to
    /// share. Both surfaces MUST hold this same `Arc`: the serialisation is
    /// per-registry, so a second registry silently disables it. Wire it at
    /// construction — see `TuiRuntime`'s `feed_sync_guard`.
    pub(crate) fn sync_guard(&self) -> Arc<FeedSyncGuard> {
        Arc::clone(&self.guard)
    }

    /// Await every job spawned by the ticks run so far, draining the handle
    /// list. Deterministic replacement for "wait for an `McpEvent`" in tests
    /// covering paths that emit no event.
    #[cfg(test)]
    pub(crate) async fn join_spawned_jobs(&mut self) {
        for handle in std::mem::take(&mut self.spawned) {
            let _ = handle.await;
        }
    }

    /// Returns a sender that can be used to invalidate the feed-command cache.
    /// Clone and retain this handle before calling `start()`.
    pub fn epic_invalidate_tx(&self) -> tokio::sync::watch::Sender<()> {
        self.epic_changed_tx.clone()
    }

    /// Inspection accessor for the cached "does any epic have a feed command?"
    /// flag. `Some(false)` means the next `tick()` short-circuits without DB
    /// work; `None` means it will re-query. Used by tests asserting that a
    /// freshly-enabled feed becomes pollable after the cache is invalidated.
    #[cfg(test)]
    pub(crate) fn any_feed_cmds_cache(&self) -> Option<bool> {
        self.any_feed_cmds
    }

    /// Spawns as an independent background task so slow feed commands can't freeze the UI.
    pub fn start(self) {
        tokio::spawn(async move {
            let mut runner = self;
            let mut interval = tokio::time::interval(FEED_POLL_INTERVAL);
            loop {
                interval.tick().await;
                runner.tick().await;
            }
        });
    }

    pub async fn tick(&mut self) {
        // Invalidate the cache if an EpicChanged signal arrived since last tick.
        if self.epic_changed_rx.has_changed().unwrap_or(true) {
            self.epic_changed_rx.borrow_and_update();
            self.any_feed_cmds = None;
        }

        // Skip all DB work when we know no epic has a feed command.
        if self.any_feed_cmds == Some(false) {
            return;
        }

        let epics = match self.db.list_epics().await {
            Ok(e) => e,
            Err(err) => {
                tracing::warn!("FeedRunner: failed to list epics: {err:#}");
                return;
            }
        };

        let active_ids: std::collections::HashSet<EpicId> = epics.iter().map(|e| e.id).collect();
        self.last_run.retain(|id, _| active_ids.contains(id));

        let has_feed_cmd = epics.iter().any(|e| e.feed_command.is_some());
        self.any_feed_cmds = Some(has_feed_cmd);

        if !has_feed_cmd {
            return;
        }

        // Fetch once per tick so N concurrent spawned tasks don't each hit the DB.
        let known_paths = Arc::new(match self.db.list_repo_paths().await {
            Ok(p) => p,
            Err(err) => {
                tracing::warn!(
                    "FeedRunner: failed to list repo_paths, using empty sentinel: {err:#}"
                );
                vec![]
            }
        });

        let now = Instant::now();
        for epic in epics {
            // Scheduling only reads feed_command to decide whether this epic is
            // pollable at all; the command the cycle actually runs is re-read
            // from the epic inside FeedCycle::run, after the claim.
            if epic.feed_command.is_none() {
                continue;
            }

            if !epic_due(&epic, &self.last_run, now) {
                continue;
            }

            self.last_run.insert(epic.id, now);
            self.spawn_epic_cycle(epic.id, epic.title, Arc::clone(&known_paths));
        }
    }

    /// Spawn one epic's feed cycle, so a slow feed command cannot stall the
    /// poll loop. The claim is taken INSIDE the cycle rather than here:
    /// `tick` must not block, so contention is resolved by whichever spawned
    /// cycle reaches try_claim first, and the loser returns Busy.
    fn spawn_epic_cycle(
        &mut self,
        epic_id: EpicId,
        epic_title: String,
        known_paths: Arc<Vec<String>>,
    ) {
        let cycle = cycle::FeedCycle {
            db: self.db.clone(),
            runner: self.runner.clone(),
            guard: Arc::clone(&self.guard),
            epic_id,
            epic_title,
            known_paths: Some(known_paths),
            command_timeout: FEED_COMMAND_TIMEOUT,
        };
        let notify = self.notify.clone();

        let _handle = tokio::task::spawn(async move {
            match cycle.run().await {
                // The cycle has already torn down every removed task's
                // worktree by the time it returns, so these notifications
                // mean "reconciled and cleaned up" (feeds.allium
                // RoleRoutedFeedSync).
                FeedCycleOutcome::Synced { affected_epics, .. } => {
                    for id in affected_epics {
                        let _ = notify.send(McpEvent::EpicChanged(id));
                    }
                }
                // Both already logged by the cycle. The auto-poll path adds
                // no TUI surface, per feeds.allium FeedCommandFailure ("the
                // TUI is NOT notified"); a dropped request is not a failure.
                FeedCycleOutcome::Busy | FeedCycleOutcome::Failed(_) => {}
            }
        });
        #[cfg(test)]
        self.spawned.push(_handle);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use std::sync::Arc;

    use super::*;
    use crate::db::{Database, EpicCrud, EpicPatch, EpicRead, SettingsStore, TaskCrud};
    use crate::models::{test_tmux_window, TaskStatus, TaskTag, MIN_FEED_INTERVAL_SECS};

    use super::exec::AlwaysFailRunner;

    // --- FeedRunner tests ---

    fn make_runner(db: Arc<Database>) -> (FeedRunner, mpsc::UnboundedReceiver<McpEvent>) {
        make_runner_with_runner(db, Arc::new(AlwaysFailRunner))
    }

    fn make_runner_with_runner(
        db: Arc<Database>,
        runner: Arc<dyn ProcessRunner>,
    ) -> (FeedRunner, mpsc::UnboundedReceiver<McpEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (FeedRunner::new(db, tx, runner), rx)
    }

    /// `tick()` hands a slow feed command to a background task; it must not
    /// await the command itself. The bound is an outer `timeout` rather than an
    /// assertion on measured elapsed time — 5s is far above what a handful of
    /// in-memory DB round-trips cost even on a loaded machine, and far below
    /// the 30s a `tick()` that awaited the command inline would take.
    #[tokio::test]
    async fn tick_does_not_block_event_loop() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("Slow Epic", "", None).await.unwrap();
        db.patch_epic(epic.id, &EpicPatch::new().feed_command(Some("sleep 30")))
            .await
            .unwrap();

        let (mut runner, _rx) = make_runner(db.clone());

        tokio::time::timeout(Duration::from_secs(5), runner.tick())
            .await
            .expect("tick() must dispatch the feed command, not await it");
    }

    #[tokio::test]
    async fn tick_background_task_upserts_tasks() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("BG Epic", "", None).await.unwrap();
        db.patch_epic(
            epic.id,
            &EpicPatch::new().feed_command(Some(
                r#"echo '[{"external_id":"bg1","title":"BG","description":"","status":"backlog","tag":"bug"}]'"#,
            )),
        ).await
        .unwrap();

        let (mut runner, mut rx) = make_runner(db.clone());
        runner.tick().await;

        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for McpEvent::Refresh")
            .expect("channel closed");

        let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].title, "BG");
    }

    #[tokio::test]
    async fn tick_done_epic_moves_to_backlog_when_new_feed_tasks_added() {
        // Regression test: a done epic should regress to backlog when the feed
        // adds new non-done tasks, because recalculate_epic_status must be
        // called after upsert_feed_tasks.
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("Done Epic", "", None).await.unwrap();

        // Mark the epic as done before the feed runs.
        db.patch_epic(
            epic.id,
            &EpicPatch::new()
                .status(TaskStatus::Done)
                .feed_command(Some(
                    r#"echo '[{"external_id":"new1","title":"New Task","description":"","status":"backlog","tag":"bug"}]'"#,
                )),
        )
        .await
        .unwrap();

        let (mut runner, mut rx) = make_runner(db.clone());
        runner.tick().await;

        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for McpEvent")
            .expect("channel closed");

        // After the feed adds a new backlog task, the epic must regress to backlog.
        let refreshed = db.get_epic(epic.id).await.unwrap().unwrap();
        assert_eq!(
            refreshed.status,
            TaskStatus::Backlog,
            "done epic with new backlog feed task should regress to backlog"
        );
    }

    #[tokio::test]
    async fn tick_valid_json_upserts_tasks() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("My Epic", "", None).await.unwrap();
        db.patch_epic(
            epic.id,
            &EpicPatch::new().feed_command(Some(
                r#"echo '[{"external_id":"1","title":"T","description":"D","status":"backlog","tag":"bug"}]'"#,
            )),
        ).await
        .unwrap();

        let (mut runner, mut rx) = make_runner(db.clone());
        runner.tick().await;

        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for McpEvent::Refresh")
            .expect("channel closed");

        let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].title, "T");
        assert_eq!(tasks[0].external_id.as_deref(), Some("1"));
    }

    // Regression coverage for feeds.allium: FeedCommandStderrOnSuccess on the
    // auto-poll path — a command that writes to stderr while still exiting 0
    // with a valid item array must sync exactly as if it had written nothing.
    // Only the manual "r" path (src/runtime/tests.rs:3211) had this proven
    // before; this closes the gap for the auto-poll path.
    #[tokio::test]
    async fn tick_stderr_on_zero_exit_does_not_suppress_sync() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("Noisy Epic", "", None).await.unwrap();
        db.patch_epic(
            epic.id,
            &EpicPatch::new().feed_command(Some(
                r#"echo 'Invalid search query' >&2; echo '[{"external_id":"1","title":"T","description":"D","status":"backlog","tag":"bug"}]'"#,
            )),
        )
        .await
        .unwrap();

        let (mut runner, mut rx) = make_runner(db.clone());
        runner.tick().await;

        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for McpEvent::Refresh")
            .expect("channel closed");

        let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
        assert_eq!(
            tasks.len(),
            1,
            "stderr on a zero exit must not suppress the sync"
        );
        assert_eq!(tasks[0].title, "T");
        assert_eq!(tasks[0].external_id.as_deref(), Some("1"));
    }

    // Regression for #3989 (feeds.allium: DegradedEmptyEmission). A command that
    // soft-fails to `[]` while reporting the reason on stderr must NOT reconcile —
    // syncing it would delete every feed task already in the epic.
    #[tokio::test]
    async fn tick_degraded_empty_emission_does_not_delete_existing_tasks() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("Degraded Epic", "", None).await.unwrap();

        // Seed one feed task, as a previous healthy poll would have.
        db.upsert_feed_tasks(
            epic.id,
            &[crate::models::FeedItem {
                external_id: "pr-1".to_string(),
                title: "Existing PR".to_string(),
                description: String::new(),
                url: String::new(),
                url_type: None,
                status: TaskStatus::Backlog,
                tag: TaskTag::PrReview,
                labels: Vec::new(),
                sort_order: None,
                signals: vec![],
                wrap_up_mode: None,
            }],
            &["".to_string()],
            &["main".to_string()],
        )
        .await
        .unwrap();
        assert_eq!(db.list_tasks_for_epic(epic.id).await.unwrap().len(), 1);

        db.patch_epic(
            epic.id,
            &EpicPatch::new().feed_command(Some("echo 'Invalid search query' >&2; echo '[]'")),
        )
        .await
        .unwrap();

        let (mut runner, _rx) = make_runner(db.clone());
        runner.tick().await;
        runner.join_spawned_jobs().await;

        let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
        assert_eq!(
            tasks.len(),
            1,
            "a degraded empty emission must not delete existing feed tasks"
        );
        assert_eq!(tasks[0].external_id.as_deref(), Some("pr-1"));
    }

    /// End-to-end wiring guard for the AUTO-POLL path: a real `FeedRunner::tick`
    /// whose emission drops a task must actually shell out `git worktree remove`
    /// for it.
    ///
    /// This crosses the seam the rest of the suite leaves untested. The ingest
    /// tests prove `FeedSyncOutcome::removed` is populated; the `cleanup_*` tests
    /// call `cleanup_removed_feed_tasks` directly with a hand-built `Vec`.
    /// Neither notices if the cycle stops passing one to the other — before
    /// this test, deleting the fan-out call left the whole suite green. Removing
    /// the helper's `#[allow(dead_code)]` was the compiler's only check on that
    /// wiring, and it is gone now that a caller exists.
    #[tokio::test]
    async fn tick_removed_task_tears_down_its_worktree() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("Reviews", "", None).await.unwrap();

        // Seed one feed task, as a previous healthy poll would have, and give it
        // the on-disk state a dispatched agent would own.
        db.upsert_feed_tasks(
            epic.id,
            &[crate::models::FeedItem {
                external_id: "pr-1".to_string(),
                title: "Merged PR".to_string(),
                description: String::new(),
                url: String::new(),
                url_type: None,
                status: TaskStatus::Backlog,
                tag: TaskTag::PrReview,
                labels: Vec::new(),
                sort_order: None,
                signals: vec![],
                wrap_up_mode: None,
            }],
            &["/repo/a".to_string()],
            &["main".to_string()],
        )
        .await
        .unwrap();
        let task = db.list_tasks_for_epic(epic.id).await.unwrap().remove(0);
        db.patch_task(
            task.id,
            &TaskPatch::new()
                .worktree(Some("/repo/a/.worktrees/7-pr-1"))
                .tmux_window(Some(&test_tmux_window("dispatch:pr-1"))),
        )
        .await
        .unwrap();

        // The PR merged, so this poll's emission no longer carries it. A clean
        // empty emission (no stderr) is a genuine reconcile, not a degraded run.
        db.patch_epic(epic.id, &EpicPatch::new().feed_command(Some("echo '[]'")))
            .await
            .unwrap();

        let proc_runner = Arc::new(MockProcessRunner::new(vec![
            // has_window: list-windows names the window, so the kill proceeds
            MockProcessRunner::ok_with_stdout(b"dispatch:pr-1\n"),
            MockProcessRunner::ok(), // tmux kill-window
            MockProcessRunner::ok(), // git worktree remove
            MockProcessRunner::ok(), // git branch -D (best effort)
        ]));
        let (mut runner, _rx) = make_runner_with_runner(db.clone(), proc_runner.clone());
        runner.tick().await;
        runner.join_spawned_jobs().await;

        assert!(
            db.list_tasks_for_epic(epic.id).await.unwrap().is_empty(),
            "the merged PR's row is gone"
        );

        let calls = proc_runner.flattened_calls();
        assert!(
            calls
                .iter()
                .any(|c| c.contains("worktree remove") && c.contains("/repo/a/.worktrees/7-pr-1")),
            "the auto-poll path must tear the removed task's worktree down, got: {calls:?}"
        );
        assert!(
            calls.iter().any(|c| c.contains("kill-window")),
            "and kill its tmux window, got: {calls:?}"
        );
    }

    /// Regression for #4095 (feeds.allium: DegradedNonEmptyEmission). The
    /// counterpart to `tick_removed_task_tears_down_its_worktree`: the same
    /// emission-drops-a-task shape, but the command reports an error on stderr
    /// while still emitting an item — a PARTIALLY degraded run. The row must
    /// survive AND no teardown may be shelled out for it.
    ///
    /// The `MockProcessRunner` is scripted with no responses at all beyond the
    /// base-branch probe, so any teardown attempt fails loudly rather than
    /// passing silently: this test would be nearly worthless asserting only on
    /// the DB, since the destroyed worktree is the part that cannot be undone.
    #[tokio::test]
    async fn tick_partially_degraded_emission_does_not_delete_or_tear_down() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("Reviews", "", None).await.unwrap();

        // Two feed tasks from a previous healthy poll; `pr-1` carries a live
        // agent's worktree and tmux window.
        let seeded: Vec<crate::models::FeedItem> = ["pr-1", "pr-2"]
            .iter()
            .map(|ext| crate::models::FeedItem {
                external_id: ext.to_string(),
                title: "Seeded".to_string(),
                description: String::new(),
                url: String::new(),
                url_type: None,
                status: TaskStatus::Backlog,
                tag: TaskTag::PrReview,
                labels: Vec::new(),
                sort_order: None,
                signals: vec![],
                wrap_up_mode: None,
            })
            .collect();
        db.upsert_feed_tasks(
            epic.id,
            &seeded,
            &vec!["/repo/a".to_string(); 2],
            &vec!["main".to_string(); 2],
        )
        .await
        .unwrap();

        let live = db
            .list_tasks_for_epic(epic.id)
            .await
            .unwrap()
            .into_iter()
            .find(|t| t.external_id.as_deref() == Some("pr-1"))
            .unwrap();
        db.patch_task(
            live.id,
            &TaskPatch::new()
                .status(TaskStatus::Running)
                .sub_status(crate::models::SubStatus::Active)
                .worktree(Some("/repo/a/.worktrees/7-pr-1"))
                .tmux_window(Some(&test_tmux_window("dispatch:pr-1"))),
        )
        .await
        .unwrap();

        // One sub-query soft-failed: pr-1 is missing from an otherwise valid
        // emission, and the reason is on stderr.
        db.patch_epic(
            epic.id,
            &EpicPatch::new().feed_command(Some(
                r#"echo 'fetch-reviews: gh search prs failed' >&2; echo '[{"external_id":"pr-2","title":"Other","description":"","status":"backlog","tag":"pr-review"}]'"#,
            )),
        )
        .await
        .unwrap();

        let proc_runner = Arc::new(MockProcessRunner::new(vec![]));
        let (mut runner, _rx) = make_runner_with_runner(db.clone(), proc_runner.clone());
        runner.tick().await;
        runner.join_spawned_jobs().await;

        let ids: Vec<String> = db
            .list_tasks_for_epic(epic.id)
            .await
            .unwrap()
            .into_iter()
            .filter_map(|t| t.external_id)
            .collect();
        assert!(
            ids.contains(&"pr-1".to_string()),
            "a task omitted by a partially degraded emission must survive, got {ids:?}"
        );
        assert!(
            ids.contains(&"pr-2".to_string()),
            "the emitted item is still synced — additive, not suppressed"
        );

        let calls = proc_runner.flattened_calls();
        assert!(
            !calls.iter().any(|c| c.contains("worktree remove")),
            "a degraded emission must not force-remove a live agent's worktree, got: {calls:?}"
        );
        assert!(
            !calls.iter().any(|c| c.contains("kill-window")),
            "nor kill its tmux window, got: {calls:?}"
        );
    }

    #[tokio::test]
    async fn tick_persists_feed_tag() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("Tagged Epic", "", None).await.unwrap();
        db.patch_epic(
            epic.id,
            &EpicPatch::new().feed_command(Some(
                r#"echo '[{"external_id":"1","title":"T","description":"","status":"backlog","tag":"pr-review"}]'"#,
            )),
        ).await
        .unwrap();

        let (mut runner, mut rx) = make_runner(db.clone());
        runner.tick().await;

        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for McpEvent::Refresh")
            .expect("channel closed");

        let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].tag, Some(TaskTag::PrReview));
    }

    #[tokio::test]
    async fn tick_missing_tag_rejects_item() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("Untagged Epic", "", None).await.unwrap();
        db.patch_epic(
            epic.id,
            &EpicPatch::new().feed_command(Some(
                r#"echo '[{"external_id":"1","title":"T","description":"","status":"backlog"}]'"#,
            )),
        )
        .await
        .unwrap();

        let (mut runner, mut rx) = make_runner(db.clone());
        runner.tick().await;

        // Parse must fail and no Refresh is sent.
        let result = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await;
        assert!(
            result.is_err(),
            "expected no notification when tag is missing"
        );

        let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
        assert!(tasks.is_empty(), "no task should be inserted on parse fail");
    }

    #[tokio::test]
    async fn tick_nonzero_exit_no_panic() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("Err Epic", "", None).await.unwrap();
        db.patch_epic(epic.id, &EpicPatch::new().feed_command(Some("exit 1")))
            .await
            .unwrap();

        let (mut runner, mut rx) = make_runner(db.clone());
        runner.tick().await; // must not panic

        // No Refresh is sent on failure — expect timeout
        let result = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await;
        assert!(result.is_err(), "expected timeout but got a notification");

        let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
        assert!(tasks.is_empty());
    }

    #[tokio::test]
    async fn tick_malformed_json_no_panic() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("Bad JSON Epic", "", None).await.unwrap();
        db.patch_epic(
            epic.id,
            &EpicPatch::new().feed_command(Some("echo 'not-json'")),
        )
        .await
        .unwrap();

        let (mut runner, mut rx) = make_runner(db.clone());
        runner.tick().await; // must not panic

        let result = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await;
        assert!(result.is_err(), "expected timeout but got a notification");

        let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
        assert!(tasks.is_empty());
    }

    #[tokio::test]
    async fn tick_interval_not_elapsed_skips_command() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("Interval Epic", "", None).await.unwrap();

        // Write a counter to a temp file so we can count how many times the command ran.
        let tmp = std::env::temp_dir().join(format!("feed_test_{}", epic.id.0));
        let cmd = format!(
            r#"echo 0 >> {path}; echo '[{{"external_id":"1","title":"T","description":"","status":"backlog","tag":"bug"}}]'"#,
            path = tmp.display()
        );
        db.patch_epic(
            epic.id,
            &EpicPatch::new()
                .feed_command(Some(&cmd))
                .feed_interval_secs(Some(10000)),
        )
        .await
        .unwrap();

        let (mut runner, mut rx) = make_runner(db.clone());
        // First tick: command runs, counter file gets one line.
        runner.tick().await;
        // Wait for the background task to finish before checking interval logic.
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for first tick refresh")
            .expect("channel closed");
        // Second tick immediately: interval (10000s) not elapsed, command must not run again.
        runner.tick().await;

        let content = std::fs::read_to_string(&tmp).unwrap_or_default();
        let lines: Vec<_> = content.lines().collect();
        assert_eq!(
            lines.len(),
            1,
            "command ran {count} times, expected 1",
            count = lines.len()
        );

        let _ = std::fs::remove_file(&tmp);
    }

    // --- the feed-cadence floor (feeds.allium: FeedTick) ---

    /// Make an epic due on the next `tick()` without touching its interval and
    /// without sleeping: dropping its `last_run` entry makes `elapsed` read as
    /// `Duration::MAX`, which beats any interval.
    ///
    /// This is the lever for "run the command again immediately". Setting the
    /// interval to 0 used to serve that purpose, but a sub-floor interval is
    /// now refused at read (`FeedTick`), so an interval of 0 tests the refusal
    /// rather than the re-run.
    fn force_due(runner: &mut FeedRunner, epic_id: EpicId) {
        runner.last_run.remove(&epic_id);
    }

    /// The default an unset interval inherits must itself clear the floor —
    /// otherwise a blank field polls faster than the fastest value a user is
    /// permitted to enter. (feeds.allium: DefaultFeedIntervalClearsTheFloor)
    #[test]
    fn the_default_interval_clears_the_floor() {
        assert!(
            DEFAULT_FEED_INTERVAL >= Duration::from_secs(MIN_FEED_INTERVAL_SECS as u64),
            "DEFAULT_FEED_INTERVAL ({DEFAULT_FEED_INTERVAL:?}) must not be below the floor \
             of {MIN_FEED_INTERVAL_SECS}s"
        );
    }

    // --- epic_due: the extracted per-epic predicate ---

    fn cadence_test_epic(interval_secs: Option<i64>) -> crate::models::Epic {
        crate::models::Epic {
            id: EpicId(1),
            title: "Cadence Test".to_string(),
            description: String::new(),
            status: TaskStatus::Backlog,
            plan_path: None,
            sort_order: None,
            auto_dispatch: false,
            parent_epic_id: None,
            feed_command: Some("echo hi".to_string()),
            feed_interval_secs: interval_secs,
            group_by_repo: false,
            feed_role: crate::models::FeedRole::None,
            origin: crate::models::EpicOrigin::Manual,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn epic_due_is_true_when_never_run() {
        let epic = cadence_test_epic(Some(MIN_FEED_INTERVAL_SECS));
        let last_run = HashMap::new();
        assert!(epic_due(&epic, &last_run, Instant::now()));
    }

    #[test]
    fn epic_due_is_false_before_the_interval_elapses() {
        let epic = cadence_test_epic(Some(3600));
        let mut last_run = HashMap::new();
        let now = Instant::now();
        last_run.insert(epic.id, now);
        assert!(!epic_due(&epic, &last_run, now));
    }

    #[test]
    fn epic_due_is_true_once_the_interval_elapses() {
        let epic = cadence_test_epic(Some(MIN_FEED_INTERVAL_SECS));
        let mut last_run = HashMap::new();
        let started = Instant::now();
        last_run.insert(epic.id, started);
        let now = started + Duration::from_secs(MIN_FEED_INTERVAL_SECS as u64);
        assert!(epic_due(&epic, &last_run, now));
    }

    #[test]
    fn epic_due_rejects_an_interval_below_the_floor() {
        let epic = cadence_test_epic(Some(MIN_FEED_INTERVAL_SECS - 1));
        let last_run = HashMap::new();
        assert!(!epic_due(&epic, &last_run, Instant::now()));
    }

    #[test]
    fn epic_due_rejects_a_negative_interval_rather_than_wrapping() {
        let epic = cadence_test_epic(Some(-5));
        let last_run = HashMap::new();
        assert!(!epic_due(&epic, &last_run, Instant::now()));
    }

    /// Write-time validation is what normally keeps a sub-floor row from
    /// existing, so this test writes one via `patch_epic` — the same bypass a
    /// hand-edited database represents. The epic must not be polled at all:
    /// clamping would run it at a cadence nobody chose while looking healthy.
    #[tokio::test]
    async fn tick_skips_an_epic_whose_stored_interval_is_below_the_floor() {
        for bad in [0, MIN_FEED_INTERVAL_SECS - 1] {
            let db = Arc::new(Database::open_in_memory().await.unwrap());
            let epic = db.create_epic("Too Fast", "", None).await.unwrap();
            db.patch_epic(
                epic.id,
                &EpicPatch::new()
                    .feed_command(Some(
                        r#"echo '[{"external_id":"1","title":"T","description":"","status":"backlog","tag":"bug"}]'"#,
                    ))
                    .feed_interval_secs(Some(bad)),
            )
            .await
            .unwrap();

            let (mut runner, mut rx) = make_runner(db.clone());
            runner.tick().await;
            runner.join_spawned_jobs().await;

            assert!(
                db.list_tasks_for_epic(epic.id).await.unwrap().is_empty(),
                "interval {bad} is below the floor, so the command must not have run"
            );
            assert!(
                tokio::time::timeout(Duration::from_millis(200), rx.recv())
                    .await
                    .is_err(),
                "a skipped epic must not emit a refresh"
            );
        }
    }

    /// A negative interval failed differently and worse than a zero one: `as
    /// u64` wrapped it into an effectively infinite cadence, so the feed went
    /// permanently silent with nothing logged. It now takes the same visible
    /// skip path as any other sub-floor value.
    #[tokio::test]
    async fn tick_skips_an_epic_with_a_negative_interval_rather_than_wrapping() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("Negative", "", None).await.unwrap();
        db.patch_epic(
            epic.id,
            &EpicPatch::new()
                .feed_command(Some(
                    r#"echo '[{"external_id":"1","title":"T","description":"","status":"backlog","tag":"bug"}]'"#,
                ))
                .feed_interval_secs(Some(-5)),
        )
        .await
        .unwrap();

        let (mut runner, mut rx) = make_runner(db.clone());
        runner.tick().await;
        runner.join_spawned_jobs().await;

        assert!(
            db.list_tasks_for_epic(epic.id).await.unwrap().is_empty(),
            "a negative interval must skip, not wrap into a huge one"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(200), rx.recv())
                .await
                .is_err(),
            "a skipped epic must not emit a refresh"
        );
    }

    /// The boundary from the other side: the floor itself is a legal cadence,
    /// so an epic set exactly there polls normally.
    #[tokio::test]
    async fn tick_polls_an_epic_whose_interval_is_exactly_the_floor() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("At Floor", "", None).await.unwrap();
        db.patch_epic(
            epic.id,
            &EpicPatch::new()
                .feed_command(Some(
                    r#"echo '[{"external_id":"1","title":"T","description":"","status":"backlog","tag":"bug"}]'"#,
                ))
                .feed_interval_secs(Some(MIN_FEED_INTERVAL_SECS)),
        )
        .await
        .unwrap();

        let (mut runner, mut rx) = make_runner(db.clone());
        runner.tick().await;
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for the refresh")
            .expect("channel closed");
        runner.join_spawned_jobs().await;

        assert_eq!(
            db.list_tasks_for_epic(epic.id).await.unwrap().len(),
            1,
            "an interval at the floor is legal and must poll"
        );
    }

    #[tokio::test]
    async fn tick_null_feed_command_skipped() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        // Epic with no feed_command (default)
        let epic = db.create_epic("Plain Epic", "", None).await.unwrap();

        let (mut runner, mut rx) = make_runner(db.clone());
        runner.tick().await;

        // No background task spawned — channel stays empty
        let result = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await;
        assert!(
            result.is_err(),
            "expected empty channel but got notification"
        );

        let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
        assert!(tasks.is_empty());
    }

    // --- group_by_repo feed grouping tests ---

    #[tokio::test]
    async fn tick_grouped_creates_sub_epics_per_repo() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("Dependabot", "", None).await.unwrap();
        db.patch_epic(
            epic.id,
            &EpicPatch::new()
                .feed_command(Some(
                    r#"echo '[
                        {"external_id":"1","title":"A","description":"","url":"https://github.com/org/repo-a/pull/1","status":"backlog","tag":"pr-review"},
                        {"external_id":"2","title":"B","description":"","url":"https://github.com/org/repo-b/pull/1","status":"backlog","tag":"pr-review"}
                    ]'"#,
                ))
                .group_by_repo(true),
        )
        .await
        .unwrap();

        let (mut runner, mut rx) = make_runner(db.clone());
        runner.tick().await;
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out")
            .expect("channel closed");

        let sub_epics = db.list_sub_epics(epic.id).await.unwrap();
        assert_eq!(sub_epics.len(), 2);
        let names: Vec<&str> = sub_epics.iter().map(|e| e.title.as_str()).collect();
        assert!(
            names.contains(&"repo-a"),
            "expected repo-a sub-epic, got {names:?}"
        );
        assert!(
            names.contains(&"repo-b"),
            "expected repo-b sub-epic, got {names:?}"
        );

        for sub in &sub_epics {
            let tasks = db.list_tasks_for_epic(sub.id).await.unwrap();
            assert_eq!(tasks.len(), 1, "sub-epic {} should have 1 task", sub.title);
        }

        let parent_tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
        assert_eq!(parent_tasks.len(), 0, "parent should have no direct tasks");
    }

    #[tokio::test]
    async fn tick_done_epic_grouped_moves_to_backlog_when_new_feed_tasks_added() {
        // Grouped feed variant: a done parent epic should regress to backlog when
        // the feed adds new backlog tasks into a sub-epic.
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("Done Grouped Epic", "", None).await.unwrap();

        // Mark the parent epic as done before the feed runs.
        db.patch_epic(
            epic.id,
            &EpicPatch::new()
                .status(TaskStatus::Done)
                .feed_command(Some(
                    r#"echo '[{"external_id":"g1","title":"G Task","description":"","url":"https://github.com/org/repo-a/pull/1","status":"backlog","tag":"pr-review"}]'"#,
                ))
                .group_by_repo(true),
        )
        .await
        .unwrap();

        let (mut runner, mut rx) = make_runner(db.clone());
        runner.tick().await;

        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for McpEvent")
            .expect("channel closed");

        // After the feed adds a new backlog task into a sub-epic, the parent
        // epic must regress to backlog.
        let refreshed = db.get_epic(epic.id).await.unwrap().unwrap();
        assert_eq!(
            refreshed.status,
            TaskStatus::Backlog,
            "done parent epic with new grouped feed task should regress to backlog"
        );
    }

    #[tokio::test]
    async fn tick_grouped_migrates_existing_flat_tasks() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("Dependabot", "", None).await.unwrap();
        // First run: flat (group_by_repo = false by default)
        db.patch_epic(
            epic.id,
            &EpicPatch::new().feed_command(Some(
                r#"echo '[{"external_id":"1","title":"A","description":"","url":"https://github.com/org/repo-a/pull/1","status":"backlog","tag":"pr-review"}]'"#,
            )),
        )
        .await
        .unwrap();
        let (mut runner, mut rx) = make_runner(db.clone());
        runner.tick().await;
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out")
            .expect("closed");

        let flat_tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
        assert_eq!(
            flat_tasks.len(),
            1,
            "flat task should exist before migration"
        );

        // Enable group_by_repo and run again
        db.patch_epic(epic.id, &EpicPatch::new().group_by_repo(true))
            .await
            .unwrap();
        let (mut runner2, mut rx2) = make_runner(db.clone());
        runner2.tick().await;
        tokio::time::timeout(Duration::from_secs(5), rx2.recv())
            .await
            .expect("timed out")
            .expect("closed");

        let parent_tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
        assert_eq!(parent_tasks.len(), 0, "flat task should have migrated");

        let sub_epics = db.list_sub_epics(epic.id).await.unwrap();
        assert_eq!(sub_epics.len(), 1);
        assert_eq!(sub_epics[0].title, "repo-a");
        let sub_tasks = db.list_tasks_for_epic(sub_epics[0].id).await.unwrap();
        assert_eq!(sub_tasks.len(), 1);
    }

    #[tokio::test]
    async fn tick_grouped_uses_other_for_no_url() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("Feed", "", None).await.unwrap();
        db.patch_epic(
            epic.id,
            &EpicPatch::new()
                .feed_command(Some(
                    r#"echo '[{"external_id":"1","title":"X","description":"","status":"backlog","tag":"bug"}]'"#,
                ))
                .group_by_repo(true),
        )
        .await
        .unwrap();

        let (mut runner, mut rx) = make_runner(db.clone());
        runner.tick().await;
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out")
            .expect("closed");

        let sub_epics = db.list_sub_epics(epic.id).await.unwrap();
        assert_eq!(sub_epics.len(), 1);
        assert_eq!(sub_epics[0].title, "other");
    }

    #[tokio::test]
    async fn tick_grouped_creates_fresh_sub_epic_when_existing_one_is_archived() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let feed_cmd = r#"echo '[{"external_id":"1","title":"A","description":"","url":"https://github.com/org/repo-a/pull/1","status":"backlog","tag":"pr-review"}]'"#;

        let epic = db.create_epic("Reviews", "", None).await.unwrap();
        db.patch_epic(
            epic.id,
            &EpicPatch::new()
                .feed_command(Some(feed_cmd))
                .group_by_repo(true),
        )
        .await
        .unwrap();

        // First run: creates sub-epic for repo-a
        let (mut runner, mut rx) = make_runner(db.clone());
        runner.tick().await;
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out")
            .expect("channel closed");

        let sub_epics = db.list_sub_epics(epic.id).await.unwrap();
        assert_eq!(sub_epics.len(), 1);
        let archived_id = sub_epics[0].id;

        // User archives the sub-epic
        db.patch_epic(
            archived_id,
            &EpicPatch::new().status(crate::models::TaskStatus::Archived),
        )
        .await
        .unwrap();

        // Second run: must create a NEW active sub-epic, not reuse the archived one
        let (mut runner2, mut rx2) = make_runner(db.clone());
        runner2.tick().await;
        tokio::time::timeout(Duration::from_secs(5), rx2.recv())
            .await
            .expect("timed out")
            .expect("channel closed");

        let all_sub_epics = db.list_sub_epics(epic.id).await.unwrap();
        let active: Vec<_> = all_sub_epics
            .iter()
            .filter(|e| e.status != crate::models::TaskStatus::Archived)
            .collect();
        assert_eq!(
            active.len(),
            1,
            "expected a fresh active sub-epic after archiving; got sub-epics: {:?}",
            all_sub_epics
                .iter()
                .map(|e| (&e.title, &e.status))
                .collect::<Vec<_>>()
        );
        assert_eq!(active[0].title, "repo-a");
        assert_ne!(
            active[0].id, archived_id,
            "must be a new sub-epic, not the archived one"
        );
        let tasks = db.list_tasks_for_epic(active[0].id).await.unwrap();
        assert_eq!(tasks.len(), 1, "new sub-epic should have the feed task");
    }

    // --- reviews_parent role routing (WP3) ---

    /// Drain all pending `EpicChanged` events, returning once the channel has
    /// been quiet for the timeout window. Used by routing tests to wait for the
    /// spawned reconcile(s) to finish without `tokio::time::sleep`.
    async fn drain_events(rx: &mut mpsc::UnboundedReceiver<McpEvent>) {
        while tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .is_ok_and(|m| m.is_some())
        {}
    }

    fn role_sub(
        subs: &[crate::models::Epic],
        role: crate::models::FeedRole,
    ) -> &crate::models::Epic {
        subs.iter()
            .find(|e| e.feed_role == role)
            .unwrap_or_else(|| panic!("missing {role:?} sub-epic in {subs:?}"))
    }

    #[tokio::test]
    async fn tick_routes_reviews_parent_into_role_sub_epics() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let parent = db.create_epic("Reviews", "", None).await.unwrap();
        db.patch_epic(
            parent.id,
            &EpicPatch::new()
                .feed_role(crate::models::FeedRole::ReviewsParent)
                .feed_command(Some(
                    r#"echo '[
                        {"external_id":"pr-1","title":"Direct","description":"","url":"https://github.com/org/repo/pull/1","status":"backlog","tag":"pr-review","signals":["direct-request"]},
                        {"external_id":"pr-2","title":"Team","description":"","url":"https://github.com/org/repo/pull/2","status":"backlog","tag":"pr-review","signals":["team-request"]}
                    ]'"#,
                )),
        )
        .await
        .unwrap();

        let (mut runner, mut rx) = make_runner(db.clone());
        runner.tick().await;
        drain_events(&mut rx).await;

        let subs = db.list_sub_epics(parent.id).await.unwrap();
        let my = role_sub(&subs, crate::models::FeedRole::MyReviews);
        let team = role_sub(&subs, crate::models::FeedRole::TeamReviews);
        let bots = role_sub(&subs, crate::models::FeedRole::Bots);

        let my_tasks = db.list_tasks_for_epic(my.id).await.unwrap();
        assert_eq!(my_tasks.len(), 1, "direct-request PR routes to My Reviews");
        assert_eq!(my_tasks[0].external_id.as_deref(), Some("pr-1"));

        let team_tasks = db.list_tasks_for_epic(team.id).await.unwrap();
        assert_eq!(
            team_tasks.len(),
            1,
            "team-request PR routes to Team Reviews"
        );
        assert_eq!(team_tasks[0].external_id.as_deref(), Some("pr-2"));

        assert!(db.list_tasks_for_epic(bots.id).await.unwrap().is_empty());
        assert!(
            db.list_tasks_for_epic(parent.id).await.unwrap().is_empty(),
            "parent holds no direct feed tasks"
        );
    }

    /// feeds.allium SerialisedFeedCycle: a tick for an epic whose cycle is
    /// already in flight is DROPPED — it must not exec, sync, or notify.
    ///
    /// The claim is taken here in the test, which is what makes this
    /// deterministic: no second cycle has to be raced into existence.
    #[tokio::test]
    async fn tick_skips_an_epic_whose_cycle_is_already_in_flight() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("Busy Epic", "", None).await.unwrap();
        // The command would insert a task if it ever ran. It must not run.
        db.patch_epic(
            epic.id,
            &EpicPatch::new().feed_command(Some(
                r#"echo '[{"external_id":"1","title":"T","description":"","status":"backlog","tag":"bug"}]'"#,
            )),
        )
        .await
        .unwrap();

        let (mut runner, mut rx) = make_runner(db.clone());

        // Stand in for a cycle already running for this epic.
        let _claim = runner
            .sync_guard()
            .try_claim(epic.id)
            .expect("the epic starts unclaimed");

        runner.tick().await;
        runner.join_spawned_jobs().await;

        assert!(
            tokio::time::timeout(Duration::from_millis(200), rx.recv())
                .await
                .is_err(),
            "a dropped tick must send no notification"
        );
        assert!(
            db.list_tasks_for_epic(epic.id).await.unwrap().is_empty(),
            "a dropped tick must not run the feed command or write anything"
        );
    }

    /// The other half of the claim's contract: once the in-flight cycle ends,
    /// the epic polls normally again. Without this, a guard that never released
    /// would pass the test above and silently kill the feed.
    #[tokio::test]
    async fn tick_resumes_after_the_in_flight_cycle_releases() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("Busy Epic", "", None).await.unwrap();
        db.patch_epic(
            epic.id,
            &EpicPatch::new()
                .feed_command(Some(
                    r#"echo '[{"external_id":"1","title":"T","description":"","status":"backlog","tag":"bug"}]'"#,
                )),
        )
        .await
        .unwrap();

        let (mut runner, mut rx) = make_runner(db.clone());

        let claim = runner
            .sync_guard()
            .try_claim(epic.id)
            .expect("the epic starts unclaimed");
        runner.tick().await;
        runner.join_spawned_jobs().await;
        assert!(
            db.list_tasks_for_epic(epic.id).await.unwrap().is_empty(),
            "precondition: the first tick was dropped"
        );

        drop(claim);

        // The dropped tick still bumped last_run, so clear it to make the epic
        // eligible again immediately.
        force_due(&mut runner, epic.id);
        runner.tick().await;
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for the resumed poll")
            .expect("channel closed");
        assert_eq!(
            db.list_tasks_for_epic(epic.id).await.unwrap().len(),
            1,
            "releasing the claim must let the epic poll again"
        );
    }

    /// B3 concurrency: two back-to-back ticks must not drop the task to a
    /// move/delete interleave.
    ///
    /// Still race-free under SerialisedFeedCycle, and worth stating why: the
    /// claim is taken INSIDE the spawned job, not synchronously in `tick()`, so
    /// `tick` never blocks and never observes contention itself. Whichever of
    /// the two spawned jobs reaches `try_claim` first wins and the loser is
    /// dropped. The outcome is order-dependent; this assertion is not, which is
    /// why it must not be rewritten to expect one specific arm.
    #[tokio::test]
    async fn tick_two_ticks_lose_nothing() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let parent = db.create_epic("Reviews", "", None).await.unwrap();
        db.patch_epic(
            parent.id,
            &EpicPatch::new()
                .feed_role(crate::models::FeedRole::ReviewsParent)
                .feed_command(Some(
                    r#"echo '[{"external_id":"pr-1","title":"Team","description":"","url":"https://github.com/org/repo/pull/1","status":"backlog","tag":"pr-review","signals":["team-request"]}]'"#,
                )),
        )
        .await
        .unwrap();

        let (mut runner, mut rx) = make_runner(db.clone());
        // Both ticks must run the feed and spawn a reconcile, so clear last_run
        // between them to make the epic eligible again immediately.
        runner.tick().await;
        force_due(&mut runner, parent.id);
        runner.tick().await;
        drain_events(&mut rx).await;

        let subs = db.list_sub_epics(parent.id).await.unwrap();
        let team = role_sub(&subs, crate::models::FeedRole::TeamReviews);
        let team_tasks = db.list_tasks_for_epic(team.id).await.unwrap();
        assert_eq!(
            team_tasks.len(),
            1,
            "the PR must survive two reconciles, exactly once"
        );
        assert_eq!(team_tasks[0].external_id.as_deref(), Some("pr-1"));

        // No duplicate or orphaned feed task anywhere in the subtree.
        let total_feed: usize = {
            let mut n = 0;
            for s in &subs {
                n += db
                    .list_tasks_for_epic(s.id)
                    .await
                    .unwrap()
                    .iter()
                    .filter(|t| t.external_id.is_some())
                    .count();
            }
            n
        };
        assert_eq!(total_feed, 1, "exactly one feed task across the subtree");
    }

    /// The loop `start()` spawns really does poll and run the feed command.
    ///
    /// This is deliberately the whole of `start()`'s coverage: `start()` is a
    /// synchronous `fn` whose body is a bare `tokio::spawn`, so "does not await
    /// the poll loop" is a type-level property with no runtime signal to assert
    /// on. See "No `tokio::time::sleep` in tests" in docs/conventions.md.
    #[tokio::test]
    async fn start_background_task_eventually_runs_feed_command() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("BG Feed Epic", "", None).await.unwrap();
        db.patch_epic(
            epic.id,
            &EpicPatch::new().feed_command(Some(
                r#"echo '[{"external_id":"bg1","title":"BG Task","description":"","status":"backlog","tag":"bug"}]'"#,
            )),
        ).await
        .unwrap();

        let (tx, mut rx) = mpsc::unbounded_channel();
        let proc_runner: Arc<dyn ProcessRunner> =
            Arc::new(crate::process::MockProcessRunner::new(vec![]));
        let runner = FeedRunner::new(
            Arc::clone(&db) as Arc<dyn crate::db::TaskStore>,
            tx,
            proc_runner,
        );
        runner.start();

        // The tokio interval fires on the first tick almost immediately; await
        // the EpicChanged event the background task emits after upserting.
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for McpEvent")
            .expect("channel closed");

        let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
        assert_eq!(
            tasks.len(),
            1,
            "background task should have upserted one feed task"
        );
        assert_eq!(tasks[0].title, "BG Task");
        assert_eq!(tasks[0].external_id.as_deref(), Some("bg1"));
    }

    // --- repo_path resolution via URL ---

    #[tokio::test]
    async fn tick_github_url_resolves_to_known_repo_path() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        // Register a known repo path matching "myrepo"
        db.save_repo_path("/home/user/code/myrepo").await.unwrap();
        let epic = db.create_epic("Feed Epic", "", None).await.unwrap();
        let cmd = r#"echo '[{"external_id":"1","title":"T","description":"","url":"https://github.com/org/myrepo/pull/42","status":"backlog","tag":"bug"}]'"#;
        db.patch_epic(epic.id, &EpicPatch::new().feed_command(Some(cmd)))
            .await
            .unwrap();

        let (mut runner, mut rx) = make_runner(db.clone());
        runner.tick().await;

        // Await the background upsert deterministically.
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for McpEvent")
            .expect("channel closed");

        let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(
            tasks[0].repo_path, "/home/user/code/myrepo",
            "repo_path should be resolved from GitHub URL"
        );
    }

    #[tokio::test]
    async fn tick_no_matching_repo_stores_empty_sentinel() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        // Known repo is "other-repo", not matching "myrepo"
        db.save_repo_path("/home/user/code/other-repo")
            .await
            .unwrap();
        let epic = db.create_epic("Feed Epic", "", None).await.unwrap();
        let cmd = r#"echo '[{"external_id":"1","title":"T","description":"","url":"https://github.com/org/myrepo/pull/42","status":"backlog","tag":"bug"}]'"#;
        db.patch_epic(epic.id, &EpicPatch::new().feed_command(Some(cmd)))
            .await
            .unwrap();

        let (mut runner, mut rx) = make_runner(db.clone());
        runner.tick().await;

        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for McpEvent")
            .expect("channel closed");

        let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(
            tasks[0].repo_path, "",
            "unresolved URL should store empty sentinel"
        );
    }

    #[tokio::test]
    async fn tick_empty_url_stores_empty_sentinel() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        db.save_repo_path("/home/user/code/myrepo").await.unwrap();
        let epic = db.create_epic("Feed Epic", "", None).await.unwrap();
        let cmd = r#"echo '[{"external_id":"1","title":"T","description":"","status":"backlog","tag":"bug"}]'"#;
        db.patch_epic(epic.id, &EpicPatch::new().feed_command(Some(cmd)))
            .await
            .unwrap();

        let (mut runner, mut rx) = make_runner(db.clone());
        runner.tick().await;

        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for McpEvent")
            .expect("channel closed");

        let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(
            tasks[0].repo_path, "",
            "empty url should store empty sentinel"
        );
    }

    /// A `ProcessRunner` that returns a fixed `origin/HEAD` per repo path,
    /// counting how many times each path was queried.
    struct PerRepoBranchRunner {
        branches: HashMap<String, String>,
        calls: std::sync::Mutex<HashMap<String, usize>>,
    }

    impl PerRepoBranchRunner {
        fn new(pairs: &[(&str, &str)]) -> Self {
            Self {
                branches: pairs
                    .iter()
                    .map(|(p, b)| (p.to_string(), b.to_string()))
                    .collect(),
                calls: std::sync::Mutex::new(HashMap::new()),
            }
        }

        fn calls_for(&self, path: &str) -> usize {
            self.calls
                .lock()
                .expect("feed lock poisoned")
                .get(path)
                .copied()
                .unwrap_or(0)
        }
    }

    impl ProcessRunner for PerRepoBranchRunner {
        fn run(&self, program: &str, args: &[&str]) -> anyhow::Result<std::process::Output> {
            assert_eq!(program, "git");
            // args = ["-C", <path>, "symbolic-ref", "refs/remotes/origin/HEAD"]
            let path = args.get(1).copied().unwrap_or("");
            *self
                .calls
                .lock()
                .unwrap()
                .entry(path.to_string())
                .or_insert(0) += 1;
            match self.branches.get(path) {
                Some(branch) => crate::process::MockProcessRunner::ok_with_stdout(
                    format!("refs/remotes/origin/{branch}\n").as_bytes(),
                ),
                None => crate::process::MockProcessRunner::fail("unknown repo"),
            }
        }
    }

    #[tokio::test]
    async fn tick_resolves_default_branch_per_unique_repo() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        db.save_repo_path("/home/user/code/repo-a").await.unwrap();
        db.save_repo_path("/home/user/code/repo-b").await.unwrap();
        let epic = db.create_epic("Feed Epic", "", None).await.unwrap();
        // Three items: two for repo-a (master), one for repo-b (develop).
        let cmd = r#"echo '[
            {"external_id":"1","title":"A1","description":"","url":"https://github.com/org/repo-a/pull/1","status":"backlog","tag":"bug"},
            {"external_id":"2","title":"A2","description":"","url":"https://github.com/org/repo-a/pull/2","status":"backlog","tag":"bug"},
            {"external_id":"3","title":"B1","description":"","url":"https://github.com/org/repo-b/pull/1","status":"backlog","tag":"bug"}
        ]'"#;
        db.patch_epic(epic.id, &EpicPatch::new().feed_command(Some(cmd)))
            .await
            .unwrap();

        let proc_runner = Arc::new(PerRepoBranchRunner::new(&[
            ("/home/user/code/repo-a", "master"),
            ("/home/user/code/repo-b", "develop"),
        ]));
        let (mut runner, mut rx) = make_runner_with_runner(db.clone(), proc_runner.clone());
        runner.tick().await;

        // Await the spawned task finishing its writes deterministically.
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for McpEvent")
            .expect("channel closed");

        let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
        assert_eq!(tasks.len(), 3);

        let by_ext = |ext: &str| {
            tasks
                .iter()
                .find(|t| t.external_id.as_deref() == Some(ext))
                .unwrap()
        };
        assert_eq!(by_ext("1").base_branch, "master");
        assert_eq!(by_ext("2").base_branch, "master");
        assert_eq!(by_ext("3").base_branch, "develop");

        // Cache check: each unique repo should have been queried exactly once.
        assert_eq!(
            proc_runner.calls_for("/home/user/code/repo-a"),
            1,
            "repo-a default branch should be resolved once, not per-item"
        );
        assert_eq!(proc_runner.calls_for("/home/user/code/repo-b"), 1);
    }

    #[tokio::test]
    async fn tick_falls_back_to_main_when_origin_head_missing() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        db.save_repo_path("/home/user/code/repo-a").await.unwrap();
        let epic = db.create_epic("Feed Epic", "", None).await.unwrap();
        let cmd = r#"echo '[{"external_id":"1","title":"T","description":"","url":"https://github.com/org/repo-a/pull/1","status":"backlog","tag":"bug"}]'"#;
        db.patch_epic(epic.id, &EpicPatch::new().feed_command(Some(cmd)))
            .await
            .unwrap();

        // AlwaysFailRunner → detect_default_branch returns "main".
        let (mut runner, mut rx) = make_runner_with_runner(db.clone(), Arc::new(AlwaysFailRunner));
        runner.tick().await;
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for McpEvent")
            .expect("channel closed");

        let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].base_branch, "main");
    }

    #[tokio::test]
    async fn tick_twice_is_idempotent() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("Idem Epic", "", None).await.unwrap();
        db.patch_epic(
            epic.id,
            &EpicPatch::new()
                .feed_command(Some(
                    r#"echo '[{"external_id":"1","title":"T","description":"","status":"backlog","tag":"bug"}]'"#,
                )),
        ).await
        .unwrap();

        let (mut runner, mut rx) = make_runner(db.clone());

        runner.tick().await;
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("first tick: timed out waiting for refresh")
            .expect("channel closed");

        let first = db.list_tasks_for_epic(epic.id).await.unwrap();
        assert_eq!(first.len(), 1);
        let first_id = first[0].id;

        // Clear last_run so the second tick re-runs the command.
        force_due(&mut runner, epic.id);
        runner.tick().await;
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("second tick: timed out waiting for refresh")
            .expect("channel closed");

        let second = db.list_tasks_for_epic(epic.id).await.unwrap();
        assert_eq!(
            second.len(),
            1,
            "running the same feed twice must not duplicate tasks"
        );
        assert_eq!(
            second[0].id, first_id,
            "task id must be stable across upserts"
        );
        assert_eq!(second[0].external_id.as_deref(), Some("1"));
    }

    #[tokio::test]
    async fn tick_empty_array_creates_no_tasks() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("Empty Epic", "", None).await.unwrap();
        db.patch_epic(epic.id, &EpicPatch::new().feed_command(Some("echo '[]'")))
            .await
            .unwrap();

        let (mut runner, mut rx) = make_runner(db.clone());
        runner.tick().await;

        let _ = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await;

        let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
        assert!(tasks.is_empty(), "empty feed array must not create tasks");
    }

    // --- cache / EpicChanged invalidation tests ---

    #[tokio::test]
    async fn tick_sets_cache_to_false_when_no_feed_commands() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        db.create_epic("Plain Epic", "", None).await.unwrap();

        let (mut runner, _rx) = make_runner(db.clone());
        runner.tick().await;

        assert_eq!(
            runner.any_feed_cmds,
            Some(false),
            "cache should be Some(false) after tick with no feed commands"
        );
    }

    #[tokio::test]
    async fn tick_sets_cache_to_true_when_feed_command_exists() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("Feed Epic", "", None).await.unwrap();
        db.patch_epic(epic.id, &EpicPatch::new().feed_command(Some("echo '[]'")))
            .await
            .unwrap();

        let (mut runner, _rx) = make_runner(db.clone());
        runner.tick().await;

        assert_eq!(
            runner.any_feed_cmds,
            Some(true),
            "cache should be Some(true) when at least one epic has a feed command"
        );
    }

    #[tokio::test]
    async fn tick_skips_db_queries_when_cache_is_false_and_no_invalidation() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        db.create_epic("Plain Epic", "", None).await.unwrap();

        let (mut runner, mut rx) = make_runner(db.clone());

        // First tick: no feed commands → cache = Some(false)
        runner.tick().await;
        assert_eq!(runner.any_feed_cmds, Some(false));

        // Add a feed command directly to the DB (simulates MCP update, no EpicChanged signal)
        let epic2 = db.create_epic("Feed Epic", "", None).await.unwrap();
        let cmd = r#"echo '[{"external_id":"c1","title":"C","description":"","status":"backlog","tag":"bug"}]'"#;
        db.patch_epic(epic2.id, &EpicPatch::new().feed_command(Some(cmd)))
            .await
            .unwrap();

        // Second tick: cache is Some(false) → body skipped → task not created
        runner.tick().await;

        let result = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await;
        assert!(
            result.is_err(),
            "tick should skip body when cache is Some(false)"
        );
        let tasks = db.list_tasks_for_epic(epic2.id).await.unwrap();
        assert!(
            tasks.is_empty(),
            "no task should be created while cache prevents DB query"
        );
    }

    #[tokio::test]
    async fn tick_re_queries_after_epic_changed_invalidation() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        db.create_epic("Plain Epic", "", None).await.unwrap();

        let (mut runner, mut rx) = make_runner(db.clone());

        // First tick: no feed commands → cache = Some(false)
        runner.tick().await;
        assert_eq!(runner.any_feed_cmds, Some(false));

        // Add a feed command and then invalidate the cache via the watch sender
        let epic2 = db.create_epic("Feed Epic", "", None).await.unwrap();
        let cmd = r#"echo '[{"external_id":"r1","title":"R","description":"","status":"backlog","tag":"bug"}]'"#;
        db.patch_epic(epic2.id, &EpicPatch::new().feed_command(Some(cmd)))
            .await
            .unwrap();
        runner.epic_invalidate_tx().send(()).ok();

        // Third tick: cache invalidated → re-queries → processes feed command
        runner.tick().await;
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for McpEvent after cache invalidation")
            .expect("channel closed");

        let tasks = db.list_tasks_for_epic(epic2.id).await.unwrap();
        assert_eq!(
            tasks.len(),
            1,
            "task should be created after cache invalidation"
        );
    }

    #[tokio::test]
    async fn tick_non_github_url_stores_empty_sentinel() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        db.save_repo_path("/home/user/code/myrepo").await.unwrap();
        let epic = db.create_epic("Feed Epic", "", None).await.unwrap();
        let cmd = r#"echo '[{"external_id":"1","title":"T","description":"","url":"https://jira.company.com/PROJ-123","status":"backlog","tag":"bug"}]'"#;
        db.patch_epic(epic.id, &EpicPatch::new().feed_command(Some(cmd)))
            .await
            .unwrap();

        let (mut runner, mut rx) = make_runner(db.clone());
        runner.tick().await;

        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for McpEvent")
            .expect("channel closed");

        let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(
            tasks[0].repo_path, "",
            "non-github url should store empty sentinel"
        );
    }

    // --- cleanup_removed_feed_tasks ---

    use std::collections::HashSet;
    use std::sync::{Condvar, Mutex};

    use crate::db::{RemovedFeedTask, TaskPatch};
    use crate::models::TaskId;
    use crate::process::MockProcessRunner;

    fn removed_task(
        id: i64,
        repo: &str,
        worktree: Option<&str>,
        window: Option<&str>,
    ) -> RemovedFeedTask {
        RemovedFeedTask {
            id: TaskId(id),
            repo_path: repo.to_string(),
            worktree: worktree.map(str::to_string),
            tmux_window: window.map(test_tmux_window),
        }
    }

    #[tokio::test]
    async fn cleanup_removes_worktree_and_kills_window() {
        let runner = Arc::new(MockProcessRunner::new(vec![
            // has_window: list-windows names the window, so the kill proceeds
            MockProcessRunner::ok_with_stdout(b"dispatch:pr-1\n"),
            MockProcessRunner::ok(), // tmux kill-window
            MockProcessRunner::ok(), // git worktree remove
            MockProcessRunner::ok(), // git branch -D (best effort)
        ]));

        cleanup_removed_feed_tasks(
            runner.clone(),
            vec![removed_task(
                1,
                "/repo/a",
                Some("/repo/a/.worktrees/pr-1"),
                Some("dispatch:pr-1"),
            )],
        )
        .await;

        let calls = runner.flattened_calls();
        assert!(
            calls
                .iter()
                .any(|c| c.contains("worktree remove") && c.contains("/repo/a/.worktrees/pr-1")),
            "must remove the worktree, got: {calls:?}"
        );
        assert!(
            calls.iter().any(|c| c.contains("kill-window")),
            "must kill the tmux window, got: {calls:?}"
        );
        assert!(
            calls.iter().any(|c| c.contains("branch -D")),
            "must best-effort delete the branch, got: {calls:?}"
        );
    }

    // This module's copy of the shared-worktree tripwire
    // (cleanup_removes_the_worktree_even_if_another_row_names_it) went with #4096's
    // unification; the survivor is
    // `src/runtime/tests.rs::exec_cleanup_tears_down_even_if_another_row_names_the_worktree`.

    #[tokio::test]
    async fn cleanup_kills_window_only_when_there_is_no_worktree() {
        let runner = Arc::new(MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"dispatch:pr-1\n"),
            MockProcessRunner::ok(), // tmux kill-window
        ]));

        cleanup_removed_feed_tasks(
            runner.clone(),
            vec![removed_task(7, "/repo/a", None, Some("dispatch:pr-1"))],
        )
        .await;

        let calls = runner.flattened_calls();
        assert!(
            calls.iter().any(|c| c.contains("kill-window")),
            "the window must be killed, got: {calls:?}"
        );
        assert!(
            !calls.iter().any(|c| c.starts_with("git ")),
            "with no worktree there is nothing for git to do, got: {calls:?}"
        );
    }

    #[tokio::test]
    async fn cleanup_of_a_stateless_row_runs_no_commands() {
        // An empty response queue panics on the first shell-out — but that panic
        // happens inside `spawn_blocking` and `cleanup_removed_feed_tasks` only
        // *logs* the resulting `JoinError`, so queue exhaustion alone cannot fail
        // this test. The explicit negative below is what carries the assertion.
        let runner = Arc::new(MockProcessRunner::new(vec![]));

        cleanup_removed_feed_tasks(runner.clone(), vec![removed_task(8, "/repo/a", None, None)])
            .await;

        let calls = runner.flattened_calls();
        assert!(
            calls.is_empty(),
            "a row with neither worktree nor window must shell out to nothing, got: {calls:?}"
        );
    }

    /// One task's failure must not abort the rest of its repo's queue.
    #[tokio::test]
    async fn cleanup_continues_after_a_failure() {
        let runner = Arc::new(MockProcessRunner::new(vec![
            MockProcessRunner::fail("fatal: could not lock index"), // pr-1 remove
            MockProcessRunner::ok(),                                // pr-2 remove
            MockProcessRunner::ok(),                                // pr-2 branch -D
        ]));

        cleanup_removed_feed_tasks(
            runner.clone(),
            vec![
                removed_task(1, "/repo/a", Some("/repo/a/.worktrees/pr-1"), None),
                removed_task(2, "/repo/a", Some("/repo/a/.worktrees/pr-2"), None),
            ],
        )
        .await;

        let calls = runner.flattened_calls();
        assert!(
            calls.iter().any(|c| c.contains("/repo/a/.worktrees/pr-2")),
            "pr-2 must still be torn down after pr-1 failed, got: {calls:?}"
        );
    }

    /// How long [`OverlapRunner`] holds the first call of each repo open, giving
    /// a concurrently-issued sibling call the chance to be seen. Generous: the
    /// correct implementation cannot end the wait early, so this is the test's
    /// floor, while a wrong implementation ends it in microseconds.
    const GATE_WINDOW: Duration = Duration::from_millis(500);

    /// A `ProcessRunner` decorator that observes call *overlap* rather than call
    /// order — order alone cannot distinguish "serialised" from "concurrent but
    /// happened to finish in order".
    ///
    /// For every `git -C <repo>` call it records whether another call for the
    /// same repo — or for a different repo — was in flight at that moment. To
    /// make an overlap observable at all it holds the FIRST call of each repo
    /// open for [`GATE_WINDOW`], releasing early the moment a same-repo overlap
    /// is seen. This is a bounded wait on a *signal*, not a wall-clock sleep:
    /// what the test asserts on is whether the signal arrived.
    ///
    /// The two flags it collects are the two halves of the design requirement,
    /// and neither can be produced by scheduling luck:
    ///
    /// * a per-repo-sequential implementation can NEVER show a same-repo
    ///   overlap — the held call occupies the one thread that would issue the
    ///   sibling call, so no amount of scheduling produces one;
    /// * with both repos held open concurrently for the same window, an
    ///   all-serialised implementation can NEVER show a cross-repo overlap —
    ///   the second repo cannot reach the runner while the first is held.
    struct OverlapRunner {
        inner: MockProcessRunner,
        state: Mutex<OverlapState>,
        wake: Condvar,
    }

    #[derive(Default)]
    struct OverlapState {
        in_flight: HashMap<String, usize>,
        gated: HashSet<String>,
        same_repo_overlap: bool,
        cross_repo_overlap: bool,
    }

    impl OverlapRunner {
        fn new(responses: Vec<anyhow::Result<std::process::Output>>) -> Self {
            Self {
                inner: MockProcessRunner::new(responses),
                state: Mutex::new(OverlapState::default()),
                wake: Condvar::new(),
            }
        }

        /// The repo a `git -C <repo> …` call targets. `None` for anything else.
        fn repo_of(program: &str, args: &[&str]) -> Option<String> {
            if program != "git" {
                return None;
            }
            let i = args.iter().position(|a| *a == "-C")?;
            args.get(i + 1).map(|r| (*r).to_string())
        }
    }

    impl ProcessRunner for OverlapRunner {
        fn run(&self, program: &str, args: &[&str]) -> anyhow::Result<std::process::Output> {
            let Some(repo) = Self::repo_of(program, args) else {
                return self.inner.run(program, args);
            };

            {
                let mut state = self.state.lock().unwrap();
                let others: usize = state
                    .in_flight
                    .iter()
                    .filter(|(other, _)| *other != &repo)
                    .map(|(_, n)| *n)
                    .sum();
                if state.in_flight.get(&repo).copied().unwrap_or(0) > 0 {
                    state.same_repo_overlap = true;
                }
                if others > 0 {
                    state.cross_repo_overlap = true;
                }
                *state.in_flight.entry(repo.clone()).or_default() += 1;
                let gate = state.gated.insert(repo.clone());
                self.wake.notify_all();
                if gate {
                    let _unused = self
                        .wake
                        .wait_timeout_while(state, GATE_WINDOW, |state| !state.same_repo_overlap)
                        .unwrap();
                }
            }

            let out = self.inner.run(program, args);
            *self
                .state
                .lock()
                .unwrap()
                .in_flight
                .entry(repo)
                .or_default() -= 1;
            out
        }
    }

    // Two removals in the same repo must not run git concurrently — git locks
    // the repo's worktree metadata and index. Removals in *different* repos
    // still may.
    #[tokio::test]
    async fn cleanup_serialises_same_repo_removals() {
        // Every response is an identical success: with two repos in flight the
        // pop order is not deterministic, so nothing may depend on it.
        let runner = Arc::new(OverlapRunner::new(
            (0..6).map(|_| MockProcessRunner::ok()).collect(),
        ));

        cleanup_removed_feed_tasks(
            runner.clone(),
            vec![
                removed_task(1, "/repo/a", Some("/repo/a/.worktrees/pr-1"), None),
                removed_task(2, "/repo/a", Some("/repo/a/.worktrees/pr-2"), None),
                removed_task(3, "/repo/b", Some("/repo/b/.worktrees/pr-3"), None),
            ],
        )
        .await;

        let calls = runner.inner.flattened_calls();
        for wanted in ["pr-1", "pr-2", "pr-3"] {
            assert!(
                calls.iter().any(|c| c.contains(wanted)),
                "{wanted} must have been torn down, got: {calls:?}"
            );
        }

        let state = runner.state.lock().unwrap();
        assert!(
            !state.same_repo_overlap,
            "two removals in one repo must never have git calls in flight together"
        );
        assert!(
            state.cross_repo_overlap,
            "removals in different repos must proceed in parallel"
        );

        // Belt and braces on ordering: within /repo/a, pr-1 is fully handled
        // before pr-2 starts.
        let pr1 = calls.iter().rposition(|c| c.contains("pr-1")).unwrap();
        let pr2 = calls.iter().position(|c| c.contains("pr-2")).unwrap();
        assert!(
            pr1 < pr2,
            "pr-1's git calls must all precede pr-2's, got: {calls:?}"
        );
    }
}
