use std::sync::Arc;

use axum::{Json, extract::State};
use serde_json::{Value, json};

use crate::db::{self, Database};
use crate::models::TaskStatus;
use crate::mcp::McpState;
use crate::process::{ProcessRunner, MockProcessRunner};

use super::dispatch::{handle_mcp, tool_definitions};
use super::tasks::{UpdateTaskArgs, GetTaskArgs, CreateTaskWithEpicArgs, ListTasksArgs, ClaimTaskArgs, WrapUpArgs, ReportUsageArgs};
use super::epics::{CreateEpicArgs, GetEpicArgs, UpdateEpicArgs};
use super::types::{JsonRpcRequest, JsonRpcResponse};

fn test_state() -> Arc<McpState> {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
    Arc::new(McpState { db, notify_tx: None, runner })
}

async fn call(state: &Arc<McpState>, method: &str, params: Option<Value>) -> JsonRpcResponse {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(1)),
        method: method.to_string(),
        params,
    };
    let (_, Json(response)) = handle_mcp(State(state.clone()), Json(req)).await;
    response
}

// -- Shared helpers --------------------------------------------------------

/// Create a task with sensible defaults, returning the TaskId.
fn create_task_fixture(state: &Arc<McpState>) -> crate::models::TaskId {
    state
        .db
        .create_task("Test Task", "test description", "/repo", None, TaskStatus::Backlog)
        .unwrap()
}

/// Assert response is an error whose message contains `substr`.
fn assert_error(resp: &JsonRpcResponse, substr: &str) {
    let err = resp.error.as_ref().unwrap_or_else(|| {
        panic!("expected error containing {substr:?}, got success: {:?}", resp.result)
    });
    assert!(
        err.message.contains(substr),
        "expected error containing {substr:?}, got: {:?}",
        err.message,
    );
}

/// Extract the text content from a successful MCP response.
fn extract_response_text(resp: &JsonRpcResponse) -> String {
    let result = resp.result.as_ref().unwrap_or_else(|| {
        panic!("expected success, got error: {:?}", resp.error)
    });
    result["content"][0]["text"]
        .as_str()
        .expect("missing text in response content")
        .to_string()
}

#[tokio::test]
async fn initialize_returns_capabilities() {
    let state = test_state();
    let resp = call(&state, "initialize", None).await;
    let result = resp.result.unwrap();
    assert_eq!(result["protocolVersion"], "2024-11-05");
    assert!(result["capabilities"]["tools"].is_object());
}

#[tokio::test]
async fn tools_list_returns_tools() {
    let state = test_state();
    let resp = call(&state, "tools/list", None).await;
    let result = resp.result.unwrap();
    let tools = result["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"update_task"));
    assert!(names.contains(&"get_task"));
    assert!(names.contains(&"create_task"));
    assert!(names.contains(&"list_tasks"));
    assert!(names.contains(&"claim_task"));
    assert!(names.contains(&"wrap_up"));
}

#[tokio::test]
async fn update_task_valid() {
    let state = test_state();
    let task_id = create_task_fixture(&state);

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "status": "running" }
        })),
    ).await;
    assert!(resp.result.is_some());
    assert!(resp.error.is_none());

    let task = state.db.get_task(task_id).unwrap().unwrap();
    assert_eq!(task.status, crate::models::TaskStatus::Running);
}

#[tokio::test]
async fn update_task_invalid_status() {
    let state = test_state();
    let task_id = create_task_fixture(&state);

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "status": "bogus" }
        })),
    ).await;
    assert_error(&resp, "Unknown status");
}

#[tokio::test]
async fn update_task_rejects_done_status() {
    let state = test_state();
    let task_id = create_task_fixture(&state);

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "status": "done" }
        })),
    ).await;
    assert_error(&resp, "Cannot set status to done or archived via MCP");

    // Verify task status unchanged
    let task = state.db.get_task(task_id).unwrap().unwrap();
    assert_ne!(task.status, crate::models::TaskStatus::Done);

    // Also verify archived is rejected
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "status": "archived" }
        })),
    ).await;
    assert_error(&resp, "Cannot set status to done or archived via MCP");
}

#[tokio::test]
async fn update_task_still_allows_other_statuses() {
    let state = test_state();
    let task_id = create_task_fixture(&state);

    for status in &["running", "review", "ready", "backlog"] {
        let resp = call(
            &state,
            "tools/call",
            Some(json!({
                "name": "update_task",
                "arguments": { "task_id": task_id.0, "status": status }
            })),
        ).await;
        assert!(resp.error.is_none(), "status={status} should be allowed, got: {:?}", resp.error);
    }
}

#[tokio::test]
async fn update_task_missing_args() {
    let state = test_state();
    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "update_task", "arguments": {} })),
    ).await;
    assert!(resp.error.is_some());
}

#[tokio::test]
async fn get_task_found() {
    let state = test_state();
    let task_id = state.db.create_task("My Task", "desc", "/repo", None, crate::models::TaskStatus::Backlog).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "get_task",
            "arguments": { "task_id": task_id.0 }
        })),
    ).await;
    let result = resp.result.unwrap();
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("My Task"));
}

#[tokio::test]
async fn get_task_not_found() {
    let state = test_state();
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "get_task",
            "arguments": { "task_id": 9999 }
        })),
    ).await;
    assert!(resp.error.is_some());
    assert!(resp.error.unwrap().message.contains("not found"));
}

#[tokio::test]
async fn unknown_tool() {
    let state = test_state();
    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "bogus_tool", "arguments": {} })),
    ).await;
    assert!(resp.error.is_some());
    assert!(resp.error.unwrap().message.contains("Unknown tool"));
}

#[tokio::test]
async fn unknown_method() {
    let state = test_state();
    let resp = call(&state, "bogus/method", None).await;
    assert!(resp.error.is_some());
    assert!(resp.error.unwrap().message.contains("Method not found"));
}

#[tokio::test]
async fn create_task_minimal() {
    let state = test_state();
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_task",
            "arguments": { "title": "New Task", "repo_path": "/my/repo" }
        })),
    ).await;
    assert!(resp.error.is_none());
    let result = resp.result.unwrap();
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("created"));

    // Verify task was created in DB
    let tasks = state.db.list_all().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].title, "New Task");
    assert_eq!(tasks[0].status, TaskStatus::Backlog);
    assert!(tasks[0].plan.is_none());
}

#[tokio::test]
async fn create_task_with_plan_stays_backlog() {
    let dir = tempfile::tempdir().unwrap();
    let plan_file = dir.path().join("plan.md");
    std::fs::write(&plan_file, "# Plan").unwrap();

    let state = test_state();
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_task",
            "arguments": {
                "title": "Planned Task",
                "repo_path": "/my/repo",
                "plan": plan_file.to_string_lossy()
            }
        })),
    ).await;
    assert!(resp.error.is_none());

    let tasks = state.db.list_all().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].status, TaskStatus::Backlog);
    let stored = tasks[0].plan.as_deref().unwrap();
    assert!(std::path::Path::new(stored).is_absolute(), "plan path should be absolute, got: {stored}");
    assert_eq!(stored, std::fs::canonicalize(&plan_file).unwrap().to_string_lossy());
}

#[tokio::test]
async fn create_task_with_description() {
    let state = test_state();
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_task",
            "arguments": {
                "title": "Described Task",
                "repo_path": "/repo",
                "description": "Some details"
            }
        })),
    ).await;
    assert!(resp.error.is_none());

    let tasks = state.db.list_all().unwrap();
    assert_eq!(tasks[0].description, "Some details");
}

#[tokio::test]
async fn create_task_missing_title() {
    let state = test_state();
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_task",
            "arguments": { "repo_path": "/repo" }
        })),
    ).await;
    assert!(resp.error.is_some());
}

// -- String task_id coercion (Claude Code sends integers as strings) ------

#[tokio::test]
async fn update_task_accepts_string_task_id() {
    let state = test_state();
    let task_id = state.db.create_task("Test", "desc", "/repo", None, crate::models::TaskStatus::Backlog).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0.to_string(), "status": "running" }
        })),
    ).await;
    assert!(resp.error.is_none(), "update_task should accept string task_id, got: {:?}", resp.error);

    let task = state.db.get_task(task_id).unwrap().unwrap();
    assert_eq!(task.status, crate::models::TaskStatus::Running);
}

#[tokio::test]
async fn get_task_accepts_string_task_id() {
    let state = test_state();
    let task_id = state.db.create_task("My Task", "desc", "/repo", None, crate::models::TaskStatus::Backlog).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "get_task",
            "arguments": { "task_id": task_id.0.to_string() }
        })),
    ).await;
    assert!(resp.error.is_none(), "get_task should accept string task_id, got: {:?}", resp.error);
    let result = resp.result.unwrap();
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("My Task"));
}

#[tokio::test]
async fn update_task_with_plan() {
    let state = test_state();
    let task_id = state.db.create_task("Test", "desc", "/repo", None, crate::models::TaskStatus::Backlog).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "status": "ready", "plan": "/path/to/plan.md" }
        })),
    ).await;
    assert!(resp.error.is_none());

    let task = state.db.get_task(task_id).unwrap().unwrap();
    assert_eq!(task.status, crate::models::TaskStatus::Backlog);
    assert_eq!(task.plan.as_deref(), Some("/path/to/plan.md"));
}

#[tokio::test]
async fn update_task_title_only() {
    let state = test_state();
    let task_id = state.db.create_task("Old", "desc", "/repo", None, crate::models::TaskStatus::Backlog).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "title": "New Title" }
        })),
    ).await;
    assert!(resp.error.is_none(), "should succeed with title only: {:?}", resp.error);

    let task = state.db.get_task(task_id).unwrap().unwrap();
    assert_eq!(task.title, "New Title");
    assert_eq!(task.status, crate::models::TaskStatus::Backlog); // unchanged
}

#[tokio::test]
async fn update_task_status_optional() {
    let state = test_state();
    let task_id = state.db.create_task("Test", "desc", "/repo", None, crate::models::TaskStatus::Backlog).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "title": "Renamed" }
        })),
    ).await;
    assert!(resp.error.is_none());

    let task = state.db.get_task(task_id).unwrap().unwrap();
    assert_eq!(task.title, "Renamed");
    assert_eq!(task.status, crate::models::TaskStatus::Backlog);
}

#[tokio::test]
async fn update_task_title_and_description() {
    let state = test_state();
    let task_id = state.db.create_task("Old", "old desc", "/repo", None, crate::models::TaskStatus::Backlog).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "title": "New", "description": "new desc" }
        })),
    ).await;
    assert!(resp.error.is_none());

    let task = state.db.get_task(task_id).unwrap().unwrap();
    assert_eq!(task.title, "New");
    assert_eq!(task.description, "new desc");
}

#[tokio::test]
async fn update_task_repo_path() {
    let state = test_state();
    let task_id = state.db.create_task("Test", "desc", "/old/repo", None, crate::models::TaskStatus::Backlog).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "repo_path": "/new/repo" }
        })),
    ).await;
    assert!(resp.error.is_none(), "should succeed with repo_path only: {:?}", resp.error);

    let task = state.db.get_task(task_id).unwrap().unwrap();
    assert_eq!(task.repo_path, "/new/repo");
    assert_eq!(task.status, crate::models::TaskStatus::Backlog); // unchanged
}

#[tokio::test]
async fn update_task_no_fields_errors() {
    let state = test_state();
    let task_id = state.db.create_task("Test", "desc", "/repo", None, crate::models::TaskStatus::Backlog).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0 }
        })),
    ).await;
    assert!(resp.error.is_some(), "should error with no fields to update");
}

#[tokio::test]
async fn patch_task_sets_multiple_fields() {
    let state = test_state();
    let task_id = state.db.create_task("Test", "Desc", "/repo", None, TaskStatus::Backlog).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": {
                "task_id": task_id.0,
                "status": "ready",
                "title": "Updated Title"
            }
        })),
    ).await;
    assert!(resp.error.is_none());

    let task = state.db.get_task(task_id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Backlog);
    assert_eq!(task.title, "Updated Title");
}

#[tokio::test]
async fn update_task_without_plan_preserves_existing() {
    let state = test_state();
    let task_id = state.db.create_task("Test", "desc", "/repo", Some("/existing.md"), crate::models::TaskStatus::Backlog).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "status": "ready" }
        })),
    ).await;
    assert!(resp.error.is_none());

    let task = state.db.get_task(task_id).unwrap().unwrap();
    assert_eq!(task.plan.as_deref(), Some("/existing.md"), "plan should be preserved when not provided");
}

#[tokio::test]
async fn update_task_sets_pr_fields() {
    let state = test_state();
    let task_id = state.db.create_task("PR test", "desc", "/repo", None, TaskStatus::Backlog).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": {
                "task_id": task_id.0,
                "pr_url": "https://github.com/org/repo/pull/99"
            }
        })),
    ).await;
    assert!(resp.error.is_none(), "Expected success, got: {:?}", resp.error);

    let updated = state.db.get_task(task_id).unwrap().unwrap();
    assert_eq!(updated.pr_url.as_deref(), Some("https://github.com/org/repo/pull/99"));
}

// -- list_tasks tests -------------------------------------------------------

#[tokio::test]
async fn list_tasks_returns_all_when_no_filter() {
    let state = test_state();
    state.db.create_task("Task A", "desc a", "/repo", None, TaskStatus::Backlog).unwrap();
    state.db.create_task("Task B", "desc b", "/repo", None, TaskStatus::Running).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_tasks", "arguments": {} })),
    ).await;
    assert!(resp.error.is_none());
    let result = resp.result.unwrap();
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Task A"));
    assert!(text.contains("Task B"));
}

#[tokio::test]
async fn list_tasks_filters_by_single_status() {
    let state = test_state();
    state.db.create_task("Backlog Task", "desc", "/repo", None, TaskStatus::Backlog).unwrap();
    state.db.create_task("Running Task", "desc", "/repo", None, TaskStatus::Running).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_tasks", "arguments": { "status": "backlog" } })),
    ).await;
    assert!(resp.error.is_none());
    let result = resp.result.unwrap();
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Backlog Task"));
    assert!(!text.contains("Running Task"));
}

#[tokio::test]
async fn list_tasks_filters_by_multiple_statuses() {
    let state = test_state();
    state.db.create_task("Backlog Task", "desc", "/repo", None, TaskStatus::Backlog).unwrap();
    state.db.create_task("Running Task", "desc", "/repo", None, TaskStatus::Running).unwrap();
    state.db.create_task("Review Task", "desc", "/repo", None, TaskStatus::Review).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_tasks", "arguments": { "status": ["backlog", "running"] } })),
    ).await;
    assert!(resp.error.is_none());
    let result = resp.result.unwrap();
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Backlog Task"));
    assert!(text.contains("Running Task"));
    assert!(!text.contains("Review Task"));
}

#[tokio::test]
async fn list_tasks_empty_result() {
    let state = test_state();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_tasks", "arguments": { "status": "running" } })),
    ).await;
    assert!(resp.error.is_none());
    let result = resp.result.unwrap();
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("No tasks found"));
}

// -- claim_task tests -------------------------------------------------------

#[tokio::test]
async fn claim_task_success() {
    let state = test_state();
    let task_id = state.db.create_task("Claimable", "desc", "/repo", None, TaskStatus::Backlog).unwrap();

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
    ).await;
    assert!(resp.error.is_none(), "claim should succeed: {:?}", resp.error);

    let task = state.db.get_task(task_id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.worktree.as_deref(), Some("/repo/.worktrees/5-other-task"));
    assert_eq!(task.tmux_window.as_deref(), Some("task-5"));
}

#[tokio::test]
async fn claim_task_rejects_running_task() {
    let state = test_state();
    let task_id = state.db.create_task("Running", "desc", "/repo", None, TaskStatus::Running).unwrap();

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
    ).await;
    assert!(resp.error.is_some());
    assert!(resp.error.unwrap().message.contains("already"));
}

#[tokio::test]
async fn claim_task_rejects_different_repo() {
    let state = test_state();
    let task_id = state.db.create_task("Other Repo", "desc", "/other-repo", None, TaskStatus::Backlog).unwrap();

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
    ).await;
    assert!(resp.error.is_some());
    assert!(resp.error.unwrap().message.contains("repo"));
}

#[tokio::test]
async fn claim_task_not_found() {
    let state = test_state();

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
    ).await;
    assert!(resp.error.is_some());
    assert!(resp.error.unwrap().message.contains("not found"));
}

#[tokio::test]
async fn report_usage_stores_and_accumulates() {
    let state = test_state();
    let task_id = create_task_fixture(&state);

    // First session
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "report_usage",
            "arguments": {
                "task_id": task_id.0,
                "cost_usd": 0.10,
                "input_tokens": 1000,
                "output_tokens": 500
            }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "first call failed: {:?}", resp.error);

    // Second session — should accumulate
    let resp2 = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "report_usage",
            "arguments": {
                "task_id": task_id.0,
                "cost_usd": 0.05,
                "input_tokens": 500,
                "output_tokens": 250,
                "cache_read_tokens": 100,
                "cache_write_tokens": 50
            }
        })),
    )
    .await;
    assert!(resp2.error.is_none(), "second call failed: {:?}", resp2.error);

    let all = state.db.get_all_usage().unwrap();
    assert_eq!(all.len(), 1);
    let u = &all[0];
    assert_eq!(u.task_id, task_id);
    assert!((u.cost_usd - 0.15).abs() < 1e-9);
    assert_eq!(u.input_tokens, 1_500);
    assert_eq!(u.output_tokens, 750);
    assert_eq!(u.cache_read_tokens, 100);
    assert_eq!(u.cache_write_tokens, 50);
}

#[tokio::test]
async fn report_usage_unknown_task_returns_error() {
    let state = test_state();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "report_usage",
            "arguments": {
                "task_id": 9999,
                "cost_usd": 0.10,
                "input_tokens": 1000,
                "output_tokens": 500
            }
        })),
    )
    .await;
    assert_error(&resp, "not found");
}

/// Validates that JSON schemas in `tool_definitions()` match the argument structs.
/// Catches drift when a field is added/removed from a struct but not the schema
/// (or vice versa). The `expected_props` lists must be kept in sync with struct
/// fields — if you add a field to a struct, add it here too; the test then fails
/// if the schema wasn't also updated.
#[test]
fn tool_schemas_match_arg_structs() {
    use std::collections::BTreeSet;

    let defs = tool_definitions();
    let tools_arr = defs["tools"].as_array().unwrap();
    let tools: std::collections::HashMap<&str, &Value> = tools_arr
        .iter()
        .map(|t| (t["name"].as_str().unwrap(), t))
        .collect();

    fn schema_props<'a>(tool: &'a Value) -> (BTreeSet<&'a str>, BTreeSet<&'a str>) {
        let schema = &tool["inputSchema"];
        let props: BTreeSet<&str> = schema["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.as_str())
            .collect();
        let required: BTreeSet<&str> = schema
            .get("required")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().map(|v| v.as_str().unwrap()).collect())
            .unwrap_or_default();
        (props, required)
    }

    // (tool_name, expected_properties, expected_required, full_payload)
    // expected_properties MUST match the struct fields — update when structs change.
    let cases: Vec<(&str, BTreeSet<&str>, BTreeSet<&str>, Value)> = vec![
        (
            "update_task",
            BTreeSet::from(["task_id", "status", "plan", "title", "description", "repo_path", "sort_order", "pr_url", "tag", "sub_status"]),
            BTreeSet::from(["task_id"]),
            json!({"task_id": 1, "status": "review", "plan": "/p.md", "title": "t", "description": "d", "repo_path": "/r", "sort_order": 100, "pr_url": "https://github.com/org/repo/pull/1", "tag": "bug", "sub_status": "awaiting_review"}),
        ),
        (
            "get_task",
            BTreeSet::from(["task_id"]),
            BTreeSet::from(["task_id"]),
            json!({"task_id": 1}),
        ),
        (
            "create_task",
            BTreeSet::from(["title", "repo_path", "description", "plan", "epic_id", "sort_order", "tag"]),
            BTreeSet::from(["title", "repo_path"]),
            json!({"title": "t", "repo_path": "/r", "description": "d", "plan": "/p.md", "sort_order": 10, "tag": "feature"}),
        ),
        (
            "list_tasks",
            BTreeSet::from(["status"]),
            BTreeSet::new(),
            json!({"status": "backlog"}),
        ),
        (
            "claim_task",
            BTreeSet::from(["task_id", "worktree", "tmux_window"]),
            BTreeSet::from(["task_id", "worktree", "tmux_window"]),
            json!({"task_id": 1, "worktree": "/w", "tmux_window": "tw"}),
        ),
        (
            "create_epic",
            BTreeSet::from(["title", "repo_path", "description", "sort_order"]),
            BTreeSet::from(["title", "repo_path"]),
            json!({"title": "Epic", "repo_path": "/repo", "sort_order": 5}),
        ),
        (
            "get_epic",
            BTreeSet::from(["epic_id"]),
            BTreeSet::from(["epic_id"]),
            json!({"epic_id": 1}),
        ),
        (
            "list_epics",
            BTreeSet::new(),
            BTreeSet::new(),
            json!({}),
        ),
        (
            "update_epic",
            BTreeSet::from(["epic_id", "title", "description", "done", "plan", "sort_order"]),
            BTreeSet::from(["epic_id"]),
            json!({"epic_id": 1, "plan": "docs/plan.md", "sort_order": 42}),
        ),
        (
            "wrap_up",
            BTreeSet::from(["task_id", "action"]),
            BTreeSet::from(["task_id", "action"]),
            json!({"task_id": 1, "action": "rebase"}),
        ),
        (
            "report_usage",
            BTreeSet::from(["task_id", "cost_usd", "input_tokens", "output_tokens",
                             "cache_read_tokens", "cache_write_tokens"]),
            BTreeSet::from(["task_id", "cost_usd", "input_tokens", "output_tokens"]),
            json!({"task_id": 1, "cost_usd": 0.42, "input_tokens": 1000,
                   "output_tokens": 500, "cache_read_tokens": 100, "cache_write_tokens": 50}),
        ),
    ];

    // Verify we cover exactly the tools that exist
    let expected_names: BTreeSet<&str> = cases.iter().map(|(name, _, _, _)| *name).collect();
    let actual_names: BTreeSet<&str> = tools.keys().copied().collect();
    assert_eq!(actual_names, expected_names, "Tool list mismatch — update this test when adding/removing tools");

    for (name, exp_props, exp_required, payload) in &cases {
        let tool = tools[name];
        let (actual_props, actual_required) = schema_props(tool);

        assert_eq!(
            &actual_props, exp_props,
            "Property mismatch for '{name}'"
        );
        assert_eq!(
            &actual_required, exp_required,
            "Required field mismatch for '{name}'"
        );

        // Verify the full payload deserializes into the struct
        match *name {
            "update_task" => { serde_json::from_value::<UpdateTaskArgs>(payload.clone()).unwrap(); }
            "get_task" => { serde_json::from_value::<GetTaskArgs>(payload.clone()).unwrap(); }
            "create_task" => { serde_json::from_value::<CreateTaskWithEpicArgs>(payload.clone()).unwrap(); }
            "list_tasks" => { serde_json::from_value::<ListTasksArgs>(payload.clone()).unwrap(); }
            "claim_task" => { serde_json::from_value::<ClaimTaskArgs>(payload.clone()).unwrap(); }
            "report_usage" => { serde_json::from_value::<ReportUsageArgs>(payload.clone()).unwrap(); }
            "create_epic" => { serde_json::from_value::<CreateEpicArgs>(payload.clone()).unwrap(); }
            "get_epic" => { serde_json::from_value::<GetEpicArgs>(payload.clone()).unwrap(); }
            "list_epics" => {} // no args
            "update_epic" => { serde_json::from_value::<UpdateEpicArgs>(payload.clone()).unwrap(); }
            "wrap_up" => { serde_json::from_value::<WrapUpArgs>(payload.clone()).unwrap(); }
            other => panic!("No deserialization check for tool: {other}"),
        }
    }
}

#[tokio::test]
async fn claim_task_accepts_string_task_id() {
    let state = test_state();
    let task_id = state.db.create_task("Claimable", "desc", "/repo", None, TaskStatus::Backlog).unwrap();

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
    ).await;
    assert!(resp.error.is_none(), "should accept string task_id: {:?}", resp.error);
}

// =======================================================================
// Epic tool tests
// =======================================================================

#[tokio::test]
async fn create_epic_minimal() {
    let state = test_state();
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_epic",
            "arguments": { "title": "My Epic", "repo_path": "/repo" }
        })),
    ).await;
    let text = extract_response_text(&resp);
    assert!(text.contains("Epic"));
    assert!(text.contains("created"));

    let epics = state.db.list_epics().unwrap();
    assert_eq!(epics.len(), 1);
    assert_eq!(epics[0].title, "My Epic");
    assert_eq!(epics[0].repo_path, "/repo");
}

#[tokio::test]
async fn create_epic_with_all_fields() {
    let state = test_state();
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_epic",
            "arguments": {
                "title": "Full Epic",
                "repo_path": "/repo",
                "description": "Epic desc"
            }
        })),
    ).await;
    assert!(resp.error.is_none(), "{:?}", resp.error);

    let epics = state.db.list_epics().unwrap();
    assert_eq!(epics[0].description, "Epic desc");
}

#[tokio::test]
async fn create_epic_missing_title() {
    let state = test_state();
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_epic",
            "arguments": { "repo_path": "/repo" }
        })),
    ).await;
    assert_error(&resp, "Invalid arguments");
}

#[tokio::test]
async fn create_epic_missing_repo_path() {
    let state = test_state();
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_epic",
            "arguments": { "title": "No Repo" }
        })),
    ).await;
    assert_error(&resp, "Invalid arguments");
}

#[tokio::test]
async fn get_epic_found() {
    let state = test_state();
    let epic = state.db.create_epic("Get Me", "desc", "/repo").unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "get_epic",
            "arguments": { "epic_id": epic.id.0 }
        })),
    ).await;
    let text = extract_response_text(&resp);
    assert!(text.contains("Get Me"));
    assert!(text.contains("desc"));
    assert!(text.contains("/repo"));
}

#[tokio::test]
async fn get_epic_not_found() {
    let state = test_state();
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "get_epic",
            "arguments": { "epic_id": 9999 }
        })),
    ).await;
    assert_error(&resp, "not found");
}

#[tokio::test]
async fn get_epic_shows_subtask_summary() {
    let state = test_state();
    let epic = state.db.create_epic("With Tasks", "", "/repo").unwrap();
    let t1 = state.db.create_task("Sub 1", "", "/repo", None, TaskStatus::Done).unwrap();
    let t2 = state.db.create_task("Sub 2", "", "/repo", None, TaskStatus::Backlog).unwrap();
    state.db.set_task_epic_id(t1, Some(epic.id)).unwrap();
    state.db.set_task_epic_id(t2, Some(epic.id)).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "get_epic",
            "arguments": { "epic_id": epic.id.0 }
        })),
    ).await;
    let text = extract_response_text(&resp);
    assert!(text.contains("1/2 done"), "expected subtask summary, got: {text}");
}

#[tokio::test]
async fn get_epic_accepts_string_id() {
    let state = test_state();
    let epic = state.db.create_epic("String ID", "", "/repo").unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "get_epic",
            "arguments": { "epic_id": epic.id.0.to_string() }
        })),
    ).await;
    assert!(resp.error.is_none(), "should accept string epic_id: {:?}", resp.error);
    let text = extract_response_text(&resp);
    assert!(text.contains("String ID"));
}

#[tokio::test]
async fn list_epics_empty() {
    let state = test_state();
    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_epics", "arguments": {} })),
    ).await;
    let text = extract_response_text(&resp);
    assert!(text.contains("No epics found"));
}

#[tokio::test]
async fn list_epics_with_items() {
    let state = test_state();
    state.db.create_epic("Epic A", "desc a", "/repo").unwrap();
    state.db.create_epic("Epic B", "desc b", "/repo").unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_epics", "arguments": {} })),
    ).await;
    let text = extract_response_text(&resp);
    assert!(text.contains("Epic A"));
    assert!(text.contains("Epic B"));
}

#[tokio::test]
async fn list_epics_shows_subtask_counts() {
    let state = test_state();
    let epic = state.db.create_epic("Tracked", "", "/repo").unwrap();
    let t1 = state.db.create_task("Done", "", "/repo", None, TaskStatus::Done).unwrap();
    let t2 = state.db.create_task("Pending", "", "/repo", None, TaskStatus::Backlog).unwrap();
    state.db.set_task_epic_id(t1, Some(epic.id)).unwrap();
    state.db.set_task_epic_id(t2, Some(epic.id)).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_epics", "arguments": {} })),
    ).await;
    let text = extract_response_text(&resp);
    assert!(text.contains("1/2 done"), "expected subtask counts, got: {text}");
}

#[tokio::test]
async fn update_epic_title() {
    let state = test_state();
    let epic = state.db.create_epic("Old Title", "", "/repo").unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_epic",
            "arguments": { "epic_id": epic.id.0, "title": "New Title" }
        })),
    ).await;
    let text = extract_response_text(&resp);
    assert!(text.contains("updated"));
    assert!(text.contains("title"));

    let updated = state.db.get_epic(epic.id).unwrap().unwrap();
    assert_eq!(updated.title, "New Title");
}

#[tokio::test]
async fn update_epic_mark_done() {
    let state = test_state();
    let epic = state.db.create_epic("To Finish", "", "/repo").unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_epic",
            "arguments": { "epic_id": epic.id.0, "done": true }
        })),
    ).await;
    let text = extract_response_text(&resp);
    assert!(text.contains("done"));

    let updated = state.db.get_epic(epic.id).unwrap().unwrap();
    assert!(updated.done);
}

#[tokio::test]
async fn update_epic_multiple_fields() {
    let state = test_state();
    let epic = state.db.create_epic("Old", "old desc", "/repo").unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_epic",
            "arguments": {
                "epic_id": epic.id.0,
                "title": "New",
                "description": "new desc"
            }
        })),
    ).await;
    assert!(resp.error.is_none());

    let updated = state.db.get_epic(epic.id).unwrap().unwrap();
    assert_eq!(updated.title, "New");
    assert_eq!(updated.description, "new desc");
}

#[tokio::test]
async fn update_epic_accepts_string_id() {
    let state = test_state();
    let epic = state.db.create_epic("Str Epic", "", "/repo").unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_epic",
            "arguments": { "epic_id": epic.id.0.to_string(), "title": "Updated" }
        })),
    ).await;
    assert!(resp.error.is_none(), "should accept string epic_id: {:?}", resp.error);
}

#[tokio::test]
async fn update_epic_plan() {
    let state = test_state();
    let epic = state.db.create_epic("Planned Epic", "", "/repo").unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_epic",
            "arguments": { "epic_id": epic.id.0, "plan": "docs/plans/epic-plan.md" }
        })),
    ).await;
    let text = extract_response_text(&resp);
    assert!(text.contains("plan"), "response should mention plan: {text}");

    let updated = state.db.get_epic(epic.id).unwrap().unwrap();
    assert_eq!(updated.plan.as_deref(), Some("docs/plans/epic-plan.md"));
}

// =======================================================================
// Additional edge case tests
// =======================================================================

#[tokio::test]
async fn list_tasks_invalid_status_string() {
    let state = test_state();
    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_tasks", "arguments": { "status": "bogus" } })),
    ).await;
    assert_error(&resp, "Unknown status");
}

#[tokio::test]
async fn list_tasks_invalid_status_in_array() {
    let state = test_state();
    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_tasks", "arguments": { "status": ["backlog", "bogus"] } })),
    ).await;
    assert_error(&resp, "Invalid status in array");
}

#[tokio::test]
async fn list_tasks_status_as_number_errors() {
    let state = test_state();
    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_tasks", "arguments": { "status": 42 } })),
    ).await;
    assert_error(&resp, "string or array");
}

#[tokio::test]
async fn claim_task_rejects_done_task() {
    let state = test_state();
    let task_id = state.db.create_task("Done", "desc", "/repo", None, TaskStatus::Done).unwrap();

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
    ).await;
    assert_error(&resp, "already");
}

#[tokio::test]
async fn claim_task_rejects_review_task() {
    let state = test_state();
    let task_id = state.db.create_task("Review", "desc", "/repo", None, TaskStatus::Review).unwrap();

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
    ).await;
    assert_error(&resp, "already");
}

#[tokio::test]
async fn claim_task_worktree_without_worktrees_dir() {
    let state = test_state();
    // Task repo is "/repo", worktree path has no /.worktrees/ segment
    // so the full path is used as the repo — should match when equal
    let task_id = state.db.create_task("Direct", "desc", "/repo", None, TaskStatus::Backlog).unwrap();

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
    ).await;
    assert!(resp.error.is_none(), "should match when worktree equals repo: {:?}", resp.error);
}

#[tokio::test]
async fn create_task_with_epic_id() {
    let state = test_state();
    let epic = state.db.create_epic("Parent Epic", "", "/repo").unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_task",
            "arguments": {
                "title": "Epic Child",
                "repo_path": "/repo",
                "epic_id": epic.id.0
            }
        })),
    ).await;
    assert!(resp.error.is_none(), "{:?}", resp.error);

    let subtasks = state.db.list_tasks_for_epic(epic.id).unwrap();
    assert_eq!(subtasks.len(), 1);
    assert_eq!(subtasks[0].title, "Epic Child");
}

#[tokio::test]
async fn create_task_with_string_epic_id() {
    let state = test_state();
    let epic = state.db.create_epic("Parent", "", "/repo").unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_task",
            "arguments": {
                "title": "String Epic Child",
                "repo_path": "/repo",
                "epic_id": epic.id.0.to_string()
            }
        })),
    ).await;
    assert!(resp.error.is_none(), "should accept string epic_id: {:?}", resp.error);

    let subtasks = state.db.list_tasks_for_epic(epic.id).unwrap();
    assert_eq!(subtasks.len(), 1);
}

#[tokio::test]
async fn tools_list_includes_epic_tools() {
    let state = test_state();
    let resp = call(&state, "tools/list", None).await;
    let result = resp.result.unwrap();
    let tools = result["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"create_epic"));
    assert!(names.contains(&"get_epic"));
    assert!(names.contains(&"list_epics"));
    assert!(names.contains(&"update_epic"));
}

#[tokio::test]
async fn update_epic_no_fields_errors() {
    let state = test_state();
    let epic = state.db.create_epic("Test", "", "/repo").unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_epic",
            "arguments": { "epic_id": epic.id.0 }
        })),
    ).await;
    assert_error(&resp, "At least one");
}

#[tokio::test]
async fn claim_task_updates_status_to_running() {
    let state = test_state();
    let task_id = state.db.create_task("Claim", "desc", "/repo", None, TaskStatus::Backlog).unwrap();

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
    ).await;
    assert!(resp.error.is_none(), "{:?}", resp.error);

    let task = state.db.get_task(crate::models::TaskId(task_id.0)).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.worktree.as_deref(), Some("/repo/.worktrees/1-claim"));
    assert_eq!(task.tmux_window.as_deref(), Some("task-1"));
}

// ---------------------------------------------------------------------------
// wrap_up tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn wrap_up_task_not_found() {
    let state = test_state();
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "wrap_up",
            "arguments": { "task_id": 9999, "action": "rebase" }
        })),
    ).await;
    assert_error(&resp, "not found");
}

#[tokio::test]
async fn wrap_up_rejects_backlog_task() {
    let state = test_state();
    let task_id = state.db.create_task("T", "d", "/repo", None, TaskStatus::Backlog).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "wrap_up",
            "arguments": { "task_id": task_id.0, "action": "rebase" }
        })),
    ).await;
    assert_error(&resp, "cannot be wrapped up");
}

#[tokio::test]
async fn wrap_up_accepts_running_blocked_task() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::fail(""),                   // detect_default_branch (symbolic-ref)
        MockProcessRunner::ok_with_stdout(b"main\n"), // git rev-parse --abbrev-ref HEAD
        MockProcessRunner::fail(""),                   // git remote get-url (no remote)
        MockProcessRunner::ok(),                       // git rebase main
        MockProcessRunner::ok(),                       // git merge --ff-only
    ]));
    let state = Arc::new(McpState { db: db.clone(), notify_tx: None, runner });

    let task_id = db.create_task("My Task", "desc", "/repo", None, TaskStatus::Running).unwrap();
    db.patch_task(task_id, &db::TaskPatch::new()
        .worktree(Some("/repo/.worktrees/1-my-task"))
        .sub_status(crate::models::SubStatus::NeedsInput)
    ).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "wrap_up",
            "arguments": { "task_id": task_id.0, "action": "rebase" }
        })),
    ).await;

    let text = extract_response_text(&resp);
    assert!(text.contains("wrap_up complete"), "Expected 'wrap_up complete', got: {text}");
}

#[tokio::test]
async fn wrap_up_accepts_running_active_task() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::fail(""),                   // detect_default_branch (symbolic-ref)
        MockProcessRunner::ok_with_stdout(b"main\n"), // git rev-parse --abbrev-ref HEAD
        MockProcessRunner::fail(""),                   // git remote get-url (no remote)
        MockProcessRunner::ok(),                       // git rebase main
        MockProcessRunner::ok(),                       // git merge --ff-only
    ]));
    let state = Arc::new(McpState { db: db.clone(), notify_tx: None, runner, mcp_port: crate::DEFAULT_PORT });

    let task_id = db.create_task("T", "d", "/repo", None, TaskStatus::Running).unwrap();
    db.patch_task(task_id, &db::TaskPatch::new()
        .worktree(Some("/repo/.worktrees/1-t"))
    ).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "wrap_up",
            "arguments": { "task_id": task_id.0, "action": "rebase" }
        })),
    ).await;
    let text = extract_response_text(&resp);
    assert!(text.contains("wrap_up complete"), "Expected 'wrap_up complete', got: {text}");
}

#[tokio::test]
async fn wrap_up_task_no_worktree() {
    let state = test_state();
    let task_id = state.db.create_task("T", "d", "/repo", None, TaskStatus::Review).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "wrap_up",
            "arguments": { "task_id": task_id.0, "action": "rebase" }
        })),
    ).await;
    assert_error(&resp, "cannot be wrapped up");
}

#[tokio::test]
async fn wrap_up_invalid_action() {
    let state = test_state();
    let task_id = state.db.create_task("T", "d", "/repo", None, TaskStatus::Review).unwrap();
    state.db.patch_task(task_id, &db::TaskPatch::new().worktree(Some("/repo/.worktrees/1-t"))).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "wrap_up",
            "arguments": { "task_id": task_id.0, "action": "teleport" }
        })),
    ).await;
    assert_error(&resp, "Unknown action");
}

#[tokio::test]
async fn wrap_up_rebase_returns_started() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::fail(""),                   // detect_default_branch (symbolic-ref)
        MockProcessRunner::ok_with_stdout(b"main\n"), // git rev-parse --abbrev-ref HEAD
        MockProcessRunner::fail(""),                   // git remote get-url (no remote)
        MockProcessRunner::ok(),                       // git rebase main
        MockProcessRunner::ok(),                       // git merge --ff-only
    ]));
    let state = Arc::new(McpState { db: db.clone(), notify_tx: None, runner });

    let task_id = db.create_task("My Task", "desc", "/repo", None, TaskStatus::Review).unwrap();
    db.patch_task(task_id, &db::TaskPatch::new()
        .worktree(Some("/repo/.worktrees/1-my-task"))
    ).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "wrap_up",
            "arguments": { "task_id": task_id.0, "action": "rebase" }
        })),
    ).await;

    let text = extract_response_text(&resp);
    assert!(text.contains("wrap_up complete"), "Expected 'wrap_up complete', got: {text}");
}

#[tokio::test]
async fn wrap_up_pr_returns_started() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::fail(""),                                          // detect_default_branch (symbolic-ref)
        MockProcessRunner::ok(),                                              // git push
        MockProcessRunner::ok_with_stdout(b"git@github.com:org/repo.git\n"), // git remote get-url
        MockProcessRunner::ok_with_stdout(b"https://github.com/org/repo/pull/7\n"), // gh pr create
    ]));
    let state = Arc::new(McpState { db: db.clone(), notify_tx: None, runner });

    let task_id = db.create_task("My Feature", "desc", "/repo", None, TaskStatus::Review).unwrap();
    db.patch_task(task_id, &db::TaskPatch::new()
        .worktree(Some("/repo/.worktrees/1-my-feature"))
    ).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "wrap_up",
            "arguments": { "task_id": task_id.0, "action": "pr" }
        })),
    ).await;

    let text = extract_response_text(&resp);
    assert!(text.contains("wrap_up complete"), "Expected 'wrap_up complete', got: {text}");
}

// ---------------------------------------------------------------------------
// wrap_up auto-dispatch tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn wrap_up_rebase_no_epic_skips_auto_dispatch() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::fail(""),                   // detect_default_branch (symbolic-ref)
        MockProcessRunner::ok_with_stdout(b"main\n"), // git rev-parse --abbrev-ref HEAD
        MockProcessRunner::fail(""),                   // git remote get-url (no remote)
        MockProcessRunner::ok(),                       // git rebase main
        MockProcessRunner::ok(),                       // git merge --ff-only
        // No auto-dispatch calls expected
    ]));
    let state = Arc::new(McpState { db: db.clone(), notify_tx: None, runner });

    let task_id = db.create_task("Solo Task", "desc", "/repo", None, TaskStatus::Review).unwrap();
    db.patch_task(task_id, &db::TaskPatch::new()
        .worktree(Some("/repo/.worktrees/1-solo-task"))
    ).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "wrap_up",
            "arguments": { "task_id": task_id.0, "action": "rebase" }
        })),
    ).await;

    let text = extract_response_text(&resp);
    assert!(!text.contains("next epic task"), "Non-epic task should not mention auto-dispatch");
}

#[tokio::test]
async fn wrap_up_rebase_auto_dispatches_next_epic_task() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo_path = dir.path().to_str().unwrap().to_string();
    // Create .worktrees directory for the auto-dispatched task
    std::fs::create_dir_all(dir.path().join(".worktrees")).unwrap();

    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![
        // Rebase calls for current task
        MockProcessRunner::fail(""),                   // detect_default_branch (symbolic-ref)
        MockProcessRunner::ok_with_stdout(b"main\n"), // git rev-parse --abbrev-ref HEAD
        MockProcessRunner::fail(""),                   // git remote get-url (no remote)
        MockProcessRunner::ok(),                       // git rebase main
        MockProcessRunner::ok(),                       // git merge --ff-only
        // Auto-dispatch calls for next task (worktree dir pre-created, so git add is skipped)
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-hook
        MockProcessRunner::ok(), // tmux send-keys -l (literal text)
        MockProcessRunner::ok(), // tmux send-keys Enter
    ]));
    let state = Arc::new(McpState { db: db.clone(), notify_tx: None, runner });

    // Create epic and two subtasks
    let epic = db.create_epic("Test Epic", "desc", &repo_path).unwrap();
    let task1_id = db.create_task("Task 1", "first", &repo_path, Some("docs/plan.md"), TaskStatus::Review).unwrap();
    let task2_id = db.create_task("Task 2", "second", &repo_path, Some("docs/plan2.md"), TaskStatus::Backlog).unwrap();
    db.set_task_epic_id(task1_id, Some(epic.id)).unwrap();
    db.set_task_epic_id(task2_id, Some(epic.id)).unwrap();
    db.patch_task(task1_id, &db::TaskPatch::new()
        .worktree(Some(&format!("{repo_path}/.worktrees/{}-task-1", task1_id.0)))
    ).unwrap();
    // Pre-create the worktree directory for the next task (mocked git won't create it)
    std::fs::create_dir_all(dir.path().join(".worktrees").join(format!("{}-task-2", task2_id.0))).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "wrap_up",
            "arguments": { "task_id": task1_id.0, "action": "rebase" }
        })),
    ).await;

    let text = extract_response_text(&resp);
    assert!(text.contains("next epic task"), "Expected auto-dispatch mention, got: {text}");
    assert!(text.contains(&format!("#{}", task2_id.0)), "Expected next task ID in message, got: {text}");

    // Wait for spawn_blocking to complete
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Verify the next task was dispatched
    let task2 = db.get_task(task2_id).unwrap().unwrap();
    assert_eq!(task2.status, TaskStatus::Running, "Next task should be Running after auto-dispatch");
    assert!(task2.worktree.is_some(), "Next task should have a worktree");
    assert!(task2.tmux_window.is_some(), "Next task should have a tmux_window");
}

#[tokio::test]
async fn wrap_up_rebase_no_backlog_tasks_skips_dispatch() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::fail(""),                   // detect_default_branch (symbolic-ref)
        MockProcessRunner::ok_with_stdout(b"main\n"), // git rev-parse --abbrev-ref HEAD
        MockProcessRunner::fail(""),                   // git remote get-url (no remote)
        MockProcessRunner::ok(),                       // git rebase main
        MockProcessRunner::ok(),                       // git merge --ff-only
    ]));
    let state = Arc::new(McpState { db: db.clone(), notify_tx: None, runner });

    // Create epic with only one task (already in review — no backlog subtasks)
    let epic = db.create_epic("Test Epic", "desc", "/repo").unwrap();
    let task1_id = db.create_task("Task 1", "first", "/repo", None, TaskStatus::Review).unwrap();
    db.set_task_epic_id(task1_id, Some(epic.id)).unwrap();
    db.patch_task(task1_id, &db::TaskPatch::new()
        .worktree(Some("/repo/.worktrees/1-task-1"))
    ).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "wrap_up",
            "arguments": { "task_id": task1_id.0, "action": "rebase" }
        })),
    ).await;

    let text = extract_response_text(&resp);
    assert!(!text.contains("next epic task"), "No backlog tasks — should not mention auto-dispatch");
}

// ---------------------------------------------------------------------------
// sub_status tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_task_sets_sub_status() {
    let state = test_state();
    let task_id = state.db.create_task("T", "desc", "/repo", None, TaskStatus::Running).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "sub_status": "needs_input" }
        })),
    ).await;
    assert!(resp.error.is_none(), "expected success: {:?}", resp.error);
    let text = extract_response_text(&resp);
    assert!(text.contains("sub_status"), "response should mention sub_status: {text}");

    let task = state.db.get_task(task_id).unwrap().unwrap();
    assert_eq!(task.sub_status, crate::models::SubStatus::NeedsInput);
}

#[tokio::test]
async fn update_task_rejects_invalid_sub_status_for_status() {
    let state = test_state();
    let task_id = state.db.create_task("T", "desc", "/repo", None, TaskStatus::Running).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "sub_status": "approved" }
        })),
    ).await;
    assert_error(&resp, "not valid for status");
}

#[tokio::test]
async fn update_task_rejects_bogus_sub_status() {
    let state = test_state();
    let task_id = state.db.create_task("T", "desc", "/repo", None, TaskStatus::Running).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "sub_status": "bogus" }
        })),
    ).await;
    assert_error(&resp, "Invalid sub_status");
}

#[tokio::test]
async fn update_task_sub_status_with_status_change() {
    let state = test_state();
    let task_id = state.db.create_task("T", "desc", "/repo", None, TaskStatus::Running).unwrap();

    // Change status to review and set sub_status to approved in one call
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "status": "review", "sub_status": "approved" }
        })),
    ).await;
    assert!(resp.error.is_none(), "expected success: {:?}", resp.error);

    let task = state.db.get_task(task_id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Review);
    assert_eq!(task.sub_status, crate::models::SubStatus::Approved);
}

#[tokio::test]
async fn update_task_sub_status_invalid_for_new_status() {
    let state = test_state();
    let task_id = state.db.create_task("T", "desc", "/repo", None, TaskStatus::Running).unwrap();

    // Change status to review but set sub_status to active (valid for running, not review)
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "status": "review", "sub_status": "active" }
        })),
    ).await;
    assert_error(&resp, "not valid for status");
}

#[tokio::test]
async fn list_tasks_shows_sub_status() {
    let state = test_state();
    let task_id = state.db.create_task("Listed Task", "desc", "/repo", None, TaskStatus::Running).unwrap();
    state.db.patch_task(task_id, &db::TaskPatch::new().sub_status(crate::models::SubStatus::NeedsInput)).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_tasks", "arguments": {} })),
    ).await;
    let text = extract_response_text(&resp);
    assert!(text.contains("running/needs_input"), "expected running/needs_input in list output, got: {text}");
}

#[tokio::test]
async fn get_task_shows_sub_status() {
    let state = test_state();
    let task_id = state.db.create_task("Detail Task", "desc", "/repo", None, TaskStatus::Review).unwrap();
    state.db.patch_task(task_id, &db::TaskPatch::new().sub_status(crate::models::SubStatus::ChangesRequested)).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "get_task",
            "arguments": { "task_id": task_id.0 }
        })),
    ).await;
    let text = extract_response_text(&resp);
    assert!(text.contains("Sub-status: changes_requested"), "expected sub-status in detail, got: {text}");
}
async fn wrap_up_rebase_conflict_returns_error() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::fail(""),                   // detect_default_branch
        MockProcessRunner::ok_with_stdout(b"main\n"), // git rev-parse HEAD
        MockProcessRunner::fail(""),                   // git remote get-url (no remote)
        MockProcessRunner::fail("CONFLICT (content): Merge conflict in foo.rs"), // git rebase
        MockProcessRunner::ok(),                       // git rebase --abort
    ]));
    let state = Arc::new(McpState { db: db.clone(), notify_tx: None, runner });

    let task_id = db.create_task("Conflict Task", "desc", "/repo", None, TaskStatus::Review).unwrap();
    db.patch_task(task_id, &db::TaskPatch::new()
        .worktree(Some("/repo/.worktrees/1-conflict-task"))
    ).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "wrap_up",
            "arguments": { "task_id": task_id.0, "action": "rebase" }
        })),
    ).await;

    assert_error(&resp, "conflict");
    let task = db.get_task(task_id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Review, "Task should remain Review on rebase conflict");
}

#[tokio::test]
async fn wrap_up_rebase_not_on_main_returns_error() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::fail(""),                       // detect_default_branch
        MockProcessRunner::ok_with_stdout(b"feature\n"),   // git rev-parse HEAD (not on main)
    ]));
    let state = Arc::new(McpState { db: db.clone(), notify_tx: None, runner });

    let task_id = db.create_task("Wrong Branch", "desc", "/repo", None, TaskStatus::Review).unwrap();
    db.patch_task(task_id, &db::TaskPatch::new()
        .worktree(Some("/repo/.worktrees/1-wrong-branch"))
    ).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "wrap_up",
            "arguments": { "task_id": task_id.0, "action": "rebase" }
        })),
    ).await;

    assert_error(&resp, "not on main");
    let task = db.get_task(task_id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Review, "Task should remain Review on error");
}

#[tokio::test]
async fn wrap_up_pr_push_fails_returns_error() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![
        MockProcessRunner::fail(""),                        // detect_default_branch
        MockProcessRunner::fail("remote: Permission denied"), // git push fails
    ]));
    let state = Arc::new(McpState { db: db.clone(), notify_tx: None, runner });

    let task_id = db.create_task("Push Fail", "desc", "/repo", None, TaskStatus::Review).unwrap();
    db.patch_task(task_id, &db::TaskPatch::new()
        .worktree(Some("/repo/.worktrees/1-push-fail"))
    ).unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "wrap_up",
            "arguments": { "task_id": task_id.0, "action": "pr" }
        })),
    ).await;

    assert_error(&resp, "wrap_up failed");
    let task = db.get_task(task_id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Review, "Task should remain Review on push failure");
    assert!(task.pr_url.is_none(), "No PR URL should be set on failure");
}
