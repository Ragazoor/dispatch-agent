use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use std::time::Duration;

use crate::models::{expand_tilde, slugify, Task};
use crate::process::ProcessRunner;
use crate::tmux;

use super::git_output::WORKTREE_ALREADY_REMOVED;
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

/// Which ref a worktree branch was created from — and therefore what the agent
/// should rebase onto if it ever needs to.
///
/// Both arms carry the bare branch name because the `git fetch origin <base>`
/// line is identical either way; only the rebase target differs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum StartPoint {
    /// `origin/<base>`: origin is at least as new as local `<base>`.
    Remote { base: String },
    /// Bare local `<base>`: it carries commits `origin/<base>` does not, or
    /// origin has no such branch at all.
    Local { base: String },
}

impl StartPoint {
    /// The ref to hand `git worktree add`, and to rebase onto.
    pub(super) fn git_ref(&self) -> String {
        match self {
            StartPoint::Remote { base } => format!("origin/{base}"),
            StartPoint::Local { base } => base.clone(),
        }
    }

    /// The bare branch name, for the `git fetch origin <base>` line.
    pub(super) fn base(&self) -> &str {
        match self {
            StartPoint::Remote { base } | StartPoint::Local { base } => base,
        }
    }
}

#[derive(Debug)]
pub(super) struct ProvisionResult {
    pub(super) worktree_path: String,
    pub(super) tmux_window: String,
    /// `Some(...)` when there is no `origin/<base>` to base on and the local
    /// branch was used instead. Injected into the agent's prompt as a `Note:`
    /// line by `dispatch_with_prompt`.
    pub(super) fetch_warning: Option<String>,
    /// The ref the branch was created from. `None` when no base was given.
    /// The caller needs it to point the rebase preamble at the same ref.
    pub(super) start_point: Option<StartPoint>,
    /// True when the worktree directory already existed and `git worktree add`
    /// was skipped, so the branch may still hold a previous attempt's state.
    pub(super) reused_worktree: bool,
}

/// `git ls-remote --exit-code` returns this when no ref matched — as opposed to
/// 128, which means it could not reach the remote at all.
const LS_REMOTE_NO_MATCHING_REF: i32 = 2;

/// The outcome of making `origin/<base>` current before provisioning.
#[derive(Debug)]
enum FetchOutcome {
    /// `origin/<base>` is up to date locally.
    Fetched,
    /// There is no `origin/<base>` to fetch. Carries the message shown to the
    /// agent as a `Note:` line.
    NoOriginRef(String),
}

/// Why a `git fetch origin <base>` failed.
enum FetchFailure {
    /// Nothing to fetch: no `origin` remote, or origin has no such branch.
    NoOriginRef(String),
    /// Origin has the branch, or we could not even determine that. Either way
    /// this is infrastructure, not a missing ref.
    Unreachable,
}

/// Classify a fetch failure without pattern-matching git's stderr text.
///
/// `git fetch` exits 128 for a missing ref, an unresolvable host and an
/// unreadable remote alike, so its own status cannot classify.
/// `git ls-remote --exit-code` can: 2 means "no matching ref", 128 means "could
/// not reach the remote". Anything we cannot positively identify as a missing
/// ref is treated as unreachable — the safe polarity, since only a recognised
/// 404 earns the local-branch fallback.
fn classify_fetch_failure(
    runner: &dyn ProcessRunner,
    repo_path: &str,
    base: &str,
    timeout: Duration,
) -> FetchFailure {
    if !crate::git::has_origin_remote(repo_path, runner) {
        return FetchFailure::NoOriginRef("no origin remote is configured".to_string());
    }
    let refspec = format!("refs/heads/{base}");
    let probe = runner.run_with_timeout(
        "git",
        &[
            "-C",
            repo_path,
            "ls-remote",
            "--exit-code",
            "origin",
            &refspec,
        ],
        timeout,
    );
    match probe {
        Ok(output) if output.status.code() == Some(LS_REMOTE_NO_MATCHING_REF) => {
            FetchFailure::NoOriginRef(format!("origin has no branch {base}"))
        }
        _ => FetchFailure::Unreachable,
    }
}

/// Make `origin/<base>` current, or establish that there is no such ref.
///
/// An infrastructure failure is retried up to `FETCH_MAX_ATTEMPTS` and then
/// aborts the dispatch: a worktree silently branched off a stale local ref is
/// worse than a dispatch that refuses to start. A missing ref is not retried —
/// retrying a branch that does not exist cannot succeed — and is not an error,
/// because local `<base>` is then the only ref there is.
fn fetch_origin(
    runner: &dyn ProcessRunner,
    repo_path: &str,
    base: &str,
    timeout: Duration,
) -> Result<FetchOutcome> {
    let mut last_err = String::new();
    for attempt in 1..=FETCH_MAX_ATTEMPTS {
        match runner.run_with_timeout("git", &["-C", repo_path, "fetch", "origin", base], timeout) {
            Ok(output) if output.status.success() => return Ok(FetchOutcome::Fetched),
            Ok(output) => last_err = stderr_str(&output),
            Err(e) => last_err = e.to_string(),
        }
        // Classify once, on the first failure: the answer cannot change between
        // attempts, and a 404 must not burn the retry budget.
        if attempt == 1 {
            if let FetchFailure::NoOriginRef(reason) =
                classify_fetch_failure(runner, repo_path, base, timeout)
            {
                tracing::info!(base, %reason, "no origin ref to fetch; using the local branch");
                return Ok(FetchOutcome::NoOriginRef(format!(
                    "Could not fetch origin/{base} ({reason}); this worktree is based on \
                     the local {base} branch."
                )));
            }
        }
        if attempt < FETCH_MAX_ATTEMPTS {
            std::thread::sleep(FETCH_RETRY_DELAY);
        }
    }
    tracing::warn!(base, error = %last_err, "could not reach origin; aborting dispatch");
    anyhow::bail!(
        "Could not reach origin to fetch {base} after {FETCH_MAX_ATTEMPTS} attempts: {last_err}"
    )
}

/// What a worktree is being based on, and whether local history may be
/// preferred over origin's.
#[derive(Debug, Clone, Copy)]
pub(super) enum BaseRef<'a> {
    /// The repo's base branch. Local `<base>` may legitimately hold commits
    /// origin lacks, so the two are compared and the one with unique commits
    /// wins.
    Branch(&'a str),
    /// A PR's head branch. Always `origin/<branch>`, never compared: a review
    /// must see exactly the PR's code, and a stale local branch of the same
    /// name would silently poison it.
    ///
    /// Constructed by `dispatch::agents::dispatch_with_prompt` whenever a
    /// review task resolves a PR head branch. Exercised directly by
    /// `provision_worktree_never_measures_a_pr_head_branch`.
    PrHead(&'a str),
}

impl BaseRef<'_> {
    fn name(&self) -> &str {
        match self {
            BaseRef::Branch(b) | BaseRef::PrHead(b) => b,
        }
    }
}

/// Choose the ref a new worktree branch starts from, given a fetch that just
/// succeeded so both refs are current.
///
/// Local `<base>` wins only on a positive `ahead > 0` reading. That polarity is
/// load-bearing: `ahead_behind` yields `None` whenever local `<base>` does not
/// resolve, which is the normal case for a base branch the human never checked
/// out, and preferring local there would fail `git worktree add`.
fn select_start_point(runner: &dyn ProcessRunner, repo_path: &str, base: &str) -> StartPoint {
    let base = base.to_string();
    match crate::repo_sync::ahead_behind(repo_path, &base, runner) {
        Some(counts) if counts.ahead > 0 => StartPoint::Local { base },
        _ => StartPoint::Remote { base },
    }
}

/// Create a git worktree and open a tmux window.
/// Shared by `dispatch_agent`, `research_agent`, and `quick_dispatch_agent`,
/// all of which reach it via `dispatch_with_prompt`.
///
/// `timeout` is passed to `run_with_timeout` for long-running git subprocesses
/// (`git fetch`, `git worktree add`). Use [`crate::process::SUBPROCESS_TIMEOUT`]
/// in production; pass a short duration in tests.
pub(super) fn provision_worktree(
    task: &Task,
    runner: &dyn ProcessRunner,
    base: Option<BaseRef<'_>>,
    timeout: Duration,
) -> Result<ProvisionResult> {
    let repo_path = validate_repo_path(&task.repo_path).map_err(|e| anyhow::anyhow!(e))?;
    let slug = slugify(&task.title);
    let worktree_name = format!("{}-{slug}", task.id);
    let worktree_path = format!("{repo_path}/.worktrees/{worktree_name}");
    let tmux_window = build_tmux_window_name(task.id);

    tracing::info!(task_id = task.id.0, %worktree_path, ?base, "provisioning worktree");

    fs::create_dir_all(format!("{repo_path}/.worktrees"))
        .context("failed to create .worktrees directory")?;

    // The fetch runs unconditionally — even when reusing an existing worktree
    // directory — so `origin/<base>` stays fresh for whatever rebases onto it
    // later. An unreachable origin aborts here rather than quietly producing a
    // worktree based on a stale local ref.
    let (start_point, fetch_warning): (Option<StartPoint>, Option<String>) = match base {
        Some(base_ref) => match fetch_origin(runner, &repo_path, base_ref.name(), timeout)? {
            FetchOutcome::Fetched => {
                let sp = match base_ref {
                    BaseRef::PrHead(b) => StartPoint::Remote {
                        base: b.to_string(),
                    },
                    BaseRef::Branch(b) => select_start_point(runner, &repo_path, b),
                };
                (Some(sp), None)
            }
            FetchOutcome::NoOriginRef(warning) => (
                Some(StartPoint::Local {
                    base: base_ref.name().to_string(),
                }),
                Some(warning),
            ),
        },
        None => (None, None),
    };

    let start_ref = start_point.as_ref().map(StartPoint::git_ref);

    let reused_worktree = std::path::Path::new(&worktree_path).exists();
    if reused_worktree {
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
        if let Some(sp) = start_ref.as_deref() {
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

    tracing::info!(
        task_id = task.id.0,
        base = start_point.as_ref().map(StartPoint::base),
        start_ref = start_ref.as_deref(),
        reused_worktree,
        "worktree provisioned"
    );

    Ok(ProvisionResult {
        worktree_path,
        tmux_window,
        fetch_warning,
        start_point,
        reused_worktree,
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
        tmux::kill_window_if_present(window, runner)
            .context("failed to kill tmux window during cleanup")?;
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
        if stderr.contains(WORKTREE_ALREADY_REMOVED) {
            tracing::info!(worktree_path, "worktree already removed, skipping");
        } else {
            anyhow::bail!(
                "git worktree remove failed for path {worktree_path}: {}",
                stderr
            );
        }
    }

    if let Some(branch) = branch_from_worktree(worktree_path) {
        // Best-effort: ignore errors (branch may not exist).
        let _ = runner.run("git", &["-C", &repo, "branch", "-D", &branch]);
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod fetch_tests {
    use super::*;
    use crate::process::MockProcessRunner;
    use std::time::Duration;

    const T: Duration = Duration::from_secs(5);

    #[test]
    fn successful_fetch_reports_fetched() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        assert!(matches!(
            fetch_origin(&mock, "/repo", "main", T).unwrap(),
            FetchOutcome::Fetched
        ));
        assert_eq!(mock.recorded_calls().len(), 1, "no classification needed");
    }

    #[test]
    fn missing_origin_remote_is_not_retried() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::fail("no such remote"), // git fetch
            MockProcessRunner::fail("no origin"),      // git remote get-url origin
        ]);
        let outcome = fetch_origin(&mock, "/repo", "main", T).unwrap();
        let FetchOutcome::NoOriginRef(warning) = outcome else {
            panic!("expected NoOriginRef");
        };
        assert!(warning.contains("origin remote"), "got: {warning}");
        assert_eq!(
            mock.recorded_calls().len(),
            2,
            "one fetch, one classification — no retries: {:?}",
            mock.recorded_calls()
        );
    }

    #[test]
    fn branch_absent_from_origin_is_not_retried() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::fail("couldn't find remote ref"), // git fetch
            MockProcessRunner::ok(),                             // git remote get-url origin
            MockProcessRunner::fail_with_code(2, ""),            // git ls-remote --exit-code
        ]);
        let outcome = fetch_origin(&mock, "/repo", "nosuch", T).unwrap();
        let FetchOutcome::NoOriginRef(warning) = outcome else {
            panic!("expected NoOriginRef");
        };
        assert!(warning.contains("nosuch"), "got: {warning}");
        assert_eq!(mock.recorded_calls().len(), 3, "no retries after a 404");
    }

    #[test]
    fn unreachable_origin_is_retried_then_aborts() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::fail("Could not resolve host"), // fetch 1
            MockProcessRunner::ok(),                           // remote get-url origin
            MockProcessRunner::fail_with_code(128, ""),        // ls-remote: unreachable
            MockProcessRunner::fail("Could not resolve host"), // fetch 2
            MockProcessRunner::fail("Could not resolve host"), // fetch 3
        ]);
        let err = fetch_origin(&mock, "/repo", "main", T).unwrap_err();
        assert!(
            err.to_string().contains("Could not reach origin"),
            "got: {err}"
        );
        assert_eq!(
            mock.recorded_calls().len(),
            5,
            "classify once, then retry the fetch: {:?}",
            mock.recorded_calls()
        );
    }

    #[test]
    fn existing_ref_that_fails_to_fetch_aborts_rather_than_using_local() {
        // ls-remote finds the ref, so origin is reachable and the branch is
        // there — a fetch that still fails is infrastructure, never a 404.
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::fail("early EOF"),
            MockProcessRunner::ok(), // remote get-url origin
            MockProcessRunner::ok(), // ls-remote: exit 0, ref exists
            MockProcessRunner::fail("early EOF"),
            MockProcessRunner::fail("early EOF"),
        ]);
        assert!(fetch_origin(&mock, "/repo", "main", T).is_err());
    }

    #[test]
    fn fetch_succeeding_on_retry_reports_fetched() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::fail("early EOF"),
            MockProcessRunner::ok(), // remote get-url origin
            MockProcessRunner::fail_with_code(128, ""), // ls-remote: unreachable
            MockProcessRunner::ok(), // fetch 2 succeeds
        ]);
        assert!(matches!(
            fetch_origin(&mock, "/repo", "main", T).unwrap(),
            FetchOutcome::Fetched
        ));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod selection_tests {
    use super::*;
    use crate::process::MockProcessRunner;

    // `git rev-list --count --left-right <base>...origin/<base>` prints
    // "<ahead>\t<behind>".
    fn counts(ahead: u32, behind: u32) -> anyhow::Result<std::process::Output> {
        MockProcessRunner::ok_with_stdout(format!("{ahead}\t{behind}\n").as_bytes())
    }

    #[test]
    fn local_wins_when_it_holds_commits_origin_lacks() {
        let mock = MockProcessRunner::new(vec![counts(3, 0)]);
        assert_eq!(
            select_start_point(&mock, "/repo", "main"),
            StartPoint::Local {
                base: "main".to_string()
            }
        );
    }

    #[test]
    fn origin_wins_when_local_is_behind() {
        let mock = MockProcessRunner::new(vec![counts(0, 2)]);
        assert_eq!(
            select_start_point(&mock, "/repo", "main"),
            StartPoint::Remote {
                base: "main".to_string()
            }
        );
    }

    #[test]
    fn origin_wins_when_the_two_are_level() {
        let mock = MockProcessRunner::new(vec![counts(0, 0)]);
        assert_eq!(
            select_start_point(&mock, "/repo", "main"),
            StartPoint::Remote {
                base: "main".to_string()
            }
        );
    }

    #[test]
    fn diverged_takes_local_silently() {
        let mock = MockProcessRunner::new(vec![counts(3, 2)]);
        assert_eq!(
            select_start_point(&mock, "/repo", "main"),
            StartPoint::Local {
                base: "main".to_string()
            }
        );
    }

    #[test]
    fn unmeasurable_falls_to_origin_not_local() {
        // This is what "local <base> does not exist" looks like — a base branch
        // the human never checked out. Preferring local here would hand
        // `git worktree add` a ref that is not there.
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("unknown revision")]);
        assert_eq!(
            select_start_point(&mock, "/repo", "develop"),
            StartPoint::Remote {
                base: "develop".to_string()
            }
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod start_point_tests {
    use super::*;

    #[test]
    fn remote_start_point_refs_origin_and_keeps_the_bare_base() {
        let sp = StartPoint::Remote {
            base: "develop".to_string(),
        };
        assert_eq!(sp.git_ref(), "origin/develop");
        assert_eq!(sp.base(), "develop");
    }

    #[test]
    fn local_start_point_refs_the_bare_branch() {
        let sp = StartPoint::Local {
            base: "develop".to_string(),
        };
        assert_eq!(sp.git_ref(), "develop");
        assert_eq!(sp.base(), "develop");
    }
}
