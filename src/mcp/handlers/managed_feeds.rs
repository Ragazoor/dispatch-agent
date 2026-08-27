use serde::Deserialize;
use serde_json::{json, Value};

use crate::mcp::identity::CallerIdentity;
use crate::mcp::McpState;
use crate::service::ServiceError;

use super::types::{
    deserialize_nullable_flexible_i64, deserialize_nullable_string, parse_args,
    service_err_to_response, JsonRpcResponse,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SetManagedFeedConfigArgs {
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub(super) reviews_command: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable_flexible_i64")]
    pub(super) reviews_interval_secs: Option<Option<i64>>,
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub(super) cve_command: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_nullable_flexible_i64")]
    pub(super) cve_interval_secs: Option<Option<i64>>,
}

fn fmt_opt_str(v: Option<&str>) -> String {
    match v {
        Some(s) => format!("`{s}`"),
        None => "(unset)".to_string(),
    }
}

fn fmt_opt_int(v: Option<i64>) -> String {
    match v {
        Some(n) => n.to_string(),
        None => "(unset)".to_string(),
    }
}

/// Compose the four-line text summary of the current managed-feed config.
async fn config_summary(state: &McpState, heading: &str) -> anyhow::Result<String> {
    let rc = state.db.get_reviews_feed_command().await?;
    let ri = state.db.get_reviews_feed_interval_secs().await?;
    let cc = state.db.get_cve_feed_command().await?;
    let ci = state.db.get_cve_feed_interval_secs().await?;
    Ok(format!(
        "{heading}\n\
         - reviews_command: {}\n\
         - reviews_interval_secs: {}\n\
         - cve_command: {}\n\
         - cve_interval_secs: {}",
        fmt_opt_str(rc.as_deref()),
        fmt_opt_int(ri),
        fmt_opt_str(cc.as_deref()),
        fmt_opt_int(ci),
    ))
}

pub(super) async fn handle_get_managed_feed_config(
    state: &McpState,
    id: Option<Value>,
    _identity: &CallerIdentity,
    _args: Value,
) -> JsonRpcResponse {
    tracing::info!("MCP get_managed_feed_config");
    match config_summary(state, "Managed-feed config:").await {
        Ok(text) => JsonRpcResponse::ok(id, json!({"content": [{"type": "text", "text": text}]})),
        Err(e) => service_err_to_response(
            id,
            ServiceError::Internal(e.context("failed to read managed feed config")),
        ),
    }
}

pub(super) async fn handle_set_managed_feed_config(
    state: &McpState,
    id: Option<Value>,
    _identity: &CallerIdentity,
    args: Value,
) -> JsonRpcResponse {
    let parsed = match parse_args::<SetManagedFeedConfigArgs>(&id, args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!("MCP set_managed_feed_config");

    // Interval validation is NOT done here. The floor is a domain invariant of
    // the field rather than a property of this entry point, so it lives in the
    // service (`validate_feed_interval`) where every write path inherits it —
    // see "Interval literals" in docs/specs/core.allium. A handler-local check
    // is exactly the shape that let a floor bind one surface and not another.

    // Persist only the provided fields; an omitted field (None) is left as-is.
    let write = crate::service::write_managed_feed_settings(
        &*state.db,
        crate::service::ManagedFeedSettingsPatch {
            reviews_command: parsed.reviews_command,
            reviews_interval_secs: parsed.reviews_interval_secs,
            cve_command: parsed.cve_command,
            cve_interval_secs: parsed.cve_interval_secs,
        },
    )
    .await;
    // The service owns interval validation (core.allium: "Interval literals",
    // CLAIM 2), so its error is passed through unwrapped: a sub-floor cadence
    // must reach the caller as a -32602 validation error, not be relabelled
    // internal.
    if let Err(e) = write {
        return service_err_to_response(id, e);
    }

    // Re-materialise the managed epic tree so a newly-enabled feed provisions
    // its epics without a restart. The notify() below then reaches the runtime,
    // which invalidates the FeedRunner's any_feed_cmds cache so those epics are
    // actually polled (see the invalidation invariant in docs/specs/feeds.allium).
    let settings = match crate::service::read_managed_feed_settings(&*state.db).await {
        Ok(s) => s,
        Err(e) => {
            return service_err_to_response(
                id,
                ServiceError::Internal(e.context("failed to read managed feed settings")),
            );
        }
    };
    if let Err(e) = state.epic_svc.provision_managed_feeds(settings).await {
        return service_err_to_response(id, e);
    }
    state.notify();

    match config_summary(state, "Managed-feed config saved:").await {
        Ok(text) => JsonRpcResponse::ok(id, json!({"content": [{"type": "text", "text": text}]})),
        // Persist + provision already succeeded; a summary read failure is non-fatal.
        Err(_) => JsonRpcResponse::ok(
            id,
            json!({"content": [{"type": "text", "text": "Managed-feed config saved."}]}),
        ),
    }
}
