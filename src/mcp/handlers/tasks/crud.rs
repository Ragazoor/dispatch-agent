use std::collections::HashMap;

use serde_json::{json, Value};

use crate::mcp::identity::CallerIdentity;
use crate::mcp::McpState;
use crate::models::{EpicId, Task, TaskId, TaskStatus};
use crate::service::{
    CreateTaskParams, ListTasksFilter, ServiceError, UpdateTaskParams, UrlUpdate,
};

use super::{
    fetch_caller_task, parse_args, service_err_to_response, CreateTaskWithEpicArgs, GetTaskArgs,
    JsonRpcResponse, ListTasksArgs, QueryUsageArgs, StatusFilter, UpdateTaskArgs, INVALID_PARAMS,
};

pub(crate) async fn handle_update_task(
    state: &McpState,
    id: Option<Value>,
    identity: &CallerIdentity,
    args: Value,
) -> JsonRpcResponse {
    let mut parsed = match parse_args::<UpdateTaskArgs>(&id, args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    let task_id = parsed.task_id;
    tracing::info!(task_id = task_id.0, status = ?parsed.status, "MCP update_task");

    // status="done" is a dedicated close-only path (MarkTaskDoneViaMcp,
    // mcp-task-tools.allium). Checked FIRST, against the raw parsed args —
    // before the archived check below and before the url/url_type pairing
    // validation, both of which never apply to this path — so a field that
    // would otherwise slip past unnoticed (url_type sent without a url has
    // no slot in UpdateTaskParams at all) or get answered by the wrong error
    // (url without url_type) is instead caught by this call's own
    // "only field set" guard.
    if parsed.status == Some(TaskStatus::Done) {
        return handle_mark_task_done(state, id, identity, parsed).await;
    }

    // MCP-specific restriction: archival stays TUI-only.
    if parsed.status == Some(TaskStatus::Archived) {
        return service_err_to_response(
            id,
            ServiceError::Validation(
                "Cannot set status to archived via MCP. Please ask the human operator to manage this from the TUI.".into(),
            ),
        );
    }

    let mut params = UpdateTaskParams::for_task(task_id);

    // `url`/`url_type` are the two `[manual]` fields of `UpdateTaskArgs`: they
    // validate as a pair rather than mapping to one setter each, so they are
    // taken out here and everything else is folded in by the generated
    // `apply_to_params` below.
    match parsed.url.take() {
        // Empty string clears the URL (legacy clear convention).
        Some(ref u) if u.is_empty() => {
            params = params.url(UrlUpdate::Clear);
        }
        Some(u) => {
            let url_type = match parsed.url_type.take() {
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
            params = params.url(UrlUpdate::Set(crate::models::TaskUrl::new(u, url_type)));
        }
        None => {}
    }

    let params = parsed.apply_to_params(params);
    let fields_display = params.updated_field_names().join(", ");

    match state.task_svc.update_task(params).await {
        Ok(result) => {
            state.notify_task_changed(task_id);
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

/// Every `UpdateTaskArgs` field other than `task_id`/`status` that the caller
/// actually set. Used by [`handle_mark_task_done`] to enforce
/// `MarkTaskDoneViaMcp`'s "status=\"done\" must be the only field set" guard
/// directly against the raw args — deliberately not
/// `UpdateTaskParams::updated_field_names`, which has no slot for `url_type`
/// sent without a `url` (it is `[manual]` and only ever read inside the
/// non-empty-`url` branch above), so that combination would otherwise pass
/// through unnoticed.
///
/// Exhaustive destructuring (no `..`): a new `UpdateTaskArgs` field must be
/// added here explicitly, or this fails to compile.
fn other_update_task_fields_set(parsed: &UpdateTaskArgs) -> Vec<&'static str> {
    let UpdateTaskArgs {
        task_id: _,
        status: _,
        plan_path,
        title,
        description,
        repo_path,
        sort_order,
        url,
        url_type,
        tag,
        sub_status,
        epic_id,
        base_branch,
        wrap_up_mode,
        auto_run_plan,
        phoenix,
    } = parsed;

    [
        ("plan_path", plan_path.is_some()),
        ("title", title.is_some()),
        ("description", description.is_some()),
        ("repo_path", repo_path.is_some()),
        ("sort_order", sort_order.is_some()),
        ("url", url.is_some()),
        ("url_type", url_type.is_some()),
        ("tag", tag.is_some()),
        ("sub_status", sub_status.is_some()),
        ("epic_id", epic_id.is_some()),
        ("base_branch", base_branch.is_some()),
        ("wrap_up_mode", wrap_up_mode.is_some()),
        ("auto_run_plan", auto_run_plan.is_some()),
        ("phoenix", phoenix.is_some()),
    ]
    .into_iter()
    .filter_map(|(name, is_set)| is_set.then_some(name))
    .collect()
}

/// `update_task(status="done")`: a dedicated close-only call — see
/// `MarkTaskDoneViaMcp` (docs/specs/mcp-task-tools.allium). Reachable for a
/// session caller or a dispatched agent acting on a task other than its own;
/// never for a dispatched agent closing itself (that stays wrap_up +
/// exit_session), and never combined with any other field in the same call.
/// Reuses `exit_session`'s own terminal-write/teardown/chain tail
/// (`perform_close` in `wrap_up.rs`) rather than re-deriving it.
async fn handle_mark_task_done(
    state: &McpState,
    id: Option<Value>,
    identity: &CallerIdentity,
    parsed: UpdateTaskArgs,
) -> JsonRpcResponse {
    let task_id = parsed.task_id;

    let other_fields = other_update_task_fields_set(&parsed);
    if !other_fields.is_empty() {
        return service_err_to_response(
            id,
            ServiceError::Validation(format!(
                "status=\"done\" must be the only field set in this call (also set: {}). \
                 Update other fields separately, then close with a status-only call.",
                other_fields.join(", ")
            )),
        );
    }

    if matches!(identity, CallerIdentity::Task(caller_task_id) if *caller_task_id == task_id) {
        return service_err_to_response(
            id,
            ServiceError::Validation(
                "Cannot mark your own task done via update_task — call wrap_up then exit_session instead."
                    .into(),
            ),
        );
    }

    let task = match state.db.get_task(task_id).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return service_err_to_response(
                id,
                ServiceError::NotFound(format!("Task {} not found", task_id.0)),
            )
        }
        Err(e) => return service_err_to_response(id, ServiceError::Internal(e)),
    };

    match super::wrap_up::perform_close(state, &task, crate::service::CloseSessionOutcome::Done)
        .await
    {
        super::wrap_up::ClosePathOutcome::NotPersisted => JsonRpcResponse::ok(
            id,
            json!({"content": [{"type": "text", "text": format!(
                "Task #{} could NOT be moved to done — the close did not take effect. \
                 The task is still in its previous status; try again.",
                task_id.0
            )}]}),
        ),
        super::wrap_up::ClosePathOutcome::Persisted { chained } => {
            let text = match chained {
                Some((next_id, next_title)) => format!(
                    "Task #{} marked done. Dispatching next epic subtask #{} '{next_title}'.",
                    task_id.0, next_id.0
                ),
                None => format!("Task #{} marked done.", task_id.0),
            };
            JsonRpcResponse::ok(id, json!({"content": [{"type": "text", "text": text}]}))
        }
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
                Some(inner) => inner,
                None => caller.epic_id,
            }
        }
        CallerIdentity::Session => parsed.epic_id.flatten(),
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
            phoenix: parsed.phoenix,
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
    tracing::info!(task_id = parsed.task_id.0, "MCP get_task");

    match state.task_svc.get_task(parsed.task_id).await {
        Ok(task) => {
            let (epic_titles, verify_command) = tokio::join!(
                super::build_epic_titles(state),
                crate::dispatch::fetch_verify_command(&*state.db, &task.repo_path)
            );
            let text = super::format_task_detail(&task, &epic_titles, verify_command.as_deref());
            JsonRpcResponse::ok(id, json!({"content": [{"type": "text", "text": text}]}))
        }
        Err(e) => service_err_to_response(id, e),
    }
}

/// Read each unique plan file once to avoid repeated I/O per task.
async fn build_plan_goal_cache(tasks: &[Task]) -> HashMap<String, String> {
    let mut cache = HashMap::new();
    for t in tasks {
        if let Some(path) = t.plan_path.as_deref() {
            if !cache.contains_key(path) {
                let goal = super::plan_goal(path).await.unwrap_or_default();
                cache.insert(path.to_owned(), goal);
            }
        }
    }
    cache
}

/// Resolve the epic/exclusion scope for `list_tasks`. A task caller inherits
/// its own epic as the default scope, unless the request explicitly names an
/// `epic_id` or `repo_paths` — an explicit scope always overrides inheritance.
/// A session caller has no inherited scope.
async fn resolve_list_scope(
    state: &McpState,
    id: &Option<Value>,
    identity: &CallerIdentity,
    parsed: &ListTasksArgs,
) -> Result<(Option<EpicId>, Option<TaskId>), JsonRpcResponse> {
    match identity {
        CallerIdentity::Task(caller_id) => {
            let caller = fetch_caller_task(&*state.db, id, *caller_id).await?;
            let has_explicit_scope = parsed.epic_id.is_some() || parsed.repo_paths.is_some();
            let epic = if has_explicit_scope {
                None
            } else {
                caller.epic_id
            };
            Ok((epic, Some(caller.id)))
        }
        CallerIdentity::Session => Ok((None, None)),
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

    let (derived_epic_id, exclude_task_id) =
        match resolve_list_scope(state, &id, identity, &parsed).await {
            Ok(scope) => scope,
            Err(resp) => return resp,
        };

    let status_filter: Option<Vec<TaskStatus>> = parsed.status.map(StatusFilter::into_vec);
    let epic_id = parsed.epic_id.or(derived_epic_id);

    let filtered = match state
        .task_svc
        .list_tasks(ListTasksFilter {
            statuses: status_filter,
            epic_id,
            repo_paths: parsed.repo_paths,
            exclude_task_id,
        })
        .await
    {
        Ok(filtered) => filtered,
        Err(e) => return service_err_to_response(id, e),
    };

    if filtered.is_empty() {
        return JsonRpcResponse::ok(
            id,
            json!({"content": [{"type": "text", "text": "No tasks found"}]}),
        );
    }
    let (epic_titles, plan_goals) = tokio::join!(
        super::build_epic_titles(state),
        build_plan_goal_cache(&filtered)
    );
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
            return JsonRpcResponse::err(id, INVALID_PARAMS, format!("unknown category: {c}"));
        }
    }
    if let Some(ref a) = args.actor {
        if crate::models::UsageActor::parse(a).is_none() {
            return JsonRpcResponse::err(id, INVALID_PARAMS, format!("unknown actor: {a}"));
        }
    }

    let since = match args.since.as_deref().map(parse_usage_since) {
        Some(Ok(dt)) => Some(dt),
        Some(Err(msg)) => return JsonRpcResponse::err(id, INVALID_PARAMS, msg),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn base_args() -> UpdateTaskArgs {
        UpdateTaskArgs {
            task_id: TaskId(1),
            status: Some(TaskStatus::Done),
            plan_path: None,
            title: None,
            description: None,
            repo_path: None,
            sort_order: None,
            url: None,
            url_type: None,
            tag: None,
            sub_status: None,
            epic_id: None,
            base_branch: None,
            wrap_up_mode: None,
            auto_run_plan: None,
            phoenix: None,
        }
    }

    /// Each field set individually must be reported by name. The exhaustive
    /// destructuring in `other_update_task_fields_set` already makes an
    /// unhandled new field a compile error (or an unused-binding warning
    /// under `-D warnings`); what this test uniquely covers is the *name* —
    /// a copy-paste that reports "title" for the `description` field
    /// compiles fine and is caught only here. Mirrors
    /// `update_task_params_every_field_covered` in
    /// `src/service/tasks/params.rs`, the same convention applied to the
    /// sibling struct.
    #[test]
    fn other_update_task_fields_set_every_field_covered() {
        let cases: Vec<(&str, UpdateTaskArgs)> = vec![
            (
                "plan_path",
                UpdateTaskArgs {
                    plan_path: Some("p".to_string()),
                    ..base_args()
                },
            ),
            (
                "title",
                UpdateTaskArgs {
                    title: Some("t".to_string()),
                    ..base_args()
                },
            ),
            (
                "description",
                UpdateTaskArgs {
                    description: Some("d".to_string()),
                    ..base_args()
                },
            ),
            (
                "repo_path",
                UpdateTaskArgs {
                    repo_path: Some("r".to_string()),
                    ..base_args()
                },
            ),
            (
                "sort_order",
                UpdateTaskArgs {
                    sort_order: Some(0),
                    ..base_args()
                },
            ),
            (
                "url",
                UpdateTaskArgs {
                    url: Some("https://example.com".to_string()),
                    ..base_args()
                },
            ),
            (
                "url_type",
                UpdateTaskArgs {
                    url_type: Some(crate::models::UrlType::Pr),
                    ..base_args()
                },
            ),
            (
                "tag",
                UpdateTaskArgs {
                    tag: Some(crate::models::TaskTag::Bug),
                    ..base_args()
                },
            ),
            (
                "sub_status",
                UpdateTaskArgs {
                    sub_status: Some(crate::models::SubStatus::Active),
                    ..base_args()
                },
            ),
            (
                "epic_id",
                UpdateTaskArgs {
                    epic_id: Some(EpicId(1)),
                    ..base_args()
                },
            ),
            (
                "base_branch",
                UpdateTaskArgs {
                    base_branch: Some("main".to_string()),
                    ..base_args()
                },
            ),
            (
                "wrap_up_mode",
                UpdateTaskArgs {
                    wrap_up_mode: Some(Some(crate::models::WrapUpMode::Rebase)),
                    ..base_args()
                },
            ),
            (
                "auto_run_plan",
                UpdateTaskArgs {
                    auto_run_plan: Some(true),
                    ..base_args()
                },
            ),
            (
                "phoenix",
                UpdateTaskArgs {
                    phoenix: Some(true),
                    ..base_args()
                },
            ),
        ];
        for (expected, args) in &cases {
            assert_eq!(
                other_update_task_fields_set(args),
                vec![*expected],
                "setting {expected} should report exactly that field name"
            );
        }
    }

    #[test]
    fn other_update_task_fields_set_empty_when_only_status_set() {
        assert!(other_update_task_fields_set(&base_args()).is_empty());
    }
}
