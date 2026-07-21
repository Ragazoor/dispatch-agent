//! Plugin install: skills, slash commands, hooks (embedded at compile time),
//! plus the example feed script and feed-epic seeding.

use anyhow::{Context, Result};
use include_dir::{include_dir, Dir};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::db::{Database, EpicCrud, EpicPatch, EpicRead};

// The entire plugin/ directory is embedded at compile time. Any file added to
// plugin/ is automatically picked up — no manual registration required.
pub(super) static PLUGIN_DIR: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/plugin");

// ---------------------------------------------------------------------------
// Plugin installation
// ---------------------------------------------------------------------------

pub(super) fn plugin_dir() -> Result<PathBuf> {
    Ok(plugin_dir_under(&super::claude_dir()?))
}

/// Resolve the plugin install directory beneath an explicit `~/.claude`-style
/// directory. Kept separate from [`plugin_dir`] so orchestration code can
/// inject a temp directory in tests.
pub(super) fn plugin_dir_under(claude_dir: &Path) -> PathBuf {
    claude_dir.join("plugins").join("local").join("dispatch")
}

fn is_executable(path: &std::path::Path) -> bool {
    path.starts_with("hooks/scripts")
}

pub(super) fn install_plugin_in(base: &Path) -> Result<bool> {
    let mut changed = false;
    install_dir_recursive(&PLUGIN_DIR, base, &mut changed)?;
    remove_stale_files(base, &mut changed)?;
    Ok(changed)
}

fn install_dir_recursive(dir: &Dir, base: &std::path::Path, changed: &mut bool) -> Result<()> {
    for file in dir.files() {
        let path = base.join(file.path());
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create {}", parent.display()))?;
        }
        let content = file
            .contents_utf8()
            .with_context(|| format!("Non-UTF-8 plugin file: {}", file.path().display()))?;
        *changed |= write_file_if_changed(&path, content, is_executable(file.path()))?;
    }
    for subdir in dir.dirs() {
        install_dir_recursive(subdir, base, changed)?;
    }
    Ok(())
}

fn embedded_path_set() -> std::collections::HashSet<PathBuf> {
    fn collect(dir: &Dir, paths: &mut std::collections::HashSet<PathBuf>) {
        for file in dir.files() {
            paths.insert(file.path().to_path_buf());
        }
        for subdir in dir.dirs() {
            collect(subdir, paths);
        }
    }
    let mut paths = std::collections::HashSet::new();
    collect(&PLUGIN_DIR, &mut paths);
    paths
}

fn remove_stale_files(base: &Path, changed: &mut bool) -> Result<()> {
    if !base.exists() {
        return Ok(());
    }
    let embedded = embedded_path_set();
    remove_stale_recursive(base, base, &embedded, changed)
}

fn remove_stale_recursive(
    base: &Path,
    dir: &Path,
    embedded: &std::collections::HashSet<PathBuf>,
    changed: &mut bool,
) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("Failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            remove_stale_recursive(base, &path, embedded, changed)?;
            if fs::read_dir(&path)?.next().is_none() {
                fs::remove_dir(&path)
                    .with_context(|| format!("Failed to remove {}", path.display()))?;
                *changed = true;
            }
        } else {
            let relative = path.strip_prefix(base).with_context(|| {
                format!(
                    "path {} is not under base {}",
                    path.display(),
                    base.display()
                )
            })?;
            if !embedded.contains(relative) {
                fs::remove_file(&path)
                    .with_context(|| format!("Failed to remove {}", path.display()))?;
                *changed = true;
            }
        }
    }
    Ok(())
}

fn write_file_if_changed(path: &std::path::Path, content: &str, executable: bool) -> Result<bool> {
    if path.exists() {
        let existing = fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        if existing == content {
            return Ok(false);
        }
    }
    fs::write(path, content).with_context(|| format!("Failed to write {}", path.display()))?;
    if executable {
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .with_context(|| format!("Failed to set permissions on {}", path.display()))?;
    }
    Ok(true)
}

pub(super) fn plugin_needs_update_in(base: &std::path::Path) -> Result<bool> {
    if needs_update_recursive(&PLUGIN_DIR, base)? {
        return Ok(true);
    }
    has_stale_files(base)
}

fn needs_update_recursive(dir: &Dir, base: &std::path::Path) -> Result<bool> {
    for file in dir.files() {
        let path = base.join(file.path());
        let content = file.contents_utf8().unwrap_or("");
        match fs::read_to_string(&path) {
            Ok(existing) if existing == content => continue,
            _ => return Ok(true),
        }
    }
    for subdir in dir.dirs() {
        if needs_update_recursive(subdir, base)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn has_stale_files(base: &Path) -> Result<bool> {
    if !base.exists() {
        return Ok(false);
    }
    let embedded = embedded_path_set();
    has_stale_recursive(base, base, &embedded)
}

fn has_stale_recursive(
    base: &Path,
    dir: &Path,
    embedded: &std::collections::HashSet<PathBuf>,
) -> Result<bool> {
    for entry in fs::read_dir(dir).with_context(|| format!("Failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            if has_stale_recursive(base, &path, embedded)? {
                return Ok(true);
            }
        } else {
            let relative = path.strip_prefix(base).with_context(|| {
                format!(
                    "path {} is not under base {}",
                    path.display(),
                    base.display()
                )
            })?;
            if !embedded.contains(relative) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub fn remove_plugin(plugin_path: &std::path::Path) -> Result<bool> {
    if !plugin_path.exists() {
        return Ok(false);
    }
    fs::remove_dir_all(plugin_path)
        .with_context(|| format!("Failed to remove {}", plugin_path.display()))?;
    Ok(true)
}

// ---------------------------------------------------------------------------
// Example feed script + epic seeding
// ---------------------------------------------------------------------------

const EXAMPLE_FEED_SCRIPT: &str = include_str!("../../scripts/fetch-dependabot.sh");
const EXAMPLE_REPOS_CONF: &str = include_str!("../../scripts/repos.conf");

/// Create `path` with `content` only if it does not already exist. Preserves
/// user edits across repeated `dispatch setup` runs.
fn install_if_absent(path: &std::path::Path, content: &str, executable: bool) -> Result<()> {
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => {
            file.write_all(content.as_bytes())
                .with_context(|| format!("Failed to write {}", path.display()))?;
            if executable {
                fs::set_permissions(path, fs::Permissions::from_mode(0o755))
                    .with_context(|| format!("Failed to set permissions on {}", path.display()))?;
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => {
            Err(anyhow::Error::new(e).context(format!("Failed to create {}", path.display())))
        }
    }
}

/// Write the embedded example feed script and repos.conf to `<data_dir>/scripts/`.
/// Idempotent: existing files are left untouched so user edits survive across
/// `dispatch setup` runs.
pub fn install_example_script(data_dir: &Path) -> Result<PathBuf> {
    let scripts_dir = data_dir.join("scripts");
    fs::create_dir_all(&scripts_dir)
        .with_context(|| format!("Failed to create {}", scripts_dir.display()))?;

    let path = scripts_dir.join("fetch-dependabot.sh");
    install_if_absent(&path, EXAMPLE_FEED_SCRIPT, true)?;
    install_if_absent(&scripts_dir.join("repos.conf"), EXAMPLE_REPOS_CONF, false)?;
    Ok(path)
}

/// Seed exactly one example feed epic ("Dependabot") wired to the installed
/// example script. Idempotent: re-running does not duplicate the epic.
pub async fn seed_feed_epics(db: &Database, data_dir: &Path) -> Result<()> {
    let script_path = install_example_script(data_dir)?;
    let cmd = script_path
        .to_str()
        .context("example script path is not valid UTF-8")?;

    let already_seeded = db
        .list_epics()
        .await?
        .iter()
        .any(|e| e.feed_command.as_deref() == Some(cmd));
    if already_seeded {
        return Ok(());
    }

    let epic = db.create_epic("Dependabot", "", None).await?;
    db.patch_epic(
        epic.id,
        &EpicPatch::new()
            .feed_command(Some(cmd))
            .feed_interval_secs(Some(300))
            .sort_order(Some(0)),
    )
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use serde_json::Value;

    // -- seed_feed_epics --

    #[tokio::test]
    async fn seed_feed_epics_creates_single_example_epic() {
        let db = Database::open_in_memory().await.unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        seed_feed_epics(&db, data_dir.path()).await.unwrap();

        let epics = db.list_epics().await.unwrap();
        assert_eq!(
            epics.len(),
            1,
            "setup must seed exactly one example feed epic"
        );

        let epic = &epics[0];
        assert_eq!(epic.title, "Dependabot");
        assert_eq!(epic.sort_order, Some(0));
        assert_eq!(epic.feed_interval_secs, Some(300));

        let expected_path = data_dir.path().join("scripts").join("fetch-dependabot.sh");
        assert_eq!(
            epic.feed_command.as_deref(),
            Some(expected_path.to_str().unwrap())
        );
    }

    #[tokio::test]
    async fn seed_feed_epics_is_idempotent() {
        let db = Database::open_in_memory().await.unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        seed_feed_epics(&db, data_dir.path()).await.unwrap();
        seed_feed_epics(&db, data_dir.path()).await.unwrap();

        let epics = db.list_epics().await.unwrap();
        assert_eq!(epics.len(), 1, "Dependabot epic must not be duplicated");
    }

    #[test]
    fn shipped_fetch_dependabot_script_emits_dependabot_tag() {
        let body = EXAMPLE_FEED_SCRIPT;
        assert!(
            body.contains("tag: \"dependabot\""),
            "fetch-dependabot.sh must emit tag \"dependabot\""
        );
        assert!(
            !body.contains("tag: \"pr-review\""),
            "fetch-dependabot.sh must no longer emit tag \"pr-review\""
        );
    }

    #[test]
    fn shipped_fetch_dependabot_script_filters_on_renovate_bot() {
        let body = EXAMPLE_FEED_SCRIPT;
        assert!(
            body.contains("--author app/kognic-renovate"),
            "fetch-dependabot.sh must filter PRs on the Renovate bot (app/kognic-renovate)"
        );
        assert!(
            !body.contains("app/dependabot"),
            "fetch-dependabot.sh must no longer filter on app/dependabot"
        );
    }

    // -- install_example_script --

    #[test]
    fn install_example_script_writes_executable_file() {
        use std::os::unix::fs::PermissionsExt;
        let data_dir = tempfile::tempdir().unwrap();
        let path = install_example_script(data_dir.path()).unwrap();
        assert!(path.exists());
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o111,
            0o111,
            "example script must be executable for owner/group/other"
        );
    }

    #[test]
    fn install_example_script_is_idempotent() {
        let data_dir = tempfile::tempdir().unwrap();
        let p1 = install_example_script(data_dir.path()).unwrap();
        let c1 = std::fs::read_to_string(&p1).unwrap();
        let p2 = install_example_script(data_dir.path()).unwrap();
        let c2 = std::fs::read_to_string(&p2).unwrap();
        assert_eq!(p1, p2);
        assert_eq!(c1, c2);
    }

    #[test]
    fn install_example_script_preserves_user_edits() {
        let data_dir = tempfile::tempdir().unwrap();
        let path = install_example_script(data_dir.path()).unwrap();
        std::fs::write(&path, "#!/usr/bin/env bash\nexit 0\n").unwrap();
        let after = install_example_script(data_dir.path()).unwrap();
        assert_eq!(path, after);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "#!/usr/bin/env bash\nexit 0\n",
            "install must not overwrite user edits to the example script"
        );
    }

    // -- repos.conf --

    #[test]
    fn install_example_script_also_installs_repos_conf() {
        let data_dir = tempfile::tempdir().unwrap();
        install_example_script(data_dir.path()).unwrap();
        let repos_conf = data_dir.path().join("scripts").join("repos.conf");
        assert!(
            repos_conf.exists(),
            "repos.conf must be installed alongside fetch-dependabot.sh"
        );
    }

    #[test]
    fn install_example_script_preserves_user_repos_conf() {
        let data_dir = tempfile::tempdir().unwrap();
        install_example_script(data_dir.path()).unwrap();
        let repos_conf = data_dir.path().join("scripts").join("repos.conf");
        std::fs::write(&repos_conf, "REPOS=(\"myorg/custom\")\n").unwrap();
        install_example_script(data_dir.path()).unwrap();
        let content = std::fs::read_to_string(&repos_conf).unwrap();
        assert_eq!(
            content, "REPOS=(\"myorg/custom\")\n",
            "install must not overwrite user edits to repos.conf"
        );
    }

    #[test]
    fn fetch_dependabot_uses_repos_conf_when_present() {
        // Write a repos.conf with a fake repo; the script should attempt to probe
        // it and fail — but the failure message confirms repos.conf was sourced.
        let data_dir = tempfile::tempdir().unwrap();
        let script_path = install_example_script(data_dir.path()).unwrap();
        let repos_conf = data_dir.path().join("scripts").join("repos.conf");
        std::fs::write(&repos_conf, "REPOS=(\"fake-owner/fake-repo-xyz\")\n").unwrap();

        let output = std::process::Command::new("bash")
            .arg(&script_path)
            .output()
            .expect("script must be runnable");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("fake-owner/fake-repo-xyz"),
            "script must attempt to probe repos from repos.conf; stderr={stderr}"
        );
    }

    #[test]
    fn installed_example_script_emits_empty_feed_item_array() {
        // The shipped example must be inert (REPOS empty) so a fresh install
        // does not flood the kanban board with someone else's repos.
        let data_dir = tempfile::tempdir().unwrap();
        let path = install_example_script(data_dir.path()).unwrap();

        let output = std::process::Command::new("bash")
            .arg(&path)
            .output()
            .expect("running the installed example script must not fail");
        assert!(
            output.status.success(),
            "example script exited non-zero: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let parsed: Vec<crate::models::FeedItem> = serde_json::from_slice(&output.stdout)
            .expect("example script must emit a JSON array of FeedItem");
        assert!(parsed.is_empty(), "example script must emit [] by default");
    }

    // -- Plugin metadata --

    #[test]
    fn plugin_json_is_valid() {
        let content = PLUGIN_DIR
            .get_file(".claude-plugin/plugin.json")
            .expect("plugin.json must be embedded")
            .contents_utf8()
            .expect("plugin.json must be UTF-8");
        let value: Value = serde_json::from_str(content).expect("plugin.json is invalid JSON");
        assert_eq!(value["name"], "dispatch");
    }

    #[test]
    fn plugin_embeds_required_files() {
        let required = [
            ".claude-plugin/plugin.json",
            "hooks/hooks.json",
            "hooks/scripts/task-status-hook",
            "hooks/scripts/task-usage-hook",
            "hooks/scripts/pr-learnings-hook",
            "skills/wrap-up/SKILL.md",
            "skills/decompose-review/SKILL.md",
            "skills/decompose-review/references/plan-template.md",
            "skills/learnings/SKILL.md",
            "skills/summarize/SKILL.md",
            "skills/grill/SKILL.md",
            "skills/allium-loop/SKILL.md",
            "skills/allium-loop/prompt.md",
        ];
        for path in required {
            assert!(
                PLUGIN_DIR.get_file(path).is_some(),
                "{path} must be embedded in PLUGIN_DIR"
            );
        }
    }

    #[test]
    fn wrap_up_skill_uses_simplify_not_code_simplifier() {
        let content = PLUGIN_DIR
            .get_file("skills/wrap-up/SKILL.md")
            .expect("wrap-up SKILL.md must be embedded")
            .contents_utf8()
            .expect("wrap-up SKILL.md must be UTF-8");
        assert!(
            !content.contains("code-simplifier"),
            "wrap-up skill must not reference the old 'code-simplifier' skill"
        );
        assert!(
            content.contains("\"simplify\""),
            "wrap-up skill must reference the 'simplify' skill"
        );
    }

    #[test]
    fn summarize_skill_does_not_claim_unconditional_finality() {
        // wrap-up invokes summarize mid-flow (before wrap_up/retro/exit_session).
        // An unconditional "this is always the final step" claim reads as an
        // instruction to stop the whole session right there, which is how
        // wrap-up gets stuck after summarize and never reaches exit_session.
        let content = PLUGIN_DIR
            .get_file("skills/summarize/SKILL.md")
            .expect("summarize SKILL.md must be embedded")
            .contents_utf8()
            .expect("summarize SKILL.md must be UTF-8");
        assert!(
            !content.contains("always the final step"),
            "summarize skill must not unconditionally claim to be the final step, \
             since wrap-up invokes it as a mid-flow sub-step"
        );
    }

    #[test]
    fn retro_skill_tells_agent_to_resume_the_caller() {
        // wrap-up invokes retro between wrap_up and exit_session. Without an
        // explicit instruction to resume the caller's remaining steps, an
        // agent that just finished following retro's own steps has nothing
        // telling it to continue — that's how wrap-up gets stuck after retro
        // and never reaches exit_session.
        let content = PLUGIN_DIR
            .get_file("skills/retro/SKILL.md")
            .expect("retro SKILL.md must be embedded")
            .contents_utf8()
            .expect("retro SKILL.md must be UTF-8");
        assert!(
            content.contains("do not stop here") || content.contains("Do not stop here"),
            "retro skill must explicitly instruct the agent to resume the \
             calling skill's next step instead of stopping"
        );
    }

    #[test]
    fn plugin_hook_scripts_are_executable() {
        let hooks_scripts = PLUGIN_DIR
            .get_dir("hooks/scripts")
            .expect("hooks/scripts dir must exist");
        for file in hooks_scripts.files() {
            assert!(
                is_executable(file.path()),
                "{} should be marked executable",
                file.path().display()
            );
        }
    }

    // -- write_file_if_changed --

    #[test]
    fn write_file_if_changed_creates_new() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.txt");
        let changed = write_file_if_changed(&path, "hello", false).unwrap();
        assert!(changed);
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello");
    }

    #[test]
    fn write_file_if_changed_skips_identical() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("same.txt");
        fs::write(&path, "hello").unwrap();
        let changed = write_file_if_changed(&path, "hello", false).unwrap();
        assert!(!changed);
    }

    #[test]
    fn write_file_if_changed_updates_stale() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stale.txt");
        fs::write(&path, "old").unwrap();
        let changed = write_file_if_changed(&path, "new", false).unwrap();
        assert!(changed);
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
    }

    #[test]
    fn write_file_if_changed_sets_executable_permission() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("script.sh");
        write_file_if_changed(&path, "#!/bin/bash", true).unwrap();
        let metadata = fs::metadata(&path).unwrap();
        let mode = metadata.permissions().mode();
        assert_eq!(mode & 0o755, 0o755, "should have executable permissions");
    }

    // -- Plugin removal --

    #[test]
    fn remove_plugin_deletes_directory() {
        let dir = tempfile::tempdir().unwrap();
        let plugin = dir.path().join("dispatch");
        fs::create_dir_all(plugin.join("hooks/scripts")).unwrap();
        fs::write(plugin.join("hooks/hooks.json"), "{}").unwrap();

        let removed = remove_plugin(&plugin).unwrap();
        assert!(removed);
        assert!(!plugin.exists());
    }

    #[test]
    fn remove_plugin_noop_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let plugin = dir.path().join("dispatch");

        let removed = remove_plugin(&plugin).unwrap();
        assert!(!removed);
    }

    // -- plugin_needs_update --

    #[test]
    fn plugin_needs_update_true_when_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert!(plugin_needs_update_in(dir.path()).unwrap());
    }

    fn write_all_plugin_files(base: &std::path::Path) {
        fn write_dir(dir: &Dir, base: &std::path::Path) {
            for file in dir.files() {
                let path = base.join(file.path());
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(&path, file.contents_utf8().unwrap_or("")).unwrap();
            }
            for subdir in dir.dirs() {
                write_dir(subdir, base);
            }
        }
        write_dir(&PLUGIN_DIR, base);
    }

    #[test]
    fn plugin_needs_update_false_when_all_match() {
        let dir = tempfile::tempdir().unwrap();
        write_all_plugin_files(dir.path());
        assert!(!plugin_needs_update_in(dir.path()).unwrap());
    }

    #[test]
    fn plugin_needs_update_true_when_one_file_differs() {
        let dir = tempfile::tempdir().unwrap();
        write_all_plugin_files(dir.path());
        // Corrupt one file
        fs::write(dir.path().join(".claude-plugin/plugin.json"), "corrupted").unwrap();
        assert!(plugin_needs_update_in(dir.path()).unwrap());
    }

    #[test]
    fn plugin_needs_update_true_when_stale_file_present() {
        let dir = tempfile::tempdir().unwrap();
        write_all_plugin_files(dir.path());
        // Add a file that is no longer in the embedded plugin
        let stale_dir = dir.path().join("skills").join("old-removed-skill");
        fs::create_dir_all(&stale_dir).unwrap();
        fs::write(stale_dir.join("SKILL.md"), "# Old skill").unwrap();
        assert!(
            plugin_needs_update_in(dir.path()).unwrap(),
            "stale on-disk file should trigger update"
        );
    }

    #[test]
    fn install_removes_stale_files() {
        let dir = tempfile::tempdir().unwrap();
        write_all_plugin_files(dir.path());
        // Plant a stale skill that is no longer embedded
        let stale_dir = dir.path().join("skills").join("old-removed-skill");
        fs::create_dir_all(&stale_dir).unwrap();
        let stale_file = stale_dir.join("SKILL.md");
        fs::write(&stale_file, "# Old skill").unwrap();

        let changed = install_plugin_in(dir.path()).unwrap();

        assert!(changed, "removing a stale file must count as a change");
        assert!(
            !stale_file.exists(),
            "stale file must be removed after install"
        );
    }

    #[test]
    fn install_removes_empty_dirs_after_stale_file_pruned() {
        let dir = tempfile::tempdir().unwrap();
        write_all_plugin_files(dir.path());
        let stale_dir = dir.path().join("skills").join("old-removed-skill");
        fs::create_dir_all(&stale_dir).unwrap();
        fs::write(stale_dir.join("SKILL.md"), "# Old skill").unwrap();

        install_plugin_in(dir.path()).unwrap();

        assert!(
            !stale_dir.exists(),
            "empty stale directory must be removed after pruning its files"
        );
    }

    #[test]
    fn install_is_idempotent_after_pruning() {
        let dir = tempfile::tempdir().unwrap();
        write_all_plugin_files(dir.path());
        let stale_dir = dir.path().join("skills").join("old-removed-skill");
        fs::create_dir_all(&stale_dir).unwrap();
        fs::write(stale_dir.join("SKILL.md"), "# Old skill").unwrap();

        install_plugin_in(dir.path()).unwrap();
        let changed = install_plugin_in(dir.path()).unwrap();
        assert!(
            !changed,
            "second install with nothing to change must be idempotent"
        );
    }
}
