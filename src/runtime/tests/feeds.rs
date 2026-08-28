use super::*;

mod epic_tests {
    use super::*;

    #[tokio::test]
    async fn exec_insert_epic_creates_in_db_and_app() {
        let (rt, mut app) = test_runtime().await;
        rt.exec_insert_epic(&mut app, "My Epic".into(), "description".into(), None)
            .await;
        assert_eq!(app.epics().len(), 1);
        assert_eq!(app.epics()[0].title, "My Epic");
        assert_eq!(rt.database.list_epics().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn exec_delete_epic_removes_from_db() {
        let (rt, mut app) = test_runtime().await;
        let epic = rt
            .db_write()
            .create_epic("Doomed", "bye", None)
            .await
            .unwrap();
        rt.exec_delete_epic(&mut app, epic.id).await;
        assert!(rt.database.list_epics().await.unwrap().is_empty());
        assert!(app.error_popup().is_none());
    }

    #[tokio::test]
    async fn exec_persist_epic_updates_status() {
        let (rt, mut app) = test_runtime().await;
        let epic = rt
            .db_write()
            .create_epic("Epic", "desc", None)
            .await
            .unwrap();
        rt.exec_persist_epic(&mut app, epic.id, Some(models::TaskStatus::Running), None)
            .await;
        let updated = rt.database.get_epic(epic.id).await.unwrap().unwrap();
        assert_eq!(updated.status, models::TaskStatus::Running);
    }

    #[tokio::test]
    async fn exec_persist_epic_noop_when_nothing_to_update() {
        let (rt, mut app) = test_runtime().await;
        let epic = rt
            .db_write()
            .create_epic("Epic", "desc", None)
            .await
            .unwrap();
        // Should return early without error
        rt.exec_persist_epic(&mut app, epic.id, None, None).await;
        assert!(app.error_popup().is_none());
    }

    /// Regression for the whole-branch review finding, epic side:
    /// `exec_persist_epic` (routed through `exec_patch_epic`, the shared
    /// chokepoint) must write the service-computed `sort_order` into the
    /// in-memory board itself, not just the DB. Drives the actual
    /// `exec_persist_epic` runtime path and asserts on `app.epics()` with no
    /// `exec_refresh_epics_from_db` call in between, to prove the write-back is
    /// immediate.
    #[tokio::test]
    async fn exec_persist_epic_writes_back_done_transition_sort_order_immediately() {
        let (rt, mut app) = test_runtime().await;
        let epic = rt
            .db_write()
            .create_epic("Epic", "desc", None)
            .await
            .unwrap();
        // Load the epic into the in-memory board (mirrors what a real session
        // would already have from a prior refresh).
        rt.exec_refresh_epics_from_db(&mut app).await;
        assert_eq!(
            app.epics()
                .iter()
                .find(|e| e.id == epic.id)
                .unwrap()
                .sort_order,
            None,
            "precondition: no sort_order yet"
        );

        rt.exec_persist_epic(&mut app, epic.id, Some(models::TaskStatus::Done), None)
            .await;

        // Assert on the in-memory board directly — no
        // exec_refresh_epics_from_db call in between — to prove the write-back
        // is immediate.
        let in_memory = app.epics().iter().find(|e| e.id == epic.id).unwrap();
        assert!(
            in_memory.sort_order.is_some_and(|so| so < 0),
            "expected a negative completion-recency sort_order written back to \
         the in-memory board immediately, got {:?}",
            in_memory.sort_order
        );

        let db_epic = rt.database.get_epic(epic.id).await.unwrap().unwrap();
        assert_eq!(
            in_memory.sort_order, db_epic.sort_order,
            "in-memory sort_order must match what was actually persisted"
        );
    }

    /// The clear direction of the same rule, epic side:
    /// `sort_order_for_status_transition(Done, <non-Done>)` returns
    /// `Some(None)`, so `write_back_epic_sort_order` must clear the in-memory
    /// epic's `sort_order` — not skip the write-back because the new value is
    /// `None`. Asserts on `app.epics()` with no `exec_refresh_epics_from_db`
    /// call in between.
    #[tokio::test]
    async fn exec_persist_epic_writes_back_leaving_done_sort_order_clear_immediately() {
        let (rt, mut app) = test_runtime().await;
        let epic = rt
            .db_write()
            .create_epic("Epic", "desc", None)
            .await
            .unwrap();
        // Put the epic in Done with a completion-recency sort_order, the state a
        // just-completed epic is in before it gets moved back out of Done.
        rt.db_write()
            .patch_epic(
                epic.id,
                &db::EpicPatch::new()
                    .status(models::TaskStatus::Done)
                    .sort_order(Some(-1234)),
            )
            .await
            .unwrap();
        // Load that state into the in-memory board.
        rt.exec_refresh_epics_from_db(&mut app).await;
        assert_eq!(
            app.epics()
                .iter()
                .find(|e| e.id == epic.id)
                .unwrap()
                .sort_order,
            Some(-1234),
            "precondition: in-memory epic carries the Done sort_order"
        );

        rt.exec_persist_epic(&mut app, epic.id, Some(models::TaskStatus::Review), None)
            .await;

        let in_memory = app.epics().iter().find(|e| e.id == epic.id).unwrap();
        assert_eq!(
            in_memory.sort_order, None,
            "leaving Done must clear the in-memory epic's sort_order immediately"
        );

        let db_epic = rt.database.get_epic(epic.id).await.unwrap().unwrap();
        assert_eq!(
            db_epic.sort_order, None,
            "in-memory clear must match what was actually persisted"
        );
    }

    /// Regression for learning #162, ported from the retired TUI `[C]` save path
    /// (docs/plans/archive/2026-07-31-3809-keybinding-pruning-implementation.md §6): a freshly-enabled
    /// feed on a previously feed-less instance must become pollable after
    /// `set_managed_feed_config` notifies the runtime, not stay stranded behind the
    /// FeedRunner's `any_feed_cmds == Some(false)` short-circuit until an unrelated
    /// EpicChanged or a restart. MCP is now the only configuration path, so the
    /// `McpEvent::Refresh` arm is the only thing that can invalidate the cache.
    #[tokio::test]
    async fn mcp_refresh_invalidates_feed_runner_cache_after_enabling_a_feed() {
        let (mut rt, mut app) = test_runtime().await;
        let mut feed_runner = rt.feed_runner.take().expect("runtime has a feed runner");

        // First tick with no feeds configured -> cache settles to Some(false),
        // which makes every subsequent tick short-circuit before any DB work.
        feed_runner.tick().await;
        assert_eq!(
            feed_runner.any_feed_cmds_cache(),
            Some(false),
            "feed-less instance should cache Some(false) and short-circuit"
        );

        // Enable the reviews feed and provision it, exactly as
        // set_managed_feed_config does, then deliver the notification it sends.
        rt.database
            .set_reviews_feed_command(Some("reviews.sh"))
            .await
            .unwrap();
        let settings = crate::service::read_managed_feed_settings(&*rt.database)
            .await
            .unwrap();
        rt.epic_svc.provision_managed_feeds(settings).await.unwrap();
        apply_loop_event(&mut app, LoopEvent::Mcp(mcp::McpEvent::Refresh), &rt);

        // The refresh must have invalidated the cache so the next tick re-queries
        // and discovers the freshly-provisioned reviews_parent feed command.
        feed_runner.tick().await;
        assert_eq!(
            feed_runner.any_feed_cmds_cache(),
            Some(true),
            "refresh must invalidate the cache so the freshly-enabled feed becomes pollable"
        );
    }

    #[tokio::test]
    async fn exec_refresh_epics_from_db_syncs_to_app() {
        let (rt, mut app) = test_runtime().await;
        // Insert epic directly into DB, bypassing app
        rt.db_write()
            .create_epic("Direct", "desc", None)
            .await
            .unwrap();
        assert!(app.epics().is_empty());
        rt.exec_refresh_epics_from_db(&mut app).await;
        assert_eq!(app.epics().len(), 1);
        assert_eq!(app.epics()[0].title, "Direct");
    }
}

/// exec_trigger_epic_feed / SerialisedFeedCycle (feeds.allium) — one feed cycle per
/// epic at a time. The race these close: nothing used to serialise a manual "r"
/// refresh against an in-flight auto-poll for the SAME epic, so the two could
/// interleave between the non-transactional steps of run_role_routed_feed_sync.
/// Each of those steps filters on a task's CURRENT epic_id, so one pass could see
/// a task the other had moved-but-not-yet-committed as absent from its keep-set,
/// delete it, and -- since feed deletes now feed TaskTeardown -- force-remove a
/// live review agent's worktree.
mod feed_epic_trigger {
    use super::*;

    /// The manual path's half of the drop contract: a refresh requested while a
    /// cycle for that epic is in flight reports AlreadyRefreshing and writes
    /// nothing. Deterministic because the test holds the claim itself.
    #[tokio::test]
    async fn exec_trigger_epic_feed_reports_already_refreshing_while_a_cycle_is_in_flight() {
        let db = test_db().await;
        let epic = db.create_epic("Reviews", "", None).await.unwrap();
        // Would delete the seeded task (absent from an empty emission) and tear its
        // worktree down, if it ever ran.
        set_feed_command(&db, epic.id, "echo '[]'").await;
        seed_feed_task_with_worktree(&db, epic.id, "In-flight PR").await;

        let (tx, mut rx) = mpsc::unbounded_channel();
        let rt = make_runtime(db.clone(), tx, Arc::new(MockProcessRunner::new(vec![]))).await;

        let _claim = rt
            .feed_sync_guard
            .try_claim(epic.id)
            .expect("the epic starts unclaimed");

        rt.exec_trigger_epic_feed(epic.id, "Reviews".to_string());

        let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
            .await
            .expect("timed out")
            .expect("channel closed");
        assert!(
            matches!(
                msg,
                Message::Feed(crate::tui::messages::FeedMessage::AlreadyRefreshing { .. })
            ),
            "a dropped refresh must report AlreadyRefreshing, not a success or a \
         failure, got: {msg:?}"
        );
        assert_eq!(
            db.list_tasks_for_epic(epic.id).await.unwrap().len(),
            1,
            "a dropped refresh must run no sync, so the existing task survives"
        );
    }

    /// The wiring invariant, pinned directly and cheaply: the manual "r" path and
    /// the auto-poll runner must share ONE claim registry. A second registry
    /// type-checks, compiles, and silently serialises nothing.
    ///
    /// This is a one-line structural check, so it survives any change to feed
    /// behaviour. It does NOT subsume the FIFO test below: identity of the registry
    /// is not the same property as the claim actually being HELD across the exec,
    /// and only a real in-flight cycle can show the latter.
    #[tokio::test]
    async fn the_manual_path_and_the_feed_runner_share_one_claim_registry() {
        let db = test_db().await;
        let (tx, _rx) = mpsc::unbounded_channel();
        let rt = make_runtime(db, tx, Arc::new(MockProcessRunner::new(vec![]))).await;

        assert!(
            Arc::ptr_eq(
                &rt.feed_sync_guard,
                &rt.feed_runner
                    .as_ref()
                    .expect("make_runtime wires a FeedRunner")
                    .sync_guard(),
            ),
            "TuiRuntime.feed_sync_guard must be the FeedRunner's registry, not a \
         second one -- otherwise the two feed surfaces never serialise"
        );
    }

    /// The flagship: BOTH surfaces against ONE epic, with a real auto-poll cycle
    /// genuinely mid-exec rather than a claim the test planted.
    ///
    /// This is the only test here that would catch the two surfaces being wired to
    /// DIFFERENT `FeedSyncGuard` registries — a mistake that type-checks, passes
    /// every other test in this file, and silently serialises nothing.
    ///
    /// Determinism without sleeping: the feed command blocks on `cat <fifo>`, and
    /// opening a FIFO for WRITING blocks until a reader opens it. So the successful
    /// return of that open IS the proof that the cycle has reached its exec. No
    /// polling, no `sleep` (which `./scripts/check-no-test-sleep.sh` bans anyway).
    ///
    /// The open is deadline-bounded on purpose. If a regression makes the cycle
    /// bail before exec — a broken claim, a failed epic read — no reader ever opens
    /// the FIFO and an unbounded open would wedge CI silently instead of failing.
    /// Note what the timeout does and does not buy: `spawn_blocking` work is not
    /// cancellable, so it frees the test, not the thread; the blocked thread leaks
    /// until the process exits. That is the right trade in a test binary, and it is
    /// strictly better than a hang.
    #[tokio::test]
    async fn manual_refresh_is_dropped_while_a_real_auto_poll_cycle_is_in_flight() {
        let db = test_db().await;
        let epic = db.create_epic("Reviews", "", None).await.unwrap();

        let fifo = std::env::temp_dir().join(format!("dispatch_feed_gate_{}", epic.id.0));
        let _ = std::fs::remove_file(&fifo);
        let mkfifo = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("failed to run mkfifo");
        assert!(mkfifo.success(), "mkfifo failed for {}", fifo.display());

        // Blocks in exec until the test opens the write end and closes it.
        set_feed_command(&db, epic.id, &format!("cat {}; echo '[]'", fifo.display())).await;

        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut rt = make_runtime(db.clone(), tx, Arc::new(MockProcessRunner::new(vec![]))).await;

        // Start the auto-poll cycle. `tick` only spawns; it does not block.
        rt.feed_runner
            .as_mut()
            .expect("make_runtime wires a FeedRunner")
            .tick()
            .await;

        // Handshake: unblocks only once the spawned cycle's `cat` has the FIFO open,
        // i.e. once it is genuinely inside exec_feed_command holding the claim.
        let gate = fifo.clone();
        let write_end = tokio::time::timeout(
            TEST_TIMEOUT,
            tokio::task::spawn_blocking(move || std::fs::OpenOptions::new().write(true).open(gate)),
        )
        .await
        .expect(
            "timed out waiting for the feed cycle to reach its exec -- it bailed \
         earlier (claim? epic read?), so no reader ever opened the FIFO",
        )
        .expect("the opener thread panicked")
        .expect("failed to open the FIFO for writing");

        // With a cycle provably in flight, the manual refresh must be dropped.
        rt.exec_trigger_epic_feed(epic.id, "Reviews".to_string());

        let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
            .await
            .expect("timed out waiting for the manual refresh outcome")
            .expect("channel closed");
        assert!(
            matches!(
                msg,
                Message::Feed(crate::tui::messages::FeedMessage::AlreadyRefreshing { .. })
            ),
            "a manual refresh during a live auto-poll cycle must be dropped; if this \
         is Refreshed, the two paths are not sharing one FeedSyncGuard. got: {msg:?}"
        );

        // Release the feed command so the in-flight cycle can finish.
        drop(write_end);
        rt.feed_runner
            .as_mut()
            .expect("feed runner")
            .join_spawned_jobs()
            .await;

        // And the epic is claimable again, so the drop was not a permanent wedge.
        assert!(
            rt.feed_sync_guard.try_claim(epic.id).is_some(),
            "the finished cycle must have released its claim"
        );

        let _ = std::fs::remove_file(&fifo);
    }

    /// Seed one feed task carrying a worktree and tmux window, as a dispatched
    /// review agent would. Its survival is what distinguishes "the cycle was
    /// dropped" from "the cycle ran and destroyed a live session".
    async fn seed_feed_task_with_worktree(
        db: &Arc<Database>,
        epic_id: crate::models::EpicId,
        title: &str,
    ) {
        db.upsert_feed_tasks(
            epic_id,
            &[crate::models::FeedItem {
                external_id: "pr-1".to_string(),
                title: title.to_string(),
                description: String::new(),
                url: String::new(),
                url_type: None,
                status: crate::models::TaskStatus::Backlog,
                tag: crate::models::TaskTag::PrReview,
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
        let task = db.list_tasks_for_epic(epic_id).await.unwrap().remove(0);
        db.patch_task(
            task.id,
            &db::TaskPatch::new()
                .worktree(Some("/repo/a/.worktrees/7-pr-1"))
                .tmux_window(Some("dispatch:pr-1")),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn exec_trigger_epic_feed_success() {
        let db = test_db().await;
        let epic = db
            .create_epic("Security Vulnerabilities", "", None)
            .await
            .unwrap();

        let cmd = r#"echo '[{"external_id":"vuln:1","title":"CVE-1","description":"desc","status":"backlog","tag":"fix"}]'"#;
        set_feed_command(&db, epic.id, cmd).await;

        let (tx, mut rx) = mpsc::unbounded_channel();
        let rt = make_runtime(db, tx, Arc::new(MockProcessRunner::new(vec![]))).await;

        rt.exec_trigger_epic_feed(epic.id, "Security Vulnerabilities".to_string());

        let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
            .await
            .expect("timed out waiting for FeedRefreshed")
            .expect("channel closed");
        assert!(
            matches!(
                msg,
                Message::Feed(crate::tui::messages::FeedMessage::Refreshed { count: 1, .. })
            ),
            "expected FeedRefreshed with count=1, got: {msg:?}"
        );
    }

    #[tokio::test]
    async fn exec_trigger_epic_feed_zero_items() {
        let db = test_db().await;
        let epic = db.create_epic("Empty Feed", "", None).await.unwrap();
        set_feed_command(&db, epic.id, "echo '[]'").await;

        let (tx, mut rx) = mpsc::unbounded_channel();
        let rt = make_runtime(db, tx, Arc::new(MockProcessRunner::new(vec![]))).await;

        rt.exec_trigger_epic_feed(epic.id, "Empty Feed".to_string());

        let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
            .await
            .expect("timed out")
            .expect("channel closed");
        assert!(
            matches!(
                msg,
                Message::Feed(crate::tui::messages::FeedMessage::Refreshed { count: 0, .. })
            ),
            "empty feed should still succeed with count=0, got: {msg:?}"
        );
    }

    // feeds.allium: DegradedEmptyEmission. A zero-item emission that wrote to
    // stderr is a failure, not a refresh — the sync is skipped entirely so the
    // epic's existing tasks survive. Inverted from the #3900 behaviour, which
    // reported it as a successful zero-task refresh AFTER the delete had run.
    #[tokio::test]
    async fn exec_trigger_epic_feed_fails_on_degraded_empty_emission() {
        let db = test_db().await;
        let epic = db.create_epic("Degraded Feed", "", None).await.unwrap();
        set_feed_command(&db, epic.id, "echo 'Invalid search query' >&2; echo '[]'").await;

        let (tx, mut rx) = mpsc::unbounded_channel();
        let rt = make_runtime(db, tx, Arc::new(MockProcessRunner::new(vec![]))).await;

        rt.exec_trigger_epic_feed(epic.id, "Degraded Feed".to_string());

        let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
            .await
            .expect("timed out")
            .expect("channel closed");
        match msg {
            Message::Feed(crate::tui::messages::FeedMessage::Failed { error, .. }) => {
                assert!(
                    error.contains("Invalid search query"),
                    "failure must carry the stderr, got: {error}"
                );
            }
            other => panic!("expected FeedMessage::Failed, got: {other:?}"),
        }
    }

    /// Companion to the auto-poll guard test
    /// (`tick_degraded_empty_emission_does_not_delete_existing_tasks`): the
    /// message-only assertion above would still pass if the DegradedEmptyEmission
    /// guard were relocated to AFTER `run_feed_sync_by_role`, by which point the
    /// stale-delete has already run — and, since feed removals now tear down
    /// worktrees, already destroyed a live agent's session. Pin "no sync ran" on
    /// the manual path, not just "a failure was reported".
    #[tokio::test]
    async fn exec_trigger_epic_feed_degraded_empty_emission_does_not_delete_existing_tasks() {
        let db = test_db().await;
        let epic = db.create_epic("Degraded Feed", "", None).await.unwrap();

        // Seed one feed task, as a previous healthy refresh would have.
        db.upsert_feed_tasks(
            epic.id,
            &[crate::models::FeedItem {
                external_id: "pr-1".to_string(),
                title: "Existing PR".to_string(),
                description: String::new(),
                url: String::new(),
                url_type: None,
                status: crate::models::TaskStatus::Backlog,
                tag: crate::models::TaskTag::PrReview,
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
        set_feed_command(&db, epic.id, "echo 'Invalid search query' >&2; echo '[]'").await;

        let (tx, mut rx) = mpsc::unbounded_channel();
        let rt = make_runtime(db.clone(), tx, Arc::new(MockProcessRunner::new(vec![]))).await;

        rt.exec_trigger_epic_feed(epic.id, "Degraded Feed".to_string());

        // Awaiting the message is the deterministic completion signal: the spawned
        // job sends it on its way out, so the DB is settled once it arrives.
        let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
            .await
            .expect("timed out")
            .expect("channel closed");
        assert!(
            matches!(
                msg,
                Message::Feed(crate::tui::messages::FeedMessage::Failed { .. })
            ),
            "expected FeedMessage::Failed, got: {msg:?}"
        );

        let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
        assert_eq!(
            tasks.len(),
            1,
            "a degraded empty emission must not run the sync, so existing feed tasks survive"
        );
        assert_eq!(tasks[0].external_id.as_deref(), Some("pr-1"));
    }

    /// Manual counterpart of
    /// `feed::tests::tick_partially_degraded_emission_does_not_delete_or_tear_down`
    /// (#4095, feeds.allium: DegradedNonEmptyEmission). A partially degraded
    /// emission is NOT a failure — the sync still runs and the emitted item still
    /// lands — but it removes nothing, and the status line says so rather than
    /// reading like an ordinary reconcile.
    #[tokio::test]
    async fn exec_trigger_epic_feed_partially_degraded_emission_does_not_delete() {
        let db = test_db().await;
        let epic = db.create_epic("Reviews", "", None).await.unwrap();

        let seeded: Vec<crate::models::FeedItem> = ["pr-1", "pr-2"]
            .iter()
            .map(|ext| crate::models::FeedItem {
                external_id: ext.to_string(),
                title: "Seeded".to_string(),
                description: String::new(),
                url: String::new(),
                url_type: None,
                status: crate::models::TaskStatus::Backlog,
                tag: crate::models::TaskTag::PrReview,
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
                .status(crate::models::TaskStatus::Running)
                .sub_status(crate::models::SubStatus::Active)
                .worktree(Some("/repo/a/.worktrees/7-pr-1")),
        )
        .await
        .unwrap();

        set_feed_command(
        &db,
        epic.id,
        r#"echo 'fetch-reviews: gh search prs failed' >&2; echo '[{"external_id":"pr-2","title":"Other","description":"","status":"backlog","tag":"pr-review"}]'"#,
    )
    .await;

        let (tx, mut rx) = mpsc::unbounded_channel();
        let proc_runner = Arc::new(MockProcessRunner::new(vec![]));
        let rt = make_runtime(db.clone(), tx, proc_runner.clone()).await;

        rt.exec_trigger_epic_feed(epic.id, "Reviews".to_string());

        let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
            .await
            .expect("timed out")
            .expect("channel closed");
        match msg {
            Message::Feed(crate::tui::messages::FeedMessage::Refreshed { degraded, .. }) => {
                let reason = degraded.expect("a degraded refresh must carry its reason");
                assert!(
                    reason.contains("gh search prs failed"),
                    "the reason must name what the script reported, got: {reason}"
                );
            }
            other => panic!("expected a degraded FeedMessage::Refreshed, got: {other:?}"),
        }

        let ids: Vec<String> = db
            .list_tasks_for_epic(epic.id)
            .await
            .unwrap()
            .into_iter()
            .filter_map(|t| t.external_id)
            .collect();
        assert!(
            ids.contains(&"pr-1".to_string()),
            "the omitted task must survive a degraded manual refresh, got {ids:?}"
        );
        assert!(
            !proc_runner
                .flattened_calls()
                .iter()
                .any(|c| c.contains("worktree remove")),
            "and its worktree must not be torn down"
        );
    }

    /// End-to-end wiring guard for the MANUAL "r" refresh path — the mirror of
    /// `feed::tests::tick_removed_task_tears_down_its_worktree` on the auto-poll
    /// side. A refresh whose emission drops a task must actually shell out
    /// `git worktree remove` for it.
    ///
    /// Both call sites need their own guard: they are separate call sites, and the
    /// coverage either side of the seam never crosses it (ingest tests prove
    /// `outcome.removed` is populated; the `cleanup_*` tests call the helper
    /// directly with a hand-built `Vec`). Gutting either fan-out call to
    /// `let _ = outcome.removed;` left the whole suite green before these landed.
    #[tokio::test]
    async fn exec_trigger_epic_feed_removed_task_tears_down_its_worktree() {
        let db = test_db().await;
        let epic = db.create_epic("Reviews", "", None).await.unwrap();

        // Seed one feed task with the on-disk state a dispatched agent would own.
        seed_feed_task_with_worktree(&db, epic.id, "Merged PR").await;

        set_feed_command(&db, epic.id, "echo '[]'").await;

        let proc_runner = Arc::new(MockProcessRunner::new(vec![
            // has_window: list-windows names the window, so the kill proceeds
            MockProcessRunner::ok_with_stdout(b"dispatch:pr-1\n"),
            MockProcessRunner::ok(), // tmux kill-window
            MockProcessRunner::ok(), // git worktree remove
            MockProcessRunner::ok(), // git branch -D (best effort)
        ]));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let rt = make_runtime(db.clone(), tx, proc_runner.clone()).await;

        // The PR merged, so the refresh's emission no longer carries it. A clean
        // empty emission (no stderr) is a genuine reconcile, not a degraded run.
        rt.exec_trigger_epic_feed(epic.id, "Reviews".to_string());

        // Refreshed is sent AFTER the teardown is awaited, so its arrival is a
        // deterministic signal that the cleanup has run.
        let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
            .await
            .expect("timed out")
            .expect("channel closed");
        assert!(
            matches!(
                msg,
                Message::Feed(crate::tui::messages::FeedMessage::Refreshed { count: 0, .. })
            ),
            "expected FeedRefreshed with count=0, got: {msg:?}"
        );

        assert!(
            db.list_tasks_for_epic(epic.id).await.unwrap().is_empty(),
            "the merged PR's row is gone"
        );

        let calls: Vec<String> = proc_runner.flattened_calls();
        assert!(
            calls
                .iter()
                .any(|c| c.contains("worktree remove") && c.contains("/repo/a/.worktrees/7-pr-1")),
            "the manual refresh path must tear the removed task's worktree down, got: {calls:?}"
        );
        assert!(
            calls.iter().any(|c| c.contains("kill-window")),
            "and kill its tmux window, got: {calls:?}"
        );
    }

    #[tokio::test]
    async fn exec_trigger_epic_feed_command_fails() {
        let db = test_db().await;
        let epic = db.create_epic("Failing Feed", "", None).await.unwrap();
        set_feed_command(&db, epic.id, "exit 1").await;

        let (tx, mut rx) = mpsc::unbounded_channel();
        let rt = make_runtime(db, tx, Arc::new(MockProcessRunner::new(vec![]))).await;

        rt.exec_trigger_epic_feed(epic.id, "Failing Feed".to_string());

        let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
            .await
            .expect("timed out")
            .expect("channel closed");
        assert_feed_failed_because(&msg, None, "non-zero exit");
    }

    #[tokio::test]
    async fn exec_trigger_epic_feed_malformed_json() {
        let db = test_db().await;
        let epic = db.create_epic("Bad JSON Feed", "", None).await.unwrap();
        set_feed_command(&db, epic.id, "echo 'not-json'").await;

        let (tx, mut rx) = mpsc::unbounded_channel();
        let rt = make_runtime(db, tx, Arc::new(MockProcessRunner::new(vec![]))).await;

        rt.exec_trigger_epic_feed(epic.id, "Bad JSON Feed".to_string());

        let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
            .await
            .expect("timed out")
            .expect("channel closed");
        assert_feed_failed_because(&msg, Some("parse"), "malformed JSON");
    }

    #[tokio::test]
    async fn exec_trigger_epic_feed_missing_tag_fails_and_upserts_nothing() {
        // The manual "r" path must reject a tag-less item exactly as the auto-poll
        // path and verify-feed do. This held by accident while all three called
        // serde_json separately; once the manual path routes through the shared
        // parse_feed_items it holds by construction. See feeds.allium's
        // FeedItemParse block.
        let db = test_db().await;
        let epic = db.create_epic("Untagged Feed", "", None).await.unwrap();
        set_feed_command(
            &db,
            epic.id,
            r#"echo '[{"external_id":"x1","title":"T","description":"","status":"backlog"}]'"#,
        )
        .await;

        let (tx, mut rx) = mpsc::unbounded_channel();
        let rt = make_runtime(db.clone(), tx, Arc::new(MockProcessRunner::new(vec![]))).await;

        rt.exec_trigger_epic_feed(epic.id, "Untagged Feed".to_string());

        let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
            .await
            .expect("timed out")
            .expect("channel closed");
        assert_feed_failed_because(&msg, Some("parse"), "a missing tag");
        let tasks = db.list_all().await.unwrap();
        assert!(
            tasks.is_empty(),
            "a rejected emission must upsert no task, got: {tasks:?}"
        );
    }

    #[tokio::test]
    async fn exec_trigger_epic_feed_grouped_puts_tasks_in_sub_epics() {
        let db = test_db().await;
        let epic = db.create_epic("Reviews", "", None).await.unwrap();

        let cmd = r#"echo '[{"external_id":"pr-1","title":"PR 1","description":"","url":"https://github.com/org/repo-a/pull/1","status":"backlog","tag":"pr-review"}]'"#;
        // group_by_repo lives on the epic now, not on the trigger call: the cycle
        // reads it from the DB so a manual refresh cannot use a stale flag.
        db.patch_epic(
            epic.id,
            &db::EpicPatch::new()
                .feed_command(Some(cmd))
                .group_by_repo(true),
        )
        .await
        .unwrap();

        let (tx, mut rx) = mpsc::unbounded_channel();
        let rt = make_runtime(db.clone(), tx, Arc::new(MockProcessRunner::new(vec![]))).await;

        rt.exec_trigger_epic_feed(epic.id, "Reviews".to_string());

        let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
            .await
            .expect("timed out")
            .expect("channel closed");
        assert!(
            matches!(
                msg,
                Message::Feed(crate::tui::messages::FeedMessage::Refreshed { count: 1, .. })
            ),
            "expected FeedRefreshed with count=1, got: {msg:?}"
        );

        let parent_tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
        assert_eq!(
            parent_tasks.len(),
            0,
            "parent should have no direct tasks when group_by_repo=true"
        );

        let sub_epics = db.list_sub_epics(epic.id).await.unwrap();
        assert_eq!(sub_epics.len(), 1);
        assert_eq!(sub_epics[0].title, "repo-a");
        let sub_tasks = db.list_tasks_for_epic(sub_epics[0].id).await.unwrap();
        assert_eq!(sub_tasks.len(), 1);
    }

    /// Bug A: a MANUAL "r" refresh of a reviews_parent epic must dispatch by
    /// feed_role exactly like the auto-poll path — routing the emission into the
    /// My/Team/Bots subtree — and must NOT flat-upsert into the parent. Regression
    /// guard for the parent-flat routing bug.
    #[tokio::test]
    async fn exec_trigger_epic_feed_reviews_parent_routes_into_subtree() {
        let db = test_db().await;
        let epic = db.create_epic("Reviews", "", None).await.unwrap();
        // A single direct-request PR: route(signals) => my_reviews.
        let cmd = r#"echo '[{"external_id":"pr-1","title":"PR 1","description":"","url":"https://github.com/org/repo/pull/1","status":"backlog","tag":"pr-review","signals":["direct-request"]}]'"#;
        // group_by_repo stays false; dispatch must key on feed_role, not that flag.
        db.patch_epic(
            epic.id,
            &db::EpicPatch::new()
                .feed_role(crate::models::FeedRole::ReviewsParent)
                .feed_command(Some(cmd)),
        )
        .await
        .unwrap();

        let (tx, mut rx) = mpsc::unbounded_channel();
        let rt = make_runtime(db.clone(), tx, Arc::new(MockProcessRunner::new(vec![]))).await;

        rt.exec_trigger_epic_feed(epic.id, "Reviews".to_string());

        let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
            .await
            .expect("timed out")
            .expect("channel closed");
        assert!(
            matches!(
                msg,
                Message::Feed(crate::tui::messages::FeedMessage::Refreshed { count: 1, .. })
            ),
            "expected FeedRefreshed with count=1, got: {msg:?}"
        );

        // No feed task may be stranded flat on the reviews_parent epic.
        let parent_tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
        assert!(
            parent_tasks.iter().all(|t| t.external_id.is_none()),
            "manual reviews_parent refresh must route, not flat-upsert onto the parent"
        );

        // The PR must land in the My Reviews role sub-epic.
        let subs = db.list_sub_epics(epic.id).await.unwrap();
        let my = subs
            .iter()
            .find(|e| e.feed_role == crate::models::FeedRole::MyReviews)
            .expect("My Reviews role sub-epic ensured by the role router");
        let my_tasks = db.list_tasks_for_epic(my.id).await.unwrap();
        assert_eq!(
            my_tasks.len(),
            1,
            "direct-request PR routed into My Reviews"
        );
        assert_eq!(my_tasks[0].external_id.as_deref(), Some("pr-1"));
    }
}

mod epic_auto_dispatch_and_group_by_repo {
    use super::*;

    #[tokio::test]
    async fn exec_toggle_epic_auto_dispatch_sets_flag_to_false() {
        let (rt, mut app) = test_runtime().await;
        let epic = rt
            .db_write()
            .create_epic("AutoDispatch Epic", "desc", None)
            .await
            .unwrap();
        // Default is false; opt in first so the toggle-to-false is meaningful.
        rt.db_write()
            .patch_epic(epic.id, &db::EpicPatch::new().auto_dispatch(true))
            .await
            .unwrap();
        let enabled = rt.database.get_epic(epic.id).await.unwrap().unwrap();
        assert!(enabled.auto_dispatch);

        rt.exec_toggle_epic_auto_dispatch(&mut app, epic.id, false)
            .await;

        let updated = rt.database.get_epic(epic.id).await.unwrap().unwrap();
        assert!(!updated.auto_dispatch);
        assert!(app.error_popup().is_none());
    }

    #[tokio::test]
    async fn exec_toggle_epic_auto_dispatch_sets_flag_to_true() {
        let (rt, mut app) = test_runtime().await;
        let epic = rt
            .db_write()
            .create_epic("AutoDispatch Epic", "desc", None)
            .await
            .unwrap();
        rt.db_write()
            .patch_epic(epic.id, &db::EpicPatch::new().auto_dispatch(false))
            .await
            .unwrap();

        rt.exec_toggle_epic_auto_dispatch(&mut app, epic.id, true)
            .await;

        let updated = rt.database.get_epic(epic.id).await.unwrap().unwrap();
        assert!(updated.auto_dispatch);
        assert!(app.error_popup().is_none());
    }

    #[tokio::test]
    async fn exec_toggle_epic_group_by_repo_sets_flag_to_true() {
        let (rt, mut app) = test_runtime().await;
        let epic = rt
            .db_write()
            .create_epic("GroupByRepo Epic", "desc", None)
            .await
            .unwrap();
        assert!(!epic.group_by_repo, "default group_by_repo should be false");

        rt.exec_toggle_epic_group_by_repo(&mut app, epic.id, true)
            .await;

        let updated = rt.database.get_epic(epic.id).await.unwrap().unwrap();
        assert!(updated.group_by_repo);
        assert!(app.error_popup().is_none());
    }

    #[tokio::test]
    async fn exec_toggle_epic_group_by_repo_sets_flag_to_false() {
        let (rt, mut app) = test_runtime().await;
        let epic = rt
            .db_write()
            .create_epic("GroupByRepo Epic", "desc", None)
            .await
            .unwrap();
        rt.db_write()
            .patch_epic(epic.id, &db::EpicPatch::new().group_by_repo(true))
            .await
            .unwrap();

        rt.exec_toggle_epic_group_by_repo(&mut app, epic.id, false)
            .await;

        let updated = rt.database.get_epic(epic.id).await.unwrap().unwrap();
        assert!(!updated.group_by_repo);
        assert!(app.error_popup().is_none());
    }
}

mod epic_group_by_repo_migration {
    use super::*;

    #[tokio::test]
    async fn toggle_group_by_repo_on_regroups_existing_tasks() {
        let (rt, mut app) = test_runtime().await;
        let root = rt.db_write().create_epic("root", "", None).await.unwrap();
        // Add a backlog task on root with repo "/x/alpha".
        let _task_id = rt
            .db_write()
            .create_task(CreateTaskRequest {
                title: "task on root",
                description: "",
                repo_path: "/x/alpha",
                plan: None,
                status: models::TaskStatus::Backlog,
                base_branch: "main",
                epic_id: Some(root.id),
                sort_order: None,
                tag: None,
                wrap_up_mode: None,
                auto_run_plan: false,
                phoenix: false,
            })
            .await
            .unwrap();

        rt.exec_toggle_epic_group_by_repo(&mut app, root.id, true)
            .await;

        assert!(
            rt.database
                .list_tasks_for_epic(root.id)
                .await
                .unwrap()
                .is_empty(),
            "root tasks should have been migrated into sub-epics"
        );
        assert_eq!(
            rt.database.list_sub_epics(root.id).await.unwrap().len(),
            1,
            "one sub-epic should exist for the repo group"
        );
        assert!(app.error_popup().is_none());
    }
}
