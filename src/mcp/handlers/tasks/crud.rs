use std::collections::HashMap;

use serde_json::{json, Value};

use crate::mcp::identity::CallerIdentity;
use crate::mcp::McpState;
use crate::models::{EpicId, TaskId, TaskStatus};
use crate::service::{
    CreateTaskParams, FieldUpdate, ListTasksFilter, ServiceError, UpdateTaskParams, UrlUpdate,
};

use super::{
    fetch_caller_task, parse_args, service_err_to_response, CreateTaskWithEpicArgs, GetTaskArgs,
    JsonRpcResponse, ListTasksArgs, QueryUsageArgs, StatusFilter, UpdateTaskArgs,
};

pub(crate) async fn handle_update_task(
    state: &McpState,
    id: Option<Value>,
    _identity: &CallerIdentity,
    args: Value,
) -> JsonRpcResponse {
    let parsed = match parse_args::<UpdateTaskArgs>(&id, args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!(task_id = parsed.task_id, status = ?parsed.status, "MCP update_task");

    // MCP-specific restriction: agents cannot set status to done or archived
    if matches!(parsed.status, Some(TaskStatus::Done | TaskStatus::Archived)) {
        return service_err_to_response(
            id,
            ServiceError::Validation(
                "Cannot set status to done or archived via MCP. Please ask the human operator to manage this from the TUI.".into(),
            ),
        );
    }

    // MCP tag semantics: absent = leave untouched, present = set. There is no
    // clear-via-MCP, so map `Some(t)` to `Some(Some(t))` and `None` to `None`.
    let mut params = UpdateTaskParams::for_task(TaskId(parsed.task_id))
        .tag(parsed.tag.map(Some))
        .base_branch(parsed.base_branch);
    if let Some(status) = parsed.status {
        params = params.status(status);
    }
    if let Some(plan_path) = parsed.plan_path {
        params = params.plan_path(FieldUpdate::Set(plan_path));
    }
    if let Some(title) = parsed.title {
        params = params.title(title);
    }
    if let Some(description) = parsed.description {
        params = params.description(description);
    }
    if let Some(repo_path) = parsed.repo_path {
        params = params.repo_path(repo_path);
    }
    if let Some(sort_order) = parsed.sort_order {
        params = params.sort_order(sort_order);
    }
    match parsed.url {
        // Empty string clears the URL (legacy clear convention).
        Some(ref u) if u.is_empty() => {
            params = params.url(UrlUpdate::Clear);
        }
        Some(u) => {
            let type_str = match parsed.url_type {
                Some(t) => t,
                None => {
                    return service_err_to_response(
                        id,
                        ServiceError::Validation(
                            "url_type is required when url is set (one of: pr, security_alert, issue, other)".into(),
                        ),
                    )
                }
            };
            let url_type = match crate::models::UrlType::parse(&type_str) {
                Some(t) => t,
                None => {
                    return service_err_to_response(
                        id,
                        ServiceError::Validation(format!(
                            "unknown url_type '{type_str}' (expected one of: pr, security_alert, issue, other)"
                        )),
                    )
                }
            };
            params = params.url(UrlUpdate::Set(crate::models::TaskUrl::new(u, url_type)));
        }
        None => {}
    }
    if let Some(sub_status) = parsed.sub_status {
        params = params.sub_status(sub_status);
    }
    if let Some(epic_id) = parsed.epic_id {
        params = params.epic_id(EpicId(epic_id));
    }
    if let Some(mode) = parsed.wrap_up_mode {
        params = params.wrap_up_mode(mode);
    }
    if let Some(value) = parsed.auto_run_plan {
        params = params.auto_run_plan(value);
    }
    let fields_display = params.updated_field_names().join(", ");

    match state.task_svc.update_task(params).await {
        Ok(result) => {
            state.notify_task_changed(TaskId(parsed.task_id));
            let nudge = if result.was_pr_finalisation {
                super::reflection_nudge(&*state.db).await
            } else {
                ""
            };
            JsonRpcResponse::ok(
                id,
                json!({"content": [{"type": "text", "text": format!("Task {} updated ({}){}", result.task_id, fields_display, nudge)}]}),
            )
        }
        Err(e) => service_err_to_response(id, e),
    }
}

pub(crate) async fn handle_create_task(
    state: &McpState,
    id: Option<Value>,
    identity: &CallerIdentity,
    args: Value,
) -> JsonRpcResponse {
    let parsed = match parse_args::<CreateTaskWithEpicArgs>(&id, args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!(
        title = %parsed.title,
        epic_id = ?parsed.epic_id,
        identity = ?identity,
        "MCP create_task"
    );

    let effective_epic_id = match identity {
        CallerIdentity::Task(caller_id) => {
            let caller = match fetch_caller_task(&*state.db, &id, *caller_id).await {
                Ok(t) => t,
                Err(resp) => return resp,
            };
            match parsed.epic_id {
                Some(inner) => inner.map(EpicId),
                None => caller.epic_id,
            }
        }
        CallerIdentity::Session => parsed.epic_id.and_then(|inner| inner.map(EpicId)),
    };

    match state
        .task_svc
        .create_task(CreateTaskParams {
            title: parsed.title,
            description: parsed.description,
            repo_path: parsed.repo_path,
            plan_path: parsed.plan_path,
            epic_id: effective_epic_id,
            sort_order: parsed.sort_order,
            tag: parsed.tag,
            base_branch: parsed.base_branch,
            wrap_up_mode: parsed.wrap_up_mode,
            auto_run_plan: parsed.auto_run_plan,
        })
        .await
    {
        Ok(task_id) => {
            state.notify_task_changed(task_id);
            JsonRpcResponse::ok(
                id,
                json!({"content": [{"type": "text", "text": format!("Task {task_id} created")}]}),
            )
        }
        Err(e) => service_err_to_response(id, e),
    }
}

pub(crate) async fn handle_get_task(
    state: &McpState,
    id: Option<Value>,
    _identity: &CallerIdentity,
    args: Value,
) -> JsonRpcResponse {
    let parsed = match parse_args::<GetTaskArgs>(&id, args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!(task_id = parsed.task_id, "MCP get_task");

    match state.task_svc.get_task(TaskId(parsed.task_id)).await {
        Ok(task) => {
            let epic_titles = super::build_epic_titles(state).await;
            let text = super::format_task_detail(&task, &epic_titles);
            JsonRpcResponse::ok(id, json!({"content": [{"type": "text", "text": text}]}))
        }
        Err(e) => service_err_to_response(id, e),
    }
}

pub(crate) async fn handle_list_tasks(
    state: &McpState,
    id: Option<Value>,
    identity: &CallerIdentity,
    args: Value,
) -> JsonRpcResponse {
    let parsed = match parse_args::<ListTasksArgs>(&id, args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!(status = ?parsed.status, identity = ?identity, "MCP list_tasks");

    let status_filter: Option<Vec<TaskStatus>> = parsed.status.map(StatusFilter::into_vec);

    let (derived_epic_id, exclude_task_id) = match identity {
        CallerIdentity::Task(caller_id) => {
            let caller = match fetch_caller_task(&*state.db, &id, *caller_id).await {
                Ok(t) => t,
                Err(resp) => return resp,
            };
            let has_explicit_scope = parsed.epic_id.is_some() || parsed.repo_paths.is_some();
            let epic = if has_explicit_scope {
                None
            } else {
                caller.epic_id
            };
            (epic, Some(caller.id))
        }
        CallerIdentity::Session => (None, None),
    };

    let epic_id = parsed.epic_id.map(EpicId).or(derived_epic_id);

    match state
        .task_svc
        .list_tasks(ListTasksFilter {
            statuses: status_filter,
            epic_id,
            repo_paths: parsed.repo_paths,
            exclude_task_id,
        })
        .await
    {
        Ok(filtered) => {
            if filtered.is_empty() {
                return JsonRpcResponse::ok(
                    id,
                    json!({"content": [{"type": "text", "text": "No tasks found"}]}),
                );
            }
            let epic_titles = super::build_epic_titles(state).await;
            // Read each unique plan file once to avoid repeated I/O per task.
            let plan_goals: HashMap<String, String> = {
                let mut cache = HashMap::new();
                for t in &filtered {
                    if let Some(path) = t.plan_path.as_deref() {
                        if !cache.contains_key(path) {
                            let goal = super::plan_goal(path).await.unwrap_or_default();
                            cache.insert(path.to_owned(), goal);
                        }
                    }
                }
                cache
            };
            let lines: Vec<String> = filtered
                .iter()
                .map(|t| {
                    let goal = match t.plan_path.as_deref().and_then(|p| plan_goals.get(p)) {
                        Some(g) if !g.is_empty() => g.clone(),
                        _ => super::description_preview(&t.description),
                    };
                    super::format_task_line(t, &epic_titles, &goal)
                })
                .collect();
            JsonRpcResponse::ok(
                id,
                json!({"content": [{"type": "text", "text": lines.join("\n")}]}),
            )
        }
        Err(e) => service_err_to_response(id, e),
    }
}

// ---------------------------------------------------------------------------
// query_usage
// ---------------------------------------------------------------------------

fn parse_usage_since(s: &str) -> std::result::Result<chrono::DateTime<chrono::Utc>, String> {
    use chrono::TimeZone;
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
                .map(|ndt| chrono::Utc.from_utc_datetime(&ndt))
        })
        .map_err(|_| format!("invalid `since` datetime: {s}"))
}

pub(crate) async fn handle_query_usage(
    state: &McpState,
    id: Option<Value>,
    _identity: &CallerIdentity,
    args: Value,
) -> JsonRpcResponse {
    let args: QueryUsageArgs = match parse_args(&id, args) {
        Ok(a) => a,
        Err(e) => return e,
    };

    // Reject unknown enum strings up front rather than silently returning an
    // empty result set when the caller mistypes a filter.
    if let Some(ref c) = args.category {
        if crate::models::UsageCategory::parse(c).is_none() {
            return JsonRpcResponse::err(id, -32602, format!("unknown category: {c}"));
        }
    }
    if let Some(ref a) = args.actor {
        if crate::models::UsageActor::parse(a).is_none() {
            return JsonRpcResponse::err(id, -32602, format!("unknown actor: {a}"));
        }
    }

    let since = match args.since.as_deref().map(parse_usage_since) {
        Some(Ok(dt)) => Some(dt),
        Some(Err(msg)) => return JsonRpcResponse::err(id, -32602, msg),
        None => None,
    };

    let query = crate::db::UsageQuery {
        category: args.category,
        actor: args.actor,
        since,
        limit: args.limit.map(|l| l as usize),
    };

    match state.db.query_usage(&query).await {
        Ok(summaries) => {
            let json_rows: Vec<Value> = summaries
                .into_iter()
                .map(|s| {
                    serde_json::json!({
                        "category": s.category,
                        "action": s.action,
                        "detail": s.detail,
                        "actor": s.actor,
                        "count": s.count,
                        "last_used": s.last_used.to_rfc3339(),
                    })
                })
                .collect();
            let text =
                serde_json::to_string_pretty(&json_rows).unwrap_or_else(|_| "[]".to_string());
            JsonRpcResponse::ok(
                id,
                serde_json::json!({
                    "content": [{ "type": "text", "text": text }]
                }),
            )
        }
        Err(e) => service_err_to_response(id, ServiceError::Internal(e)),
    }
}
