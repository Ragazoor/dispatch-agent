#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;

use crate::mcp::handlers::tasks::WrapUpAction;

/// An epic that does not resolve stops the chain silently. This is the
/// warn-and-skip branch of `AutoDispatchNextSubtask`: a chain problem must never
/// surface as an error, because by the time it runs the session has already
/// closed. `tasks.epic_id` carries a foreign key, so this branch is only
/// reachable by calling the chain directly — not by wiring a task to a missing
/// epic and closing it.
#[tokio::test]
async fn auto_dispatch_next_returns_none_for_missing_epic() {
    let state = test_state().await;
    assert!(crate::mcp::handlers::tasks::dispatch::auto_dispatch_next(
        &state,
        crate::models::EpicId(9999)
    )
    .await
    .is_none());
}

async fn wait_for_task_changed(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::mcp::McpEvent>,
    expected_id: crate::models::TaskId,
) {
    loop {
        match rx.recv().await {
            Some(crate::mcp::McpEvent::TaskChanged(id)) if id == expected_id => break,
            Some(_) => continue,
            None => panic!("notification channel closed before dispatch completed"),
        }
    }
}

// -- claim_task tests -------------------------------------------------------

#[tokio::test]
async fn claim_task_success() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Claimable",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "claim_task",
            "arguments": {
                "task_id": task_id.0,
                "worktree": "/repo/.worktrees/5-other-task",
                "tmux_window": "task-5"
            }
        })),
    )
    .await;
    assert!(
        resp.error.is_none(),
        "claim should succeed: {:?}",
        resp.error
    );

    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(
        task.worktree.as_deref(),
        Some("/repo/.worktrees/5-other-task")
    );
    assert_eq!(task.tmux_window.as_deref(), Some("task-5"));
}

#[tokio::test]
async fn claim_task_rejects_running_task() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Running",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "claim_task",
            "arguments": {
                "task_id": task_id.0,
                "worktree": "/repo/.worktrees/5-other",
                "tmux_window": "task-5"
            }
        })),
    )
    .await;
    assert!(is_error(&resp));
    assert!(error_message(&resp).contains("already"));
}

#[tokio::test]
async fn claim_task_rejects_different_repo() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Other Repo",
            description: "desc",
            repo_path: "/other-repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "claim_task",
            "arguments": {
                "task_id": task_id.0,
                "worktree": "/repo/.worktrees/5-other-task",
                "tmux_window": "task-5"
            }
        })),
    )
    .await;
    assert!(is_error(&resp));
    assert!(error_message(&resp).contains("repo"));
}

#[tokio::test]
async fn claim_task_not_found() {
    let state = test_state().await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "claim_task",
            "arguments": {
                "task_id": 9999,
                "worktree": "/repo/.worktrees/5-other",
                "tmux_window": "task-5"
            }
        })),
    )
    .await;
    assert!(is_error(&resp));
    assert!(error_message(&resp).contains("not found"));
}

// -- claim_task tests -------------------------------------------------------

#[tokio::test]
async fn claim_task_accepts_string_task_id() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Claimable",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "claim_task",
            "arguments": {
                "task_id": task_id.0.to_string(),
                "worktree": "/repo/.worktrees/5-other-task",
                "tmux_window": "task-5"
            }
        })),
    )
    .await;
    assert!(
        resp.error.is_none(),
        "should accept string task_id: {:?}",
        resp.error
    );
}

#[tokio::test]
async fn claim_task_rejects_done_task() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Done",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Done,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "claim_task",
            "arguments": {
                "task_id": task_id.0,
                "worktree": "/repo/.worktrees/5-other",
                "tmux_window": "task-5"
            }
        })),
    )
    .await;
    assert_error(&resp, "already");
}

#[tokio::test]
async fn claim_task_rejects_review_task() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Review",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Review,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "claim_task",
            "arguments": {
                "task_id": task_id.0,
                "worktree": "/repo/.worktrees/5-other",
                "tmux_window": "task-5"
            }
        })),
    )
    .await;
    assert_error(&resp, "already");
}

#[tokio::test]
async fn claim_task_worktree_without_worktrees_dir() {
    let state = test_state().await;
    // Task repo is "/repo", worktree path has no /.worktrees/ segment
    // so the full path is used as the repo — should match when equal
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Direct",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "claim_task",
            "arguments": {
                "task_id": task_id.0,
                "worktree": "/repo",
                "tmux_window": "task-5"
            }
        })),
    )
    .await;
    assert!(
        resp.error.is_none(),
        "should match when worktree equals repo: {:?}",
        resp.error
    );
}
#[tokio::test]
async fn claim_task_updates_status_to_running() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Claim",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "claim_task",
            "arguments": {
                "task_id": task_id.0,
                "worktree": "/repo/.worktrees/1-claim",
                "tmux_window": "task-1"
            }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "{:?}", resp.error);

    let task = state
        .db
        .get_task(crate::models::TaskId(task_id.0))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.worktree.as_deref(), Some("/repo/.worktrees/1-claim"));
    assert_eq!(task.tmux_window.as_deref(), Some("task-1"));
}

// ---------------------------------------------------------------------------
// send_message tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn send_message_writes_file_and_sends_keys() {
    let tmp = tempfile::tempdir().unwrap();
    let worktree_path = tmp.path().to_str().unwrap().to_string();

    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // tmux send-keys -l (notification text)
        MockProcessRunner::ok(), // tmux send-keys Enter
    ]));
    let state = Arc::new(McpState::new(
        McpDeps {
            db: db.clone(),
            runner,
            embedding_service: EmbeddingService::new_test(),
            data_dir: std::env::temp_dir(),
        },
        None,
    ));

    // Create sender and receiver tasks
    let sender_id = db
        .create_task(CreateTaskRequest {
            title: "Fix auth bug",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let receiver_id = db
        .create_task(CreateTaskRequest {
            title: "Review PR",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    db.patch_task(
        receiver_id,
        &db::TaskPatch::new()
            .worktree(Some(&worktree_path))
            .tmux_window(Some("task-2")),
    )
    .await
    .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "send_message",
            "arguments": {
                "from_task_id": sender_id.0,
                "to_task_id": receiver_id.0,
                "body": "Can you review path/to/file.rs?"
            }
        })),
    )
    .await;

    let text = extract_response_text(&resp);
    assert!(
        text.contains("Message sent to task"),
        "Expected success message, got: {text}"
    );

    // Verify message file was written in .claude-messages/ directory
    let messages_dir = tmp.path().join(".claude-messages");
    assert!(
        messages_dir.is_dir(),
        ".claude-messages directory should exist"
    );
    let entries: Vec<_> = std::fs::read_dir(&messages_dir).unwrap().collect();
    assert_eq!(entries.len(), 1, "Should have exactly one message file");
    let message_path = entries[0].as_ref().unwrap().path();
    let file_name = message_path.file_name().unwrap().to_str().unwrap();
    assert!(
        file_name.starts_with(&format!("{}-", sender_id.0)),
        "Filename should start with sender task id"
    );
    assert!(file_name.ends_with(".md"), "Filename should end with .md");
    let content = std::fs::read_to_string(&message_path).unwrap();
    assert!(
        content.contains("Fix auth bug"),
        "Message should contain sender title"
    );
    assert!(
        content.contains("Can you review path/to/file.rs?"),
        "Message should contain body"
    );
}

#[tokio::test]
async fn send_message_target_not_found() {
    let state = test_state().await;

    let sender_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Sender",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "send_message",
            "arguments": {
                "from_task_id": sender_id.0,
                "to_task_id": 9999,
                "body": "hello"
            }
        })),
    )
    .await;

    assert!(is_error(&resp), "Should return error for missing target");
    let msg = error_message(&resp);
    assert!(
        msg.contains("not found"),
        "Error should mention not found: {msg}"
    );
}

#[tokio::test]
async fn send_message_target_no_worktree() {
    let state = test_state().await;

    let sender_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Sender",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let receiver_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Receiver",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "send_message",
            "arguments": {
                "from_task_id": sender_id.0,
                "to_task_id": receiver_id.0,
                "body": "hello"
            }
        })),
    )
    .await;

    assert!(
        is_error(&resp),
        "Should return error for target without worktree"
    );
    let msg = error_message(&resp);
    assert!(
        msg.contains("no worktree"),
        "Error should mention no worktree: {msg}"
    );
}

#[tokio::test]
async fn send_message_target_no_tmux_window() {
    let state = test_state().await;

    let sender_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Sender",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let receiver_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Receiver",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    state
        .db_write()
        .patch_task(
            receiver_id,
            &db::TaskPatch::new().worktree(Some("/some/worktree")),
        )
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "send_message",
            "arguments": {
                "from_task_id": sender_id.0,
                "to_task_id": receiver_id.0,
                "body": "hello"
            }
        })),
    )
    .await;

    assert!(
        is_error(&resp),
        "Should return error for target without tmux window"
    );
    let msg = error_message(&resp);
    assert!(
        msg.contains("no tmux window"),
        "Error should mention no tmux window: {msg}"
    );
}
// -- automatic epic chaining on exit_session ---------------------------------
//
// Epic chaining is a server-side side effect of closing a session, not a tool
// an agent calls: `AutoDispatchNextSubtask` in docs/specs/epics.allium is keyed
// on the `SessionClosed(task)` event that `ExitSessionViaMcp` emits last. The
// old `dispatch_next` MCP tool is gone, so every test below drives the real
// close path instead.

/// Wiring shared by the chaining tests: a temp repo, an in-memory DB, an
/// `McpState` with a notification channel, and a runner with a generous queue
/// of successes covering the closing task's detached `tmux kill-window` plus a
/// full worktree provisioning for the chained subtask.
struct ChainFixture {
    _dir: tempfile::TempDir,
    repo_path: String,
    db: Arc<dyn db::TaskStore>,
    state: Arc<McpState>,
    notify_rx: tokio::sync::mpsc::UnboundedReceiver<crate::mcp::McpEvent>,
    /// The same runner the state holds, kept concrete so a test can inspect
    /// which commands the close path actually issued.
    runner: Arc<MockProcessRunner>,
}

impl ChainFixture {
    async fn new() -> Self {
        Self::build(None).await
    }

    /// Like [`ChainFixture::new`], but with a caller-supplied runner script, for
    /// tests that assert on the exact command sequence a dispatch issues (or
    /// need one to fail).
    async fn with_runner(runner: Arc<MockProcessRunner>) -> Self {
        Self::build_with(runner, None).await
    }

    /// Like [`ChainFixture::new`], but with a `task_svc` whose `update_task`
    /// always fails, so `exit_session`'s terminal close patch cannot land. This
    /// is the `close_persisted = false` branch of `ExitSession` /
    /// `ExitSessionViaMcp`.
    async fn with_failing_close() -> Self {
        Self::build(Some(Arc::new(FailingCloseTaskService))).await
    }

    async fn build(task_svc_override: Option<Arc<dyn crate::service::TaskServiceApi>>) -> Self {
        // The kill-window teardown and the chained dispatch race by design, and
        // MockProcessRunner pops a shared FIFO, so queue uniform successes
        // rather than a command-ordered script.
        let runner = Arc::new(MockProcessRunner::new(
            (0..24).map(|_| MockProcessRunner::ok()).collect(),
        ));
        Self::build_with(runner, task_svc_override).await
    }

    async fn build_with(
        runner: Arc<MockProcessRunner>,
        task_svc_override: Option<Arc<dyn crate::service::TaskServiceApi>>,
    ) -> Self {
        let dir = tempfile::TempDir::new().unwrap();
        let repo_path = dir.path().to_str().unwrap().to_string();
        std::fs::create_dir_all(dir.path().join(".worktrees")).unwrap();

        let (notify_tx, notify_rx) = tokio::sync::mpsc::unbounded_channel::<crate::mcp::McpEvent>();
        let (state, db) = test_state_with_overrides(
            runner.clone() as Arc<dyn ProcessRunner>,
            Some(notify_tx),
            task_svc_override,
        )
        .await;
        Self {
            _dir: dir,
            repo_path,
            db,
            state,
            notify_rx,
            runner,
        }
    }

    /// Every `tmux kill-window` the close path issued, in order.
    fn kill_window_calls(&self) -> Vec<(String, Vec<String>)> {
        self.runner
            .recorded_calls()
            .into_iter()
            .filter(|(program, args)| {
                program == "tmux" && args.first().is_some_and(|a| a == "kill-window")
            })
            .collect()
    }

    async fn epic(&self, auto_dispatch: bool) -> crate::models::EpicId {
        let epic = self
            .db
            .create_epic("Chained Epic", "desc", None)
            .await
            .unwrap();
        self.db
            .patch_epic(epic.id, &db::EpicPatch::new().auto_dispatch(auto_dispatch))
            .await
            .unwrap();
        epic.id
    }

    /// A backlog subtask that a chain can pick up. `repo_path` defaults to the
    /// fixture's temp repo; pass an override to make provisioning fail.
    async fn backlog_subtask(
        &self,
        epic_id: Option<crate::models::EpicId>,
        title: &str,
        sort_order: Option<i64>,
        repo_path: Option<&str>,
    ) -> crate::models::TaskId {
        let id = self
            .db
            .create_task(CreateTaskRequest {
                title,
                description: "",
                repo_path: repo_path.unwrap_or(&self.repo_path),
                plan: Some("docs/plan.md"),
                status: TaskStatus::Backlog,
                base_branch: "main",
                epic_id,
                sort_order,
                tag: None,
                wrap_up_mode: None,
                auto_run_plan: false,
            })
            .await
            .unwrap();
        // Mocked git never creates the worktree directory; pre-create it so
        // provisioning takes the "reuse existing worktree" branch.
        std::fs::create_dir_all(
            std::path::Path::new(&self.repo_path)
                .join(".worktrees")
                .join(format!("{}-{}", id.0, crate::models::slugify(title))),
        )
        .unwrap();
        id
    }

    /// The task whose session is about to close: Running with a worktree and a
    /// tmux window, which is what `is_wrappable` and `exit_session` demand.
    /// Only the epic membership and the fixture's real temp repo distinguish it
    /// from the shared helper, so it supplies those and delegates.
    async fn closing_subtask(
        &self,
        epic_id: Option<crate::models::EpicId>,
    ) -> crate::models::TaskId {
        create_running_task_with_window_in(&self.state, &self.repo_path, epic_id).await
    }

    /// Close `task_id`'s session with `action`. Thin wrapper over the shared
    /// [`close_session_via_mcp`] so the exit-token shape lives in one place.
    async fn close(
        &self,
        task_id: crate::models::TaskId,
        action: crate::mcp::handlers::tasks::WrapUpAction,
    ) -> JsonRpcResponse {
        close_session_via_mcp(&self.state, task_id, action).await
    }
}

/// Migrated from `dispatch_next_picks_first_backlog_subtask`: closing a subtask
/// dispatches the epic's first backlog subtask and leaves the rest alone.
#[tokio::test]
async fn exit_session_dispatches_first_backlog_subtask() {
    let mut fx = ChainFixture::new().await;
    let epic_id = fx.epic(true).await;
    let closing = fx.closing_subtask(Some(epic_id)).await;
    let first = fx
        .backlog_subtask(Some(epic_id), "Task 1", Some(10), None)
        .await;
    let second = fx
        .backlog_subtask(Some(epic_id), "Task 2", Some(20), None)
        .await;

    let resp = fx.close(closing, WrapUpAction::Done).await;
    assert!(resp.error.is_none(), "close must succeed: {:?}", resp.error);

    wait_for_task_changed(&mut fx.notify_rx, first).await;

    let dispatched = fx.db.get_task(first).await.unwrap().unwrap();
    assert_eq!(dispatched.status, TaskStatus::Running);
    assert!(dispatched.worktree.is_some());
    assert!(dispatched.tmux_window.is_some());
    assert!(
        dispatched.last_pre_tool_use_at.is_some(),
        "last_pre_tool_use_at should be seeded so the tick classifier does not flicker the task to Stale"
    );

    let untouched = fx.db.get_task(second).await.unwrap().unwrap();
    assert_eq!(
        untouched.status,
        TaskStatus::Backlog,
        "only one subtask may be chained per closed session"
    );
    assert!(untouched.worktree.is_none());
}

/// The chained subtask is reported in the response text so the closing agent
/// can see what it handed off to.
#[tokio::test]
async fn exit_session_response_names_the_chained_subtask() {
    let mut fx = ChainFixture::new().await;
    let epic_id = fx.epic(true).await;
    let closing = fx.closing_subtask(Some(epic_id)).await;
    let next = fx
        .backlog_subtask(Some(epic_id), "Wire up the widget", Some(10), None)
        .await;

    let resp = fx.close(closing, WrapUpAction::Done).await;
    let text = extract_response_text(&resp);
    assert_eq!(
        text,
        format!(
            "Session closed. Dispatching next epic subtask #{} 'Wire up the widget'.",
            next.0
        ),
    );

    wait_for_task_changed(&mut fx.notify_rx, next).await;
}

/// Migrated from `dispatch_next_respects_sort_order`: selection is by
/// `sort_order` ascending, not creation order.
#[tokio::test]
async fn exit_session_chain_respects_sort_order() {
    let mut fx = ChainFixture::new().await;
    let epic_id = fx.epic(true).await;
    let closing = fx.closing_subtask(Some(epic_id)).await;
    // Created first, but sorts last.
    let later = fx
        .backlog_subtask(Some(epic_id), "Task A", Some(20), None)
        .await;
    let earlier = fx
        .backlog_subtask(Some(epic_id), "Task B", Some(10), None)
        .await;

    let resp = fx.close(closing, WrapUpAction::Done).await;
    let text = extract_response_text(&resp);
    assert!(
        text.contains(&format!("#{}", earlier.0)),
        "expected the lower sort_order subtask to be chained, got: {text}"
    );

    wait_for_task_changed(&mut fx.notify_rx, earlier).await;
    assert_eq!(
        fx.db.get_task(later).await.unwrap().unwrap().status,
        TaskStatus::Backlog
    );
}

/// Migrated from `dispatch_next_respects_tag_routing`: the chained dispatch
/// still routes through `DispatchMode::for_task`.
#[tokio::test]
async fn exit_session_chain_respects_tag_routing() {
    let mut fx = ChainFixture::new().await;
    let epic_id = fx.epic(true).await;
    let closing = fx.closing_subtask(Some(epic_id)).await;
    let next = fx
        .backlog_subtask(Some(epic_id), "Research Task", Some(10), None)
        .await;
    // Research + no plan routes to the research agent rather than the standard
    // dispatch agent.
    fx.db
        .patch_task(
            next,
            &db::TaskPatch::new()
                .plan_path(None)
                .tag(Some(crate::models::TaskTag::Research)),
        )
        .await
        .unwrap();
    let task = fx.db.get_task(next).await.unwrap().unwrap();
    assert_eq!(
        crate::models::DispatchMode::for_task(&task),
        crate::models::DispatchMode::Research,
        "fixture must exercise the research routing branch"
    );

    let resp = fx.close(closing, WrapUpAction::Done).await;
    assert!(resp.error.is_none(), "{:?}", resp.error);

    wait_for_task_changed(&mut fx.notify_rx, next).await;
    assert_eq!(
        fx.db.get_task(next).await.unwrap().unwrap().status,
        TaskStatus::Running
    );
}

/// Migrated from `dispatch_next_no_backlog_returns_success_noop`: closing the
/// epic's last subtask closes cleanly and chains nothing.
#[tokio::test]
async fn exit_session_with_no_backlog_subtask_closes_without_chaining() {
    let fx = ChainFixture::new().await;
    let epic_id = fx.epic(true).await;
    let closing = fx.closing_subtask(Some(epic_id)).await;

    let resp = fx.close(closing, WrapUpAction::Done).await;
    assert_eq!(extract_response_text(&resp), "Session closed.");
    assert_eq!(
        fx.db.get_task(closing).await.unwrap().unwrap().status,
        TaskStatus::Done
    );
}

// Migrated from `dispatch_next_epic_not_found_returns_error`, with the
// assertion inverted: an unresolvable epic is warn-and-skip, not an error. It
// cannot be driven from here — `tasks.epic_id` carries a foreign key onto
// `epics(id)` and `PRAGMA foreign_keys=ON`, so a dangling reference is not a
// reachable database state. The branch is covered directly instead by
// `auto_dispatch_next_returns_none_for_missing_epic`, inline in
// src/mcp/handlers/tasks/dispatch.rs.

/// Migrated from `dispatch_next_returns_disabled_when_auto_dispatch_off`
/// (previously in tests/tasks/crud.rs): `auto_dispatch = false` closes the
/// session and chains nothing.
#[tokio::test]
async fn exit_session_does_not_chain_when_auto_dispatch_off() {
    let fx = ChainFixture::new().await;
    let epic_id = fx.epic(false).await;
    let closing = fx.closing_subtask(Some(epic_id)).await;
    let next = fx
        .backlog_subtask(Some(epic_id), "Task 1", Some(10), None)
        .await;

    let resp = fx.close(closing, WrapUpAction::Done).await;
    assert_eq!(extract_response_text(&resp), "Session closed.");

    // The claim is synchronous and completes before exit_session returns, so a
    // subtask still in Backlog here was never claimed and can never transition.
    let untouched = fx.db.get_task(next).await.unwrap().unwrap();
    assert_eq!(untouched.status, TaskStatus::Backlog);
    assert!(untouched.worktree.is_none());
}

/// A task with no epic closes cleanly and chains nothing.
#[tokio::test]
async fn exit_session_without_epic_closes_without_chaining() {
    let fx = ChainFixture::new().await;
    let closing = fx.closing_subtask(None).await;

    let resp = fx.close(closing, WrapUpAction::Done).await;
    assert_eq!(extract_response_text(&resp), "Session closed.");
    assert_eq!(
        fx.db.get_task(closing).await.unwrap().unwrap().status,
        TaskStatus::Done
    );
}

/// The regression guard for this change: by the time the next subtask is
/// running with a worktree, the closed subtask is already terminal with its
/// tmux window cleared. The old skill-driven ordering violated exactly this.
#[tokio::test]
async fn exit_session_chain_starts_only_after_the_closing_task_is_terminal() {
    let mut fx = ChainFixture::new().await;
    let epic_id = fx.epic(true).await;
    let closing = fx.closing_subtask(Some(epic_id)).await;
    let next = fx
        .backlog_subtask(Some(epic_id), "Successor", Some(10), None)
        .await;

    fx.close(closing, WrapUpAction::Done).await;
    wait_for_task_changed(&mut fx.notify_rx, next).await;

    let successor = fx.db.get_task(next).await.unwrap().unwrap();
    assert_eq!(successor.status, TaskStatus::Running);
    assert!(
        successor.worktree.is_some(),
        "successor should be provisioned by the time TaskChanged fires"
    );

    let predecessor = fx.db.get_task(closing).await.unwrap().unwrap();
    assert_eq!(
        predecessor.status,
        TaskStatus::Done,
        "the closed subtask must already be terminal before its successor is provisioned"
    );
    assert!(
        predecessor.tmux_window.is_none(),
        "the closed subtask's window must already be cleared"
    );
}

/// All three wrap-up actions chain, matching the behaviour the skill had when
/// it fired `dispatch_next` regardless of action.
#[tokio::test]
async fn exit_session_chains_for_rebase_action() {
    assert_action_chains(WrapUpAction::Rebase).await;
}

#[tokio::test]
async fn exit_session_chains_for_done_action() {
    assert_action_chains(WrapUpAction::Done).await;
}

#[tokio::test]
async fn exit_session_chains_for_pr_action() {
    assert_action_chains(WrapUpAction::Pr).await;
}

async fn assert_action_chains(action: WrapUpAction) {
    let mut fx = ChainFixture::new().await;
    let epic_id = fx.epic(true).await;
    let closing = fx.closing_subtask(Some(epic_id)).await;
    let next = fx
        .backlog_subtask(Some(epic_id), "Successor", Some(10), None)
        .await;

    let resp = fx.close(closing, action).await;
    let text = extract_response_text(&resp);
    assert!(
        text.contains(&format!("#{}", next.0)),
        "action {:?} must chain, got: {text}",
        action
    );

    wait_for_task_changed(&mut fx.notify_rx, next).await;
    assert_eq!(
        fx.db.get_task(next).await.unwrap().unwrap().status,
        TaskStatus::Running
    );
}

/// A failed dispatch reverts the claim, leaving the subtask dispatchable
/// exactly as it was before the chain fired.
#[tokio::test]
async fn exit_session_chain_reverts_claim_when_dispatch_fails() {
    let mut fx = ChainFixture::new().await;
    let epic_id = fx.epic(true).await;
    let closing = fx.closing_subtask(Some(epic_id)).await;
    // A repo path that does not exist fails provisioning before any subprocess
    // runs, so the failure is deterministic.
    let next = fx
        .backlog_subtask(
            Some(epic_id),
            "Doomed",
            Some(10),
            Some("/nonexistent/dispatch-chain-repo"),
        )
        .await;

    let resp = fx.close(closing, WrapUpAction::Done).await;
    assert!(
        resp.error.is_none(),
        "a failed chain must not fail the close: {:?}",
        resp.error
    );

    wait_for_task_changed(&mut fx.notify_rx, next).await;

    let reverted = fx.db.get_task(next).await.unwrap().unwrap();
    assert_eq!(
        reverted.status,
        TaskStatus::Backlog,
        "a failed dispatch must return the subtask to backlog"
    );
    assert_eq!(
        reverted.sub_status,
        SubStatus::default_for(TaskStatus::Backlog)
    );
    assert!(reverted.worktree.is_none());
    assert!(
        reverted.last_pre_tool_use_at.is_none(),
        "the revert must also drop the activity timestamp the claim seeded, or the \
         subtask is not left exactly as it was before the chain fired"
    );
}

// -- a close whose terminal write does not take effect -----------------------
//
// `ExitSession` (docs/specs/pr-workflow.allium) and `ExitSessionViaMcp`
// (docs/specs/mcp-task-tools.allium) gate the terminal mutation, the tmux
// teardown (`not exists window`) and the `SessionClosed(task)` emission on
// `close_persisted` — whether the single patch carrying status, sub_status, url
// and the cleared tmux_window actually landed. Only consuming the exit token is
// unconditional. The tests below drive the `close_persisted = false` branch.

/// A task service whose `close_session` always fails, so `exit_session`'s
/// terminal close patch cannot land. Every other method inherits the panicking
/// default from `TaskServiceApiStub` — which is itself an assertion: if the
/// chain ever fired on this path it would panic on `claim_next_backlog_task`
/// rather than pass quietly.
struct FailingCloseTaskService;

#[async_trait::async_trait]
impl crate::service::TaskServiceApiStub for FailingCloseTaskService {
    async fn close_session(
        &self,
        _task_id: crate::models::TaskId,
        _outcome: crate::service::CloseSessionOutcome,
    ) -> Result<crate::service::ClosedSession, crate::service::ServiceError> {
        Err(crate::service::ServiceError::Internal(anyhow::anyhow!(
            "simulated persistence failure"
        )))
    }
}

crate::task_service_api!(service_api_stub_bridge, FailingCloseTaskService);

/// The terminal mutation is withheld: the task keeps the status, sub_status and
/// tmux_window it had, so it stays visible in its current column for a manual
/// retry instead of silently appearing finished.
#[tokio::test]
async fn exit_session_failed_close_leaves_the_task_unchanged() {
    let fx = ChainFixture::with_failing_close().await;
    let closing = fx.closing_subtask(None).await;
    let before = fx.db.get_task(closing).await.unwrap().unwrap();

    fx.close(closing, WrapUpAction::Done).await;

    let after = fx.db.get_task(closing).await.unwrap().unwrap();
    assert_eq!(
        after.status, before.status,
        "a close that did not persist must not move the task"
    );
    assert_eq!(after.sub_status, before.sub_status);
    assert_eq!(
        after.tmux_window, before.tmux_window,
        "the task's record of its window is only cleared when the write lands"
    );
}

/// The teardown is inside `if close_persisted:`, so a close that did not persist
/// issues no `tmux kill-window` at all: the window survives, still hosting a live
/// agent the human can attach to in order to retry the close. Leaving the task's
/// `tmux_window` pointing at a killed window is what used to make the failure
/// read as `crashed` from running and as awaiting-merge (via `is_detached`) from
/// review.
///
/// The negative assertion is deterministic rather than a timing snapshot: on this
/// branch nothing is spawned, so no pending task exists that could still issue a
/// kill after the call returns. (The positive counterpart — a persisting close
/// DOES kill the window — is not asserted anywhere: the teardown is a detached,
/// never-joined `spawn_blocking` with no completion signal, and waiting for it
/// would be exactly the timing-dependent pattern `scripts/check-no-test-sleep.sh`
/// exists to reject.)
#[tokio::test]
async fn exit_session_failed_close_issues_no_kill_window() {
    let fx = ChainFixture::with_failing_close().await;
    let closing = fx.closing_subtask(None).await;

    fx.close(closing, WrapUpAction::Done).await;

    let kills = fx.kill_window_calls();
    assert!(
        kills.is_empty(),
        "a close that did not persist must leave the tmux window alive, but it ran: {kills:?}"
    );
}

/// The pr branch of the same gate: neither the Review transition nor the
/// pr-typed url is recorded when the write fails.
#[tokio::test]
async fn exit_session_failed_close_records_no_pr_url() {
    let fx = ChainFixture::with_failing_close().await;
    let closing = fx.closing_subtask(None).await;

    fx.close(closing, WrapUpAction::Pr).await;

    let after = fx.db.get_task(closing).await.unwrap().unwrap();
    assert_eq!(after.status, TaskStatus::Running);
    assert!(
        after.url.is_none(),
        "the pr url is part of the same patch, so it must not appear on its own"
    );
}

/// Still a successful JSON-RPC response — the exit token is already consumed by
/// this point, so an error would strand the agent with no retry path — but the
/// text must not claim the session closed.
#[tokio::test]
async fn exit_session_failed_close_returns_success_reporting_the_failure() {
    let fx = ChainFixture::with_failing_close().await;
    let closing = fx.closing_subtask(None).await;

    let resp = fx.close(closing, WrapUpAction::Done).await;

    assert!(
        resp.error.is_none(),
        "must not be a JSON-RPC error: {:?}",
        resp.error
    );
    assert!(
        !is_error(&resp),
        "must not be an isError tools/call result either"
    );
    let text = extract_response_text(&resp);
    assert!(
        !text.contains("Session closed"),
        "the response must not claim the session closed, got: {text}"
    );
    assert!(
        !text.contains("torn down"),
        "the teardown is gated on the same write, so the response must not claim the \
         session was torn down — it is still alive, got: {text}"
    );
    assert!(
        text.contains(&format!("#{}", closing.0)),
        "the response must name the task whose close failed, got: {text}"
    );
}

/// The exit token is consumed either way, so a naive retry cannot re-enter the
/// close path — it hits the "call wrap_up first" branch instead.
#[tokio::test]
async fn exit_session_failed_close_still_consumes_the_exit_token() {
    let fx = ChainFixture::with_failing_close().await;
    let closing = fx.closing_subtask(None).await;

    fx.close(closing, WrapUpAction::Done).await;

    assert!(
        fx.state.exit_tokens.read().unwrap().get(&closing).is_none(),
        "consuming the token is unconditional"
    );
}

/// `SessionClosed` is withheld, so `AutoDispatchNextSubtask` never runs: a
/// broken close must not be compounded by a freshly launched successor.
#[tokio::test]
async fn exit_session_failed_close_does_not_chain() {
    let fx = ChainFixture::with_failing_close().await;
    let epic_id = fx.epic(true).await;
    let closing = fx.closing_subtask(Some(epic_id)).await;
    let next = fx
        .backlog_subtask(Some(epic_id), "Successor", Some(10), None)
        .await;

    let resp = fx.close(closing, WrapUpAction::Done).await;
    let text = extract_response_text(&resp);
    assert!(
        !text.contains("Dispatching next epic subtask"),
        "a failed close must not report a chain, got: {text}"
    );

    // The claim is synchronous and completes before exit_session returns, so a
    // subtask still in Backlog here was never claimed and can never transition.
    let untouched = fx.db.get_task(next).await.unwrap().unwrap();
    assert_eq!(untouched.status, TaskStatus::Backlog);
    assert!(untouched.worktree.is_none());
}

/// `ExitSessionViaMcp` guidance in docs/specs/mcp-task-tools.allium says the
/// agent must not treat the failure response as a completed close. The tool
/// description is the only surface guaranteed to be in front of the agent at the
/// moment it calls `exit_session` — an agent can reach the tool without the
/// /wrap-up skill loaded — so it is what has to carry that instruction.
#[tokio::test]
async fn exit_session_tool_description_warns_about_a_close_that_did_not_take_effect() {
    let state = test_state().await;
    let resp = call(&state, "tools/list", None).await;
    let tools = resp.result.unwrap()["tools"].as_array().unwrap().clone();
    let description = tools
        .iter()
        .find(|t| t["name"] == "exit_session")
        .expect("exit_session must be registered")["description"]
        .as_str()
        .unwrap()
        .to_string();

    assert!(
        description.contains("did not take effect"),
        "description must name the failure the response can report, got: {description}"
    );
    assert!(
        description.contains("not treat"),
        "description must tell the agent not to treat that response as a completed close, \
         got: {description}"
    );
}

/// The full agent-facing sequence — `wrap_up(action="done")` then
/// `exit_session` with the issued token — chains without the agent asking for
/// it. There is no `dispatch_next` tool to call any more.
#[tokio::test]
async fn wrap_up_then_exit_session_chains_without_an_agent_tool_call() {
    let mut fx = ChainFixture::new().await;
    let epic_id = fx.epic(true).await;
    let closing = fx.closing_subtask(Some(epic_id)).await;
    let next = fx
        .backlog_subtask(Some(epic_id), "Successor", Some(10), None)
        .await;

    let wrap = call(
        &fx.state,
        "tools/call",
        Some(json!({
            "name": "wrap_up",
            "arguments": { "task_id": closing.0, "action": "done" }
        })),
    )
    .await;
    assert!(wrap.error.is_none(), "wrap_up failed: {:?}", wrap.error);
    let token = fx
        .state
        .exit_tokens
        .read()
        .unwrap()
        .get(&closing)
        .unwrap()
        .token
        .clone();

    let resp = call(
        &fx.state,
        "tools/call",
        Some(json!({
            "name": "exit_session",
            "arguments": { "task_id": closing.0, "token": token, "action": "done" }
        })),
    )
    .await;
    let text = extract_response_text(&resp);
    assert!(
        text.contains(&format!("#{}", next.0)),
        "the close itself must chain, got: {text}"
    );

    wait_for_task_changed(&mut fx.notify_rx, next).await;
    assert_eq!(
        fx.db.get_task(next).await.unwrap().unwrap().status,
        TaskStatus::Running
    );
}

/// `dispatch_next` is gone: an agent calling it gets an unknown-tool error.
#[tokio::test]
async fn dispatch_next_tool_no_longer_exists() {
    let state = test_state().await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "dispatch_next",
            "arguments": { "epic_id": 1 }
        })),
    )
    .await;
    assert_error(&resp, "Unknown tool");
}

#[tokio::test]
async fn wrap_up_rebase_preserves_tmux_window() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"main\n"), // git rev-parse --abbrev-ref HEAD
        MockProcessRunner::ok_with_stdout(b""),       // git status --porcelain (clean)
        MockProcessRunner::fail(""),                  // git remote get-url (no remote)
        MockProcessRunner::ok(),                      // git rebase main
        MockProcessRunner::ok(),                      // git merge --ff-only
    ]));
    let state = Arc::new(McpState::new(
        McpDeps {
            db: db.clone(),
            runner,
            embedding_service: EmbeddingService::new_test(),
            data_dir: std::env::temp_dir(),
        },
        None,
    ));

    let task_id = db
        .create_task(CreateTaskRequest {
            title: "Rebase Preserve Window",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Review,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    db.patch_task(
        task_id,
        &db::TaskPatch::new()
            .worktree(Some("/repo/.worktrees/1-rebase-preserve"))
            .tmux_window(Some("task-99")),
    )
    .await
    .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "wrap_up",
            "arguments": { "task_id": task_id.0, "action": "rebase" }
        })),
    )
    .await;
    let text = extract_response_text(&resp);
    assert!(text.contains("wrap_up complete"));
    assert!(
        text.contains("exit_session"),
        "response should instruct agent to call exit_session; got: {text}"
    );

    let task = db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Review,
        "wrap_up must not change status — exit_session owns the Done transition"
    );
    assert!(
        task.tmux_window.is_some(),
        "tmux_window must NOT be cleared — exit_session owns the window kill"
    );
}

#[tokio::test]
async fn wrap_up_rebase_conflict_sets_conflict_substatus() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"main\n"), // git rev-parse HEAD
        MockProcessRunner::ok_with_stdout(b""),       // git status --porcelain (clean)
        MockProcessRunner::fail(""),                  // git remote get-url (no remote)
        MockProcessRunner::fail("CONFLICT (content): Merge conflict in foo.rs"), // git rebase
        MockProcessRunner::ok_with_stdout(b"UU foo.rs\n"), // git status --porcelain (mid-rebase, conflicted)
        MockProcessRunner::ok(),                           // git rebase --abort
    ]));
    let state = Arc::new(McpState::new(
        McpDeps {
            db: db.clone(),
            runner,
            embedding_service: EmbeddingService::new_test(),
            data_dir: std::env::temp_dir(),
        },
        None,
    ));

    let task_id = db
        .create_task(CreateTaskRequest {
            title: "Conflict Sub",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Review,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    db.patch_task(
        task_id,
        &db::TaskPatch::new().worktree(Some("/repo/.worktrees/1-conflict-sub")),
    )
    .await
    .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "wrap_up",
            "arguments": { "task_id": task_id.0, "action": "rebase" }
        })),
    )
    .await;

    assert_error(&resp, "conflict");
    let task = db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Review,
        "Task should remain Review on rebase conflict"
    );
    assert_eq!(
        task.sub_status,
        SubStatus::Conflict,
        "sub_status should be Conflict after rebase conflict"
    );
}

#[tokio::test]
async fn wrap_up_rebase_clears_conflict_substatus_on_non_conflict_error() {
    // When a task has Conflict sub_status from a previous rebase attempt,
    // and a new rebase fails with a non-conflict error (e.g. Other), the
    // stale Conflict sub_status should be cleared — matching TUI behavior.
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::fail(""), // detect_default_branch (symbolic-ref)
        MockProcessRunner::ok_with_stdout(b"main\n"), // git rev-parse --abbrev-ref HEAD
        MockProcessRunner::fail(""), // git remote get-url (no remote)
        MockProcessRunner::fail("fatal: some other git error"), // git rebase (non-conflict failure)
        MockProcessRunner::ok(),     // git rebase --abort
    ]));
    let state = Arc::new(McpState::new(
        McpDeps {
            db: db.clone(),
            runner,
            embedding_service: EmbeddingService::new_test(),
            data_dir: std::env::temp_dir(),
        },
        None,
    ));

    let task_id = db
        .create_task(CreateTaskRequest {
            title: "Stale Conflict",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Review,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    db.patch_task(
        task_id,
        &db::TaskPatch::new()
            .worktree(Some("/repo/.worktrees/1-stale-conflict"))
            .sub_status(SubStatus::Conflict),
    )
    .await
    .unwrap();

    // Verify conflict is set before wrap_up
    let task = db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(task.sub_status, SubStatus::Conflict);

    let _resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "wrap_up",
            "arguments": { "task_id": task_id.0, "action": "rebase" }
        })),
    )
    .await;

    let task = db.get_task(task_id).await.unwrap().unwrap();
    assert_ne!(
        task.sub_status,
        SubStatus::Conflict,
        "Stale Conflict sub_status should be cleared even on non-conflict rebase error"
    );
}

// ---------------------------------------------------------------------------
// dispatch_task tests
// ---------------------------------------------------------------------------

/// The exact command sequence one successful dispatch issues. Scripted rather
/// than uniformly-ok because the order is itself the assertion, and because the
/// split-window call must return a pane id.
fn dispatch_runner_script() -> Arc<MockProcessRunner> {
    Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok(),                    // git fetch origin main
        MockProcessRunner::ok(),                    // tmux new-window
        MockProcessRunner::ok(),                    // tmux set-option @dispatch_dir
        MockProcessRunner::ok(),                    // tmux set-hook
        MockProcessRunner::ok(),                    // tmux send-keys -l (writes prompt file)
        MockProcessRunner::ok(),                    // tmux send-keys Enter
        MockProcessRunner::ok_with_stdout(b"%9\n"), // tmux split-window (agent-tree)
    ]))
}

/// The `dispatch_task` tools/call for `task_id`.
async fn call_dispatch_task(
    state: &Arc<McpState>,
    task_id: crate::models::TaskId,
) -> JsonRpcResponse {
    call(
        state,
        "tools/call",
        Some(json!({
            "name": "dispatch_task",
            "arguments": { "task_id": task_id.0 }
        })),
    )
    .await
}

#[tokio::test]
async fn dispatch_task_dispatches_backlog_task() {
    let fx = ChainFixture::with_runner(dispatch_runner_script()).await;
    let task_id = fx
        .backlog_subtask(None, "My Backlog Task", None, None)
        .await;

    let resp = call_dispatch_task(&fx.state, task_id).await;

    let text = extract_response_text(&resp);
    assert!(
        text.contains("dispatched"),
        "Expected 'dispatched' in response, got: {text}"
    );

    // dispatch_task is synchronous — no sleep needed
    let task = fx.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert!(
        task.worktree.is_some(),
        "worktree should be set after dispatch"
    );
    assert!(
        task.tmux_window.is_some(),
        "tmux_window should be set after dispatch"
    );
    assert!(
        task.last_pre_tool_use_at.is_some(),
        "last_pre_tool_use_at should be seeded so the tick classifier does not flicker the task to Stale"
    );
}

#[tokio::test]
async fn dispatch_task_returns_error_for_non_backlog_task() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Running Task",
            description: "already running",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call_dispatch_task(&state, task_id).await;

    assert_error(&resp, "not in backlog");
}

#[tokio::test]
async fn dispatch_task_unknown_task_id_returns_error() {
    let state = test_state().await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "dispatch_task",
            "arguments": { "task_id": 9999 }
        })),
    )
    .await;

    assert_error(&resp, "not found");
}

#[tokio::test]
async fn dispatch_task_respects_tag_routing() {
    let fx = ChainFixture::with_runner(dispatch_runner_script()).await;
    // Feature-tagged task with no plan → still routes to the standard dispatch
    // agent (only Research + no plan routes elsewhere).
    let task_id = fx.backlog_subtask(None, "Feature Task", None, None).await;
    fx.db
        .patch_task(
            task_id,
            &db::TaskPatch::new()
                .plan_path(None)
                .tag(Some(crate::models::TaskTag::Feature)),
        )
        .await
        .unwrap();

    let resp = call_dispatch_task(&fx.state, task_id).await;

    let text = extract_response_text(&resp);
    assert!(
        text.contains("dispatched"),
        "Expected dispatch confirmation, got: {text}"
    );
    let task = fx.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Running);
}

#[tokio::test]
async fn dispatch_task_dependabot_tag_routes_through_dispatch_agent() {
    // Dependabot tag is a label now — it routes through the unified dispatch
    // agent like any other task without a dedicated dispatch mode.
    let fx = ChainFixture::with_runner(dispatch_runner_script()).await;
    let title = "Bump foo from 1.0.0 to 1.0.1";
    let task_id = fx.backlog_subtask(None, title, None, None).await;
    fx.db
        .patch_task(
            task_id,
            &db::TaskPatch::new().tag(Some(crate::models::TaskTag::Dependabot)),
        )
        .await
        .unwrap();
    let worktree_dir = std::path::Path::new(&fx.repo_path)
        .join(".worktrees")
        .join(format!("{}-{}", task_id.0, crate::models::slugify(title)));

    let resp = call_dispatch_task(&fx.state, task_id).await;

    let text = extract_response_text(&resp);
    assert!(
        text.contains("dispatched"),
        "Expected dispatch confirmation, got: {text}"
    );

    // Should have written the unified prompt with a Dependabot-specific
    // section gated on the tag — not the deleted Dependabot triage agent.
    let prompt = std::fs::read_to_string(worktree_dir.join(".claude-prompt"))
        .expect("dispatch agent should have written a prompt file");
    assert!(
        prompt.contains("Your task is:"),
        "expected the unified dispatch prompt, got:\n{prompt}"
    );
    assert!(
        !prompt.contains("Dependabot triage agent"),
        "Dependabot tag must no longer route to a specialised agent"
    );
    assert!(
        prompt.contains("Dependabot PR review"),
        "Dependabot tag must inject the dependabot review section, got:\n{prompt}"
    );
    assert!(
        prompt.contains("gh pr view") && prompt.contains("gh pr merge"),
        "Dependabot section must include gh PR commands, got:\n{prompt}"
    );
    assert!(
        prompt.contains("Do NOT") && prompt.contains("/wrap-up"),
        "Dependabot section must instruct the agent not to call /wrap-up, got:\n{prompt}"
    );
}

#[tokio::test]
async fn dispatch_task_returns_error_when_dispatch_fails() {
    // First mock call fails (tmux new-window fails) → dispatch errors out.
    let fx = ChainFixture::with_runner(Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::fail("tmux: no server running"),
    ])))
    .await;
    let task_id = fx.backlog_subtask(None, "Backlog Task", None, None).await;

    let resp = call_dispatch_task(&fx.state, task_id).await;

    assert!(is_error(&resp), "expected error when dispatch fails");

    // Task status must remain Backlog — dispatch failure must not leave it as Running
    let task = fx.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Backlog,
        "task should remain Backlog after dispatch failure"
    );
}
