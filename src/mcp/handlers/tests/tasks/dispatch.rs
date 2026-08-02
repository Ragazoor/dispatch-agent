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
    /// Only set by [`ChainFixture::with_bg_done`]: fires with
    /// [`crate::mcp::BackgroundWrite::KillWindow`] after `exit_session`'s
    /// detached tmux teardown completes, so a test can await it deterministically
    /// instead of sleeping.
    bg_done_rx: Option<tokio::sync::mpsc::UnboundedReceiver<crate::mcp::BackgroundWrite>>,
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

    /// Like [`ChainFixture::new`], but with a `task_svc` whose
    /// `claim_backlog_task` always loses, standing in for another entry point
    /// having taken the task first. Every other method panics, so a caller that
    /// dispatches without claiming is caught rather than silently passing.
    async fn with_lost_claim() -> Self {
        Self::build(Some(Arc::new(LostClaimTaskService))).await
    }

    /// Like [`ChainFixture::new`], but wires a completion signal for
    /// `exit_session`'s detached tmux teardown, so a test can await it via
    /// [`ChainFixture::wait_for_kill_window_done`] instead of sleeping.
    async fn with_bg_done() -> Self {
        let runner = Arc::new(MockProcessRunner::new(
            (0..24).map(|_| MockProcessRunner::ok()).collect(),
        ));
        let dir = tempfile::TempDir::new().unwrap();
        let repo_path = dir.path().to_str().unwrap().to_string();
        std::fs::create_dir_all(dir.path().join(".worktrees")).unwrap();

        let (notify_tx, notify_rx) = tokio::sync::mpsc::unbounded_channel::<crate::mcp::McpEvent>();
        let (bg_tx, bg_rx) = tokio::sync::mpsc::unbounded_channel::<crate::mcp::BackgroundWrite>();
        let (state, db) = test_state_with_overrides_and_bg_done(
            runner.clone() as Arc<dyn ProcessRunner>,
            Some(notify_tx),
            None,
            Some(bg_tx),
        )
        .await;
        Self {
            _dir: dir,
            repo_path,
            db,
            state,
            notify_rx,
            runner,
            bg_done_rx: Some(bg_rx),
        }
    }

    /// Await the completion signal for `exit_session`'s detached tmux teardown.
    /// Only valid on a fixture built via [`ChainFixture::with_bg_done`].
    ///
    /// Bounded, like the `bg_done.recv()` in `src/mcp/handlers/tests/usage.rs`: a
    /// teardown that never signals is a real regression, and an unbounded await
    /// would hang the whole suite instead of reporting it. This is a deadline on
    /// a completion signal, not a wall-clock wait for work to finish — the loop
    /// still returns the instant `KillWindow` arrives.
    async fn wait_for_kill_window_done(&mut self) {
        let rx = self
            .bg_done_rx
            .as_mut()
            .expect("wait_for_kill_window_done requires ChainFixture::with_bg_done");
        let wait = async {
            loop {
                match rx.recv().await {
                    Some(crate::mcp::BackgroundWrite::KillWindow) => break,
                    Some(_) => continue,
                    None => {
                        panic!("bg-done channel closed before the kill-window teardown completed")
                    }
                }
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(5), wait)
            .await
            .expect("timed out waiting for the kill-window teardown to signal completion");
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
            bg_done_rx: None,
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

// allow-phantom-symbol: renamed test, cited for provenance
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

// allow-phantom-symbol: renamed test, cited for provenance
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

// allow-phantom-symbol: renamed test, cited for provenance
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

// allow-phantom-symbol: renamed test, cited for provenance
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

// allow-phantom-symbol: renamed test, cited for provenance
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

/// A failed chained dispatch must be visible to the operator, not only to
/// `app.log`. `SurfaceAutoDispatchFailure` (docs/specs/epics.allium) turns the
/// failure into an `AutoDispatchFailed` event carrying the subtask, its epic and
/// the reason; the board renders it from there.
///
/// The event is asserted ahead of `TaskChanged` for the same subtask: it is the
/// fact the failure established, so a consumer that reloads the row first would
/// paint the reverted card before it knows the card is stalled.
#[tokio::test]
async fn exit_session_chain_reports_a_failed_dispatch_to_the_board() {
    let mut fx = ChainFixture::new().await;
    let epic_id = fx.epic(true).await;
    let closing = fx.closing_subtask(Some(epic_id)).await;
    let next = fx
        .backlog_subtask(
            Some(epic_id),
            "Doomed",
            Some(10),
            Some("/nonexistent/dispatch-chain-repo"),
        )
        .await;

    fx.close(closing, WrapUpAction::Done).await;

    loop {
        match fx.notify_rx.recv().await {
            Some(crate::mcp::McpEvent::AutoDispatchFailed {
                task_id,
                epic_id: eid,
                reason,
            }) => {
                assert_eq!(task_id, next);
                assert_eq!(eid, epic_id);
                assert!(
                    !reason.is_empty(),
                    "the event must carry why the dispatch failed"
                );
                break;
            }
            Some(crate::mcp::McpEvent::TaskChanged(id)) if id == next => {
                panic!("TaskChanged for the reverted subtask arrived before AutoDispatchFailed")
            }
            Some(_) => continue,
            None => panic!("notification channel closed before the failure was reported"),
        }
    }
}

/// A chained dispatch that SUCCEEDS reports no failure. Guards against the
/// event being emitted unconditionally, which would mark every chained subtask
/// as stalled.
#[tokio::test]
async fn exit_session_chain_reports_no_failure_when_dispatch_succeeds() {
    let mut fx = ChainFixture::new().await;
    let epic_id = fx.epic(true).await;
    let closing = fx.closing_subtask(Some(epic_id)).await;
    let next = fx
        .backlog_subtask(Some(epic_id), "Successor", Some(10), None)
        .await;

    fx.close(closing, WrapUpAction::Done).await;

    // TaskChanged is sent after the outcome is decided, so anything the failure
    // path would have emitted is already in the channel by the time it arrives.
    loop {
        match fx.notify_rx.recv().await {
            Some(crate::mcp::McpEvent::TaskChanged(id)) if id == next => break,
            Some(crate::mcp::McpEvent::AutoDispatchFailed { .. }) => {
                panic!("a successful chained dispatch must report no failure")
            }
            Some(_) => continue,
            None => panic!("notification channel closed before dispatch completed"),
        }
    }
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

/// A task service whose `claim_backlog_task` always loses the claim, standing in
/// for another dispatch entry point having taken the task a moment earlier.
///
/// Every other method keeps the panicking `TaskServiceApiStub` default on
/// purpose: that is what makes `dispatch_task_lost_claim_provisions_nothing`
/// discriminating. A handler that checked the status instead of claiming would
/// provision the task and then hit the unmocked `update_task`.
struct LostClaimTaskService;

#[async_trait::async_trait]
impl crate::service::TaskServiceApiStub for LostClaimTaskService {
    async fn claim_backlog_task(
        &self,
        _task_id: crate::models::TaskId,
    ) -> Result<bool, crate::service::ServiceError> {
        Ok(false)
    }
}

crate::task_service_api!(service_api_stub_bridge, LostClaimTaskService);

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
/// DOES kill the window — is
/// [`exit_session_successful_close_kills_the_tmux_window`], below: the teardown
/// is a detached, never-joined `spawn_blocking`, but `BackgroundWrite::KillWindow`
/// gives it a completion signal a test can await instead of sleeping.)
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

/// The positive direction of the same `close_persisted` gate
/// (`docs/specs/pr-workflow.allium`: `ExitSession`): a *successful* close issues
/// exactly one `tmux kill-window` for the closing task's window, and only after
/// the terminal Done write has landed. Without a window leak — the exact bug
/// this test exists to prevent — a closed task's tmux window would survive
/// forever.
///
/// `ChainFixture::with_bg_done` gives the detached teardown spawned by
/// `exit_session` a completion signal (`BackgroundWrite::KillWindow`) so this
/// test can await it deterministically instead of sleeping — see the
/// "No `tokio::time::sleep` in tests" section of `docs/conventions.md`.
#[tokio::test]
async fn exit_session_successful_close_kills_the_tmux_window() {
    let mut fx = ChainFixture::with_bg_done().await;
    let closing = fx.closing_subtask(None).await;
    let window = fx
        .db
        .get_task(closing)
        .await
        .unwrap()
        .unwrap()
        .tmux_window
        .expect("closing_subtask must have a tmux window");

    let resp = fx.close(closing, WrapUpAction::Done).await;
    assert!(resp.error.is_none(), "close must succeed: {:?}", resp.error);

    // The Done write is synchronous and already landed by the time `close`
    // returns; only the teardown is detached, so this is the point that needs
    // awaiting.
    fx.wait_for_kill_window_done().await;

    assert_eq!(
        fx.db.get_task(closing).await.unwrap().unwrap().status,
        TaskStatus::Done,
        "the kill-window signal must not fire before the terminal write lands"
    );

    // `kill_window` resolves the window name to tmux's pane-id target
    // (`window_target` in `src/tmux.rs`) before issuing `kill-window`, so the
    // command carries `%N`, not the literal window name — resolve the same way
    // `MockProcessRunner`'s permissive `AnyName` lookup would have.
    let pane_id = fx.runner.pane_id_of(&window);
    let kills = fx.kill_window_calls();
    assert_eq!(
        kills.len(),
        1,
        "expected exactly one tmux kill-window call, got: {kills:?}"
    );
    assert!(
        kills[0].1.iter().any(|arg| arg == &pane_id),
        "expected kill-window to target {pane_id:?} (window {window:?}), got: {kills:?}"
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
    crate::dispatch::mock_sequence::DispatchScript::dispatch().shared_runner()
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

/// Drain `rx` and report whether an `AgentLaunched` naming `repo_path` is among
/// the events already queued. Non-blocking: every emitter below has finished its
/// notifications by the time the caller awaits it.
fn saw_agent_launched(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::mcp::McpEvent>,
    repo_path: &str,
) -> bool {
    let mut seen = false;
    while let Ok(event) = rx.try_recv() {
        if matches!(&event, crate::mcp::McpEvent::AgentLaunched { repo_path: p } if p == repo_path)
        {
            seen = true;
        }
    }
    seen
}

// rule-success.RefreshRepoSyncStateAfterDispatch, MCP surface: the obligation is
// per-event, not per-surface, so `dispatch_task` owes the same refresh the board's
// own dispatch does (docs/specs/repo-sync.allium).
#[tokio::test]
async fn dispatch_task_notifies_that_the_repo_needs_remeasuring() {
    let mut fx = ChainFixture::with_runner(dispatch_runner_script()).await;
    let task_id = fx.backlog_subtask(None, "Measured Task", None, None).await;

    let resp = call_dispatch_task(&fx.state, task_id).await;
    assert!(extract_response_text(&resp).contains("dispatched"));

    let repo_path = fx.repo_path.clone();
    assert!(
        saw_agent_launched(&mut fx.notify_rx, &repo_path),
        "an MCP dispatch must ask the runtime to remeasure {repo_path}"
    );
}

// rule-failure.RefreshRepoSyncStateAfterDispatch: a dispatch that failed launched
// no agent, so there is no AgentLaunched to follow and nothing to remeasure.
#[tokio::test]
async fn a_failed_dispatch_task_notifies_no_remeasure() {
    let mut fx = ChainFixture::new().await;
    let task_id = fx
        .backlog_subtask(None, "Doomed", None, Some("/nonexistent/dispatch-mcp-repo"))
        .await;

    let resp = call_dispatch_task(&fx.state, task_id).await;
    assert_error(&resp, "dispatch failed");

    assert!(
        !saw_agent_launched(&mut fx.notify_rx, "/nonexistent/dispatch-mcp-repo"),
        "no agent launched, so nothing moved and nothing needs remeasuring"
    );
}

// rule-success.RefreshRepoSyncStateAfterDispatch, epic-chain surface: auto-dispatch
// chaining routes through the same DispatchTask rule, so it is not exempt either.
#[tokio::test]
async fn the_auto_dispatch_chain_notifies_that_the_repo_needs_remeasuring() {
    let mut fx = ChainFixture::new().await;
    let epic_id = fx.epic(true).await;
    let closing = fx.closing_subtask(Some(epic_id)).await;
    let next = fx
        .backlog_subtask(Some(epic_id), "Chained Task", Some(10), None)
        .await;

    let resp = fx.close(closing, WrapUpAction::Done).await;
    assert!(resp.error.is_none(), "close must succeed: {:?}", resp.error);

    // The chain dispatches on a spawned task, so wait for its notifications
    // rather than draining what happens to have arrived. `TaskChanged(next)` is
    // unique to the chain (the close notifies about the closing task and the
    // shared epic) and follows the refresh notification, so it bounds the wait
    // and fails fast instead of hanging when that notification is missing.
    let repo_path = fx.repo_path.clone();
    let mut launched = false;
    loop {
        match fx.notify_rx.recv().await {
            Some(crate::mcp::McpEvent::AgentLaunched { repo_path: p }) if p == repo_path => {
                launched = true;
            }
            Some(crate::mcp::McpEvent::TaskChanged(id)) if id == next => break,
            Some(_) => continue,
            None => panic!("notification channel closed before the chain finished"),
        }
    }
    assert!(
        launched,
        "a chained dispatch must ask the runtime to remeasure {repo_path}"
    );
    assert_eq!(
        fx.db.get_task(next).await.unwrap().unwrap().status,
        TaskStatus::Running,
        "the chain really did launch an agent"
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
async fn dispatch_task_reports_the_status_a_lost_claim_left_behind() {
    // The claim, not a pre-read, is what rejects this call now. A task another
    // entry point already took is simply no longer Backlog, so the conditional
    // transition finds nothing to move and the handler re-reads to name the
    // status the task actually holds (DispatchTaskViaMcp in
    // docs/specs/mcp-task-tools.allium).
    let fx = ChainFixture::with_runner(dispatch_runner_script()).await;
    let task_id = fx.backlog_subtask(None, "Contended Task", None, None).await;
    assert!(
        fx.state.task_svc.claim_backlog_task(task_id).await.unwrap(),
        "another entry point claims it first"
    );

    let resp = call_dispatch_task(&fx.state, task_id).await;

    assert_error(&resp, "not in backlog");
    assert_error(&resp, "running");
    let task = fx.db.get_task(task_id).await.unwrap().unwrap();
    assert!(
        task.worktree.is_none(),
        "a lost claim must provision nothing"
    );
}

/// A lost claim must gate provisioning, not merely be noticed afterwards.
///
/// Discriminating by construction: `LostClaimTaskService` mocks only
/// `claim_backlog_task`, and every other `TaskServiceApi` method panics. A
/// handler that read the status instead of claiming would sail past the Backlog
/// row, provision it, and then panic in the unmocked post-dispatch
/// `update_task`. Reaching the assertions at all proves the claim was consulted;
/// the empty runner log proves it was consulted *first*.
#[tokio::test]
async fn dispatch_task_lost_claim_provisions_nothing() {
    let fx = ChainFixture::with_lost_claim().await;
    let task_id = fx.backlog_subtask(None, "Contended Task", None, None).await;

    let resp = call_dispatch_task(&fx.state, task_id).await;

    assert_error(&resp, "not in backlog");
    assert!(
        fx.runner.recorded_calls().is_empty(),
        "a lost claim must run no provisioning commands at all, got: {:?}",
        fx.runner.recorded_calls()
    );
    let task = fx.db.get_task(task_id).await.unwrap().unwrap();
    assert!(task.worktree.is_none());
    assert!(task.tmux_window.is_none());
}

/// A dispatch that fails after the claim must release it, leaving the task
/// exactly as dispatchable as before the call.
///
/// The Backlog end-state is only reachable via the release, given
/// `dispatch_task_lost_claim_provisions_nothing` establishes that the claim is
/// taken before provisioning: the claim moved the row to Running, so something
/// has to move it back.
#[tokio::test]
async fn dispatch_task_releases_the_claim_when_provisioning_fails() {
    let fx = ChainFixture::new().await;
    // A repo path that does not exist fails provisioning before any subprocess
    // runs, so the failure is deterministic — the same shape
    // `exit_session_chain_reverts_claim_when_dispatch_fails` uses.
    let task_id = fx
        .backlog_subtask(None, "Doomed", None, Some("/nonexistent/dispatch-mcp-repo"))
        .await;

    let resp = call_dispatch_task(&fx.state, task_id).await;

    assert_error(&resp, "dispatch failed");
    let task = fx.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Backlog,
        "a failed dispatch must release the claim, not leave the task Running"
    );
    assert_eq!(
        task.sub_status,
        crate::models::SubStatus::default_for(TaskStatus::Backlog)
    );
    assert!(
        task.last_pre_tool_use_at.is_none(),
        "the release clears the stamp the claim seeded"
    );
    assert!(task.worktree.is_none());
    assert!(
        fx.state.task_svc.claim_backlog_task(task_id).await.unwrap(),
        "the released task is dispatchable again"
    );
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
