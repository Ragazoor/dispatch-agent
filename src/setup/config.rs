//! Claude Code MCP server config: read, merge, and remove.

use anyhow::Result;
use serde_json::{json, Map, Value};

use super::{read_json_file, write_json_file};
use crate::models::TaskId;

// ---------------------------------------------------------------------------
// MCP config merging
// ---------------------------------------------------------------------------

/// The name of the MCP server entry dispatch owns, in Claude Code's config.
///
/// Written here and matched by `dispatch::caller_identity`, whose per-task
/// entry must reuse it in order to OVERRIDE this one rather than sit beside it.
/// Renaming it in only one of the two places drops every agent back to no
/// caller identity, and says so nowhere the compiler can see — which is why
/// both ends read this constant.
pub(crate) const SERVER_NAME: &str = "dispatch";

pub struct MergeResult {
    pub value: Value,
    pub changed: bool,
}

/// Resolve the `headersHelper` command for the dispatch MCP entry.
///
/// Uses the absolute path of the currently-running dispatch binary so
/// the helper invocation is unambiguous regardless of `$PATH`. Falls
/// back to the bare command name if `current_exe()` is unavailable.
/// The installed `dispatch` entry, restated to identify `task_id` by a fixed
/// header instead of by a helper.
///
/// Read-back of what [`merge_mcp_config`] wrote, and it lives beside it so the
/// two cannot drift: this knows the `mcpServers` key path, the server name, and
/// which key it has to strip.
///
/// Deriving from the installed entry rather than composing a fresh one is what
/// keeps the transport and URL — a non-default port above all — from having to
/// be taught to a second place.
///
/// Dropping `headersHelper` is required, not tidiness: with both present the
/// two would answer the same request, and a request carrying both identity
/// headers is rejected outright (`CallerIdentity::from_headers` → `Conflict`).
///
/// `None` when there is no config to read, or no `dispatch` entry in it.
pub(crate) fn dispatch_entry_identifying(
    claude_json: &std::path::Path,
    task_id: TaskId,
) -> Option<Value> {
    let json = read_json_file(claude_json).ok().flatten()?;
    let mut entry = json
        .get("mcpServers")?
        .get(SERVER_NAME)?
        .as_object()
        .cloned()?;
    entry.remove("headersHelper");
    entry.insert(
        "headers".to_string(),
        json!({ crate::mcp::identity::HEADER_TASK_ID: task_id.0.to_string() }),
    );
    Some(json!({ "mcpServers": { SERVER_NAME: Value::Object(entry) } }))
}

fn caller_headers_helper() -> String {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(|s| s.to_owned()))
        .unwrap_or_else(|| "dispatch".to_string());
    format!("{exe} caller-headers")
}

pub fn merge_mcp_config(existing: Option<Value>, port: u16) -> MergeResult {
    let server_entry = json!({
        "type": "http",
        "url": format!("http://localhost:{port}/mcp"),
        // Serves non-dispatched sessions only. A dispatched agent's launch
        // overrides this entry with one that drops this key and carries a fixed
        // caller-task header instead — see `dispatch_entry_identifying` below,
        // and `dispatch::caller_identity` for why the helper cannot answer for
        // an agent. Removing the strip without removing this would put both
        // identity headers on the wire, which is rejected outright.
        "headersHelper": caller_headers_helper(),
    });

    let mut root = match existing {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };

    let servers = root.entry("mcpServers").or_insert_with(|| json!({}));

    if let Value::Object(servers_map) = servers {
        if servers_map.get(SERVER_NAME) == Some(&server_entry) {
            return MergeResult {
                value: Value::Object(root),
                changed: false,
            };
        }
        servers_map.insert(SERVER_NAME.to_string(), server_entry);
    }

    MergeResult {
        value: Value::Object(root),
        changed: true,
    }
}

// ---------------------------------------------------------------------------
// Removal
// ---------------------------------------------------------------------------

pub fn remove_mcp_config(mcp_path: &std::path::Path) -> Result<bool> {
    let existing = match read_json_file(mcp_path)? {
        Some(v) => v,
        None => return Ok(false),
    };

    let mut root = match existing {
        Value::Object(map) => map,
        _ => return Ok(false),
    };

    let had_dispatch = if let Some(Value::Object(servers)) = root.get_mut("mcpServers") {
        servers.remove(SERVER_NAME).is_some()
    } else {
        false
    };

    if had_dispatch {
        write_json_file(mcp_path, &Value::Object(root))?;
    }

    Ok(had_dispatch)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::DEFAULT_PORT;
    use serde_json::json;

    // -- MCP config merging --

    #[test]
    fn merge_mcp_config_into_empty() {
        let result = merge_mcp_config(None, DEFAULT_PORT);
        let dispatch = &result.value["mcpServers"]["dispatch"];
        assert_eq!(dispatch["type"], "http");
        assert_eq!(
            dispatch["url"],
            format!("http://localhost:{DEFAULT_PORT}/mcp")
        );
        assert!(dispatch["headersHelper"].is_string());
        assert!(result.changed);
    }

    #[test]
    fn merge_mcp_config_emits_headers_helper_pointing_at_caller_headers() {
        let result = merge_mcp_config(None, DEFAULT_PORT);
        let helper = result.value["mcpServers"]["dispatch"]["headersHelper"]
            .as_str()
            .unwrap();
        assert!(
            helper.ends_with("caller-headers"),
            "expected helper to end with 'caller-headers', got {helper}"
        );
    }

    #[test]
    fn merge_mcp_config_preserves_other_servers() {
        let existing = Some(json!({
            "mcpServers": {
                "github": {
                    "type": "http",
                    "url": "http://localhost:9999/mcp"
                }
            }
        }));
        let result = merge_mcp_config(existing, DEFAULT_PORT);
        assert!(result.changed);
        assert!(result.value["mcpServers"]["github"].is_object());
        assert_eq!(
            result.value["mcpServers"]["dispatch"]["url"],
            format!("http://localhost:{DEFAULT_PORT}/mcp")
        );
        assert!(result.value["mcpServers"]["dispatch"]["headersHelper"].is_string());
    }

    #[test]
    fn merge_mcp_config_already_configured() {
        // Pre-seed the existing entry with the same shape merge_mcp_config would write,
        // so the idempotency short-circuit fires.
        let first = merge_mcp_config(None, DEFAULT_PORT);
        let result = merge_mcp_config(Some(first.value.clone()), DEFAULT_PORT);
        assert!(
            !result.changed,
            "second merge with identical input must be a no-op"
        );
    }

    #[test]
    fn merge_mcp_config_rewrites_when_helper_missing() {
        // A user upgrading from a pre-headersHelper install: dispatch entry exists
        // with URL only. Merging must rewrite it to include the helper.
        let existing = Some(json!({
            "mcpServers": {
                "dispatch": {
                    "type": "http",
                    "url": format!("http://localhost:{DEFAULT_PORT}/mcp")
                }
            }
        }));
        let result = merge_mcp_config(existing, DEFAULT_PORT);
        assert!(result.changed);
        assert!(result.value["mcpServers"]["dispatch"]["headersHelper"].is_string());
    }

    #[test]
    fn merge_mcp_config_custom_port() {
        let result = merge_mcp_config(None, 4000);
        assert_eq!(
            result.value["mcpServers"]["dispatch"]["url"],
            "http://localhost:4000/mcp"
        );
    }

    // -- MCP config removal --

    #[test]
    fn remove_mcp_config_removes_dispatch_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        let existing = json!({
            "mcpServers": {
                "dispatch": {"type": "http", "url": "http://localhost:3142/mcp"},
                "github": {"type": "http", "url": "http://localhost:9999/mcp"}
            }
        });
        write_json_file(&path, &existing).unwrap();

        let removed = remove_mcp_config(&path).unwrap();
        assert!(removed);

        let result = read_json_file(&path).unwrap().unwrap();
        assert!(result["mcpServers"].get("dispatch").is_none());
        assert!(result["mcpServers"]["github"].is_object());
    }

    #[test]
    fn remove_mcp_config_noop_when_no_dispatch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        let existing = json!({
            "mcpServers": {
                "github": {"type": "http", "url": "http://localhost:9999/mcp"}
            }
        });
        write_json_file(&path, &existing).unwrap();

        let removed = remove_mcp_config(&path).unwrap();
        assert!(!removed);
    }

    #[test]
    fn remove_mcp_config_noop_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".mcp.json");

        let removed = remove_mcp_config(&path).unwrap();
        assert!(!removed);
    }

    #[test]
    fn remove_mcp_config_noop_when_root_is_not_object() {
        // File exists but top-level JSON is an array, not an object.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        // Write raw bytes since write_json_file expects an object Value
        std::fs::write(&path, "[1,2,3]\n").unwrap();

        let removed = remove_mcp_config(&path).unwrap();
        assert!(
            !removed,
            "non-object root JSON must be treated as no-op (no dispatch to remove)"
        );
    }

    #[test]
    fn remove_mcp_config_noop_when_mcp_servers_missing() {
        // Valid JSON object but no "mcpServers" key at all.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        write_json_file(&path, &json!({"otherKey": "value"})).unwrap();

        let removed = remove_mcp_config(&path).unwrap();
        assert!(!removed, "missing mcpServers must be treated as no-op");
    }
}
