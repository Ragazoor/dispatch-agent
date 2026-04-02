use serde::Deserialize;
use serde_json::{json, Value};

use crate::mcp::McpState;
use crate::models::TaskStatus;
use crate::service::{CreateEpicParams, EpicService, UpdateEpicParams};

use super::types::{
    deserialize_flexible_i64, deserialize_optional_flexible_i64, parse_args,
    service_err_to_response, JsonRpcResponse,
};

// ---------------------------------------------------------------------------
// Typed argument structs (JSON-RPC layer)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub(super) struct CreateEpicArgs {
    pub(super) title: String,
    pub(super) repo_path: String,
    #[serde(default)]
    pub(super) description: String,
    #[serde(default, deserialize_with = "deserialize_optional_flexible_i64")]
    pub(super) sort_order: Option<i64>,
}

#[derive(Deserialize)]
pub(super) struct GetEpicArgs {
    #[serde(deserialize_with = "deserialize_flexible_i64")]
    pub(super) epic_id: i64,
}

#[derive(Deserialize)]
pub(super) struct UpdateEpicArgs {
    #[serde(deserialize_with = "deserialize_flexible_i64")]
    pub(super) epic_id: i64,
    #[serde(default)]
    pub(super) title: Option<String>,
    #[serde(default)]
    pub(super) description: Option<String>,
    #[serde(default)]
    pub(super) status: Option<String>,
    #[serde(default)]
    pub(super) plan: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_flexible_i64")]
    pub(super) sort_order: Option<i64>,
    #[serde(default)]
    pub(super) repo_path: Option<String>,
}

// ---------------------------------------------------------------------------
// Epic tool handlers (thin wrappers over EpicService)
// ---------------------------------------------------------------------------

pub(super) fn handle_create_epic(
    state: &McpState,
    id: Option<Value>,
    args: Value,
) -> JsonRpcResponse {
    let parsed = match parse_args::<CreateEpicArgs>(&id, args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!(title = %parsed.title, "MCP create_epic");

    let svc = EpicService::new(state.db.clone());
    match svc.create_epic(CreateEpicParams {
        title: parsed.title,
        description: parsed.description,
        repo_path: parsed.repo_path,
        sort_order: parsed.sort_order,
    }) {
        Ok(epic) => {
            state.notify();
            JsonRpcResponse::ok(
                id,
                json!({"content": [{"type": "text", "text": format!("Epic {} created: {}", epic.id, epic.title)}]}),
            )
        }
        Err(e) => service_err_to_response(id, e),
    }
}

pub(super) fn handle_get_epic(state: &McpState, id: Option<Value>, args: Value) -> JsonRpcResponse {
    let parsed = match parse_args::<GetEpicArgs>(&id, args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!(epic_id = parsed.epic_id, "MCP get_epic");

    let svc = EpicService::new(state.db.clone());
    match svc.get_epic_with_subtasks(parsed.epic_id) {
        Ok((epic, subtasks)) => {
            let done_count = subtasks
                .iter()
                .filter(|t| t.status == TaskStatus::Done)
                .count();
            let total = subtasks.len();
            let mut text = format!(
                "Epic {id}: {title}\nDescription: {desc}\nRepo: {repo}\nStatus: {status}",
                id = epic.id,
                title = epic.title,
                desc = epic.description,
                repo = epic.repo_path,
                status = epic.status.as_str(),
            );
            if let Some(ref p) = epic.plan {
                text.push_str(&format!("\nPlan: {p}"));
            }
            if let Some(sort_order) = epic.sort_order {
                text.push_str(&format!("\nSort order: {sort_order}"));
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

pub(super) fn handle_list_epics(
    state: &McpState,
    id: Option<Value>,
    _args: Value,
) -> JsonRpcResponse {
    tracing::info!("MCP list_epics");

    let svc = EpicService::new(state.db.clone());
    match svc.list_epics_with_progress() {
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
                    let plan_indicator = if e.plan.is_some() { " [plan]" } else { "" };
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

pub(super) fn handle_update_epic(
    state: &McpState,
    id: Option<Value>,
    args: Value,
) -> JsonRpcResponse {
    let parsed = match parse_args::<UpdateEpicArgs>(&id, args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!(epic_id = parsed.epic_id, "MCP update_epic");

    let params = UpdateEpicParams {
        epic_id: parsed.epic_id,
        title: parsed.title.clone(),
        description: parsed.description.clone(),
        status: parsed.status.clone(),
        plan: parsed.plan.clone(),
        sort_order: parsed.sort_order,
        repo_path: parsed.repo_path.clone(),
    };
    let field_names: Vec<String> = params
        .updated_field_names()
        .into_iter()
        .map(String::from)
        .collect();

    let svc = EpicService::new(state.db.clone());
    match svc.update_epic(params) {
        Ok(epic_id) => {
            state.notify();
            JsonRpcResponse::ok(
                id,
                json!({"content": [{"type": "text", "text": format!("Epic {} updated ({})", epic_id, field_names.join(", "))}]}),
            )
        }
        Err(e) => service_err_to_response(id, e),
    }
}
