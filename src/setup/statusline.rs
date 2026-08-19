//! Generates the dispatch-owned statusLine settings file that is injected into
//! every dispatch-spawned Claude session via `--settings`.
//!
//! The file lives at a **fixed literal path** (`~/.claude/dispatch-statusline.json`)
//! so the spawn constant in `src/dispatch/prompts.rs` stays a compile-time
//! `const` with no runtime path and no shell-quoting hazard. Runtime paths live
//! inside this file instead, where they can be quoted properly.
//!
//! Note it is NOT placed under the plugin dir: `remove_stale_files` deletes any
//! non-embedded file there. And it is NOT `~/.claude/settings.json`, which
//! `src/setup/mod.rs` deliberately never writes.

use anyhow::{Context, Result};
use serde_json::json;
use std::path::Path;

/// The fixed file name, under the resolved `~/.claude` directory.
///
/// `pub(crate)`: also read by `runtime::bootstrap` (`src/runtime/mod.rs`),
/// which recreates this file at TUI startup if `dispatch setup` was never
/// run — see the module doc comment above.
pub(crate) const SETTINGS_FILE_NAME: &str = "dispatch-statusline.json";

/// The fixed snapshot file name, under the data directory (the database's
/// parent).
///
/// Shared because two independent sites must agree on it: setup bakes this path
/// into the generated `--snapshot` argument (`src/setup/mod.rs`), and the TUI
/// reads it back (`src/runtime/mod.rs`). If those two ever disagreed the badge
/// would silently never appear — indistinguishable from "no subscription data" —
/// so the name lives in one place rather than as two literals.
///
/// Tests deliberately keep their own literal instead of importing this: an
/// expectation derived from the same constant as the code under test asserts
/// nothing.
pub(crate) const RATE_LIMITS_FILE_NAME: &str = "rate-limits.json";

/// POSIX single-quoting: wrap in `'…'` and replace each embedded `'` with
/// `'\''`. The generated string is run through `sh -c`, so an unquoted path
/// containing a space would split into two arguments.
pub(super) fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Build the statusLine command string.
pub(crate) fn build_command(snapshot_path: &Path, chain: Option<&str>) -> String {
    let mut cmd = format!(
        "dispatch statusline --snapshot {}",
        shell_quote(&snapshot_path.display().to_string())
    );
    if let Some(chain) = chain {
        cmd.push_str(&format!(" --chain {}", shell_quote(chain)));
    }
    cmd
}

/// Read the user's current `statusLine.command` so the decorator can chain to
/// it. Read-only — this never writes `settings.json`.
///
/// Returns `None` when there is nothing to chain, including the
/// **recursion-guard** case where the user's command is already a
/// `dispatch statusline` invocation. Chaining to ourselves would loop; the
/// honest outcome is an empty status line, with the reporter still running.
pub(crate) fn discover_chain(claude_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(claude_dir.join("settings.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let command = value
        .get("statusLine")?
        .get("command")?
        .as_str()?
        .trim()
        .to_string();
    if command.is_empty() || command.contains("dispatch statusline") {
        return None;
    }
    Some(command)
}

/// Write the settings file. Returns whether the on-disk content changed, so
/// setup can report accurately and stay idempotent.
///
/// The write-if-changed contract itself lives in [`super::write_file_if_changed`],
/// shared with the plugin installer — including the parent-directory creation
/// this path needs and the exact-bytes comparison both now use.
pub(crate) fn write_settings_file(
    path: &Path,
    snapshot_path: &Path,
    chain: Option<&str>,
) -> Result<bool> {
    let content = serde_json::to_string_pretty(&json!({
        "statusLine": {
            "type": "command",
            "command": build_command(snapshot_path, chain),
        },
        "sandbox": {
            "enabled": true,
            "excludedCommands": ["./gradlew *", "gradlew *", "gh *", "git fetch *", "git push *"],
            "network": {
                "allowedDomains": ["github.com", "api.github.com"],
            },
            "credentials": {
                "files": [
                    { "path": "~/.ssh", "mode": "deny" },
                    { "path": "~/.aws", "mode": "deny" },
                    { "path": "~/.config/gcloud", "mode": "deny" },
                    { "path": "~/.kube", "mode": "deny" },
                    { "path": "~/.docker", "mode": "deny" },
                    { "path": "~/.netrc", "mode": "deny" },
                ]
            }
        }
    }))
    .context("failed to serialize statusline settings")?;

    super::write_file_if_changed(path, &content, false)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn quotes_plain_path() {
        assert_eq!(shell_quote("/home/a/b.json"), "'/home/a/b.json'");
    }

    #[test]
    fn quotes_path_with_spaces() {
        assert_eq!(shell_quote("/home/my dir/b.json"), "'/home/my dir/b.json'");
    }

    #[test]
    fn escapes_embedded_single_quote() {
        // A path containing a single quote must not terminate the quoting.
        assert_eq!(shell_quote("/home/o'brien/b"), r#"'/home/o'\''brien/b'"#);
    }

    #[test]
    fn builds_command_with_chain() {
        let cmd = build_command(Path::new("/d/rate-limits.json"), Some("claude-statusline"));
        assert_eq!(
            cmd,
            "dispatch statusline --snapshot '/d/rate-limits.json' --chain 'claude-statusline'"
        );
    }

    #[test]
    fn builds_command_without_chain() {
        let cmd = build_command(Path::new("/d/rate-limits.json"), None);
        assert_eq!(cmd, "dispatch statusline --snapshot '/d/rate-limits.json'");
    }

    #[test]
    fn discovers_existing_status_line_command() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("settings.json"),
            r#"{"statusLine":{"type":"command","command":"claude-statusline"}}"#,
        )
        .unwrap();
        assert_eq!(
            discover_chain(tmp.path()).as_deref(),
            Some("claude-statusline")
        );
    }

    #[test]
    fn discovers_none_when_no_settings_file() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(discover_chain(tmp.path()), None);
    }

    #[test]
    fn discovers_none_when_no_status_line_key() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("settings.json"), r#"{"permissions":{}}"#).unwrap();
        assert_eq!(discover_chain(tmp.path()), None);
    }

    #[test]
    fn discovers_none_when_settings_malformed() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("settings.json"), "{ not json").unwrap();
        assert_eq!(discover_chain(tmp.path()), None);
    }

    #[test]
    fn recursion_guard_refuses_to_chain_to_itself() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("settings.json"),
            r#"{"statusLine":{"type":"command","command":"dispatch statusline --snapshot /d/x.json"}}"#,
        )
        .unwrap();
        assert_eq!(
            discover_chain(tmp.path()),
            None,
            "must not chain to a dispatch statusline invocation"
        );
    }

    /// Writes settings to a fresh tempdir and parses them back, returning
    /// whether the write reported a change alongside the parsed value.
    fn write_and_parse(chain: Option<&str>) -> (bool, serde_json::Value) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("dispatch-statusline.json");
        let changed = write_settings_file(&path, Path::new("/d/rl.json"), chain).unwrap();
        let v = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        (changed, v)
    }

    /// Extracts a JSON array of strings as borrowed `&str`s, panicking with a
    /// descriptive message if the value isn't shaped that way.
    fn str_array(v: &serde_json::Value) -> Vec<&str> {
        v.as_array()
            .expect("expected a JSON array")
            .iter()
            .map(|entry| entry.as_str().expect("array entry must be a string"))
            .collect()
    }

    #[test]
    fn writes_valid_settings_json() {
        let (changed, v) = write_and_parse(Some("cs"));
        assert!(changed);
        assert_eq!(v["statusLine"]["type"], "command");
        assert_eq!(
            v["statusLine"]["command"],
            "dispatch statusline --snapshot '/d/rl.json' --chain 'cs'"
        );
    }

    #[test]
    fn writes_sandbox_config_enabled_with_no_filesystem_key() {
        let (_, v) = write_and_parse(None);
        assert_eq!(v["sandbox"]["enabled"], true);
        assert!(
            v["sandbox"]["filesystem"].is_null(),
            "filesystem key must be absent so Claude Code's defaults apply"
        );
        assert!(
            v["sandbox"]["failIfUnavailable"].is_null(),
            "failIfUnavailable must be absent so sandboxing fails open"
        );
    }

    #[test]
    fn writes_sandbox_allowed_domains_for_github_only() {
        let (_, v) = write_and_parse(None);
        let domains = str_array(&v["sandbox"]["network"]["allowedDomains"]);
        assert_eq!(
            domains,
            vec!["github.com", "api.github.com"],
            "git/gh are hard dependencies on every dispatched task, so their \
             hosts are pre-allowed; other ecosystems' registries are not"
        );
        assert!(
            v["sandbox"]["network"]["strictAllowlist"].is_null(),
            "strictAllowlist must be absent so hosts outside allowedDomains \
             still go through the normal prompt/classifier flow"
        );
    }

    #[test]
    fn writes_sandbox_excluded_commands_exactly() {
        let (_, v) = write_and_parse(None);
        let excluded = str_array(&v["sandbox"]["excludedCommands"]);
        assert_eq!(
            excluded,
            vec![
                "./gradlew *",
                "gradlew *",
                "gh *",
                "git fetch *",
                "git push *"
            ],
            "each entry here must correspond 1:1 to a documented \
             @guarantee on SandboxedAgentExecution in dispatch.allium"
        );
    }

    #[test]
    fn writes_sandbox_excluded_commands_for_gradle_wrapper() {
        let (_, v) = write_and_parse(None);
        let excluded = str_array(&v["sandbox"]["excludedCommands"]);
        assert!(
            excluded.contains(&"./gradlew *") && excluded.contains(&"gradlew *"),
            "Gradle's fresh-daemon startup needs CLONE_NEWUSER, which the \
             sandbox always blocks with no narrower fix available — see \
             GradleDaemonExcludedFromSandbox in dispatch.allium"
        );
    }

    #[test]
    fn writes_sandbox_excluded_commands_for_gh_cli() {
        let (_, v) = write_and_parse(None);
        let excluded = str_array(&v["sandbox"]["excludedCommands"]);
        assert!(
            excluded.contains(&"gh *"),
            "gh stores its token in the OS keyring, reachable only over a \
             D-Bus AF_UNIX socket the sandbox's seccomp policy always \
             blocks — see GhCliExcludedFromSandboxKeyring in dispatch.allium"
        );
    }

    #[test]
    fn writes_sandbox_excluded_commands_for_git_ssh_remotes() {
        let (_, v) = write_and_parse(None);
        let excluded = str_array(&v["sandbox"]["excludedCommands"]);
        assert!(
            excluded.contains(&"git fetch *") && excluded.contains(&"git push *"),
            "allowedDomains only matches HTTP(S) hosts, so git fetch/push over \
             an SSH remote is blocked regardless — see \
             GitSshFetchPushExcludedFromSandbox in dispatch.allium"
        );
    }

    #[test]
    fn writes_sandbox_credential_deny_list() {
        let (_, v) = write_and_parse(None);
        let files = v["sandbox"]["credentials"]["files"]
            .as_array()
            .expect("credentials.files must be an array");
        let denied_paths: Vec<&str> = files
            .iter()
            .map(|f| f["path"].as_str().expect("path must be a string"))
            .collect();
        for expected in [
            "~/.ssh",
            "~/.aws",
            "~/.config/gcloud",
            "~/.kube",
            "~/.docker",
            "~/.netrc",
        ] {
            assert!(
                denied_paths.contains(&expected),
                "expected {expected} in denied_paths, got {denied_paths:?}"
            );
        }
        assert!(
            files.iter().all(|f| f["mode"] == "deny"),
            "every credential entry must use mode: deny, got {files:?}"
        );
        assert!(
            v["sandbox"]["credentials"]["envVars"].is_null(),
            "envVars must be absent — denying tokens like GITHUB_TOKEN breaks gh"
        );
    }

    #[test]
    fn write_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("dispatch-statusline.json");
        assert!(write_settings_file(&path, Path::new("/d/rl.json"), Some("cs")).unwrap());
        assert!(
            !write_settings_file(&path, Path::new("/d/rl.json"), Some("cs")).unwrap(),
            "second identical write must report no change"
        );
    }

    /// The one thing this file owns about writing: that it goes through the shared
    /// helper at all rather than a bare `fs::write`. The write-if-changed contract
    /// itself — exact-bytes comparison included — is asserted where it lives, in
    /// `src/setup/mod.rs`.
    #[test]
    fn write_creates_missing_parent_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("claude").join("dispatch-statusline.json");
        assert!(write_settings_file(&path, Path::new("/d/rl.json"), None).unwrap());
        assert!(path.exists());
    }

    #[test]
    fn write_reports_change_when_chain_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("dispatch-statusline.json");
        write_settings_file(&path, Path::new("/d/rl.json"), Some("old")).unwrap();
        assert!(write_settings_file(&path, Path::new("/d/rl.json"), Some("new")).unwrap());
    }
}
