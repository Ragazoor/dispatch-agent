#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;

// `db` is the concrete `Arc<Database>` in this fixture (see `test_db`), so the
// store traits must be in scope for their methods to resolve on it.
use crate::db::{
    CreateLearningRow, CreateTaskRequest, Database, EpicCrud, EpicRead, TaskCrud, TaskPatch,
};
use crate::dispatch::mock_sequence::DispatchScript;
use crate::process::MockProcessRunner;
use crate::tui::commands::SettingsCommand;

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
        // Absent by default, so `is_trusted_at` reads "not trusted" and
        // `trust_at` fails to write (no such directory) rather than falling
        // through to the developer's real `$HOME/.claude.json`. A test that
        // needs the "trusted" branch overrides this with a real tempfile.
        claude_json_path: std::path::PathBuf::from("/nonexistent-test-path/.claude.json"),
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
            phoenix: false,
        })
        .await?;
    db.get_task(id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Task {id} vanished after insert"))
}

/// An empty `App` fixture — a board with no tasks. Distinct from
/// `crate::tui::tests::helpers::make_app`, which seeds four fixed tasks; the two
/// are not interchangeable, hence the different name rather than a shared import.
fn empty_app() -> App {
    App::new(vec![])
}

/// Receive the next message or fail the test rather than hang forever.
async fn recv_msg(rx: &mut mpsc::UnboundedReceiver<Message>) -> Message {
    tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .expect("a message should arrive well within the timeout")
        .expect("the sender should still be alive")
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
    rt.exec_persist_task(
        &mut app,
        crate::tui::commands::PersistFields::from_task(&task),
    )
    .await;
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
    rt.exec_persist_task(
        &mut app,
        crate::tui::commands::PersistFields::from_task(&task),
    )
    .await;

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
    rt.exec_persist_task(
        &mut app,
        crate::tui::commands::PersistFields::from_task(&stale),
    )
    .await;

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
        Box::new(task.clone()),
    )));

    rt.exec_persist_task(
        &mut app,
        crate::tui::commands::PersistFields::from_task(&task),
    )
    .await;

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
        Box::new(task.clone()),
    )));

    rt.exec_persist_task(
        &mut app,
        crate::tui::commands::PersistFields::from_task(&task),
    )
    .await;

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
        Box::new(board_task.clone()),
    )));
    let mut stale = board_task;
    stale.last_pre_tool_use_at = None;

    rt.exec_persist_task(
        &mut app,
        crate::tui::commands::PersistFields::from_task(&stale),
    )
    .await;

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
    rt.exec_persist_task(
        &mut app,
        crate::tui::commands::PersistFields::from_task(&done),
    )
    .await;

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

mod command_dispatch;
mod event_loop;
mod feeds;
mod main_session;
mod misc;
mod refresh;
mod split_mode;
mod task_exec;
