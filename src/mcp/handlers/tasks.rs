use serde::Deserialize;
use serde_json::{json, Value};

use crate::db;
use crate::dispatch;
use crate::mcp::McpState;
use crate::models::{DispatchMode, EpicId, SubStatus, Task, TaskId, TaskStatus, TaskTag, UsageReport};

use super::types::{
    deserialize_flexible_i64, deserialize_optional_flexible_i64, parse_args, JsonRpcResponse,
};

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
    #[serde(default, deserialize_with = "deserialize_optional_flexible_i64")]
    pub(super) sort_order: Option<i64>,
    #[serde(default)]
    pub(super) pr_url: Option<String>,
    #[serde(default)]
    pub(super) tag: Option<String>,
    #[serde(default)]
    pub(super) sub_status: Option<String>,
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
// Response formatting
// ---------------------------------------------------------------------------

fn format_task_detail(task: &Task) -> String {
    let mut text = format!(
        "Task {id}: {title}\nStatus: {status}\nRepo: {repo}\nDescription: {desc}",
        id = task.id,
        title = task.title,
        status = task.status.as_str(),
        repo = task.repo_path,
        desc = task.description,
    );
    text.push_str(&format!("\nSub-status: {}", task.sub_status.as_str()));
    if let Some(ref epic_id) = task.epic_id {
        text.push_str(&format!("\nEpic: {epic_id}"));
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

fn format_task_line(t: &Task) -> String {
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
        Some(eid) => format!(" (epic:{eid})"),
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
// Task tool handlers
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

    let has_update = parsed.status.is_some()
        || parsed.plan.is_some()
        || parsed.title.is_some()
        || parsed.description.is_some()
        || parsed.repo_path.is_some()
        || parsed.sort_order.is_some()
        || parsed.pr_url.is_some()
        || parsed.tag.is_some()
        || parsed.sub_status.is_some();

    if !has_update {
        return JsonRpcResponse::err(
            id,
            -32602,
            "At least one of status, plan, title, description, repo_path, sort_order, pr_url, tag, or sub_status must be provided",
        );
    }

    let status = if let Some(ref status_str) = parsed.status {
        match TaskStatus::parse(status_str) {
            Some(s) => Some(s),
            None => {
                return JsonRpcResponse::err(
                    id,
                    -32602,
                    format!("Unknown status: {status_str}. Valid values: backlog, running, review"),
                )
            }
        }
    } else {
        None
    };

    if matches!(status, Some(TaskStatus::Done | TaskStatus::Archived)) {
        return JsonRpcResponse::err(
            id,
            -32602,
            "Cannot set status to done or archived via MCP. Please ask the human operator to manage this from the TUI.",
        );
    }

    let expanded_repo_path = parsed.repo_path.as_deref().map(crate::models::expand_tilde);

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
    if let Some(ref r) = expanded_repo_path {
        patch = patch.repo_path(r);
    }
    if let Some(so) = parsed.sort_order {
        patch = patch.sort_order(Some(so));
    }
    if let Some(ref url) = parsed.pr_url {
        patch = patch.pr_url(Some(url.as_str()));
    }
    if let Some(ref t) = parsed.tag {
        match TaskTag::parse(t) {
            Some(tag) => { patch = patch.tag(Some(tag)); }
            None => return JsonRpcResponse::err(
                id, -32602,
                format!("Invalid tag: {t}. Valid values: bug, feature, chore, epic"),
            ),
        }
    }

    if let Some(ref ss_str) = parsed.sub_status {
        let ss = match SubStatus::parse(ss_str) {
            Some(ss) => ss,
            None => return JsonRpcResponse::err(
                id, -32602,
                format!("Invalid sub_status: {ss_str}. Valid values: none, active, needs_input, stale, crashed, awaiting_review, changes_requested, approved"),
            ),
        };
        // Validate against current (or new) status
        let effective_status = parsed
            .status
            .as_deref()
            .and_then(TaskStatus::parse)
            .or_else(|| {
                state
                    .db
                    .get_task(TaskId(parsed.task_id))
                    .ok()
                    .flatten()
                    .map(|t| t.status)
            });
        if let Some(eff) = effective_status {
            if !ss.is_valid_for(eff) {
                return JsonRpcResponse::err(
                    id,
                    -32602,
                    format!(
                        "sub_status '{}' is not valid for status '{}'",
                        ss_str,
                        eff.as_str()
                    ),
                );
            }
        }
        patch = patch.sub_status(ss);
    }

    if let Err(e) = state.db.patch_task(TaskId(parsed.task_id), &patch) {
        return JsonRpcResponse::err(id, -32603, format!("Database error: {e}"));
    }

    // Recalculate parent epic status if subtask status changed
    if parsed.status.is_some() {
        if let Ok(Some(task)) = state.db.get_task(TaskId(parsed.task_id)) {
            if let Some(epic_id) = task.epic_id {
                let _ = state.db.recalculate_epic_status(epic_id);
            }
        }
    }

    state.notify();

    let mut updated = Vec::new();
    if let Some(ref s) = parsed.status {
        updated.push(format!("status={s}"));
    }
    if parsed.plan.is_some() {
        updated.push("plan".to_string());
    }
    if parsed.title.is_some() {
        updated.push("title".to_string());
    }
    if parsed.description.is_some() {
        updated.push("description".to_string());
    }
    if parsed.repo_path.is_some() {
        updated.push("repo_path".to_string());
    }
    if parsed.sort_order.is_some() {
        updated.push("sort_order".to_string());
    }
    if parsed.pr_url.is_some() {
        updated.push("pr_url".to_string());
    }
    if parsed.tag.is_some() {
        updated.push("tag".to_string());
    }
    if parsed.sub_status.is_some() {
        updated.push("sub_status".to_string());
    }

    JsonRpcResponse::ok(
        id,
        json!({"content": [{"type": "text", "text": format!("Task {} updated ({})", parsed.task_id, updated.join(", "))}]}),
    )
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

    let repo_path = crate::models::expand_tilde(&parsed.repo_path);

    let plan = parsed.plan.as_deref().map(|p| {
        std::fs::canonicalize(p)
            .map(|abs| abs.to_string_lossy().into_owned())
            .unwrap_or_else(|_| p.to_string())
    });

    let status = TaskStatus::Backlog;

    match state.db.create_task(
        &parsed.title,
        &parsed.description,
        &repo_path,
        plan.as_deref(),
        status,
    ) {
        Ok(task_id) => {
            if let Some(eid) = parsed.epic_id {
                if let Err(e) = state.db.set_task_epic_id(task_id, Some(EpicId(eid))) {
                    return JsonRpcResponse::err(
                        id,
                        -32603,
                        format!("Failed to link task to epic: {e}"),
                    );
                }
            }
            if let Some(so) = parsed.sort_order {
                let _ = state
                    .db
                    .patch_task(task_id, &db::TaskPatch::new().sort_order(Some(so)));
            }
            if let Some(ref t) = parsed.tag {
                match TaskTag::parse(t) {
                    Some(tag) => {
                        let _ = state
                            .db
                            .patch_task(task_id, &db::TaskPatch::new().tag(Some(tag)));
                    }
                    None => return JsonRpcResponse::err(
                        id, -32602,
                        format!("Invalid tag: {t}. Valid values: bug, feature, chore, epic"),
                    ),
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
    let parsed = match parse_args::<GetTaskArgs>(&id, args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!(task_id = parsed.task_id, "MCP get_task");
    match state.db.get_task(TaskId(parsed.task_id)) {
        Ok(Some(task)) => {
            let text = format_task_detail(&task);
            JsonRpcResponse::ok(id, json!({"content": [{"type": "text", "text": text}]}))
        }
        Ok(None) => JsonRpcResponse::err(id, -32602, format!("Task {} not found", parsed.task_id)),
        Err(e) => JsonRpcResponse::err(id, -32603, format!("Database error: {e}")),
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

    let tasks = match state.db.list_all() {
        Ok(t) => t,
        Err(e) => return JsonRpcResponse::err(id, -32603, format!("Database error: {e}")),
    };

    let filtered: Vec<_> = match &status_filter {
        Some(statuses) => tasks
            .into_iter()
            .filter(|t| statuses.contains(&t.status))
            .collect(),
        None => tasks,
    };

    if filtered.is_empty() {
        return JsonRpcResponse::ok(
            id,
            json!({"content": [{"type": "text", "text": "No tasks found"}]}),
        );
    }

    let lines: Vec<String> = filtered.iter().map(format_task_line).collect();

    let text = lines.join("\n");
    JsonRpcResponse::ok(id, json!({"content": [{"type": "text", "text": text}]}))
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

    // 2. Validate status is backlog
    if task.status != TaskStatus::Backlog {
        return JsonRpcResponse::err(
            id,
            -32602,
            format!(
                "Task {} is already {}",
                parsed.task_id,
                task.status.as_str()
            ),
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

    state.notify();
    JsonRpcResponse::ok(
        id,
        json!({"content": [{"type": "text", "text": format!("Task {} claimed: {}", parsed.task_id, task.title)}]}),
    )
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

    if parsed.action != "rebase" && parsed.action != "pr" {
        return JsonRpcResponse::err(
            id,
            -32602,
            format!(
                "Unknown action: {}. Valid values: rebase, pr",
                parsed.action
            ),
        );
    }

    let task = match state.db.get_task(TaskId(parsed.task_id)) {
        Ok(Some(t)) => t,
        Ok(None) => {
            return JsonRpcResponse::err(id, -32602, format!("Task {} not found", parsed.task_id))
        }
        Err(e) => return JsonRpcResponse::err(id, -32603, format!("Database error: {e}")),
    };

    if !dispatch::is_wrappable(&task) {
        return JsonRpcResponse::err(
            id,
            -32602,
            format!(
                "Task {} cannot be wrapped up. Requires Running or Review status with a worktree.",
                parsed.task_id
            ),
        );
    }

    let worktree = task
        .worktree
        .clone()
        .expect("is_wrappable guarantees worktree is Some");

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
    // If this task belongs to an epic, find the next backlog subtask to chain-dispatch.
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
                dispatch::finish_task(
                    &repo_path,
                    &worktree,
                    &branch,
                    None, // Don't kill tmux yet -- need to return response first
                    &*rebase_runner,
                )
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
                    // Fire-and-forget: kill tmux, auto-dispatch, notify
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
                    // Save before closure moves result
                    let pr_url = result.pr_url.clone();
                    // Fire-and-forget: inject code review, auto-dispatch, notify
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

    let from_task = match state.db.get_task(TaskId(parsed.from_task_id)) {
        Ok(Some(t)) => t,
        Ok(None) => {
            return JsonRpcResponse::err(
                id,
                -32602,
                format!("sender task {} not found", parsed.from_task_id),
            );
        }
        Err(e) => {
            return JsonRpcResponse::err(id, -32603, format!("failed to look up sender: {e}"));
        }
    };

    let to_task = match state.db.get_task(TaskId(parsed.to_task_id)) {
        Ok(Some(t)) => t,
        Ok(None) => {
            return JsonRpcResponse::err(
                id,
                -32602,
                format!("target task {} not found", parsed.to_task_id),
            );
        }
        Err(e) => {
            return JsonRpcResponse::err(id, -32603, format!("failed to look up target: {e}"));
        }
    };

    let Some(ref worktree) = to_task.worktree else {
        return JsonRpcResponse::err(
            id,
            -32602,
            format!("target task {} has no worktree (not running)", parsed.to_task_id),
        );
    };

    let Some(ref tmux_window) = to_task.tmux_window else {
        return JsonRpcResponse::err(
            id,
            -32602,
            format!("target task {} has no tmux window (not running)", parsed.to_task_id),
        );
    };

    // Write message to file in target's worktree
    let message_content = format!(
        "[Message from task {}: \"{}\"]\n{}",
        from_task.id.0, from_task.title, parsed.body
    );
    let message_path = format!("{worktree}/.claude-message");
    if let Err(e) = std::fs::write(&message_path, &message_content) {
        return JsonRpcResponse::err(
            id,
            -32603,
            format!("failed to write message file: {e}"),
        );
    }

    // Inject a short notification prompt into the target's tmux window
    let notification = format!(
        "You received a message from task {}. Read .claude-message for the full content, then delete the file.",
        from_task.id.0
    );
    if let Err(e) = crate::tmux::send_keys(tmux_window, &notification, &*state.runner) {
        // Clean up the file if we can't deliver
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
/// Called inside `spawn_blocking` after the rebase/PR work completes.
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

    let result = match DispatchMode::for_task(&next_task) {
        DispatchMode::Dispatch => dispatch::dispatch_chained_agent(&next_task, runner),
        DispatchMode::Brainstorm => dispatch::brainstorm_chained_agent(&next_task, runner),
        DispatchMode::Plan => dispatch::plan_chained_agent(&next_task, runner),
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

    match state.db.get_task(TaskId(parsed.task_id)) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return JsonRpcResponse::err(id, -32602, format!("Task {} not found", parsed.task_id))
        }
        Err(e) => return JsonRpcResponse::err(id, -32603, format!("Database error: {e}")),
    }

    match state.db.report_usage(
        TaskId(parsed.task_id),
        &UsageReport {
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
        Err(e) => JsonRpcResponse::err(id, -32603, format!("Failed to record usage: {e}")),
    }
}
