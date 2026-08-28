use super::*;
use crate::models::test_tmux_window;

mod frame_rate_cap {
    use super::*;

    #[test]
    fn min_frame_interval_is_16ms() {
        assert_eq!(MIN_FRAME_INTERVAL, Duration::from_millis(16));
    }

    #[test]
    fn frame_ready_true_when_dirty_and_interval_elapsed() {
        assert!(
            frame_ready(Duration::from_millis(20), true),
            "should render when dirty and interval has elapsed"
        );
    }

    #[test]
    fn frame_ready_false_when_interval_not_elapsed() {
        assert!(
            !frame_ready(Duration::from_millis(8), true),
            "should not render when interval has not elapsed even if dirty"
        );
    }

    #[test]
    fn frame_ready_false_when_not_dirty_even_if_interval_elapsed() {
        assert!(
            !frame_ready(Duration::from_millis(20), false),
            "should not render when not dirty even if interval has elapsed"
        );
    }

    #[test]
    fn frame_ready_false_when_zero_elapsed() {
        assert!(
            !frame_ready(Duration::ZERO, true),
            "should not render when no time has elapsed"
        );
    }

    #[test]
    fn frame_ready_true_at_exact_interval_boundary() {
        assert!(
            frame_ready(Duration::from_millis(16), true),
            "should render exactly at the 16ms boundary"
        );
    }
}

/// next_loop_event / apply_loop_event / run_loop
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// A tick interval whose first (and every) tick is far in the future, so it
/// never fires during a test — lets us assert which non-tick arm `select!`
/// chooses without the immediate-first-tick of a plain `interval`.
fn quiet_tick() -> tokio::time::Interval {
    let far = tokio::time::Instant::now() + Duration::from_secs(3600);
    tokio::time::interval_at(far, Duration::from_secs(3600))
}

fn status_info(text: &str) -> Message {
    Message::System(crate::tui::messages::SystemMessage::StatusInfo(
        text.to_string(),
    ))
}

/// `next_loop_event` drains queued async messages FIFO — the order they were
/// sent is the order the loop observes them.
#[tokio::test]
async fn next_loop_event_drains_messages_in_order() {
    let (_key_tx, mut key_rx) = mpsc::unbounded_channel::<KeyEvent>();
    let (msg_tx, mut msg_rx) = mpsc::unbounded_channel::<Message>();
    let (_mcp_tx, mut mcp_rx) = mpsc::unbounded_channel::<mcp::McpEvent>();
    let mut tick = quiet_tick();

    msg_tx.send(status_info("first")).unwrap();
    msg_tx.send(status_info("second")).unwrap();

    let mut seen = Vec::new();
    for _ in 0..2 {
        match next_loop_event(&mut key_rx, &mut msg_rx, &mut mcp_rx, &mut tick).await {
            LoopEvent::Message(Message::System(
                crate::tui::messages::SystemMessage::StatusInfo(s),
            )) => seen.push(s),
            other => panic!("expected a StatusInfo message, got something else: {other:?}"),
        }
    }

    assert_eq!(seen, vec!["first".to_string(), "second".to_string()]);
}

/// A `Message` loop event is applied to the app and marks it dirty so the next
/// frame redraws.
#[tokio::test]
async fn apply_loop_event_message_applies_and_marks_dirty() {
    let (rt, mut app) = test_runtime().await;
    app.dirty = false;

    let cmds = apply_loop_event(&mut app, LoopEvent::Message(status_info("hello")), &rt);

    assert!(
        app.dirty,
        "applying an async message must mark the app dirty"
    );
    assert!(
        cmds.is_empty(),
        "a status-info message produces no commands"
    );
    assert_eq!(app.status_message(), Some("hello"));
}

/// A `Tick` loop event routes through `App::handle_tick`, which emits a single
/// batched window-staleness check for the windowed tasks on the board.
#[tokio::test]
async fn apply_loop_event_tick_triggers_window_sweep() {
    let db = test_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "windowed",
            description: "",
            repo_path: "/repo",
            plan: None,
            status: models::TaskStatus::Running,
            epic_id: None,
            sort_order: None,
            tag: None,
            base_branch: "main",
            wrap_up_mode: None,
            auto_run_plan: false,
            phoenix: false,
        })
        .await
        .unwrap();
    // Give the task a live tmux window so the tick has something to sweep.
    db.patch_task(
        id,
        &crate::db::TaskPatch::new().tmux_window(Some(&test_tmux_window("dispatch:1"))),
    )
    .await
    .unwrap();

    let (tx, _rx) = mpsc::unbounded_channel();
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
    let rt = make_runtime(db.clone(), tx, runner).await;
    let mut app = App::new(db.list_all().await.unwrap());

    let cmds = apply_loop_event(&mut app, LoopEvent::Tick, &rt);

    let batch_checks = cmds
        .iter()
        .filter(|c| {
            matches!(
                c,
                Command::Task(crate::tui::commands::TaskCommand::BatchCheckWindows { .. })
            )
        })
        .count();
    assert_eq!(
        batch_checks, 1,
        "tick must emit exactly one batched window-staleness check"
    );
}

fn count_archive_stale(cmds: &[Command]) -> usize {
    cmds.iter()
        .filter(|c| {
            matches!(
                c,
                Command::Learning(crate::tui::commands::LearningCommand::ArchiveStale)
            )
        })
        .count()
}

/// A `Tick` emits the stale-learning cleanup command when the cleanup interval
/// has elapsed (tracker = None means never run). See
/// docs/specs/learnings.allium: ArchiveStaleLearning.
#[tokio::test]
async fn apply_loop_event_tick_emits_stale_cleanup_when_interval_elapsed() {
    let db = test_db().await;
    let (tx, _rx) = mpsc::unbounded_channel();
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
    let rt = make_runtime(db.clone(), tx, runner).await;
    let mut app = App::new(vec![]);
    app.last_stale_cleanup_at = None;

    let cmds = apply_loop_event(&mut app, LoopEvent::Tick, &rt);

    assert_eq!(
        count_archive_stale(&cmds),
        1,
        "tick must emit exactly one ArchiveStale command when the interval has elapsed"
    );
    assert!(
        app.last_stale_cleanup_at.is_some(),
        "the sweep must record its run time to space out subsequent sweeps"
    );
}

/// A `Tick` does NOT re-emit the stale-learning cleanup command when the last
/// sweep ran just now (interval not yet elapsed).
#[tokio::test]
async fn apply_loop_event_tick_skips_stale_cleanup_when_interval_not_elapsed() {
    let db = test_db().await;
    let (tx, _rx) = mpsc::unbounded_channel();
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
    let rt = make_runtime(db.clone(), tx, runner).await;
    let mut app = App::new(vec![]);
    app.last_stale_cleanup_at = Some(std::time::Instant::now());

    let cmds = apply_loop_event(&mut app, LoopEvent::Tick, &rt);

    assert_eq!(
        count_archive_stale(&cmds),
        0,
        "tick must not re-emit ArchiveStale before the interval has elapsed"
    );
}

/// An MCP `Refresh` event marks the app dirty and produces no immediate
/// commands (the DB refresh is spawned; its result returns via a later message).
#[tokio::test]
async fn apply_loop_event_mcp_refresh_spawns_and_yields_no_commands() {
    let (rt, mut app) = test_runtime().await;
    app.dirty = false;

    let cmds = apply_loop_event(&mut app, LoopEvent::Mcp(mcp::McpEvent::Refresh), &rt);

    assert!(app.dirty, "an MCP event must mark the app dirty");
    assert!(
        cmds.is_empty(),
        "Refresh spawns a background refresh and returns no synchronous commands"
    );
}

/// An MCP `TaskChanged` event spawns a targeted refresh of just that task
/// (`spawn_refresh_task`), not a full board refresh.
#[tokio::test]
async fn apply_loop_event_mcp_task_changed_spawns_a_targeted_refresh() {
    let db = test_db().await;
    let (msg_tx, mut msg_rx) = mpsc::unbounded_channel();
    let rt = make_runtime(db.clone(), msg_tx, Arc::new(MockProcessRunner::new(vec![]))).await;
    let mut app = empty_app();
    rt.exec_insert_task(
        &mut app,
        tui::TaskDraft {
            title: "Changed elsewhere".into(),
            description: "".into(),
            repo_path: "/repo".into(),
            ..Default::default()
        },
        None,
    )
    .await;
    let id = app.tasks()[0].id;

    let cmds = apply_loop_event(
        &mut app,
        LoopEvent::Mcp(mcp::McpEvent::TaskChanged(id)),
        &rt,
    );

    assert!(cmds.is_empty(), "the refresh is spawned, not queued");
    let msg = recv_msg(&mut msg_rx).await;
    assert!(
        matches!(
            &msg,
            Message::Task(crate::tui::messages::TaskMessage::Updated(t)) if t.id == id
        ),
        "expected an Updated message for the changed task, got: {msg:?}"
    );
}

/// An MCP `EpicChanged` event spawns a targeted refresh of just that epic
/// (`spawn_refresh_epic`), and also invalidates the feed cache so a newly
/// added feed_command becomes visible on the next poll.
#[tokio::test]
async fn apply_loop_event_mcp_epic_changed_spawns_a_targeted_refresh() {
    let db = test_db().await;
    let epic = db.create_epic("Changed elsewhere", "", None).await.unwrap();
    let (msg_tx, mut msg_rx) = mpsc::unbounded_channel();
    let rt = make_runtime(db, msg_tx, Arc::new(MockProcessRunner::new(vec![]))).await;
    let mut app = empty_app();
    let mut invalidate_rx = rt
        .feed_invalidate_tx
        .as_ref()
        .expect("make_runtime always wires up a live feed runner")
        .subscribe();

    let cmds = apply_loop_event(
        &mut app,
        LoopEvent::Mcp(mcp::McpEvent::EpicChanged(epic.id)),
        &rt,
    );

    assert!(cmds.is_empty(), "the refresh is spawned, not queued");
    tokio::time::timeout(TEST_TIMEOUT, invalidate_rx.changed())
        .await
        .expect("EpicChanged must invalidate the feed cache within the timeout")
        .expect("the sender must still be alive");
    let msg = recv_msg(&mut msg_rx).await;
    assert!(
        matches!(
            &msg,
            Message::Epic(crate::tui::messages::EpicMessage::Updated(e)) if e.id == epic.id
        ),
        "expected an Updated message for the changed epic, got: {msg:?}"
    );
}

/// Driving `run_loop` (on a headless `TestBackend`) with a `q`→`y` quit
/// sequence exits the loop cleanly, after draining the queued key events.
#[tokio::test]
async fn run_loop_exits_cleanly_on_quit_sequence() {
    let (mut rt, mut app) = test_runtime().await;
    // Don't start the real feed poll loop in a unit test.
    rt.feed_runner = None;

    let (key_tx, mut key_rx) = mpsc::unbounded_channel::<KeyEvent>();
    let (_msg_tx, mut msg_rx) = mpsc::unbounded_channel::<Message>();
    let (_mcp_tx, mut mcp_rx) = mpsc::unbounded_channel::<mcp::McpEvent>();
    let mut tick = quiet_tick();

    // q opens the quit confirm; y confirms. FIFO ordering guarantees q first.
    key_tx.send(KeyEvent::from(KeyCode::Char('q'))).unwrap();
    key_tx.send(KeyEvent::from(KeyCode::Char('y'))).unwrap();

    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();

    let result = tokio::time::timeout(
        TEST_TIMEOUT,
        run_loop(
            &mut app,
            &mut terminal,
            &mut key_rx,
            &mut msg_rx,
            &mut mcp_rx,
            &mut tick,
            &mut rt,
        ),
    )
    .await
    .expect("run_loop should exit well within the timeout");

    assert!(result.is_ok(), "run_loop returned an error: {result:?}");
    assert!(app.should_quit(), "the quit sequence must set should_quit");
}

/// A live `feed_runner` is started (`.take()` + `start()`) exactly once, at
/// the top of `run_loop` — the sibling `run_loop_exits_cleanly_on_quit_sequence`
/// test deliberately nils it out to avoid this, so it never drives that arm.
#[tokio::test]
async fn run_loop_starts_a_live_feed_runner_before_the_first_command() {
    let (mut rt, mut app) = test_runtime().await;
    assert!(
        rt.feed_runner.is_some(),
        "precondition: the fixture wires up a feed runner"
    );

    let (key_tx, mut key_rx) = mpsc::unbounded_channel::<KeyEvent>();
    let (_msg_tx, mut msg_rx) = mpsc::unbounded_channel::<Message>();
    let (_mcp_tx, mut mcp_rx) = mpsc::unbounded_channel::<mcp::McpEvent>();
    let mut tick = quiet_tick();

    key_tx.send(KeyEvent::from(KeyCode::Char('q'))).unwrap();
    key_tx.send(KeyEvent::from(KeyCode::Char('y'))).unwrap();

    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();

    let result = tokio::time::timeout(
        TEST_TIMEOUT,
        run_loop(
            &mut app,
            &mut terminal,
            &mut key_rx,
            &mut msg_rx,
            &mut mcp_rx,
            &mut tick,
            &mut rt,
        ),
    )
    .await
    .expect("run_loop should exit well within the timeout");

    assert!(result.is_ok(), "run_loop returned an error: {result:?}");
    assert!(
        rt.feed_runner.is_none(),
        "run_loop must take() the feed runner to start it in the background"
    );
}

/// `execute_commands` drains not just the commands it is handed but every
/// command those handlers cascade into (`commands::dispatch` can return
/// follow-on commands, which `queue.extend(extra)` feeds back into the same
/// loop) — a test that only calls `commands::dispatch` once would miss that.
mod execute_commands {
    use super::*;
    use crate::tui::commands::EditorCommand;

    async fn run(rt: &TuiRuntime, app: &mut App, cmds: Vec<Command>) {
        let (_key_tx, mut key_rx) = mpsc::unbounded_channel::<KeyEvent>();
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        super::execute_commands(app, cmds, rt, &mut terminal, &mut key_rx)
            .await
            .unwrap();
    }

    /// `Editor(FinalizeResult)` cascades into a follow-on `Task` command
    /// (`exec_finalize_editor_result`'s return value) — the queue must drain
    /// that cascade, not just the top-level command it was handed.
    #[tokio::test]
    async fn drains_a_cascaded_follow_on_command() {
        let (rt, mut app) = test_runtime().await;
        let task = create_task_returning(
            &**rt.db_write(),
            "Old title",
            "Old description",
            "/repo",
            None,
            models::TaskStatus::Backlog,
        )
        .await
        .unwrap();
        app.update(Message::Task(crate::tui::messages::TaskMessage::Refresh(
            vec![task.clone()],
        )));

        let edited = "--- TITLE ---\nNew title\n\
            --- DESCRIPTION ---\nNew description\n\
            --- REPO_PATH ---\n\n\
            --- STATUS ---\n\n\
            --- PLAN ---\n\n\
            --- TAG ---\n\n\
            --- BASE_BRANCH ---\n\n";

        run(
            &rt,
            &mut app,
            vec![Command::Editor(EditorCommand::FinalizeResult {
                kind: crate::tui::EditKind::TaskEdit(Box::new(task.clone())),
                outcome: crate::tui::EditorOutcome::Saved(edited.into()),
            })],
        )
        .await;

        let stored = rt.database.get_task(task.id).await.unwrap().unwrap();
        assert_eq!(
            stored.title, "New title",
            "the cascaded follow-on command must reach the DB, not just the \
             top-level FinalizeResult handler"
        );
    }

    /// Multiple independent top-level commands are all executed, in order —
    /// not just the first one popped from the queue.
    #[tokio::test]
    async fn executes_every_top_level_command_in_order() {
        let (rt, mut app) = test_runtime().await;
        rt.exec_insert_task(
            &mut app,
            tui::TaskDraft {
                title: "First".into(),
                description: "".into(),
                repo_path: "/repo".into(),
                ..Default::default()
            },
            None,
        )
        .await;
        let id = app.tasks()[0].id;

        run(
            &rt,
            &mut app,
            vec![
                Command::Settings(SettingsCommand::SaveRepoPath("/some/repo".into())),
                Command::Task(crate::tui::commands::TaskCommand::Delete(id)),
            ],
        )
        .await;

        assert!(
            app.repo_paths().contains(&"/some/repo".to_string()),
            "the first queued command must run"
        );
        assert!(
            rt.database.get_task(id).await.unwrap().is_none(),
            "the second queued command must also run, not just the first"
        );
    }
}

/// The three result arms
mod run_blocking_dispatch {
    use super::*;

    /// Receive the next message or fail the test rather than hang forever.
    #[tokio::test]
    async fn run_blocking_dispatch_sends_dispatched_on_success() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        tasks::run_blocking_dispatch(models::TaskId(7), "Dispatch", true, tx, || {
            Ok(models::DispatchResult {
                worktree_path: "/wt".into(),
                tmux_window: test_tmux_window("win"),
            })
        });

        match recv_msg(&mut rx).await {
            Message::Task(crate::tui::messages::TaskMessage::Dispatched {
                id,
                worktree,
                tmux_window,
                switch_focus,
            }) => {
                assert_eq!(id, models::TaskId(7));
                assert_eq!(worktree, "/wt");
                assert_eq!(tmux_window.as_str(), "win");
                assert!(switch_focus);
            }
            other => panic!("expected Dispatched, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_blocking_dispatch_reports_panics_as_dispatch_failure() {
        // The panic arm is unreachable from production code on demand: it only
        // fires when the dispatch closure itself unwinds. Without this test the
        // downcast-and-report logic is never executed.
        let (tx, mut rx) = mpsc::unbounded_channel();
        tasks::run_blocking_dispatch(models::TaskId(9), "Dispatch", false, tx, || {
            panic!("worktree exploded")
        });

        match recv_msg(&mut rx).await {
            Message::Task(crate::tui::messages::TaskMessage::DispatchFailed(id)) => {
                assert_eq!(id, models::TaskId(9));
            }
            other => panic!("expected DispatchFailed, got {other:?}"),
        }
        match recv_msg(&mut rx).await {
            Message::System(crate::tui::messages::SystemMessage::Error(msg)) => {
                assert!(
                    msg.contains("panicked") && msg.contains("worktree exploded"),
                    "the panic payload must be surfaced to the user, got {msg:?}"
                );
            }
            other => panic!("expected a System error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_blocking_dispatch_reports_non_string_panic_payload_as_unknown() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        tasks::run_blocking_dispatch(models::TaskId(11), "Dispatch", false, tx, || {
            std::panic::panic_any(42_u32)
        });

        assert!(matches!(
            recv_msg(&mut rx).await,
            Message::Task(crate::tui::messages::TaskMessage::DispatchFailed(_))
        ));
        match recv_msg(&mut rx).await {
            Message::System(crate::tui::messages::SystemMessage::Error(msg)) => {
                assert!(
                    msg.contains("unknown"),
                    "an undowncastable payload must fall back to 'unknown', got {msg:?}"
                );
            }
            other => panic!("expected a System error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn spawn_refresh_from_db_sends_task_and_epic_refresh_messages() {
        // `do_full_board_refresh` is the *unguarded* twin of `exec_refresh_from_db`
        // (see the doc comments on both). It is only reachable through the
        // `spawn_refresh_*` helpers, so it was never executed by any test.
        let db = test_db().await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
        let rt = make_runtime(db, tx, runner).await;
        create_task_returning(
            &**rt.db_write(),
            "Refreshed",
            "desc",
            "/repo",
            None,
            models::TaskStatus::Backlog,
        )
        .await
        .unwrap();

        rt.spawn_refresh_from_db().await.unwrap();

        match recv_msg(&mut rx).await {
            Message::Task(crate::tui::messages::TaskMessage::Refresh(tasks)) => {
                assert_eq!(tasks.len(), 1);
            }
            other => panic!("expected a task Refresh, got {other:?}"),
        }
        assert!(matches!(
            recv_msg(&mut rx).await,
            Message::Epic(crate::tui::messages::EpicMessage::Refresh(_))
        ));
    }
}
