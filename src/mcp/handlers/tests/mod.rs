#![allow(clippy::unwrap_used, clippy::expect_used)]
mod epics;
mod learnings;
mod managed_feeds;
mod repo_rag;
mod tasks;
mod usage;

use std::sync::Arc;

use axum::{
    body::to_bytes,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Extension, Json,
};
use serde_json::{json, Value};

use tokio::sync::mpsc;

use crate::db::{self, CreateLearningRow, CreateTaskRequest, Database};
use crate::mcp::identity::{CallerIdentity, IdentityError};
use crate::mcp::{BackgroundWrite, McpDeps, McpState};
use crate::models::{SubStatus, TaskStatus};
use crate::process::{MockProcessRunner, ProcessRunner};
use crate::service::embeddings::{serialize_embedding, EmbeddingService};

use super::dispatch::{handle_mcp, tool_definitions};
use super::epics::{CreateEpicArgs, GetEpicArgs, UpdateEpicArgs};
use super::tasks::{
    ClaimTaskArgs, CreateTaskWithEpicArgs, DispatchNextArgs, ExitSessionArgs, GetTaskArgs,
    ListTasksArgs, SendMessageArgs, UpdateTaskArgs, WrapUpArgs,
};
use super::types::{JsonRpcRequest, JsonRpcResponse};

async fn test_state() -> Arc<McpState> {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
    Arc::new(McpState::new(
        McpDeps {
            db,
            runner,
            embedding_service: EmbeddingService::new_test(),
            data_dir: std::env::temp_dir(),
        },
        None,
    ))
}

/// Like [`test_state`], but installs a completion signal that fires after each
/// fire-and-forget background write. Returns the receiver so the test can await
/// the write (e.g. usage recording) deterministically instead of sleeping.
async fn test_state_with_bg_done() -> (Arc<McpState>, mpsc::UnboundedReceiver<BackgroundWrite>) {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
    let (tx, rx) = mpsc::unbounded_channel();
    let mut state = McpState::new(
        McpDeps {
            db,
            runner,
            embedding_service: EmbeddingService::new_test(),
            data_dir: std::env::temp_dir(),
        },
        None,
    );
    state.test_hooks.bg_write_done_tx = Some(tx);
    (Arc::new(state), rx)
}

async fn test_state_with_db() -> (Arc<McpState>, Arc<dyn db::TaskStore>) {
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
    (state, db)
}

async fn call(state: &Arc<McpState>, method: &str, params: Option<Value>) -> JsonRpcResponse {
    call_as(state, method, params, CallerIdentity::Session).await
}

async fn call_as(
    state: &Arc<McpState>,
    method: &str,
    params: Option<Value>,
    identity: CallerIdentity,
) -> JsonRpcResponse {
    call_with_identity(state, method, params, Ok(identity)).await
}

async fn call_with_identity(
    state: &Arc<McpState>,
    method: &str,
    params: Option<Value>,
    identity: Result<CallerIdentity, IdentityError>,
) -> JsonRpcResponse {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(1)),
        method: method.to_string(),
        params,
    };
    let response: Response = handle_mcp(State(state.clone()), Extension(identity), Json(req))
        .await
        .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// Send a JSON-RPC notification (no `id`) and return the raw (status, body) for inspection.
async fn call_notification(
    state: &Arc<McpState>,
    method: &str,
    params: Option<Value>,
) -> (StatusCode, Vec<u8>) {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: None,
        method: method.to_string(),
        params,
    };
    let response: Response = handle_mcp(
        State(state.clone()),
        Extension(Ok(CallerIdentity::Session)),
        Json(req),
    )
    .await
    .into_response();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, bytes.to_vec())
}

// -- Shared helpers --------------------------------------------------------

/// Create a task with sensible defaults, returning the TaskId.
async fn create_task_fixture(state: &Arc<McpState>) -> crate::models::TaskId {
    create_task_fixture_at(state, "/repo").await
}

async fn create_task_fixture_at(state: &Arc<McpState>, repo_path: &str) -> crate::models::TaskId {
    state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Test Task",
            description: "test description",
            repo_path,
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
        .unwrap()
}

/// Create a Running task with worktree and tmux_window set — ready for exit_session.
async fn create_running_task_with_window(state: &Arc<McpState>) -> crate::models::TaskId {
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Running Task",
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
    let patch = crate::db::TaskPatch::new()
        .worktree(Some("/repo/.worktrees/task-123"))
        .tmux_window(Some("task-123"));
    state.db_write().patch_task(task_id, &patch).await.unwrap();
    task_id
}

/// Returns `true` if the response is either a JSON-RPC protocol error or an
/// MCP tool-execution error result (`result.isError == true`).
fn is_error(resp: &JsonRpcResponse) -> bool {
    if resp.error.is_some() {
        return true;
    }
    resp.result
        .as_ref()
        .and_then(|r| r.get("isError"))
        .and_then(Value::as_bool)
        == Some(true)
}

/// Extract the error message from a response — works for protocol errors and
/// for MCP tool-execution error results (`isError: true` with a text content
/// block).
fn error_message(resp: &JsonRpcResponse) -> String {
    if let Some(err) = resp.error.as_ref() {
        return err.message.clone();
    }
    if let Some(result) = resp.result.as_ref() {
        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            return result["content"][0]["text"]
                .as_str()
                .unwrap_or("")
                .to_string();
        }
    }
    panic!("expected error, got success: {:?}", resp.result);
}

/// Assert response is an error whose message contains `substr`.
fn assert_error(resp: &JsonRpcResponse, substr: &str) {
    let message = error_message(resp);
    assert!(
        message.contains(substr),
        "expected error containing {substr:?}, got: {message:?}",
    );
}

/// Extract the text content from a successful MCP response.
fn extract_response_text(resp: &JsonRpcResponse) -> String {
    let result = resp
        .result
        .as_ref()
        .unwrap_or_else(|| panic!("expected success, got error: {:?}", resp.error));
    result["content"][0]["text"]
        .as_str()
        .expect("missing text in response content")
        .to_string()
}

// -- Dispatch-level tests --------------------------------------------------

#[tokio::test]
async fn initialize_returns_capabilities() {
    let state = test_state().await;
    let resp = call(&state, "initialize", None).await;
    let result = resp.result.unwrap();
    assert_eq!(result["protocolVersion"], "2025-06-18");
    assert!(result["capabilities"]["tools"].is_object());
}

/// When the client offers a supported older protocol version, the server
/// echoes it back so the session downgrades gracefully.
#[tokio::test]
async fn initialize_echoes_supported_client_version() {
    let state = test_state().await;
    let resp = call(
        &state,
        "initialize",
        Some(json!({"protocolVersion": "2024-11-05"})),
    )
    .await;
    let result = resp.result.unwrap();
    assert_eq!(result["protocolVersion"], "2024-11-05");
}

/// When the client offers an unknown version, the server replies with its
/// latest supported version (the client may then decide to abort).
#[tokio::test]
async fn initialize_falls_back_to_server_version_for_unknown_client_version() {
    let state = test_state().await;
    let resp = call(
        &state,
        "initialize",
        Some(json!({"protocolVersion": "1999-01-01"})),
    )
    .await;
    let result = resp.result.unwrap();
    assert_eq!(result["protocolVersion"], "2025-06-18");
}

/// MCP defines `ping` for liveness probes. It must return an empty result —
/// not a `-32601 Method not found` protocol error.
#[tokio::test]
async fn ping_returns_empty_result() {
    let state = test_state().await;
    let resp = call(&state, "ping", None).await;
    assert!(
        resp.error.is_none(),
        "ping should not error: {:?}",
        resp.error
    );
    assert_eq!(resp.result.unwrap(), json!({}));
}

#[tokio::test]
async fn tools_list_returns_tools() {
    let state = test_state().await;
    let resp = call(&state, "tools/list", None).await;
    let result = resp.result.unwrap();
    let tools = result["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    for expected in super::dispatch::TOOL_NAMES {
        assert!(
            names.contains(expected),
            "tools/list missing tool: {expected}"
        );
    }
    assert_eq!(names.len(), super::dispatch::TOOL_NAMES.len());
}

/// Per MCP spec, tool-execution failures (including "tool not found" inside
/// `tools/call`) must surface as `result.isError == true`, not as a JSON-RPC
/// protocol error. Strict clients reject the wrong shape and abort the session.
#[tokio::test]
async fn tools_call_unknown_tool_returns_is_error_result() {
    let state = test_state().await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "bogus_tool", "arguments": {} })),
    )
    .await;
    assert!(resp.error.is_none(), "should not be a protocol error");
    let result = resp.result.expect("expected isError result");
    assert_eq!(result["isError"], json!(true));
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Unknown tool"), "got: {text}");
}

/// Tool handler failures (e.g. NotFound from the service layer) likewise
/// surface as `result.isError == true` rather than a JSON-RPC protocol error.
#[tokio::test]
async fn tools_call_handler_error_returns_is_error_result() {
    let state = test_state().await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "get_task", "arguments": { "task_id": 999_999 } })),
    )
    .await;
    assert!(resp.error.is_none(), "should not be a protocol error");
    let result = resp.result.expect("expected isError result");
    assert_eq!(result["isError"], json!(true));
}

#[tokio::test]
async fn unknown_method() {
    let state = test_state().await;
    let resp = call(&state, "bogus/method", None).await;
    assert!(resp.error.is_some());
    assert!(resp.error.unwrap().message.contains("Method not found"));
}

/// JSON-RPC 2.0 §4.1: the server MUST NOT reply to a notification. The MCP
/// streamable-HTTP transport spec maps that to HTTP 202 Accepted with an empty
/// body. Claude Code's strict response schema rejects `id: null`, so any body
/// here aborts the session.
#[tokio::test]
async fn notification_initialized_returns_202_with_no_body() {
    let state = test_state().await;
    let (status, body) =
        call_notification(&state, "notifications/initialized", Some(json!({}))).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert!(
        body.is_empty(),
        "expected empty body for notification, got: {:?}",
        String::from_utf8_lossy(&body)
    );
}

/// Even unknown notifications must be silently accepted — JSON-RPC forbids any
/// response (errors included) to messages without an `id`.
#[tokio::test]
async fn unknown_notification_returns_202_with_no_body() {
    let state = test_state().await;
    let (status, body) = call_notification(&state, "notifications/something_new", None).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert!(body.is_empty());
}

#[tokio::test]
async fn initialize_succeeds_without_identity() {
    let state = test_state().await;
    let resp = call_with_identity(&state, "initialize", None, Err(IdentityError::Missing)).await;
    assert!(resp.error.is_none(), "got error: {:?}", resp.error);
    assert_eq!(resp.result.unwrap()["protocolVersion"], "2025-06-18");
}

#[tokio::test]
async fn ping_succeeds_without_identity() {
    let state = test_state().await;
    let resp = call_with_identity(&state, "ping", None, Err(IdentityError::Missing)).await;
    assert!(resp.error.is_none(), "got error: {:?}", resp.error);
    assert_eq!(resp.result.unwrap(), json!({}));
}

#[tokio::test]
async fn tools_list_succeeds_without_identity() {
    let state = test_state().await;
    let resp = call_with_identity(&state, "tools/list", None, Err(IdentityError::Missing)).await;
    assert!(resp.error.is_none(), "got error: {:?}", resp.error);
    let tools = resp.result.unwrap()["tools"].as_array().unwrap().len();
    assert!(tools > 0);
}

#[tokio::test]
async fn tools_call_without_identity_returns_invalid_request_with_request_id() {
    let state = test_state().await;
    let resp = call_with_identity(
        &state,
        "tools/call",
        Some(json!({ "name": "list_tasks", "arguments": {} })),
        Err(IdentityError::Missing),
    )
    .await;
    let err = resp.error.expect("expected JSON-RPC error");
    assert_eq!(err.code, -32600);
    assert!(err.message.contains("missing"), "got: {}", err.message);
    // Strict MCP clients reject `id: null` on error responses; the handler must
    // echo back the request id (1) it parsed from the body.
    assert_eq!(resp.id, Some(json!(1)));
}

#[tokio::test]
async fn tools_call_with_conflict_identity_returns_invalid_request() {
    let state = test_state().await;
    let resp = call_with_identity(
        &state,
        "tools/call",
        Some(json!({ "name": "list_tasks", "arguments": {} })),
        Err(IdentityError::Conflict),
    )
    .await;
    let err = resp.error.expect("expected JSON-RPC error");
    assert_eq!(err.code, -32600);
    assert_eq!(resp.id, Some(json!(1)));
}

/// Validates that JSON schemas in `tool_definitions()` match the argument structs.
/// Catches drift when a field is added/removed from a struct but not the schema
/// (or vice versa). The `expected_props` lists must be kept in sync with struct
/// fields — if you add a field to a struct, add it here too; the test then fails
/// if the schema wasn't also updated.
#[tokio::test]
async fn tool_schemas_match_arg_structs() {
    use std::collections::BTreeSet;

    let defs = tool_definitions();
    let tools_arr = defs["tools"].as_array().unwrap();
    let tools: std::collections::HashMap<&str, &Value> = tools_arr
        .iter()
        .map(|t| (t["name"].as_str().unwrap(), t))
        .collect();

    fn schema_props(tool: &Value) -> (BTreeSet<&str>, BTreeSet<&str>) {
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
            BTreeSet::from([
                "task_id",
                "status",
                "plan_path",
                "title",
                "description",
                "repo_path",
                "sort_order",
                "url",
                "url_type",
                "tag",
                "sub_status",
                "epic_id",
                "base_branch",
                "wrap_up_mode",
                "auto_run_plan",
            ]),
            BTreeSet::from(["task_id"]),
            json!({"task_id": 1, "status": "review", "plan_path": "/p.md", "title": "t", "description": "d", "repo_path": "/r", "sort_order": 100, "url": "https://github.com/org/repo/pull/1", "url_type": "pr", "tag": "bug", "sub_status": "awaiting_review", "epic_id": 5}),
        ),
        (
            "get_task",
            BTreeSet::from(["task_id"]),
            BTreeSet::from(["task_id"]),
            json!({"task_id": 1}),
        ),
        (
            "create_task",
            BTreeSet::from([
                "title",
                "repo_path",
                "description",
                "plan_path",
                "epic_id",
                "sort_order",
                "tag",
                "base_branch",
                "wrap_up_mode",
                "auto_run_plan",
            ]),
            BTreeSet::from(["title", "repo_path"]),
            json!({"title": "t", "repo_path": "/r", "description": "d", "plan_path": "/p.md", "sort_order": 10, "tag": "feature"}),
        ),
        (
            "list_tasks",
            BTreeSet::from(["status", "epic_id", "repo_paths"]),
            BTreeSet::new(),
            json!({"status": "backlog", "epic_id": 1, "repo_paths": ["/r"]}),
        ),
        (
            "claim_task",
            BTreeSet::from(["task_id", "worktree", "tmux_window"]),
            BTreeSet::from(["task_id", "worktree", "tmux_window"]),
            json!({"task_id": 1, "worktree": "/w", "tmux_window": "tw"}),
        ),
        (
            "create_epic",
            BTreeSet::from([
                "title",
                "repo_path",
                "description",
                "sort_order",
                "parent_epic_id",
            ]),
            BTreeSet::from(["title", "repo_path"]),
            json!({"title": "Epic", "repo_path": "/repo", "sort_order": 5}),
        ),
        (
            "get_epic",
            BTreeSet::from(["epic_id"]),
            BTreeSet::from(["epic_id"]),
            json!({"epic_id": 1}),
        ),
        ("list_epics", BTreeSet::new(), BTreeSet::new(), json!({})),
        (
            "update_epic",
            BTreeSet::from([
                "epic_id",
                "title",
                "description",
                "status",
                "plan_path",
                "sort_order",
                "repo_path",
                "feed_command",
                "feed_interval_secs",
                "group_by_repo",
                "parent_epic_id",
            ]),
            BTreeSet::from(["epic_id"]),
            json!({"epic_id": 1, "plan_path": "docs/plan.md", "sort_order": 42, "repo_path": "/new/path"}),
        ),
        (
            "wrap_up",
            BTreeSet::from(["task_id", "action"]),
            BTreeSet::from(["task_id", "action"]),
            json!({"task_id": 1, "action": "rebase"}),
        ),
        (
            "send_message",
            BTreeSet::from(["from_task_id", "to_task_id", "body"]),
            BTreeSet::from(["from_task_id", "to_task_id", "body"]),
            json!({"from_task_id": 1, "to_task_id": 2, "body": "Hello from task 1"}),
        ),
        (
            "subscribe_to_task",
            BTreeSet::from(["watcher_task_id", "target_task_id"]),
            BTreeSet::from(["watcher_task_id", "target_task_id"]),
            json!({"watcher_task_id": 1, "target_task_id": 2}),
        ),
        (
            "unsubscribe_from_task",
            BTreeSet::from(["watcher_task_id", "target_task_id"]),
            BTreeSet::from(["watcher_task_id", "target_task_id"]),
            json!({"watcher_task_id": 1, "target_task_id": 2}),
        ),
        (
            "dispatch_next",
            BTreeSet::from(["epic_id"]),
            BTreeSet::from(["epic_id"]),
            json!({"epic_id": 1}),
        ),
        (
            "dispatch_task",
            BTreeSet::from(["task_id"]),
            BTreeSet::from(["task_id"]),
            json!({"task_id": 1}),
        ),
        (
            "record_learning",
            BTreeSet::from([
                "task_id",
                "kind",
                "summary",
                "scope",
                "detail",
                "scope_ref",
                "tags",
            ]),
            BTreeSet::from(["task_id", "kind", "summary", "scope"]),
            json!({"task_id": 1, "kind": "pitfall", "summary": "Watch out", "scope": "user"}),
        ),
        (
            "query_learnings",
            BTreeSet::from(["task_id", "query", "tag_filter", "limit"]),
            BTreeSet::from(["task_id"]),
            json!({"task_id": 1}),
        ),
        (
            "rate_learning",
            BTreeSet::from(["learning_id", "task_id", "verdict"]),
            BTreeSet::from(["learning_id", "task_id", "verdict"]),
            json!({"learning_id": 1, "task_id": 1, "verdict": "helped"}),
        ),
        (
            "delete_learning",
            BTreeSet::from(["learning_id"]),
            BTreeSet::from(["learning_id"]),
            json!({"learning_id": 1}),
        ),
        (
            "set_verify_command",
            BTreeSet::from(["repo_path", "command"]),
            BTreeSet::from(["repo_path"]),
            json!({"repo_path": "/repo"}),
        ),
        (
            "exit_session",
            BTreeSet::from(["task_id", "token", "action", "pr_url"]),
            BTreeSet::from(["task_id", "token", "action"]),
            json!({"task_id": 1, "token": "tok", "action": "rebase"}),
        ),
        (
            "index_repo",
            BTreeSet::from(["repo_path"]),
            BTreeSet::new(),
            json!({"repo_path": "/some/path"}),
        ),
        (
            "search_docs",
            BTreeSet::from(["query", "repo_path", "limit"]),
            BTreeSet::from(["query"]),
            json!({"query": "escalation patterns"}),
        ),
        (
            "get_managed_feed_config",
            BTreeSet::new(),
            BTreeSet::new(),
            json!({}),
        ),
        (
            "set_managed_feed_config",
            BTreeSet::from([
                "reviews_command",
                "reviews_interval_secs",
                "cve_command",
                "cve_interval_secs",
            ]),
            BTreeSet::new(),
            json!({}),
        ),
        (
            "query_usage",
            BTreeSet::from(["category", "actor", "since", "limit"]),
            BTreeSet::new(),
            json!({"category": "mcp_tool", "actor": "human", "since": "2026-01-01T00:00:00Z", "limit": 25}),
        ),
    ];

    // Verify we cover exactly the tools that exist
    let expected_names: BTreeSet<&str> = cases.iter().map(|(name, _, _, _)| *name).collect();
    let actual_names: BTreeSet<&str> = tools.keys().copied().collect();
    assert_eq!(
        actual_names, expected_names,
        "Tool list mismatch — update this test when adding/removing tools"
    );

    for (name, exp_props, exp_required, payload) in &cases {
        let tool = tools[name];
        let (actual_props, actual_required) = schema_props(tool);

        assert_eq!(&actual_props, exp_props, "Property mismatch for '{name}'");
        assert_eq!(
            &actual_required, exp_required,
            "Required field mismatch for '{name}'"
        );

        // Verify the full payload deserializes into the struct
        match *name {
            "update_task" => {
                serde_json::from_value::<UpdateTaskArgs>(payload.clone()).unwrap();
            }
            "get_task" => {
                serde_json::from_value::<GetTaskArgs>(payload.clone()).unwrap();
            }
            "create_task" => {
                serde_json::from_value::<CreateTaskWithEpicArgs>(payload.clone()).unwrap();
            }
            "list_tasks" => {
                serde_json::from_value::<ListTasksArgs>(payload.clone()).unwrap();
            }
            "claim_task" => {
                serde_json::from_value::<ClaimTaskArgs>(payload.clone()).unwrap();
            }
            "create_epic" => {
                serde_json::from_value::<CreateEpicArgs>(payload.clone()).unwrap();
            }
            "get_epic" => {
                serde_json::from_value::<GetEpicArgs>(payload.clone()).unwrap();
            }
            "list_epics" => {} // no args
            "update_epic" => {
                serde_json::from_value::<UpdateEpicArgs>(payload.clone()).unwrap();
            }
            "wrap_up" => {
                serde_json::from_value::<WrapUpArgs>(payload.clone()).unwrap();
            }
            "send_message" => {
                serde_json::from_value::<SendMessageArgs>(payload.clone()).unwrap();
            }
            "subscribe_to_task" => {
                serde_json::from_value::<super::tasks::SubscribeToTaskArgs>(payload.clone())
                    .unwrap();
            }
            "unsubscribe_from_task" => {
                serde_json::from_value::<super::tasks::UnsubscribeFromTaskArgs>(payload.clone())
                    .unwrap();
            }
            "dispatch_next" => {
                serde_json::from_value::<DispatchNextArgs>(payload.clone()).unwrap();
            }
            "dispatch_task" => {
                serde_json::from_value::<super::tasks::DispatchTaskArgs>(payload.clone()).unwrap();
            }
            "record_learning" => {
                serde_json::from_value::<super::learnings::RecordLearningArgs>(payload.clone())
                    .unwrap();
            }
            "query_learnings" => {
                serde_json::from_value::<super::learnings::QueryLearningsArgs>(payload.clone())
                    .unwrap();
            }
            "rate_learning" => {
                serde_json::from_value::<super::learnings::RateLearningArgs>(payload.clone())
                    .unwrap();
            }
            "delete_learning" => {
                serde_json::from_value::<super::learnings::DeleteLearningArgs>(payload.clone())
                    .unwrap();
            }
            "set_verify_command" => {
                serde_json::from_value::<super::tasks::SetVerifyCommandArgs>(payload.clone())
                    .unwrap();
            }
            "exit_session" => {
                serde_json::from_value::<ExitSessionArgs>(payload.clone()).unwrap();
            }
            "index_repo" => {
                serde_json::from_value::<super::repo_rag::IndexRepoArgs>(payload.clone()).unwrap();
            }
            "search_docs" => {
                serde_json::from_value::<super::repo_rag::SearchDocsArgs>(payload.clone()).unwrap();
            }
            "get_managed_feed_config" => {} // no args
            "set_managed_feed_config" => {
                serde_json::from_value::<super::managed_feeds::SetManagedFeedConfigArgs>(
                    payload.clone(),
                )
                .unwrap();
            }
            "query_usage" => {
                serde_json::from_value::<super::tasks::QueryUsageArgs>(payload.clone()).unwrap();
            }
            other => panic!("No deserialization check for tool: {other}"),
        }
    }
}

#[tokio::test]
async fn list_projects_tool_is_removed() {
    let state = test_state().await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_projects", "arguments": {} })),
    )
    .await;
    // Per MCP spec, unknown tools surface as isError: true
    let result = resp.result.expect("expected isError result");
    assert_eq!(result["isError"], json!(true));
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Unknown tool"), "got: {text}");
}

#[tokio::test]
async fn create_task_from_session_succeeds() {
    let (state, db) = test_state_with_db().await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_task",
            "arguments": { "title": "T", "repo_path": "/r" }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
    let result = resp.result.as_ref().expect("expected result");
    assert!(
        result.get("isError").is_none() || result["isError"] != json!(true),
        "unexpected isError: {:?}",
        resp.result
    );
    let tasks = db.list_all().await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].title, "T");
}
