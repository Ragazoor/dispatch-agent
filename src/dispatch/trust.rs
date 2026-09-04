use crate::models::expand_tilde;
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

fn project_key(repo_path: &str) -> String {
    expand_tilde(repo_path).trim_end_matches('/').to_string()
}

/// The path-parameterised half of the trust check. `TuiRuntime::claude_json_path`
/// carries the path a caller should use — the real `$HOME/.claude.json` in
/// production, an injected tempfile in tests — so this is the seam a
/// `commands::dispatch` test can point elsewhere without mutating the real
/// file. The production default is `crate::setup::user_global_config_path`,
/// resolved once by `runtime::StartupPaths::resolve` — one resolution shared
/// with the configuration directory beside it, so an absent `$HOME` is
/// reported rather than composed into a location at the filesystem root. See
/// docs/specs/dispatch.allium:
/// `AnUnavailableHomeDirectoryIsAFailureNotAPath`.
pub(crate) fn is_trusted_at(claude_json: &Path, repo_path: &str) -> Result<bool> {
    if !claude_json.exists() {
        return Ok(false);
    }
    let content = std::fs::read_to_string(claude_json)
        .with_context(|| format!("failed to read {}", claude_json.display()))?;
    let json: Value = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse {}", claude_json.display()))?;
    let key = project_key(repo_path);
    Ok(json["projects"][key.as_str()]["hasTrustDialogAccepted"]
        .as_bool()
        .unwrap_or(false))
}

/// The path-parameterised half of granting trust. See [`is_trusted_at`] for
/// why this takes the path rather than resolving it itself.
pub(crate) fn trust_at(claude_json: &Path, repo_path: &str) -> Result<()> {
    let mut json: Value = if claude_json.exists() {
        let content = std::fs::read_to_string(claude_json)
            .with_context(|| format!("failed to read {}", claude_json.display()))?;
        serde_json::from_str(&content)
            .with_context(|| format!("failed to parse {}", claude_json.display()))?
    } else {
        serde_json::json!({})
    };
    let key = project_key(repo_path);
    json.as_object_mut()
        .context("~/.claude.json root is not a JSON object")?
        .entry("projects")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .context("~/.claude.json 'projects' is not a JSON object")?
        .entry(key)
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .context("project entry is not a JSON object")?
        .insert("hasTrustDialogAccepted".to_string(), Value::Bool(true));
    let updated =
        serde_json::to_string_pretty(&json).context("failed to serialize ~/.claude.json")?;
    std::fs::write(claude_json, updated)
        .with_context(|| format!("failed to write {}", claude_json.display()))?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn json_path(dir: &tempfile::TempDir) -> std::path::PathBuf {
        dir.path().join(".claude.json")
    }

    #[test]
    fn absent_file_is_not_trusted() {
        let dir = tempdir().unwrap();
        assert!(!is_trusted_at(&json_path(&dir), "/repo").unwrap());
    }

    #[test]
    fn trusted_repo_is_detected() {
        let dir = tempdir().unwrap();
        let content = serde_json::json!({
            "projects": { "/repo": { "hasTrustDialogAccepted": true } }
        });
        std::fs::write(json_path(&dir), content.to_string()).unwrap();
        assert!(is_trusted_at(&json_path(&dir), "/repo").unwrap());
    }

    #[test]
    fn explicit_false_is_not_trusted() {
        let dir = tempdir().unwrap();
        let content = serde_json::json!({
            "projects": { "/repo": { "hasTrustDialogAccepted": false } }
        });
        std::fs::write(json_path(&dir), content.to_string()).unwrap();
        assert!(!is_trusted_at(&json_path(&dir), "/repo").unwrap());
    }

    #[test]
    fn missing_project_entry_is_not_trusted() {
        let dir = tempdir().unwrap();
        let content = serde_json::json!({ "projects": {} });
        std::fs::write(json_path(&dir), content.to_string()).unwrap();
        assert!(!is_trusted_at(&json_path(&dir), "/repo").unwrap());
    }

    #[test]
    fn malformed_json_returns_err() {
        let dir = tempdir().unwrap();
        std::fs::write(json_path(&dir), "not json").unwrap();
        assert!(is_trusted_at(&json_path(&dir), "/repo").is_err());
    }

    #[test]
    fn trust_repo_creates_file_when_absent() {
        let dir = tempdir().unwrap();
        trust_at(&json_path(&dir), "/repo").unwrap();
        let content = std::fs::read_to_string(json_path(&dir)).unwrap();
        let v: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(v["projects"]["/repo"]["hasTrustDialogAccepted"], true);
    }

    #[test]
    fn trust_repo_sets_flag_on_existing_entry() {
        let dir = tempdir().unwrap();
        let content = serde_json::json!({
            "projects": { "/repo": { "someOtherKey": "preserved" } }
        });
        std::fs::write(json_path(&dir), content.to_string()).unwrap();
        trust_at(&json_path(&dir), "/repo").unwrap();
        let updated: Value =
            serde_json::from_str(&std::fs::read_to_string(json_path(&dir)).unwrap()).unwrap();
        assert_eq!(updated["projects"]["/repo"]["hasTrustDialogAccepted"], true);
        assert_eq!(updated["projects"]["/repo"]["someOtherKey"], "preserved");
    }

    #[test]
    fn trust_repo_is_idempotent() {
        let dir = tempdir().unwrap();
        trust_at(&json_path(&dir), "/repo").unwrap();
        let before = std::fs::read_to_string(json_path(&dir)).unwrap();
        trust_at(&json_path(&dir), "/repo").unwrap();
        let after = std::fs::read_to_string(json_path(&dir)).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn tilde_in_repo_path_is_expanded() {
        let dir = tempdir().unwrap();
        let home = std::env::var("HOME").unwrap();
        let content = serde_json::json!({
            "projects": { format!("{home}/myrepo"): { "hasTrustDialogAccepted": true } }
        });
        std::fs::write(json_path(&dir), content.to_string()).unwrap();
        assert!(is_trusted_at(&json_path(&dir), "~/myrepo").unwrap());
    }
}
