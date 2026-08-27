#![allow(clippy::unwrap_used, clippy::expect_used)]
mod epics;
mod learnings;
mod managed_feeds;
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
use super::types::{JsonRpcRequest, JsonRpcResponse};

/// The single `McpState` constructor the test module builds on: an in-memory DB
/// plus whichever of the three injectable seams a test cares about. Everything
/// else here (`test_state`, `test_state_with_db`, `state_with_mock_task_svc`,
/// `ChainFixture`) delegates, so the wiring exists once.
async fn test_state_with_overrides(
    runner: Arc<dyn ProcessRunner>,
    notify_tx: Option<mpsc::UnboundedSender<crate::mcp::McpEvent>>,
    task_svc: Option<Arc<dyn crate::service::TaskServiceApi>>,
) -> (Arc<McpState>, Arc<dyn db::TaskStore>) {
    test_state_with_overrides_and_bg_done(runner, notify_tx, task_svc, None).await
}

/// Like [`test_state_with_overrides`], but also installs a completion signal
/// for fire-and-forget background writes (usage, trajectory, the
/// `exit_session` tmux teardown), so a test can await one deterministically
/// instead of sleeping.
async fn test_state_with_overrides_and_bg_done(
    runner: Arc<dyn ProcessRunner>,
    notify_tx: Option<mpsc::UnboundedSender<crate::mcp::McpEvent>>,
    task_svc: Option<Arc<dyn crate::service::TaskServiceApi>>,
    bg_write_done_tx: Option<mpsc::UnboundedSender<BackgroundWrite>>,
) -> (Arc<McpState>, Arc<dyn db::TaskStore>) {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let mut state = McpState::new(
        McpDeps {
            db: db.clone(),
            runner,
            embedding_service: EmbeddingService::new_test(),
            data_dir: std::env::temp_dir(),
        },
        notify_tx,
    );
    if let Some(task_svc) = task_svc {
        state.task_svc = task_svc;
    }
    state.test_hooks.bg_write_done_tx = bg_write_done_tx;
    (Arc::new(state), db)
}

async fn test_state() -> Arc<McpState> {
    test_state_with_db().await.0
}

/// Like [`test_state`], but installs a completion signal that fires after each
/// fire-and-forget background write. Returns the receiver so the test can await
/// the write (e.g. usage recording) deterministically instead of sleeping.
async fn test_state_with_bg_done() -> (Arc<McpState>, mpsc::UnboundedReceiver<BackgroundWrite>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
    let (state, _db) = test_state_with_overrides_and_bg_done(runner, None, None, Some(tx)).await;
    (state, rx)
}

async fn test_state_with_db() -> (Arc<McpState>, Arc<dyn db::TaskStore>) {
    test_state_with_overrides(Arc::new(MockProcessRunner::new(vec![])), None, None).await
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
            schedule_interval_secs: None,
            pinned_branch: None,
        })
        .await
        .unwrap()
}

/// Create a Running task with worktree and tmux_window set — ready for
/// exit_session. Uses the placeholder `/repo` path, which is fine for any test
/// that never provisions.
async fn create_running_task_with_window(state: &Arc<McpState>) -> crate::models::TaskId {
    create_running_task_with_window_in(state, "/repo", None).await
}

/// [`create_running_task_with_window`] for a task that must live in a real
/// on-disk repo (so a dispatch can provision against it) and/or belong to an
/// epic. The worktree and window are derived from the task id, so a fixture
/// holding several of them keeps them distinct.
async fn create_running_task_with_window_in(
    state: &Arc<McpState>,
    repo_path: &str,
    epic_id: Option<crate::models::EpicId>,
) -> crate::models::TaskId {
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Running Task",
            description: "description",
            repo_path,
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id,
            sort_order: Some(0),
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
            schedule_interval_secs: None,
            pinned_branch: None,
        })
        .await
        .unwrap();
    let worktree = format!("{repo_path}/.worktrees/{}-running-task", task_id.0);
    let window = format!("task-{}", task_id.0);
    let patch = crate::db::TaskPatch::new()
        .worktree(Some(worktree.as_str()))
        .tmux_window(Some(window.as_str()));
    state.db_write().patch_task(task_id, &patch).await.unwrap();
    task_id
}

/// The PR url [`close_session_via_mcp`] supplies for the `Pr` action.
const TEST_PR_URL: &str = "https://github.com/acme/repo/pull/1";

/// Seed the in-memory exit token `wrap_up` would have issued for `action`, and
/// return it. Shared because the token's shape is one struct in one map and
/// every close test needs it — inlining it made the shape a 15-site edit.
fn seed_exit_token(
    state: &Arc<McpState>,
    task_id: crate::models::TaskId,
    action: crate::mcp::handlers::tasks::WrapUpAction,
) -> String {
    let token = "tok".to_string();
    state.exit_tokens.write().unwrap().insert(
        task_id,
        crate::mcp::ExitToken {
            token: token.clone(),
            action,
        },
    );
    token
}

/// Close `task_id`'s session with `action`, seeding the exit token the same way
/// `wrap_up` would. `pr_url` is supplied for the `Pr` action, which requires it.
async fn close_session_via_mcp(
    state: &Arc<McpState>,
    task_id: crate::models::TaskId,
    action: crate::mcp::handlers::tasks::WrapUpAction,
) -> JsonRpcResponse {
    let token = seed_exit_token(state, task_id, action);
    let mut arguments = json!({
        "task_id": task_id.0,
        "token": token,
        "action": action.as_str(),
    });
    if action == crate::mcp::handlers::tasks::WrapUpAction::Pr {
        arguments["pr_url"] = json!(TEST_PR_URL);
    }
    call(
        state,
        "tools/call",
        Some(json!({ "name": "exit_session", "arguments": arguments })),
    )
    .await
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

/// Every tool's schema is internally consistent: `required` only lists names
/// that are actually declared in `properties`. A required-but-undeclared field
/// (the shape of the `repo_path` defect: schema said "required" for a field no
/// struct backs) would slip past `deny_unknown_fields`, since that attribute
/// only rejects fields present in a *request*, not fields the schema
/// over-promises without a request ever mentioning them.
#[tokio::test]
async fn tool_schemas_have_consistent_required_fields() {
    let defs = tool_definitions();
    let tools_arr = defs["tools"].as_array().unwrap();

    for tool in tools_arr {
        let name = tool["name"].as_str().unwrap();
        let schema = &tool["inputSchema"];
        let props = schema["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{name}: inputSchema.properties must be an object"));
        let required = schema
            .get("required")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for field in &required {
            let field = field.as_str().unwrap();
            assert!(
                props.contains_key(field),
                "{name}: '{field}' is required but not declared in properties"
            );
        }
    }
}

/// `WrapUpAction::ALL` backs the wrap_up/exit_session MCP schema's action
/// enum (dispatch.rs) — a variant added there without updating `ALL` would
/// silently under-advertise it.
#[test]
fn wrap_up_action_all_has_every_variant() {
    assert_eq!(crate::mcp::handlers::tasks::WrapUpAction::ALL.len(), 3);
}

/// `update_task.status` and `update_task.sub_status` are the two schema
/// fields that deliberately advertise a SUBSET of their backing enum
/// (status excludes `done`/`archived`; sub_status excludes the
/// system-derived `stale_shell` and `pr_closed`) rather than the full
/// `::ALL`. Each subset
/// is its own named const — `TaskStatus::MCP_UPDATABLE` /
/// `SubStatus::MCP_ADVERTISED` — derived in the schema exactly like every
/// full-set field, so a variant silently missing from either (not just a
/// typo in an already-present string) fails here instead of just not being
/// advertised.
#[test]
fn subset_enum_consts_have_every_intended_variant() {
    assert_eq!(TaskStatus::MCP_UPDATABLE.len(), 3);
    assert_eq!(SubStatus::MCP_ADVERTISED.len(), 9);
}

/// Every MCP arg struct carries `#[serde(deny_unknown_fields)]`, so a stray or
/// stale argument (like the discarded `repo_path` on `create_epic`) surfaces
/// as a JSON-RPC error instead of being silently dropped. This exercises every
/// registered tool with a minimal valid payload plus one bogus field — the
/// per-field/per-value behaviour of each struct is covered by that handler's
/// own tests elsewhere in this suite.
///
/// `list_epics` and `get_managed_feed_config` are intentionally absent: both
/// take zero arguments and their handlers never parse `args` at all, so there
/// is no struct for `deny_unknown_fields` to guard.
#[tokio::test]
async fn every_tool_with_args_rejects_unknown_field() {
    let state = test_state().await;

    let payloads: &[(&str, Value)] = &[
        ("update_task", json!({"task_id": 1})),
        ("get_task", json!({"task_id": 1})),
        ("create_task", json!({"title": "t", "repo_path": "/r"})),
        ("list_tasks", json!({})),
        ("create_epic", json!({"title": "t"})),
        ("get_epic", json!({"epic_id": 1})),
        ("update_epic", json!({"epic_id": 1})),
        ("wrap_up", json!({"task_id": 1, "action": "rebase"})),
        ("dispatch_task", json!({"task_id": 1})),
        (
            "subscribe_to_task",
            json!({"watcher_task_id": 1, "target_task_id": 2}),
        ),
        (
            "unsubscribe_from_task",
            json!({"watcher_task_id": 1, "target_task_id": 2}),
        ),
        (
            "record_learning",
            json!({"task_id": 1, "kind": "pitfall", "summary": "s", "scope": "user"}),
        ),
        ("query_learnings", json!({"task_id": 1})),
        (
            "rate_learning",
            json!({"learning_id": 1, "task_id": 1, "verdict": "helped"}),
        ),
        ("delete_learning", json!({"learning_id": 1})),
        ("set_verify_command", json!({"repo_path": "/r"})),
        (
            "exit_session",
            json!({"task_id": 1, "token": "t", "action": "rebase"}),
        ),
        ("set_managed_feed_config", json!({})),
        ("query_usage", json!({})),
    ];

    let no_arg_tools = ["list_epics", "get_managed_feed_config"];
    let covered: std::collections::BTreeSet<&str> = payloads
        .iter()
        .map(|(n, _)| *n)
        .chain(no_arg_tools)
        .collect();
    let all_tools: std::collections::BTreeSet<&str> =
        super::dispatch::TOOL_NAMES.iter().copied().collect();
    assert_eq!(
        covered, all_tools,
        "every registered tool must appear either in `payloads` or `no_arg_tools`"
    );

    for (name, base_args) in payloads {
        let mut args = base_args.clone();
        args.as_object_mut()
            .unwrap()
            .insert("__bogus_unknown_field__".to_string(), json!(true));

        let resp = call(
            &state,
            "tools/call",
            Some(json!({ "name": name, "arguments": args })),
        )
        .await;
        assert!(
            is_error(&resp),
            "tool '{name}' should reject an unknown argument, got: {:?}",
            resp.result
        );
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

/// `claim_task` was the agent-facing "adopt this backlog task into the worktree
/// I already have" call, removed in #3808: it had zero recorded invocations
/// across 627 agent trajectories, no skill told an agent to call it, and it was
/// the last Backlog -> Running path still doing a read-then-write status check
/// instead of an atomic claim.
///
/// `TOOL_NAMES` is the whole guard, rather than a `tools/call` round-trip or
/// `tools_list_returns_tools`. That test compares `tools/list` *against*
/// `TOOL_NAMES`, so it is self-consistent by construction and would stay green
/// if the tool came back. And `TOOL_NAMES` and `dispatch_tool`'s match arms
/// expand from the same `$name` literals in `mcp_tools!`, so a name absent here
/// is unroutable by construction — a live-dispatch check could not fail
/// independently of this assert.
///
/// #3824 tracks folding this and the two sibling tool-absence tests into one
/// registry test that pins `TOOL_NAMES` against an explicit list.
#[test]
fn claim_task_tool_is_removed() {
    assert!(
        !super::dispatch::TOOL_NAMES.contains(&"claim_task"),
        "claim_task must not be a registered tool"
    );
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
