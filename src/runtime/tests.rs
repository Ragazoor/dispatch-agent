#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;

// `db` is the concrete `Arc<Database>` in this fixture (see `test_db`), so the
// store traits must be in scope for their methods to resolve on it.
use crate::db::{CreateLearningRow, CreateTaskRequest, Database, EpicCrud, EpicRead, TaskCrud};
use crate::dispatch::mock_sequence::DispatchScript;
use crate::process::MockProcessRunner;

/// Timeout for async receive assertions in tests.
const TEST_TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
async fn db_error_formats_consistently() {
    assert_eq!(
        TuiRuntime::db_error("creating task", "disk full"),
        "DB error creating task: disk full"
    );
}

#[test]
fn startup_commands_prime_the_budget_snapshot() {
    // App::new leaves budget: None and tick_budget_poll increments before
    // comparing, so the first tick-driven poll is BUDGET_POLL_TICKS away. Without
    // a startup read the badge is blank for the first ~10s of every session even
    // when a warm snapshot is already on disk. dispatch.allium:
    // TokenBudgetIndicator
    // (@guarantee RefreshedAtStartupThenPeriodicallyNoRedrawWhenUnchanged).
    assert!(
        startup_commands().iter().any(|c| matches!(
            c,
            Command::Budget(crate::tui::commands::BudgetCommand::Refresh)
        )),
        "startup must read the budget snapshot once before the first poll"
    );
}

#[tokio::test]
async fn setup_tmux_for_tui_renames_window_and_binds_key() {
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // current_pane_id (display-message)
        MockProcessRunner::ok(), // rename_window
        MockProcessRunner::ok(), // bind_key (space)
        MockProcessRunner::ok(), // bind_key (agent-tree toggle)
    ]);
    setup_tmux_for_tui(&mock);
    let calls = mock.recorded_calls();
    assert_eq!(calls.len(), 4);
    assert_eq!(calls[0].1, vec!["display-message", "-p", "#{pane_id}"]);
    assert_eq!(calls[1].1, vec!["rename-window", "-t", "", TUI_WINDOW_NAME]);
    // `=` anchors the target to an exact name match. This binding is executed by
    // tmux itself, so it cannot use the pane-ID resolution `tmux::window_target`
    // applies elsewhere — a pane ID captured at bind time would go stale. tmux's
    // `=` sigil does work for `select-window` (verified against 3.5a), and
    // without it a window whose name merely starts with the TUI window's name
    // could absorb the jump. See the `TmuxWindowTargetedExactly` invariant in
    // docs/specs/dispatch.allium.
    assert_eq!(
        calls[2].1,
        vec![
            "bind-key",
            "space",
            &format!("select-window -t ={TUI_WINDOW_NAME}")
        ]
    );
    assert_eq!(
        calls[3].1,
        vec!["bind-key", AGENT_TREE_TOGGLE_KEY, AGENT_TREE_TOGGLE_COMMAND]
    );
}

#[tokio::test]
async fn teardown_tmux_for_tui_unbinds_and_restores_name() {
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // unbind_key (space)
        MockProcessRunner::ok(), // unbind_key (agent-tree toggle)
        MockProcessRunner::ok(), // rename_window
    ])
    .with_windows(&[TUI_WINDOW_NAME]);
    teardown_tmux_for_tui(Some("my-shell"), &mock);
    let calls = mock.recorded_calls();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0].1, vec!["unbind-key", "space"]);
    assert_eq!(calls[1].1, vec!["unbind-key", AGENT_TREE_TOGGLE_KEY]);
    // The rename targets the TUI window by its resolved pane ID — see
    // `tmux::window_target`. Only `my-shell`, the *new* name, stays a name.
    assert_eq!(
        calls[2].1,
        vec![
            "rename-window",
            "-t",
            &mock.pane_id_of(TUI_WINDOW_NAME),
            "my-shell",
        ]
    );
}

#[tokio::test]
async fn teardown_tmux_for_tui_skips_rename_when_no_original_name() {
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // unbind_key (space)
        MockProcessRunner::ok(), // unbind_key (agent-tree toggle)
    ]);
    teardown_tmux_for_tui(None, &mock);
    let calls = mock.recorded_calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].1, vec!["unbind-key", "space"]);
    assert_eq!(calls[1].1, vec!["unbind-key", AGENT_TREE_TOGGLE_KEY]);
}

/// One in-memory SQLite database, shared by every service the fixture builds.
///
/// Returns the concrete `Arc<Database>` rather than `Arc<dyn db::TaskStore>` so
/// `make_runtime` can hand the *same* handle to both `TaskService` and
/// `TodoService` — the two trait objects (`TaskStore` / `TodoStore`) can only be
/// derived from a concrete type. A previous version gave `todo_svc` its own
/// database, which silently hid every cross-entity behaviour between todos and
/// tasks (a todo linked to a task id that does not exist in the todo database
/// fails the `todos.task_id → tasks(id)` foreign key).
pub(super) async fn test_db() -> Arc<Database> {
    Arc::new(Database::open_in_memory().await.unwrap())
}

/// Persist `cmd` as `epic_id`'s feed command.
///
/// The manual trigger reads the command (and feed_role, and group_by_repo) from
/// the epic itself rather than from its caller, so a test that only hands a
/// command string to `exec_trigger_epic_feed` exercises nothing — the cycle
/// fails with "epic has no feed command". That mirrors production, where the
/// "r" key is only live for an epic whose feed_command is set.
/// Assert `msg` is a feed failure that reached the failure it is testing for,
/// rather than tripping over the epic's own configuration first.
///
/// Deliberately stricter than `matches!(.., Failed { .. })`. The cycle has six
/// failure buckets (feeds.allium: FeedCommandFailure) and two of them are
/// config ones — "epic no longer exists", "epic has no feed command" — which a
/// test that forgets `set_feed_command` hits instead of the bucket it is named
/// after. A bare Failed match cannot tell the difference, and three of these
/// tests silently went vacuous exactly that way.
///
/// `needle` is `None` where the failure legitimately carries no text: a bare
/// `exit 1` writes nothing to stderr, so its error string is empty.
fn assert_feed_failed_because(msg: &Message, needle: Option<&str>, what: &str) {
    let Message::Feed(crate::tui::messages::FeedMessage::Failed { error, .. }) = msg else {
        panic!("{what} should produce FeedMessage::Failed, got: {msg:?}");
    };
    for config_bucket in ["no feed command", "no longer exists"] {
        assert!(
            !error.contains(config_bucket),
            "{what} failed for a CONFIG reason ({error:?}) -- the test never got \
             as far as {what}. Is set_feed_command missing?"
        );
    }
    if let Some(needle) = needle {
        assert!(
            error.contains(needle),
            "{what} must fail with an error mentioning {needle:?}, got: {error:?}"
        );
    }
}

pub(super) async fn set_feed_command(
    db: &Arc<Database>,
    epic_id: crate::models::EpicId,
    cmd: &str,
) {
    db.patch_epic(
        epic_id,
        &crate::db::EpicPatch::new().feed_command(Some(cmd)),
    )
    .await
    .expect("failed to set feed command");
}

pub(super) async fn make_runtime(
    db: Arc<Database>,
    tx: mpsc::UnboundedSender<Message>,
    runner: Arc<dyn ProcessRunner>,
) -> TuiRuntime {
    let (feed_tx, _) = mpsc::unbounded_channel();
    let store: Arc<dyn db::TaskStore> = db.clone();
    let feed_runner = crate::feed::FeedRunner::new(store.clone(), feed_tx, runner.clone());
    let feed_invalidate_tx = Some(feed_runner.epic_invalidate_tx());
    // Taken from THIS runner, so a runtime built here serialises against its
    // own feed poller exactly as production does. A fresh FeedSyncGuard would
    // compile and silently serialise nothing.
    let feed_sync_guard = feed_runner.sync_guard();
    TuiRuntime {
        task_svc: Arc::new(crate::service::TaskService::new(
            store.clone(),
            runner.clone(),
        )),
        epic_svc: Arc::new(crate::service::EpicService::new(store.clone())),
        todo_svc: Arc::new(crate::service::TodoService::new(db.clone())),
        feed_runner: Some(feed_runner),
        feed_invalidate_tx,
        feed_sync_guard,
        learning_svc: Arc::new(crate::service::MockLearningService),
        feed_db: store.clone(),
        database: store,
        msg_tx: tx,
        runner,
        editor_session: Arc::new(std::sync::Mutex::new(None)),
        emb_svc: crate::service::embeddings::EmbeddingService::new_noop(),
        last_change_count: std::sync::atomic::AtomicI64::new(-1),
        budget_snapshot_path: std::path::PathBuf::from("/nonexistent-test-path/rate-limits.json"),
    }
}

async fn test_runtime() -> (TuiRuntime, App) {
    let db = test_db().await;
    let (tx, _rx) = mpsc::unbounded_channel();
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
    let rt = make_runtime(db.clone(), tx, runner).await;
    let tasks = db.list_all().await.unwrap();
    let app = App::new(tasks);
    (rt, app)
}

/// Helper: create_task + get_task in one step (replaces removed trait method).
async fn create_task_returning(
    db: &dyn db::TaskStore,
    title: &str,
    description: &str,
    repo_path: &str,
    plan: Option<&str>,
    status: models::TaskStatus,
) -> anyhow::Result<models::Task> {
    let id = db
        .create_task(CreateTaskRequest {
            title,
            description,
            repo_path,
            plan,
            status,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await?;
    db.get_task(id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Task {id} vanished after insert"))
}

#[tokio::test]
async fn exec_insert_task_adds_to_db_and_app() {
    let (rt, mut app) = test_runtime().await;
    rt.exec_insert_task(
        &mut app,
        tui::TaskDraft {
            title: "Test".into(),
            description: "Desc".into(),
            repo_path: "/repo".into(),
            ..Default::default()
        },
        None,
    )
    .await;
    assert_eq!(app.tasks().len(), 1);
    assert_eq!(app.tasks()[0].title, "Test");
    assert_eq!(rt.database.list_all().await.unwrap().len(), 1);
}

#[tokio::test]
async fn exec_delete_task_removes_from_db() {
    let (rt, mut app) = test_runtime().await;
    rt.exec_insert_task(
        &mut app,
        tui::TaskDraft {
            title: "Test".into(),
            description: "Desc".into(),
            repo_path: "/repo".into(),
            ..Default::default()
        },
        None,
    )
    .await;
    let id = app.tasks()[0].id;
    rt.exec_delete_task(&mut app, id).await;
    assert!(rt.database.list_all().await.unwrap().is_empty());
}

#[tokio::test]
async fn exec_persist_task_saves_status_to_db() {
    let (rt, mut app) = test_runtime().await;
    rt.exec_insert_task(
        &mut app,
        tui::TaskDraft {
            title: "Test".into(),
            description: "Desc".into(),
            repo_path: "/repo".into(),
            ..Default::default()
        },
        None,
    )
    .await;
    let mut task = app.tasks()[0].clone();
    task.status = models::TaskStatus::Running;
    task.sub_status = models::SubStatus::Active;
    task.worktree = Some("/repo/.worktrees/1-test".into());
    rt.exec_persist_task(&mut app, task).await;
    let db_task = rt
        .database
        .get_task(app.tasks()[0].id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(db_task.status, models::TaskStatus::Running);
    assert_eq!(db_task.worktree.as_deref(), Some("/repo/.worktrees/1-test"));
}

#[tokio::test]
async fn exec_persist_task_preserves_sub_status() {
    let (rt, mut app) = test_runtime().await;
    rt.exec_insert_task(
        &mut app,
        tui::TaskDraft {
            title: "PR Task".into(),
            description: "Desc".into(),
            repo_path: "/repo".into(),
            ..Default::default()
        },
        None,
    )
    .await;
    let id = app.tasks()[0].id;
    // Put task in Review+Approved state in DB, then sync to app
    let url = models::TaskUrl::new("https://github.com/org/repo/pull/42", models::UrlType::Pr);
    rt.db_write()
        .patch_task(
            id,
            &db::TaskPatch::new()
                .status(models::TaskStatus::Review)
                .sub_status(models::SubStatus::Approved)
                .url(Some(&url)),
        )
        .await
        .unwrap();
    rt.exec_refresh_from_db(&mut app).await;
    assert_eq!(app.tasks()[0].sub_status, models::SubStatus::Approved);

    // Persist the in-memory task (simulates handle_pr_review_state saving after PR approval)
    let task = app.tasks()[0].clone();
    rt.exec_persist_task(&mut app, task).await;

    // sub_status must survive the round-trip to DB
    let db_task = rt.database.get_task(id).await.unwrap().unwrap();
    assert_eq!(db_task.sub_status, models::SubStatus::Approved);
}

/// Persist must not clobber `last_pre_tool_use_at`. Hooks own that column —
/// if the TUI's in-memory snapshot races against a fresh hook write and wins,
/// the task flickers Active → Stale on the next tick.
#[tokio::test]
async fn exec_persist_task_does_not_overwrite_last_pre_tool_use_at() {
    let (rt, mut app) = test_runtime().await;
    rt.exec_insert_task(
        &mut app,
        tui::TaskDraft {
            title: "Hook race".into(),
            description: "Desc".into(),
            repo_path: "/repo".into(),
            ..Default::default()
        },
        None,
    )
    .await;
    let id = app.tasks()[0].id;

    // Simulate the hook CLI writing a fresh PreToolUse timestamp directly.
    let hook_ts = chrono::Utc::now();
    rt.db_write()
        .patch_task(
            id,
            &db::TaskPatch::new()
                .status(models::TaskStatus::Running)
                .sub_status(models::SubStatus::Active)
                .last_pre_tool_use_at(Some(hook_ts)),
        )
        .await
        .unwrap();

    // In-memory still holds the pre-hook (NULL) snapshot. Persist it.
    let mut stale = app.tasks()[0].clone();
    stale.status = models::TaskStatus::Running;
    stale.sub_status = models::SubStatus::Active;
    stale.last_pre_tool_use_at = None;
    rt.exec_persist_task(&mut app, stale).await;

    // The hook's timestamp must survive — Persist owns status/sub_status,
    // not the activity stamp.
    let db_task = rt.database.get_task(id).await.unwrap().unwrap();
    assert_eq!(
        db_task.last_pre_tool_use_at.map(|t| t.timestamp()),
        Some(hook_ts.timestamp()),
        "Persist clobbered hook-written last_pre_tool_use_at"
    );
}

/// Regression for the whole-branch review finding: `exec_persist_task` must
/// write the service-computed `sort_order` into the in-memory board itself,
/// not just the DB — otherwise a freshly-completed task renders at the
/// bottom of Done (stale/`None` sort_order) until the next ~2s DB refresh,
/// the exact inverse of the completion-recency ordering this feature
/// promises. Drives the actual `exec_persist_task` runtime path (not a pure
/// sort function) and asserts on the in-memory board with no
/// `exec_refresh_from_db` call in between, to prove the write-back is
/// immediate.
#[tokio::test]
async fn exec_persist_task_writes_back_done_transition_sort_order_immediately() {
    let (rt, mut app) = test_runtime().await;
    rt.exec_insert_task(
        &mut app,
        tui::TaskDraft {
            title: "Finish me".into(),
            description: "Desc".into(),
            repo_path: "/repo".into(),
            ..Default::default()
        },
        None,
    )
    .await;
    let id = app.tasks()[0].id;

    // Put the task in Review in the DB (the state a task is normally in
    // right before ConfirmDone), without refreshing the in-memory board —
    // mirrors how the service fetches "prior" independently of what the TUI
    // happens to hold in memory.
    rt.db_write()
        .patch_task(id, &db::TaskPatch::new().status(models::TaskStatus::Review))
        .await
        .unwrap();

    // Simulate handle_confirm_done: the handler flips the *in-memory board*
    // task to Done (via find_task_mut) and hands a clone straight to
    // exec_persist_task. sort_order is still None — only the service computes
    // it, inside update_task.
    let mut task = app.tasks()[0].clone();
    task.status = models::TaskStatus::Done;
    assert_eq!(task.sort_order, None, "precondition: no sort_order yet");
    app.update(Message::Task(crate::tui::messages::TaskMessage::Updated(
        task.clone(),
    )));

    rt.exec_persist_task(&mut app, task).await;

    // Assert on the in-memory board directly — no exec_refresh_from_db call
    // in between — to prove the write-back is immediate, not deferred to
    // the next refresh.
    let in_memory = app.tasks().iter().find(|t| t.id == id).unwrap();
    assert_eq!(in_memory.status, models::TaskStatus::Done);
    assert!(
        in_memory.sort_order.is_some_and(|so| so < 0),
        "expected a negative completion-recency sort_order written back to \
         the in-memory board immediately, got {:?}",
        in_memory.sort_order
    );

    let db_task = rt.database.get_task(id).await.unwrap().unwrap();
    assert_eq!(
        in_memory.sort_order, db_task.sort_order,
        "in-memory sort_order must match what was actually persisted"
    );
}

/// The write-back's other direction: leaving Done clears `sort_order` back to
/// `None` (`sort_order_for_status_transition` returns `Some(None)`), and that
/// clear must reach the in-memory board immediately too — otherwise a task
/// moved Done→Review keeps its negative completion rank and stays pinned to
/// the top of Review until the next ~2s DB refresh. The entering-Done tests
/// (task and epic side) only cover the set direction.
#[tokio::test]
async fn exec_persist_task_writes_back_leaving_done_sort_order_clear_immediately() {
    let (rt, mut app) = test_runtime().await;
    rt.exec_insert_task(
        &mut app,
        tui::TaskDraft {
            title: "Un-finish me".into(),
            description: "Desc".into(),
            repo_path: "/repo".into(),
            ..Default::default()
        },
        None,
    )
    .await;
    let id = app.tasks()[0].id;

    // Put the task in Done *with* a completion-recency sort_order, then load
    // that state into the board — the state a task is in right before a
    // MoveTaskBackward out of Done.
    rt.db_write()
        .patch_task(
            id,
            &db::TaskPatch::new()
                .status(models::TaskStatus::Done)
                .sort_order(Some(-1_700_000_000_000)),
        )
        .await
        .unwrap();
    rt.exec_refresh_from_db(&mut app).await;
    assert_eq!(
        app.tasks()[0].sort_order,
        Some(-1_700_000_000_000),
        "precondition: board holds the completion-recency sort_order"
    );

    // Simulate handle_move_task_backward: mutate the board task to Review
    // (status + the default sub_status for it, as the handler does), then hand
    // a clone to exec_persist_task. The snapshot still carries the stale
    // negative sort_order — only the service knows to clear it.
    let mut task = app.tasks()[0].clone();
    task.status = models::TaskStatus::Review;
    task.sub_status = models::SubStatus::default_for(models::TaskStatus::Review);
    app.update(Message::Task(crate::tui::messages::TaskMessage::Updated(
        task.clone(),
    )));

    rt.exec_persist_task(&mut app, task).await;

    // No exec_refresh_from_db in between — the clear must be immediate.
    let in_memory = app.tasks().iter().find(|t| t.id == id).unwrap();
    assert_eq!(
        in_memory.sort_order, None,
        "leaving Done must clear the in-memory sort_order, not leave the \
         stale completion rank in place"
    );

    let db_task = rt.database.get_task(id).await.unwrap().unwrap();
    assert_eq!(
        in_memory.sort_order, db_task.sort_order,
        "in-memory sort_order must match what was actually persisted"
    );
}

/// The write-back must patch only `sort_order` onto the *live* board task, not
/// splice the caller's whole snapshot into the board. Splicing would re-impose
/// every field the snapshot holds — including `last_pre_tool_use_at`, which
/// hooks own — reintroducing in memory exactly the clobber `exec_persist_task`
/// already avoids on the DB write, and flipping the task to Stale on the next
/// tick.
#[tokio::test]
async fn exec_persist_task_write_back_does_not_clobber_fresher_board_fields() {
    let (rt, mut app) = test_runtime().await;
    rt.exec_insert_task(
        &mut app,
        tui::TaskDraft {
            title: "Hook race, in memory".into(),
            description: "Desc".into(),
            repo_path: "/repo".into(),
            ..Default::default()
        },
        None,
    )
    .await;
    let id = app.tasks()[0].id;

    // A hook wrote a fresh PreToolUse stamp and a refresh has already brought
    // it into the board.
    let hook_ts = chrono::Utc::now();
    rt.db_write()
        .patch_task(
            id,
            &db::TaskPatch::new()
                .status(models::TaskStatus::Review)
                .last_pre_tool_use_at(Some(hook_ts)),
        )
        .await
        .unwrap();
    rt.exec_refresh_from_db(&mut app).await;
    // Read the stamp back off the board rather than reusing `hook_ts`: the
    // column round-trips through SQLite at second precision.
    let board_stamp = app.tasks()[0].last_pre_tool_use_at;
    assert!(
        board_stamp.is_some(),
        "precondition: board holds the hook-written stamp"
    );

    // The board moves to Done; the snapshot handed to the persist is stale on
    // last_pre_tool_use_at (e.g. it was cloned before the hook refresh).
    let mut board_task = app.tasks()[0].clone();
    board_task.status = models::TaskStatus::Done;
    board_task.sub_status = models::SubStatus::default_for(models::TaskStatus::Done);
    app.update(Message::Task(crate::tui::messages::TaskMessage::Updated(
        board_task.clone(),
    )));
    let mut stale = board_task;
    stale.last_pre_tool_use_at = None;

    rt.exec_persist_task(&mut app, stale).await;

    let in_memory = app.tasks().iter().find(|t| t.id == id).unwrap();
    assert_eq!(
        in_memory.last_pre_tool_use_at, board_stamp,
        "the sort_order write-back spliced the caller's stale snapshot and \
         clobbered the board's hook-written last_pre_tool_use_at"
    );
    assert!(
        in_memory.sort_order.is_some_and(|so| so < 0),
        "the write-back must still deliver the new sort_order, got {:?}",
        in_memory.sort_order
    );
}

/// A task absent from the in-memory board must not be re-inserted by the
/// write-back. `handle_task_updated` pushes when the id isn't found, so
/// without a guard a persist racing a delete/archive would resurrect a ghost
/// card. Mirrors the guard `write_back_epic_sort_order` already has.
#[tokio::test]
async fn exec_persist_task_write_back_does_not_resurrect_task_absent_from_board() {
    let (rt, mut app) = test_runtime().await;
    let task = create_task_returning(
        &**rt.db_write(),
        "Ghost",
        "Desc",
        "/repo",
        None,
        models::TaskStatus::Review,
    )
    .await
    .unwrap();
    // Never loaded into the board — stands in for a task deleted from the
    // board while this persist was in flight.
    assert!(
        app.tasks().iter().all(|t| t.id != task.id),
        "precondition: task is not in the board"
    );

    let mut done = task.clone();
    done.status = models::TaskStatus::Done;
    done.sub_status = models::SubStatus::default_for(models::TaskStatus::Done);
    rt.exec_persist_task(&mut app, done).await;

    assert!(
        app.tasks().iter().all(|t| t.id != task.id),
        "write-back re-inserted a task that is not on the board"
    );
    // The DB write itself still lands — only the in-memory splice is skipped.
    let db_task = rt.database.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(db_task.status, models::TaskStatus::Done);
    assert!(db_task.sort_order.is_some_and(|so| so < 0));
}

/// SeedActivity writes only `last_pre_tool_use_at`, leaving every other
/// column untouched.
#[tokio::test]
async fn exec_seed_activity_writes_only_timestamp() {
    let (rt, mut app) = test_runtime().await;
    rt.exec_insert_task(
        &mut app,
        tui::TaskDraft {
            title: "Seed".into(),
            description: "Desc".into(),
            repo_path: "/repo".into(),
            ..Default::default()
        },
        None,
    )
    .await;
    let id = app.tasks()[0].id;
    rt.db_write()
        .patch_task(
            id,
            &db::TaskPatch::new()
                .status(models::TaskStatus::Running)
                .sub_status(models::SubStatus::NeedsInput),
        )
        .await
        .unwrap();

    let seed_at = chrono::Utc::now();
    rt.exec_seed_activity(&mut app, id, seed_at).await;

    let db_task = rt.database.get_task(id).await.unwrap().unwrap();
    assert_eq!(
        db_task.last_pre_tool_use_at.map(|t| t.timestamp()),
        Some(seed_at.timestamp())
    );
    // SeedActivity must not touch status/sub_status — those are owned by
    // the dispatch lifecycle, not the activity stamp.
    assert_eq!(db_task.status, models::TaskStatus::Running);
    assert_eq!(db_task.sub_status, models::SubStatus::NeedsInput);
}

#[tokio::test]
async fn exec_save_repo_path_updates_app_state() {
    let (rt, mut app) = test_runtime().await;
    rt.exec_save_repo_path(&mut app, "/repo".into()).await;
    assert!(app.repo_paths().contains(&"/repo".to_string()));
}

#[tokio::test]
async fn exec_save_repo_path_expands_tilde() {
    let (rt, mut app) = test_runtime().await;
    let home = std::env::var("HOME").unwrap();
    rt.exec_save_repo_path(&mut app, "~/myrepo".into()).await;
    let expected = format!("{home}/myrepo");
    assert!(
        app.repo_paths().contains(&expected),
        "Expected repo_paths to contain '{expected}', got: {:?}",
        app.repo_paths()
    );
    // Verify the DB also has the expanded path, not the tilde version
    let db_paths = rt.database.list_repo_paths().await.unwrap();
    assert!(db_paths.contains(&expected));
    assert!(!db_paths.iter().any(|p| p.starts_with("~/")));
}

// -----------------------------------------------------------------------
// Base branch history tests (task #3422) — see docs/specs/dispatch.allium:
// RecordBaseBranch, BaseBranchPicker.
// -----------------------------------------------------------------------

#[tokio::test]
async fn exec_save_base_branch_records_and_updates_app_state() {
    let (rt, mut app) = test_runtime().await;
    rt.exec_save_base_branch(&mut app, "/repo".into(), "develop".into())
        .await;
    assert_eq!(
        app.base_branches_for("/repo"),
        &["develop".to_string()],
        "app.board.repo_base_branches should reflect the newly recorded branch"
    );
    let all = rt.database.list_all_base_branches().await.unwrap();
    assert!(all.contains(&("/repo".to_string(), "develop".to_string())));
}

#[tokio::test]
async fn exec_save_base_branch_upsert_keeps_most_recent_first() {
    let (rt, mut app) = test_runtime().await;
    rt.exec_save_base_branch(&mut app, "/repo".into(), "main".into())
        .await;
    rt.exec_save_base_branch(&mut app, "/repo".into(), "develop".into())
        .await;
    assert_eq!(
        app.base_branches_for("/repo"),
        &["develop".to_string(), "main".to_string()],
        "most-recently-used branch should be first"
    );
}

#[tokio::test]
async fn finish_task_creation_emits_save_repo_path_and_save_base_branch() {
    let (_rt, mut app) = test_runtime().await;

    // Drive the whole manual task-creation flow through the public Message
    // API (App fields are `pub(in crate::tui)` and unreachable from here).
    app.update(Message::Input(
        crate::tui::messages::InputMessage::StartNewTask,
    ));
    app.update(Message::Input(
        crate::tui::messages::InputMessage::SubmitTitle("T".to_string()),
    ));
    app.update(Message::Input(
        crate::tui::messages::InputMessage::SubmitTag(None),
    ));
    app.update(Message::Input(
        crate::tui::messages::InputMessage::SubmitDescription("D".to_string()),
    ));
    app.update(Message::Input(
        crate::tui::messages::InputMessage::SubmitRepoPath("/tmp".to_string()),
    ));
    app.update(Message::Input(
        crate::tui::messages::InputMessage::SubmitBaseBranch("develop".to_string()),
    ));
    let cmds = app.update(Message::Input(
        crate::tui::messages::InputMessage::SubmitWrapUpMode(None),
    ));

    assert!(
        cmds.iter()
            .any(|c| matches!(c, Command::SaveRepoPath(p) if p == "/tmp")),
        "expected a SaveRepoPath(\"/tmp\") command, got: {cmds:?}"
    );
    assert!(
        cmds.iter().any(
            |c| matches!(c, Command::SaveBaseBranch(repo, branch) if repo == "/tmp" && branch == "develop")
        ),
        "expected a SaveBaseBranch(\"/tmp\", \"develop\") command, got: {cmds:?}"
    );
}

#[tokio::test]
async fn exec_quick_dispatch_does_not_record_base_branch_history() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().to_str().unwrap();
    std::fs::create_dir_all(format!("{repo}/.worktrees/1-quick-task")).unwrap();

    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = DispatchScript::dispatch()
        .detecting_default_branch("main")
        .shared_runner();
    let rt = make_runtime(db.clone(), tx, mock).await;
    let tasks = db.list_all().await.unwrap();
    let mut app = App::new(tasks);

    rt.exec_quick_dispatch(
        &mut app,
        tui::TaskDraft {
            title: "Quick task".into(),
            description: String::new(),
            repo_path: repo.to_string(),
            tag: None,
            base_branch: "main".into(),
            wrap_up_mode: None,
        },
        None,
    )
    .await;

    // Repo path IS recorded (existing RecordRepoPath behavior)...
    assert!(app.repo_paths().contains(&repo.to_string()));
    // ...but base branch history is deliberately NOT recorded for quick
    // dispatch — see dispatch.allium: RecordBaseBranch's "recording scope
    // (deliberately narrow)" guidance. Only the manual new-task form records.
    assert!(
        app.base_branches_for(repo).is_empty(),
        "quick dispatch must not record base branch history"
    );
    assert!(rt
        .database
        .list_all_base_branches()
        .await
        .unwrap()
        .is_empty());

    // Drain the async Dispatched message so the sender isn't left dangling.
    let _ = tokio::time::timeout(TEST_TIMEOUT, rx.recv()).await;
}

#[tokio::test]
async fn exec_refresh_from_db_syncs_external_changes() {
    let (rt, mut app) = test_runtime().await;
    // Insert directly into DB, bypassing app
    rt.db_write()
        .create_task(CreateTaskRequest {
            title: "External",
            description: "Added via CLI",
            repo_path: "/repo",
            plan: None,
            status: models::TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    assert!(app.tasks().is_empty());
    rt.exec_refresh_from_db(&mut app).await;
    assert_eq!(app.tasks().len(), 1);
    assert_eq!(app.tasks()[0].title, "External");
}

#[tokio::test]
async fn exec_refresh_from_db_returns_commands_from_refresh() {
    let (rt, mut app) = test_runtime().await;
    // Insert a task directly into DB as Running
    rt.db_write()
        .create_task(CreateTaskRequest {
            title: "Test",
            description: "Desc",
            repo_path: "/repo",
            plan: None,
            status: models::TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    // Load it into app
    let cmds = rt.exec_refresh_from_db(&mut app).await;
    assert!(cmds.is_empty()); // First load — no transition

    let task = rt.database.list_all().await.unwrap()[0].clone();
    rt.db_write()
        .patch_task(
            task.id,
            &db::TaskPatch::new().status(models::TaskStatus::Review),
        )
        .await
        .unwrap();

    app.set_notifications_enabled(true);
    let cmds = rt.exec_refresh_from_db(&mut app).await;
    assert!(cmds.iter().any(|c| matches!(
        c,
        Command::System(crate::tui::commands::SystemCommand::SendNotification { .. })
    )));
}

#[tokio::test]
async fn exec_delete_task_nonexistent_shows_error() {
    let (rt, mut app) = test_runtime().await;
    rt.exec_delete_task(&mut app, TaskId(999)).await;
    assert!(app.error_popup().is_some());
}

#[tokio::test]
async fn exec_jump_to_tmux_calls_select_window() {
    let db = test_db().await;
    let (tx, _rx) = mpsc::unbounded_channel();
    let mock = Arc::new(
        MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // for select-window
        ])
        .with_windows(&["my-window"]),
    );
    let rt = make_runtime(db.clone(), tx, mock.clone()).await;
    let tasks = db.list_all().await.unwrap();
    let mut app = App::new(tasks);

    rt.exec_jump_to_tmux(&mut app, "my-window".to_string());

    let calls = mock.recorded_calls();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].1.contains(&"select-window".to_string()));
    // Targeted by resolved pane ID, not by name — see `tmux::window_target`.
    assert!(calls[0].1.contains(&mock.pane_id_of("my-window")));
    assert!(app.error_popup().is_none());
}

#[tokio::test]
async fn exec_dispatch_sends_dispatched_message() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().to_str().unwrap();
    // Create .worktrees/ and fake worktree directory so file writes succeed
    std::fs::create_dir_all(format!("{repo}/.worktrees/1-test-task")).unwrap();

    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = DispatchScript::dispatch().shared_runner();
    let rt = make_runtime(db.clone(), tx, mock).await;

    let task = create_task_returning(
        &*db,
        "Test Task",
        "desc",
        repo,
        None,
        models::TaskStatus::Backlog,
    )
    .await
    .unwrap();
    let id = task.id;
    rt.exec_dispatch_agent(task, models::DispatchMode::Dispatch)
        .await;

    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            msg,
            Message::Task(crate::tui::messages::TaskMessage::Dispatched { .. })
        ),
        "Expected Dispatched, got: {msg:?}"
    );

    // The claim, not `handle_dispatched`'s Persist, owns the Running write — and
    // nothing here runs that Persist, so the row can only have left Backlog via
    // the claim `exec_dispatch_agent` takes before provisioning
    // (`DispatchClaimExclusive` in docs/specs/dispatch.allium).
    let claimed = db.get_task(id).await.unwrap().unwrap();
    assert_eq!(claimed.status, models::TaskStatus::Running);
    assert!(
        claimed.last_pre_tool_use_at.is_some(),
        "the claim seeds the activity stamp"
    );
}

/// A lost claim must stop the dispatch dead, before any provisioning command
/// runs, and report the failure so the spinner drains (`LostClaimReported` in
/// docs/specs/dispatch.allium).
#[tokio::test]
async fn exec_dispatch_agent_lost_claim_provisions_nothing() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    // An empty script is itself the assertion: any provisioning command would
    // panic the mock rather than pass quietly.
    let mock = Arc::new(MockProcessRunner::new(vec![]));
    let rt = make_runtime(db.clone(), tx, mock.clone()).await;

    // Backlog in the caller's snapshot, already Running in the DB — exactly the
    // race the claim exists to catch.
    let task = create_task_returning(
        &*db,
        "Contended Task",
        "desc",
        "/repo",
        None,
        models::TaskStatus::Backlog,
    )
    .await
    .unwrap();
    assert!(rt.task_svc.claim_backlog_task(task.id).await.unwrap());

    rt.exec_dispatch_agent(task.clone(), models::DispatchMode::Dispatch)
        .await;

    let msg1 = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(msg1, Message::Task(crate::tui::messages::TaskMessage::DispatchAbandoned(id)) if id == task.id),
        "a lost claim must report DispatchAbandoned, not DispatchFailed — the latter \
         releases, and the claim we lost belongs to the winner. Got: {msg1:?}"
    );
    let msg2 = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            msg2,
            Message::System(crate::tui::messages::SystemMessage::Error(_))
        ),
        "Expected Error, got: {msg2:?}"
    );
    assert!(
        mock.recorded_calls().is_empty(),
        "a lost claim must run no provisioning commands, got: {:?}",
        mock.recorded_calls()
    );
    // The winner's claim is untouched: still Running, still unprovisioned,
    // still theirs to finish.
    let after = db.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(after.status, models::TaskStatus::Running);
    assert!(after.worktree.is_none());
}

/// `ReleaseClaim` returns a claimed-but-unprovisioned task to Backlog. This is
/// the command `DispatchFailed` emits.
#[tokio::test]
async fn exec_release_claim_returns_the_task_to_backlog() {
    let db = test_db().await;
    let (tx, _rx) = mpsc::unbounded_channel();
    let rt = make_runtime(db.clone(), tx, Arc::new(MockProcessRunner::new(vec![]))).await;
    let mut app = App::new(vec![]);
    let task = create_task_returning(
        &*db,
        "Claimed Task",
        "desc",
        "/repo",
        None,
        models::TaskStatus::Backlog,
    )
    .await
    .unwrap();
    assert!(rt.task_svc.claim_backlog_task(task.id).await.unwrap());

    rt.exec_release_claim(&mut app, task.id).await;

    let released = db.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(released.status, models::TaskStatus::Backlog);
    assert!(
        released.last_pre_tool_use_at.is_none(),
        "the release clears the stamp the claim seeded"
    );
    assert!(app.error_popup().is_none());
}

#[tokio::test]
async fn exec_dispatch_sends_error_on_failure() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::fail("fatal: not a git repository"), // git worktree add fails
    ]));
    let rt = make_runtime(db.clone(), tx, mock).await;

    let task = create_task_returning(
        &*db,
        "Fail Task",
        "desc",
        "/nonexistent",
        None,
        models::TaskStatus::Backlog,
    )
    .await
    .unwrap();
    rt.exec_dispatch_agent(task.clone(), models::DispatchMode::Dispatch)
        .await;

    let msg1 = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(msg1, Message::Task(crate::tui::messages::TaskMessage::DispatchFailed(id)) if id == task.id),
        "Expected DispatchFailed, got: {msg1:?}"
    );

    let msg2 = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            msg2,
            Message::System(crate::tui::messages::SystemMessage::Error(_))
        ),
        "Expected Error, got: {msg2:?}"
    );
}

#[tokio::test]
async fn exec_check_window_sends_window_gone_when_absent() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        // has_window: list-windows returns other window names (not our window)
        MockProcessRunner::ok_with_stdout(b"other-window\n"),
    ]));
    let rt = make_runtime(db.clone(), tx, mock).await;

    rt.exec_check_window(TaskId(1), "gone-window".to_string());

    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            msg,
            Message::Task(crate::tui::messages::TaskMessage::WindowGone(TaskId(1)))
        ),
        "Expected WindowGone, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_check_window_sends_nothing_when_present() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        // has_window: list-windows returns our window
        MockProcessRunner::ok_with_stdout(b"task-1\n"),
    ]));
    let rt = make_runtime(db.clone(), tx, mock).await;

    rt.exec_check_window(TaskId(1), "task-1".to_string())
        .await
        .unwrap();
    assert!(
        rx.try_recv().is_err(),
        "Expected no message but received one"
    );
}

#[tokio::test]
async fn exec_check_window_sends_nothing_when_query_fails() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    // The runner itself errors (e.g. tmux binary missing) — a transient
    // failure must not be mistaken for the window (and therefore the agent)
    // being gone.
    let mock = Arc::new(MockProcessRunner::new(vec![Err(anyhow::anyhow!(
        "failed to run tmux"
    ))]));
    let rt = make_runtime(db.clone(), tx, mock).await;

    rt.exec_check_window(TaskId(1), "task-1".to_string())
        .await
        .unwrap();

    assert!(
        rx.try_recv().is_err(),
        "a tmux query failure must not send WindowGone"
    );
}

#[tokio::test]
async fn exec_batch_check_windows_sends_window_gone_only_for_absent() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    // Single `tmux list-windows -a` reports task-1 present, task-2 gone (died mid-run).
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"task-1\nother-window\n"),
    ]));
    let rt = make_runtime(db.clone(), tx, mock.clone()).await;

    rt.exec_batch_check_windows(vec![
        (TaskId(1), "task-1".to_string()),
        (TaskId(2), "task-2".to_string()),
    ])
    .await
    .unwrap();

    // Exactly one WindowGone, for the absent window (task-2).
    let mut gone = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        if let Message::Task(crate::tui::messages::TaskMessage::WindowGone(id)) = msg {
            gone.push(id);
        } else {
            panic!("unexpected message: {msg:?}");
        }
    }
    assert_eq!(gone, vec![TaskId(2)], "only the absent window should crash");

    // A single batched tmux call, not one per window. (The exact argv of
    // list-windows is owned by tmux.rs's own unit tests — assert only the
    // batching guarantee here.)
    let calls = mock.recorded_calls();
    assert_eq!(calls.len(), 1, "batch check should issue one tmux call");
    assert_eq!(calls[0].1[0], "list-windows");
}

#[tokio::test]
async fn exec_batch_check_windows_sends_nothing_when_all_present() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"task-1\ntask-2\n"),
    ]));
    let rt = make_runtime(db.clone(), tx, mock).await;

    rt.exec_batch_check_windows(vec![
        (TaskId(1), "task-1".to_string()),
        (TaskId(2), "task-2".to_string()),
    ])
    .await
    .unwrap();

    assert!(
        rx.try_recv().is_err(),
        "no WindowGone expected when all windows are present"
    );
}

#[tokio::test]
async fn exec_batch_check_windows_stays_silent_when_tmux_cannot_be_spawned() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    // The runner itself errors (e.g. tmux binary missing) — `list_all_window_names`
    // propagates the Err, and the batch check bails without marking any window
    // gone, so a transient tmux failure can't crash every running task at once.
    let mock = Arc::new(MockProcessRunner::new(vec![Err(anyhow::anyhow!(
        "failed to run tmux"
    ))]));
    let rt = make_runtime(db.clone(), tx, mock).await;

    rt.exec_batch_check_windows(vec![(TaskId(1), "task-1".to_string())])
        .await
        .unwrap();

    assert!(
        rx.try_recv().is_err(),
        "a tmux spawn error must not be treated as every window being gone"
    );
}

#[tokio::test]
async fn exec_jump_to_tmux_failure_shows_error() {
    let db = test_db().await;
    let (tx, _rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::fail("no such window"), // simulate tmux failure
    ]));
    let rt = make_runtime(db.clone(), tx, mock.clone()).await;
    let tasks = db.list_all().await.unwrap();
    let mut app = App::new(tasks);

    rt.exec_jump_to_tmux(&mut app, "nonexistent-window".to_string());

    assert!(app.error_popup().is_some());
}

/// Seed one archived task that owns `worktree`, and a runtime whose runner
/// answers `script`. Returns the runtime, the task id, the message receiver and
/// the concrete runner, so a caller that needs to assert on the issued commands
/// can reach `flattened_calls()`.
async fn cleanup_fixture(
    script: Vec<anyhow::Result<std::process::Output>>,
    worktree: &str,
) -> (
    TuiRuntime,
    models::TaskId,
    mpsc::UnboundedReceiver<Message>,
    Arc<MockProcessRunner>,
) {
    let db = test_db().await;
    let (tx, rx) = mpsc::unbounded_channel();
    let runner = Arc::new(MockProcessRunner::new(script));
    let rt = make_runtime(db.clone(), tx, runner.clone()).await;

    let task = create_task_returning(
        &*db,
        "Doomed",
        "desc",
        "/repo",
        None,
        models::TaskStatus::Archived,
    )
    .await
    .unwrap();
    db.patch_task(task.id, &db::TaskPatch::new().worktree(Some(worktree)))
        .await
        .unwrap();

    (rt, task.id, rx, runner)
}

/// A failed `git worktree remove` must not let the operation forget the path.
/// The row keeps its pointer so the leftover directory stays reachable from the
/// board, and the failure is reported. `WorktreeReleaseIsGated` in
/// docs/specs/tasks.allium; the silent-orphan mechanism from
/// docs/plans/2026-08-11-3897-worktree-cleanup-investigation.md §3.
#[tokio::test]
async fn exec_cleanup_failure_keeps_the_worktree_pointer() {
    let worktree = "/repo/.worktrees/1-doomed";
    let (rt, id, mut rx, _runner) = cleanup_fixture(
        vec![MockProcessRunner::fail("fatal: could not lock index")],
        worktree,
    )
    .await;

    let handle = rt.exec_cleanup(
        id,
        "/repo".into(),
        Some(worktree.into()),
        None,
        crate::tui::commands::CleanupFollowUp::ClearPointer,
    );
    handle.await.unwrap();

    let row = rt.database.get_task(id).await.unwrap().unwrap();
    assert_eq!(
        row.worktree.as_deref(),
        Some(worktree),
        "a failed removal must leave the pointer in place"
    );

    let msg = rx.recv().await.unwrap();
    assert!(
        matches!(
            msg,
            Message::Task(crate::tui::messages::TaskMessage::CleanupFailed { worktree: ref w, .. })
                if w == worktree
        ),
        "the failure must reach the app, got: {msg:?}"
    );
}

/// The success half: only a removal that actually happened earns the follow-up.
#[tokio::test]
async fn exec_cleanup_success_reports_its_follow_up() {
    let worktree = "/repo/.worktrees/1-doomed";
    let (rt, id, mut rx, _runner) = cleanup_fixture(
        vec![
            MockProcessRunner::ok(), // git worktree remove
            MockProcessRunner::ok(), // git branch -D
        ],
        worktree,
    )
    .await;

    let handle = rt.exec_cleanup(
        id,
        "/repo".into(),
        Some(worktree.into()),
        None,
        crate::tui::commands::CleanupFollowUp::DeleteRow,
    );
    handle.await.unwrap();

    let msg = rx.recv().await.unwrap();
    assert!(
        matches!(
            msg,
            Message::Task(crate::tui::messages::TaskMessage::CleanupSucceeded {
                follow_up: crate::tui::commands::CleanupFollowUp::DeleteRow,
                ..
            })
        ),
        "a successful removal must carry its follow-up back, got: {msg:?}"
    );
}

/// The delete path's half of the gate: a failed removal means the row survives,
/// still archived and still pointing at what is on disk, so deleting again
/// retries the removal.
#[tokio::test]
async fn exec_cleanup_failure_does_not_delete_the_row() {
    let worktree = "/repo/.worktrees/1-doomed";
    let (rt, id, mut rx, _runner) = cleanup_fixture(
        vec![MockProcessRunner::fail("fatal: could not lock index")],
        worktree,
    )
    .await;

    let handle = rt.exec_cleanup(
        id,
        "/repo".into(),
        Some(worktree.into()),
        None,
        crate::tui::commands::CleanupFollowUp::DeleteRow,
    );
    handle.await.unwrap();

    let row = rt
        .database
        .get_task(id)
        .await
        .unwrap()
        .expect("the row must survive a failed removal");
    assert_eq!(row.status, models::TaskStatus::Archived);
    assert_eq!(row.worktree.as_deref(), Some(worktree));

    let msg = rx.recv().await.unwrap();
    assert!(
        !matches!(
            msg,
            Message::Task(crate::tui::messages::TaskMessage::CleanupSucceeded { .. })
        ),
        "a failed removal must never report success, got: {msg:?}"
    );
}

/// Seed one archived task that owns `window` and no worktree — the row shape
/// whose window archive/delete used to leak (#4096).
async fn window_only_cleanup_fixture(
    script: Vec<anyhow::Result<std::process::Output>>,
    window: &str,
) -> (
    TuiRuntime,
    models::TaskId,
    mpsc::UnboundedReceiver<Message>,
    Arc<MockProcessRunner>,
) {
    let db = test_db().await;
    let (tx, rx) = mpsc::unbounded_channel();
    let runner = Arc::new(MockProcessRunner::new(script));
    let rt = make_runtime(db.clone(), tx, runner.clone()).await;

    let task = create_task_returning(
        &*db,
        "Window but no worktree",
        "desc",
        "/repo",
        None,
        models::TaskStatus::Archived,
    )
    .await
    .unwrap();
    db.patch_task(task.id, &db::TaskPatch::new().tmux_window(Some(window)))
        .await
        .unwrap();

    (rt, task.id, rx, runner)
}

/// `TeardownIsOwedWheneverThereIsSomethingToRelease` in docs/specs/tasks.allium:
/// a task with a window and no worktree still owes step 1. Before #4096
/// `take_cleanup` dropped the whole command for this shape, so nothing ever ran.
#[tokio::test]
async fn exec_cleanup_kills_the_window_of_a_task_with_no_worktree() {
    let (rt, id, mut rx, runner) = window_only_cleanup_fixture(
        vec![
            MockProcessRunner::ok_with_stdout(b"task-1\n"), // has_window
            MockProcessRunner::ok(),                        // tmux kill-window
        ],
        "task-1",
    )
    .await;

    rt.exec_cleanup(
        id,
        "/repo".into(),
        None,
        Some("task-1".into()),
        crate::tui::commands::CleanupFollowUp::DeleteRow,
    )
    .await
    .unwrap();

    let calls = runner.flattened_calls();
    assert!(
        calls.iter().any(|c| c.contains("kill-window")),
        "the window must be reclaimed even with no worktree, got: {calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c.contains("worktree remove")),
        "there is no worktree to remove, got: {calls:?}"
    );

    let msg = rx.recv().await.unwrap();
    assert!(
        matches!(
            msg,
            Message::Task(crate::tui::messages::TaskMessage::CleanupSucceeded {
                follow_up: crate::tui::commands::CleanupFollowUp::DeleteRow,
                ..
            })
        ),
        "the follow-up must come back so the row is deleted, got: {msg:?}"
    );
}

/// The gate is keyed on step 2 and only on step 2 (`WorktreeReleaseIsGated`).
/// With no worktree there is nothing to release and nothing to retry, so a failed
/// window kill is warn-logged and the follow-up still applies — withholding it
/// would strand the row instead of the resource.
#[tokio::test]
async fn exec_cleanup_window_only_kill_failure_still_applies_the_follow_up() {
    let (rt, id, mut rx, _runner) = window_only_cleanup_fixture(
        vec![
            MockProcessRunner::ok_with_stdout(b"task-1\n"), // has_window
            MockProcessRunner::fail("can't find window"),   // kill-window fails
        ],
        "task-1",
    )
    .await;

    rt.exec_cleanup(
        id,
        "/repo".into(),
        None,
        Some("task-1".into()),
        crate::tui::commands::CleanupFollowUp::DeleteRow,
    )
    .await
    .unwrap();

    let msg = rx.recv().await.unwrap();
    assert!(
        matches!(
            msg,
            Message::Task(crate::tui::messages::TaskMessage::CleanupSucceeded {
                follow_up: crate::tui::commands::CleanupFollowUp::DeleteRow,
                ..
            })
        ),
        "a window-only teardown must not withhold its follow-up, got: {msg:?}"
    );
}

/// `exec_cleanup` tears the worktree down unconditionally — deliberately.
///
/// Hand-builds the state the removed sharing exception described — a second live
/// row naming the very same worktree, which the dispatch flow cannot produce —
/// and pins that the full teardown runs anyway, follow-up and all. A reinstated
/// guard fails here rather than passing silently. `WorktreeIsNeverShared` in
/// docs/specs/tasks.allium is the argument; this is only its tripwire.
#[tokio::test]
async fn exec_cleanup_tears_down_even_if_another_row_names_the_worktree() {
    let worktree = "/repo/.worktrees/1-doomed";
    let (rt, id, mut rx, runner) = cleanup_fixture(
        vec![
            MockProcessRunner::ok_with_stdout(b"task-1\n"), // has_window
            MockProcessRunner::ok(),                        // tmux kill-window
            MockProcessRunner::ok(),                        // git worktree remove
            MockProcessRunner::ok(),                        // git branch -D
        ],
        worktree,
    )
    .await;

    // The impossible second holder of the same path.
    let sharer = create_task_returning(
        &**rt.db_write(),
        "Impossible sharer",
        "desc",
        "/repo",
        None,
        models::TaskStatus::Running,
    )
    .await
    .unwrap();
    rt.db_write()
        .patch_task(sharer.id, &db::TaskPatch::new().worktree(Some(worktree)))
        .await
        .unwrap();

    rt.exec_cleanup(
        id,
        "/repo".into(),
        Some(worktree.into()),
        Some("task-1".into()),
        crate::tui::commands::CleanupFollowUp::DeleteRow,
    )
    .await
    .unwrap();

    let calls = runner.flattened_calls();
    let removed_at = calls
        .iter()
        .position(|c| c.contains("worktree remove") && c.contains(worktree))
        .unwrap_or_else(|| {
            panic!("the worktree goes regardless of what other rows name, got: {calls:?}")
        });
    let killed_at = calls
        .iter()
        .position(|c| c.contains("kill-window"))
        .unwrap_or_else(|| panic!("the window is reclaimed too, got: {calls:?}"));
    // TaskTeardown's clause order: the window goes before the worktree.
    assert!(
        killed_at < removed_at,
        "the window must be killed before the worktree is removed, got: {calls:?}"
    );

    let msg = rx.recv().await.unwrap();
    assert!(
        matches!(
            msg,
            Message::Task(crate::tui::messages::TaskMessage::CleanupSucceeded {
                follow_up: crate::tui::commands::CleanupFollowUp::DeleteRow,
                ..
            })
        ),
        "a real removal must earn its follow-up, got: {msg:?}"
    );
}

#[tokio::test]
async fn send_system_error_sends_error_message() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
    let rt = make_runtime(db, tx, runner).await;

    rt.send_system_error("something went wrong");

    let msg = rx.recv().await.unwrap();
    assert!(
        matches!(msg, Message::System(crate::tui::messages::SystemMessage::Error(ref e)) if e == "something went wrong"),
        "Expected SystemMessage::Error, got: {msg:?}"
    );
}

// `TaskCommand::Finish`/`exec_finish` and `TaskCommand::CloseSession`/
// `exec_close_session` no longer exist — the TUI wrap-up entry point (`W`)
// that used to dispatch them is gone. Wrap-up rebase/merge and session close
// are now exclusively the MCP `wrap_up`/`exit_session` tools' job (see
// src/mcp/handlers/tasks/wrap_up.rs), which drive `dispatch::finish_task` and
// `TaskService::close_session` directly rather than through a runtime
// command. The ExitSession ordering invariant — the tmux teardown follows the
// terminal write and is gated on it, so a task whose write failed keeps BOTH
// its live window and its `tmux_window` reference — is covered at that layer
// by `exit_session_failed_close_leaves_the_task_unchanged` and
// `exit_session_failed_close_issues_no_kill_window` in
// src/mcp/handlers/tests/tasks/dispatch.rs.

#[tokio::test]
async fn exec_send_notification_calls_notify_send() {
    let db = test_db().await;
    let (tx, _rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // notify-send call
    ]));
    let rt = make_runtime(db, tx, mock.clone()).await;
    rt.exec_send_notification("Task #1: Fix bug", "Ready for review", false)
        .await
        .unwrap();
    let calls = mock.recorded_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "notify-send");
    assert!(calls[0].1.contains(&"Task #1: Fix bug".to_string()));
    assert!(calls[0].1.contains(&"Ready for review".to_string()));
}

#[tokio::test]
async fn exec_send_notification_urgent_uses_critical() {
    let db = test_db().await;
    let (tx, _rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![MockProcessRunner::ok()]));
    let rt = make_runtime(db, tx, mock.clone()).await;
    rt.exec_send_notification("Task #1: Fix bug", "Agent needs your input", true)
        .await
        .unwrap();
    let calls = mock.recorded_calls();
    assert!(calls[0].1.contains(&"critical".to_string()));
}

#[tokio::test]
async fn exec_send_notification_failure_does_not_panic() {
    let db = test_db().await;
    let (tx, _rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![MockProcessRunner::fail(
        "command not found",
    )]));
    let rt = make_runtime(db, tx, mock.clone()).await;
    // Should not panic — just logs a warning
    rt.exec_send_notification("Task #1: Fix bug", "Ready for review", false)
        .await
        .unwrap();
}

#[tokio::test]
async fn exec_persist_setting_writes_to_db() {
    let (rt, mut app) = test_runtime().await;
    rt.exec_persist_setting(&mut app, "notifications_enabled", true)
        .await;
    assert_eq!(
        rt.database
            .get_setting_bool("notifications_enabled")
            .await
            .unwrap(),
        Some(true)
    );
}

#[tokio::test]
async fn exec_check_pr_status_sends_merged() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"MERGED\n"), // gh pr view (no review decision line)
    ]));
    let rt = make_runtime(db, tx, mock).await;

    rt.exec_check_pr_status(TaskId(1), "https://github.com/org/repo/pull/42".to_string());

    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        msg,
        Message::Pr(crate::tui::messages::PrMessage::Merged(TaskId(1)))
    ));
}

#[tokio::test]
async fn exec_check_pr_status_open_sends_review_state() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"OPEN\nAPPROVED\n"), // gh pr view
    ]));
    let rt = make_runtime(db, tx, mock).await;

    rt.exec_check_pr_status(TaskId(1), "https://github.com/org/repo/pull/42".to_string());

    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    match msg {
        Message::Pr(crate::tui::messages::PrMessage::ReviewState {
            id,
            review_decision,
        }) => {
            assert_eq!(id, TaskId(1));
            assert_eq!(review_decision, Some(models::ReviewDecision::Approved));
        }
        other => panic!("Expected PrReviewState, got {:?}", other),
    }
}

#[tokio::test]
async fn exec_check_pr_status_sends_closed() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"CLOSED\n"), // gh pr view (no review decision line)
    ]));
    let rt = make_runtime(db, tx, mock).await;

    rt.exec_check_pr_status(TaskId(1), "https://github.com/org/repo/pull/42".to_string());

    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        msg,
        Message::Pr(crate::tui::messages::PrMessage::Closed(TaskId(1)))
    ));
}

#[tokio::test]
async fn exec_persist_string_setting_writes_to_db() {
    let (rt, mut app) = test_runtime().await;
    rt.exec_persist_string_setting(&mut app, "repo_filter", "/repo1\n/repo2")
        .await;
    assert_eq!(
        rt.database.get_setting_string("repo_filter").await.unwrap(),
        Some("/repo1\n/repo2".to_string())
    );
}

#[tokio::test]
async fn exec_quick_dispatch_creates_task_and_dispatches() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().to_str().unwrap();
    // Pre-create worktree directory so provision_worktree skips git worktree add
    std::fs::create_dir_all(format!("{repo}/.worktrees/1-my-task")).unwrap();

    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = DispatchScript::dispatch()
        .detecting_default_branch("main")
        .shared_runner();
    let rt = make_runtime(db.clone(), tx, mock).await;
    let tasks = db.list_all().await.unwrap();
    let mut app = App::new(tasks);

    rt.exec_quick_dispatch(
        &mut app,
        tui::TaskDraft {
            title: "My Task".into(),
            description: "Do stuff".into(),
            repo_path: repo.to_string(),
            tag: None,
            base_branch: "main".into(),
            wrap_up_mode: None,
        },
        None,
    )
    .await;

    // Task was created in app and DB synchronously
    assert_eq!(app.tasks().len(), 1);
    assert_eq!(app.tasks()[0].title, "My Task");
    assert_eq!(db.list_all().await.unwrap().len(), 1);

    // Repo path was saved
    assert!(app.repo_paths().contains(&repo.to_string()));

    // Dispatch message arrives asynchronously
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            msg,
            Message::Task(crate::tui::messages::TaskMessage::Dispatched {
                switch_focus: true,
                ..
            })
        ),
        "Expected Dispatched, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_quick_dispatch_sets_base_branch_to_repo_default() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().to_str().unwrap();
    std::fs::create_dir_all(format!("{repo}/.worktrees/1-quick-task")).unwrap();

    let db = test_db().await;
    let (tx, _rx) = mpsc::unbounded_channel();
    let mock = DispatchScript::dispatch()
        .detecting_default_branch("master")
        .shared_runner();
    let rt = make_runtime(db.clone(), tx, mock).await;
    let tasks = db.list_all().await.unwrap();
    let mut app = App::new(tasks);

    rt.exec_quick_dispatch(
        &mut app,
        tui::TaskDraft {
            title: "Quick task".into(),
            description: String::new(),
            repo_path: repo.to_string(),
            tag: None,
            // The draft default doesn't matter — quick-dispatch resolves
            // base_branch from the repo's `origin/HEAD`.
            base_branch: "main".into(),
            wrap_up_mode: None,
        },
        None,
    )
    .await;

    let stored = db.list_all().await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(
        stored[0].base_branch, "master",
        "quick-dispatch should resolve and persist the repo's default branch"
    );
}

#[tokio::test]
async fn exec_quick_dispatch_with_epic_dispatches_successfully() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().to_str().unwrap();
    std::fs::create_dir_all(format!("{repo}/.worktrees/1-epic-task")).unwrap();

    let db = test_db().await;
    let epic = db.create_epic("My Epic", "epic desc", None).await.unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = DispatchScript::dispatch()
        .detecting_default_branch("main")
        .shared_runner();
    let rt = make_runtime(db.clone(), tx, mock).await;
    let tasks = db.list_all().await.unwrap();
    let mut app = App::new(tasks);

    rt.exec_quick_dispatch(
        &mut app,
        tui::TaskDraft {
            title: "Epic Task".into(),
            description: "do stuff".into(),
            repo_path: repo.to_string(),
            tag: None,
            base_branch: "main".into(),
            wrap_up_mode: None,
        },
        Some(epic.id),
    )
    .await;

    // Task was created with epic linkage
    assert_eq!(app.tasks().len(), 1);
    assert_eq!(app.tasks()[0].epic_id, Some(epic.id));

    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            msg,
            Message::Task(crate::tui::messages::TaskMessage::Dispatched { .. })
        ),
        "Expected Dispatched, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_quick_dispatch_sends_error_on_failure() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::fail("not a git repo"), // detect_default_branch
    ]));
    let rt = make_runtime(db.clone(), tx, mock).await;
    let tasks = db.list_all().await.unwrap();
    let mut app = App::new(tasks);

    // /nonexistent won't have .worktrees dir, so provision_worktree fails
    rt.exec_quick_dispatch(
        &mut app,
        tui::TaskDraft {
            title: "Fail Task".into(),
            description: "desc".into(),
            repo_path: "/nonexistent".into(),
            tag: None,
            base_branch: "main".into(),
            wrap_up_mode: None,
        },
        None,
    )
    .await;

    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            msg,
            Message::Task(crate::tui::messages::TaskMessage::DispatchFailed(_))
                | Message::System(crate::tui::messages::SystemMessage::Error(_))
        ),
        "Expected DispatchFailed or Error, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_quick_dispatch_failure_sends_dispatch_failed_and_error() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![MockProcessRunner::fail(
        "not a git repo",
    )]));
    let rt = make_runtime(db.clone(), tx, mock).await;
    let tasks = db.list_all().await.unwrap();
    let mut app = App::new(tasks);

    rt.exec_quick_dispatch(
        &mut app,
        tui::TaskDraft {
            title: "Fail Task".into(),
            description: String::new(),
            repo_path: "/nonexistent".into(),
            tag: None,
            base_branch: "main".into(),
            wrap_up_mode: None,
        },
        None,
    )
    .await;

    // The task was created synchronously
    let created_id = app.tasks()[0].id;

    let msg1 = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(msg1, Message::Task(crate::tui::messages::TaskMessage::DispatchFailed(id)) if id == created_id),
        "Expected DispatchFailed, got: {msg1:?}"
    );
    let msg2 = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            msg2,
            Message::System(crate::tui::messages::SystemMessage::Error(_))
        ),
        "Expected Error, got: {msg2:?}"
    );
}

#[tokio::test]
async fn exec_resume_sends_resumed_message() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = DispatchScript::resume().shared_runner();
    let rt = make_runtime(db.clone(), tx, mock).await;

    let mut task = create_task_returning(
        &*db,
        "Resume Me",
        "desc",
        "/repo",
        None,
        models::TaskStatus::Running,
    )
    .await
    .unwrap();
    task.worktree = Some("/repo/.worktrees/1-resume-me".into());
    let id = task.id;

    rt.exec_resume(task);

    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    let Message::Task(crate::tui::messages::TaskMessage::Resumed {
        id: tid,
        tmux_window,
    }) = msg
    else {
        panic!("Expected Resumed, got: {msg:?}");
    };
    assert_eq!(tid, id);
    assert_eq!(tmux_window, format!("task-{id}"));
}

#[tokio::test]
async fn exec_resume_sends_error_on_failure() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::fail("no tmux session"), // tmux new-window fails
    ]));
    let rt = make_runtime(db.clone(), tx, mock).await;

    let task = create_task_returning(
        &*db,
        "Fail Resume",
        "desc",
        "/repo",
        None,
        models::TaskStatus::Running,
    )
    .await
    .unwrap();
    rt.exec_resume(task);

    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            msg,
            Message::System(crate::tui::messages::SystemMessage::Error(_))
        ),
        "Expected Error, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_kill_tmux_window_failure_does_not_send_error() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::fail("no such window"), // tmux kill-window fails
    ]));
    let rt = make_runtime(db.clone(), tx, mock).await;

    rt.exec_kill_tmux_window("task-99".to_string())
        .await
        .unwrap();

    // Channel should be empty — no error message sent
    assert!(rx.try_recv().is_err(), "Expected no message, but got one");
}

#[tokio::test]
async fn exec_patch_sub_status_updates_db() {
    let (rt, mut app) = test_runtime().await;
    rt.exec_insert_task(
        &mut app,
        tui::TaskDraft {
            title: "Test".into(),
            description: "Desc".into(),
            repo_path: "/repo".into(),
            ..Default::default()
        },
        None,
    )
    .await;
    let id = app.tasks()[0].id;

    // Move task to Running first
    rt.db_write()
        .patch_task(
            id,
            &db::TaskPatch::new().status(models::TaskStatus::Running),
        )
        .await
        .unwrap();

    rt.exec_patch_sub_status(&mut app, id, models::SubStatus::NeedsInput)
        .await;

    let db_task = rt.database.get_task(id).await.unwrap().unwrap();
    assert_eq!(db_task.sub_status, models::SubStatus::NeedsInput);
    assert!(app.error_popup().is_none());
}

#[tokio::test]
async fn exec_patch_sub_status_shows_error_for_missing_task() {
    let (rt, mut app) = test_runtime().await;
    rt.exec_patch_sub_status(&mut app, TaskId(999), models::SubStatus::Active)
        .await;
    assert!(app.error_popup().is_some());
}

#[tokio::test]
async fn exec_move_task_to_epic_links_and_refreshes() {
    let (rt, mut app) = test_runtime().await;
    let epic = rt
        .db_write()
        .create_epic("Epic", "desc", None)
        .await
        .unwrap();
    rt.exec_insert_task(
        &mut app,
        tui::TaskDraft {
            title: "T".into(),
            description: "".into(),
            repo_path: "/repo".into(),
            ..Default::default()
        },
        None,
    )
    .await;
    let id = app.tasks()[0].id;

    rt.exec_move_task_to_epic(&mut app, id, Some(epic.id)).await;

    assert_eq!(
        rt.database.get_task(id).await.unwrap().unwrap().epic_id,
        Some(epic.id)
    );
    // Board reflects the new membership after refresh.
    assert_eq!(
        app.tasks().iter().find(|t| t.id == id).unwrap().epic_id,
        Some(epic.id)
    );
    assert!(app.error_popup().is_none());
}

#[tokio::test]
async fn exec_move_task_to_epic_detaches_to_none() {
    let (rt, mut app) = test_runtime().await;
    let epic = rt
        .db_write()
        .create_epic("Epic", "desc", None)
        .await
        .unwrap();
    rt.exec_insert_task(
        &mut app,
        tui::TaskDraft {
            title: "T".into(),
            description: "".into(),
            repo_path: "/repo".into(),
            ..Default::default()
        },
        Some(epic.id),
    )
    .await;
    let id = app.tasks()[0].id;

    rt.exec_move_task_to_epic(&mut app, id, None).await;

    assert_eq!(
        rt.database.get_task(id).await.unwrap().unwrap().epic_id,
        None
    );
    assert!(app.error_popup().is_none());
}

#[tokio::test]
async fn exec_move_task_to_epic_shows_error_for_missing_epic() {
    let (rt, mut app) = test_runtime().await;
    rt.exec_insert_task(
        &mut app,
        tui::TaskDraft {
            title: "T".into(),
            description: "".into(),
            repo_path: "/repo".into(),
            ..Default::default()
        },
        None,
    )
    .await;
    let id = app.tasks()[0].id;

    rt.exec_move_task_to_epic(&mut app, id, Some(models::EpicId(9999)))
        .await;

    assert!(app.error_popup().is_some());
    assert_eq!(
        rt.database.get_task(id).await.unwrap().unwrap().epic_id,
        None
    );
}

// -----------------------------------------------------------------------
// Filter preset tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn exec_persist_filter_preset_saves_to_db() {
    let (rt, mut app) = test_runtime().await;
    rt.exec_persist_filter_preset(
        &mut app,
        "my-preset",
        &["/repo1".into(), "/repo2".into()],
        "include",
    )
    .await;
    let presets = rt.database.list_filter_presets().await.unwrap();
    assert_eq!(presets.len(), 1);
    assert_eq!(presets[0].0, "my-preset");
    assert_eq!(presets[0].2, "include");
    assert!(app.error_popup().is_none());
}

#[tokio::test]
async fn exec_delete_filter_preset_removes_from_db() {
    let (rt, mut app) = test_runtime().await;
    rt.database
        .save_filter_preset("doomed", &["/repo".into()], "include")
        .await
        .unwrap();
    rt.exec_delete_filter_preset(&mut app, "doomed").await;
    assert!(rt.database.list_filter_presets().await.unwrap().is_empty());
    assert!(app.error_popup().is_none());
}

// -----------------------------------------------------------------------
// parse_raw_presets tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn parse_raw_presets_converts_all_paths() {
    let raw = vec![(
        "backend".to_string(),
        vec!["/a".to_string(), "/b".to_string()],
        "include".to_string(),
    )];
    let result = parse_raw_presets(raw, None);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, "backend");
    assert_eq!(
        result[0].1,
        HashSet::from(["/a".to_string(), "/b".to_string()])
    );
    assert_eq!(result[0].2, RepoFilterMode::Include);
}

#[tokio::test]
async fn parse_raw_presets_filters_against_known_repos() {
    let raw = vec![(
        "backend".to_string(),
        vec!["/a".to_string(), "/b".to_string(), "/gone".to_string()],
        "exclude".to_string(),
    )];
    let known = HashSet::from(["/a".to_string(), "/b".to_string()]);
    let result = parse_raw_presets(raw, Some(&known));
    assert_eq!(
        result[0].1,
        HashSet::from(["/a".to_string(), "/b".to_string()])
    );
    assert_eq!(result[0].2, RepoFilterMode::Exclude);
}

#[tokio::test]
async fn parse_raw_presets_defaults_invalid_mode() {
    let raw = vec![("x".to_string(), vec![], "bogus".to_string())];
    let result = parse_raw_presets(raw, None);
    assert_eq!(result[0].2, RepoFilterMode::Include);
}

#[tokio::test]
async fn parse_raw_presets_empty_input() {
    let result = parse_raw_presets(vec![], None);
    assert!(result.is_empty());
}

#[tokio::test]
async fn parse_raw_presets_multiple_presets() {
    let raw = vec![
        (
            "a".to_string(),
            vec!["/x".to_string()],
            "include".to_string(),
        ),
        (
            "b".to_string(),
            vec!["/y".to_string()],
            "exclude".to_string(),
        ),
    ];
    let result = parse_raw_presets(raw, None);
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].2, RepoFilterMode::Include);
    assert_eq!(result[1].2, RepoFilterMode::Exclude);
}

// -----------------------------------------------------------------------
// Repo path tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn exec_delete_repo_path_removes_and_refreshes() {
    let (rt, mut app) = test_runtime().await;
    rt.exec_save_repo_path(&mut app, "/repo1".into()).await;
    rt.exec_save_repo_path(&mut app, "/repo2".into()).await;
    assert_eq!(app.repo_paths().len(), 2);

    rt.exec_delete_repo_path(&mut app, "/repo1").await;
    assert_eq!(app.repo_paths().len(), 1);
    assert!(app.repo_paths().contains(&"/repo2".to_string()));
    assert!(app.error_popup().is_none());
}

// -----------------------------------------------------------------------
// Epic tests
// -----------------------------------------------------------------------

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
/// (docs/plans/3809-keybinding-pruning-implementation.md §6): a freshly-enabled
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

// -----------------------------------------------------------------------
// Split mode tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn exec_enter_split_mode_opens_pane() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"%1\n"), // current_pane_id
        MockProcessRunner::ok_with_stdout(b"%2\n"), // split_window_horizontal
    ]));
    let rt = make_runtime(db.clone(), tx, mock).await;

    rt.exec_enter_split_mode().await.unwrap();
    // PaneOpened message arrives via msg_tx — no error message expected.
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            msg,
            Message::Split(crate::tui::messages::SplitMessage::PaneOpened { .. })
        ),
        "Expected PaneOpened, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_enter_split_mode_no_tmux_shows_status() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::fail("no server"), // current_pane_id fails
    ]));
    let rt = make_runtime(db.clone(), tx, mock).await;

    rt.exec_enter_split_mode().await.unwrap();
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            &msg,
            Message::System(crate::tui::messages::SystemMessage::StatusInfo(s)) if s == "Split mode requires tmux"
        ),
        "Expected StatusInfo, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_enter_split_mode_with_task_joins_pane() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    // join_pane no longer needs its own display-message: resolving the source
    // window by exact name already yields that window's pane ID, out of band.
    let mock = Arc::new(
        MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"%1\n"), // current_pane_id
            // companion_pane_ids: no pane carries a dispatch role
            MockProcessRunner::ok_with_stdout(b"%1 \n"),
            MockProcessRunner::ok(), // join_pane: join-pane command
        ])
        .with_windows(&["task-1"]),
    );
    let rt = make_runtime(db.clone(), tx, mock.clone()).await;

    rt.exec_enter_split_mode_with_task(TaskId(1), "task-1")
        .await
        .unwrap();
    let calls = mock.recorded_calls();
    assert!(calls[2].1.contains(&"join-pane".to_string()));
    assert!(
        calls[2].1.contains(&mock.pane_id_of("task-1")),
        "the source must be the resolved pane, not the window name"
    );
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            &msg,
            Message::Split(crate::tui::messages::SplitMessage::PaneOpened {
                task_id: Some(TaskId(1)),
                ..
            })
        ),
        "Expected PaneOpened with task 1, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_enter_split_mode_with_task_kills_leftover_companion_panes_after_join() {
    // The window holds the agent's own pane (%1), the agent-tree companion
    // (%2) and an editor pane opened from it (%5). Once the agent's pane is
    // joined out, both companions must be killed: a lone tree pane is
    // indistinguishable from "hidden" to the agent-tree toggle, and an editor
    // pane has no owner left at all (docs/specs/agent-tree.allium:
    // ToggleVsSplitPaneInteraction).
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(
        MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"%1\n"), // current_pane_id
            // companion_pane_ids: both dispatch-created panes, by their roles.
            MockProcessRunner::ok_with_stdout(b"%1 \n%2 agent_tree\n%5 editor\n"),
            MockProcessRunner::ok(), // join_pane: join-pane
            MockProcessRunner::ok(), // kill-pane %2
            MockProcessRunner::ok(), // kill-pane %5
        ])
        .with_windows(&["task-1"]),
    );
    let rt = make_runtime(db.clone(), tx, mock.clone()).await;

    rt.exec_enter_split_mode_with_task(TaskId(1), "task-1")
        .await
        .unwrap();
    let calls = mock.recorded_calls();
    assert_eq!(calls.len(), 5, "calls: {calls:?}");
    assert_eq!(calls[3].1, vec!["kill-pane", "-t", "%2"]);
    assert_eq!(calls[4].1, vec!["kill-pane", "-t", "%5"]);
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            &msg,
            Message::Split(crate::tui::messages::SplitMessage::PaneOpened {
                task_id: Some(TaskId(1)),
                ..
            })
        ),
        "Expected PaneOpened with task 1, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_enter_split_mode_with_task_succeeds_even_if_companion_check_fails() {
    // A failed companion-pane check must not block the join itself — it's a
    // best-effort cleanup, not the primary action.
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(
        MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"%1\n"), // current_pane_id
            MockProcessRunner::fail("list-panes error"), // companion_pane_ids check
            MockProcessRunner::ok(),                    // join_pane: join-pane
        ])
        .with_windows(&["task-1"]),
    );
    let rt = make_runtime(db.clone(), tx, mock.clone()).await;

    rt.exec_enter_split_mode_with_task(TaskId(1), "task-1")
        .await
        .unwrap();
    let calls = mock.recorded_calls();
    assert_eq!(
        calls.len(),
        3,
        "no kill-pane attempted after a failed check"
    );
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            &msg,
            Message::Split(crate::tui::messages::SplitMessage::PaneOpened { .. })
        ),
        "Expected PaneOpened despite the failed companion check, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_enter_split_mode_with_task_succeeds_even_if_companion_kill_fails() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(
        MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"%1\n"), // current_pane_id
            // companion_pane_ids: tree pane %2, no editor pane
            MockProcessRunner::ok_with_stdout(b"%1 \n%2 agent_tree\n"),
            MockProcessRunner::ok(), // join_pane: join-pane
            MockProcessRunner::fail("kill-pane error"),
        ])
        .with_windows(&["task-1"]),
    );
    let rt = make_runtime(db.clone(), tx, mock.clone()).await;

    rt.exec_enter_split_mode_with_task(TaskId(1), "task-1")
        .await
        .unwrap();
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            &msg,
            Message::Split(crate::tui::messages::SplitMessage::PaneOpened { .. })
        ),
        "Expected PaneOpened despite the failed companion kill, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_exit_split_mode_with_restore_breaks_pane() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // break_pane_to_window
    ]));
    let rt = make_runtime(db.clone(), tx, mock.clone()).await;

    rt.exec_exit_split_mode("%2", Some("task-1")).await.unwrap();
    let calls = mock.recorded_calls();
    assert!(calls[0].1.contains(&"break-pane".to_string()));
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            msg,
            Message::Split(crate::tui::messages::SplitMessage::PaneClosed)
        ),
        "Expected PaneClosed, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_exit_split_mode_without_restore_kills_pane() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // kill_pane
    ]));
    let rt = make_runtime(db.clone(), tx, mock.clone()).await;

    rt.exec_exit_split_mode("%2", None).await.unwrap();
    let calls = mock.recorded_calls();
    assert!(calls[0].1.contains(&"kill-pane".to_string()));
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            msg,
            Message::Split(crate::tui::messages::SplitMessage::PaneClosed)
        ),
        "Expected PaneClosed, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_check_split_pane_existing_pane_no_message() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"%1\n%2\n"), // pane_exists → listing contains %2
    ]));
    let rt = make_runtime(db.clone(), tx, mock).await;

    rt.exec_check_split_pane("%2").await.unwrap();
    assert!(
        rx.try_recv().is_err(),
        "expected no message when pane exists"
    );
}

#[tokio::test]
async fn exec_check_split_pane_gone_sends_closed() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        // pane_exists → the listing no longer contains %2. Note this is a
        // *successful* tmux call: real tmux exits 0 for an unknown pane, which is
        // why absence has to be detected by membership rather than exit status.
        MockProcessRunner::ok_with_stdout(b"%1\n%7\n"),
    ]));
    let rt = make_runtime(db.clone(), tx, mock).await;

    rt.exec_check_split_pane("%2").await.unwrap();
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            msg,
            Message::Split(crate::tui::messages::SplitMessage::PaneClosed)
        ),
        "Expected PaneClosed, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_respawn_split_pane_gone_sends_closed() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::fail("no pane"), // respawn_pane fails when pane is gone
    ]));
    let rt = make_runtime(db.clone(), tx, mock).await;

    rt.exec_respawn_split_pane("%2").await.unwrap();
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            msg,
            Message::Split(crate::tui::messages::SplitMessage::PaneClosed)
        ),
        "Expected PaneClosed when pane gone, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_respawn_split_pane_respawn_fails_sends_closed() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::fail("respawn err"), // respawn_pane fails
    ]));
    let rt = make_runtime(db.clone(), tx, mock).await;

    rt.exec_respawn_split_pane("%2").await.unwrap();
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            msg,
            Message::Split(crate::tui::messages::SplitMessage::PaneClosed)
        ),
        "Expected PaneClosed when respawn fails, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_swap_split_pane_uses_swap_pane() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    // pane_id_for_window resolves out of band, so it is not a recorded call.
    let mock = Arc::new(
        MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // swap-pane
            MockProcessRunner::ok(), // kill-window (old pane had no task)
        ])
        .with_windows(&["task-1"]),
    );
    let rt = make_runtime(db.clone(), tx, mock.clone()).await;

    rt.exec_swap_split_pane(TaskId(1), "task-1", Some("%2"), None)
        .await
        .unwrap();
    let calls = mock.recorded_calls();
    // 1st call: swap-pane, sourced from the resolved pane ID rather than
    // `task-1.0` — a `<window>.<index>` target would prefix-match the window
    // name and depend on pane-base-index.
    assert!(calls[0].1.contains(&"swap-pane".to_string()));
    assert!(calls[0].1.contains(&mock.pane_id_of("task-1")));
    // 2nd call: kill-window (no old task to rename)
    assert!(calls[1].1.contains(&"kill-window".to_string()));
    // No 3rd call — focus must NOT be transferred
    assert_eq!(calls.len(), 2, "select-pane must not be called after swap");
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            &msg,
            Message::Split(crate::tui::messages::SplitMessage::PaneOpened {
                task_id: Some(TaskId(1)),
                ..
            })
        ),
        "Expected PaneOpened with task 1, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_swap_split_pane_renames_old_task_window() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    // pane_id_for_window / the resync's own window lookups resolve out of band,
    // so they are not recorded calls.
    let mock = Arc::new(
        MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // swap-pane
            MockProcessRunner::ok(), // rename-window (old task had a window)
            // resync: list-panes finds the companion. It is still running the
            // *incoming* task's tree (3), which is exactly why it is stale — the
            // lookup matches on the binary and subcommand, not the id.
            MockProcessRunner::ok_with_stdout(b"%10 \n%11 agent_tree\n"),
            MockProcessRunner::ok(), // resync: kill-pane %11
            MockProcessRunner::ok_with_stdout(b"%12\n"), // resync: split-window relaunch
            MockProcessRunner::ok(), // resync: set-option, the new pane's role
        ])
        .with_windows(&["task-3", "task-2"]),
    );
    let rt = make_runtime(db.clone(), tx, mock.clone()).await;

    rt.exec_swap_split_pane(TaskId(3), "task-3", Some("%2"), Some("task-2"))
        .await
        .unwrap();
    let calls = mock.recorded_calls();
    // 2nd call should be rename-window, not kill-window
    assert!(calls[1].1.contains(&"rename-window".to_string()));
    // The rename *target* is the resolved pane ID; the new name stays a name.
    assert!(calls[1].1.contains(&mock.pane_id_of("task-3")));
    assert!(calls[1].1.contains(&"task-2".to_string()));
    // Companion pane resync: the renamed window's stale companion (still
    // showing the incoming task's tree) is killed and replaced with one for
    // the correct (old) task.
    assert!(calls[2].1.contains(&"list-panes".to_string()));
    assert_eq!(calls[3].1, vec!["kill-pane", "-t", "%11"]);
    assert!(calls[4].1.contains(&"split-window".to_string()));
    assert!(calls[4].1.contains(&"2".to_string()));
    // …and the respawned pane is marked, or the resynced window would read as
    // companion-less to the next toggle.
    assert!(calls[5].1.contains(&"set-option".to_string()));
    // No further call — focus must NOT be transferred
    assert_eq!(calls.len(), 6, "select-pane must not be called after swap");
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            &msg,
            Message::Split(crate::tui::messages::SplitMessage::PaneOpened {
                task_id: Some(TaskId(3)),
                ..
            })
        ),
        "Expected PaneOpened with task 3, got: {msg:?}"
    );
}

// -----------------------------------------------------------------------
// Event-loop: split-mode functions send results via msg_tx (not app.update)
// -----------------------------------------------------------------------

#[tokio::test]
async fn exec_enter_split_mode_sends_pane_opened_via_msg_tx() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"%1\n"), // current_pane_id
        MockProcessRunner::ok_with_stdout(b"%2\n"), // split_window_horizontal
    ]));
    let rt = make_runtime(db.clone(), tx, mock).await;

    rt.exec_enter_split_mode().await.unwrap();

    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            &msg,
            Message::Split(crate::tui::messages::SplitMessage::PaneOpened {
                pane_id,
                task_id: None
            }) if pane_id == "%2"
        ),
        "Expected PaneOpened(%2), got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_enter_split_mode_no_tmux_sends_status_info_via_msg_tx() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::fail("no server"), // current_pane_id fails
    ]));
    let rt = make_runtime(db.clone(), tx, mock).await;

    rt.exec_enter_split_mode().await.unwrap();

    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            &msg,
            Message::System(crate::tui::messages::SystemMessage::StatusInfo(s)) if s.contains("tmux")
        ),
        "Expected StatusInfo about tmux, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_enter_split_mode_with_task_sends_pane_opened_via_msg_tx() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(
        MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"%1\n"), // current_pane_id
            // companion_pane_ids: no pane carries a dispatch role
            MockProcessRunner::ok_with_stdout(b"%1 \n"),
            MockProcessRunner::ok(), // join_pane: join-pane command
        ])
        .with_windows(&["task-1"]),
    );
    let rt = make_runtime(db.clone(), tx, mock.clone()).await;

    rt.exec_enter_split_mode_with_task(TaskId(1), "task-1")
        .await
        .unwrap();

    let calls = mock.recorded_calls();
    assert!(calls[2].1.contains(&"join-pane".to_string()));
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            &msg,
            Message::Split(crate::tui::messages::SplitMessage::PaneOpened {
                task_id: Some(TaskId(1)),
                ..
            })
        ),
        "Expected PaneOpened with task, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_exit_split_mode_with_restore_sends_pane_closed_via_msg_tx() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // break_pane_to_window
    ]));
    let rt = make_runtime(db.clone(), tx, mock.clone()).await;

    rt.exec_exit_split_mode("%2", Some("task-1")).await.unwrap();

    let calls = mock.recorded_calls();
    assert!(calls[0].1.contains(&"break-pane".to_string()));
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            msg,
            Message::Split(crate::tui::messages::SplitMessage::PaneClosed)
        ),
        "Expected PaneClosed, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_exit_split_mode_without_restore_sends_pane_closed_via_msg_tx() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // kill_pane
    ]));
    let rt = make_runtime(db.clone(), tx, mock.clone()).await;

    rt.exec_exit_split_mode("%2", None).await.unwrap();

    let calls = mock.recorded_calls();
    assert!(calls[0].1.contains(&"kill-pane".to_string()));
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            msg,
            Message::Split(crate::tui::messages::SplitMessage::PaneClosed)
        ),
        "Expected PaneClosed, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_swap_split_pane_kills_old_window_and_sends_pane_opened_via_msg_tx() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    // pane_id_for_window resolves out of band, so it is not a recorded call.
    let mock = Arc::new(
        MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // swap-pane
            MockProcessRunner::ok(), // kill-window (old pane had no task)
        ])
        .with_windows(&["task-1"]),
    );
    let rt = make_runtime(db.clone(), tx, mock.clone()).await;

    rt.exec_swap_split_pane(TaskId(1), "task-1", Some("%2"), None)
        .await
        .unwrap();

    let calls = mock.recorded_calls();
    assert!(calls[0].1.contains(&"swap-pane".to_string()));
    assert!(calls[1].1.contains(&"kill-window".to_string()));
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            &msg,
            Message::Split(crate::tui::messages::SplitMessage::PaneOpened {
                task_id: Some(TaskId(1)),
                ..
            })
        ),
        "Expected PaneOpened with task, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_focus_split_pane_returns_join_handle() {
    let db = test_db().await;
    let (tx, _rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // select-pane
    ]));
    let rt = make_runtime(db.clone(), tx, mock.clone()).await;

    // Must return a JoinHandle so the caller can fire-and-forget without blocking.
    rt.exec_focus_split_pane("%2".to_string()).await.unwrap();

    let calls = mock.recorded_calls();
    assert!(calls[0].1.contains(&"select-pane".to_string()));
}

// -----------------------------------------------------------------------
// Event-loop: spawn_refresh_from_db sends board data via msg_tx
// -----------------------------------------------------------------------

#[tokio::test]
async fn spawn_refresh_from_db_sends_task_refresh_via_msg_tx() {
    let db = test_db().await;
    // Create a task so the refresh has something to send.
    db.create_task(crate::db::CreateTaskRequest {
        title: "test task",
        description: "desc",
        repo_path: "/repo",
        plan: None,
        status: crate::models::TaskStatus::Backlog,
        epic_id: None,
        sort_order: None,
        tag: None,
        base_branch: "main",
        wrap_up_mode: None,
        auto_run_plan: false,
    })
    .await
    .unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
    let rt = make_runtime(db.clone(), tx, runner).await;

    rt.spawn_refresh_from_db().await.unwrap();

    // First message should be a task Refresh.
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            &msg,
            Message::Task(crate::tui::messages::TaskMessage::Refresh(tasks)) if !tasks.is_empty()
        ),
        "Expected Task::Refresh with tasks, got: {msg:?}"
    );
}

// -----------------------------------------------------------------------
// Browser / tmux window
// -----------------------------------------------------------------------

#[tokio::test]
async fn exec_open_in_browser_calls_xdg_open() {
    let db = test_db().await;
    let (tx, _rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // xdg-open
    ]));
    let rt = make_runtime(db, tx, mock.clone()).await;

    rt.exec_open_in_browser("https://github.com/org/repo/pull/1".into())
        .await
        .unwrap();
    let calls = mock.recorded_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "xdg-open");
    assert!(calls[0]
        .1
        .contains(&"https://github.com/org/repo/pull/1".to_string()));
}

#[tokio::test]
async fn exec_kill_tmux_window_calls_kill() {
    let db = test_db().await;
    let (tx, _rx) = mpsc::unbounded_channel();
    let mock = Arc::new(
        MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // tmux kill-window
        ])
        .with_windows(&["task-1"]),
    );
    let rt = make_runtime(db, tx, mock.clone()).await;

    rt.exec_kill_tmux_window("task-1".into()).await.unwrap();
    let calls = mock.recorded_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "tmux");
    assert!(calls[0].1.contains(&"kill-window".to_string()));
    // Targeted by resolved pane ID, not by name — see `tmux::window_target`.
    assert!(calls[0].1.contains(&mock.pane_id_of("task-1")));
}

#[tokio::test]
async fn exec_kill_tmux_window_failure_is_best_effort() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![MockProcessRunner::fail(
        "no such window",
    )]));
    let rt = make_runtime(db, tx, mock).await;

    rt.exec_kill_tmux_window("gone-window".into())
        .await
        .unwrap();

    // Kill-window failure is best-effort — no error message sent
    assert!(rx.try_recv().is_err(), "Expected no message, but got one");
}

// load_* init helper tests
// -----------------------------------------------------------------------

fn make_app() -> App {
    App::new(vec![])
}

#[tokio::test]
async fn load_notifications_pref_defaults_to_false_when_not_set() {
    let db = Database::open_in_memory().await.unwrap();
    let mut app = make_app();
    load_notifications_pref(&db, &mut app).await;
    assert!(!app.notifications_enabled());
}

#[tokio::test]
async fn load_notifications_pref_sets_true_when_enabled() {
    let db = Database::open_in_memory().await.unwrap();
    db.set_setting_bool("notifications_enabled", true)
        .await
        .unwrap();
    let mut app = make_app();
    load_notifications_pref(&db, &mut app).await;
    assert!(app.notifications_enabled());
}

#[tokio::test]
async fn load_filter_presets_returns_none_on_success() {
    let db = Database::open_in_memory().await.unwrap();
    let mut app = make_app();
    let result = load_filter_presets(&db, &mut app);
    assert!(result.await.is_none());
}

#[tokio::test]
async fn load_filter_presets_loads_saved_presets() {
    let db = Database::open_in_memory().await.unwrap();
    db.save_filter_preset("backend", &["/repo/a".into()], "include")
        .await
        .unwrap();
    let mut app = make_app();
    load_filter_presets(&db, &mut app).await;
    assert_eq!(app.filter_presets().len(), 1);
    assert_eq!(app.filter_presets()[0].0, "backend");
}

#[tokio::test]
async fn apply_tmux_focus_warning_returns_none_when_enabled() {
    let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"on\n")]);
    let result = apply_tmux_focus_warning(&mock);
    assert!(result.is_none());
}

#[tokio::test]
async fn apply_tmux_focus_warning_returns_status_info_when_disabled() {
    let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"off\n")]);
    let result = apply_tmux_focus_warning(&mock);
    assert!(matches!(
        result,
        Some(Message::System(
            crate::tui::messages::SystemMessage::StatusInfo(_)
        ))
    ));
}

// ---------------------------------------------------------------------------
// ensure_statusline_settings_file — Finding 1: bootstrap safety net for the
// dispatch-owned statusline settings file (see src/setup/statusline.rs).
// ---------------------------------------------------------------------------

#[test]
fn ensure_statusline_settings_file_creates_when_absent() {
    let dir = tempfile::tempdir().unwrap();
    let claude_dir = dir.path().join("claude");
    let snapshot_path = dir.path().join("data").join("rate-limits.json");

    ensure_statusline_settings_file_in(&claude_dir, &snapshot_path).unwrap();

    let settings_path = claude_dir.join(crate::setup::statusline::SETTINGS_FILE_NAME);
    assert!(settings_path.exists(), "settings file must be created");
    let content = std::fs::read_to_string(&settings_path).unwrap();
    assert!(content.contains("dispatch statusline"));
}

#[test]
fn ensure_statusline_settings_file_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let claude_dir = dir.path().join("claude");
    let snapshot_path = dir.path().join("data").join("rate-limits.json");

    ensure_statusline_settings_file_in(&claude_dir, &snapshot_path).unwrap();
    let settings_path = claude_dir.join(crate::setup::statusline::SETTINGS_FILE_NAME);
    let first = std::fs::read_to_string(&settings_path).unwrap();

    // A normal TUI start on an already-configured machine must not rewrite
    // the file (setup's write_settings_file already guarantees this; this
    // asserts bootstrap doesn't bypass that guarantee).
    ensure_statusline_settings_file_in(&claude_dir, &snapshot_path).unwrap();
    let second = std::fs::read_to_string(&settings_path).unwrap();
    assert_eq!(first, second);
}

#[test]
fn ensure_statusline_settings_file_errors_when_directory_unwritable() {
    // Point `claude_dir` at a path whose parent is a *file*, not a directory
    // — `create_dir_all` fails deterministically without touching real
    // permission bits (which vary by OS/CI and can be blocked by sandboxing).
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"not a directory").unwrap();
    let claude_dir = blocker.join("claude");
    let snapshot_path = dir.path().join("rate-limits.json");

    let result = ensure_statusline_settings_file_in(&claude_dir, &snapshot_path);

    assert!(
        result.is_err(),
        "must surface an error rather than silently doing nothing"
    );
}

// ---------------------------------------------------------------------------
// exec_trigger_epic_feed
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// SerialisedFeedCycle (feeds.allium) — one feed cycle per epic at a time.
//
// The race these close: nothing used to serialise a manual "r" refresh against
// an in-flight auto-poll for the SAME epic, so the two could interleave between
// the non-transactional steps of run_role_routed_feed_sync. Each of those steps
// filters on a task's CURRENT epic_id, so one pass could see a task the other
// had moved-but-not-yet-committed as absent from its keep-set, delete it, and --
// since feed deletes now feed TaskTeardown -- force-remove a live review agent's
// worktree.
// ---------------------------------------------------------------------------

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

// ── exec_open_main_session ──

#[tokio::test]
async fn exec_open_jumps_when_window_alive() {
    let db = test_db().await;
    let (tx, _rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"dispatch-main\n"), // has_window → true
        MockProcessRunner::ok(),                               // select-window
    ]));
    let rt = make_runtime(db.clone(), tx, mock.clone()).await;
    let mut app = make_app();

    rt.exec_open_main_session(&mut app).await;

    let calls = mock.recorded_calls();
    // Jumped to the live window — never created one, never opened the picker.
    assert!(!calls
        .iter()
        .any(|(_, args)| args.contains(&"new-window".to_string())));
    assert_ne!(app.mode(), &crate::tui::InputMode::MainSessionDir);
    assert!(app.error_popup().is_none());
}

#[tokio::test]
async fn exec_open_enters_picker_when_no_window() {
    let db = test_db().await;
    let (tx, _rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // has_window → false (empty list)
    ]));
    let rt = make_runtime(db.clone(), tx, mock.clone()).await;
    let mut app = make_app();
    // A previously-configured dir does not stop the picker from re-prompting.
    app.set_main_session_dir(Some("/home/user".to_string()));

    rt.exec_open_main_session(&mut app).await;

    // No live window — opened the picker to (re)select the directory.
    assert_eq!(app.mode(), &crate::tui::InputMode::MainSessionDir);
    let calls = mock.recorded_calls();
    assert!(!calls
        .iter()
        .any(|(_, args)| args.contains(&"new-window".to_string())));
    assert!(app.error_popup().is_none());
}

// ── exec_check_main_session_liveness (MainSessionIndicator poll) ──

// @guarantee LivenessFromLiveTmuxCheck: the poll derives liveness from a live
// tmux has-window check and reports true when the window is present.
#[tokio::test]
async fn exec_check_liveness_emits_alive_when_window_present() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"dispatch-main\n"), // has_window → true
    ]));
    let rt = make_runtime(db.clone(), tx, mock).await;

    rt.exec_check_main_session_liveness().await.unwrap();

    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            &msg,
            Message::MainSession(crate::tui::messages::MainSessionMessage::LivenessChanged(
                true
            ))
        ),
        "expected LivenessChanged(true), got: {msg:?}"
    );
}

// @guarantee LivenessFromLiveTmuxCheck: reports false when the window is absent.
#[tokio::test]
async fn exec_check_liveness_emits_not_alive_when_window_absent() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // has_window → false (empty list)
    ]));
    let rt = make_runtime(db.clone(), tx, mock).await;

    rt.exec_check_main_session_liveness().await.unwrap();

    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            &msg,
            Message::MainSession(crate::tui::messages::MainSessionMessage::LivenessChanged(
                false
            ))
        ),
        "expected LivenessChanged(false), got: {msg:?}"
    );
}

// ── exec_create_main_session ──

#[tokio::test]
async fn exec_create_makes_window_and_jumps_without_persisting_window() {
    let db = test_db().await;
    let (tx, _rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // new-window
        MockProcessRunner::ok(), // send-keys -l
        MockProcessRunner::ok(), // send-keys Enter
        MockProcessRunner::ok(), // select-window
    ]));
    let rt = make_runtime(db.clone(), tx, mock.clone()).await;
    let mut app = make_app();
    app.set_main_session_dir(Some("/home/user".to_string()));

    rt.exec_create_main_session(&mut app).await;

    let calls = mock.recorded_calls();
    assert!(calls
        .iter()
        .any(|(_, args)| args.contains(&"new-window".to_string())));
    assert!(app.error_popup().is_none());
    // The window identity is never persisted.
    let stored = db.get_setting_string("main_session.window").await.unwrap();
    assert!(stored.as_deref().unwrap_or("").is_empty());
}

#[tokio::test]
async fn exec_create_with_no_dir_errors() {
    let (rt, mut app) = test_runtime().await;
    rt.exec_create_main_session(&mut app).await;
    assert!(app.error_popup().is_some());
}

// ── load_main_session ──

#[tokio::test]
async fn load_main_session_sets_dir_from_db() {
    let db = Database::open_in_memory().await.unwrap();
    db.set_setting_string("main_session.dir", "/home/user/code")
        .await
        .unwrap();
    let mut app = make_app();

    load_main_session(&db, &mut app).await;

    assert_eq!(app.main_session_dir(), Some("/home/user/code"));
}

#[tokio::test]
async fn load_main_session_ignores_empty_dir() {
    let db = Database::open_in_memory().await.unwrap();
    db.set_setting_string("main_session.dir", "").await.unwrap();
    let mut app = make_app();

    load_main_session(&db, &mut app).await;

    assert_eq!(app.main_session_dir(), None);
}

#[tokio::test]
async fn build_learning_injections_partitions_and_records_retrievals() {
    use crate::models::{LearningKind, LearningScope, RetrievalSource};
    use crate::service::embeddings::{serialize_embedding, EmbeddingService};

    let (rt, _app) = test_runtime().await;
    // Seed a task in the default project.
    let task = create_task_returning(
        &**rt.db_write(),
        "title",
        "desc",
        "/repo/a",
        None,
        models::TaskStatus::Backlog,
    )
    .await
    .unwrap();

    // RAG pipeline requires stored embeddings. Seed fake BLOB bytes so both
    // learnings survive the `embedding IS NULL` filter.
    let fake_emb = serialize_embedding(&[0.1f32; 384]);

    // Seed two approved learnings: one repo-scoped non-procedural, one
    // user-scoped procedural. Both should land in the dispatch list for
    // a task in /repo/a.
    let proc_id = rt
        .database
        .create_learning(CreateLearningRow {
            kind: LearningKind::Procedural,
            summary: "Always run tests before committing.",
            detail: None,
            scope: LearningScope::User,
            scope_ref: None,
            tags: &[],
            source_task_id: None,
            embedding: Some(&fake_emb),
        })
        .await
        .unwrap();
    let repo_id = rt
        .database
        .create_learning(CreateLearningRow {
            kind: LearningKind::Convention,
            summary: "Use Arc for shared state.",
            detail: None,
            scope: LearningScope::Repo,
            scope_ref: Some("/repo/a"),
            tags: &[],
            source_task_id: None,
            embedding: Some(&fake_emb),
        })
        .await
        .unwrap();

    let emb_svc = EmbeddingService::new_test();
    let injected =
        crate::dispatch::build_and_record_injections(&*rt.database, &task, &emb_svc).await;
    assert_eq!(injected.len(), 2);
    let ids: Vec<_> = injected.iter().map(|l| l.id).collect();
    assert!(ids.contains(&proc_id));
    assert!(ids.contains(&repo_id));

    let rows = rt.database.list_retrievals_for_task(task.id).await.unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows
        .iter()
        .all(|r| matches!(r.source, RetrievalSource::PromptInjection)));
}

// ---------------------------------------------------------------------------
// prepare_inputs tests
// ---------------------------------------------------------------------------
//
// The shared dispatch prologue. Four launch sites (dispatch_task and the epic
// chain in src/mcp/handlers/tasks/dispatch.rs, exec_quick_dispatch and
// exec_dispatch_agent in src/runtime/tasks.rs) run it; their own end-to-end
// tests cover the wiring, these pin the prologue itself.

#[tokio::test]
async fn prepare_inputs_reads_epic_context_and_injections() {
    use crate::db::CreateLearningRow;
    use crate::models::{LearningKind, LearningScope, RetrievalSource};
    use crate::service::embeddings::{serialize_embedding, EmbeddingService};

    let (rt, _app) = test_runtime().await;
    let db = rt.db_write().clone();
    let epic = db.create_epic("Chained Epic", "desc", None).await.unwrap();
    let task_id = db
        .create_task(CreateTaskRequest {
            title: "title",
            description: "desc",
            repo_path: "/repo/a",
            plan: None,
            status: models::TaskStatus::Backlog,
            base_branch: "main",
            epic_id: Some(epic.id),
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let task = db.get_task(task_id).await.unwrap().unwrap();
    let learning_id = db
        .create_learning(CreateLearningRow {
            kind: LearningKind::Convention,
            summary: "Use Arc for shared state.",
            detail: None,
            scope: LearningScope::Repo,
            scope_ref: Some("/repo/a"),
            tags: &[],
            source_task_id: None,
            embedding: Some(&serialize_embedding(&[0.1f32; 384])),
        })
        .await
        .unwrap();

    let inputs = crate::dispatch::prepare_inputs(&*db, &task, &EmbeddingService::new_test()).await;

    let epic_ctx = inputs.epic_ctx.expect("epic context read from the DB");
    assert_eq!(epic_ctx.epic_id, epic.id);
    assert_eq!(epic_ctx.epic_title, "Chained Epic");
    assert_eq!(
        inputs.injected.iter().map(|l| l.id).collect::<Vec<_>>(),
        vec![learning_id]
    );

    // The prologue's side effect: each injection is recorded as a retrieval.
    let rows = db.list_retrievals_for_task(task.id).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert!(matches!(rows[0].source, RetrievalSource::PromptInjection));
}

#[tokio::test]
async fn prepare_inputs_with_epic_ctx_uses_the_supplied_context() {
    use crate::service::embeddings::EmbeddingService;

    let (rt, _app) = test_runtime().await;
    let db = rt.db_write().clone();
    // Deliberately epic-less: a from_db read would yield None, so seeing the
    // supplied context proves it was not re-read.
    let task = create_task_returning(
        &*db,
        "title",
        "desc",
        "/repo/a",
        None,
        models::TaskStatus::Backlog,
    )
    .await
    .unwrap();
    let supplied = crate::dispatch::EpicContext {
        epic_id: models::EpicId(7),
        epic_title: "Already in hand".to_string(),
    };

    let inputs = crate::dispatch::prepare_inputs_with_epic_ctx(
        &*db,
        &task,
        &EmbeddingService::new_test(),
        Some(supplied),
    )
    .await;

    let epic_ctx = inputs.epic_ctx.expect("the supplied context is returned");
    assert_eq!(epic_ctx.epic_id, models::EpicId(7));
    assert_eq!(epic_ctx.epic_title, "Already in hand");
    assert!(inputs.injected.is_empty());
}

// ---------------------------------------------------------------------------
// backfill_embeddings tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn backfill_fills_missing_embeddings() {
    use crate::db::{CreateLearningRow, LearningStore};
    use crate::models::{LearningKind, LearningScope};
    use crate::service::embeddings::EmbeddingService;

    let db = Arc::new(Database::open_in_memory().await.unwrap());

    // Insert two learnings without embeddings.
    let id1 = db
        .create_learning(CreateLearningRow {
            kind: LearningKind::Convention,
            summary: "always use snake_case",
            detail: None,
            scope: LearningScope::User,
            scope_ref: None,
            tags: &[],
            source_task_id: None,
            embedding: None,
        })
        .await
        .unwrap();
    let id2 = db
        .create_learning(CreateLearningRow {
            kind: LearningKind::Pitfall,
            summary: "avoid unwrap in production",
            detail: Some("use ? or expect with a message"),
            scope: LearningScope::User,
            scope_ref: None,
            tags: &["rust".to_string()],
            source_task_id: None,
            embedding: None,
        })
        .await
        .unwrap();

    // Confirm both are missing embeddings before backfill.
    let missing_before = db.list_learnings_missing_embedding().await.unwrap();
    assert_eq!(
        missing_before.len(),
        2,
        "expected 2 learnings missing embeddings"
    );

    // Run the backfill using the test stub service.
    let emb_svc = EmbeddingService::new_noop();
    let db_for_backfill: Arc<dyn crate::db::LearningStore + Send + Sync> = db.clone();
    super::backfill_embeddings(db_for_backfill, emb_svc)
        .await
        .unwrap();

    // After backfill, no learnings should be missing embeddings.
    let missing_after = db.list_learnings_missing_embedding().await.unwrap();
    assert!(
        missing_after.is_empty(),
        "expected 0 learnings missing embeddings after backfill, got {}",
        missing_after.len()
    );

    // Both learnings should now have non-empty embeddings stored.
    let l1 = db.get_learning(id1).await.unwrap().unwrap();
    let l2 = db.get_learning(id2).await.unwrap().unwrap();
    // Verify via list_all_approved_non_task_learnings which returns embeddings
    let all = db.list_all_approved_non_task_learnings().await.unwrap();
    let emb1 = all.iter().find(|(l, _)| l.id == l1.id).map(|(_, e)| e);
    let emb2 = all.iter().find(|(l, _)| l.id == l2.id).map(|(_, e)| e);
    assert!(
        emb1.is_some_and(|e| !e.is_empty()),
        "learning 1 should have embedding"
    );
    assert!(
        emb2.is_some_and(|e| !e.is_empty()),
        "learning 2 should have embedding"
    );
}

#[tokio::test]
async fn backfill_is_noop_when_no_missing_embeddings() {
    use crate::db::{CreateLearningRow, LearningStore};
    use crate::models::{LearningKind, LearningScope};
    use crate::service::embeddings::{serialize_embedding, EmbeddingService};

    let db = Arc::new(Database::open_in_memory().await.unwrap());

    // Insert a learning that already has an embedding.
    let sentinel = serialize_embedding(&vec![0.1f32; 384]);
    db.create_learning(CreateLearningRow {
        kind: LearningKind::Convention,
        summary: "already embedded",
        detail: None,
        scope: LearningScope::User,
        scope_ref: None,
        tags: &[],
        source_task_id: None,
        embedding: Some(&sentinel),
    })
    .await
    .unwrap();

    let missing_before = db.list_learnings_missing_embedding().await.unwrap();
    assert!(
        missing_before.is_empty(),
        "precondition: no missing embeddings"
    );

    // Backfill should succeed without doing any work.
    let emb_svc = EmbeddingService::new_noop();
    let db_for_backfill: Arc<dyn crate::db::LearningStore + Send + Sync> = db.clone();
    super::backfill_embeddings(db_for_backfill, emb_svc)
        .await
        .unwrap();

    let missing_after = db.list_learnings_missing_embedding().await.unwrap();
    assert!(
        missing_after.is_empty(),
        "still no missing embeddings after no-op backfill"
    );
}

// ---------------------------------------------------------------------------
// spawn_refresh_task
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spawn_refresh_task_sends_updated_task_via_msg_tx() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
    let rt = make_runtime(db.clone(), tx, runner).await;
    let tasks = db.list_all().await.unwrap();
    let mut app = App::new(tasks);
    rt.exec_insert_task(
        &mut app,
        tui::TaskDraft {
            title: "Refresh Me".into(),
            description: "Desc".into(),
            repo_path: "/repo".into(),
            ..Default::default()
        },
        None,
    )
    .await;
    let id = app.tasks()[0].id;
    rt.db_write()
        .patch_task(
            id,
            &db::TaskPatch::new()
                .status(models::TaskStatus::Running)
                .sub_status(models::SubStatus::Active),
        )
        .await
        .unwrap();

    rt.spawn_refresh_task(id).await.unwrap();

    // Drain messages to find the Updated one.
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            &msg,
            Message::Task(crate::tui::messages::TaskMessage::Updated(t)) if t.status == models::TaskStatus::Running
        ),
        "Expected Updated with Running status, got: {msg:?}"
    );
}

#[tokio::test]
async fn spawn_refresh_task_falls_back_when_task_gone() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
    let rt = make_runtime(db.clone(), tx, runner).await;
    let tasks = db.list_all().await.unwrap();
    let mut app = App::new(tasks);
    rt.exec_insert_task(
        &mut app,
        tui::TaskDraft {
            title: "Gone Task".into(),
            description: "Desc".into(),
            repo_path: "/repo".into(),
            ..Default::default()
        },
        None,
    )
    .await;
    let id = app.tasks()[0].id;
    rt.db_write().delete_task(id).await.unwrap();

    rt.spawn_refresh_task(id).await.unwrap();

    // The fallback sends a Refresh message with an empty list.
    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            &msg,
            Message::Task(crate::tui::messages::TaskMessage::Refresh(tasks)) if tasks.is_empty()
        ),
        "Expected empty Refresh fallback, got: {msg:?}"
    );
}

// ---------------------------------------------------------------------------
// spawn_refresh_epic
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spawn_refresh_epic_sends_updated_epic_via_msg_tx() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
    let rt = make_runtime(db.clone(), tx, runner).await;
    let epic = db.create_epic("Epic", "desc", None).await.unwrap();
    db.patch_epic(
        epic.id,
        &db::EpicPatch::new().status(models::TaskStatus::Running),
    )
    .await
    .unwrap();

    rt.spawn_refresh_epic(epic.id).await.unwrap();

    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            &msg,
            Message::Epic(crate::tui::messages::EpicMessage::Updated(e)) if e.status == models::TaskStatus::Running
        ),
        "Expected Updated epic with Running status, got: {msg:?}"
    );
}

#[tokio::test]
async fn spawn_refresh_epic_falls_back_when_epic_gone() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
    let rt = make_runtime(db.clone(), tx, runner).await;
    let epic = db.create_epic("Gone Epic", "desc", None).await.unwrap();
    db.delete_epic(epic.id).await.unwrap();

    rt.spawn_refresh_epic(epic.id).await.unwrap();

    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    // Fallback sends a full Refresh (tasks list, may be empty).
    assert!(
        matches!(
            msg,
            Message::Task(crate::tui::messages::TaskMessage::Refresh(_))
        ),
        "Expected Task::Refresh fallback, got: {msg:?}"
    );
}

#[tokio::test]
async fn spawn_refresh_epic_also_sends_epic_tasks_via_msg_tx() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
    let rt = make_runtime(db.clone(), tx, runner).await;
    let epic = db.create_epic("Feed Epic", "desc", None).await.unwrap();
    db.create_task(crate::db::CreateTaskRequest {
        title: "Feed Task",
        description: "from feed",
        repo_path: "/repo",
        plan: None,
        status: models::TaskStatus::Backlog,
        base_branch: "main",
        epic_id: Some(epic.id),
        sort_order: None,
        tag: None,
        wrap_up_mode: None,
        auto_run_plan: false,
    })
    .await
    .unwrap();

    rt.spawn_refresh_epic(epic.id).await.unwrap();

    // First message: EpicMessage::Updated
    let msg1 = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            msg1,
            Message::Epic(crate::tui::messages::EpicMessage::Updated(_))
        ),
        "Expected Epic::Updated first, got: {msg1:?}"
    );
    // Second message: TaskMessage::Updated for the linked task
    let msg2 = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            &msg2,
            Message::Task(crate::tui::messages::TaskMessage::Updated(t)) if t.title == "Feed Task"
        ),
        "Expected Task::Updated with 'Feed Task', got: {msg2:?}"
    );
}

// ---------------------------------------------------------------------------
// exec_toggle_epic_auto_dispatch / exec_toggle_epic_group_by_repo
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// exec_toggle_epic_group_by_repo — migration behaviour
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Frame rate cap
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Event loop: next_loop_event / apply_loop_event / run_loop
// ---------------------------------------------------------------------------

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
        })
        .await
        .unwrap();
    // Give the task a live tmux window so the tick has something to sweep.
    db.patch_task(
        id,
        &crate::db::TaskPatch::new().tmux_window(Some("dispatch:1")),
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

// ---------------------------------------------------------------------------
// commands::dispatch — Command → side-effect coverage
// ---------------------------------------------------------------------------
//
// `commands::dispatch` (`src/runtime/commands.rs`) is the single entry point
// `execute_commands` uses to turn a `Command` into a side effect. The rest of
// this file exercises the `exec_*` handlers directly, which leaves the
// dispatcher itself — the wiring from variant to handler, and which arms feed
// follow-on commands back into the queue — unexecuted.
//
// Assertions here are on *observable effects* (DB rows, `App` state, returned
// follow-on commands), never on the shape of the match, so they survive a
// refactor of the dispatcher's internals.
mod command_dispatch {
    use super::*;
    use crate::tui::commands::{EditorCommand, TaskCommand, TodoCommand};

    /// Run one command through the real dispatcher and return its follow-on
    /// commands (the vec `execute_commands` extends its queue with).
    async fn dispatch_one(rt: &TuiRuntime, app: &mut App, cmd: Command) -> Vec<Command> {
        commands::dispatch(cmd, app, rt).await
    }

    /// Mirror of `execute_commands`' drain loop without the terminal/key-rx
    /// plumbing: run `cmd` and every command it cascades into.
    async fn drain(rt: &TuiRuntime, app: &mut App, cmd: Command) {
        let mut queue = std::collections::VecDeque::from(vec![cmd]);
        while let Some(command) = queue.pop_front() {
            queue.extend(commands::dispatch(command, app, rt).await);
        }
    }

    async fn seed(rt: &TuiRuntime, title: &str, status: models::TaskStatus) -> models::Task {
        create_task_returning(&**rt.db_write(), title, "desc", "/repo", None, status)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn dispatch_save_repo_path_persists_and_updates_app() {
        let (rt, mut app) = test_runtime().await;

        let extra = dispatch_one(&rt, &mut app, Command::SaveRepoPath("/some/repo".into())).await;

        assert!(
            extra.is_empty(),
            "SaveRepoPath produces no follow-on commands"
        );
        assert!(
            rt.database
                .list_repo_paths()
                .await
                .unwrap()
                .contains(&"/some/repo".to_string()),
            "repo path should be persisted"
        );
        assert!(
            app.repo_paths().contains(&"/some/repo".to_string()),
            "App should have been refreshed with the new path"
        );
    }

    #[tokio::test]
    async fn dispatch_save_base_branch_persists_and_updates_app() {
        let (rt, mut app) = test_runtime().await;

        dispatch_one(
            &rt,
            &mut app,
            Command::SaveBaseBranch("/some/repo".into(), "develop".into()),
        )
        .await;

        let pairs = rt.database.list_all_base_branches().await.unwrap();
        assert!(
            pairs
                .iter()
                .any(|(repo, branch)| repo == "/some/repo" && branch == "develop"),
            "base branch should be persisted, got {pairs:?}"
        );
    }

    #[tokio::test]
    async fn dispatch_task_persist_writes_status_to_db() {
        let (rt, mut app) = test_runtime().await;
        let mut task = seed(&rt, "Persist me", models::TaskStatus::Backlog).await;
        // Mirror what every production status move does: the sub-status travels
        // with the status (`SubStatus::default_for`), because the service
        // rejects any (status, sub_status) pair `SubStatus::is_valid_for`
        // disallows.
        task.status = models::TaskStatus::Running;
        task.sub_status = models::SubStatus::default_for(models::TaskStatus::Running);
        task.worktree = Some("/wt".into());

        dispatch_one(
            &rt,
            &mut app,
            Command::Task(TaskCommand::Persist(task.clone())),
        )
        .await;

        let stored = rt.database.get_task(task.id).await.unwrap().unwrap();
        assert_eq!(stored.status, models::TaskStatus::Running);
        assert_eq!(stored.sub_status, models::SubStatus::Active);
        assert_eq!(stored.worktree.as_deref(), Some("/wt"));
        assert!(
            app.dirty_since_refresh,
            "a successful persist must mark the board dirty so the next tick refreshes"
        );
    }

    #[tokio::test]
    async fn dispatch_task_persist_surfaces_service_rejection_and_writes_nothing() {
        let (rt, mut app) = test_runtime().await;
        let mut task = seed(&rt, "Persist me", models::TaskStatus::Backlog).await;
        // Running + SubStatus::None is rejected by `SubStatus::is_valid_for`, so
        // the whole patch is refused — including the worktree field.
        task.status = models::TaskStatus::Running;
        task.worktree = Some("/wt".into());

        dispatch_one(
            &rt,
            &mut app,
            Command::Task(TaskCommand::Persist(task.clone())),
        )
        .await;

        let stored = rt.database.get_task(task.id).await.unwrap().unwrap();
        assert_eq!(stored.status, models::TaskStatus::Backlog);
        assert_eq!(stored.worktree, None);
        let err = app.error_popup().unwrap_or_default();
        assert!(
            err.contains("persisting task"),
            "the rejection must reach the user as an error popup, got {err:?}"
        );
    }

    #[tokio::test]
    async fn dispatch_task_delete_removes_the_row() {
        let (rt, mut app) = test_runtime().await;
        let task = seed(&rt, "Delete me", models::TaskStatus::Backlog).await;

        dispatch_one(&rt, &mut app, Command::Task(TaskCommand::Delete(task.id))).await;

        assert!(
            rt.database.get_task(task.id).await.unwrap().is_none(),
            "the task row should be gone"
        );
    }

    /// `ClearWorktreePointer` is the write a *successful* teardown earns: it is the
    /// only thing that clears the worktree column on the archive path.
    #[tokio::test]
    async fn dispatch_task_clear_worktree_pointer_clears_both_pointers() {
        let (rt, mut app) = test_runtime().await;
        let task = seed(&rt, "Torn down", models::TaskStatus::Archived).await;
        rt.db_write()
            .patch_task(
                task.id,
                &db::TaskPatch::new()
                    .worktree(Some("/repo/.worktrees/1-torn-down"))
                    .tmux_window(Some("task-1")),
            )
            .await
            .unwrap();

        dispatch_one(
            &rt,
            &mut app,
            Command::Task(TaskCommand::ClearWorktreePointer(task.id)),
        )
        .await;

        let stored = rt.database.get_task(task.id).await.unwrap().unwrap();
        assert_eq!(stored.worktree, None);
        assert_eq!(stored.tmux_window, None);
    }

    #[tokio::test]
    async fn dispatch_task_move_to_epic_reparents_and_cascades_a_refresh() {
        let (rt, mut app) = test_runtime().await;
        let task = seed(&rt, "Adopt me", models::TaskStatus::Backlog).await;
        let epic_id = rt
            .db_write()
            .create_epic("Parent epic", "desc", None)
            .await
            .unwrap()
            .id;

        let extra = dispatch_one(
            &rt,
            &mut app,
            Command::Task(TaskCommand::MoveToEpic {
                id: task.id,
                new_epic: Some(epic_id),
            }),
        )
        .await;

        let stored = rt.database.get_task(task.id).await.unwrap().unwrap();
        assert_eq!(
            stored.epic_id,
            Some(epic_id),
            "the task should now belong to the epic"
        );
        // MoveToEpic chains into exec_refresh_from_db, so App must see the move
        // without any further command being dispatched by the caller.
        assert!(
            app.tasks().iter().any(|t| t.id == task.id),
            "the refreshed board should contain the moved task"
        );
        // Any follow-on commands the refresh produced must be drainable, not
        // silently dropped.
        for cmd in extra {
            drain(&rt, &mut app, cmd).await;
        }
    }

    #[tokio::test]
    async fn dispatch_task_refresh_from_db_syncs_app_from_db() {
        let (rt, mut app) = test_runtime().await;
        assert!(app.tasks().is_empty(), "precondition: empty board");
        let task = seed(
            &rt,
            "Written behind App's back",
            models::TaskStatus::Backlog,
        )
        .await;

        dispatch_one(&rt, &mut app, Command::Task(TaskCommand::RefreshFromDb)).await;

        assert_eq!(app.tasks().len(), 1);
        assert_eq!(app.tasks()[0].id, task.id);
    }

    #[tokio::test]
    async fn dispatch_task_patch_sub_status_writes_to_db() {
        let (rt, mut app) = test_runtime().await;
        let task = seed(&rt, "Patch me", models::TaskStatus::Running).await;

        dispatch_one(
            &rt,
            &mut app,
            Command::Task(TaskCommand::PatchSubStatus {
                id: task.id,
                sub_status: models::SubStatus::NeedsInput,
            }),
        )
        .await;

        let stored = rt.database.get_task(task.id).await.unwrap().unwrap();
        assert_eq!(stored.sub_status, models::SubStatus::NeedsInput);
    }

    #[tokio::test]
    async fn dispatch_epic_insert_then_persist_writes_to_db() {
        let (rt, mut app) = test_runtime().await;

        dispatch_one(
            &rt,
            &mut app,
            Command::Epic(crate::tui::commands::EpicCommand::Insert(tui::EpicDraft {
                title: "Epic via dispatch".into(),
                description: "desc".into(),
                parent_epic_id: None,
            })),
        )
        .await;

        let epics = rt.database.list_epics().await.unwrap();
        assert_eq!(epics.len(), 1);
        assert_eq!(epics[0].title, "Epic via dispatch");
    }

    #[tokio::test]
    async fn dispatch_persist_setting_writes_both_kinds() {
        let (rt, mut app) = test_runtime().await;

        dispatch_one(
            &rt,
            &mut app,
            Command::PersistSetting {
                key: "notifications_enabled".into(),
                value: true,
            },
        )
        .await;
        dispatch_one(
            &rt,
            &mut app,
            Command::PersistStringSetting {
                key: "main_session_dir".into(),
                value: "/main".into(),
            },
        )
        .await;

        assert_eq!(
            rt.database
                .get_setting_bool("notifications_enabled")
                .await
                .unwrap(),
            Some(true)
        );
        assert_eq!(
            rt.database
                .get_setting_string("main_session_dir")
                .await
                .unwrap()
                .as_deref(),
            Some("/main")
        );
    }

    #[tokio::test]
    async fn dispatch_editor_pop_out_refuses_when_a_session_is_already_open() {
        let (rt, mut app) = test_runtime().await;
        let task = seed(&rt, "Already editing", models::TaskStatus::Backlog).await;
        *rt.editor_session.lock().unwrap() =
            Some(super::editor::EditorSession::occupied_for_test("edit-1"));

        dispatch_one(
            &rt,
            &mut app,
            Command::Editor(EditorCommand::PopOut(crate::tui::EditKind::TaskEdit(task))),
        )
        .await;

        assert_eq!(
            app.status_message(),
            Some(super::editor::EDITOR_ALREADY_OPEN_MSG),
            "the guard must surface a status message instead of opening a second editor"
        );
    }

    #[tokio::test]
    async fn dispatch_editor_finalize_result_persists_the_edit() {
        let (rt, mut app) = test_runtime().await;
        let task = seed(&rt, "Old title", models::TaskStatus::Backlog).await;
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

        drain(
            &rt,
            &mut app,
            Command::Editor(EditorCommand::FinalizeResult {
                kind: crate::tui::EditKind::TaskEdit(task.clone()),
                outcome: crate::tui::EditorOutcome::Saved(edited.into()),
            }),
        )
        .await;

        let stored = rt.database.get_task(task.id).await.unwrap().unwrap();
        assert_eq!(stored.title, "New title");
        assert_eq!(stored.description, "New description");
    }

    #[tokio::test]
    async fn dispatch_todo_create_links_to_a_task_in_the_shared_database() {
        // Regression guard for the fixture itself: `todos.task_id` has a real
        // foreign key onto `tasks(id)` (migration v68), so a todo can only be
        // linked to a task when both live in the *same* database. When
        // `make_runtime` gave `todo_svc` its own database the insert failed the
        // FK check, `exec_create_todo` swallowed the error into a warning, and
        // the whole todo↔task coupling was invisible to every test.
        let (rt, mut app) = test_runtime().await;
        let task = seed(&rt, "Linked task", models::TaskStatus::Backlog).await;

        dispatch_one(
            &rt,
            &mut app,
            Command::Todo(TodoCommand::Create {
                title: "Follow up on the linked task".into(),
                linked: Some(crate::models::TodoLink::Task(task.id)),
                reopen: false,
            }),
        )
        .await;

        let todos = rt.todo_svc.list_todos().await.unwrap();
        assert_eq!(todos.len(), 1, "the todo must have been inserted");
        assert_eq!(
            todos[0].linked,
            Some(crate::models::TodoLink::Task(task.id)),
            "both sides must observe the link"
        );
        assert!(
            rt.database.get_task(task.id).await.unwrap().is_some(),
            "the task must be readable from the same database the todo links into"
        );
        assert_eq!(app.todo_open_count(), 1);
    }

    #[tokio::test]
    async fn dispatch_todo_update_and_delete_reach_the_service() {
        let (rt, mut app) = test_runtime().await;
        dispatch_one(
            &rt,
            &mut app,
            Command::Todo(TodoCommand::Create {
                title: "Transient".into(),
                linked: None,
                reopen: false,
            }),
        )
        .await;
        let id = rt.todo_svc.list_todos().await.unwrap()[0].id;

        dispatch_one(
            &rt,
            &mut app,
            Command::Todo(TodoCommand::Update {
                id,
                update: crate::service::todos::TodoUpdate {
                    done: Some(true),
                    ..Default::default()
                },
            }),
        )
        .await;
        assert!(rt.todo_svc.list_todos().await.unwrap()[0].done);

        dispatch_one(&rt, &mut app, Command::Todo(TodoCommand::ClearDone)).await;
        assert!(
            rt.todo_svc.list_todos().await.unwrap().is_empty(),
            "ClearDone should have removed the completed todo"
        );

        dispatch_one(
            &rt,
            &mut app,
            Command::Todo(TodoCommand::Create {
                title: "Doomed".into(),
                linked: None,
                reopen: false,
            }),
        )
        .await;
        let id = rt.todo_svc.list_todos().await.unwrap()[0].id;
        dispatch_one(&rt, &mut app, Command::Todo(TodoCommand::Delete(id))).await;
        assert!(rt.todo_svc.list_todos().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn dispatch_task_insert_writes_a_new_row() {
        let (rt, mut app) = test_runtime().await;

        dispatch_one(
            &rt,
            &mut app,
            Command::Task(TaskCommand::Insert {
                draft: tui::TaskDraft {
                    title: "Inserted via dispatch".into(),
                    description: "desc".into(),
                    repo_path: "/repo".into(),
                    ..Default::default()
                },
                epic_id: None,
            }),
        )
        .await;

        let tasks = rt.database.list_all().await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].title, "Inserted via dispatch");
        assert_eq!(app.tasks().len(), 1);
    }

    #[tokio::test]
    async fn dispatch_task_seed_activity_stamps_the_hook_column() {
        let (rt, mut app) = test_runtime().await;
        let task = seed(&rt, "Just dispatched", models::TaskStatus::Running).await;
        let at = chrono::Utc::now();

        dispatch_one(
            &rt,
            &mut app,
            Command::Task(TaskCommand::SeedActivity { id: task.id, at }),
        )
        .await;

        let stored = rt.database.get_task(task.id).await.unwrap().unwrap();
        assert!(
            stored.last_pre_tool_use_at.is_some(),
            "the activity timestamp must be seeded so the task is not immediately Stale"
        );
    }

    #[tokio::test]
    async fn dispatch_task_batch_patch_sub_status_updates_every_task() {
        let (rt, mut app) = test_runtime().await;
        let a = seed(&rt, "A", models::TaskStatus::Running).await;
        let b = seed(&rt, "B", models::TaskStatus::Running).await;

        dispatch_one(
            &rt,
            &mut app,
            Command::Task(TaskCommand::BatchPatchSubStatus {
                updates: vec![
                    (a.id, models::SubStatus::Stale),
                    (b.id, models::SubStatus::Crashed),
                ],
            }),
        )
        .await;

        assert_eq!(
            rt.database
                .get_task(a.id)
                .await
                .unwrap()
                .unwrap()
                .sub_status,
            models::SubStatus::Stale
        );
        assert_eq!(
            rt.database
                .get_task(b.id)
                .await
                .unwrap()
                .unwrap()
                .sub_status,
            models::SubStatus::Crashed
        );
    }

    #[tokio::test]
    async fn dispatch_epic_persist_delete_and_toggles_reach_the_db() {
        use crate::tui::commands::EpicCommand;
        let (rt, mut app) = test_runtime().await;
        let epic = rt
            .db_write()
            .create_epic("Toggled", "desc", None)
            .await
            .unwrap();

        dispatch_one(
            &rt,
            &mut app,
            Command::Epic(EpicCommand::Persist {
                id: epic.id,
                status: Some(models::TaskStatus::Review),
                sort_order: Some(3),
            }),
        )
        .await;
        dispatch_one(
            &rt,
            &mut app,
            Command::Epic(EpicCommand::ToggleAutoDispatch {
                id: epic.id,
                auto_dispatch: true,
            }),
        )
        .await;
        dispatch_one(
            &rt,
            &mut app,
            Command::Epic(EpicCommand::ToggleGroupByRepo {
                id: epic.id,
                group_by_repo: true,
            }),
        )
        .await;

        let stored = rt.database.get_epic(epic.id).await.unwrap().unwrap();
        assert_eq!(stored.status, models::TaskStatus::Review);
        assert_eq!(stored.sort_order, Some(3));
        assert!(stored.auto_dispatch);
        assert!(stored.group_by_repo);

        // RefreshFromDb must make the App agree with the DB.
        dispatch_one(&rt, &mut app, Command::Epic(EpicCommand::RefreshFromDb)).await;
        assert!(app.epics().iter().any(|e| e.id == epic.id));

        dispatch_one(&rt, &mut app, Command::Epic(EpicCommand::Delete(epic.id))).await;
        assert!(rt.database.get_epic(epic.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn dispatch_epic_reparent_moves_the_child() {
        use crate::tui::commands::EpicCommand;
        let (rt, mut app) = test_runtime().await;
        let parent = rt.db_write().create_epic("Parent", "", None).await.unwrap();
        let child = rt.db_write().create_epic("Child", "", None).await.unwrap();

        dispatch_one(
            &rt,
            &mut app,
            Command::Epic(EpicCommand::Reparent {
                id: child.id,
                new_parent: Some(parent.id),
            }),
        )
        .await;

        let stored = rt.database.get_epic(child.id).await.unwrap().unwrap();
        assert_eq!(stored.parent_epic_id, Some(parent.id));
    }

    #[tokio::test]
    async fn dispatch_repo_filter_persists_then_deletes_a_preset() {
        use crate::tui::commands::RepoFilterCommand;
        let (rt, mut app) = test_runtime().await;

        dispatch_one(
            &rt,
            &mut app,
            Command::RepoFilter(RepoFilterCommand::PersistFilterPreset {
                name: "mine".into(),
                repo_paths: vec!["/repo".into()],
                mode: RepoFilterMode::Include,
            }),
        )
        .await;
        let presets = rt.database.list_filter_presets().await.unwrap();
        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].0, "mine");

        dispatch_one(
            &rt,
            &mut app,
            Command::RepoFilter(RepoFilterCommand::DeleteFilterPreset("mine".into())),
        )
        .await;
        assert!(rt.database.list_filter_presets().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn dispatch_repo_filter_delete_repo_path_forgets_the_path() {
        use crate::tui::commands::RepoFilterCommand;
        let (rt, mut app) = test_runtime().await;
        dispatch_one(&rt, &mut app, Command::SaveRepoPath("/doomed".into())).await;

        dispatch_one(
            &rt,
            &mut app,
            Command::RepoFilter(RepoFilterCommand::DeleteRepoPath("/doomed".into())),
        )
        .await;

        assert!(
            !rt.database
                .list_repo_paths()
                .await
                .unwrap()
                .contains(&"/doomed".to_string()),
            "the path should no longer be known"
        );
    }

    #[tokio::test]
    async fn dispatch_todo_load_populates_the_todos_view() {
        let (rt, mut app) = test_runtime().await;
        dispatch_one(
            &rt,
            &mut app,
            Command::Todo(TodoCommand::Create {
                title: "Visible".into(),
                linked: None,
                reopen: false,
            }),
        )
        .await;

        dispatch_one(&rt, &mut app, Command::Todo(TodoCommand::Load)).await;

        assert!(
            matches!(app.view_mode(), tui::ViewMode::Todos { todos, .. } if todos.len() == 1),
            "Load must switch the view to Todos with the loaded item"
        );
    }
}

// ---------------------------------------------------------------------------
// run_blocking_dispatch — the three result arms
// ---------------------------------------------------------------------------

/// Receive the next message or fail the test rather than hang forever.
async fn recv_msg(rx: &mut mpsc::UnboundedReceiver<Message>) -> Message {
    tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .expect("a message should arrive well within the timeout")
        .expect("the sender should still be alive")
}

#[tokio::test]
async fn run_blocking_dispatch_sends_dispatched_on_success() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    tasks::run_blocking_dispatch(models::TaskId(7), "Dispatch", true, tx, || {
        Ok(models::DispatchResult {
            worktree_path: "/wt".into(),
            tmux_window: "win".into(),
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
            assert_eq!(tmux_window, "win");
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

// ---------------------------------------------------------------------------
// Local-first repo sync (docs/specs/repo-sync.allium)
// ---------------------------------------------------------------------------

/// The three responses one fetching refresh consumes: symbolic-ref, fetch,
/// rev-list.
fn refresh_responses_fetching(counts: &[u8]) -> Vec<anyhow::Result<std::process::Output>> {
    vec![
        MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/main\n"),
        MockProcessRunner::ok(),
        MockProcessRunner::ok_with_stdout(counts),
    ]
}

async fn expect_measurement(
    rx: &mut mpsc::UnboundedReceiver<Message>,
) -> crate::repo_sync::RepoSyncMeasurement {
    match recv_msg(rx).await {
        Message::RepoSync(crate::tui::messages::RepoSyncMessage::Measured(m)) => m,
        other => panic!("expected a repo-sync measurement, got {other:?}"),
    }
}

// rule-success.RefreshRepoSyncState: the refresh runs off the event loop and
// reports its measurement back as a message.
#[tokio::test]
async fn exec_refresh_repo_sync_reports_the_measurement() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(refresh_responses_fetching(
        b"3\t1\n",
    )));
    let rt = make_runtime(db, tx, mock.clone()).await;

    rt.exec_refresh_repo_sync("/repo".to_string(), true)
        .await
        .unwrap();

    let m = expect_measurement(&mut rx).await;
    assert_eq!(m.repo_path, "/repo");
    assert_eq!(m.base_branch, "main");
    assert_eq!(
        m.counts,
        Some(crate::repo_sync::AheadBehind {
            ahead: 3,
            behind: 1
        })
    );
    assert!(mock
        .recorded_calls()
        .iter()
        .any(|(_, a)| a.contains(&"fetch".to_string())));
}

// Only the fetching refresh points perform a fetch; every other caller rides
// refs some other operation already refreshed.
#[tokio::test]
async fn exec_refresh_repo_sync_without_fetch_touches_no_network() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/main\n"),
        MockProcessRunner::ok_with_stdout(b"0\t2\n"),
    ]));
    let rt = make_runtime(db, tx, mock.clone()).await;

    rt.exec_refresh_repo_sync("/repo".to_string(), false)
        .await
        .unwrap();

    let m = expect_measurement(&mut rx).await;
    assert_eq!(
        m.counts,
        Some(crate::repo_sync::AheadBehind {
            ahead: 0,
            behind: 2
        })
    );
    assert!(
        !mock
            .recorded_calls()
            .iter()
            .any(|(_, a)| a.contains(&"fetch".to_string())),
        "a non-fetching refresh must be a pure local ref read"
    );
}

// rule-success.RefreshRepoSyncStateOnStartup + OneRepoSetForDriftMeasurement:
// one fetching refresh per saved repo path, and no other repository.
#[tokio::test]
async fn exec_refresh_all_repo_sync_fetches_once_per_saved_repo_path() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut responses = refresh_responses_fetching(b"1\t0\n");
    responses.extend(refresh_responses_fetching(b"0\t1\n"));
    let mock = Arc::new(MockProcessRunner::new(responses));
    let rt = make_runtime(db, tx, mock.clone()).await;

    let paths = vec!["/repo-a".to_string(), "/repo-b".to_string()];
    for handle in rt.exec_refresh_all_repo_sync(&paths) {
        handle.await.unwrap();
    }

    let mut seen = vec![
        expect_measurement(&mut rx).await.repo_path,
        expect_measurement(&mut rx).await.repo_path,
    ];
    seen.sort();
    assert_eq!(seen, paths);
    assert_eq!(
        mock.recorded_calls()
            .iter()
            .filter(|(_, a)| a.contains(&"fetch".to_string()))
            .count(),
        2,
        "exactly one fetch per saved repo path"
    );
}

#[tokio::test]
async fn exec_refresh_all_repo_sync_does_nothing_without_saved_paths() {
    let db = test_db().await;
    let (tx, _rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![]));
    let rt = make_runtime(db, tx, mock.clone()).await;

    assert!(rt.exec_refresh_all_repo_sync(&[]).is_empty());
    assert!(mock.recorded_calls().is_empty());
}

// rule-success.SyncRepo, reported back through the success channel.
#[tokio::test]
async fn exec_sync_repo_reports_the_counts_it_moved() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"git@github.com:org/repo.git\n"), // remote
        MockProcessRunner::ok_with_stdout(b"main\n"),                        // branch
        MockProcessRunner::ok_with_stdout(b""),                              // clean
        MockProcessRunner::ok(),                                             // fetch
        MockProcessRunner::ok_with_stdout(b"3\t1\n"),                        // rev-list
        MockProcessRunner::ok(),                                             // merge
        MockProcessRunner::ok_with_stdout(b"4\t0\n"),                        // recount
        MockProcessRunner::ok(),                                             // push
    ]));
    let rt = make_runtime(db, tx, mock).await;

    rt.exec_sync_repo("/repo".to_string(), "main".to_string())
        .await
        .unwrap();

    match recv_msg(&mut rx).await {
        Message::RepoSync(crate::tui::messages::RepoSyncMessage::Succeeded {
            repo_path,
            outcome,
        }) => {
            assert_eq!(repo_path, "/repo");
            assert_eq!(
                outcome,
                crate::repo_sync::SyncOutcome::Synced {
                    pulled: 1,
                    pushed: 4
                }
            );
        }
        other => panic!("expected a sync success, got {other:?}"),
    }
}

// rule-success.ReportRepoSyncFailure: the failure channel carries the detail
// that makes the cause actionable, plus whether retrying is the fix.
#[tokio::test]
async fn exec_sync_repo_reports_a_failure_with_its_detail() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"git@github.com:org/repo.git\n"),
        MockProcessRunner::ok_with_stdout(b"feature\n"), // not on base branch
    ]));
    let rt = make_runtime(db, tx, mock).await;

    rt.exec_sync_repo("/repo".to_string(), "main".to_string())
        .await
        .unwrap();

    match recv_msg(&mut rx).await {
        Message::RepoSync(crate::tui::messages::RepoSyncMessage::Failed {
            repo_path,
            detail,
            retryable,
        }) => {
            assert_eq!(repo_path, "/repo");
            assert!(
                detail.contains("feature") && detail.contains("main"),
                "the branch found and the one expected: {detail}"
            );
            assert!(!retryable, "the operator must checkout main first");
        }
        other => panic!("expected a sync failure, got {other:?}"),
    }
}

// rule-success.RefreshRepoSyncStateAfterRebase: a rebase that moved the repo's
// base branch triggers a non-fetching refresh.
#[tokio::test]
async fn apply_loop_event_branch_rebased_refreshes_the_repo() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/main\n"),
        MockProcessRunner::ok_with_stdout(b"2\t0\n"),
    ]));
    let rt = make_runtime(db, tx, mock.clone()).await;
    let mut app = App::new(vec![]);

    let cmds = apply_loop_event(
        &mut app,
        LoopEvent::Mcp(mcp::McpEvent::BranchRebased {
            repo_path: "/repo".to_string(),
        }),
        &rt,
    );

    assert!(cmds.is_empty(), "the refresh is spawned, not queued");
    let m = expect_measurement(&mut rx).await;
    assert_eq!(m.repo_path, "/repo");
    assert!(
        !mock
            .recorded_calls()
            .iter()
            .any(|(_, a)| a.contains(&"fetch".to_string())),
        "the rebase already refreshed the refs"
    );
}

// rule-success.RefreshRepoSyncStateAfterDispatch: an agent launched off-board
// (the dispatch_task tool, or epic auto-dispatch chaining) refreshes the
// repository's drift too, without a fetch — provisioning already fetched
// origin/<base>.
#[tokio::test]
async fn apply_loop_event_agent_launched_refreshes_the_repo() {
    let db = test_db().await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/main\n"),
        MockProcessRunner::ok_with_stdout(b"1\t0\n"),
    ]));
    let rt = make_runtime(db, tx, mock.clone()).await;
    let mut app = App::new(vec![]);

    let cmds = apply_loop_event(
        &mut app,
        LoopEvent::Mcp(mcp::McpEvent::AgentLaunched {
            repo_path: "/repo".to_string(),
        }),
        &rt,
    );

    assert!(cmds.is_empty(), "the refresh is spawned, not queued");
    let m = expect_measurement(&mut rx).await;
    assert_eq!(m.repo_path, "/repo");
    assert!(
        !mock
            .recorded_calls()
            .iter()
            .any(|(_, a)| a.contains(&"fetch".to_string())),
        "provisioning already fetched origin/<base>"
    );
}

// rule-failure.RefreshRepoSyncStateAfterRebase.1: no repository could be
// resolved from the rebased branch, so nothing is refreshed.
#[tokio::test]
async fn apply_loop_event_branch_rebased_without_a_repo_refreshes_nothing() {
    let db = test_db().await;
    let (tx, _rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![]));
    let rt = make_runtime(db, tx, mock.clone()).await;
    let mut app = App::new(vec![]);

    apply_loop_event(
        &mut app,
        LoopEvent::Mcp(mcp::McpEvent::BranchRebased {
            repo_path: String::new(),
        }),
        &rt,
    );

    assert!(
        mock.recorded_calls().is_empty(),
        "an unresolvable repository must not be measured"
    );
}

/// `SurfaceAutoDispatchFailure` (docs/specs/epics.allium): the chain's failure
/// event reaches the board as a message, so the marker, the status line and the
/// notification are all decided by the app rather than by the loop.
#[tokio::test]
async fn apply_loop_event_auto_dispatch_failed_marks_the_subtask() {
    let (rt, mut app) = test_runtime().await;

    let cmds = apply_loop_event(
        &mut app,
        LoopEvent::Mcp(mcp::McpEvent::AutoDispatchFailed {
            task_id: TaskId(1),
            epic_id: crate::models::EpicId(9),
            reason: "no such repo".to_string(),
        }),
        &rt,
    );

    assert!(
        app.auto_dispatch_failed(TaskId(1)),
        "the failure must reach the board's marker, got commands: {cmds:?}"
    );
    let status = app.status_message().unwrap_or_default();
    assert!(
        status.contains("no such repo"),
        "the reason must reach the status line, got: {status}"
    );
}
