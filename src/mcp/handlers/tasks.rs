use serde::Deserialize;
use serde_json::{Value, json};

use crate::db;
use crate::models::{EpicId, TaskId, TaskStatus};
use crate::mcp::McpState;

use super::types::{JsonRpcResponse, deserialize_flexible_i64, deserialize_optional_flexible_i64, parse_args};

// ---------------------------------------------------------------------------
// Typed argument structs
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub(super) struct UpdateTaskArgs {
    #[serde(deserialize_with = "deserialize_flexible_i64")]
    pub(super) task_id: i64,
    #[serde(default)]
    pub(super) status: Option<String>,
    #[serde(default)]
    pub(super) plan: Option<String>,
    #[serde(default)]
    pub(super) title: Option<String>,
    #[serde(default)]
    pub(super) description: Option<String>,
    #[serde(default)]
    pub(super) repo_path: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct GetTaskArgs {
    #[serde(deserialize_with = "deserialize_flexible_i64")]
    pub(super) task_id: i64,
}

#[derive(Deserialize)]
pub(super) struct ListTasksArgs {
    #[serde(default)]
    pub(super) status: Option<Value>,
}

#[derive(Deserialize)]
pub(super) struct ClaimTaskArgs {
    #[serde(deserialize_with = "deserialize_flexible_i64")]
    pub(super) task_id: i64,
    pub(super) worktree: String,
    pub(super) tmux_window: String,
}

#[derive(Deserialize)]
pub(super) struct CreateTaskWithEpicArgs {
    pub(super) title: String,
    pub(super) repo_path: String,
    #[serde(default)]
    pub(super) description: String,
    pub(super) plan: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_flexible_i64")]
    pub(super) epic_id: Option<i64>,
}

// ---------------------------------------------------------------------------
// Task tool handlers
// ---------------------------------------------------------------------------

pub(super) fn handle_update_task(state: &McpState, id: Option<Value>, args: Value) -> JsonRpcResponse {
    let parsed = match parse_args::<UpdateTaskArgs>(id.clone(), args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!(task_id = parsed.task_id, status = ?parsed.status, "MCP update_task");

    let has_update = parsed.status.is_some()
        || parsed.plan.is_some()
        || parsed.title.is_some()
        || parsed.description.is_some()
        || parsed.repo_path.is_some();

    if !has_update {
        return JsonRpcResponse::err(
            id,
            -32602,
            "At least one of status, plan, title, description, or repo_path must be provided",
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

    if matches!(status, Some(TaskStatus::Done)) {
        return JsonRpcResponse::err(
            id,
            -32602,
            "Cannot set status to done via MCP. Please ask the human operator to move the task to done from the TUI.",
        );
    }

    let mut patch = db::TaskPatch::new();
    if let Some(s) = status {
        patch = patch.status(s);
    }
    if let Some(ref p) = parsed.plan {
        patch = patch.plan(Some(p.as_str()));
    }
    if let Some(ref t) = parsed.title {
        patch = patch.title(t);
    }
    if let Some(ref d) = parsed.description {
        patch = patch.description(d);
    }
    if let Some(ref r) = parsed.repo_path {
        patch = patch.repo_path(r);
    }

    if let Err(e) = state.db.patch_task(TaskId(parsed.task_id), &patch) {
        return JsonRpcResponse::err(id, -32603, format!("Database error: {e}"));
    }

    state.notify();

    let mut updated = Vec::new();
    if let Some(ref s) = parsed.status { updated.push(format!("status={s}")); }
    if parsed.plan.is_some() { updated.push("plan".to_string()); }
    if parsed.title.is_some() { updated.push("title".to_string()); }
    if parsed.description.is_some() { updated.push("description".to_string()); }
    if parsed.repo_path.is_some() { updated.push("repo_path".to_string()); }

    JsonRpcResponse::ok(
        id,
        json!({"content": [{"type": "text", "text": format!("Task {} updated ({})", parsed.task_id, updated.join(", "))}]}),
    )
}

pub(super) fn handle_create_task(state: &McpState, id: Option<Value>, args: Value) -> JsonRpcResponse {
    let parsed = match parse_args::<CreateTaskWithEpicArgs>(id.clone(), args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!(title = %parsed.title, epic_id = ?parsed.epic_id, "MCP create_task");

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
        Ok(task_id) => {
            if let Some(eid) = parsed.epic_id {
                if let Err(e) = state.db.set_task_epic_id(task_id, Some(EpicId(eid))) {
                    return JsonRpcResponse::err(id, -32603, format!("Failed to link task to epic: {e}"));
                }
            }
            state.notify();
            JsonRpcResponse::ok(
                id,
                json!({"content": [{"type": "text", "text": format!("Task {task_id} created")}]}),
            )
        }
        Err(e) => JsonRpcResponse::err(id, -32603, format!("Database error: {e}")),
    }
}

pub(super) fn handle_get_task(state: &McpState, id: Option<Value>, args: Value) -> JsonRpcResponse {
    let parsed = match parse_args::<GetTaskArgs>(id.clone(), args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!(task_id = parsed.task_id, "MCP get_task");
    match state.db.get_task(TaskId(parsed.task_id)) {
        Ok(Some(task)) => {
            let mut text = format!(
                "Task {id}: {title}\nStatus: {status}\nRepo: {repo}\nDescription: {desc}",
                id = task.id,
                title = task.title,
                status = task.status.as_str(),
                repo = task.repo_path,
                desc = task.description,
            );
            if task.needs_input {
                text.push_str("\nNeeds input: yes");
            }
            JsonRpcResponse::ok(id, json!({"content": [{"type": "text", "text": text}]}))
        }
        Ok(None) => JsonRpcResponse::err(id, -32602, format!("Task {} not found", parsed.task_id)),
        Err(e) => JsonRpcResponse::err(id, -32603, format!("Database error: {e}")),
    }
}

pub(super) fn handle_list_tasks(state: &McpState, id: Option<Value>, args: Value) -> JsonRpcResponse {
    let parsed = match parse_args::<ListTasksArgs>(id.clone(), args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!(status = ?parsed.status, "MCP list_tasks");

    let status_filter: Option<Vec<TaskStatus>> = match parsed.status {
        Some(Value::String(ref s)) => match TaskStatus::parse(s) {
            Some(st) => Some(vec![st]),
            None => {
                return JsonRpcResponse::err(
                    id,
                    -32602,
                    format!("Unknown status: {s}. Valid values: backlog, ready, running, review, done"),
                );
            }
        },
        Some(Value::Array(ref arr)) => {
            let mut statuses = Vec::new();
            for v in arr {
                match v.as_str().and_then(TaskStatus::parse) {
                    Some(st) => statuses.push(st),
                    None => {
                        return JsonRpcResponse::err(
                            id,
                            -32602,
                            format!("Invalid status in array: {v}"),
                        );
                    }
                }
            }
            Some(statuses)
        }
        Some(_) => {
            return JsonRpcResponse::err(id, -32602, "status must be a string or array of strings");
        }
        None => None,
    };

    let tasks = match state.db.list_all() {
        Ok(t) => t,
        Err(e) => return JsonRpcResponse::err(id, -32603, format!("Database error: {e}")),
    };

    let filtered: Vec<_> = match &status_filter {
        Some(statuses) => tasks.into_iter().filter(|t| statuses.contains(&t.status)).collect(),
        None => tasks,
    };

    if filtered.is_empty() {
        return JsonRpcResponse::ok(
            id,
            json!({"content": [{"type": "text", "text": "No tasks found"}]}),
        );
    }

    let lines: Vec<String> = filtered
        .iter()
        .map(|t| {
            let desc_preview = if t.description.len() > 200 {
                let end = t.description.char_indices()
                    .take_while(|(i, _)| *i < 200)
                    .last()
                    .map(|(i, c)| i + c.len_utf8())
                    .unwrap_or(0);
                format!("{}...", &t.description[..end])
            } else {
                t.description.clone()
            };
            format!(
                "- [{}] {} ({}): {}",
                t.id, t.title, t.status.as_str(), desc_preview
            )
        })
        .collect();

    let text = lines.join("\n");
    JsonRpcResponse::ok(
        id,
        json!({"content": [{"type": "text", "text": text}]}),
    )
}

pub(super) fn handle_claim_task(state: &McpState, id: Option<Value>, args: Value) -> JsonRpcResponse {
    let parsed = match parse_args::<ClaimTaskArgs>(id.clone(), args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!(task_id = parsed.task_id, worktree = %parsed.worktree, "MCP claim_task");

    // 1. Fetch the task
    let task = match state.db.get_task(TaskId(parsed.task_id)) {
        Ok(Some(t)) => t,
        Ok(None) => {
            return JsonRpcResponse::err(id, -32602, format!("Task {} not found", parsed.task_id));
        }
        Err(e) => {
            return JsonRpcResponse::err(id, -32603, format!("Database error: {e}"));
        }
    };

    // 2. Validate status is backlog or ready
    if task.status != TaskStatus::Backlog && task.status != TaskStatus::Ready {
        return JsonRpcResponse::err(
            id,
            -32602,
            format!("Task {} is already {}", parsed.task_id, task.status.as_str()),
        );
    }

    // 3. Same-repo check: derive repo from worktree by stripping /.worktrees/<anything>
    let repo_from_worktree = parsed
        .worktree
        .find("/.worktrees/")
        .map(|idx| &parsed.worktree[..idx])
        .unwrap_or(&parsed.worktree);

    if repo_from_worktree != task.repo_path {
        return JsonRpcResponse::err(
            id,
            -32602,
            format!(
                "Repo mismatch: task belongs to {}, your worktree is in {}",
                task.repo_path, repo_from_worktree
            ),
        );
    }

    // 4. Atomically set status + worktree + tmux_window
    if let Err(e) = state.db.patch_task(
        TaskId(parsed.task_id),
        &db::TaskPatch::new()
            .status(TaskStatus::Running)
            .worktree(Some(&parsed.worktree))
            .tmux_window(Some(&parsed.tmux_window)),
    ) {
        return JsonRpcResponse::err(id, -32603, format!("Database error: {e}"));
    }

    JsonRpcResponse::ok(
        id,
        json!({"content": [{"type": "text", "text": format!("Task {} claimed: {}", parsed.task_id, task.title)}]}),
    )
}
