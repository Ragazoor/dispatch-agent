use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::db;
use crate::dispatch;
use crate::mcp::McpState;
use crate::models::{DispatchMode, EpicId, Task, TaskStatus};
use crate::service::{
    ClaimTaskParams, CreateTaskParams, ListTasksFilter, TaskService, UpdateTaskParams,
};

use super::types::{
    deserialize_flexible_i64, deserialize_optional_flexible_i64, parse_args,
    service_err_to_response, JsonRpcResponse,
};

// ---------------------------------------------------------------------------
// Typed argument structs (JSON-RPC layer)
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
    #[serde(default, deserialize_with = "deserialize_optional_flexible_i64")]
    pub(super) sort_order: Option<i64>,
    #[serde(default)]
    pub(super) pr_url: Option<String>,
    #[serde(default)]
    pub(super) tag: Option<String>,
    #[serde(default)]
    pub(super) sub_status: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_flexible_i64")]
    pub(super) epic_id: Option<i64>,
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
    #[serde(default, deserialize_with = "deserialize_optional_flexible_i64")]
    pub(super) epic_id: Option<i64>,
}

#[derive(Deserialize)]
pub(super) struct ClaimTaskArgs {
    #[serde(deserialize_with = "deserialize_flexible_i64")]
    pub(super) task_id: i64,
    pub(super) worktree: String,
    pub(super) tmux_window: String,
}

#[derive(Deserialize)]
pub(super) struct ReportUsageArgs {
    #[serde(deserialize_with = "deserialize_flexible_i64")]
    pub(super) task_id: i64,
    pub(super) cost_usd: f64,
    pub(super) input_tokens: i64,
    pub(super) output_tokens: i64,
    #[serde(default)]
    pub(super) cache_read_tokens: i64,
    #[serde(default)]
    pub(super) cache_write_tokens: i64,
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
    #[serde(default, deserialize_with = "deserialize_optional_flexible_i64")]
    pub(super) sort_order: Option<i64>,
    #[serde(default)]
    pub(super) tag: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct WrapUpArgs {
    #[serde(deserialize_with = "deserialize_flexible_i64")]
    pub(super) task_id: i64,
    pub(super) action: String, // "rebase" | "pr"
}

#[derive(Deserialize)]
pub(super) struct SendMessageArgs {
    #[serde(deserialize_with = "deserialize_flexible_i64")]
    pub(super) from_task_id: i64,
    #[serde(deserialize_with = "deserialize_flexible_i64")]
    pub(super) to_task_id: i64,
    pub(super) body: String,
}

// ---------------------------------------------------------------------------
// Response formatting (presentation layer)
// ---------------------------------------------------------------------------

fn build_epic_titles(state: &McpState) -> HashMap<EpicId, String> {
    state
        .db
        .list_epics()
        .unwrap_or_default()
        .into_iter()
        .map(|e| (e.id, e.title))
        .collect()
}

fn format_task_detail(task: &Task, epic_titles: &HashMap<EpicId, String>) -> String {
    let mut text = format!(
        "Task {id}: {title}\nStatus: {status}\nRepo: {repo}\nDescription: {desc}",
        id = task.id,
        title = task.title,
        status = task.status.as_str(),
        repo = task.repo_path,
        desc = task.description,
    );
    text.push_str(&format!("\nSub-status: {}", task.sub_status.as_str()));
    if let Some(epic_id) = task.epic_id {
        let epic_label = match epic_titles.get(&epic_id) {
            Some(title) => format!("{title} (#{epic_id})"),
            None => format!("#{epic_id}"),
        };
        text.push_str(&format!("\nEpic: {epic_label}"));
    }
    if let Some(ref tag) = task.tag {
        text.push_str(&format!("\nTag: {tag}"));
    }
    if let Some(ref plan) = task.plan {
        text.push_str(&format!("\nPlan: {plan}"));
    }
    if let Some(ref pr_url) = task.pr_url {
        text.push_str(&format!("\nPR: {pr_url}"));
    }
    if let Some(ref worktree) = task.worktree {
        text.push_str(&format!("\nWorktree: {worktree}"));
    }
    if let Some(ref tmux_window) = task.tmux_window {
        text.push_str(&format!("\nTmux window: {tmux_window}"));
    }
    if let Some(sort_order) = task.sort_order {
        text.push_str(&format!("\nSort order: {sort_order}"));
    }
    text.push_str(&format!(
        "\nCreated: {}",
        task.created_at.format("%Y-%m-%d %H:%M:%S UTC")
    ));
    text.push_str(&format!(
        "\nUpdated: {}",
        task.updated_at.format("%Y-%m-%d %H:%M:%S UTC")
    ));
    text
}

fn format_task_line(t: &Task, epic_titles: &HashMap<EpicId, String>) -> String {
    let desc_preview = if t.description.len() > 200 {
        let end = t
            .description
            .char_indices()
            .take_while(|(i, _)| *i < 200)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        format!("{}...", &t.description[..end])
    } else {
        t.description.clone()
    };
    let plan_indicator = if t.plan.is_some() { " [plan]" } else { "" };
    let tag_indicator = match t.tag {
        Some(tag) => format!(" [{}]", tag.as_str()),
        None => String::new(),
    };
    let epic_indicator = match t.epic_id {
        Some(eid) => match epic_titles.get(&eid) {
            Some(title) => format!(" (epic:{eid} {title})"),
            None => format!(" (epic:{eid})"),
        },
        None => String::new(),
    };
    format!(
        "- [{}] {} ({}/{}){}{}{}: {}",
        t.id,
        t.title,
        t.status.as_str(),
        t.sub_status.as_str(),
        plan_indicator,
        tag_indicator,
        epic_indicator,
        desc_preview
    )
}

// ---------------------------------------------------------------------------
// Task tool handlers (thin wrappers over TaskService)
// ---------------------------------------------------------------------------

pub(super) fn handle_update_task(
    state: &McpState,
    id: Option<Value>,
    args: Value,
) -> JsonRpcResponse {
    let parsed = match parse_args::<UpdateTaskArgs>(&id, args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!(task_id = parsed.task_id, status = ?parsed.status, "MCP update_task");

    let params = UpdateTaskParams {
        task_id: parsed.task_id,
        status: parsed.status.clone(),
        plan: parsed.plan,
        title: parsed.title,
        description: parsed.description,
        repo_path: parsed.repo_path,
        sort_order: parsed.sort_order,
        pr_url: parsed.pr_url,
        tag: parsed.tag,
        sub_status: parsed.sub_status,
        epic_id: parsed.epic_id,
    };
    let field_names = params.updated_field_names();

    let svc = TaskService::new(state.db.clone());
    match svc.update_task(params) {
        Ok(task_id) => {
            state.notify();
            JsonRpcResponse::ok(
                id,
                json!({"content": [{"type": "text", "text": format!("Task {} updated ({})", task_id, field_names.join(", "))}]}),
            )
        }
        Err(e) => service_err_to_response(id, e),
    }
}

pub(super) fn handle_create_task(
    state: &McpState,
    id: Option<Value>,
    args: Value,
) -> JsonRpcResponse {
    let parsed = match parse_args::<CreateTaskWithEpicArgs>(&id, args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!(title = %parsed.title, epic_id = ?parsed.epic_id, "MCP create_task");

    let svc = TaskService::new(state.db.clone());
    match svc.create_task(CreateTaskParams {
        title: parsed.title,
        description: parsed.description,
        repo_path: parsed.repo_path,
        plan: parsed.plan,
        epic_id: parsed.epic_id,
        sort_order: parsed.sort_order,
        tag: parsed.tag,
    }) {
        Ok(task_id) => {
            state.notify();
            JsonRpcResponse::ok(
                id,
                json!({"content": [{"type": "text", "text": format!("Task {task_id} created")}]}),
            )
        }
        Err(e) => service_err_to_response(id, e),
    }
}

pub(super) fn handle_get_task(state: &McpState, id: Option<Value>, args: Value) -> JsonRpcResponse {
    let parsed = match parse_args::<GetTaskArgs>(&id, args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!(task_id = parsed.task_id, "MCP get_task");

    let svc = TaskService::new(state.db.clone());
    match svc.get_task(parsed.task_id) {
        Ok(task) => {
            let epic_titles = build_epic_titles(state);
            let text = format_task_detail(&task, &epic_titles);
            JsonRpcResponse::ok(id, json!({"content": [{"type": "text", "text": text}]}))
        }
        Err(e) => service_err_to_response(id, e),
    }
}

pub(super) fn handle_list_tasks(
    state: &McpState,
    id: Option<Value>,
    args: Value,
) -> JsonRpcResponse {
    let parsed = match parse_args::<ListTasksArgs>(&id, args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!(status = ?parsed.status, "MCP list_tasks");

    // Parse status filter (supports string or array) — this is JSON-RPC parsing logic
    let status_filter: Option<Vec<TaskStatus>> = match parsed.status {
        Some(Value::String(ref s)) => match TaskStatus::parse(s) {
            Some(st) => Some(vec![st]),
            None => {
                return JsonRpcResponse::err(
                    id,
                    -32602,
                    format!("Unknown status: {s}. Valid values: backlog, running, review, done"),
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

    let svc = TaskService::new(state.db.clone());
    match svc.list_tasks(ListTasksFilter {
        statuses: status_filter,
        epic_id: parsed.epic_id.map(EpicId),
    }) {
        Ok(filtered) => {
            if filtered.is_empty() {
                return JsonRpcResponse::ok(
                    id,
                    json!({"content": [{"type": "text", "text": "No tasks found"}]}),
                );
            }
            let epic_titles = build_epic_titles(state);
            let lines: Vec<String> = filtered
                .iter()
                .map(|t| format_task_line(t, &epic_titles))
                .collect();
            JsonRpcResponse::ok(
                id,
                json!({"content": [{"type": "text", "text": lines.join("\n")}]}),
            )
        }
        Err(e) => service_err_to_response(id, e),
    }
}

pub(super) fn handle_claim_task(
    state: &McpState,
    id: Option<Value>,
    args: Value,
) -> JsonRpcResponse {
    let parsed = match parse_args::<ClaimTaskArgs>(&id, args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!(task_id = parsed.task_id, worktree = %parsed.worktree, "MCP claim_task");

    let svc = TaskService::new(state.db.clone());
    match svc.claim_task(ClaimTaskParams {
        task_id: parsed.task_id,
        worktree: parsed.worktree,
        tmux_window: parsed.tmux_window,
    }) {
        Ok(task) => {
            state.notify();
            JsonRpcResponse::ok(
                id,
                json!({"content": [{"type": "text", "text": format!("Task {} claimed: {}", parsed.task_id, task.title)}]}),
            )
        }
        Err(e) => service_err_to_response(id, e),
    }
}

pub(super) async fn handle_wrap_up(
    state: &McpState,
    id: Option<Value>,
    args: Value,
) -> JsonRpcResponse {
    let parsed = match parse_args::<WrapUpArgs>(&id, args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!(task_id = parsed.task_id, action = %parsed.action, "MCP wrap_up");

    let svc = TaskService::new(state.db.clone());
    let task = match svc.validate_wrap_up(parsed.task_id, &parsed.action) {
        Ok(t) => t,
        Err(e) => return service_err_to_response(id, e),
    };

    let worktree = task
        .worktree
        .clone()
        .expect("validate_wrap_up guarantees worktree is Some");

    let branch = match dispatch::branch_from_worktree(&worktree) {
        Some(b) => b,
        None => {
            return JsonRpcResponse::err(
                id,
                -32602,
                format!("Cannot derive branch from worktree: {worktree}"),
            )
        }
    };

    let repo_path = task.repo_path.clone();
    let tmux_window = task.tmux_window.clone();
    let runner = state.runner.clone();
    let notify_tx = state.notify_tx.clone();
    let task_id = task.id;

    // Gather auto-dispatch context for epic subtasks.
    let next_epic_task =
        task.epic_id
            .and_then(|epic_id| match state.db.list_tasks_for_epic(epic_id) {
                Ok(tasks) => tasks.into_iter().find(|t| t.status == TaskStatus::Backlog),
                Err(e) => {
                    tracing::warn!(
                        epic_id = epic_id.0,
                        "auto-dispatch: failed to list epic tasks: {e}"
                    );
                    None
                }
            });

    let auto_dispatch_msg = next_epic_task
        .as_ref()
        .map(|t| {
            format!(
                "; next epic task #{} '{}' will be dispatched",
                t.id, t.title
            )
        })
        .unwrap_or_default();

    match parsed.action.as_str() {
        "rebase" => {
            let db = state.db.clone();
            let rebase_runner = runner.clone();
            let rebase_result = match tokio::task::spawn_blocking(move || {
                tracing::info!(task_id = task_id.0, %branch, "MCP wrap_up rebase starting");
                dispatch::finish_task(&repo_path, &worktree, &branch, None, &*rebase_runner)
            })
            .await
            {
                Ok(r) => r,
                Err(e) => return JsonRpcResponse::err(id, -32603, format!("internal error: {e}")),
            };

            match rebase_result {
                Ok(()) => {
                    let patch = db::TaskPatch::new().status(TaskStatus::Done);
                    if let Err(e) = db.patch_task(task_id, &patch) {
                        tracing::warn!(
                            task_id = task_id.0,
                            "MCP wrap_up: failed to set task to done: {e}"
                        );
                    }
                    if let Some(epic_id) = task.epic_id {
                        let _ = db.recalculate_epic_status(epic_id);
                    }
                    let ad_runner = state.runner.clone();
                    tokio::task::spawn_blocking(move || {
                        if let Some(window) = &tmux_window {
                            let _ = crate::tmux::kill_window(window, &*ad_runner);
                        }
                        auto_dispatch_next(next_epic_task, &*db, &*ad_runner);
                        if let Some(tx) = notify_tx {
                            let _ = tx.send(crate::mcp::McpEvent::Refresh);
                        }
                    });
                    JsonRpcResponse::ok(
                        id,
                        json!({"content": [{"type": "text", "text": format!("wrap_up complete (task {}, action: rebase){auto_dispatch_msg}", parsed.task_id)}]}),
                    )
                }
                Err(e) => {
                    if let Some(tx) = notify_tx {
                        let _ = tx.send(crate::mcp::McpEvent::Refresh);
                    }
                    JsonRpcResponse::err(id, -32603, format!("wrap_up failed: {e}"))
                }
            }
        }
        "pr" => {
            let db = state.db.clone();
            let pr_runner = runner.clone();
            let title = task.title.clone();
            let description = task.description.clone();
            let pr_result = match tokio::task::spawn_blocking(move || {
                tracing::info!(task_id = task_id.0, %branch, "MCP wrap_up pr starting");
                dispatch::create_pr(&repo_path, &branch, &title, &description, &*pr_runner)
            })
            .await
            {
                Ok(r) => r,
                Err(e) => return JsonRpcResponse::err(id, -32603, format!("internal error: {e}")),
            };

            match pr_result {
                Ok(result) => {
                    let patch = db::TaskPatch::new()
                        .status(TaskStatus::Review)
                        .pr_url(Some(result.pr_url.as_str()));
                    if let Err(e) = db.patch_task(task_id, &patch) {
                        tracing::warn!(
                            task_id = task_id.0,
                            "MCP wrap_up: failed to save PR fields: {e}"
                        );
                    }
                    if let Some(epic_id) = task.epic_id {
                        let _ = db.recalculate_epic_status(epic_id);
                    }
                    let pr_url = result.pr_url.clone();
                    let ad_runner = state.runner.clone();
                    tokio::task::spawn_blocking(move || {
                        if let Some(window) = &tmux_window {
                            let review_cmd = format!("/code-review {}", result.pr_url);
                            if let Err(e) = crate::tmux::send_keys(window, &review_cmd, &*ad_runner)
                            {
                                tracing::warn!(
                                    task_id = task_id.0,
                                    "Failed to inject review command: {e}"
                                );
                            }
                        }
                        auto_dispatch_next(next_epic_task, &*db, &*ad_runner);
                        if let Some(tx) = notify_tx {
                            let _ = tx.send(crate::mcp::McpEvent::Refresh);
                        }
                    });
                    JsonRpcResponse::ok(
                        id,
                        json!({"content": [{"type": "text", "text": format!("wrap_up complete (task {}, action: pr, pr_url: {}){auto_dispatch_msg}", parsed.task_id, pr_url)}]}),
                    )
                }
                Err(e) => {
                    if let Some(tx) = notify_tx {
                        let _ = tx.send(crate::mcp::McpEvent::Refresh);
                    }
                    JsonRpcResponse::err(id, -32603, format!("wrap_up failed: {e}"))
                }
            }
        }
        _ => unreachable!(),
    }
}

pub(super) fn handle_send_message(
    state: &McpState,
    id: Option<Value>,
    args: Value,
) -> JsonRpcResponse {
    let parsed: SendMessageArgs = match parse_args(&id, args) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let svc = TaskService::new(state.db.clone());
    let (from_task, to_task) =
        match svc.validate_send_message(parsed.from_task_id, parsed.to_task_id) {
            Ok(pair) => pair,
            Err(e) => return service_err_to_response(id, e),
        };

    let worktree = to_task.worktree.as_ref().expect("validated by service");
    let tmux_window = to_task.tmux_window.as_ref().expect("validated by service");

    // Write message to a uniquely-named file in target's worktree
    let messages_dir = format!("{worktree}/.claude-messages");
    if let Err(e) = std::fs::create_dir_all(&messages_dir) {
        return JsonRpcResponse::err(
            id,
            -32603,
            format!("failed to create messages dir: {e}"),
        );
    }
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let filename = format!("{}-{}.md", from_task.id.0, timestamp);
    let message_path = format!("{messages_dir}/{filename}");

    let message_content = format!(
        "[Message from task {}: \"{}\"]\n{}",
        from_task.id.0, from_task.title, parsed.body
    );
    if let Err(e) = std::fs::write(&message_path, &message_content) {
        return JsonRpcResponse::err(id, -32603, format!("failed to write message file: {e}"));
    }

    // Inject notification into the target's tmux window
    let notification = format!(
        "You received a message from task {}. Read .claude-messages/{} for the full content, then delete the file.",
        from_task.id.0, filename
    );
    if let Err(e) = crate::tmux::send_keys(tmux_window, &notification, &*state.runner) {
        let _ = std::fs::remove_file(&message_path);
        return JsonRpcResponse::err(
            id,
            -32603,
            format!("failed to send notification to target agent: {e}"),
        );
    }

    state.notify_message_sent(to_task.id);

    tracing::info!(
        from_task_id = parsed.from_task_id,
        to_task_id = parsed.to_task_id,
        "message sent between agents"
    );

    JsonRpcResponse::ok(
        id,
        json!({"content": [{"type": "text", "text": format!(
            "Message sent to task {} ({})",
            to_task.id.0, to_task.title
        )}]}),
    )
}

/// Auto-dispatch the next epic subtask from main.
fn auto_dispatch_next(
    next_task: Option<Task>,
    db: &dyn db::TaskStore,
    runner: &dyn crate::process::ProcessRunner,
) {
    let Some(next_task) = next_task else { return };
    let next_id = next_task.id;

    tracing::info!(
        next_task_id = next_id.0,
        has_plan = next_task.plan.is_some(),
        "auto-dispatching next epic subtask"
    );

    let epic_ctx = dispatch::EpicContext::from_db(&next_task, db);
    let result = match DispatchMode::for_task(&next_task) {
        DispatchMode::Dispatch => {
            dispatch::dispatch_chained_agent(&next_task, runner, epic_ctx.as_ref())
        }
        DispatchMode::Brainstorm => {
            dispatch::brainstorm_chained_agent(&next_task, runner, epic_ctx.as_ref())
        }
        DispatchMode::Plan => dispatch::plan_chained_agent(&next_task, runner, epic_ctx.as_ref()),
    };

    match result {
        Ok(dispatch_result) => {
            let patch = db::TaskPatch::new()
                .status(TaskStatus::Running)
                .worktree(Some(&dispatch_result.worktree_path))
                .tmux_window(Some(&dispatch_result.tmux_window));
            if let Err(e) = db.patch_task(next_id, &patch) {
                tracing::warn!(
                    task_id = next_id.0,
                    "auto-dispatch: failed to update task: {e}"
                );
            }
            if let Some(epic_id) = next_task.epic_id {
                let _ = db.recalculate_epic_status(epic_id);
            }
        }
        Err(e) => {
            tracing::warn!(task_id = next_id.0, "auto-dispatch: dispatch failed: {e:#}");
        }
    }
}

pub(super) fn handle_report_usage(
    state: &McpState,
    id: Option<Value>,
    args: Value,
) -> JsonRpcResponse {
    let parsed = match parse_args::<ReportUsageArgs>(&id, args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!(
        task_id = parsed.task_id,
        cost_usd = parsed.cost_usd,
        "MCP report_usage"
    );

    let svc = TaskService::new(state.db.clone());
    match svc.report_usage(
        parsed.task_id,
        &crate::models::UsageReport {
            cost_usd: parsed.cost_usd,
            input_tokens: parsed.input_tokens,
            output_tokens: parsed.output_tokens,
            cache_read_tokens: parsed.cache_read_tokens,
            cache_write_tokens: parsed.cache_write_tokens,
        },
    ) {
        Ok(()) => {
            state.notify();
            JsonRpcResponse::ok(
                id,
                json!({"content": [{"type": "text", "text": format!("Usage recorded for task {}", parsed.task_id)}]}),
            )
        }
        Err(e) => service_err_to_response(id, e),
    }
}
