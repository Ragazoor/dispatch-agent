use serde::Deserialize;
use serde_json::{Value, json};

use crate::db::EpicPatch;
use crate::models::{EpicId, TaskStatus};
use crate::mcp::McpState;

use super::types::{JsonRpcResponse, deserialize_flexible_i64, parse_args};

// ---------------------------------------------------------------------------
// Typed argument structs
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub(super) struct CreateEpicArgs {
    pub(super) title: String,
    pub(super) repo_path: String,
    #[serde(default)]
    pub(super) description: String,
    #[serde(default)]
    pub(super) plan: String,
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
    pub(super) plan: Option<String>,
    #[serde(default)]
    pub(super) done: Option<bool>,
}

// ---------------------------------------------------------------------------
// Epic tool handlers
// ---------------------------------------------------------------------------

pub(super) fn handle_create_epic(state: &McpState, id: Option<Value>, args: Value) -> JsonRpcResponse {
    let parsed = match parse_args::<CreateEpicArgs>(id.clone(), args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!(title = %parsed.title, "MCP create_epic");

    match state.db.create_epic(&parsed.title, &parsed.description, &parsed.plan, &parsed.repo_path) {
        Ok(epic) => {
            state.notify();
            JsonRpcResponse::ok(
                id,
                json!({"content": [{"type": "text", "text": format!("Epic {} created: {}", epic.id, epic.title)}]}),
            )
        }
        Err(e) => JsonRpcResponse::err(id, -32603, format!("Database error: {e}")),
    }
}

pub(super) fn handle_get_epic(state: &McpState, id: Option<Value>, args: Value) -> JsonRpcResponse {
    let parsed = match parse_args::<GetEpicArgs>(id.clone(), args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!(epic_id = parsed.epic_id, "MCP get_epic");

    match state.db.get_epic(EpicId(parsed.epic_id)) {
        Ok(Some(epic)) => {
            let subtasks = state.db.list_tasks_for_epic(epic.id).unwrap_or_default();
            let done = subtasks.iter().filter(|t| t.status == TaskStatus::Done).count();
            let total = subtasks.len();
            let text = format!(
                "Epic {id}: {title}\nDescription: {desc}\nRepo: {repo}\nDone: {done_flag}\nSubtasks: {done}/{total} done\n\nPlan:\n{plan}",
                id = epic.id,
                title = epic.title,
                desc = epic.description,
                repo = epic.repo_path,
                done_flag = epic.done,
                plan = epic.plan,
            );
            JsonRpcResponse::ok(id, json!({"content": [{"type": "text", "text": text}]}))
        }
        Ok(None) => JsonRpcResponse::err(id, -32602, format!("Epic {} not found", parsed.epic_id)),
        Err(e) => JsonRpcResponse::err(id, -32603, format!("Database error: {e}")),
    }
}

pub(super) fn handle_list_epics(state: &McpState, id: Option<Value>, _args: Value) -> JsonRpcResponse {
    tracing::info!("MCP list_epics");

    match state.db.list_epics() {
        Ok(epics) => {
            if epics.is_empty() {
                return JsonRpcResponse::ok(
                    id,
                    json!({"content": [{"type": "text", "text": "No epics found"}]}),
                );
            }
            let lines: Vec<String> = epics.iter().map(|e| {
                let subtasks = state.db.list_tasks_for_epic(e.id).unwrap_or_default();
                let done = subtasks.iter().filter(|t| t.status == TaskStatus::Done).count();
                format!("- [{}] {} ({}/{} done): {}", e.id, e.title, done, subtasks.len(), e.description)
            }).collect();
            JsonRpcResponse::ok(id, json!({"content": [{"type": "text", "text": lines.join("\n")}]}))
        }
        Err(e) => JsonRpcResponse::err(id, -32603, format!("Database error: {e}")),
    }
}

pub(super) fn handle_update_epic(state: &McpState, id: Option<Value>, args: Value) -> JsonRpcResponse {
    let parsed = match parse_args::<UpdateEpicArgs>(id.clone(), args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!(epic_id = parsed.epic_id, "MCP update_epic");

    let mut patch = EpicPatch::new();
    if let Some(ref t) = parsed.title { patch = patch.title(t); }
    if let Some(ref d) = parsed.description { patch = patch.description(d); }
    if let Some(ref p) = parsed.plan { patch = patch.plan(p); }
    if let Some(d) = parsed.done { patch = patch.done(d); }

    if let Err(e) = state.db.patch_epic(EpicId(parsed.epic_id), &patch) {
        return JsonRpcResponse::err(id, -32603, format!("Database error: {e}"));
    }

    state.notify();
    let mut updated = Vec::new();
    if parsed.title.is_some() { updated.push("title"); }
    if parsed.description.is_some() { updated.push("description"); }
    if parsed.plan.is_some() { updated.push("plan"); }
    if parsed.done.is_some() { updated.push("done"); }

    JsonRpcResponse::ok(
        id,
        json!({"content": [{"type": "text", "text": format!("Epic {} updated ({})", parsed.epic_id, updated.join(", "))}]}),
    )
}
