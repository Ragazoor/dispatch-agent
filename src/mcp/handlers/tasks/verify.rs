use serde_json::{json, Value};

use crate::mcp::identity::CallerIdentity;
use crate::mcp::McpState;

use super::{parse_args, JsonRpcResponse, SetVerifyCommandArgs, INTERNAL_ERROR, INVALID_PARAMS};

pub(crate) async fn handle_set_verify_command(
    state: &McpState,
    id: Option<Value>,
    _identity: &CallerIdentity,
    args: Value,
) -> JsonRpcResponse {
    let parsed = match parse_args::<SetVerifyCommandArgs>(&id, args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!(repo_path = %parsed.repo_path, "MCP set_verify_command");
    // Newline guard returns -32602 (invalid params) for a user-input error; the DB layer
    // would surface the same constraint as -32603 (internal error), so we validate here.
    // Blank/whitespace commands are intentionally not rejected — the DB normalises them to
    // None (clear), which matches the trait contract.
    if parsed
        .command
        .as_deref()
        .is_some_and(|c| c.contains('\n') || c.contains('\r'))
    {
        return JsonRpcResponse::err(id, INVALID_PARAMS, "command must be a single line");
    }
    match state
        .db
        .set_verify_command(&parsed.repo_path, parsed.command.as_deref())
        .await
    {
        Ok(()) => {
            let msg = match &parsed.command {
                Some(cmd) => format!("Verify command set for `{}`: `{cmd}`", parsed.repo_path),
                None => format!("Verify command cleared for `{}`", parsed.repo_path),
            };
            JsonRpcResponse::ok(id, json!({"content": [{"type": "text", "text": msg}]}))
        }
        Err(e) => JsonRpcResponse::err(
            id,
            INTERNAL_ERROR,
            format!("failed to set verify command: {e}"),
        ),
    }
}
