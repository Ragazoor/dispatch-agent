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
    claude_dir.join(crate::claude_paths::plugin_dir_rel!())
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
        let content = file
            .contents_utf8()
            .with_context(|| format!("Non-UTF-8 plugin file: {}", file.path().display()))?;
        *changed |= super::write_file_if_changed(&path, content, is_executable(file.path()))?;
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
        // Same predicate the installer writes by, so "needs an update" and "was
        // actually written" can never disagree.
        if !super::file_is_up_to_date(&path, content) {
            return Ok(true);
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
            "hooks/scripts/pr-learnings-hook",
            "skills/wrap-up/SKILL.md",
            "skills/retro/SKILL.md",
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
        let content = skill_body("wrap-up");
        assert!(
            !content.contains("code-simplifier"),
            "wrap-up skill must not reference the old 'code-simplifier' skill"
        );
        assert!(
            content.contains("\"simplify\""),
            "wrap-up skill must reference the 'simplify' skill"
        );
    }

    /// The embedded copy of the wrap-up skill is what agents actually read, so
    /// it is the only thing that catches a regression here: epic chaining is a
    /// server-side effect of `exit_session` and there is no `dispatch_next` tool
    /// left for the skill to name.
    #[test]
    fn wrap_up_skill_does_not_instruct_calling_dispatch_next() {
        let content = skill_body("wrap-up");
        assert!(
            !content.contains("dispatch_next"),
            "wrap-up skill must not tell the agent to call dispatch_next — \
             exit_session chains the next epic subtask automatically"
        );
    }

    /// A regression in this guidance is silent: an agent that reads a non-error
    /// response as a completed close leaves a live tmux window and a task stuck
    /// in its old status, with no error anywhere to notice.
    #[test]
    fn wrap_up_skill_warns_a_successful_exit_session_can_report_a_failed_close() {
        assert!(
            failed_close_guidance().contains("success"),
            "failed-close guidance must say the response is a *successful* one, \
             not an error — that is the whole trap"
        );
    }

    /// The one reaction that must survive any rewording of the failed-close
    /// guidance: the exit token is consumed before the terminal write is
    /// attempted, so neither retrying `exit_session` nor taking a fresh token
    /// from `wrap_up` can work. Asserted positively — a negative "must not
    /// contain 'retry'" would also pass if the guidance were deleted outright,
    /// which is the regression that matters.
    #[test]
    fn wrap_up_skill_tells_the_agent_not_to_retry_a_failed_close() {
        let section = failed_close_guidance();
        assert!(
            section.contains("not retry") || section.contains("n't retry"),
            "failed-close guidance must tell the agent not to retry exit_session"
        );
        assert!(
            section.contains("wrap_up")
                && (section.contains("again") || section.contains("fresh token")),
            "failed-close guidance must tell the agent not to call wrap_up again \
             for a fresh token"
        );
    }

    /// Regression guard for the stale instruction that survived unnoticed until
    /// task #3769: the skill told the agent to record the PR URL via
    /// `update_task`, when the URL actually travels with `exit_session`. It
    /// drifted precisely because nothing pinned it. Scoped to lines that pair
    /// `update_task` with a URL, so an unrelated future use of the tool is not
    /// blocked.
    #[test]
    fn wrap_up_skill_does_not_record_the_pr_url_via_update_task() {
        for line in skill_body("wrap-up").lines() {
            let lower = line.to_lowercase();
            assert!(
                !(lower.contains("update_task") && lower.contains("url")),
                "wrap-up skill must not tell the agent to record the PR URL via \
                 update_task — the URL travels with exit_session: {line}"
            );
        }
    }

    /// The "Draft the title and body" step of the PR path, isolated from its
    /// neighbouring steps. Not built on `section_after`: that helper ends a
    /// section at the next line starting with `#`, but this step's own
    /// Markdown example contains fenced `## Summary` / `## Test plan` headings
    /// as literal example text, which would truncate the section before the
    /// part these tests need to inspect.
    fn pr_body_draft_section() -> String {
        let content = skill_body("wrap-up");
        let (_, after) = content
            .split_once("### Draft the title and body")
            .expect("wrap-up skill must have a 'Draft the title and body' step");
        let (section, _) = after.split_once("### Push and create the draft PR").expect(
            "wrap-up skill must have a 'Push and create the draft PR' step \
                 after drafting",
        );
        section.to_string()
    }

    /// Distilled from the shared PR-description knowledge base (learning #36):
    /// PR bodies must describe the user-visible change and why, not the
    /// implementation — so the template must never invite a function, class,
    /// file, or variable name.
    #[test]
    fn wrap_up_skill_pr_body_uses_categorized_summary_like_coderabbit() {
        let section = pr_body_draft_section();
        for label in ["**Breaking Changes**", "**New Features**", "**Bug Fixes**"] {
            assert!(
                section.contains(label),
                "PR body template must group changes under bold category labels \
                 (CodeRabbit-style), missing: {label}"
            );
        }
        assert!(
            section.to_lowercase().contains("coderabbit"),
            "PR body template should name CodeRabbit as the format it mirrors, \
             so a future editor knows why the categories look like this"
        );
    }

    /// Distilled from the shared PR-description knowledge base (learnings
    /// #146 and #154): dispatch task IDs must never appear in a PR body —
    /// GitHub auto-links `#N` to an unrelated issue/PR in the target repo.
    /// The old template's `Implements #{task_id}.` line was a live
    /// contradiction of both learnings until this test locked it out.
    #[test]
    fn wrap_up_skill_pr_body_omits_task_references() {
        let section = pr_body_draft_section();
        assert!(
            !section.contains("Implements #{task_id}"),
            "PR body template must not instruct the agent to write \
             \"Implements #{{task_id}}\" into the body — GitHub auto-links #N \
             to an unrelated issue/PR in this repo"
        );
        assert!(
            section
                .to_lowercase()
                .contains("do not reference the dispatch task"),
            "PR body template must explicitly warn against referencing the \
             dispatch task (task IDs, \"Implements #N\")"
        );
    }

    /// Distilled from the shared PR-description knowledge base (learning
    /// #188): the Test plan section is opt-in for dangerous/breaking PRs,
    /// not a default part of every PR body.
    #[test]
    fn wrap_up_skill_pr_body_test_plan_is_opt_in_not_default() {
        let section = pr_body_draft_section();
        assert!(
            !section.contains("- [ ] {how to verify"),
            "PR body template must not show an unconditional Test plan checklist \
             in its default example"
        );
        let lower = section.to_lowercase();
        assert!(
            lower.contains("test plan")
                && (lower.contains("dangerous") || lower.contains("breaking")),
            "PR body template must say the Test plan section is only for \
             dangerous or breaking PRs, omitted by default"
        );
    }

    /// The wrap-up skill's failed-close guidance block, lowercased: the heading
    /// section telling the agent that a *successful* `exit_session` response can
    /// still report that the close did not take effect.
    ///
    /// Scoped to that one section deliberately — "do not retry" also appears in
    /// the neighbouring `exit_session` *errors* section, so a whole-document
    /// check would still pass with this section's retry guidance deleted. The
    /// section is anchored on the phrase below and ends at the next heading of
    /// any depth (so promoting or demoting the heading cannot silently widen it
    /// to the rest of the file); if you reword the heading, re-anchor it here.
    fn failed_close_guidance() -> String {
        let content = skill_body("wrap-up").to_lowercase();
        section_after(&content, "did not take effect").expect(
            "wrap-up skill must document that a successful exit_session response \
             can still report the close did not take effect",
        )
    }

    /// The slice of `content` that follows `anchor`, ending at the next Markdown
    /// heading of any depth — so promoting or demoting a heading cannot silently
    /// widen a scoped assertion to the rest of the document. `None` if `anchor`
    /// is absent, letting each caller phrase its own "this copy is gone" panic.
    fn section_after(content: &str, anchor: &str) -> Option<String> {
        let (_, section) = content.split_once(anchor)?;
        Some(
            section
                .split_once("\n#")
                .map_or(section, |(block, _)| block)
                .to_string(),
        )
    }

    /// A loop iteration does rebase, tend, propagate, red check, implement,
    /// verify and weed, and repeats up to the loop's configured maximum — so an
    /// iteration agent left on the session model (Opus) multiplies that cost by
    /// the iteration count. Nothing in the loop's own output reveals which model
    /// ran, so dropping this instruction would regress silently.
    #[test]
    fn allium_loop_dispatches_iteration_agents_on_sonnet() {
        let section = allium_loop_dispatch_instruction();
        assert!(
            section.contains("sonnet"),
            "allium-loop's dispatch step must name the sonnet model for \
             iteration agents"
        );
    }

    /// The no-fork rule and the model override are load-bearing together: a
    /// `fork` ignores `model` entirely and runs on the session model, so losing
    /// the no-fork constraint would silently undo the sonnet pin even with the
    /// override still written down.
    #[test]
    fn allium_loop_dispatch_still_forbids_fork() {
        let section = allium_loop_dispatch_instruction();
        assert!(
            section.contains("fork"),
            "allium-loop's dispatch step must keep forbidding `fork` — a fork \
             ignores the model override and runs on the session model"
        );
    }

    /// The allium-loop skill's per-iteration dispatch instruction: the "Each
    /// Iteration" section.
    ///
    /// Scoped to that one section deliberately — the kickoff section also names
    /// the model (it resolves and records the loop parameter), so a
    /// whole-document check would still pass with the dispatch step's override
    /// deleted. Re-anchor here if the heading is reworded.
    fn allium_loop_dispatch_instruction() -> String {
        section_after(skill_body("allium-loop"), "### Each Iteration")
            .expect("allium-loop skill must have an 'Each Iteration' section")
    }

    /// Both halves of the loop's convergence gate, added after two iterations of
    /// one run reported `CONVERGED: yes` while their own prose named an
    /// unresolved item. The driver acts on the label, not the prose, so a
    /// self-contradicting report ends the loop with work still outstanding —
    /// and nothing in the loop's output reveals that, which is what makes
    /// losing this copy a silent regression rather than a visible one.
    #[test]
    fn allium_loop_convergence_gate_rejects_deferred_and_uncovered_work() {
        let section = section_after(
            skill_file("allium-loop", "prompt.md"),
            "Emit `CONVERGED: yes` ONLY when ALL hold:",
        )
        .expect("allium-loop prompt must state its CONVERGED criteria");
        assert!(
            section.contains("pending a decision"),
            "the convergence gate must keep disqualifying work left pending a \
             later run — a flagged-but-unresolved item is divergence"
        );
        assert!(
            section.contains("name the surviving test"),
            "the convergence gate must keep requiring a deleted test's \
             replacement to be named, not asserted in the abstract"
        );
    }

    /// Read an embedded skill's `SKILL.md` by skill name, for tests that assert
    /// on skill copy.
    fn skill_body(skill: &str) -> &'static str {
        skill_file(skill, "SKILL.md")
    }

    /// Read any embedded file from a skill directory. `SKILL.md` is the common
    /// case ([`skill_body`]); allium-loop also ships the `prompt.md` that its
    /// per-iteration agents actually run from, and that copy needs the same
    /// deletion-is-a-regression protection.
    fn skill_file(skill: &str, file: &str) -> &'static str {
        let path = format!("skills/{skill}/{file}");
        PLUGIN_DIR
            .get_file(&path)
            .unwrap_or_else(|| panic!("{path} must be embedded"))
            .contents_utf8()
            .unwrap_or_else(|| panic!("{path} must be UTF-8"))
    }

    /// A lowercased section of the retro skill body, via [`section_after`]:
    /// from the first occurrence of `anchor` up to the next Markdown heading of
    /// any depth. Anchors passed here must therefore be lowercase.
    ///
    /// Scoped per-section deliberately. Retro repeats words like "task",
    /// "spec" and "fix" across its steps, so a whole-document `contains` can
    /// still pass after the instruction under test has been deleted. If you
    /// reword an anchor heading, re-anchor it here.
    fn retro_section(anchor: &str) -> String {
        let content = skill_body("retro").to_lowercase();
        section_after(&content, anchor).unwrap_or_else(|| {
            panic!("retro skill must contain the section anchored on {anchor:?}")
        })
    }

    #[test]
    fn retro_admission_test_is_next_agent_benefit_not_doc_accuracy() {
        // The old Step 2 asked whether CLAUDE.md or a spec was "stale or
        // wrong" — a correctness question every trivial nit passes, which is
        // how retro came to file 38 one-line doc chores. The bar is now
        // whether the *next* agent would do better, and each finding must
        // trace to a concrete moment this session actually lost time on.
        let section = retro_section("## step 2:");
        assert!(
            section.contains("would the next agent do better"),
            "retro's admission test must be whether the next agent benefits, \
             not whether a sentence is inaccurate"
        );
        assert!(
            section.contains("concrete moment"),
            "retro must require every finding to trace to a concrete moment \
             from Step 1 rather than to a hypothetical"
        );
        assert!(
            !section.contains("stale or wrong"),
            "retro must not frame its check as a documentation-accuracy audit"
        );
    }

    #[test]
    fn retro_first_step_asks_whether_user_corrected_or_steered_you() {
        // Task #4326: the old Step 1 was entirely "context turned out wrong"
        // shaped (a bad CLAUDE.md assumption, a convention found by reading
        // source, a stale spec, a guessed rule, a failing command) — none of
        // it about the user. #4316's code-review feedback (test suffix,
        // one-class-per-file, ADTs over `require()`, and more) went
        // uncaptured until the human explicitly asked whether it had been
        // saved as learnings. Step 1 now asks specifically whether the user
        // corrected or steered the agent, and must still say "nothing
        // notable" is a real answer. (The Step-1-to-Step-2 linkage is
        // asserted separately, by the "concrete moment" check in
        // `retro_admission_test_is_next_agent_benefit_not_doc_accuracy`.)
        let section = retro_section("## step 1:");
        assert!(
            section.contains("correct") && section.contains("steer"),
            "retro's first step must ask whether the user corrected or \
             steered the agent this session"
        );
        assert!(
            section.contains("nothing notable"),
            "retro must state that an empty reflection is a real answer, so a \
             smooth session is not pressured into inventing findings"
        );
    }

    #[test]
    fn retro_first_step_scopes_out_self_discovered_gaps() {
        // A stale spec is `weed`'s job (spec-code alignment), and a command
        // that failed until the right invocation was found is a tooling
        // problem — neither is feedback from the user, so Step 1 must not
        // invite them back in as findings.
        let section = retro_section("## step 1:");
        assert!(
            section.contains("weed"),
            "retro's first step must route spec-code alignment to the weed \
             skill rather than treating a stale spec as its own finding"
        );
        assert!(
            section.contains("tooling problem"),
            "retro's first step must name a failing command as a tooling \
             problem, not a correction from the user"
        );
    }

    #[test]
    fn retro_first_step_names_code_review_and_design_examples() {
        // Step 1's checklist must give the agent concrete shapes of user
        // feedback to look for, and must say this counts even when it cost
        // no time in the moment — design/style feedback rarely does.
        let section = retro_section("## step 1:");
        assert!(
            section.contains("code review"),
            "retro's first step must prompt for feedback given during a \
             code review pass"
        );
        assert!(
            section.contains("design choice") || section.contains("design decision"),
            "retro's first step must ask about a design choice the user \
             made or corrected this session"
        );
        assert!(
            section.contains("even if it didn't") || section.contains("even if it did not"),
            "retro must state this category counts even when it didn't cost \
             time in the moment, since design/style feedback often doesn't \
             slow the agent down"
        );
    }

    #[test]
    fn retro_relationship_section_says_not_to_wait_to_be_asked_for_feedback() {
        // #4316's design/style feedback only became learnings after the
        // human asked "did you save learnings about the feedback I gave
        // you" — retro must not rely on being asked.
        let section = retro_section("## relationship to other skills");
        assert!(
            section.contains("don't wait") || section.contains("do not wait"),
            "retro's relationship-to-learnings note must say to record \
             design/style feedback without waiting to be asked"
        );
    }

    #[test]
    fn retro_skill_permits_fixing_small_context_drift_in_session() {
        // The old Step 3 said "Do not edit files yourself", which turned every
        // one-line doc correction into a task + worktree + agent dispatch. The
        // agent that just did the work has the context and is already in a
        // worktree whose next step is a commit; it should make the fix.
        let content = skill_body("retro").to_lowercase();
        assert!(
            !content.contains("do not edit files yourself"),
            "retro must no longer ban editing outright — fixing small context \
             drift in place is now its job"
        );
        let section = retro_section("## step 3:");
        assert!(
            section.contains("fix it yourself"),
            "retro must tell the agent to fix small context drift in this session"
        );
        assert!(
            section.contains("small and self-evident"),
            "retro's edit licence must be bounded to small, self-evident \
             corrections that need no design judgement"
        );
    }

    #[test]
    fn retro_skill_forbids_speccing_unimplemented_behaviour() {
        // A spec edit describing behaviour the session already implemented is
        // documentation catching up. One describing behaviour the code lacks is
        // a design change, and this repo runs those spec -> tests -> code with
        // their own dispatch — so retro must file it, not write it.
        let section = retro_section("## step 3:");
        assert!(
            section.contains("already implemented"),
            "retro may only edit a spec to describe behaviour this session \
             already implemented"
        );
        assert!(
            section.contains("spec → tests → code"),
            "retro must route a spec change for not-yet-implemented behaviour \
             to a task, naming the spec -> tests -> code loop as the reason"
        );
    }

    #[test]
    fn retro_skill_does_not_file_feature_tasks() {
        // Every archived retro-created task was a speculative refactor dressed
        // as an enhancement — "this invariant is enforced by convention, so the
        // same omission could recur", "this could be a single atomic insert".
        // feature leaves retro's vocabulary entirely.
        let section = retro_section("### what you may file");
        assert!(
            section.contains("never file a `feature`"),
            "retro must explicitly refuse to file feature tasks"
        );
        assert!(
            section.contains("speculative refactor"),
            "retro must name speculative refactors as a non-finding, since that \
             is the shape of every retro task that got archived"
        );
        // Scoped to the whole skill body, not just this section: the string
        // this guards against previously lived in Step 3's tag list, a
        // section this assertion does not otherwise cover. A future agent
        // restoring "`feature` for an enhancement idea" to Step 3 is the
        // likeliest regression, and no other section legitimately contains
        // this string, so widening the scope here is safe.
        let whole_skill = skill_body("retro").to_lowercase();
        assert!(
            !whole_skill.contains("`feature` for"),
            "retro must not still describe when to use the feature tag, in any section"
        );
    }

    #[test]
    fn retro_skill_requires_a_duplicate_check_before_filing() {
        // Two findings were each filed twice: one stale sentence that appeared
        // in two documents, and one recurring shape nobody recognised. Nothing
        // in the skill told the agent to look first.
        let section = retro_section("### before you file");
        assert!(
            section.contains("list_tasks"),
            "retro must check for an existing task with list_tasks before filing"
        );
        assert!(
            section.contains("one task per finding"),
            "retro must collapse a finding that spans several files into one task"
        );
    }

    #[test]
    fn retro_skill_states_zero_findings_is_the_normal_outcome() {
        // The old skill buried this under three steps of checklist-shaped
        // instructions, which read as a quota to fill rather than a bar to
        // clear.
        let section = retro_section("### before you file");
        assert!(
            section.contains("zero tasks is the normal outcome"),
            "retro must state outright that filing nothing is the expected result"
        );
    }

    #[test]
    fn retro_step_2_asks_whether_root_cause_is_outside_this_repo() {
        // Task #4256: retro found the Bash sandbox blocks Gradle daemon
        // startup, wrote a workaround note to user-scala's CLAUDE.md, and
        // stopped — the sandbox defect itself was never filed. Nothing in
        // Step 2 asked where the root cause actually lived, so this passed
        // retro's own rubric as closed. Step 2 must now ask that question for
        // every finding, in addition to the next-agent-benefit test.
        let section = retro_section("## step 2:");
        assert!(
            section.contains("root cause"),
            "retro's Step 2 must ask whether a finding's root cause lies \
             outside this repo"
        );
        assert!(
            section.contains("sandbox") && section.contains("claude code"),
            "retro's root-cause question must name the sandbox, dispatch \
             itself, and claude code as tool/environment loci, not just \
             gesture at 'somewhere else'"
        );
    }

    #[test]
    fn retro_root_cause_subsection_requires_filing_not_just_a_workaround() {
        // A local doc workaround treats the symptom, not the defect. Retro
        // must not let it read as full closure when the root cause is the
        // tool/environment running the agent rather than this repo — and
        // must route the filing itself based on who owns that root cause:
        // this task's own repo (file directly) or a different one (flag to
        // the user rather than filing unprompted onto a foreign board).
        let section = retro_section("### when the root cause is the tool or environment");
        let flattened = section.replace('\n', " ");
        assert!(
            section.contains("workaround"),
            "the new subsection must address the local workaround explicitly"
        );
        assert!(
            flattened.contains("is not") && flattened.contains("sufficient closure"),
            "the new subsection must state that a workaround alone is not \
             sufficient closure for a tool/environment root cause"
        );
        assert!(
            section.contains("create_task"),
            "retro must file a same-repo root-cause defect with create_task"
        );
        assert!(
            section.contains("not file") || section.contains("don't file silently"),
            "retro must not file a cross-repo root-cause defect silently"
        );
        assert!(
            section.contains("flag"),
            "retro must flag a cross-repo root-cause defect explicitly instead"
        );
    }

    #[test]
    fn retro_output_template_has_a_root_cause_line() {
        // The #4256 gap wasn't just a missing filing rule — retro's own
        // output never said anything, so nothing forced the question in
        // front of the user before the session closed.
        //
        // Not scoped via retro_section("## step 4:"): the output template is
        // a fenced code block containing its own "## Session Retrospective"
        // heading, which retro_section's "next heading of any depth" cutoff
        // would treat as the section boundary and truncate before this line.
        let content = skill_body("retro").to_lowercase();
        assert!(
            content.contains("root-cause issues flagged"),
            "retro's output template must include a line surfacing \
             root-cause issues flagged elsewhere"
        );
    }

    #[test]
    fn decompose_review_skill_defaults_wrap_up_mode_to_rebase() {
        // Review work packages are small and land on main — one draft PR per
        // package is noise. The skill pre-sets wrap_up_mode purely to skip
        // wrap-up's AskUserQuestion step, and 'rebase' is the right value.
        let content = skill_body("decompose-review");
        assert!(
            content.contains("`wrap_up_mode`: `\"rebase\"`"),
            "decompose-review skill must set wrap_up_mode to \"rebase\""
        );
        assert!(
            !content.contains("`wrap_up_mode`: `\"pr\"`"),
            "decompose-review skill must not set wrap_up_mode to \"pr\" — \
             a decomposed review epic would open one draft PR per work package"
        );
    }

    #[test]
    fn decompose_review_skill_does_not_pass_repo_path_to_create_epic() {
        // Epics carry no repo_path — create_epic rejects the field outright
        // ("unknown field `repo_path`"). Step 6's subtasks are where the repo
        // path belongs, so this assertion is scoped to the Step 5 section:
        // a whole-document check would trip over Step 6's legitimate uses.
        let content = skill_body("decompose-review");
        let step5 = content
            .split("## Step 5: Create Epic")
            .nth(1)
            .expect("decompose-review skill must have a 'Step 5: Create Epic' section")
            .split("## Step 6")
            .next()
            .expect("split always yields at least one element");
        assert!(
            !step5.contains("`repo_path`:"),
            "decompose-review must not tell the agent to pass repo_path to \
             create_epic — the tool rejects it (found in Step 5: {step5:?})"
        );
    }

    /// Every sub-skill wrap-up invokes is a place the agent can mistake the
    /// sub-skill's end for the session's end and go idle — retro and simplify
    /// both needed explicit guards for exactly that. Summarize sat in the
    /// closing sequence, between `wrap_up` and `exit_session`, which is the
    /// worst possible place to stall: the rebase has already fast-forwarded
    /// base_branch and the task is stuck in its old status. It bought a recap
    /// the user rarely needed, so it is gone rather than guarded (task #4505).
    #[test]
    fn wrap_up_skill_does_not_invoke_summarize() {
        let content = skill_body("wrap-up");
        assert!(
            !content.contains(r#"skill: "summarize""#),
            "wrap-up must not invoke the summarize skill — a sub-skill call \
             between wrap_up and exit_session is a stall point on the one path \
             where stalling has already touched base_branch"
        );
    }

    /// A preset `wrap_up_mode` is the user's answer, given earlier. Suppressing
    /// the question without carrying the wrap-up through leaves the agent idle
    /// with the question it was told not to ask unanswered, and nothing reaches
    /// base_branch until a human notices (task #4505, observed twice).
    #[test]
    fn wrap_up_skill_carries_a_preset_mode_through_without_asking_or_stopping() {
        let content = skill_body("wrap-up");
        let step_2 = content
            .split("## Step 2")
            .nth(1)
            .and_then(|s| s.split("## Step 3").next())
            .expect("wrap-up skill must have a Step 2 that reads the task");
        assert!(
            step_2.contains("Wrap-up mode:"),
            "Step 2 must read the preset mode off the line get_task actually \
             prints (`Wrap-up mode: <mode>`) — the response is prose, not JSON, \
             so an agent told to look for a `wrap_up_mode` field can find \
             nothing and fall through to the question it was meant to skip"
        );
        assert!(
            step_2.contains("Step 4"),
            "Step 2 must say what a preset mode does to the question step \
             (Step 4), or the agent has to infer it"
        );
        let lowered = step_2.to_lowercase();
        assert!(
            lowered.contains("do not stop") || lowered.contains("don't stop"),
            "a preset mode must come with an explicit instruction not to stop \
             part-way — suppressing the question alone is what left agents idle \
             at a blank prompt"
        );
    }

    #[test]
    fn summarize_skill_does_not_claim_unconditional_finality() {
        // summarize is usually run standalone (/summarize), but any skill may
        // invoke it mid-flow as a sub-step. An unconditional "this is always
        // the final step" claim reads as an instruction to stop the whole
        // session right there, stranding whatever the caller had left to do.
        // wrap-up used to be that caller and stalled exactly this way, which
        // is why its call is gone — see wrap_up_skill_does_not_invoke_summarize.
        let content = skill_body("summarize");
        assert!(
            !content.contains("always the final step"),
            "summarize skill must not unconditionally claim to be the final step, \
             since wrap-up invokes it as a mid-flow sub-step"
        );
    }

    #[test]
    fn retro_skill_tells_agent_to_resume_the_caller() {
        // wrap-up invokes retro pre-commit, before its commit step. Without an
        // explicit instruction to resume the caller's remaining steps, an
        // agent that just finished following retro's own steps has nothing
        // telling it to continue — that's how wrap-up gets stuck after retro
        // and never reaches the commit, the user's action choice, or
        // exit_session. Retro's own edits are among what that commit carries,
        // so stopping here loses them.
        let content = skill_body("retro");
        assert!(
            content.contains("do not stop here") || content.contains("Do not stop here"),
            "retro skill must explicitly instruct the agent to resume the \
             calling skill's next step instead of stopping"
        );
    }

    #[test]
    fn wrap_up_skill_runs_retro_between_the_action_choice_and_the_commit() {
        // Retro is bracketed on both sides, and each bound fixes a real defect:
        //
        //  • After the action choice, because retro decides fix-vs-file from the
        //    action. On `done` there is no rebase and no push, so a fix would be
        //    stranded — and retro cannot know that while the action is unsettled.
        //  • Before the commit, because that commit is what carries the fixes it
        //    does make. Invoked after wrap_up instead, they are stranded anyway:
        //    the rebase path has already fast-forwarded base_branch, so a later
        //    commit sits on a branch nobody merges, and the PR path has pushed.
        let content = skill_body("wrap-up");
        let choice_at = content
            .find("## Step 4: Ask the user to choose")
            .expect("wrap-up skill must have an action-choice step to anchor retro after");
        let retro_at = content
            .find("Skill({ skill: \"retro\" })")
            .expect("wrap-up skill must invoke the retro skill");
        let commit_at = content
            .find("## Step 6: Commit uncommitted changes")
            .expect("wrap-up skill must have a commit step to anchor retro before");
        assert!(
            choice_at < retro_at,
            "wrap-up must settle the action before invoking retro, so retro can \
             tell whether a fix it makes can reach the base branch at all"
        );
        assert!(
            retro_at < commit_at,
            "wrap-up must invoke retro before its commit step, so retro's \
             context fixes are committed with the session's work"
        );
        assert_eq!(
            content.matches("Skill({ skill: \"retro\" })").count(),
            1,
            "wrap-up must invoke retro exactly once — a leftover call in the \
             closing sequence would run it twice and re-file its findings"
        );
        assert!(
            !content.to_lowercase().contains("run `/retro`"),
            "wrap-up must not still invoke retro from the closing sequence \
             between wrap_up and exit_session"
        );
    }

    /// The "Do NOT record" list must name the internal-code-citation failure
    /// mode explicitly (task #4152 — learning #401 carried a stale
    // allow-phantom-symbol: the actual stale citation learning #401 carried
    /// `src/feed/cycle.rs::run_feed_cycle` citation that no gate ever caught).
    /// Scoped to that one section: the rest of the skill mentions plenty of
    /// backticked identifiers (tool names) that would make a whole-document
    /// check pass even with this specific rule missing.
    #[test]
    fn learnings_skill_forbids_code_citations() {
        let section = section_after(skill_body("learnings"), "### Do NOT record:")
            .expect("learnings skill must have a 'Do NOT record' section");
        assert!(
            section.contains("path.rs::symbol") || section.contains("path.rs"),
            "the Do NOT record list must name the path.rs::symbol citation shape: {section}"
        );
        assert!(
            section.to_lowercase().contains("rot"),
            "the rule must explain WHY (silent rot, no re-check) not just state a ban: {section}"
        );
    }

    /// The two skills that both tell an agent to rate a learning must not
    /// disagree about *when*. `learnings` says at the moment you act on the
    /// entry, explicitly "not deferred to wrap-up"; wrap-up's own copy used to
    /// permit "at the latest before you wrap up", which is that deferral. An
    /// agent reads whichever it loaded, so the looser wording wins whenever
    /// wrap-up is the one in context — and rating a batch at the end is the
    /// behaviour the immediate rule exists to prevent.
    #[test]
    fn wrap_up_and_learnings_agree_that_rating_is_not_deferred() {
        let learnings = skill_body("learnings");
        assert!(
            learnings.contains("not deferred to wrap-up"),
            "the learnings skill must keep the rate-immediately rule this test pins wrap-up to"
        );
        let wrap_up = skill_body("wrap-up");
        assert!(
            !wrap_up.contains("at the latest before you wrap up"),
            "wrap-up must not permit the deferral the learnings skill forbids"
        );
    }

    /// The validator only catches shapes prose never produces — a bare
    /// PascalCase or short snake_case name walks straight through it (task
    /// #4402: no regex separates `TuiRuntime` from `GitHub`). The skill copy
    /// is therefore the only thing keeping those out, so it has to state the
    /// rule in full rather than deferring to what `record_learning` rejects.
    #[test]
    fn learnings_skill_forbids_naming_implementation_detail_a_validator_misses() {
        let section = section_after(skill_body("learnings"), "### Do NOT record:")
            .expect("learnings skill must have a 'Do NOT record' section");
        for term in ["function", "type", "macro", "fixture", "file"] {
            assert!(
                section.contains(term),
                "the Do NOT record list must name '{term}' among the things an entry \
                 may not name — the validator does not catch all of them: {section}"
            );
        }
        assert!(
            section.contains("rejects only"),
            "the list must tell the agent the validator is narrower than the rule, \
             or an agent will read a successful call as approval: {section}"
        );
    }

    /// Ported from the `kognic-knowledge` plugin's capture triage (task
    /// #4402). Without it, "Do NOT record" is a list of categories an agent
    /// has to pattern-match against; with it there is one question that
    /// decides, and it routes a whole class of would-be entries to a lint
    /// instead of to prose.
    #[test]
    fn learnings_skill_carries_the_machine_check_triage() {
        let content = skill_body("learnings").to_lowercase();
        assert!(
            content.contains("failing check"),
            "the skill must ask whether a failing check could be written from the \
             source alone: {content}"
        );
        assert!(
            content.contains("lint"),
            "the triage's yes-branch must route to a lint rule rather than prose: {content}"
        );
        assert!(
            content.contains("smell"),
            "the triage's third branch — wrongly-shaped code is a smell, and the fix \
             is a refactor not a sentence — must survive: {content}"
        );
    }

    /// `record_learning` enforces only that a procedural entry HAS a detail.
    /// What the detail must contain — the case where the agent stops and asks
    /// a human — is convention, and this copy is where it lives (task #4402,
    /// ported from OKF's required `# Escalate` section).
    #[test]
    fn learnings_skill_requires_a_boundary_on_procedural_entries() {
        let content = skill_body("learnings").to_lowercase();
        assert!(
            content.contains("procedural") && content.contains("ask a human"),
            "the skill must say a procedural entry's detail names when to stop and \
             ask a human: {content}"
        );
    }

    /// The summary-writing guidance used to illustrate "name the specific
    /// thing" with an example naming a Rust type — the exact habit task #4402
    /// exists to break. An example teaches harder than a rule, so a stale one
    /// here would quietly undo the section above it.
    #[test]
    fn learnings_skill_summary_guidance_names_no_symbol() {
        let section = section_after(skill_body("learnings"), "### Writing a good summary")
            .expect("learnings skill must have a summary-writing section");
        assert!(
            !section.contains("TaskPatch"),
            "the summary example must not name a type — it models the banned habit: {section}"
        );
    }

    /// The scope table must not offer `project` — LearningScope has only
    /// user/repo/epic/task (task #4152: the row was stale, and an agent
    /// passing scope="project" gets a deserialization error).
    #[test]
    fn learnings_skill_scope_table_has_no_project_row() {
        let content = skill_body("learnings");
        assert!(
            !content.contains("| `project` |"),
            "the scope table must not offer a project scope row: {content}"
        );
    }

    /// The `wrong` verdict bullet must not claim a human-review step exists
    /// (learnings.allium: no human gate, no needs_review state, no status
    /// change on either verdict).
    #[test]
    fn learnings_skill_wrong_verdict_does_not_claim_human_review() {
        // The correct text says "there is no human review step", which itself
        // contains the substring "human review" — so this checks for the old
        // false CLAIM (routing an entry TO review) rather than the substring.
        let content = skill_body("learnings").to_lowercase();
        assert!(
            !content.contains("routes an approved entry"),
            "the learnings skill must not claim a wrong verdict routes an entry \
             for human review — learnings.allium: no human gate, no status \
             change on either verdict: {content}"
        );
        assert!(
            content.contains("no human review"),
            "the learnings skill should state plainly that there is no human \
             review step: {content}"
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

    /// Task #4193: the verify command must reach the agent, and be acted on,
    /// before `wrap_up` is ever called — not just via the response's
    /// after-the-fact "Verify before exiting" line, which for the rebase path
    /// arrives after the base branch has already been fast-forwarded.
    #[test]
    fn wrap_up_skill_runs_verification_before_calling_wrap_up() {
        let content = skill_body("wrap-up");
        let commit_at = content
            .find("## Step 6: Commit uncommitted changes")
            .expect("wrap-up skill must have a commit step to anchor verification after");
        let verify_at = content
            .find("## Step 7: Run verification")
            .expect("wrap-up skill must have a verification step between commit and closing");
        let closing_at = content
            .find("## Step 8: The closing sequence")
            .expect("wrap-up skill's closing sequence must be Step 8, after verification");
        assert!(
            commit_at < verify_at,
            "verification step must come after the commit step"
        );
        assert!(
            verify_at < closing_at,
            "verification step must come before the closing sequence (and so before wrap_up \
             is ever called)"
        );
        assert!(
            !content.contains("## Step 7: The closing sequence"),
            "the closing sequence must be renumbered to Step 8, not left as Step 7"
        );

        let verify_section = section_after(content, "## Step 7: Run verification")
            .expect("wrap-up skill must have a verification step to anchor the section on");
        assert!(
            verify_section.contains("Verify command") && verify_section.contains("Step 2"),
            "verification step must tell the agent to read the Verify command shown by \
             get_task in Step 2, got: {verify_section}"
        );
        assert!(
            verify_section.to_lowercase().contains("before")
                && verify_section.contains("`wrap_up`"),
            "verification step must instruct running verification before calling wrap_up, \
             got: {verify_section}"
        );
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
