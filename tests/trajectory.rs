#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Integration tests for MCP trajectory persistence.

mod common;

use serde_json::{json, Value};

use dispatch_tui::db::CreateTaskRequest;
use dispatch_tui::mcp::identity::{HEADER_KIND, HEADER_TASK_ID};
use dispatch_tui::mcp::trajectory::TRAJECTORIES_SUBDIR;
use dispatch_tui::mcp::BackgroundWrite;
use dispatch_tui::models::{TaskId, TaskStatus};

/// Happy path: a task-identity call writes one JSONL entry to
/// `<data_dir>/trajectories/<task_id>.jsonl`.
#[tokio::test]
async fn task_identity_writes_trajectory_entry() {
    let data_dir = tempfile::tempdir().unwrap();
    let (router, db, mut bg_done) = common::test_router_with_bg_done(data_dir.path()).await;

    let task_id: TaskId = db
        .create_task(CreateTaskRequest {
            title: "trajectory-test-task",
            description: "",
            repo_path: "/r",
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

    let _resp = common::post_mcp(
        router,
        &[(HEADER_TASK_ID, &task_id.0.to_string())],
        json!({
            "jsonrpc": "2.0", "id": 1,
            "method": "tools/call",
            "params": { "name": "list_tasks", "arguments": {} }
        }),
    )
    .await;

    // Await the fire-and-forget trajectory write deterministically.
    common::await_bg_write(&mut bg_done, BackgroundWrite::Trajectory).await;

    let trajectory_path = data_dir
        .path()
        .join(TRAJECTORIES_SUBDIR)
        .join(format!("{}.jsonl", task_id.0));
    assert!(
        trajectory_path.exists(),
        "trajectory file should exist at {}",
        trajectory_path.display()
    );

    let content = tokio::fs::read_to_string(&trajectory_path).await.unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "expected exactly 1 trajectory line, got: {lines:?}"
    );

    let parsed: Value = serde_json::from_str(lines[0]).expect("trajectory line must be valid JSON");
    assert_eq!(
        parsed["task_id"], task_id.0,
        "task_id field mismatch: {parsed}"
    );
    assert_eq!(
        parsed["method"].as_str(),
        Some("list_tasks"),
        "method field mismatch: {parsed}"
    );
    assert_eq!(
        parsed["schema_version"].as_str(),
        Some("1.0.0"),
        "schema_version field mismatch: {parsed}"
    );
    assert!(
        parsed["duration_ms"].is_number(),
        "duration_ms must be a number: {parsed}"
    );
    assert!(
        parsed["timestamp"].is_string(),
        "timestamp must be a string: {parsed}"
    );
}

/// A task-identity call where the task has NO worktree must still write
/// a trajectory entry — the worktree field is no longer a gate.
#[tokio::test]
async fn task_identity_without_worktree_still_writes_trajectory() {
    let data_dir = tempfile::tempdir().unwrap();
    let (router, db, mut bg_done) = common::test_router_with_bg_done(data_dir.path()).await;

    let task_id: TaskId = db
        .create_task(CreateTaskRequest {
            title: "no-worktree-task",
            description: "",
            repo_path: "/r",
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

    let resp = common::post_mcp(
        router,
        &[(HEADER_TASK_ID, &task_id.0.to_string())],
        json!({
            "jsonrpc": "2.0", "id": 1,
            "method": "tools/call",
            "params": { "name": "list_tasks", "arguments": {} }
        }),
    )
    .await;

    common::await_bg_write(&mut bg_done, BackgroundWrite::Trajectory).await;

    assert!(
        resp.get("error").is_none(),
        "expected no protocol error for task-identity call without worktree, got: {resp}"
    );

    let trajectory_path = data_dir
        .path()
        .join(TRAJECTORIES_SUBDIR)
        .join(format!("{}.jsonl", task_id.0));
    assert!(
        trajectory_path.exists(),
        "trajectory file should be written even when task has no worktree"
    );
}

/// A session-identity call should never write a trajectory file.
#[tokio::test]
async fn session_identity_writes_no_trajectory_file() {
    let data_dir = tempfile::tempdir().unwrap();
    let (router, _db, mut bg_done) = common::test_router_with_bg_done(data_dir.path()).await;

    let _resp = common::post_mcp(
        router,
        &[(HEADER_KIND, "session")],
        json!({
            "jsonrpc": "2.0", "id": 1,
            "method": "tools/call",
            "params": { "name": "list_epics", "arguments": {} }
        }),
    )
    .await;

    // A session call records usage but never spawns a trajectory write. Awaiting
    // the usage write is a deterministic barrier proving the handler's
    // background section ran; the trajectory dir must still be absent.
    common::await_bg_write(&mut bg_done, BackgroundWrite::Usage).await;

    let trajectories_dir = data_dir.path().join(TRAJECTORIES_SUBDIR);
    assert!(
        !trajectories_dir.exists(),
        "trajectories/ must NOT be created for session-identity calls"
    );
}
