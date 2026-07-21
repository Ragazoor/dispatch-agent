#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;

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
// -- dispatch_next tests ------------------------------------------------------

#[tokio::test]
async fn dispatch_next_epic_not_found_returns_error() {
    let state = test_state().await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "dispatch_next",
            "arguments": { "epic_id": 9999 }
        })),
    )
    .await;
    assert_error(&resp, "not found");
}

#[tokio::test]
async fn dispatch_next_no_backlog_returns_success_noop() {
    let state = test_state().await;
    let epic = state
        .db_write()
        .create_epic("Test Epic", "desc", None)
        .await
        .unwrap();
    state
        .db_write()
        .patch_epic(epic.id, &db::EpicPatch::new().auto_dispatch(true))
        .await
        .unwrap();

    // Add a task that's already Running (not Backlog)
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Running Task",
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
        .set_task_epic_id(task_id, Some(epic.id))
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "dispatch_next",
            "arguments": { "epic_id": epic.id.0 }
        })),
    )
    .await;

    let text = extract_response_text(&resp);
    assert!(
        text.contains("no backlog tasks"),
        "Expected noop message, got: {text}"
    );
}

#[tokio::test]
async fn dispatch_next_picks_first_backlog_subtask() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo_path = dir.path().to_str().unwrap().to_string();
    std::fs::create_dir_all(dir.path().join(".worktrees")).unwrap();

    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let (notify_tx, mut notify_rx) = tokio::sync::mpsc::unbounded_channel::<crate::mcp::McpEvent>();
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // git fetch origin main
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook
        MockProcessRunner::ok(), // tmux send-keys -l (literal text)
        MockProcessRunner::ok(), // tmux send-keys Enter
    ]));
    let state = Arc::new(McpState::new(
        McpDeps {
            db: db.clone(),
            runner,
            embedding_service: EmbeddingService::new_test(),
            data_dir: std::env::temp_dir(),
        },
        Some(notify_tx),
    ));

    let epic = db.create_epic("Test Epic", "desc", None).await.unwrap();
    db.patch_epic(epic.id, &db::EpicPatch::new().auto_dispatch(true))
        .await
        .unwrap();
    let task1_id = db
        .create_task(CreateTaskRequest {
            title: "Task 1",
            description: "first",
            repo_path: &repo_path,
            plan: Some("docs/plan.md"),
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
    let task2_id = db
        .create_task(CreateTaskRequest {
            title: "Task 2",
            description: "second",
            repo_path: &repo_path,
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
    db.set_task_epic_id(task1_id, Some(epic.id)).await.unwrap();
    db.set_task_epic_id(task2_id, Some(epic.id)).await.unwrap();

    // Pre-create the worktree directory (mocked git won't create it)
    std::fs::create_dir_all(
        dir.path()
            .join(".worktrees")
            .join(format!("{}-task-1", task1_id.0)),
    )
    .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "dispatch_next",
            "arguments": { "epic_id": epic.id.0 }
        })),
    )
    .await;

    let text = extract_response_text(&resp);
    assert!(
        text.contains(&format!("#{}", task1_id.0)),
        "Expected first task ID in response, got: {text}"
    );

    wait_for_task_changed(&mut notify_rx, task1_id).await;

    // Verify the task was dispatched
    let task1 = db.get_task(task1_id).await.unwrap().unwrap();
    assert_eq!(task1.status, TaskStatus::Running);
    assert!(task1.worktree.is_some());
    assert!(task1.tmux_window.is_some());
    assert!(
        task1.last_pre_tool_use_at.is_some(),
        "last_pre_tool_use_at should be seeded so the tick classifier does not flicker the task to Stale"
    );

    // task2 should still be Backlog
    let task2 = db.get_task(task2_id).await.unwrap().unwrap();
    assert_eq!(task2.status, TaskStatus::Backlog);
}

#[tokio::test]
async fn dispatch_next_respects_sort_order() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo_path = dir.path().to_str().unwrap().to_string();
    std::fs::create_dir_all(dir.path().join(".worktrees")).unwrap();

    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook
        MockProcessRunner::ok(), // tmux send-keys -l (literal text)
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

    let epic = db.create_epic("Test Epic", "desc", None).await.unwrap();
    db.patch_epic(epic.id, &db::EpicPatch::new().auto_dispatch(true))
        .await
        .unwrap();

    // task1 has higher ID but lower sort_order — should be picked second
    let task1_id = db
        .create_task(CreateTaskRequest {
            title: "Task A",
            description: "first by id",
            repo_path: &repo_path,
            plan: Some("docs/plan.md"),
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
    let task2_id = db
        .create_task(CreateTaskRequest {
            title: "Task B",
            description: "second by id",
            repo_path: &repo_path,
            plan: Some("docs/plan.md"),
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
    db.set_task_epic_id(task1_id, Some(epic.id)).await.unwrap();
    db.set_task_epic_id(task2_id, Some(epic.id)).await.unwrap();

    // Give task2 a lower sort_order so it should be picked first
    db.patch_task(task2_id, &db::TaskPatch::new().sort_order(Some(1)))
        .await
        .unwrap();
    db.patch_task(task1_id, &db::TaskPatch::new().sort_order(Some(2)))
        .await
        .unwrap();

    // Pre-create worktree dir for task2 (the one that should be dispatched)
    std::fs::create_dir_all(
        dir.path()
            .join(".worktrees")
            .join(format!("{}-task-b", task2_id.0)),
    )
    .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "dispatch_next",
            "arguments": { "epic_id": epic.id.0 }
        })),
    )
    .await;

    let text = extract_response_text(&resp);
    assert!(
        text.contains(&format!("#{}", task2_id.0)),
        "Expected task2 (lower sort_order) to be dispatched, got: {text}"
    );
}

#[tokio::test]
async fn dispatch_next_respects_tag_routing() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo_path = dir.path().to_str().unwrap().to_string();
    std::fs::create_dir_all(dir.path().join(".worktrees")).unwrap();

    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let (notify_tx, mut notify_rx) = tokio::sync::mpsc::unbounded_channel::<crate::mcp::McpEvent>();
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // git fetch origin main
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook
        MockProcessRunner::ok(), // tmux send-keys -l (literal text)
        MockProcessRunner::ok(), // tmux send-keys Enter
    ]));
    let state = Arc::new(McpState::new(
        McpDeps {
            db: db.clone(),
            runner,
            embedding_service: EmbeddingService::new_test(),
            data_dir: std::env::temp_dir(),
        },
        Some(notify_tx),
    ));

    let epic = db.create_epic("Test Epic", "desc", None).await.unwrap();
    db.patch_epic(epic.id, &db::EpicPatch::new().auto_dispatch(true))
        .await
        .unwrap();

    // Create a feature-tagged task with no plan — should use Plan mode
    let task_id = db
        .create_task(CreateTaskRequest {
            title: "Feature Task",
            description: "a feature",
            repo_path: &repo_path,
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
    db.set_task_epic_id(task_id, Some(epic.id)).await.unwrap();
    db.patch_task(
        task_id,
        &db::TaskPatch::new().tag(Some(crate::models::TaskTag::Feature)),
    )
    .await
    .unwrap();

    // Pre-create worktree dir
    std::fs::create_dir_all(
        dir.path()
            .join(".worktrees")
            .join(format!("{}-feature-task", task_id.0)),
    )
    .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "dispatch_next",
            "arguments": { "epic_id": epic.id.0 }
        })),
    )
    .await;

    let text = extract_response_text(&resp);
    assert!(
        text.contains(&format!("#{}", task_id.0)),
        "Expected feature task to be dispatched, got: {text}"
    );

    wait_for_task_changed(&mut notify_rx, task_id).await;

    let task = db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Running);
}

#[tokio::test]
async fn wrap_up_rebase_preserves_tmux_window() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"main\n"), // git rev-parse --abbrev-ref HEAD
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
        MockProcessRunner::fail(""),                  // git remote get-url (no remote)
        MockProcessRunner::fail("CONFLICT (content): Merge conflict in foo.rs"), // git rebase
        MockProcessRunner::ok(),                      // git rebase --abort
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

#[tokio::test]
async fn dispatch_task_dispatches_backlog_task() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo_path = dir.path().to_str().unwrap().to_string();
    std::fs::create_dir_all(dir.path().join(".worktrees")).unwrap();

    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // git fetch origin main
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook
        MockProcessRunner::ok(), // tmux send-keys -l (literal text / write prompt file)
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

    let task_id = db
        .create_task(CreateTaskRequest {
            title: "My Backlog Task",
            description: "do the thing",
            repo_path: &repo_path,
            plan: Some("docs/plan.md"),
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

    // Pre-create worktree dir (mocked git won't create it)
    std::fs::create_dir_all(
        dir.path()
            .join(".worktrees")
            .join(format!("{}-my-backlog-task", task_id.0)),
    )
    .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "dispatch_task",
            "arguments": { "task_id": task_id.0 }
        })),
    )
    .await;

    let text = extract_response_text(&resp);
    assert!(
        text.contains("dispatched"),
        "Expected 'dispatched' in response, got: {text}"
    );

    // dispatch_task is synchronous — no sleep needed
    let task = db.get_task(task_id).await.unwrap().unwrap();
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

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "dispatch_task",
            "arguments": { "task_id": task_id.0 }
        })),
    )
    .await;

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
    let dir = tempfile::TempDir::new().unwrap();
    let repo_path = dir.path().to_str().unwrap().to_string();
    std::fs::create_dir_all(dir.path().join(".worktrees")).unwrap();

    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // git fetch origin main
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook
        MockProcessRunner::ok(), // tmux send-keys -l (literal text / write prompt file)
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

    // Feature-tagged task with no plan → should route to Plan mode
    let task_id = db
        .create_task(CreateTaskRequest {
            title: "Feature Task",
            description: "a new feature",
            repo_path: &repo_path,
            plan: None,
            status: // no plan
            TaskStatus::Backlog,
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
        &db::TaskPatch::new().tag(Some(crate::models::TaskTag::Feature)),
    )
    .await
    .unwrap();

    // Pre-create worktree dir
    std::fs::create_dir_all(
        dir.path()
            .join(".worktrees")
            .join(format!("{}-feature-task", task_id.0)),
    )
    .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "dispatch_task",
            "arguments": { "task_id": task_id.0 }
        })),
    )
    .await;

    let text = extract_response_text(&resp);
    assert!(
        text.contains("dispatched"),
        "Expected dispatch confirmation, got: {text}"
    );

    // Task should be Running — plan mode still dispatches an agent
    let task = db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Running);
}

#[tokio::test]
async fn dispatch_task_dependabot_tag_routes_through_dispatch_agent() {
    // Dependabot tag is a label now — it routes through the unified dispatch
    // agent like any other task without a dedicated dispatch mode.
    let dir = tempfile::TempDir::new().unwrap();
    let repo_path = dir.path().to_str().unwrap().to_string();
    std::fs::create_dir_all(dir.path().join(".worktrees")).unwrap();

    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // git fetch origin main
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook
        MockProcessRunner::ok(), // tmux send-keys -l (writes prompt file)
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

    let task_id = db
        .create_task(CreateTaskRequest {
            title: "Bump foo from 1.0.0 to 1.0.1",
            description: "https://github.com/example/repo/pull/7",
            repo_path: &repo_path,
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
    db.patch_task(
        task_id,
        &db::TaskPatch::new().tag(Some(crate::models::TaskTag::Dependabot)),
    )
    .await
    .unwrap();

    let slug = crate::models::slugify("Bump foo from 1.0.0 to 1.0.1");
    let worktree_dir = dir
        .path()
        .join(".worktrees")
        .join(format!("{}-{}", task_id.0, slug));
    std::fs::create_dir_all(&worktree_dir).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "dispatch_task",
            "arguments": { "task_id": task_id.0 }
        })),
    )
    .await;

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
    let dir = tempfile::TempDir::new().unwrap();
    let repo_path = dir.path().to_str().unwrap().to_string();
    std::fs::create_dir_all(dir.path().join(".worktrees")).unwrap();

    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    // First mock call fails (tmux new-window fails) → dispatch errors out
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::fail("tmux: no server running"), // tmux new-window fails
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
            title: "Backlog Task",
            description: "will fail to dispatch",
            repo_path: &repo_path,
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
            "name": "dispatch_task",
            "arguments": { "task_id": task_id.0 }
        })),
    )
    .await;

    assert!(is_error(&resp), "expected error when dispatch fails");

    // Task status must remain Backlog — dispatch failure must not leave it as Running
    let task = db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Backlog,
        "task should remain Backlog after dispatch failure"
    );
}
