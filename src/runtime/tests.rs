#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;

use crate::db::{CreateLearningRow, CreateTaskRequest, Database};
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

#[tokio::test]
async fn setup_tmux_for_tui_renames_window_and_binds_key() {
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // current_pane_id (display-message)
        MockProcessRunner::ok(), // rename_window
        MockProcessRunner::ok(), // bind_key
    ]);
    setup_tmux_for_tui(&mock);
    let calls = mock.recorded_calls();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0].1, vec!["display-message", "-p", "#{pane_id}"]);
    assert_eq!(calls[1].1, vec!["rename-window", "-t", "", TUI_WINDOW_NAME]);
    assert_eq!(
        calls[2].1,
        vec![
            "bind-key",
            "space",
            &format!("select-window -t {TUI_WINDOW_NAME}")
        ]
    );
}

#[tokio::test]
async fn teardown_tmux_for_tui_unbinds_and_restores_name() {
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // unbind_key
        MockProcessRunner::ok(), // rename_window
    ]);
    teardown_tmux_for_tui(Some("my-shell"), &mock);
    let calls = mock.recorded_calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].1, vec!["unbind-key", "space"]);
    assert_eq!(
        calls[1].1,
        vec!["rename-window", "-t", TUI_WINDOW_NAME, "my-shell"]
    );
}

#[tokio::test]
async fn teardown_tmux_for_tui_skips_rename_when_no_original_name() {
    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // unbind_key
    ]);
    teardown_tmux_for_tui(None, &mock);
    let calls = mock.recorded_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1, vec!["unbind-key", "space"]);
}

async fn make_runtime(
    db: Arc<dyn db::TaskStore>,
    tx: mpsc::UnboundedSender<Message>,
    runner: Arc<dyn ProcessRunner>,
) -> TuiRuntime {
    let (feed_tx, _) = mpsc::unbounded_channel();
    let feed_runner = crate::feed::FeedRunner::new(db.clone(), feed_tx, runner.clone());
    let feed_invalidate_tx = Some(feed_runner.epic_invalidate_tx());
    let todo_db: Arc<dyn crate::db::TodoStore> =
        Arc::new(Database::open_in_memory().await.unwrap());
    TuiRuntime {
        task_svc: Arc::new(crate::service::TaskService::new(db.clone(), runner.clone())),
        epic_svc: Arc::new(crate::service::EpicService::new(db.clone())),
        todo_svc: Arc::new(crate::service::TodoService::new(todo_db)),
        feed_runner: Some(feed_runner),
        feed_invalidate_tx,
        learning_svc: Arc::new(crate::service::MockLearningService),
        feed_db: db.clone(),
        database: db,
        msg_tx: tx,
        runner,
        editor_session: Arc::new(std::sync::Mutex::new(None)),
        emb_svc: crate::service::embeddings::EmbeddingService::new_noop(),
        last_change_count: std::sync::atomic::AtomicI64::new(-1),
    }
}

async fn test_runtime() -> (TuiRuntime, App) {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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

    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        // detect_default_branch (resolved to "main")
        MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/main\n"),
        MockProcessRunner::ok(), // git fetch origin main
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook
        MockProcessRunner::ok(), // tmux send-keys -l
        MockProcessRunner::ok(), // tmux send-keys Enter
    ]));
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let (tx, _rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // for select-window
    ]));
    let rt = make_runtime(db.clone(), tx, mock.clone()).await;
    let tasks = db.list_all().await.unwrap();
    let mut app = App::new(tasks);

    rt.exec_jump_to_tmux(&mut app, "my-window".to_string());

    let calls = mock.recorded_calls();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].1.contains(&"select-window".to_string()));
    assert!(calls[0].1.contains(&"my-window".to_string()));
    assert!(app.error_popup().is_none());
}

#[tokio::test]
async fn exec_dispatch_sends_dispatched_message() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().to_str().unwrap();
    // Create .worktrees/ and fake worktree directory so file writes succeed
    std::fs::create_dir_all(format!("{repo}/.worktrees/1-test-task")).unwrap();

    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        // No detect_default_branch call — task.base_branch is used directly
        MockProcessRunner::ok(), // git fetch origin main
        // git worktree add is skipped (dir pre-created above)
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook (after-split-window)
        MockProcessRunner::ok(), // tmux send-keys -l
        MockProcessRunner::ok(), // tmux send-keys Enter
    ]));
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
    rt.exec_dispatch_agent(task, models::DispatchMode::Dispatch);

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
async fn exec_dispatch_sends_error_on_failure() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    rt.exec_dispatch_agent(task.clone(), models::DispatchMode::Dispatch);

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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
async fn exec_batch_check_windows_sends_window_gone_only_for_absent() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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

#[tokio::test]
async fn exec_cleanup_detaches_when_shared() {
    let (rt, mut app) = test_runtime().await;

    // Create two tasks sharing the same worktree
    rt.exec_insert_task(
        &mut app,
        tui::TaskDraft {
            title: "Task A".into(),
            description: "desc".into(),
            repo_path: "/repo".into(),
            ..Default::default()
        },
        None,
    )
    .await;
    rt.exec_insert_task(
        &mut app,
        tui::TaskDraft {
            title: "Task B".into(),
            description: "desc".into(),
            repo_path: "/repo".into(),
            ..Default::default()
        },
        None,
    )
    .await;

    let id_a = app.tasks()[0].id;
    let id_b = app.tasks()[1].id;

    let worktree = "/repo/.worktrees/1-task-a";
    rt.db_write()
        .patch_task(
            id_a,
            &db::TaskPatch::new()
                .status(models::TaskStatus::Running)
                .worktree(Some(worktree))
                .tmux_window(Some("task-1")),
        )
        .await
        .unwrap();
    rt.db_write()
        .patch_task(
            id_b,
            &db::TaskPatch::new()
                .status(models::TaskStatus::Running)
                .worktree(Some(worktree))
                .tmux_window(Some("task-1")),
        )
        .await
        .unwrap();

    // Cleanup task A — should detach only (worktree is shared)
    rt.exec_cleanup(id_a, "/repo".into(), worktree.into(), Some("task-1".into()))
        .await;

    let task_a = rt.database.get_task(id_a).await.unwrap().unwrap();
    assert!(task_a.worktree.is_none(), "task A should be detached");
    assert!(
        task_a.tmux_window.is_none(),
        "task A tmux should be cleared"
    );

    // Task B should still have the worktree
    let task_b = rt.database.get_task(id_b).await.unwrap().unwrap();
    assert_eq!(task_b.worktree.as_deref(), Some(worktree));
}

#[tokio::test]
async fn send_system_error_sends_error_message() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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

#[tokio::test]
async fn detach_only_clears_worktree_and_tmux_window() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
    let rt = make_runtime(db.clone(), tx, runner).await;
    let mut app = App::new(db.list_all().await.unwrap());

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
    db.patch_task(
        id,
        &db::TaskPatch::new()
            .worktree(Some("/repo/.worktrees/1-t"))
            .tmux_window(Some("win")),
    )
    .await
    .unwrap();

    rt.detach_only(id).await;

    let task = db.get_task(id).await.unwrap().unwrap();
    assert!(task.worktree.is_none(), "worktree should be cleared");
    assert!(task.tmux_window.is_none(), "tmux_window should be cleared");
    // No error message should have been sent
    assert!(
        rx.try_recv().is_err(),
        "no error message expected on success"
    );
}

#[tokio::test]
async fn exec_finish_sends_complete_when_shared_worktree() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
    let rt = make_runtime(db.clone(), tx, runner).await;
    let mut app = App::new(db.list_all().await.unwrap());

    // Create two tasks sharing the same worktree
    for title in ["Task A", "Task B"] {
        rt.exec_insert_task(
            &mut app,
            tui::TaskDraft {
                title: title.into(),
                description: "desc".into(),
                repo_path: "/repo".into(),
                ..Default::default()
            },
            None,
        )
        .await;
    }
    let id_a = app.tasks()[0].id;
    let id_b = app.tasks()[1].id;
    let worktree = "/repo/.worktrees/1-task-a";
    for id in [id_a, id_b] {
        db.patch_task(
            id,
            &db::TaskPatch::new()
                .status(models::TaskStatus::Running)
                .worktree(Some(worktree))
                .tmux_window(Some("task-1")),
        )
        .await
        .unwrap();
    }

    rt.exec_finish(
        id_a,
        "/repo".into(),
        "1-task-a".into(),
        "main".into(),
        worktree.into(),
        Some("task-1".into()),
    )
    .await;

    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(msg, Message::Task(crate::tui::messages::TaskMessage::FinishComplete(tid)) if tid == id_a),
        "Expected FinishComplete for id_a when worktree is shared, got: {msg:?}"
    );
    // Task A detached, task B still has the worktree
    let task_a = db.get_task(id_a).await.unwrap().unwrap();
    assert!(task_a.worktree.is_none());
}

#[tokio::test]
async fn exec_finish_happy_path_sends_complete() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"main\n"), // rev-parse HEAD
        MockProcessRunner::fail(""),                  // remote get-url (no remote)
        MockProcessRunner::ok(),                      // git rebase main (from worktree)
        MockProcessRunner::ok(),                      // git merge --ff-only (fast-forward)
                                                      // Worktree is preserved; cleanup happens later during archive.
    ]));
    let rt = make_runtime(db.clone(), tx, mock).await;

    let task = create_task_returning(
        &*db,
        "Test",
        "desc",
        "/repo",
        None,
        models::TaskStatus::Done,
    )
    .await
    .unwrap();
    let id = task.id;

    rt.exec_finish(
        id,
        "/repo".into(),
        "1-test".into(),
        "main".into(),
        "/repo/.worktrees/1-test".into(),
        None,
    )
    .await;

    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(msg, Message::Task(crate::tui::messages::TaskMessage::FinishComplete(tid)) if tid == id),
        "Expected FinishComplete, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_finish_conflict_sends_failed() {
    use crate::process::exit_fail;
    use std::process::Output;

    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"main\n"), // rev-parse HEAD
        MockProcessRunner::fail(""),                  // remote get-url (no remote)
        Ok(Output {
            status: exit_fail(),
            stdout: b"".to_vec(),
            stderr:
                b"CONFLICT (content): Merge conflict in file.rs\nerror: could not apply abc1234\n"
                    .to_vec(),
        }),
        MockProcessRunner::ok(), // git rebase --abort
    ]));
    let rt = make_runtime(db.clone(), tx, mock).await;

    let task = create_task_returning(
        &*db,
        "Test",
        "desc",
        "/repo",
        None,
        models::TaskStatus::Done,
    )
    .await
    .unwrap();
    let id = task.id;

    rt.exec_finish(
        id,
        "/repo".into(),
        "1-test".into(),
        "main".into(),
        "/repo/.worktrees/1-test".into(),
        None,
    )
    .await;

    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    let Message::Task(crate::tui::messages::TaskMessage::FinishFailed {
        id: tid,
        is_conflict,
        ..
    }) = msg
    else {
        panic!("Expected FinishFailed, got: {msg:?}");
    };
    assert_eq!(tid, id);
    assert!(is_conflict, "Expected is_conflict=true");
}

#[tokio::test]
async fn exec_finish_not_on_main_sends_failed() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"feature-branch\n"), // rev-parse HEAD (not main)
    ]));
    let rt = make_runtime(db.clone(), tx, mock).await;

    let task = create_task_returning(
        &*db,
        "Test",
        "desc",
        "/repo",
        None,
        models::TaskStatus::Done,
    )
    .await
    .unwrap();
    let id = task.id;

    rt.exec_finish(
        id,
        "/repo".into(),
        "1-test".into(),
        "main".into(),
        "/repo/.worktrees/1-test".into(),
        None,
    )
    .await;

    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .unwrap()
        .unwrap();
    let Message::Task(crate::tui::messages::TaskMessage::FinishFailed {
        id: tid,
        is_conflict,
        ..
    }) = msg
    else {
        panic!("Expected FinishFailed, got: {msg:?}");
    };
    assert_eq!(tid, id);
    assert!(!is_conflict, "Expected is_conflict=false for not-on-main");
}

#[tokio::test]
async fn exec_send_notification_calls_notify_send() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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

    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        // detect_default_branch (resolved to "main")
        MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/main\n"),
        // provision_worktree: fetch runs, then dir exists so git worktree add is skipped
        MockProcessRunner::ok(), // git fetch origin main
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook (after-split-window)
        MockProcessRunner::ok(), // tmux send-keys -l (claude command)
        MockProcessRunner::ok(), // tmux send-keys Enter
    ]));
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

    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let (tx, _rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        // detect_default_branch resolves to master
        MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/master\n"),
        MockProcessRunner::ok(), // git fetch origin master
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook
        MockProcessRunner::ok(), // tmux send-keys -l
        MockProcessRunner::ok(), // tmux send-keys Enter
    ]));
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

    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let epic = db.create_epic("My Epic", "epic desc", None).await.unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        // detect_default_branch (resolved to "main")
        MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/main\n"),
        MockProcessRunner::ok(), // git fetch origin main
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook
        MockProcessRunner::ok(), // tmux send-keys -l (claude command)
        MockProcessRunner::ok(), // tmux send-keys Enter
    ]));
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook (after-split-window)
        MockProcessRunner::ok(), // tmux send-keys -l (claude --continue)
        MockProcessRunner::ok(), // tmux send-keys Enter
    ]));
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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

#[tokio::test]
async fn exec_persist_managed_feed_config_writes_all_settings() {
    let (rt, mut app) = test_runtime().await;
    rt.exec_persist_managed_feed_config(
        &mut app,
        Some("reviews.sh".to_string()),
        Some(300),
        None,
        None,
    )
    .await;

    assert_eq!(
        rt.database
            .get_reviews_feed_command()
            .await
            .unwrap()
            .as_deref(),
        Some("reviews.sh")
    );
    assert_eq!(
        rt.database.get_reviews_feed_interval_secs().await.unwrap(),
        Some(300)
    );
    assert_eq!(rt.database.get_cve_feed_command().await.unwrap(), None);
    assert_eq!(
        rt.database.get_cve_feed_interval_secs().await.unwrap(),
        None
    );
}

#[tokio::test]
async fn exec_provision_and_refresh_provisions_and_syncs_to_app() {
    let (rt, mut app) = test_runtime().await;
    // Enable the reviews feed out of band, then re-provision live.
    rt.database
        .set_reviews_feed_command(Some("reviews.sh"))
        .await
        .unwrap();
    assert!(app.epics().is_empty());

    rt.exec_provision_and_refresh(&mut app).await;

    // reviews_parent + my_reviews + team_reviews + bots = 4 epics, synced to app.
    assert_eq!(
        app.epics().len(),
        4,
        "reviews subtree provisioned and refreshed"
    );
    assert!(app
        .epics()
        .iter()
        .any(|e| e.feed_role == crate::models::FeedRole::ReviewsParent));
    assert!(app.error_popup().is_none());
}

#[tokio::test]
async fn exec_provision_and_refresh_invalidates_feed_runner_cache() {
    // Regression for the TUI [C] save gap: a freshly-enabled feed on a
    // previously feed-less instance must become pollable after the save, not
    // stay stranded behind the FeedRunner's `any_feed_cmds == Some(false)`
    // short-circuit until an unrelated EpicChanged/Refresh or a restart.
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

    // Enable the reviews feed and provision via the TUI [C] save path.
    rt.database
        .set_reviews_feed_command(Some("reviews.sh"))
        .await
        .unwrap();
    rt.exec_provision_and_refresh(&mut app).await;
    assert!(app.error_popup().is_none());

    // The save must have invalidated the cache so the next tick re-queries and
    // discovers the freshly-provisioned reviews_parent feed command.
    feed_runner.tick().await;
    assert_eq!(
        feed_runner.any_feed_cmds_cache(),
        Some(true),
        "save must invalidate the cache so the freshly-enabled feed becomes pollable"
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"%1\n"), // current_pane_id
        MockProcessRunner::ok_with_stdout(b"%3\n"), // join_pane: display-message for source pane ID
        MockProcessRunner::ok(),                    // join_pane: join-pane command
    ]));
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
        "Expected PaneOpened with task 1, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_exit_split_mode_with_restore_breaks_pane() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // pane_exists → display-message succeeds
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::fail("no pane"), // pane_exists → display-message fails
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"%5\n"), // pane_id_for_window (new task)
        MockProcessRunner::ok(),                    // swap-pane
        MockProcessRunner::ok(),                    // kill-window (old pane had no task)
    ]));
    let rt = make_runtime(db.clone(), tx, mock.clone()).await;

    rt.exec_swap_split_pane(TaskId(1), "task-1", Some("%2"), None)
        .await
        .unwrap();
    let calls = mock.recorded_calls();
    // 1st call: display-message to get new pane ID
    assert!(calls[0].1.contains(&"display-message".to_string()));
    // 2nd call: swap-pane
    assert!(calls[1].1.contains(&"swap-pane".to_string()));
    // 3rd call: kill-window (no old task to rename)
    assert!(calls[2].1.contains(&"kill-window".to_string()));
    // No 4th call — focus must NOT be transferred
    assert_eq!(calls.len(), 3, "select-pane must not be called after swap");
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"%5\n"), // pane_id_for_window (new task)
        MockProcessRunner::ok(),                    // swap-pane
        MockProcessRunner::ok(),                    // rename-window (old task had a window)
    ]));
    let rt = make_runtime(db.clone(), tx, mock.clone()).await;

    rt.exec_swap_split_pane(TaskId(1), "task-new", Some("%2"), Some("task-old"))
        .await
        .unwrap();
    let calls = mock.recorded_calls();
    // 3rd call should be rename-window, not kill-window
    assert!(calls[2].1.contains(&"rename-window".to_string()));
    // Verify the rename target and new name
    assert!(calls[2].1.contains(&"task-new".to_string()));
    assert!(calls[2].1.contains(&"task-old".to_string()));
    // No 4th call — focus must NOT be transferred
    assert_eq!(calls.len(), 3, "select-pane must not be called after swap");
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

// -----------------------------------------------------------------------
// Event-loop: split-mode functions send results via msg_tx (not app.update)
// -----------------------------------------------------------------------

#[tokio::test]
async fn exec_enter_split_mode_sends_pane_opened_via_msg_tx() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"%1\n"), // current_pane_id
        MockProcessRunner::ok_with_stdout(b"%3\n"), // join_pane: display-message for source pane ID
        MockProcessRunner::ok(),                    // join_pane: join-pane command
    ]));
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"%5\n"), // pane_id_for_window (new task)
        MockProcessRunner::ok(),                    // swap-pane
        MockProcessRunner::ok(),                    // kill-window (old pane had no task)
    ]));
    let rt = make_runtime(db.clone(), tx, mock.clone()).await;

    rt.exec_swap_split_pane(TaskId(1), "task-1", Some("%2"), None)
        .await
        .unwrap();

    let calls = mock.recorded_calls();
    assert!(calls[1].1.contains(&"swap-pane".to_string()));
    assert!(calls[2].1.contains(&"kill-window".to_string()));
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let (tx, _rx) = mpsc::unbounded_channel();
    let mock = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // tmux kill-window
    ]));
    let rt = make_runtime(db, tx, mock.clone()).await;

    rt.exec_kill_tmux_window("task-1".into()).await.unwrap();
    let calls = mock.recorded_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "tmux");
    assert!(calls[0].1.contains(&"kill-window".to_string()));
    assert!(calls[0].1.contains(&"task-1".to_string()));
}

#[tokio::test]
async fn exec_kill_tmux_window_failure_is_best_effort() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
// exec_trigger_epic_feed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn exec_trigger_epic_feed_success() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let epic = db
        .create_epic("Security Vulnerabilities", "", None)
        .await
        .unwrap();

    let (tx, mut rx) = mpsc::unbounded_channel();
    let rt = make_runtime(db, tx, Arc::new(MockProcessRunner::new(vec![]))).await;

    let cmd = r#"echo '[{"external_id":"vuln:1","title":"CVE-1","description":"desc","status":"backlog","tag":"fix"}]'"#;
    rt.exec_trigger_epic_feed(
        epic.id,
        "Security Vulnerabilities".to_string(),
        cmd.to_string(),
        false,
    );

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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let epic = db.create_epic("Empty Feed", "", None).await.unwrap();

    let (tx, mut rx) = mpsc::unbounded_channel();
    let rt = make_runtime(db, tx, Arc::new(MockProcessRunner::new(vec![]))).await;

    rt.exec_trigger_epic_feed(
        epic.id,
        "Empty Feed".to_string(),
        "echo '[]'".to_string(),
        false,
    );

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

#[tokio::test]
async fn exec_trigger_epic_feed_command_fails() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let epic = db.create_epic("Failing Feed", "", None).await.unwrap();

    let (tx, mut rx) = mpsc::unbounded_channel();
    let rt = make_runtime(db, tx, Arc::new(MockProcessRunner::new(vec![]))).await;

    rt.exec_trigger_epic_feed(
        epic.id,
        "Failing Feed".to_string(),
        "exit 1".to_string(),
        false,
    );

    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .expect("timed out")
        .expect("channel closed");
    assert!(
        matches!(
            msg,
            Message::Feed(crate::tui::messages::FeedMessage::Failed { .. })
        ),
        "non-zero exit should produce FeedFailed, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_trigger_epic_feed_malformed_json() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let epic = db.create_epic("Bad JSON Feed", "", None).await.unwrap();

    let (tx, mut rx) = mpsc::unbounded_channel();
    let rt = make_runtime(db, tx, Arc::new(MockProcessRunner::new(vec![]))).await;

    rt.exec_trigger_epic_feed(
        epic.id,
        "Bad JSON Feed".to_string(),
        "echo 'not-json'".to_string(),
        false,
    );

    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .expect("timed out")
        .expect("channel closed");
    assert!(
        matches!(
            msg,
            Message::Feed(crate::tui::messages::FeedMessage::Failed { .. })
        ),
        "malformed JSON should produce FeedFailed, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_trigger_epic_feed_grouped_puts_tasks_in_sub_epics() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let epic = db.create_epic("Reviews", "", None).await.unwrap();

    let (tx, mut rx) = mpsc::unbounded_channel();
    let rt = make_runtime(db.clone(), tx, Arc::new(MockProcessRunner::new(vec![]))).await;

    let cmd = r#"echo '[{"external_id":"pr-1","title":"PR 1","description":"","url":"https://github.com/org/repo-a/pull/1","status":"backlog","tag":"pr-review"}]'"#;
    rt.exec_trigger_epic_feed(epic.id, "Reviews".to_string(), cmd.to_string(), true);

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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let epic = db.create_epic("Reviews", "", None).await.unwrap();
    db.patch_epic(
        epic.id,
        &db::EpicPatch::new().feed_role(crate::models::FeedRole::ReviewsParent),
    )
    .await
    .unwrap();

    let (tx, mut rx) = mpsc::unbounded_channel();
    let rt = make_runtime(db.clone(), tx, Arc::new(MockProcessRunner::new(vec![]))).await;

    // A single direct-request PR: route(signals) => my_reviews.
    let cmd = r#"echo '[{"external_id":"pr-1","title":"PR 1","description":"","url":"https://github.com/org/repo/pull/1","status":"backlog","tag":"pr-review","signals":["direct-request"]}]'"#;
    // group_by_repo=false; dispatch must key on feed_role, not this flag.
    rt.exec_trigger_epic_feed(epic.id, "Reviews".to_string(), cmd.to_string(), false);

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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
// exec_save_tips_state
// ---------------------------------------------------------------------------

#[tokio::test]
async fn exec_save_tips_state_persists_to_db() {
    let (rt, _app) = test_runtime().await;

    rt.exec_save_tips_state(7, models::TipsShowMode::NewOnly)
        .await;

    let (seen_up_to, show_mode) = rt.database.get_tips_state().await.unwrap();
    assert_eq!(seen_up_to, 7);
    assert_eq!(show_mode, models::TipsShowMode::NewOnly);
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
