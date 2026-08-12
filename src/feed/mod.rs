mod exec;
mod ingest;
mod parse;
mod routing;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::db::TaskStore;
use crate::dispatch::resolve_feed_item_repo_paths;
use crate::mcp::McpEvent;
use crate::models::EpicId;
use crate::process::ProcessRunner;

pub(crate) use exec::{exec_feed_command, resolve_base_branches};
pub(crate) use ingest::{run_feed_sync_by_role, FeedItemWithTarget};
pub use routing::route;

/// Log `result`'s error via `tracing::warn!` and discard it. Shared by the
/// feed pipeline's fire-and-forget per-epic DB writes, where a failure is
/// logged but must not abort the surrounding reconciliation pass. Pass
/// `sub_epic_id` when the write is scoped to a sub-epic beneath `epic_id`.
///
/// `context` labels the call site in the log line (e.g.
/// `"sync_grouped_feed: upsert_feed_tasks failed"`).
pub(crate) fn warn_on_err<T>(
    result: anyhow::Result<T>,
    epic_id: EpicId,
    sub_epic_id: Option<EpicId>,
    context: &str,
) {
    if let Err(err) = result {
        match sub_epic_id {
            Some(sub_epic_id) => tracing::warn!(
                epic_id = epic_id.0,
                sub_epic_id = sub_epic_id.0,
                "{context}: {err:#}"
            ),
            None => tracing::warn!(epic_id = epic_id.0, "{context}: {err:#}"),
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

/// Per-item dispatch context for one epic's feed poll, spawned by
/// [`FeedRunner::tick`]. Bundles the fields `tick` used to clone individually
/// into its spawned task, so `tick` only has to build a `FeedJob` and spawn
/// it — scheduling (interval bookkeeping, `last_run`) stays in `tick`,
/// separate from the exec→parse→sync→notify sequence in [`FeedJob::run`].
struct FeedJob {
    db: Arc<dyn TaskStore>,
    notify: mpsc::UnboundedSender<McpEvent>,
    runner: Arc<dyn ProcessRunner>,
    cmd: String,
    epic: crate::models::Epic,
    known_paths: Arc<Vec<String>>,
}

impl FeedJob {
    /// Execute the command, parse its output, and reconcile the epic's tasks
    /// from it. Any failure along the way is logged and the job simply
    /// returns — `tick` fires-and-forgets these onto the tokio runtime.
    async fn run(self) {
        // exec_feed_command has already logged the failure (and any
        // stderr-on-success); the auto-poll path adds nothing, per
        // feeds.allium FeedCommandFailure ("the TUI is NOT notified").
        let Ok(output) = exec::exec_feed_command(&self.cmd, self.epic.id.0, &self.epic.title).await
        else {
            return;
        };
        let stdout = output.stdout;

        let items = match parse::parse_feed_items(&stdout) {
            Ok(i) => i,
            Err(err) => {
                tracing::warn!(
                    epic_id = self.epic.id.0,
                    epic_title = %self.epic.title,
                    "FeedRunner: failed to parse JSON output: {err:#}"
                );
                return;
            }
        };

        let repo_paths = resolve_feed_item_repo_paths(&items, &self.known_paths);
        let base_branches = resolve_base_branches(&repo_paths, &*self.runner);
        let entries = ingest::FeedItemWithTarget::zip(items, repo_paths, base_branches);

        // Role sub-epics (my/team/bots) carry no feed_command (enforced
        // at provisioning in WP5), so they are never iterated here —
        // only the parent is polled. Guard against a misconfigured role
        // sub-epic that somehow has a feed_command: skip it rather than
        // reconcile a child as if it were a feed. The role→sync-path
        // decision itself is delegated to the shared dispatcher so this
        // path and the manual "r" refresh cannot drift.
        use crate::models::FeedRole;
        if matches!(
            self.epic.feed_role,
            FeedRole::MyReviews | FeedRole::TeamReviews | FeedRole::Bots
        ) {
            debug_assert!(
                false,
                "role sub-epic {} (feed_role={:?}) must not carry a feed_command",
                self.epic.id.0, self.epic.feed_role
            );
            tracing::warn!(
                epic_id = self.epic.id.0,
                feed_role = ?self.epic.feed_role,
                "FeedRunner: role sub-epic carries a feed_command; skipping (role sub-epics are reconciled only via their reviews_parent)"
            );
            return;
        }
        let sync_result = ingest::run_feed_sync_by_role(
            &*self.db,
            self.epic.id,
            self.epic.feed_role,
            self.epic.group_by_repo,
            entries,
        )
        .await;

        if let Ok(affected_ids) = &sync_result {
            recalculate_epic_status_after_feed(&*self.db, self.epic.id, "FeedRunner").await;
            for id in affected_ids {
                let _ = self.notify.send(McpEvent::EpicChanged(*id));
            }
        }
        warn_on_err(
            sync_result,
            self.epic.id,
            None,
            "FeedRunner: upsert_feed_tasks failed",
        );
    }
}

const DEFAULT_FEED_INTERVAL: Duration = Duration::from_secs(30);

/// Poll interval for the background feed task.
/// Kept in `feed` (not reusing `TICK_INTERVAL` from `runtime`) so the two
/// concerns stay independent.
const FEED_POLL_INTERVAL: Duration = Duration::from_secs(2);

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

        for epic in epics {
            let Some(ref cmd) = epic.feed_command else {
                continue;
            };

            let interval = epic
                .feed_interval_secs
                .map(|s| Duration::from_secs(s as u64))
                .unwrap_or(DEFAULT_FEED_INTERVAL);

            let elapsed = self
                .last_run
                .get(&epic.id)
                .map(|t| t.elapsed())
                .unwrap_or(Duration::MAX);

            if elapsed < interval {
                continue;
            }

            self.last_run.insert(epic.id, Instant::now());

            let job = FeedJob {
                db: self.db.clone(),
                notify: self.notify.clone(),
                runner: self.runner.clone(),
                cmd: cmd.clone(),
                epic,
                known_paths: Arc::clone(&known_paths),
            };

            tokio::task::spawn(job.run());
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use std::sync::Arc;

    use super::*;
    use crate::db::{Database, EpicCrud, EpicPatch, EpicRead, SettingsStore};
    use crate::models::{TaskStatus, TaskTag};

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

    #[tokio::test]
    async fn tick_does_not_block_event_loop() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("Slow Epic", "", None).await.unwrap();
        db.patch_epic(epic.id, &EpicPatch::new().feed_command(Some("sleep 5")))
            .await
            .unwrap();

        let (mut runner, _rx) = make_runner(db.clone());

        let start = std::time::Instant::now();
        runner.tick().await;
        let elapsed = start.elapsed();

        assert!(
            elapsed < Duration::from_millis(500),
            "tick() blocked for {elapsed:?}"
        );
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
    // before; this closes the gap for FeedJob::run.
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

    /// B3 concurrency: two back-to-back zero-interval ticks must not drop the
    /// task to a move/delete interleave.
    #[tokio::test]
    async fn tick_two_ticks_lose_nothing() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let parent = db.create_epic("Reviews", "", None).await.unwrap();
        db.patch_epic(
            parent.id,
            &EpicPatch::new()
                .feed_role(crate::models::FeedRole::ReviewsParent)
                .feed_interval_secs(Some(0))
                .feed_command(Some(
                    r#"echo '[{"external_id":"pr-1","title":"Team","description":"","url":"https://github.com/org/repo/pull/1","status":"backlog","tag":"pr-review","signals":["team-request"]}]'"#,
                )),
        )
        .await
        .unwrap();

        let (mut runner, mut rx) = make_runner(db.clone());
        // Zero interval: both ticks run the feed and spawn a reconcile.
        runner.tick().await;
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

    #[tokio::test]
    async fn start_returns_immediately_without_blocking() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("Slow Epic", "", None).await.unwrap();
        // A command that would block for 5 seconds if awaited inline.
        db.patch_epic(epic.id, &EpicPatch::new().feed_command(Some("sleep 5")))
            .await
            .unwrap();

        let (tx, _rx) = mpsc::unbounded_channel();
        let proc_runner: Arc<dyn ProcessRunner> =
            Arc::new(crate::process::MockProcessRunner::new(vec![]));
        let runner = FeedRunner::new(db as Arc<dyn crate::db::TaskStore>, tx, proc_runner);

        let before = std::time::Instant::now();
        runner.start(); // must return immediately — it just spawns a task
        let elapsed = before.elapsed();

        assert!(
            elapsed < std::time::Duration::from_millis(50),
            "start() took {elapsed:?}, expected <50ms — it must not block"
        );
    }

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
                ))
                // 0-second interval so the second tick re-runs the command.
                .feed_interval_secs(Some(0)),
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
}
