use anyhow::{Context, Result};
use serde_json::{json, Map, Value};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

const HOOK_SCRIPT: &str = include_str!("../hooks/task-status-hook");
const USAGE_HOOK_SCRIPT: &str = include_str!("../hooks/task-usage-hook");

// ---------------------------------------------------------------------------
// MCP config merging
// ---------------------------------------------------------------------------

pub struct MergeResult {
    pub value: Value,
    pub changed: bool,
}

pub fn merge_mcp_config(existing: Option<Value>, port: u16) -> MergeResult {
    let server_entry = json!({
        "type": "http",
        "url": format!("http://localhost:{port}/mcp")
    });

    let mut root = match existing {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };

    let servers = root
        .entry("mcpServers")
        .or_insert_with(|| json!({}));

    if let Value::Object(servers_map) = servers {
        if servers_map.get("dispatch") == Some(&server_entry) {
            return MergeResult {
                value: Value::Object(root),
                changed: false,
            };
        }
        servers_map.insert("dispatch".to_string(), server_entry);
    }

    MergeResult {
        value: Value::Object(root),
        changed: true,
    }
}

// ---------------------------------------------------------------------------
// Permissions merging
// ---------------------------------------------------------------------------

const MCP_PERMISSIONS: &[&str] = &[
    "mcp__dispatch__update_task",
    "mcp__dispatch__get_task",
    "mcp__dispatch__create_task",
    "mcp__dispatch__report_usage",
];

pub struct PermissionsMergeResult {
    pub value: Value,
    pub added_count: usize,
}

pub fn merge_permissions(existing: Option<Value>) -> PermissionsMergeResult {
    let mut root = match existing {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };

    let permissions = root
        .entry("permissions")
        .or_insert_with(|| json!({}));

    let allow = permissions
        .get("allow")
        .and_then(|a| a.as_array())
        .cloned()
        .unwrap_or_default();

    let mut added_count = 0;
    let mut new_allow = allow;
    for perm in MCP_PERMISSIONS {
        let val = Value::String(perm.to_string());
        if !new_allow.contains(&val) {
            new_allow.push(val);
            added_count += 1;
        }
    }

    if let Value::Object(ref mut perms_map) = permissions {
        perms_map.insert("allow".to_string(), Value::Array(new_allow));
    }

    PermissionsMergeResult {
        value: Value::Object(root),
        added_count,
    }
}

// ---------------------------------------------------------------------------
// Hook script installation
// ---------------------------------------------------------------------------

fn local_bin_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("$HOME is not set")?;
    Ok(PathBuf::from(home).join(".local").join("bin"))
}

pub fn install_hook_script() -> Result<bool> {
    let bin_dir = local_bin_dir()?;
    fs::create_dir_all(&bin_dir)
        .with_context(|| format!("Failed to create {}", bin_dir.display()))?;

    let mut changed = false;
    changed |= install_single_hook(&bin_dir, "task-status-hook", HOOK_SCRIPT)?;
    changed |= install_single_hook(&bin_dir, "task-usage-hook", USAGE_HOOK_SCRIPT)?;
    Ok(changed)
}

fn install_single_hook(bin_dir: &std::path::Path, name: &str, content: &str) -> Result<bool> {
    let hook_path = bin_dir.join(name);
    if hook_path.exists() {
        let existing = fs::read_to_string(&hook_path)
            .with_context(|| format!("Failed to read {}", hook_path.display()))?;
        if existing == content {
            return Ok(false);
        }
    }
    fs::write(&hook_path, content)
        .with_context(|| format!("Failed to write {}", hook_path.display()))?;
    fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755))
        .with_context(|| format!("Failed to set permissions on {}", hook_path.display()))?;
    Ok(true)
}

// ---------------------------------------------------------------------------
// File I/O helpers
// ---------------------------------------------------------------------------

fn claude_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("$HOME is not set")?;
    Ok(PathBuf::from(home).join(".claude"))
}

fn read_json_file(path: &std::path::Path) -> Result<Option<Value>> {
    match fs::read_to_string(path) {
        Ok(content) => {
            let value: Value = serde_json::from_str(&content)
                .with_context(|| format!("Invalid JSON in {}", path.display()))?;
            Ok(Some(value))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("Failed to read {}", path.display())),
    }
}

fn write_json_file(path: &std::path::Path, value: &Value) -> Result<()> {
    let content = serde_json::to_string_pretty(value).context("Failed to serialize JSON")?;
    fs::write(path, content + "\n")
        .with_context(|| format!("Failed to write {}", path.display()))
}

// ---------------------------------------------------------------------------
// run_setup — top-level orchestrator
// ---------------------------------------------------------------------------

pub fn run_setup(port: u16) -> Result<()> {
    let claude_dir = claude_dir()?;
    fs::create_dir_all(&claude_dir)
        .with_context(|| format!("Failed to create {}", claude_dir.display()))?;

    let mut any_changes = false;

    // 1. MCP config
    let mcp_path = claude_dir.join(".mcp.json");
    let existing_mcp = read_json_file(&mcp_path)?;
    let mcp_result = merge_mcp_config(existing_mcp, port);
    if mcp_result.changed {
        write_json_file(&mcp_path, &mcp_result.value)?;
        println!("MCP config: added dispatch to ~/.claude/.mcp.json (port {port})");
        any_changes = true;
    } else {
        println!("MCP config: dispatch already configured in ~/.claude/.mcp.json");
    }

    // 2. Permissions
    let settings_path = claude_dir.join("settings.json");
    let existing_settings = read_json_file(&settings_path)?;
    let perms_result = merge_permissions(existing_settings);
    if perms_result.added_count > 0 {
        write_json_file(&settings_path, &perms_result.value)?;
        println!(
            "Permissions: added {} MCP tool permission(s) to ~/.claude/settings.json",
            perms_result.added_count
        );
        any_changes = true;
    } else {
        println!(
            "Permissions: all MCP tool permissions already present in ~/.claude/settings.json"
        );
    }

    // 3. Hook script
    if install_hook_script()? {
        println!("Hook scripts: installed task-status-hook and task-usage-hook to ~/.local/bin/");
        any_changes = true;
    } else {
        println!("Hook scripts: task-status-hook and task-usage-hook already up to date in ~/.local/bin/");
    }

    if any_changes {
        println!("Setup complete.");
    } else {
        println!("Already configured, nothing to do.");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -- MCP config merging --

    #[test]
    fn merge_mcp_config_into_empty() {
        let existing = None;
        let result = merge_mcp_config(existing, 3142);
        let expected = json!({
            "mcpServers": {
                "dispatch": {
                    "type": "http",
                    "url": "http://localhost:3142/mcp"
                }
            }
        });
        assert_eq!(result.value, expected);
        assert!(result.changed);
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
        let result = merge_mcp_config(existing, 3142);
        assert!(result.changed);
        assert!(result.value["mcpServers"]["github"].is_object());
        assert_eq!(
            result.value["mcpServers"]["dispatch"]["url"],
            "http://localhost:3142/mcp"
        );
    }

    #[test]
    fn merge_mcp_config_already_configured() {
        let existing = Some(json!({
            "mcpServers": {
                "dispatch": {
                    "type": "http",
                    "url": "http://localhost:3142/mcp"
                }
            }
        }));
        let result = merge_mcp_config(existing, 3142);
        assert!(!result.changed);
    }

    #[test]
    fn merge_mcp_config_custom_port() {
        let result = merge_mcp_config(None, 4000);
        assert_eq!(
            result.value["mcpServers"]["dispatch"]["url"],
            "http://localhost:4000/mcp"
        );
    }

    // -- Permissions merging --

    #[test]
    fn merge_permissions_into_empty() {
        let existing = None;
        let result = merge_permissions(existing);
        let allow = result.value["permissions"]["allow"].as_array().unwrap();
        assert!(allow.contains(&json!("mcp__dispatch__update_task")));
        assert!(allow.contains(&json!("mcp__dispatch__get_task")));
        assert!(allow.contains(&json!("mcp__dispatch__create_task")));
        assert!(allow.contains(&json!("mcp__dispatch__report_usage")));
        assert_eq!(result.added_count, 4);
    }

    #[test]
    fn merge_permissions_preserves_existing() {
        let existing = Some(json!({
            "permissions": {
                "allow": ["Bash(git:*)", "Read"],
                "defaultMode": "acceptEdits"
            },
            "hooks": {"Stop": []}
        }));
        let result = merge_permissions(existing);
        let allow = result.value["permissions"]["allow"].as_array().unwrap();
        assert!(allow.contains(&json!("Bash(git:*)")));
        assert!(allow.contains(&json!("Read")));
        assert!(allow.contains(&json!("mcp__dispatch__update_task")));
        assert_eq!(result.added_count, 4);
        assert_eq!(result.value["permissions"]["defaultMode"], "acceptEdits");
        assert!(result.value["hooks"]["Stop"].is_array());
    }

    #[test]
    fn merge_permissions_already_present() {
        let existing = Some(json!({
            "permissions": {
                "allow": [
                    "mcp__dispatch__update_task",
                    "mcp__dispatch__get_task",
                    "mcp__dispatch__create_task",
                    "mcp__dispatch__report_usage"
                ]
            }
        }));
        let result = merge_permissions(existing);
        assert_eq!(result.added_count, 0);
    }

    // -- Hook script --

    #[test]
    fn hook_script_is_valid_bash() {
        assert!(HOOK_SCRIPT.starts_with("#!/usr/bin/env bash"));
    }

    #[test]
    fn usage_hook_script_is_valid_bash() {
        assert!(USAGE_HOOK_SCRIPT.starts_with("#!/usr/bin/env bash"));
    }

    #[test]
    fn hook_script_handles_all_events() {
        assert!(HOOK_SCRIPT.contains("PreToolUse)"));
        assert!(HOOK_SCRIPT.contains("Stop)"));
        assert!(HOOK_SCRIPT.contains("Notification)"));
    }

    // -- File I/O --

    #[test]
    fn read_json_file_missing_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.json");
        let result = read_json_file(&path).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn read_json_file_invalid_json_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        fs::write(&path, "not json").unwrap();
        let result = read_json_file(&path);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid JSON"),);
    }

    #[test]
    fn write_and_read_json_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.json");
        let value = json!({"key": "value"});
        write_json_file(&path, &value).unwrap();
        let read_back = read_json_file(&path).unwrap().unwrap();
        assert_eq!(read_back, value);
    }
}
