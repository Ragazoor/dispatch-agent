use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use std::time::Duration;

use crate::models::{expand_tilde, slugify, Task};
use crate::process::ProcessRunner;
use crate::tmux;

use super::prompts::build_tmux_window_name;
use super::stderr_str;

/// Directory inside a repo where dispatch stores artefacts (e.g. `rag.db` for semantic search).
/// Created on demand when actively used, not for every dispatched worktree.
/// Added to `.gitignore` when first created so agents cannot accidentally stage it.
pub(crate) const DISPATCH_DIR: &str = ".dispatch";
const GITIGNORE_FILE: &str = ".gitignore";
const DISPATCH_GITIGNORE_LINE: &str = ".dispatch/";

/// Bounded retry budget for `git fetch origin <base>` during worktree
/// provisioning. Smooths over transient failures (e.g. ref-lock contention
/// when two dispatches fetch the same repo concurrently) without needing to
/// pattern-match git's stderr text.
const FETCH_MAX_ATTEMPTS: u32 = 3;
// Zero delay under `cfg(test)` so the retry tests below don't spend real
// wall-clock time sleeping — flagged by adversarial review of this plan,
// which pointed out a fixed 500ms delay would cost ~1s of real sleep per
// retry test. The retry *count* is still fully exercised either way.
#[cfg(not(test))]
const FETCH_RETRY_DELAY: Duration = Duration::from_millis(500);
#[cfg(test)]
const FETCH_RETRY_DELAY: Duration = Duration::from_millis(0);

/// Ensure `<worktree>/.dispatch/` exists and that `<worktree>/.gitignore`
/// contains an entry for it. Idempotent: safe to call repeatedly.
pub(crate) fn ensure_dispatch_dir_and_gitignore(worktree: &Path) -> Result<()> {
    let dispatch_dir = worktree.join(DISPATCH_DIR);
    fs::create_dir_all(&dispatch_dir)
        .with_context(|| format!("failed to create {}", dispatch_dir.display()))?;

    let gitignore_path = worktree.join(GITIGNORE_FILE);
    let existing = match fs::read_to_string(&gitignore_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(e).with_context(|| format!("failed to read {}", gitignore_path.display()));
        }
    };
    if existing
        .lines()
        .any(|l| l.trim() == DISPATCH_GITIGNORE_LINE)
    {
        return Ok(());
    }

    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(DISPATCH_GITIGNORE_LINE);
    updated.push('\n');
    fs::write(&gitignore_path, updated)
        .with_context(|| format!("failed to write {}", gitignore_path.display()))
}

#[derive(Debug)]
pub(super) struct ProvisionResult {
    pub(super) worktree_path: String,
    pub(super) tmux_window: String,
    /// `Some(...)` when every `git fetch origin <base>` attempt failed;
    /// injected as a note into the agent's own prompt in `dispatch_with_prompt`.
    pub(super) fetch_warning: Option<String>,
}

/// Attempt `git fetch origin <base>` up to `FETCH_MAX_ATTEMPTS` times,
/// sleeping `FETCH_RETRY_DELAY` between attempts. Returns `Ok(())` on the
/// first success, or `Err(<last stderr/error text>)` once every attempt has
/// failed.
fn fetch_origin_with_retry(
    runner: &dyn ProcessRunner,
    repo_path: &str,
    base: &str,
    timeout: Duration,
) -> Result<(), String> {
    let mut last_err = String::new();
    for attempt in 1..=FETCH_MAX_ATTEMPTS {
        match runner.run_with_timeout("git", &["-C", repo_path, "fetch", "origin", base], timeout)
        {
            Ok(output) if output.status.success() => return Ok(()),
            Ok(output) => last_err = stderr_str(&output),
            Err(e) => last_err = e.to_string(),
        }
        if attempt < FETCH_MAX_ATTEMPTS {
            std::thread::sleep(FETCH_RETRY_DELAY);
        }
    }
    Err(last_err)
}

/// Create a git worktree and open a tmux window.
/// Shared by both `dispatch_agent` and `brainstorm_agent`.
///
/// `timeout` is passed to `run_with_timeout` for long-running git subprocesses
/// (`git fetch`, `git worktree add`). Use [`crate::process::SUBPROCESS_TIMEOUT`]
/// in production; pass a short duration in tests.
pub(super) fn provision_worktree(
    task: &Task,
    runner: &dyn ProcessRunner,
    base_branch: Option<&str>,
    timeout: Duration,
) -> Result<ProvisionResult> {
    let repo_path = validate_repo_path(&task.repo_path).map_err(|e| anyhow::anyhow!(e))?;
    let slug = slugify(&task.title);
    let worktree_name = format!("{}-{slug}", task.id);
    let worktree_path = format!("{repo_path}/.worktrees/{worktree_name}");
    let tmux_window = build_tmux_window_name(task.id);

    tracing::info!(task_id = task.id.0, %worktree_path, ?base_branch, "provisioning worktree");

    fs::create_dir_all(format!("{repo_path}/.worktrees"))
        .context("failed to create .worktrees directory")?;

    // Fetch origin/<base_branch> unconditionally — even when reusing an
    // existing worktree directory — so `origin/<base>` stays fresh for
    // whatever rebases onto it later (the agent's own rebase preamble, or a
    // manual sync). Soft-fail: if fetch is unavailable (no origin, no
    // network), fall back to the local branch and continue — dispatch is not
    // blocked.
    let mut fetch_warning: Option<String> = None;
    let start_point: Option<String> = base_branch.map(|base| {
        match fetch_origin_with_retry(runner, &repo_path, base, timeout) {
            Ok(()) => format!("origin/{base}"),
            Err(err) => {
                tracing::warn!(
                    base,
                    error = %err,
                    "git fetch origin failed after retries, falling back to local branch"
                );
                fetch_warning = Some(format!(
                    "Could not fetch origin/{base} after {FETCH_MAX_ATTEMPTS} attempts \
                     ({err}); using local branch, which may be stale."
                ));
                base.to_string()
            }
        }
    });

    if std::path::Path::new(&worktree_path).exists() {
        tracing::info!(task_id = task.id.0, %worktree_path, "worktree already exists, reusing");
    } else {
        let mut args = vec![
            "-C",
            &repo_path,
            "worktree",
            "add",
            &worktree_path,
            "-B",
            &worktree_name,
        ];
        if let Some(sp) = start_point.as_deref() {
            args.push(sp);
        }
        let output = runner
            .run_with_timeout("git", &args, timeout)
            .context("failed to run git worktree add")?;
        anyhow::ensure!(
            output.status.success(),
            "git worktree add failed: {}",
            stderr_str(&output)
        );
    }

    tmux::new_window(&tmux_window, &worktree_path, runner)
        .context("failed to create tmux window")?;

    tmux::set_window_dispatch_dir(&tmux_window, &worktree_path, runner)
        .context("failed to set tmux window dispatch dir")?;
    tmux::ensure_split_hook(runner).context("failed to ensure tmux split hook")?;

    Ok(ProvisionResult {
        worktree_path,
        tmux_window,
        fetch_warning,
    })
}

/// Remove the tmux window (if it still exists) and the git worktree.
///
/// Errors are logged but not propagated for the tmux step so that the
/// worktree removal is always attempted.
pub fn cleanup_task(
    repo_path: &str,
    worktree_path: &str,
    tmux_window: Option<&str>,
    runner: &dyn ProcessRunner,
) -> Result<()> {
    tracing::info!(worktree_path, "cleaning up task");

    if let Some(window) = tmux_window {
        match tmux::has_window(window, runner) {
            Ok(true) => {
                tmux::kill_window(window, runner)
                    .context("failed to kill tmux window during cleanup")?;
            }
            Ok(false) => {}
            Err(e) => {
                tracing::warn!("could not check tmux window during cleanup: {e}");
            }
        }
    }

    let repo = expand_tilde(repo_path);
    let output = runner
        .run(
            "git",
            &["-C", &repo, "worktree", "remove", "--force", worktree_path],
        )
        .context("failed to run git worktree remove")?;
    if !output.status.success() {
        let stderr = stderr_str(&output);
        // If the worktree is already gone (manually removed or pruned), treat as success.
        if stderr.contains("is not a working tree") {
            tracing::info!(worktree_path, "worktree already removed, skipping");
        } else {
            anyhow::bail!(
                "git worktree remove failed for path {worktree_path}: {}",
                stderr
            );
        }
    }

    if let Some(branch) = std::path::Path::new(worktree_path)
        .file_name()
        .and_then(|n| n.to_str())
    {
        // Best-effort: ignore errors (branch may not exist).
        let _ = runner.run("git", &["-C", &repo, "branch", "-D", branch]);
    }

    Ok(())
}

/// Extract the branch name from a worktree path (its last path component).
pub fn branch_from_worktree(worktree: &str) -> Option<String> {
    std::path::Path::new(worktree)
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
}

/// Validate that a repo path points to an existing directory.
///
/// Returns the expanded path on success, or an error message on failure.
pub fn validate_repo_path(path: &str) -> Result<String, String> {
    let expanded = expand_tilde(path);
    let p = std::path::Path::new(&expanded);
    if !p.exists() {
        return Err(format!("Directory does not exist: {expanded}"));
    }
    if !p.is_dir() {
        return Err(format!("Not a directory: {expanded}"));
    }
    Ok(expanded)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod gitignore_tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn provision_worktree_creates_dispatch_dir() {
        let dir = tempdir().expect("tempdir");
        ensure_dispatch_dir_and_gitignore(dir.path()).expect("ok");
        assert!(dir.path().join(".dispatch").is_dir());
    }

    #[test]
    fn provision_worktree_appends_dispatch_to_gitignore() {
        let dir = tempdir().expect("tempdir");
        ensure_dispatch_dir_and_gitignore(dir.path()).expect("ok");
        let contents = fs::read_to_string(dir.path().join(".gitignore")).expect("read");
        assert_eq!(
            contents.matches(".dispatch/").count(),
            1,
            ".dispatch/ should appear exactly once: {contents:?}"
        );
    }

    #[test]
    fn provision_worktree_gitignore_idempotent_when_already_present() {
        let dir = tempdir().expect("tempdir");
        let gi = dir.path().join(".gitignore");
        fs::write(&gi, "target/\n.dispatch/\nnode_modules/\n").expect("seed");
        let before = fs::read_to_string(&gi).expect("read");
        ensure_dispatch_dir_and_gitignore(dir.path()).expect("ok");
        let after = fs::read_to_string(&gi).expect("read");
        assert_eq!(before, after, ".gitignore should be unchanged");
    }

    #[test]
    fn provision_worktree_gitignore_preserves_prior_entries() {
        let dir = tempdir().expect("tempdir");
        let gi = dir.path().join(".gitignore");
        fs::write(&gi, "target/\n.env\n").expect("seed");
        ensure_dispatch_dir_and_gitignore(dir.path()).expect("ok");
        let after = fs::read_to_string(&gi).expect("read");
        assert!(after.contains("target/"));
        assert!(after.contains(".env"));
        assert!(after.contains(".dispatch/"));
    }

    #[test]
    fn provision_worktree_gitignore_handles_missing_trailing_newline() {
        let dir = tempdir().expect("tempdir");
        let gi = dir.path().join(".gitignore");
        fs::write(&gi, "target/").expect("seed"); // no trailing \n
        ensure_dispatch_dir_and_gitignore(dir.path()).expect("ok");
        let after = fs::read_to_string(&gi).expect("read");
        assert!(
            after.contains("target/\n"),
            "target/ retained on its own line"
        );
        assert!(after.ends_with(".dispatch/\n"));
    }
}
