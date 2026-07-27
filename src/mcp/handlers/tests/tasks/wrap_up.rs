#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;

// ---------------------------------------------------------------------------
// wrap_up tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn wrap_up_task_not_found() {
    let state = test_state().await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "wrap_up",
            "arguments": { "task_id": 9999, "action": "rebase" }
        })),
    )
    .await;
    assert_error(&resp, "not found");
}

#[tokio::test]
async fn wrap_up_rejects_backlog_task() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "T",
            description: "d",
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
            "name": "wrap_up",
            "arguments": { "task_id": task_id.0, "action": "rebase" }
        })),
    )
    .await;
    assert_error(&resp, "cannot be wrapped up");
}

#[tokio::test]
async fn wrap_up_accepts_running_blocked_task() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![
        // task.base_branch = "main" is passed explicitly to finish_task; no symbolic-ref call
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
            title: "My Task",
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
        task_id,
        &db::TaskPatch::new()
            .worktree(Some("/repo/.worktrees/1-my-task"))
            .sub_status(crate::models::SubStatus::NeedsInput),
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
    assert!(
        text.contains("wrap_up complete"),
        "Expected 'wrap_up complete', got: {text}"
    );
}

#[tokio::test]
async fn wrap_up_accepts_running_active_task() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![
        // task.base_branch = "main" is passed explicitly to finish_task; no symbolic-ref call
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
            title: "T",
            description: "d",
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
        task_id,
        &db::TaskPatch::new().worktree(Some("/repo/.worktrees/1-t")),
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
    assert!(
        text.contains("wrap_up complete"),
        "Expected 'wrap_up complete', got: {text}"
    );
}

#[tokio::test]
async fn wrap_up_rebase_response_demands_exit_session_imperatively() {
    // The wrap_up rebase response is the agent's primary cue to call
    // exit_session. It must:
    //   - name exit_session as the next call,
    //   - be imperative (not advisory like "when ready"),
    //   - say the session is not yet closed so the agent does not stop.
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
            title: "T",
            description: "d",
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
        task_id,
        &db::TaskPatch::new().worktree(Some("/repo/.worktrees/1-t")),
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

    assert!(
        text.contains("exit_session"),
        "response must name exit_session as the next call; got: {text}"
    );
    assert!(
        text.contains("MUST"),
        "response must be imperative (contain 'MUST'); got: {text}"
    );
    assert!(
        text.contains("NOT") || text.contains("not yet"),
        "response must clearly say the session is not yet closed; got: {text}"
    );
    assert!(
        !text.contains("when ready"),
        "response must not be advisory ('when ready'); got: {text}"
    );
    assert!(
        text.contains("action=\"rebase\""),
        "response must tell the agent which action to pass to exit_session; got: {text}"
    );

    let map = state.exit_tokens.read().unwrap();
    let et = map
        .get(&task_id)
        .expect("token should be stored after successful rebase");
    assert!(
        text.contains(&et.token),
        "response must include the exit token; got: {text}"
    );
}

#[tokio::test]
async fn wrap_up_task_no_worktree() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "T",
            description: "d",
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
            "name": "wrap_up",
            "arguments": { "task_id": task_id.0, "action": "rebase" }
        })),
    )
    .await;
    assert_error(&resp, "cannot be wrapped up");
}

#[tokio::test]
async fn wrap_up_invalid_action() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "T",
            description: "d",
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
    state
        .db_write()
        .patch_task(
            task_id,
            &db::TaskPatch::new().worktree(Some("/repo/.worktrees/1-t")),
        )
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "wrap_up",
            "arguments": { "task_id": task_id.0, "action": "teleport" }
        })),
    )
    .await;
    assert_error(&resp, "unknown variant `teleport`");
}

#[tokio::test]
async fn wrap_up_rebase_returns_started() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![
        // task.base_branch = "main" is passed explicitly to finish_task; no symbolic-ref call
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
            title: "My Task",
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
        &db::TaskPatch::new().worktree(Some("/repo/.worktrees/1-my-task")),
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
    assert!(
        text.contains("wrap_up complete"),
        "Expected 'wrap_up complete', got: {text}"
    );
}

#[tokio::test]
async fn wrap_up_rebase_returns_exit_token() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"main\n"),
        MockProcessRunner::ok_with_stdout(b""),
        MockProcessRunner::fail(""),
        MockProcessRunner::ok(),
        MockProcessRunner::ok(),
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
    let task_id = create_wrappable_task(&db).await;

    let resp = call(
        &state,
        "tools/call",
        Some(
            json!({ "name": "wrap_up", "arguments": { "task_id": task_id.0, "action": "rebase" } }),
        ),
    )
    .await;

    assert!(
        !is_error(&resp),
        "expected success, got: {}",
        error_message(&resp)
    );
    let text = extract_response_text(&resp);

    let map = state.exit_tokens.read().unwrap();
    let et = map
        .get(&task_id)
        .expect("token should be in exit_tokens after wrap_up rebase");
    assert!(!et.token.is_empty(), "token must be non-empty");
    assert_eq!(
        et.action,
        crate::mcp::handlers::tasks::WrapUpAction::Rebase,
        "token must record the action it was issued for"
    );
    assert!(
        text.contains(&et.token),
        "response text must contain the token; got: {text}"
    );
}

#[tokio::test]
async fn wrap_up_done_returns_exit_token() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
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
            title: "T",
            description: "d",
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
        task_id,
        &db::TaskPatch::new()
            .worktree(Some("/repo/.worktrees/1-t"))
            .tmux_window(Some("task-1")),
    )
    .await
    .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "wrap_up", "arguments": { "task_id": task_id.0, "action": "done" } })),
    )
    .await;

    assert!(
        !is_error(&resp),
        "expected success, got: {}",
        error_message(&resp)
    );
    let text = extract_response_text(&resp);

    {
        let map = state.exit_tokens.read().unwrap();
        let et = map
            .get(&task_id)
            .expect("token should be in exit_tokens after wrap_up done");
        assert!(!et.token.is_empty(), "token must be non-empty");
        assert_eq!(
            et.action,
            crate::mcp::handlers::tasks::WrapUpAction::Done,
            "token must record the action it was issued for"
        );
        assert!(
            text.contains(&et.token),
            "response text must contain the token; got: {text}"
        );
    }

    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Running,
        "wrap_up(done) must defer the Done transition to exit_session"
    );
}

// ---------------------------------------------------------------------------
// wrap_up PR action tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn wrap_up_pr_defers_review_and_url_to_exit_session() {
    // wrap_up(pr) no longer takes pr_url and no longer mutates task.status/url —
    // it only issues a token recording action=pr. The terminal mutation moves
    // to exit_session(action="pr", pr_url=...).
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "PR Task",
            description: "d",
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
            task_id,
            &db::TaskPatch::new().worktree(Some("/repo/.worktrees/1-pr-task")),
        )
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "wrap_up",
            "arguments": { "task_id": task_id.0, "action": "pr" }
        })),
    )
    .await;

    assert!(
        !is_error(&resp),
        "expected success, got: {}",
        error_message(&resp)
    );

    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Running,
        "task must stay running until exit_session closes"
    );
    assert!(task.url.is_none(), "url must not be set yet");

    let map = state.exit_tokens.read().unwrap();
    let et = map
        .get(&task_id)
        .expect("token should be in exit_tokens after wrap_up pr");
    assert_eq!(
        et.action,
        crate::mcp::handlers::tasks::WrapUpAction::Pr,
        "token must record action=pr"
    );
}

#[tokio::test]
async fn wrap_up_pr_response_contains_token_and_retro_instruction() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "T",
            description: "d",
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
            task_id,
            &db::TaskPatch::new().worktree(Some("/repo/.worktrees/1-t")),
        )
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "wrap_up",
            "arguments": { "task_id": task_id.0, "action": "pr" }
        })),
    )
    .await;

    assert!(
        !is_error(&resp),
        "expected success, got: {}",
        error_message(&resp)
    );

    // A token IS issued now — the pr path is no longer exit_session-free.
    let map = state.exit_tokens.read().unwrap();
    let et = map
        .get(&task_id)
        .expect("wrap_up(pr) must insert an exit token");

    let text = extract_response_text(&resp);
    assert!(
        text.contains(&et.token),
        "response must include the exit token; got: {text}"
    );
    assert!(
        text.contains("exit_session"),
        "response should direct the agent to exit_session; got: {text}"
    );
    assert!(
        !text.contains("do not call exit_session") && !text.contains("do not call `exit_session`"),
        "response must no longer tell the agent to skip exit_session; got: {text}"
    );
    // PR-action wrap_up still nudges the agent to rate any unrated retrieved learnings.
    assert!(
        text.contains("rate_learning"),
        "PR wrap_up response should nudge rate_learning; got: {text}"
    );
}

// ---------------------------------------------------------------------------
// wrap_up rebase helpers + tests
// ---------------------------------------------------------------------------

async fn make_state_with_runner(
    runner: Arc<dyn ProcessRunner>,
) -> (Arc<McpState>, Arc<dyn db::TaskStore>) {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let state = Arc::new(McpState::new(
        McpDeps {
            db: db.clone(),
            runner,
            embedding_service: EmbeddingService::new_test(),
            data_dir: std::env::temp_dir(),
        },
        None,
    ));
    (state, db)
}

fn rebase_ok_runner() -> Arc<dyn ProcessRunner> {
    Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"main\n"), // rev-parse HEAD
        MockProcessRunner::ok_with_stdout(b""),       // git status --porcelain (clean)
        MockProcessRunner::fail(""),                  // remote get-url
        MockProcessRunner::ok(),                      // git rebase
        MockProcessRunner::ok(),                      // git merge --ff-only
    ]))
}

async fn create_wrappable_task(db: &Arc<dyn db::TaskStore>) -> crate::models::TaskId {
    let task_id = db
        .create_task(CreateTaskRequest {
            title: "T",
            description: "d",
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
        task_id,
        &db::TaskPatch::new().worktree(Some("/repo/.worktrees/1-t")),
    )
    .await
    .unwrap();
    task_id
}

#[tokio::test]
async fn wrap_up_without_verdicts_still_succeeds() {
    let (state, db) = make_state_with_runner(rebase_ok_runner()).await;
    let task_id = create_wrappable_task(&db).await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "wrap_up",
            "arguments": { "task_id": task_id.0, "action": "rebase" }
        })),
    )
    .await;
    if let Some(err) = resp.error.as_ref() {
        assert!(
            !err.message.contains("verdict"),
            "verdict path should not trigger when none provided: {}",
            err.message
        );
    }
}

// ---------------------------------------------------------------------------
// wrap_up verify reminder tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn wrap_up_success_includes_verify_reminder_when_configured() {
    let (state, db) = make_state_with_runner(rebase_ok_runner()).await;
    let task_id = create_wrappable_task(&db).await;
    db.set_verify_command("/repo", Some("cargo test"))
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
    assert!(
        text.contains("Verify before exiting"),
        "Expected 'Verify before exiting' in response, got: {text}"
    );
    assert!(
        text.contains("cargo test"),
        "Expected 'cargo test' in response, got: {text}"
    );
    assert!(
        text.contains("exit_session"),
        "Expected 'exit_session' in response, got: {text}"
    );
}

#[tokio::test]
async fn wrap_up_success_omits_verify_reminder_when_unconfigured() {
    let (state, db) = make_state_with_runner(rebase_ok_runner()).await;
    let task_id = create_wrappable_task(&db).await;

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
    assert!(
        !text.contains("Verify before"),
        "Expected no 'Verify before' in unconfigured response, got: {text}"
    );
    assert!(
        text.contains("exit_session"),
        "Expected 'exit_session' in response, got: {text}"
    );
}

#[tokio::test]
async fn wrap_up_pr_success_includes_verify_reminder_when_configured() {
    // The pr branch previously omitted the verify-command line the spec already
    // claimed it included — now it's added, matching rebase/done.
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "T",
            description: "d",
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
            task_id,
            &db::TaskPatch::new().worktree(Some("/repo/.worktrees/1-t")),
        )
        .await
        .unwrap();
    state
        .db_write()
        .set_verify_command("/repo", Some("cargo test"))
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "wrap_up", "arguments": { "task_id": task_id.0, "action": "pr" } })),
    )
    .await;

    let text = extract_response_text(&resp);
    assert!(
        text.contains("Verify before exiting"),
        "Expected 'Verify before exiting' in pr response, got: {text}"
    );
    assert!(
        text.contains("cargo test"),
        "Expected 'cargo test' in pr response, got: {text}"
    );
}

#[tokio::test]
async fn wrap_up_rebase_conflict_returns_error() {
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
            title: "Conflict Task",
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
        &db::TaskPatch::new().worktree(Some("/repo/.worktrees/1-conflict-task")),
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
    assert_error(&resp, "foo.rs");
    let task = db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Review,
        "Task should remain Review on rebase conflict"
    );
}

#[tokio::test]
async fn wrap_up_rebase_dirty_primary_worktree_returns_error() {
    // A dirty primary worktree must be reported as its own distinct error —
    // not conflated with a rebase conflict — and must not flip the task's
    // sub_status to Conflict, since no rebase was ever attempted.
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"main\n"), // git rev-parse HEAD
        MockProcessRunner::ok_with_stdout(b" M unrelated.rs\n"), // git status --porcelain (dirty)
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
            title: "Dirty Primary Task",
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
        &db::TaskPatch::new().worktree(Some("/repo/.worktrees/1-dirty-primary-task")),
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

    assert_error(&resp, "uncommitted");
    assert_error(&resp, "/repo");
    let error_text = error_message(&resp);
    assert!(
        !error_text.to_lowercase().contains("rebase conflict"),
        "dirty primary worktree must not be reported as a rebase conflict, got: {error_text}"
    );
    let task = db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Review,
        "Task should remain Review on dirty primary worktree"
    );
    assert_ne!(
        task.sub_status,
        SubStatus::Conflict,
        "dirty primary worktree must not set the Conflict sub_status"
    );
}

#[tokio::test]
async fn wrap_up_rebase_not_on_main_returns_error() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::fail(""), // git rev-parse (empty stdout → treated as non-main)
        MockProcessRunner::ok_with_stdout(b"feature\n"), // unused
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
            title: "Wrong Branch",
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
        &db::TaskPatch::new().worktree(Some("/repo/.worktrees/1-wrong-branch")),
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

    assert_error(&resp, "not on main");
    let task = db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Review,
        "Task should remain Review on error"
    );
}

#[tokio::test]
async fn update_task_status_recalculates_epic_status() {
    let state = test_state().await;
    let epic = state.db_write().create_epic("E", "", None).await.unwrap();
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "T",
            description: "",
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
    state
        .db_write()
        .set_task_epic_id(task_id, Some(epic.id))
        .await
        .unwrap();

    // Move subtask to Running
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "status": "running" }
        })),
    )
    .await;
    assert!(
        resp.error.is_none(),
        "update_task should succeed: {:?}",
        resp.error
    );

    // Epic stays in backlog (running tasks do not auto-advance)
    let epic = state.db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(epic.status, TaskStatus::Backlog);
}
// =======================================================================
// Notification flow tests
// =======================================================================

/// Helper: creates a test state with a real notification channel.
async fn test_state_with_notify() -> (
    Arc<McpState>,
    tokio::sync::mpsc::UnboundedReceiver<crate::mcp::McpEvent>,
) {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let state = Arc::new(McpState::new(
        McpDeps {
            db,
            runner,
            embedding_service: EmbeddingService::new_test(),
            data_dir: std::env::temp_dir(),
        },
        Some(tx),
    ));
    (state, rx)
}

#[tokio::test]
async fn update_task_sends_refresh_notification() {
    let (state, mut rx) = test_state_with_notify().await;
    let task_id = create_task_fixture(&state).await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "status": "running" }
        })),
    )
    .await;
    assert!(resp.error.is_none());

    // Should have received a targeted TaskChanged(task_id) event
    let event = rx
        .try_recv()
        .expect("expected notification after update_task");
    assert!(
        matches!(event, crate::mcp::McpEvent::TaskChanged(t) if t == task_id),
        "expected TaskChanged({task_id:?}), got {event:?}"
    );
}

#[tokio::test]
async fn create_task_sends_refresh_notification() {
    let (state, mut rx) = test_state_with_notify().await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_task",
            "arguments": { "title": "Notified Task", "repo_path": "/repo" }
        })),
    )
    .await;
    assert!(resp.error.is_none());

    let event = rx
        .try_recv()
        .expect("expected notification after create_task");
    assert!(
        matches!(event, crate::mcp::McpEvent::TaskChanged(_)),
        "expected TaskChanged, got {event:?}"
    );
}

#[tokio::test]
async fn claim_task_sends_refresh_notification() {
    let (state, mut rx) = test_state_with_notify().await;
    let task_id = create_task_fixture(&state).await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "claim_task",
            "arguments": {
                "task_id": task_id.0,
                "worktree": "/repo/.worktrees/1-test",
                "tmux_window": "task-1"
            }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "{:?}", resp.error);

    let event = rx
        .try_recv()
        .expect("expected notification after claim_task");
    assert!(
        matches!(event, crate::mcp::McpEvent::TaskChanged(t) if t == task_id),
        "expected TaskChanged({task_id:?}), got {event:?}"
    );
}

#[tokio::test]
async fn failed_update_does_not_send_notification() {
    let (state, mut rx) = test_state_with_notify().await;
    let task_id = create_task_fixture(&state).await;

    // Invalid status should not trigger notification
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "status": "bogus" }
        })),
    )
    .await;
    assert!(is_error(&resp));

    assert!(
        rx.try_recv().is_err(),
        "no notification should be sent on validation error"
    );
}
// =======================================================================
// wrap_up: reflection nudge
// =======================================================================

async fn make_rebase_state() -> (Arc<dyn db::TaskStore>, Arc<McpState>) {
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
    (db, state)
}

async fn seed_task_with_worktree(
    db: &Arc<dyn db::TaskStore>,
    suffix: &str,
) -> crate::models::TaskId {
    let task_id = db
        .create_task(CreateTaskRequest {
            title: &format!("Task {suffix}"),
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
        task_id,
        &db::TaskPatch::new().worktree(Some(&format!(
            "/repo/.worktrees/{}-task-{suffix}",
            task_id.0
        ))),
    )
    .await
    .unwrap();
    task_id
}

#[tokio::test]
async fn wrap_up_rebase_directs_to_exit_session_not_reflection_nudge() {
    // After the behavioral change, wrap_up(rebase) no longer emits the
    // reflection nudge inline. Instead it tells the agent to call exit_session,
    // which handles the reflection prompt on first call.
    let (db, state) = make_rebase_state().await;
    let task_id = seed_task_with_worktree(&db, "nudge-default").await;

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
    assert!(
        text.contains("exit_session"),
        "response should direct agent to call exit_session; got: {text}"
    );
    assert!(
        !text.contains("record_learning"),
        "reflection nudge must not appear in wrap_up(rebase) response; got: {text}"
    );
}

#[tokio::test]
async fn wrap_up_rebase_omits_reflection_nudge_regardless_of_setting() {
    // The learning_reflection_enabled setting no longer affects wrap_up(rebase)
    // — the nudge has moved to exit_session.
    let (db, state) = make_rebase_state().await;
    db.set_setting_bool("learning_reflection_enabled", true)
        .await
        .unwrap();
    let task_id = seed_task_with_worktree(&db, "nudge-setting-irrelevant").await;

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
    assert!(
        !text.contains("record_learning"),
        "nudge must not appear in wrap_up(rebase) even when setting=true; got: {text}"
    );
    assert!(
        text.contains("exit_session"),
        "response should direct to exit_session; got: {text}"
    );
}

// -- exit_session tests -------------------------------------------------------
//
// exit_session is now a single call: exit_session(task_id, token, action, pr_url?).
// There is no more reflect-then-close two-phase dance — the mandatory
// reflection is the /retro skill, run before exit_session is ever called.

#[tokio::test]
async fn exit_session_rebase_closes_session_in_one_call() {
    let state = test_state().await;
    let task_id = create_running_task_with_window(&state).await;
    state.exit_tokens.write().unwrap().insert(
        task_id,
        crate::mcp::ExitToken {
            token: "tok-rebase".to_string(),
            action: crate::mcp::handlers::tasks::WrapUpAction::Rebase,
        },
    );

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "exit_session",
            "arguments": { "task_id": task_id.0, "token": "tok-rebase", "action": "rebase" }
        })),
    )
    .await;

    assert_eq!(extract_response_text(&resp), "Session closed.");
    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert!(task.tmux_window.is_none());
    assert_eq!(task.status, TaskStatus::Done);
    assert_eq!(task.sub_status, SubStatus::default_for(TaskStatus::Done));
}

#[tokio::test]
async fn exit_session_done_closes_session_in_one_call() {
    let state = test_state().await;
    let task_id = create_running_task_with_window(&state).await;
    state.exit_tokens.write().unwrap().insert(
        task_id,
        crate::mcp::ExitToken {
            token: "tok-done".to_string(),
            action: crate::mcp::handlers::tasks::WrapUpAction::Done,
        },
    );

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "exit_session",
            "arguments": { "task_id": task_id.0, "token": "tok-done", "action": "done" }
        })),
    )
    .await;

    assert_eq!(extract_response_text(&resp), "Session closed.");
    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert!(task.tmux_window.is_none());
    assert_eq!(task.status, TaskStatus::Done);
}

#[tokio::test]
async fn exit_session_pr_sets_review_and_url() {
    let state = test_state().await;
    let task_id = create_running_task_with_window(&state).await;
    state.exit_tokens.write().unwrap().insert(
        task_id,
        crate::mcp::ExitToken {
            token: "tok-pr".to_string(),
            action: crate::mcp::handlers::tasks::WrapUpAction::Pr,
        },
    );

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "exit_session",
            "arguments": {
                "task_id": task_id.0,
                "token": "tok-pr",
                "action": "pr",
                "pr_url": "https://github.com/owner/repo/pull/7"
            }
        })),
    )
    .await;

    assert_eq!(extract_response_text(&resp), "Session closed.");
    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert!(task.tmux_window.is_none());
    assert_eq!(task.status, TaskStatus::Review);
    assert_eq!(task.sub_status, SubStatus::default_for(TaskStatus::Review));
    assert_eq!(
        task.url.as_ref().map(|u| u.url.as_str()),
        Some("https://github.com/owner/repo/pull/7")
    );
    assert_eq!(
        task.url.as_ref().map(|u| u.url_type),
        Some(crate::models::UrlType::Pr)
    );
}

#[tokio::test]
async fn exit_session_pr_without_pr_url_errors() {
    let state = test_state().await;
    let task_id = create_running_task_with_window(&state).await;
    state.exit_tokens.write().unwrap().insert(
        task_id,
        crate::mcp::ExitToken {
            token: "tok-pr-nourl".to_string(),
            action: crate::mcp::handlers::tasks::WrapUpAction::Pr,
        },
    );

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "exit_session",
            "arguments": { "task_id": task_id.0, "token": "tok-pr-nourl", "action": "pr" }
        })),
    )
    .await;
    assert_error(&resp, "pr_url");

    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Running,
        "a rejected close must not mutate the task"
    );
}

#[tokio::test]
async fn exit_session_action_mismatch_errors() {
    // Token was issued for wrap_up(action="rebase"); closing with a different
    // action must be rejected — this stops an agent from doing one thing and
    // closing as another.
    let state = test_state().await;
    let task_id = create_running_task_with_window(&state).await;
    state.exit_tokens.write().unwrap().insert(
        task_id,
        crate::mcp::ExitToken {
            token: "tok-mismatch".to_string(),
            action: crate::mcp::handlers::tasks::WrapUpAction::Rebase,
        },
    );

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "exit_session",
            "arguments": {
                "task_id": task_id.0,
                "token": "tok-mismatch",
                "action": "pr",
                "pr_url": "https://github.com/owner/repo/pull/1"
            }
        })),
    )
    .await;
    assert_error(&resp, "rebase");

    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Running,
        "mismatch must not mutate the task"
    );
    assert!(
        task.tmux_window.is_some(),
        "mismatch must not clear the window"
    );
}

#[tokio::test]
async fn exit_session_without_action_errors() {
    let state = test_state().await;
    let task_id = create_running_task_with_window(&state).await;
    state.exit_tokens.write().unwrap().insert(
        task_id,
        crate::mcp::ExitToken {
            token: "tok-no-action".to_string(),
            action: crate::mcp::handlers::tasks::WrapUpAction::Done,
        },
    );

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "exit_session",
            "arguments": { "task_id": task_id.0, "token": "tok-no-action" }
        })),
    )
    .await;
    assert_error(&resp, "action");
}

#[tokio::test]
async fn exit_session_after_close_token_is_gone() {
    let state = test_state().await;
    let task_id = create_running_task_with_window(&state).await;
    state.exit_tokens.write().unwrap().insert(
        task_id,
        crate::mcp::ExitToken {
            token: "tok-once".to_string(),
            action: crate::mcp::handlers::tasks::WrapUpAction::Done,
        },
    );

    // Close.
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "exit_session",
            "arguments": { "task_id": task_id.0, "token": "tok-once", "action": "done" }
        })),
    )
    .await;
    assert_eq!(extract_response_text(&resp), "Session closed.");

    // Second call — token is gone, and there's no window left either.
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "exit_session",
            "arguments": { "task_id": task_id.0, "token": "tok-once", "action": "done" }
        })),
    )
    .await;
    assert_error(&resp, "wrap_up first");
}

#[tokio::test]
async fn exit_session_full_flow_rebase() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"main\n"),
        MockProcessRunner::ok_with_stdout(b""),
        MockProcessRunner::fail(""),
        MockProcessRunner::ok(),
        MockProcessRunner::ok(),
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
    let task_id = create_wrappable_task(&db).await;
    db.patch_task(task_id, &db::TaskPatch::new().tmux_window(Some("task-1")))
        .await
        .unwrap();

    // Rebase
    let wrap_resp = call(
        &state,
        "tools/call",
        Some(
            json!({ "name": "wrap_up", "arguments": { "task_id": task_id.0, "action": "rebase" } }),
        ),
    )
    .await;
    assert!(!is_error(&wrap_resp));

    let token = state
        .exit_tokens
        .read()
        .unwrap()
        .get(&task_id)
        .unwrap()
        .token
        .clone();

    // Single exit_session call closes the session.
    let close_resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "exit_session",
            "arguments": { "task_id": task_id.0, "token": token, "action": "rebase" }
        })),
    )
    .await;
    assert_eq!(extract_response_text(&close_resp), "Session closed.");

    let task = db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Done);
    assert!(task.tmux_window.is_none());
}

#[tokio::test]
async fn wrap_up_second_call_overwrites_token() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![
        // First rebase
        MockProcessRunner::ok_with_stdout(b"main\n"),
        MockProcessRunner::ok_with_stdout(b""),
        MockProcessRunner::fail(""),
        MockProcessRunner::ok(),
        MockProcessRunner::ok(),
        // Second rebase
        MockProcessRunner::ok_with_stdout(b"main\n"),
        MockProcessRunner::ok_with_stdout(b""),
        MockProcessRunner::fail(""),
        MockProcessRunner::ok(),
        MockProcessRunner::ok(),
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
    let task_id = create_wrappable_task(&db).await;
    db.patch_task(task_id, &db::TaskPatch::new().tmux_window(Some("task-1")))
        .await
        .unwrap();

    call(
        &state,
        "tools/call",
        Some(
            json!({ "name": "wrap_up", "arguments": { "task_id": task_id.0, "action": "rebase" } }),
        ),
    )
    .await;
    let first_token = state
        .exit_tokens
        .read()
        .unwrap()
        .get(&task_id)
        .unwrap()
        .token
        .clone();

    call(
        &state,
        "tools/call",
        Some(
            json!({ "name": "wrap_up", "arguments": { "task_id": task_id.0, "action": "rebase" } }),
        ),
    )
    .await;
    let second_token = state
        .exit_tokens
        .read()
        .unwrap()
        .get(&task_id)
        .unwrap()
        .token
        .clone();

    assert_ne!(
        first_token, second_token,
        "second wrap_up should generate a new token"
    );

    // Old token rejected
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "exit_session",
            "arguments": { "task_id": task_id.0, "token": first_token, "action": "rebase" }
        })),
    )
    .await;
    assert_error(&resp, "invalid exit token");
}

#[tokio::test]
async fn exit_session_unknown_task_returns_error() {
    let state = test_state().await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "exit_session",
            "arguments": { "task_id": 9999, "token": "any" }
        })),
    )
    .await;

    assert_error(&resp, "9999");
}

#[tokio::test]
async fn exit_session_task_without_window_returns_error() {
    let state = test_state().await;
    let task_id = create_task_fixture(&state).await; // Backlog task, no tmux_window

    // Insert a matching token+action so we get past those checks.
    state.exit_tokens.write().unwrap().insert(
        task_id,
        crate::mcp::ExitToken {
            token: "tok-no-win".to_string(),
            action: crate::mcp::handlers::tasks::WrapUpAction::Done,
        },
    );

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "exit_session",
            "arguments": { "task_id": task_id.0, "token": "tok-no-win", "action": "done" }
        })),
    )
    .await;

    assert_error(&resp, "no active session");
}

#[tokio::test]
async fn exit_session_without_token_errors() {
    let state = test_state().await;
    let task_id = create_running_task_with_window(&state).await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "exit_session", "arguments": { "task_id": task_id.0, "action": "done" } })),
    )
    .await;

    assert_error(&resp, "wrap_up first");
}

#[tokio::test]
async fn exit_session_with_wrong_token_errors() {
    let state = test_state().await;
    let task_id = create_running_task_with_window(&state).await;

    state.exit_tokens.write().unwrap().insert(
        task_id,
        crate::mcp::ExitToken {
            token: "correct-token".to_string(),
            action: crate::mcp::handlers::tasks::WrapUpAction::Done,
        },
    );

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "exit_session",
            "arguments": { "task_id": task_id.0, "token": "wrong-token", "action": "done" }
        })),
    )
    .await;

    assert_error(&resp, "invalid exit token");
}

#[tokio::test]
async fn wrap_up_rebase_does_not_kill_window() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Rebase Task",
            description: "description",
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

    // Set up worktree + tmux_window so is_wrappable passes.
    let patch = crate::db::TaskPatch::new()
        .worktree(Some("/repo/.worktrees/task-rebase"))
        .tmux_window(Some("task-rebase-window"));
    state.db_write().patch_task(task_id, &patch).await.unwrap();

    // wrap_up calls finish_task which runs git commands. With MockProcessRunner
    // the git operations will fail, but the key assertion holds for BOTH paths:
    // neither the success path nor the error path should clear tmux_window
    // after this change.
    call(
        &state,
        "tools/call",
        Some(json!({
            "name": "wrap_up",
            "arguments": { "task_id": task_id.0, "action": "rebase" }
        })),
    )
    .await;

    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    // tmux_window must NOT be cleared — exit_session owns the window kill.
    assert!(
        task.tmux_window.is_some(),
        "wrap_up(rebase) must not clear tmux_window — exit_session is responsible"
    );
}

// -- exit_session: Done transition (added with the wrap_up/exit_session alignment) ---

#[tokio::test]
async fn exit_session_marks_task_done_with_no_epic() {
    // No-epic branch: the closing call must mark the task Done even when
    // there is no epic to recalculate. Pins the `is_some()` guard around
    // recalculate_epic_status.
    let state = test_state().await;
    let task_id = create_running_task_with_window(&state).await;
    state.exit_tokens.write().unwrap().insert(
        task_id,
        crate::mcp::ExitToken {
            token: "close-tok".to_string(),
            action: crate::mcp::handlers::tasks::WrapUpAction::Done,
        },
    );

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "exit_session",
            "arguments": { "task_id": task_id.0, "token": "close-tok", "action": "done" }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "{:?}", resp.error);

    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Done);
    assert_eq!(task.sub_status, SubStatus::default_for(TaskStatus::Done));
    assert!(task.tmux_window.is_none());
    assert!(task.epic_id.is_none(), "fixture should have no epic");
}

#[tokio::test]
async fn exit_session_already_done_task_stays_done() {
    // Idempotency: a task that is somehow already Done before exit_session
    // closes must remain Done after the closing call.
    let state = test_state().await;
    let task_id = create_running_task_with_window(&state).await;
    state
        .db_write()
        .patch_task(
            task_id,
            &crate::db::TaskPatch::new().status(TaskStatus::Done),
        )
        .await
        .unwrap();
    state.exit_tokens.write().unwrap().insert(
        task_id,
        crate::mcp::ExitToken {
            token: "close-done-tok".to_string(),
            action: crate::mcp::handlers::tasks::WrapUpAction::Done,
        },
    );

    call(
        &state,
        "tools/call",
        Some(json!({
            "name": "exit_session",
            "arguments": { "task_id": task_id.0, "token": "close-done-tok", "action": "done" }
        })),
    )
    .await;

    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Done);
    assert!(task.tmux_window.is_none());
}

#[tokio::test]
async fn exit_session_recalculates_epic_status() {
    let state = test_state().await;
    let epic = state.db_write().create_epic("E", "", None).await.unwrap();
    let task_id = create_running_task_with_window(&state).await;
    state
        .db_write()
        .set_task_epic_id(task_id, Some(epic.id))
        .await
        .unwrap();
    state
        .db_write()
        .recalculate_epic_status(epic.id)
        .await
        .unwrap();
    let epic_before = state.db.get_epic(epic.id).await.unwrap().unwrap();
    assert_ne!(
        epic_before.status,
        TaskStatus::Done,
        "precondition: epic should be in-progress before exit_session"
    );
    state.exit_tokens.write().unwrap().insert(
        task_id,
        crate::mcp::ExitToken {
            token: "epic-close-tok".to_string(),
            action: crate::mcp::handlers::tasks::WrapUpAction::Done,
        },
    );

    call(
        &state,
        "tools/call",
        Some(json!({
            "name": "exit_session",
            "arguments": { "task_id": task_id.0, "token": "epic-close-tok", "action": "done" }
        })),
    )
    .await;

    let epic_after = state.db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(
        epic_after.status,
        TaskStatus::Done,
        "epic should auto-advance to Done once its only subtask is Done"
    );
}

#[tokio::test]
async fn exit_session_resets_sub_status_to_default_for_done() {
    // A task that carries a non-default sub_status (e.g. Stale) into the
    // closing call must have it reset to the Done default.
    let state = test_state().await;
    let task_id = create_running_task_with_window(&state).await;
    state
        .db_write()
        .patch_task(
            task_id,
            &crate::db::TaskPatch::new().sub_status(SubStatus::Stale),
        )
        .await
        .unwrap();
    state.exit_tokens.write().unwrap().insert(
        task_id,
        crate::mcp::ExitToken {
            token: "sub-close-tok".to_string(),
            action: crate::mcp::handlers::tasks::WrapUpAction::Done,
        },
    );

    call(
        &state,
        "tools/call",
        Some(json!({
            "name": "exit_session",
            "arguments": { "task_id": task_id.0, "token": "sub-close-tok", "action": "done" }
        })),
    )
    .await;

    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Done);
    assert_eq!(
        task.sub_status,
        SubStatus::default_for(TaskStatus::Done),
        "closing exit_session must reset sub_status to default_for(Done)"
    );
}

#[tokio::test]
async fn exit_session_emits_refresh_after_done_patch() {
    // Pin the notify ordering: a Refresh event fires after the closing call
    // commits the Done patch, so the TUI re-renders the task in Done.
    let (state, mut rx) = test_state_with_notify().await;
    let task_id = create_running_task_with_window(&state).await;

    state.exit_tokens.write().unwrap().insert(
        task_id,
        crate::mcp::ExitToken {
            token: "notify-tok".to_string(),
            action: crate::mcp::handlers::tasks::WrapUpAction::Done,
        },
    );
    while rx.try_recv().is_ok() {}

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "exit_session",
            "arguments": { "task_id": task_id.0, "token": "notify-tok", "action": "done" }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "{:?}", resp.error);

    // DB must already be Done by the time the Refresh fires.
    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Done);

    let event = rx
        .try_recv()
        .expect("expected TaskChanged after closing exit_session");
    assert!(
        matches!(event, crate::mcp::McpEvent::TaskChanged(t) if t == task_id),
        "expected TaskChanged({task_id:?}), got {event:?}"
    );
}

#[tokio::test]
async fn wrap_up_then_exit_session_end_to_end() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok_with_stdout(b"main\n"), // git rev-parse --abbrev-ref HEAD
        MockProcessRunner::ok_with_stdout(b""),       // git status --porcelain (clean)
        MockProcessRunner::fail(""),                  // git remote get-url (no remote)
        MockProcessRunner::ok(),                      // git rebase main
        MockProcessRunner::ok(),                      // git merge --ff-only
        // exit_session second call kills the tmux window:
        MockProcessRunner::ok(), // tmux has-session
        MockProcessRunner::ok(), // tmux kill-window
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

    let epic = db.create_epic("E2E Epic", "", None).await.unwrap();
    let task_id = db
        .create_task(CreateTaskRequest {
            title: "E2E Task",
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
    db.set_task_epic_id(task_id, Some(epic.id)).await.unwrap();
    db.patch_task(
        task_id,
        &crate::db::TaskPatch::new()
            .worktree(Some("/repo/.worktrees/e2e"))
            .tmux_window(Some("e2e-window")),
    )
    .await
    .unwrap();
    db.recalculate_epic_status(epic.id).await.unwrap();
    let epic_before = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_ne!(epic_before.status, TaskStatus::Done);

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "wrap_up",
            "arguments": { "task_id": task_id.0, "action": "rebase" }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "{:?}", resp.error);

    let after_wrap_up = db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(
        after_wrap_up.status,
        TaskStatus::Running,
        "after wrap_up: status must still be Running"
    );
    assert!(
        after_wrap_up.tmux_window.is_some(),
        "after wrap_up: tmux_window must be preserved"
    );
    let epic_after_wrap_up = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(
        epic_after_wrap_up.status, epic_before.status,
        "after wrap_up: epic status must not change"
    );

    // Extract the token that wrap_up placed in exit_tokens.
    let token = {
        let map = state.exit_tokens.read().unwrap();
        map.get(&task_id)
            .expect("wrap_up must have inserted an exit token")
            .token
            .clone()
    };

    // Single exit_session call closes the session.
    let close_resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "exit_session",
            "arguments": { "task_id": task_id.0, "token": token, "action": "rebase" }
        })),
    )
    .await;
    assert!(close_resp.error.is_none(), "{:?}", close_resp.error);

    let final_task = db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(final_task.status, TaskStatus::Done);
    assert_eq!(
        final_task.sub_status,
        SubStatus::default_for(TaskStatus::Done)
    );
    assert!(final_task.tmux_window.is_none());

    let final_epic = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(
        final_epic.status,
        TaskStatus::Done,
        "epic auto-advances once its only subtask completes via exit_session"
    );
}

#[tokio::test]
async fn wrap_up_done_defers_done_transition_to_exit_session() {
    use crate::process::MockProcessRunner;
    let runner: Arc<dyn crate::process::ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
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
            title: "Done Task",
            description: "",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: Some(crate::models::WrapUpMode::Done),
            auto_run_plan: false,
        })
        .await
        .unwrap();
    db.patch_task(
        task_id,
        &db::TaskPatch::new()
            .worktree(Some("/repo/.worktrees/1-done-task"))
            .tmux_window(Some("task-1")),
    )
    .await
    .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "wrap_up",
            "arguments": { "task_id": task_id.0, "action": "done" }
        })),
    )
    .await;
    assert!(
        !is_error(&resp),
        "expected success, got: {}",
        error_message(&resp)
    );

    let text = extract_response_text(&resp);
    assert!(
        text.contains("exit_session"),
        "response should instruct to call exit_session, got: {text}"
    );
    assert!(
        text.contains("done"),
        "response should mention done action, got: {text}"
    );

    let task = db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Running,
        "wrap_up(done) must not mark the task Done yet — exit_session does"
    );

    // Closing the session is what actually marks it Done.
    let token = state
        .exit_tokens
        .read()
        .unwrap()
        .get(&task_id)
        .unwrap()
        .token
        .clone();
    call(
        &state,
        "tools/call",
        Some(json!({
            "name": "exit_session",
            "arguments": { "task_id": task_id.0, "token": token, "action": "done" }
        })),
    )
    .await;
    let task = db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Done,
        "task should be Done after exit_session"
    );
}

// ---------------------------------------------------------------------------
// Epic recalc via handler paths (service layer boundary tests)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn wrap_up_done_recalculates_epic_status() {
    // wrap_up(done) on an epic's only running subtask must NOT advance the
    // epic yet (status is deferred to exit_session); the closing call is
    // what auto-advances the epic to Done.
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
    let state = Arc::new(McpState::new(
        McpDeps {
            db: db.clone(),
            runner,
            embedding_service: EmbeddingService::new_test(),
            data_dir: std::env::temp_dir(),
        },
        None,
    ));

    let epic = db.create_epic("E", "", None).await.unwrap();
    let task_id = db
        .create_task(CreateTaskRequest {
            title: "T",
            description: "",
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
    db.set_task_epic_id(task_id, Some(epic.id)).await.unwrap();
    db.patch_task(
        task_id,
        &db::TaskPatch::new()
            .worktree(Some("/repo/.worktrees/1-t"))
            .tmux_window(Some("task-1")),
    )
    .await
    .unwrap();
    db.recalculate_epic_status(epic.id).await.unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "wrap_up", "arguments": { "task_id": task_id.0, "action": "done" } })),
    )
    .await;
    assert!(
        !is_error(&resp),
        "expected success, got: {}",
        error_message(&resp)
    );

    let epic_after_wrap_up = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_ne!(
        epic_after_wrap_up.status,
        TaskStatus::Done,
        "epic must not advance until exit_session closes"
    );

    let token = state
        .exit_tokens
        .read()
        .unwrap()
        .get(&task_id)
        .unwrap()
        .token
        .clone();
    call(
        &state,
        "tools/call",
        Some(json!({
            "name": "exit_session",
            "arguments": { "task_id": task_id.0, "token": token, "action": "done" }
        })),
    )
    .await;

    let epic_after_close = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(
        epic_after_close.status,
        TaskStatus::Done,
        "epic must auto-advance to Done once its only subtask is marked Done via exit_session"
    );
}

#[tokio::test]
async fn wrap_up_pr_recalculates_epic_status() {
    // wrap_up(pr) no longer moves the task; exit_session(action="pr") does,
    // and the epic recalc should run from that closing mutation.
    let state = test_state().await;
    let epic = state.db_write().create_epic("E", "", None).await.unwrap();
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "T",
            description: "",
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
    state
        .db_write()
        .patch_task(
            task_id,
            &db::TaskPatch::new()
                .worktree(Some("/repo/.worktrees/1-t"))
                .tmux_window(Some("task-1")),
        )
        .await
        .unwrap();
    state
        .db_write()
        .recalculate_epic_status(epic.id)
        .await
        .unwrap();
    let epic_before = state.db.get_epic(epic.id).await.unwrap().unwrap();
    assert_ne!(epic_before.status, TaskStatus::Done, "precondition");

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "wrap_up", "arguments": { "task_id": task_id.0, "action": "pr" } })),
    )
    .await;
    assert!(
        !is_error(&resp),
        "expected success, got: {}",
        error_message(&resp)
    );

    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Running,
        "wrap_up(pr) must not move the task to review yet"
    );

    let token = state
        .exit_tokens
        .read()
        .unwrap()
        .get(&task_id)
        .unwrap()
        .token
        .clone();
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "exit_session",
            "arguments": {
                "task_id": task_id.0,
                "token": token,
                "action": "pr",
                "pr_url": "https://github.com/owner/repo/pull/99"
            }
        })),
    )
    .await;
    assert!(
        !is_error(&resp),
        "expected success, got: {}",
        error_message(&resp)
    );

    // Epic should still not be Done (task is Review, not Done)
    // but the recalc ran without panicking and the task is now Review.
    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Review);
}

#[tokio::test]
async fn dispatch_task_recalculates_epic_status() {
    // dispatch_task on an epic's backlog task must trigger epic recalc.
    let dir = tempfile::TempDir::new().unwrap();
    let repo_path = dir.path().to_str().unwrap().to_string();
    std::fs::create_dir_all(dir.path().join(".worktrees")).unwrap();

    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::ok(),                    // git fetch origin main
        MockProcessRunner::ok(),                    // tmux new-window
        MockProcessRunner::ok(),                    // tmux set-option @dispatch_dir
        MockProcessRunner::ok(),                    // tmux set-hook
        MockProcessRunner::ok(),                    // tmux send-keys -l
        MockProcessRunner::ok(),                    // tmux send-keys Enter
        MockProcessRunner::ok_with_stdout(b"%9\n"), // tmux split-window (agent-tree)
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

    let epic = db.create_epic("E", "", None).await.unwrap();
    let task_id = db
        .create_task(CreateTaskRequest {
            title: "Dispatch Me",
            description: "d",
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
    db.set_task_epic_id(task_id, Some(epic.id)).await.unwrap();
    db.recalculate_epic_status(epic.id).await.unwrap();

    std::fs::create_dir_all(
        dir.path()
            .join(".worktrees")
            .join(format!("{}-dispatch-me", task_id.0)),
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
    assert!(
        !is_error(&resp),
        "expected success, got: {}",
        error_message(&resp)
    );

    // Task should now be Running and epic recalculated (still not Done — task just started)
    let task = db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    // Epic recalc ran — epic should still be in backlog/non-done (task is running, not done)
    let epic_after = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_ne!(
        epic_after.status,
        TaskStatus::Done,
        "epic with a running task should not be Done"
    );
}

#[tokio::test]
async fn wrap_up_tool_schema_includes_done_action() {
    let state = test_state().await;
    let resp = call(&state, "tools/list", None).await;
    let tools = resp.result.as_ref().unwrap()["tools"].as_array().unwrap();
    let wrap_up = tools
        .iter()
        .find(|t| t["name"] == "wrap_up")
        .expect("wrap_up not in tool list");
    let action_enum = wrap_up["inputSchema"]["properties"]["action"]["enum"]
        .as_array()
        .expect("action should have an enum");
    let values: Vec<&str> = action_enum.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        values.contains(&"done"),
        "wrap_up action enum should include 'done', got: {values:?}"
    );
    assert!(
        values.contains(&"rebase"),
        "wrap_up action enum should include 'rebase', got: {values:?}"
    );
    assert!(
        values.contains(&"pr"),
        "wrap_up action enum should include 'pr', got: {values:?}"
    );
}

#[tokio::test]
async fn wrap_up_tool_schema_no_longer_has_pr_url() {
    // pr_url moved from wrap_up to exit_session.
    let state = test_state().await;
    let resp = call(&state, "tools/list", None).await;
    let tools = resp.result.as_ref().unwrap()["tools"].as_array().unwrap();
    let wrap_up = tools
        .iter()
        .find(|t| t["name"] == "wrap_up")
        .expect("wrap_up not in tool list");
    let properties = wrap_up["inputSchema"]["properties"]
        .as_object()
        .expect("wrap_up should have properties");
    assert!(
        !properties.contains_key("pr_url"),
        "wrap_up schema should no longer have pr_url, got: {properties:?}"
    );
}

#[tokio::test]
async fn exit_session_tool_schema_requires_action_and_has_pr_url() {
    let state = test_state().await;
    let resp = call(&state, "tools/list", None).await;
    let tools = resp.result.as_ref().unwrap()["tools"].as_array().unwrap();
    let exit_session = tools
        .iter()
        .find(|t| t["name"] == "exit_session")
        .expect("exit_session not in tool list");
    let required: Vec<&str> = exit_session["inputSchema"]["required"]
        .as_array()
        .expect("exit_session should have a required list")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        required.contains(&"action"),
        "exit_session schema should require 'action', got: {required:?}"
    );
    let action_enum = exit_session["inputSchema"]["properties"]["action"]["enum"]
        .as_array()
        .expect("action should have an enum");
    let values: Vec<&str> = action_enum.iter().filter_map(|v| v.as_str()).collect();
    assert!(values.contains(&"rebase"));
    assert!(values.contains(&"done"));
    assert!(values.contains(&"pr"));
    assert!(
        exit_session["inputSchema"]["properties"]
            .as_object()
            .expect("exit_session should have properties")
            .contains_key("pr_url"),
        "exit_session schema should have a pr_url property"
    );
}
