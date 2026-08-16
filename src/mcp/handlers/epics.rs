use serde::Deserialize;
use serde_json::{json, Value};

use crate::mcp::identity::CallerIdentity;
use crate::mcp::McpState;
use crate::models::{EpicId, TaskStatus};
use crate::service::{CreateEpicParams, ServiceError, UpdateEpicParams};

use super::types::{
    deserialize_flexible_id, deserialize_nullable_flexible_i64, deserialize_nullable_flexible_id,
    deserialize_nullable_string, deserialize_optional_flexible_i64,
    deserialize_optional_flexible_id, parse_args, service_err_to_response, JsonRpcResponse,
};

// ---------------------------------------------------------------------------
// Typed argument structs (JSON-RPC layer)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CreateEpicArgs {
    pub(super) title: String,
    #[serde(default)]
    pub(super) description: String,
    #[serde(default, deserialize_with = "deserialize_optional_flexible_i64")]
    pub(super) sort_order: Option<i64>,
    #[serde(default, deserialize_with = "deserialize_optional_flexible_id")]
    pub(super) parent_epic_id: Option<EpicId>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GetEpicArgs {
    #[serde(deserialize_with = "deserialize_flexible_id")]
    pub(super) epic_id: EpicId,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UpdateEpicArgs {
    #[serde(deserialize_with = "deserialize_flexible_id")]
    pub(super) epic_id: EpicId,
    #[serde(default)]
    pub(super) title: Option<String>,
    #[serde(default)]
    pub(super) description: Option<String>,
    #[serde(default)]
    pub(super) status: Option<TaskStatus>,
    #[serde(default)]
    pub(super) plan_path: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_flexible_i64")]
    pub(super) sort_order: Option<i64>,
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub(super) feed_command: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable_flexible_i64")]
    pub(super) feed_interval_secs: Option<Option<i64>>,
    #[serde(default)]
    pub(super) group_by_repo: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_nullable_flexible_id")]
    pub(super) parent_epic_id: Option<Option<EpicId>>,
}

// ---------------------------------------------------------------------------
// Epic tool handlers (thin wrappers over EpicService)
// ---------------------------------------------------------------------------

pub(super) async fn handle_create_epic(
    state: &McpState,
    id: Option<Value>,
    _identity: &CallerIdentity,
    args: Value,
) -> JsonRpcResponse {
    let parsed = match parse_args::<CreateEpicArgs>(&id, args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!(title = %parsed.title, "MCP create_epic");

    match state
        .epic_svc
        .create_epic(CreateEpicParams {
            title: parsed.title,
            description: parsed.description,
            sort_order: parsed.sort_order,
            parent_epic_id: parsed.parent_epic_id,
            feed_command: None,
            feed_interval_secs: None,
        })
        .await
    {
        Ok(epic) => {
            state.notify_epic_changed(epic.id);
            JsonRpcResponse::ok(
                id,
                json!({"content": [{"type": "text", "text": format!("Epic {} created: {}", epic.id, epic.title)}]}),
            )
        }
        Err(e) => service_err_to_response(id, e),
    }
}

pub(super) async fn handle_get_epic(
    state: &McpState,
    id: Option<Value>,
    _identity: &CallerIdentity,
    args: Value,
) -> JsonRpcResponse {
    let parsed = match parse_args::<GetEpicArgs>(&id, args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!(epic_id = parsed.epic_id.0, "MCP get_epic");

    match state.epic_svc.get_epic_with_progress(parsed.epic_id).await {
        Ok((epic, done_count, total)) => {
            let mut text = format!(
                "Epic {id}: {title}\nDescription: {desc}\nStatus: {status}",
                id = epic.id,
                title = epic.title,
                desc = epic.description,
                status = epic.status.as_str(),
            );
            if let Some(ref p) = epic.plan_path {
                text.push_str(&format!("\nPlan: {p}"));
            }
            if let Some(sort_order) = epic.sort_order {
                text.push_str(&format!("\nSort order: {sort_order}"));
            }
            if let Some(ref fc) = epic.feed_command {
                text.push_str(&format!("\nFeed command: {fc}"));
            }
            if let Some(fi) = epic.feed_interval_secs {
                text.push_str(&format!("\nFeed interval: {fi}s"));
            }
            text.push_str(&format!(
                "\nCreated: {}",
                epic.created_at.format("%Y-%m-%d %H:%M:%S UTC")
            ));
            text.push_str(&format!(
                "\nUpdated: {}",
                epic.updated_at.format("%Y-%m-%d %H:%M:%S UTC")
            ));
            text.push_str(&format!("\nSubtasks: {done_count}/{total} done"));
            JsonRpcResponse::ok(id, json!({"content": [{"type": "text", "text": text}]}))
        }
        Err(e) => service_err_to_response(id, e),
    }
}

pub(super) async fn handle_list_epics(
    state: &McpState,
    id: Option<Value>,
    _identity: &CallerIdentity,
    _args: Value,
) -> JsonRpcResponse {
    tracing::info!("MCP list_epics");

    match state.epic_svc.list_epics_with_progress().await {
        Ok(epics) => {
            if epics.is_empty() {
                return JsonRpcResponse::ok(
                    id,
                    json!({"content": [{"type": "text", "text": "No epics found"}]}),
                );
            }
            let lines: Vec<String> = epics
                .iter()
                .map(|(e, done, total)| {
                    let plan_indicator = if e.plan_path.is_some() { " [plan]" } else { "" };
                    let status_indicator = if e.status != TaskStatus::Backlog {
                        format!(" [{}]", e.status.as_str())
                    } else {
                        String::new()
                    };
                    format!(
                        "- [{}] {} ({}/{} done){}{}: {}",
                        e.id, e.title, done, total, plan_indicator, status_indicator, e.description
                    )
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

pub(super) async fn handle_update_epic(
    state: &McpState,
    id: Option<Value>,
    _identity: &CallerIdentity,
    args: Value,
) -> JsonRpcResponse {
    let parsed = match parse_args::<UpdateEpicArgs>(&id, args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!(epic_id = parsed.epic_id.0, "MCP update_epic");

    // MCP-specific restriction: agents cannot set epic status to archived
    if matches!(parsed.status, Some(TaskStatus::Archived)) {
        return service_err_to_response(
            id,
            ServiceError::Validation(
                "Cannot set epic status to archived via MCP. Please ask the human operator to manage this from the TUI.".into(),
            ),
        );
    }

    let params = UpdateEpicParams {
        epic_id: parsed.epic_id,
        title: parsed.title,
        description: parsed.description,
        status: parsed.status,
        plan_path: parsed.plan_path,
        sort_order: parsed.sort_order,
        auto_dispatch: None,
        feed_command: parsed.feed_command.map(|v| match v {
            Some(s) => crate::service::FieldUpdate::Set(s),
            None => crate::service::FieldUpdate::Clear,
        }),
        feed_interval_secs: parsed.feed_interval_secs,
        group_by_repo: parsed.group_by_repo,
        parent_epic_id: parsed.parent_epic_id,
    };
    let field_names: Vec<String> = params
        .updated_field_names()
        .into_iter()
        .map(String::from)
        .collect();

    match state.epic_svc.update_epic(params).await {
        Ok(result) => {
            let epic_id = result.epic_id;
            state.notify_epic_changed(epic_id);
            JsonRpcResponse::ok(
                id,
                json!({"content": [{"type": "text", "text": format!("Epic {} updated ({})", epic_id, field_names.join(", "))}]}),
            )
        }
        Err(e) => service_err_to_response(id, e),
    }
}
