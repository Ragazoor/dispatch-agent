use std::sync::Arc;

use axum::{Json, extract::State, http::StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::models::{TaskId, TaskStatus};

use super::McpState;

// ---------------------------------------------------------------------------
// JSON-RPC request / response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    pub params: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
}

impl JsonRpcResponse {
    fn ok(id: Option<Value>, result: Value) -> Self {
        JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    fn err(id: Option<Value>, code: i32, message: impl Into<String>) -> Self {
        JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Flexible i64 deserializer (accepts both 4 and "4")
// ---------------------------------------------------------------------------

/// Claude Code sometimes sends integer MCP arguments as strings.
/// This deserializer accepts both native integers and string-encoded integers.
fn deserialize_flexible_i64<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    struct FlexibleI64Visitor;

    impl<'de> de::Visitor<'de> for FlexibleI64Visitor {
        type Value = i64;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("an integer or a string containing an integer")
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<i64, E> {
            Ok(v)
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<i64, E> {
            i64::try_from(v).map_err(|_| E::custom(format!("u64 out of i64 range: {v}")))
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<i64, E> {
            v.parse::<i64>().map_err(|_| E::custom(format!("invalid integer string: {v}")))
        }
    }

    deserializer.deserialize_any(FlexibleI64Visitor)
}

// ---------------------------------------------------------------------------
// Typed argument structs for tool calls
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct UpdateTaskArgs {
    #[serde(deserialize_with = "deserialize_flexible_i64")]
    task_id: i64,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    plan: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Deserialize)]
struct GetTaskArgs {
    #[serde(deserialize_with = "deserialize_flexible_i64")]
    task_id: i64,
}

#[derive(Deserialize)]
struct CreateTaskArgs {
    title: String,
    repo_path: String,
    #[serde(default)]
    description: String,
    plan: Option<String>,
}

fn parse_args<T: serde::de::DeserializeOwned>(
    id: Option<Value>,
    args: Value,
) -> Result<T, JsonRpcResponse> {
    serde_json::from_value(args)
        .map_err(|e| JsonRpcResponse::err(id, -32602, format!("Invalid arguments: {e}")))
}

// ---------------------------------------------------------------------------
// Tool definitions returned by tools/list
// ---------------------------------------------------------------------------

fn tool_definitions() -> Value {
    json!({
        "tools": [
            {
                "name": "update_task",
                "description": "Update a task's status, title, description, and/or plan. At least one field besides task_id must be provided.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "task_id": {
                            "type": "integer",
                            "description": "The task ID"
                        },
                        "status": {
                            "type": "string",
                            "description": "New status: backlog, ready, running, review, or done",
                            "enum": ["backlog", "ready", "running", "review", "done"]
                        },
                        "plan": {
                            "type": "string",
                            "description": "Absolute file path to the implementation plan"
                        },
                        "title": {
                            "type": "string",
                            "description": "New title for the task"
                        },
                        "description": {
                            "type": "string",
                            "description": "New description for the task"
                        }
                    },
                    "required": ["task_id"]
                }
            },
            {
                "name": "get_task",
                "description": "Get details about a task",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "task_id": {
                            "type": "integer",
                            "description": "The task ID"
                        }
                    },
                    "required": ["task_id"]
                }
            },
            {
                "name": "create_task",
                "description": "Create a new task on the kanban board. If a plan file path is provided, the task is created in 'ready' status; otherwise it starts in 'backlog'.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "title": {
                            "type": "string",
                            "description": "Task title"
                        },
                        "repo_path": {
                            "type": "string",
                            "description": "Path to the repository for this task"
                        },
                        "description": {
                            "type": "string",
                            "description": "Task description (optional, defaults to empty)"
                        },
                        "plan": {
                            "type": "string",
                            "description": "Absolute file path to the implementation plan (optional). If provided, task starts in 'ready' status."
                        }
                    },
                    "required": ["title", "repo_path"]
                }
            }
        ]
    })
}

// ---------------------------------------------------------------------------
// MCP handler
// ---------------------------------------------------------------------------

pub async fn handle_mcp(
    State(state): State<Arc<McpState>>,
    Json(req): Json<JsonRpcRequest>,
) -> (StatusCode, Json<JsonRpcResponse>) {
    let id = req.id;
    let response = match req.method.as_str() {
        "initialize" => {
            JsonRpcResponse::ok(id, json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "task-orchestrator",
                    "version": "0.1.0"
                }
            }))
        }

        "tools/list" => JsonRpcResponse::ok(id, tool_definitions()),

        "tools/call" => {
            let params = req.params.unwrap_or(Value::Null);
            let tool_name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(Value::Null);

            match tool_name {
                "update_task" => handle_update_task(&state, id, args),
                "get_task" => handle_get_task(&state, id, args),
                "create_task" => handle_create_task(&state, id, args),
                other => JsonRpcResponse::err(id, -32602, format!("Unknown tool: {other}")),
            }
        }

        other => JsonRpcResponse::err(id, -32601, format!("Method not found: {other}")),
    };

    (StatusCode::OK, Json(response))
}

// ---------------------------------------------------------------------------
// Tool implementations
// ---------------------------------------------------------------------------

fn handle_update_task(state: &McpState, id: Option<Value>, args: Value) -> JsonRpcResponse {
    let parsed = match parse_args::<UpdateTaskArgs>(id.clone(), args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!(task_id = parsed.task_id, status = ?parsed.status, "MCP update_task");

    let has_update = parsed.status.is_some()
        || parsed.plan.is_some()
        || parsed.title.is_some()
        || parsed.description.is_some();

    if !has_update {
        return JsonRpcResponse::err(
            id,
            -32602,
            "At least one of status, plan, title, or description must be provided",
        );
    }

    let status = if let Some(ref status_str) = parsed.status {
        match TaskStatus::parse(status_str) {
            Some(s) => Some(s),
            None => {
                return JsonRpcResponse::err(
                    id,
                    -32602,
                    format!(
                        "Unknown status: {status_str}. Valid values: backlog, ready, running, review, done"
                    ),
                )
            }
        }
    } else {
        None
    };

    let plan = parsed.plan.as_ref().map(|p| Some(p.as_str()));

    if let Err(e) = state.db.update_task_partial(
        TaskId(parsed.task_id),
        status,
        plan,
        parsed.title.as_deref(),
        parsed.description.as_deref(),
    ) {
        return JsonRpcResponse::err(id, -32603, format!("Database error: {e}"));
    }

    let mut updated = Vec::new();
    if let Some(ref s) = parsed.status { updated.push(format!("status={s}")); }
    if parsed.plan.is_some() { updated.push("plan".to_string()); }
    if parsed.title.is_some() { updated.push("title".to_string()); }
    if parsed.description.is_some() { updated.push("description".to_string()); }

    JsonRpcResponse::ok(
        id,
        json!({"content": [{"type": "text", "text": format!("Task {} updated ({})", parsed.task_id, updated.join(", "))}]}),
    )
}

fn handle_create_task(state: &McpState, id: Option<Value>, args: Value) -> JsonRpcResponse {
    let parsed = match parse_args::<CreateTaskArgs>(id.clone(), args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!(title = %parsed.title, "MCP create_task");

    let plan = parsed.plan.as_deref().map(|p| {
        std::fs::canonicalize(p)
            .map(|abs| abs.to_string_lossy().into_owned())
            .unwrap_or_else(|_| p.to_string())
    });

    let status = if plan.is_some() {
        TaskStatus::Ready
    } else {
        TaskStatus::Backlog
    };

    match state.db.create_task(
        &parsed.title,
        &parsed.description,
        &parsed.repo_path,
        plan.as_deref(),
        status,
    ) {
        Ok(task_id) => JsonRpcResponse::ok(
            id,
            json!({"content": [{"type": "text", "text": format!("Task {task_id} created")}]}),
        ),
        Err(e) => JsonRpcResponse::err(id, -32603, format!("Database error: {e}")),
    }
}

fn handle_get_task(state: &McpState, id: Option<Value>, args: Value) -> JsonRpcResponse {
    let parsed = match parse_args::<GetTaskArgs>(id.clone(), args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!(task_id = parsed.task_id, "MCP get_task");
    match state.db.get_task(TaskId(parsed.task_id)) {
        Ok(Some(task)) => {
            let text = format!(
                "Task {id}: {title}\nStatus: {status}\nRepo: {repo}\nDescription: {desc}",
                id = task.id,
                title = task.title,
                status = task.status.as_str(),
                repo = task.repo_path,
                desc = task.description,
            );
            JsonRpcResponse::ok(id, json!({"content": [{"type": "text", "text": text}]}))
        }
        Ok(None) => JsonRpcResponse::err(id, -32602, format!("Task {} not found", parsed.task_id)),
        Err(e) => JsonRpcResponse::err(id, -32603, format!("Database error: {e}")),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{self, Database};

    fn test_state() -> Arc<McpState> {
        let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().unwrap());
        Arc::new(McpState { db })
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
    }

    #[tokio::test]
    async fn update_task_valid() {
        let state = test_state();
        let task_id = state.db.create_task("Test", "desc", "/repo", None, crate::models::TaskStatus::Backlog).unwrap();

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
        let task_id = state.db.create_task("Test", "desc", "/repo", None, crate::models::TaskStatus::Backlog).unwrap();

        let resp = call(
            &state,
            "tools/call",
            Some(json!({
                "name": "update_task",
                "arguments": { "task_id": task_id.0, "status": "bogus" }
            })),
        ).await;
        assert!(resp.error.is_some());
        assert!(resp.error.unwrap().message.contains("Unknown status"));
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
    async fn create_task_with_plan_sets_ready() {
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
        assert_eq!(tasks[0].status, TaskStatus::Ready);
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
        assert_eq!(task.status, crate::models::TaskStatus::Ready);
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
    async fn update_task_partial_sets_multiple_fields() {
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
        assert_eq!(task.status, TaskStatus::Ready);
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
}
