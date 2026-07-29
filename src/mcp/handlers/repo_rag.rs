use serde::Deserialize;
use serde_json::{json, Value};

use crate::mcp::identity::CallerIdentity;
use crate::mcp::McpState;
use crate::service::repo_index::{RepoIndexService, BATCH_SIZE};
use crate::service::ServiceError;

use super::types::{
    fetch_caller_task, parse_args, service_err_to_response, tool_error, JsonRpcResponse,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct IndexRepoArgs {
    #[serde(default)]
    pub(super) repo_path: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SearchDocsArgs {
    pub(super) query: String,
    #[serde(default)]
    pub(super) repo_path: Option<String>,
    #[serde(default)]
    pub(super) limit: Option<usize>,
}

async fn resolve_repo_path(
    id: &Option<Value>,
    identity: &CallerIdentity,
    override_path: Option<String>,
    state: &McpState,
) -> Result<std::path::PathBuf, JsonRpcResponse> {
    if let Some(p) = override_path {
        return Ok(std::path::PathBuf::from(crate::models::expand_tilde(&p)));
    }
    let CallerIdentity::Task(tid) = identity else {
        return Err(service_err_to_response(
            id.clone(),
            ServiceError::Validation(
                "repo_path is required when called outside a task session".to_string(),
            ),
        ));
    };
    let task = fetch_caller_task(&*state.db, id, *tid).await?;
    Ok(std::path::PathBuf::from(crate::models::expand_tilde(
        &task.repo_path,
    )))
}

pub(super) async fn handle_index_repo(
    state: &McpState,
    id: Option<Value>,
    identity: &CallerIdentity,
    args: Value,
) -> JsonRpcResponse {
    let parsed = match parse_args::<IndexRepoArgs>(&id, args) {
        Ok(a) => a,
        Err(e) => return e,
    };

    let repo_path = match resolve_repo_path(&id, identity, parsed.repo_path, state).await {
        Ok(p) => p,
        Err(e) => return e,
    };

    let svc = RepoIndexService::new(state.embedding_service.clone());
    match svc.index_repo(&repo_path, BATCH_SIZE).await {
        Ok(result) => JsonRpcResponse::ok(
            id,
            json!({
                "content": [{
                    "type": "text",
                    "text": format!(
                        "Indexed {repo} — files_indexed: {fi}, files_skipped: {fs}, files_remaining: {fr}, chunks_total: {ct}, duration_ms: {ms}",
                        repo = repo_path.display(),
                        fi = result.files_indexed,
                        fs = result.files_skipped,
                        fr = result.files_remaining,
                        ct = result.chunks_total,
                        ms = result.duration_ms,
                    )
                }]
            }),
        ),
        Err(e) => tool_error(id, format!("index_repo failed: {e}")),
    }
}

#[allow(clippy::expect_used)]
pub(super) async fn handle_search_docs(
    state: &McpState,
    id: Option<Value>,
    identity: &CallerIdentity,
    args: Value,
) -> JsonRpcResponse {
    let parsed = match parse_args::<SearchDocsArgs>(&id, args) {
        Ok(a) => a,
        Err(e) => return e,
    };

    let repo_path = match resolve_repo_path(&id, identity, parsed.repo_path, state).await {
        Ok(p) => p,
        Err(e) => return e,
    };

    let limit = parsed.limit.unwrap_or(5).clamp(1, 20);
    let svc = RepoIndexService::new(state.embedding_service.clone());

    match svc.search_docs(&repo_path, &parsed.query, limit).await {
        Ok(results) => {
            let count = results.len();
            let items: Vec<Value> = results
                .into_iter()
                .map(|r| {
                    json!({
                        "file_path": r.file_path,
                        "chunk_text": r.chunk_text,
                        "score": r.score,
                    })
                })
                .collect();
            JsonRpcResponse::ok(
                id,
                json!({
                    "content": [{
                        "type": "text",
                        "text": serde_json::to_string_pretty(&json!({
                            "results": items,
                            "count": count,
                        }))
                        .expect("Value built from json! macro is always serializable")

                    }]
                }),
            )
        }
        Err(e) => tool_error(id, format!("search_docs failed: {e}")),
    }
}
