use std::sync::Arc;

use axum::{extract::State, http::StatusCode, Json};
use serde_json::{json, Value};

use crate::mcp::McpState;

use super::epics;
use super::tasks;
use super::types::{JsonRpcRequest, JsonRpcResponse};

// ---------------------------------------------------------------------------
// Tool definitions returned by tools/list
// ---------------------------------------------------------------------------

pub(super) fn tool_definitions() -> Value {
    json!({
        "tools": [
            {
                "name": "update_task",
                "description": "Update a task's status, sub_status, title, description, repo_path, plan, and/or PR fields. At least one field besides task_id must be provided.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "task_id": {
                            "type": "integer",
                            "description": "The task ID"
                        },
                        "status": {
                            "type": "string",
                            "description": "New status: backlog, running, or review. Setting done is not allowed via MCP — ask the human operator to move the task to done from the TUI.",
                            "enum": ["backlog", "running", "review"]
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
                        },
                        "repo_path": {
                            "type": "string",
                            "description": "New repository path for the task"
                        },
                        "sort_order": {
                            "type": "integer",
                            "description": "Display order within column (lower values appear first)"
                        },
                        "pr_url": {
                            "type": "string",
                            "description": "URL of the pull request associated with this task"
                        },
                        "tag": {
                            "type": "string",
                            "description": "Task tag: bug, feature, chore, or epic. Controls dispatch behavior.",
                            "enum": ["bug", "feature", "chore", "epic"]
                        },
                        "sub_status": {
                            "type": "string",
                            "description": "Sub-status within the current status column. Running: active, needs_input, stale, crashed. Review: awaiting_review, changes_requested, approved. Must be valid for the task's current (or new) status.",
                            "enum": ["none", "active", "needs_input", "stale", "crashed", "awaiting_review", "changes_requested", "approved"]
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
                "description": "Create a new task on the kanban board. Tasks always start in 'backlog' status.",
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
                            "description": "Absolute file path to the implementation plan (optional)."
                        },
                        "epic_id": {
                            "type": "integer",
                            "description": "Optional epic ID to link this task to"
                        },
                        "sort_order": {
                            "type": "integer",
                            "description": "Display order within column (lower values appear first)"
                        },
                        "tag": {
                            "type": "string",
                            "description": "Task tag: bug, feature, chore, or epic. Controls dispatch behavior.",
                            "enum": ["bug", "feature", "chore", "epic"]
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
                                { "type": "string", "enum": ["backlog", "running", "review", "done", "archived"] },
                                { "type": "array", "items": { "type": "string", "enum": ["backlog", "running", "review", "done", "archived"] } }
                            ]
                        }
                    }
                }
            },
            {
                "name": "claim_task",
                "description": "Claim a backlog task into your current worktree. Sets the task to running and associates it with your worktree and tmux window. Only tasks in the same repo can be claimed.",
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
                        "sort_order": { "type": "integer", "description": "Display order within column (lower values appear first)" }
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
                "description": "Update an epic's title, description, done status, plan, sort order, or repo path.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "epic_id": { "type": "integer", "description": "The epic ID" },
                        "title": { "type": "string", "description": "New title" },
                        "description": { "type": "string", "description": "New description" },
                        "done": { "type": "boolean", "description": "Mark epic as done" },
                        "plan": { "type": "string", "description": "Path to the plan file" },
                        "sort_order": { "type": "integer", "description": "Display order within column (lower values appear first)" },
                        "repo_path": { "type": "string", "description": "Repository path for the epic" }
                    },
                    "required": ["epic_id"]
                }
            },
            {
                "name": "wrap_up",
                "description": "Wrap up a running or review task: rebase the branch onto main or push and create a GitHub PR. The task is moved to done on success. Returns immediately; the operation runs in the background. For rebase, the tmux window is killed when done, ending your session.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "task_id": {
                            "type": "integer",
                            "description": "The task ID to wrap up"
                        },
                        "action": {
                            "type": "string",
                            "enum": ["rebase", "pr"],
                            "description": "rebase: rebase branch onto main and fast-forward. pr: push branch and create a GitHub PR."
                        }
                    },
                    "required": ["task_id", "action"]
                }
            },
            {
                "name": "report_usage",
                "description": "Report token usage and cost for a task session. Accumulates across sessions.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "task_id":            { "type": "integer", "description": "The task ID" },
                        "cost_usd":           { "type": "number",  "description": "Session cost in USD" },
                        "input_tokens":       { "type": "integer", "description": "Input tokens used" },
                        "output_tokens":      { "type": "integer", "description": "Output tokens used" },
                        "cache_read_tokens":  { "type": "integer", "description": "Cache read tokens (optional)" },
                        "cache_write_tokens": { "type": "integer", "description": "Cache write tokens (optional)" }
                    },
                    "required": ["task_id", "cost_usd", "input_tokens", "output_tokens"]
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
        "initialize" => JsonRpcResponse::ok(
            id,
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "dispatch",
                    "version": "0.1.0"
                }
            }),
        ),

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
                "report_usage" => tasks::handle_report_usage(&state, id, args),
                "create_epic" => epics::handle_create_epic(&state, id, args),
                "get_epic" => epics::handle_get_epic(&state, id, args),
                "list_epics" => epics::handle_list_epics(&state, id, args),
                "update_epic" => epics::handle_update_epic(&state, id, args),
                "wrap_up" => tasks::handle_wrap_up(&state, id, args).await,
                other => JsonRpcResponse::err(id, -32602, format!("Unknown tool: {other}")),
            }
        }

        other => JsonRpcResponse::err(id, -32601, format!("Method not found: {other}")),
    };

    (StatusCode::OK, Json(response))
}
