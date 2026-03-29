use std::sync::Arc;

use axum::{Json, extract::State, http::StatusCode};
use serde_json::{Value, json};

use crate::mcp::McpState;

use super::types::{JsonRpcRequest, JsonRpcResponse};
use super::tasks;
use super::epics;

// ---------------------------------------------------------------------------
// Tool definitions returned by tools/list
// ---------------------------------------------------------------------------

pub(super) fn tool_definitions() -> Value {
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
                            "description": "New status: backlog, ready, running, or review. Setting done is not allowed via MCP — ask the human operator to move the task to done from the TUI.",
                            "enum": ["backlog", "ready", "running", "review"]
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
                        },
                        "epic_id": {
                            "type": "integer",
                            "description": "Optional epic ID to link this task to"
                        }
                    },
                    "required": ["title", "repo_path"]
                }
            },
            {
                "name": "list_tasks",
                "description": "List tasks on the kanban board, optionally filtered by status.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "status": {
                            "description": "Filter by status. Single string or array of strings.",
                            "oneOf": [
                                { "type": "string", "enum": ["backlog", "ready", "running", "review", "done"] },
                                { "type": "array", "items": { "type": "string", "enum": ["backlog", "ready", "running", "review", "done"] } }
                            ]
                        }
                    }
                }
            },
            {
                "name": "claim_task",
                "description": "Claim a backlog or ready task into your current worktree. Sets the task to running and associates it with your worktree and tmux window. Only tasks in the same repo can be claimed.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "task_id": {
                            "type": "integer",
                            "description": "The task ID to claim"
                        },
                        "worktree": {
                            "type": "string",
                            "description": "Your current worktree path (from git rev-parse --show-toplevel)"
                        },
                        "tmux_window": {
                            "type": "string",
                            "description": "Your current tmux window name (from tmux display-message -p '#W')"
                        }
                    },
                    "required": ["task_id", "worktree", "tmux_window"]
                }
            },
            {
                "name": "create_epic",
                "description": "Create a new epic on the kanban board.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "description": "Epic title" },
                        "repo_path": { "type": "string", "description": "Repository path" },
                        "description": { "type": "string", "description": "Epic description" },
                        "plan": { "type": "string", "description": "High-level markdown plan" }
                    },
                    "required": ["title", "repo_path"]
                }
            },
            {
                "name": "get_epic",
                "description": "Get details about an epic including its subtask summary.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "epic_id": { "type": "integer", "description": "The epic ID" }
                    },
                    "required": ["epic_id"]
                }
            },
            {
                "name": "list_epics",
                "description": "List all epics on the kanban board.",
                "inputSchema": { "type": "object", "properties": {} }
            },
            {
                "name": "update_epic",
                "description": "Update an epic's title, description, plan, or done status.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "epic_id": { "type": "integer", "description": "The epic ID" },
                        "title": { "type": "string", "description": "New title" },
                        "description": { "type": "string", "description": "New description" },
                        "plan": { "type": "string", "description": "New high-level plan" },
                        "done": { "type": "boolean", "description": "Mark epic as done" }
                    },
                    "required": ["epic_id"]
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
                "update_task" => tasks::handle_update_task(&state, id, args),
                "get_task" => tasks::handle_get_task(&state, id, args),
                "create_task" => tasks::handle_create_task(&state, id, args),
                "list_tasks" => tasks::handle_list_tasks(&state, id, args),
                "claim_task" => tasks::handle_claim_task(&state, id, args),
                "create_epic" => epics::handle_create_epic(&state, id, args),
                "get_epic" => epics::handle_get_epic(&state, id, args),
                "list_epics" => epics::handle_list_epics(&state, id, args),
                "update_epic" => epics::handle_update_epic(&state, id, args),
                other => JsonRpcResponse::err(id, -32602, format!("Unknown tool: {other}")),
            }
        }

        other => JsonRpcResponse::err(id, -32601, format!("Method not found: {other}")),
    };

    (StatusCode::OK, Json(response))
}
