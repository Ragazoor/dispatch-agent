//! Local-first repo sync: keeping a repository's primary checkout in step with
//! origin on its own default branch.
//!
//! Spec: `docs/specs/repo-sync.allium` (the `RepoSyncEngine` contract and the
//! `SyncRepo` rule). Structured like [`crate::dispatch::finish`] — synchronous,
//! [`ProcessRunner`]-driven, with no TUI or database coupling, so the same three
//! operations back the board action and the CLI.

use std::collections::HashMap;

use crate::models::expand_tilde;
use crate::process::{stderr_str, stdout_str, ProcessRunner, SUBPROCESS_TIMEOUT};

/// A two-sided commit count between a repository's local base branch and its
/// origin counterpart: `ahead` commits are reachable only from the local
/// branch, `behind` commits only from `origin/<base>`.
///
/// The pair is always read together, from the same observation, so it is one
/// value rather than two fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AheadBehind {
    pub ahead: u32,
    pub behind: u32,
}

impl AheadBehind {
    /// Any non-zero side is drift worth surfacing.
    pub fn has_drift(&self) -> bool {
        self.ahead > 0 || self.behind > 0
    }

    /// Both sides non-zero: local and origin have each moved independently.
    /// This is the case resolved by merging rather than rebasing.
    pub fn is_diverged(&self) -> bool {
        self.ahead > 0 && self.behind > 0
    }
}

/// The two ways a sync can succeed. Distinguishable to the caller on purpose:
/// `AlreadyInSync` means the fetch ran and found nothing to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncOutcome {
    /// Both counts were zero (or unmeasurable) after the fetch. No merge and no
    /// push were performed.
    AlreadyInSync,
    /// Commits actually pulled into local base and pushed to origin.
    Synced { pulled: u32, pushed: u32 },
}

/// Every way a sync can fail, one variant per cause. The split is the point:
/// each cause has a different remedy and a different message, so a dirty tree
/// can never masquerade as a merge conflict.
#[derive(Debug)]
pub enum SyncError {
    /// The repository has no `origin` remote configured.
    NoRemote,
    /// The primary checkout is on some other branch. Both the merge and the
    /// push act on whatever is checked out, so this stops the operation.
    NotOnBaseBranch { current: String, expected: String },
    /// The primary checkout has uncommitted changes; merging into a dirty tree
    /// is how work is lost.
    DirtyPrimaryWorktree { path: String, files: Vec<String> },
    /// `origin/<base>` did not merge cleanly. The merge was aborted and the
    /// conflicted paths reported.
    MergeConflict { files: Vec<String> },
    /// Origin moved between the fetch and the push. Retryable as-is.
    PushRejected { stderr: String },
    /// A git invocation failed for some other reason.
    Other(String),
}

impl SyncError {
    /// A rejected push means origin moved between the fetch and the push, so
    /// repeating the same action is the fix. Every other cause needs the
    /// operator to change something first.
    pub fn retryable(&self) -> bool {
        matches!(self, SyncError::PushRejected { .. })
    }
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncError::NoRemote => write!(f, "No origin remote configured — nothing to sync with"),
            SyncError::NotOnBaseBranch { current, expected } => write!(
                f,
                "Repo root is not on {expected} (currently on {current}) — checkout {expected} first"
            ),
            SyncError::DirtyPrimaryWorktree { path, files } => write!(
                f,
                "Primary worktree at {path} has uncommitted changes ({}) — commit or stash them before syncing",
                files.join(", ")
            ),
            SyncError::MergeConflict { files } => {
                let location = if files.is_empty() {
                    String::new()
                } else {
                    format!(" in {}", files.join(", "))
                };
                write!(
                    f,
                    "Merge conflict{location} — the merge was aborted; resolve it by hand and try again"
                )
            }
            SyncError::PushRejected { stderr } => write!(
                f,
                "Push rejected — origin moved since the fetch; try again: {stderr}"
            ),
            SyncError::Other(msg) => write!(f, "{msg}"),
        }
    }
}

/// The `<base>...origin/<base>` range whose two-sided count is the drift.
fn count_range(base_branch: &str) -> String {
    format!("{base_branch}...origin/{base_branch}")
}

/// Count commits on each side of `<base>...origin/<base>`.
///
/// Yields `None` — never `AheadBehind { 0, 0 }` — when `origin/<base_branch>`
/// does not resolve (no remote, never fetched) or when the output cannot be
/// parsed. A repository that cannot be measured must not be reported as clean
/// (`UnmeasurableIsNotInSync`).
///
/// Bounded by [`SUBPROCESS_TIMEOUT`] like every other subprocess here: walking
/// history can block on a lock, and this runs on the dispatch path and the TUI's
/// drift poll, neither of which may hang on it. A timed-out walk is simply an
/// unmeasurable one.
pub fn ahead_behind(
    repo_path: &str,
    base_branch: &str,
    runner: &dyn ProcessRunner,
) -> Option<AheadBehind> {
    let repo_path = expand_tilde(repo_path);
    let output = runner
        .run_with_timeout(
            "git",
            &[
                "-C",
                &repo_path,
                "rev-list",
                "--count",
                "--left-right",
                &count_range(base_branch),
            ],
            SUBPROCESS_TIMEOUT,
        )
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = stdout_str(&output);
    let mut fields = stdout.split_whitespace();
    let ahead = fields.next()?.parse().ok()?;
    let behind = fields.next()?.parse().ok()?;
    if fields.next().is_some() {
        // More than two counts is not output this command produces; refusing to
        // guess is the whole point of the invariant.
        return None;
    }
    Some(AheadBehind { ahead, behind })
}

/// Fetch `origin/<base_branch>` so the counts that follow are trustworthy.
///
/// Yields `Ok(())` on success and the failure message otherwise; a failed fetch
/// is non-fatal everywhere it is used.
pub fn fetch_base(
    repo_path: &str,
    base_branch: &str,
    runner: &dyn ProcessRunner,
) -> Result<(), String> {
    let repo_path = expand_tilde(repo_path);
    let output = runner
        .run_with_timeout(
            "git",
            &["-C", &repo_path, "fetch", "origin", base_branch],
            SUBPROCESS_TIMEOUT,
        )
        .map_err(|e| format!("Failed to fetch origin {base_branch}: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "Failed to fetch origin {base_branch}: {}",
        stderr_str(&output)
    ))
}

/// Bring the repository's primary checkout into step with `origin/<base_branch>`
/// and publish whatever it is ahead by.
///
/// Every precondition is checked before any write, each as its own error
/// variant (`PreconditionsPrecedeEveryWrite`). The fetch then runs
/// unconditionally, before the counts that decide whether to merge or push
/// (`FetchPrecedesCounting`). Divergence is closed by merging, never by
/// rebasing or resetting local base (`LocalBaseHistoryIsNeverRewritten`), so
/// worktrees already branched off it stay valid.
pub fn sync_repo(
    repo_path: &str,
    base_branch: &str,
    runner: &dyn ProcessRunner,
) -> Result<SyncOutcome, SyncError> {
    let repo = expand_tilde(repo_path);

    // --- Preconditions, all before any write ---

    // 1. An origin remote must exist. Both a probe that cannot be run and one
    //    that reports no origin mean the same thing here — nothing to sync
    //    against — so both report NoRemote rather than splitting the first into
    //    Other. Spec: PreconditionsPrecedeEveryWrite's stated carve-out.
    if !crate::git::has_origin_remote(&repo, runner).unwrap_or(false) {
        return Err(SyncError::NoRemote);
    }

    // 2. The checkout must be on the base branch — the merge and the push both
    //    act on whatever is checked out.
    let current = crate::git::current_branch(&repo, runner).map_err(SyncError::Other)?;
    if current != base_branch {
        return Err(SyncError::NotOnBaseBranch {
            current,
            expected: base_branch.to_string(),
        });
    }

    // 3. The checkout must be clean — merging into a dirty tree loses work.
    let dirty = crate::git::dirty_files(&repo, runner).map_err(SyncError::Other)?;
    if !dirty.is_empty() {
        return Err(SyncError::DirtyPrimaryWorktree {
            path: repo.clone(),
            files: dirty,
        });
    }

    // --- Fetch, unconditionally, then count against the refreshed refs ---

    fetch_base(&repo, base_branch, runner).map_err(SyncError::Other)?;

    let Some(counts) = ahead_behind(&repo, base_branch, runner) else {
        // Nothing measurable to act on even against freshly fetched refs.
        return Ok(SyncOutcome::AlreadyInSync);
    };
    if !counts.has_drift() {
        return Ok(SyncOutcome::AlreadyInSync);
    }

    // --- Merge (fast-forwards when ahead = 0, merge commit when diverged) ---

    let mut ahead = counts.ahead;
    if counts.behind > 0 {
        let output = runner
            .run_with_timeout(
                "git",
                &[
                    "-C",
                    &repo,
                    "merge",
                    "--no-edit",
                    &format!("origin/{base_branch}"),
                ],
                SUBPROCESS_TIMEOUT,
            )
            .map_err(|e| SyncError::Other(format!("Failed to run git merge: {e}")))?;
        if !output.status.success() {
            // Read the conflicted paths from the repo's own status *before*
            // aborting — the abort clears them
            // (`ConflictFilesCapturedBeforeAbort`).
            let conflicted = runner
                .run_with_timeout(
                    "git",
                    &["-C", &repo, "status", "--porcelain"],
                    SUBPROCESS_TIMEOUT,
                )
                .map(|o| crate::git::parse_unmerged_files(&o))
                .unwrap_or_default();
            let _ = runner.run_with_timeout(
                "git",
                &["-C", &repo, "merge", "--abort"],
                SUBPROCESS_TIMEOUT,
            );
            if !conflicted.is_empty() {
                return Err(SyncError::MergeConflict { files: conflicted });
            }
            return Err(SyncError::Other(format!(
                "Merge of origin/{base_branch} failed: {}",
                stderr_str(&output)
            )));
        }
        // A merge commit is itself something to publish, so the ahead count is
        // re-read rather than reused. Unmeasurable after the merge means no
        // push: refusing to guess beats pushing a count we cannot justify.
        ahead = ahead_behind(&repo, base_branch, runner)
            .map(|c| c.ahead)
            .unwrap_or(0);
    }

    // --- Push whatever local base is ahead by ---

    if ahead > 0 {
        let output = runner
            .run_with_timeout(
                "git",
                &["-C", &repo, "push", "origin", base_branch],
                SUBPROCESS_TIMEOUT,
            )
            .map_err(|e| SyncError::Other(format!("Failed to run git push: {e}")))?;
        if !output.status.success() {
            return Err(SyncError::PushRejected {
                stderr: stderr_str(&output),
            });
        }
    }

    Ok(SyncOutcome::Synced {
        pulled: counts.behind,
        pushed: ahead,
    })
}

// ---------------------------------------------------------------------------
// Measurement state — the `RepoSyncState` entity and its per-repo cache
// ---------------------------------------------------------------------------

/// The current drift measurement for one repository (spec: entity
/// `RepoSyncState`).
///
/// A measurement, not a record: every consumer establishes it for itself — the
/// board refreshes it on the trigger events, the CLI computes it when it runs —
/// so it crosses no process boundary and is never persisted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSyncState {
    /// Absolute path to the repository's primary checkout; identifies the
    /// measurement.
    pub repo_path: String,
    /// The repository's own default branch, as measured.
    pub base_branch: String,
    /// `None` when `origin/<base_branch>` could not be measured at all.
    pub counts: Option<AheadBehind>,
    /// Message from the most recent failed fetch; cleared once a fetch succeeds.
    pub last_fetch_error: Option<String>,
}

impl RepoSyncState {
    /// Whether the repository could be measured. An unmeasured repository is
    /// distinct from a clean one and must never be presented as clean
    /// (`UnmeasuredIsNeverPresentedAsClean`).
    pub fn is_measured(&self) -> bool {
        self.counts.is_some()
    }

    /// Drift the user can act on. False both when clean and when unmeasured.
    pub fn has_drift(&self) -> bool {
        self.counts.is_some_and(|c| c.has_drift())
    }
}

/// One refresh observation, before it is folded into the cached state.
///
/// Kept apart from [`RepoSyncState`] because a failed fetch must record only its
/// error and leave the previously known counts alone — a merge the observation
/// itself cannot perform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSyncMeasurement {
    pub repo_path: String,
    pub base_branch: String,
    pub counts: Option<AheadBehind>,
    pub fetch_error: Option<String>,
}

/// The per-repository measurement cache, keyed by `repo_path`.
///
/// Keying by path is what enforces `UniqueMeasurementPerRepo`: a repeated
/// refresh replaces the repository's measurement rather than adding a second.
#[derive(Debug, Default, Clone)]
pub struct RepoSyncCache(HashMap<String, RepoSyncState>);

impl RepoSyncCache {
    /// The measurement for `repo_path`, or `None` when it has never been
    /// refreshed.
    pub fn get(&self, repo_path: &str) -> Option<&RepoSyncState> {
        self.0.get(repo_path)
    }

    /// Number of repositories measured.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether no repository has been measured yet.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Fold one observation in, per the `RefreshRepoSyncState` rule: create the
    /// state on a first observation; on a later one, a failed fetch records only
    /// the error (keeping the previously known counts, so a slow or offline
    /// network leaves the drift indicator undisturbed) while a successful fetch
    /// replaces the branch, the counts and clears the error.
    pub fn apply(&mut self, m: RepoSyncMeasurement) {
        match self.0.get_mut(&m.repo_path) {
            None => {
                self.0.insert(
                    m.repo_path.clone(),
                    RepoSyncState {
                        repo_path: m.repo_path,
                        base_branch: m.base_branch,
                        counts: m.counts,
                        last_fetch_error: m.fetch_error,
                    },
                );
            }
            Some(state) => {
                if m.fetch_error.is_some() {
                    state.last_fetch_error = m.fetch_error;
                } else {
                    state.base_branch = m.base_branch;
                    state.counts = m.counts;
                    state.last_fetch_error = None;
                }
            }
        }
    }
}

/// Measure one repository: resolve its own default branch, optionally fetch, and
/// read the two-sided count from the refreshed refs.
///
/// This is the measurement half of `RefreshRepoSyncState`. `fetch_first` is true
/// only for the TUI's startup refresh and for `dispatch repo status` without
/// `--no-fetch`; every other caller rides refs some other operation just
/// refreshed, making this a pure local ref read.
///
/// A failed fetch yields the error and *no* counts: counting against refs this
/// call failed to refresh is exactly what `FetchPrecedesCounting` forbids.
pub fn measure_repo(
    repo_path: &str,
    fetch_first: bool,
    runner: &dyn ProcessRunner,
) -> RepoSyncMeasurement {
    let base_branch = crate::git::detect_default_branch(repo_path, runner);
    let fetch_error = if fetch_first {
        fetch_base(repo_path, &base_branch, runner).err()
    } else {
        None
    };
    let counts = if fetch_error.is_none() {
        ahead_behind(repo_path, &base_branch, runner)
    } else {
        None
    };
    RepoSyncMeasurement {
        repo_path: repo_path.to_string(),
        base_branch,
        counts,
        fetch_error,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::process::MockProcessRunner;
    use anyhow::Result;
    use std::process::Output;

    const REPO: &str = "/repo";
    const BASE: &str = "main";

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|p| (*p).to_string()).collect()
    }

    /// The three precondition responses every sync that gets past them starts
    /// with: origin exists, HEAD is on `main`, working tree clean.
    fn preconditions_ok() -> Vec<Result<Output>> {
        vec![
            MockProcessRunner::ok_with_stdout(b"git@github.com:org/repo.git\n"),
            MockProcessRunner::ok_with_stdout(b"main\n"),
            MockProcessRunner::ok_with_stdout(b""),
        ]
    }

    /// `preconditions_ok()` followed by `rest`, the shape of nearly every
    /// `sync_repo` test.
    fn responses(rest: Vec<Result<Output>>) -> Vec<Result<Output>> {
        let mut all = preconditions_ok();
        all.extend(rest);
        all
    }

    // ---------------------------------------------------------------------
    // AheadBehind — derived values
    // ---------------------------------------------------------------------

    #[test]
    fn ahead_behind_has_drift_on_either_side() {
        assert!(!AheadBehind {
            ahead: 0,
            behind: 0
        }
        .has_drift());
        assert!(AheadBehind {
            ahead: 3,
            behind: 0
        }
        .has_drift());
        assert!(AheadBehind {
            ahead: 0,
            behind: 2
        }
        .has_drift());
        assert!(AheadBehind {
            ahead: 3,
            behind: 2
        }
        .has_drift());
    }

    #[test]
    fn ahead_behind_is_diverged_only_when_both_sides_are_non_zero() {
        assert!(!AheadBehind {
            ahead: 0,
            behind: 0
        }
        .is_diverged());
        assert!(!AheadBehind {
            ahead: 3,
            behind: 0
        }
        .is_diverged());
        assert!(!AheadBehind {
            ahead: 0,
            behind: 2
        }
        .is_diverged());
        assert!(AheadBehind {
            ahead: 3,
            behind: 2
        }
        .is_diverged());
    }

    // ---------------------------------------------------------------------
    // ahead_behind
    // ---------------------------------------------------------------------

    #[test]
    fn ahead_behind_parses_left_right_counts() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"3\t1\n")]);
        assert_eq!(
            ahead_behind(REPO, BASE, &mock),
            Some(AheadBehind {
                ahead: 3,
                behind: 1
            })
        );
    }

    // A repo that is genuinely level is *measured*, so it yields a zero pair —
    // distinct from the unmeasurable None below.
    #[test]
    fn ahead_behind_reports_a_level_repo_as_measured_zeroes() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"0\t0\n")]);
        let counts = ahead_behind(REPO, BASE, &mock).expect("a level repo is still measurable");
        assert_eq!(
            counts,
            AheadBehind {
                ahead: 0,
                behind: 0
            }
        );
        assert!(!counts.has_drift());
    }

    // UnmeasurableIsNotInSync: no origin ref to compare against yields no
    // counts at all, never a zero pair.
    #[test]
    fn ahead_behind_yields_nothing_when_origin_ref_does_not_resolve() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail(
            "fatal: ambiguous argument 'main...origin/main': unknown revision",
        )]);
        assert_eq!(ahead_behind(REPO, BASE, &mock), None);
    }

    #[test]
    fn ahead_behind_yields_nothing_for_unparseable_output() {
        for stdout in [
            &b"banana\n"[..],
            &b"3\n"[..],
            &b"3\tmany\n"[..],
            &b""[..],
            &b"3\t1\t9\n"[..],
        ] {
            let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(stdout)]);
            assert_eq!(
                ahead_behind(REPO, BASE, &mock),
                None,
                "unparseable output {:?} must not be reported as counts",
                String::from_utf8_lossy(stdout)
            );
        }
    }

    #[test]
    fn ahead_behind_yields_nothing_when_the_runner_errors() {
        let mock = MockProcessRunner::new(vec![Err(anyhow::anyhow!("git not on PATH"))]);
        assert_eq!(ahead_behind(REPO, BASE, &mock), None);
    }

    #[test]
    fn ahead_behind_invokes_rev_list_left_right_against_origin() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"0\t0\n")]);
        let _ = ahead_behind("/some/repo", "master", &mock);
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "git");
        assert_eq!(
            calls[0].1,
            argv(&[
                "-C",
                "/some/repo",
                "rev-list",
                "--count",
                "--left-right",
                "master...origin/master",
            ])
        );
    }

    // Every subprocess this engine issues is bounded. rev-list walks history and
    // can take a lock, so an unbounded one wedges whatever called it — the TUI's
    // drift poll, or a dispatch that is waiting to provision a worktree.
    #[test]
    fn ahead_behind_bounds_the_rev_list_with_the_subprocess_timeout() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"0\t0\n")]);
        let _ = ahead_behind(REPO, BASE, &mock);
        assert_eq!(
            mock.recorded_timeouts(),
            vec![Some(SUBPROCESS_TIMEOUT)],
            "rev-list must be bounded like every other subprocess here"
        );
    }

    #[test]
    fn sync_repo_bounds_every_subprocess_that_can_block_on_the_network_or_a_lock() {
        let mock = MockProcessRunner::new(responses(vec![
            MockProcessRunner::ok(),                      // fetch
            MockProcessRunner::ok_with_stdout(b"3\t0\n"), // rev-list
            MockProcessRunner::ok(),                      // push
        ]));
        sync_repo(REPO, BASE, &mock).expect("an ahead repo pushes");
        // Every call, not an allowlist of three. This once named fetch/rev-list/push
        // because they were the only bounded ones; now that the preconditions are
        // bounded too, singling any out would imply the rest are exempt. The
        // push branch is this test's own — `sync_repo_bounds_every_subprocess_it_runs`
        // drives the behind/merge branch and never reaches a push.
        for ((program, args), timeout) in mock
            .recorded_calls()
            .into_iter()
            .zip(mock.recorded_timeouts())
        {
            assert!(
                timeout.is_some(),
                "{program} {args:?} must be bounded, but was run unbounded"
            );
        }
    }

    // ---------------------------------------------------------------------
    // fetch_base
    // ---------------------------------------------------------------------

    #[test]
    fn fetch_base_yields_no_error_on_success() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        assert_eq!(fetch_base(REPO, BASE, &mock), Ok(()));
    }

    #[test]
    fn fetch_base_yields_gits_message_on_failure() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail(
            "fatal: could not read from remote repository",
        )]);
        let err = fetch_base(REPO, BASE, &mock).expect_err("a failed fetch must report why");
        assert!(
            err.contains("could not read from remote repository"),
            "fetch error should carry git's own message, got: {err}"
        );
    }

    #[test]
    fn fetch_base_yields_an_error_when_the_runner_errors() {
        let mock = MockProcessRunner::new(vec![Err(anyhow::anyhow!("git not on PATH"))]);
        let err = fetch_base(REPO, BASE, &mock).expect_err("a spawn failure is a fetch failure");
        assert!(
            err.contains("git not on PATH"),
            "fetch error should carry the spawn failure, got: {err}"
        );
    }

    #[test]
    fn fetch_base_invokes_fetch_origin_base() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        let _ = fetch_base("/some/repo", "master", &mock);
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "git");
        assert_eq!(
            calls[0].1,
            argv(&["-C", "/some/repo", "fetch", "origin", "master"])
        );
    }

    // ---------------------------------------------------------------------
    // sync_repo — preconditions precede every write
    // ---------------------------------------------------------------------

    #[test]
    fn sync_repo_without_an_origin_remote_reports_no_remote_before_any_write() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("")]);
        let err = sync_repo(REPO, BASE, &mock).unwrap_err();
        assert!(
            matches!(err, SyncError::NoRemote),
            "a missing remote must be its own failure, got: {err}"
        );
        assert_eq!(
            mock.recorded_calls().len(),
            1,
            "nothing beyond the remote probe may run: {:?}",
            mock.recorded_calls()
        );
    }

    // PreconditionsPrecedeEveryWrite's stated carve-out: a probe that cannot be
    // run at all and one that reports no origin are the same fact *for this
    // operation* — there is nothing to sync against — so both report NoRemote
    // rather than splitting the first into Other. Pinned here so that giving
    // has_origin_remote a distinguishable failure channel (which other callers
    // do use) cannot silently change what sync reports.
    #[test]
    fn sync_repo_reports_no_remote_when_the_probe_cannot_be_run() {
        let mock = MockProcessRunner::new(vec![Err(anyhow::anyhow!("git not on PATH"))]);
        let err = sync_repo(REPO, BASE, &mock).unwrap_err();
        assert!(
            matches!(err, SyncError::NoRemote),
            "an unrunnable remote probe still means nothing to sync against, got: {err}"
        );
        assert_eq!(
            mock.recorded_calls().len(),
            1,
            "nothing beyond the remote probe may run: {:?}",
            mock.recorded_calls()
        );
    }

    #[test]
    fn sync_repo_on_the_wrong_branch_reports_not_on_base_branch_before_any_write() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"git@github.com:org/repo.git\n"),
            MockProcessRunner::ok_with_stdout(b"3783-local-first-repos\n"),
        ]);
        let err = sync_repo(REPO, BASE, &mock).unwrap_err();
        assert!(
            matches!(err, SyncError::NotOnBaseBranch { ref current, ref expected }
                if current == "3783-local-first-repos" && expected == "main"),
            "the branch found and the one expected must both be named, got: {err}"
        );
        assert_eq!(
            mock.recorded_calls().len(),
            2,
            "nothing beyond the branch check may run: {:?}",
            mock.recorded_calls()
        );
    }

    #[test]
    fn sync_repo_with_a_dirty_primary_worktree_reports_it_before_any_write() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"git@github.com:org/repo.git\n"),
            MockProcessRunner::ok_with_stdout(b"main\n"),
            MockProcessRunner::ok_with_stdout(b" M src/unrelated.rs\n?? scratch.txt\n"),
        ]);
        let err = sync_repo(REPO, BASE, &mock).unwrap_err();
        assert!(
            matches!(err, SyncError::DirtyPrimaryWorktree { ref path, ref files }
                if path == "/repo"
                && files == &["src/unrelated.rs".to_string(), "scratch.txt".to_string()]),
            "a dirty tree must be its own failure, naming the paths, got: {err}"
        );
        assert_eq!(
            mock.recorded_calls().len(),
            3,
            "no fetch, merge or push may run past a dirty tree: {:?}",
            mock.recorded_calls()
        );
    }

    // A dirty tree and a merge conflict are separate failures, never collapsed
    // into one another.
    #[test]
    fn sync_repo_never_reports_a_dirty_tree_as_a_conflict() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"git@github.com:org/repo.git\n"),
            MockProcessRunner::ok_with_stdout(b"main\n"),
            MockProcessRunner::ok_with_stdout(b"UU already-conflicted.rs\n"),
        ]);
        let err = sync_repo(REPO, BASE, &mock).unwrap_err();
        assert!(
            matches!(err, SyncError::DirtyPrimaryWorktree { .. }),
            "pre-existing unmerged paths are a dirty tree, not this sync's conflict, got: {err}"
        );
    }

    // ---------------------------------------------------------------------
    // sync_repo — FetchPrecedesCounting
    // ---------------------------------------------------------------------

    #[test]
    fn sync_repo_fetches_before_reading_the_counts() {
        let mock = MockProcessRunner::new(responses(vec![
            MockProcessRunner::ok(),                      // fetch
            MockProcessRunner::ok_with_stdout(b"0\t0\n"), // rev-list
        ]));
        sync_repo(REPO, BASE, &mock).expect("a level repo syncs cleanly");
        let calls = mock.recorded_calls();
        assert_eq!(
            calls[3].1,
            argv(&["-C", "/repo", "fetch", "origin", "main"]),
            "the fetch must run before the count: {calls:?}"
        );
        assert_eq!(
            calls[4].1,
            argv(&[
                "-C",
                "/repo",
                "rev-list",
                "--count",
                "--left-right",
                "main...origin/main",
            ]),
            "the count must be read from the refs the fetch just refreshed: {calls:?}"
        );
    }

    #[test]
    fn sync_repo_reports_a_failed_fetch_as_other() {
        let mock = MockProcessRunner::new(responses(vec![MockProcessRunner::fail(
            "fatal: could not read from remote repository",
        )]));
        let err = sync_repo(REPO, BASE, &mock).unwrap_err();
        assert!(
            matches!(err, SyncError::Other(ref m) if m.contains("could not read from remote")),
            "a failed fetch must carry git's message, got: {err}"
        );
        assert_eq!(
            mock.recorded_calls().len(),
            4,
            "counts must not be read against refs the fetch failed to refresh: {:?}",
            mock.recorded_calls()
        );
    }

    // ---------------------------------------------------------------------
    // sync_repo — AlreadyInSync performs no merge and no push
    // ---------------------------------------------------------------------

    #[test]
    fn sync_repo_already_in_sync_performs_no_merge_and_no_push() {
        let mock = MockProcessRunner::new(responses(vec![
            MockProcessRunner::ok(),                      // fetch
            MockProcessRunner::ok_with_stdout(b"0\t0\n"), // rev-list
        ]));
        assert_eq!(
            sync_repo(REPO, BASE, &mock).unwrap(),
            SyncOutcome::AlreadyInSync
        );
        let calls = mock.recorded_calls();
        assert_eq!(
            calls.len(),
            5,
            "the fetch and the count are the whole of an in-sync run: {calls:?}"
        );
        assert!(
            !calls
                .iter()
                .any(|(_, a)| a.contains(&"merge".to_string()) || a.contains(&"push".to_string())),
            "an in-sync repo must be neither merged nor pushed: {calls:?}"
        );
    }

    // An unmeasurable repo is not merged or pushed either — there is no count
    // to justify either write.
    #[test]
    fn sync_repo_unmeasurable_counts_perform_no_merge_and_no_push() {
        let mock = MockProcessRunner::new(responses(vec![
            MockProcessRunner::ok(),                     // fetch
            MockProcessRunner::fail("unknown revision"), // rev-list
        ]));
        assert_eq!(
            sync_repo(REPO, BASE, &mock).unwrap(),
            SyncOutcome::AlreadyInSync
        );
        let calls = mock.recorded_calls();
        assert!(
            !calls
                .iter()
                .any(|(_, a)| a.contains(&"merge".to_string()) || a.contains(&"push".to_string())),
            "an unmeasurable repo must be neither merged nor pushed: {calls:?}"
        );
    }

    // ---------------------------------------------------------------------
    // sync_repo — the four ahead/behind quadrants
    // ---------------------------------------------------------------------

    #[test]
    fn sync_repo_behind_only_merges_and_does_not_push() {
        let mock = MockProcessRunner::new(responses(vec![
            MockProcessRunner::ok(),                      // fetch
            MockProcessRunner::ok_with_stdout(b"0\t2\n"), // rev-list: behind 2
            MockProcessRunner::ok(),                      // merge (fast-forward)
            MockProcessRunner::ok_with_stdout(b"0\t0\n"), // recount after merge
        ]));
        assert_eq!(
            sync_repo(REPO, BASE, &mock).unwrap(),
            SyncOutcome::Synced {
                pulled: 2,
                pushed: 0
            }
        );
        let calls = mock.recorded_calls();
        assert_eq!(
            calls[5].1,
            argv(&["-C", "/repo", "merge", "--no-edit", "origin/main"]),
            "behind must be closed by merging origin/main: {calls:?}"
        );
        assert!(
            !calls.iter().any(|(_, a)| a.contains(&"push".to_string())),
            "a behind-only repo has nothing to push: {calls:?}"
        );
    }

    #[test]
    fn sync_repo_ahead_only_pushes_without_merging() {
        let mock = MockProcessRunner::new(responses(vec![
            MockProcessRunner::ok(),                      // fetch
            MockProcessRunner::ok_with_stdout(b"3\t0\n"), // rev-list: ahead 3
            MockProcessRunner::ok(),                      // push
        ]));
        assert_eq!(
            sync_repo(REPO, BASE, &mock).unwrap(),
            SyncOutcome::Synced {
                pulled: 0,
                pushed: 3
            }
        );
        let calls = mock.recorded_calls();
        assert!(
            !calls.iter().any(|(_, a)| a.contains(&"merge".to_string())),
            "an ahead-only repo has nothing to merge: {calls:?}"
        );
        assert_eq!(
            calls[5].1,
            argv(&["-C", "/repo", "push", "origin", "main"]),
            "ahead must be closed by pushing to origin/main: {calls:?}"
        );
    }

    // Diverged: the merge commit is itself something to publish, so the pushed
    // figure is the count re-read after the merge, not the one that decided it.
    #[test]
    fn sync_repo_diverged_merges_then_pushes_the_recounted_ahead() {
        let mock = MockProcessRunner::new(responses(vec![
            MockProcessRunner::ok(),                      // fetch
            MockProcessRunner::ok_with_stdout(b"3\t1\n"), // rev-list: diverged
            MockProcessRunner::ok(),                      // merge (merge commit)
            MockProcessRunner::ok_with_stdout(b"4\t0\n"), // recount: 3 + the merge commit
            MockProcessRunner::ok(),                      // push
        ]));
        assert_eq!(
            sync_repo(REPO, BASE, &mock).unwrap(),
            SyncOutcome::Synced {
                pulled: 1,
                pushed: 4
            }
        );
        let calls = mock.recorded_calls();
        assert_eq!(
            calls[5].1,
            argv(&["-C", "/repo", "merge", "--no-edit", "origin/main"]),
            "the merge must precede the push: {calls:?}"
        );
        assert_eq!(
            calls[7].1,
            argv(&["-C", "/repo", "push", "origin", "main"]),
            "the push must follow the recount: {calls:?}"
        );
    }

    #[test]
    fn sync_repo_skips_the_push_when_the_recount_after_merging_is_unmeasurable() {
        let mock = MockProcessRunner::new(responses(vec![
            MockProcessRunner::ok(),                      // fetch
            MockProcessRunner::ok_with_stdout(b"3\t1\n"), // rev-list: diverged
            MockProcessRunner::ok(),                      // merge
            MockProcessRunner::fail("unknown revision"),  // recount fails
        ]));
        assert_eq!(
            sync_repo(REPO, BASE, &mock).unwrap(),
            SyncOutcome::Synced {
                pulled: 1,
                pushed: 0
            }
        );
        assert!(
            !mock
                .recorded_calls()
                .iter()
                .any(|(_, a)| a.contains(&"push".to_string())),
            "a count we cannot justify must not be pushed: {:?}",
            mock.recorded_calls()
        );
    }

    // ---------------------------------------------------------------------
    // sync_repo — merge conflict
    // ---------------------------------------------------------------------

    #[test]
    fn sync_repo_merge_conflict_reads_the_files_before_aborting() {
        let mock = MockProcessRunner::new(responses(vec![
            MockProcessRunner::ok(),                      // fetch
            MockProcessRunner::ok_with_stdout(b"3\t1\n"), // rev-list: diverged
            MockProcessRunner::fail("CONFLICT (content): Merge conflict in lib.rs"), // merge
            MockProcessRunner::ok_with_stdout(b"UU lib.rs\nAA added.rs\n"), // status, mid-merge
            MockProcessRunner::ok(),                      // merge --abort
        ]));
        let err = sync_repo(REPO, BASE, &mock).unwrap_err();
        assert!(
            matches!(err, SyncError::MergeConflict { ref files }
                if files == &["lib.rs".to_string(), "added.rs".to_string()]),
            "a conflict must name the unmerged paths, got: {err}"
        );
        assert!(
            err.to_string().contains("lib.rs"),
            "the message must name the conflicted files, got: {err}"
        );
        assert!(!err.retryable(), "a conflict needs a human, not a retry");

        let calls = mock.recorded_calls();
        assert_eq!(
            calls[6].1,
            argv(&["-C", "/repo", "status", "--porcelain"]),
            "the unmerged paths must be read while the merge is still mid-flight: {calls:?}"
        );
        assert_eq!(
            calls[7].1,
            argv(&["-C", "/repo", "merge", "--abort"]),
            "the abort must come after the paths are captured: {calls:?}"
        );
        assert_eq!(
            calls.len(),
            8,
            "an aborted merge must not go on to push: {calls:?}"
        );
    }

    // A merge that fails with nothing unmerged is not a conflict — it gets its
    // own generic failure rather than an empty conflict list.
    #[test]
    fn sync_repo_merge_failure_without_unmerged_paths_reports_other() {
        let mock = MockProcessRunner::new(responses(vec![
            MockProcessRunner::ok(),                      // fetch
            MockProcessRunner::ok_with_stdout(b"3\t1\n"), // rev-list: diverged
            MockProcessRunner::fail("fatal: refusing to merge unrelated histories"), // merge
            MockProcessRunner::ok_with_stdout(b""),       // status: nothing unmerged
            MockProcessRunner::ok(),                      // merge --abort
        ]));
        let err = sync_repo(REPO, BASE, &mock).unwrap_err();
        assert!(
            matches!(err, SyncError::Other(ref m) if m.contains("unrelated histories")),
            "a non-conflict merge failure must not masquerade as a conflict, got: {err}"
        );
    }

    // ---------------------------------------------------------------------
    // sync_repo — push rejected
    // ---------------------------------------------------------------------

    #[test]
    fn sync_repo_push_rejection_is_reported_as_retryable() {
        let mock = MockProcessRunner::new(responses(vec![
            MockProcessRunner::ok(),                                            // fetch
            MockProcessRunner::ok_with_stdout(b"3\t0\n"),                       // rev-list: ahead 3
            MockProcessRunner::fail("! [rejected] main -> main (fetch first)"), // push
        ]));
        let err = sync_repo(REPO, BASE, &mock).unwrap_err();
        assert!(
            matches!(err, SyncError::PushRejected { ref stderr } if stderr.contains("rejected")),
            "a rejected push must carry git's own output, got: {err}"
        );
        assert!(
            err.retryable(),
            "origin moving between fetch and push is retryable as-is"
        );
    }

    #[test]
    fn sync_error_is_retryable_only_for_a_rejected_push() {
        let not_retryable = [
            SyncError::NoRemote,
            SyncError::NotOnBaseBranch {
                current: "feature".to_string(),
                expected: "main".to_string(),
            },
            SyncError::DirtyPrimaryWorktree {
                path: "/repo".to_string(),
                files: vec!["a.rs".to_string()],
            },
            SyncError::MergeConflict {
                files: vec!["a.rs".to_string()],
            },
            SyncError::Other("boom".to_string()),
        ];
        for err in not_retryable {
            assert!(
                !err.retryable(),
                "{err} needs the operator to change something first, so it is not retryable"
            );
        }
        assert!(SyncError::PushRejected {
            stderr: String::new()
        }
        .retryable());
    }

    // ---------------------------------------------------------------------
    // sync_repo — LocalBaseHistoryIsNeverRewritten
    // ---------------------------------------------------------------------

    #[test]
    fn sync_repo_never_rewrites_local_base_history() {
        let mock = MockProcessRunner::new(responses(vec![
            MockProcessRunner::ok(),                      // fetch
            MockProcessRunner::ok_with_stdout(b"3\t1\n"), // rev-list: diverged
            MockProcessRunner::ok(),                      // merge
            MockProcessRunner::ok_with_stdout(b"4\t0\n"), // recount
            MockProcessRunner::ok(),                      // push
        ]));
        sync_repo(REPO, BASE, &mock).expect("a diverged repo syncs by merging");
        for (program, args) in mock.recorded_calls() {
            for forbidden in ["rebase", "reset", "--force", "--force-with-lease", "-f"] {
                assert!(
                    !args.iter().any(|a| a == forbidden),
                    "sync must never rewrite local history, but ran: {program} {args:?}"
                );
            }
        }
    }

    // ---------------------------------------------------------------------
    // RepoSyncState — fields, optional fields, derived values
    // ---------------------------------------------------------------------

    fn measured_state(ahead: u32, behind: u32) -> RepoSyncState {
        RepoSyncState {
            repo_path: REPO.to_string(),
            base_branch: BASE.to_string(),
            counts: Some(AheadBehind { ahead, behind }),
            last_fetch_error: None,
        }
    }

    fn unmeasured_state() -> RepoSyncState {
        RepoSyncState {
            repo_path: REPO.to_string(),
            base_branch: BASE.to_string(),
            counts: None,
            last_fetch_error: None,
        }
    }

    #[test]
    fn repo_sync_state_carries_every_declared_field() {
        let state = RepoSyncState {
            repo_path: REPO.to_string(),
            base_branch: "master".to_string(),
            counts: Some(AheadBehind {
                ahead: 3,
                behind: 1,
            }),
            last_fetch_error: Some("offline".to_string()),
        };
        assert_eq!(state.repo_path, REPO);
        assert_eq!(state.base_branch, "master");
        assert_eq!(
            state.counts,
            Some(AheadBehind {
                ahead: 3,
                behind: 1
            })
        );
        assert_eq!(state.last_fetch_error.as_deref(), Some("offline"));
    }

    // entity-optional.RepoSyncState.counts / .last_fetch_error
    #[test]
    fn repo_sync_state_optional_fields_accept_null_and_non_null() {
        let mut state = unmeasured_state();
        assert_eq!(state.counts, None);
        assert_eq!(state.last_fetch_error, None);
        state.counts = Some(AheadBehind {
            ahead: 0,
            behind: 0,
        });
        state.last_fetch_error = Some("boom".to_string());
        assert!(state.counts.is_some());
        assert!(state.last_fetch_error.is_some());
    }

    // derived.RepoSyncState.is_measured
    #[test]
    fn repo_sync_state_is_measured_only_with_counts() {
        assert!(measured_state(0, 0).is_measured());
        assert!(!unmeasured_state().is_measured());
    }

    // derived.RepoSyncState.has_drift — false both when clean and when
    // unmeasured (UnmeasuredIsNeverPresentedAsClean).
    #[test]
    fn repo_sync_state_has_drift_is_false_when_clean_and_when_unmeasured() {
        assert!(!measured_state(0, 0).has_drift());
        assert!(!unmeasured_state().has_drift());
        assert!(measured_state(1, 0).has_drift());
        assert!(measured_state(0, 1).has_drift());
        assert!(measured_state(2, 3).has_drift());
    }

    // invariant.RepoSyncState.CountsAreNonNegative — the counts are unsigned, so
    // a negative count is unrepresentable rather than merely unexpected.
    #[test]
    fn repo_sync_state_counts_are_non_negative_by_construction() {
        let state = measured_state(7, 5);
        let counts = state.counts.expect("measured");
        // Widened to a signed type so the assertion is a real comparison rather
        // than one the unsigned range makes vacuously true.
        assert!(i64::from(counts.ahead) >= 0 && i64::from(counts.behind) >= 0);
    }

    // ---------------------------------------------------------------------
    // RepoSyncCache — UniqueMeasurementPerRepo
    // ---------------------------------------------------------------------

    fn measurement(
        repo: &str,
        counts: Option<AheadBehind>,
        err: Option<&str>,
    ) -> RepoSyncMeasurement {
        RepoSyncMeasurement {
            repo_path: repo.to_string(),
            base_branch: BASE.to_string(),
            counts,
            fetch_error: err.map(str::to_string),
        }
    }

    // invariant.RepoSyncState.UniqueMeasurementPerRepo: repeated refreshes of one
    // repo replace the measurement rather than accumulating a second one.
    #[test]
    fn cache_keeps_at_most_one_measurement_per_repo() {
        let mut cache = RepoSyncCache::default();
        cache.apply(measurement(
            REPO,
            Some(AheadBehind {
                ahead: 1,
                behind: 0,
            }),
            None,
        ));
        cache.apply(measurement(
            REPO,
            Some(AheadBehind {
                ahead: 2,
                behind: 0,
            }),
            None,
        ));
        cache.apply(measurement(
            "/other",
            Some(AheadBehind {
                ahead: 9,
                behind: 9,
            }),
            None,
        ));
        assert_eq!(cache.len(), 2, "one entry per repo_path, not per refresh");
        assert_eq!(
            cache.get(REPO).and_then(|s| s.counts),
            Some(AheadBehind {
                ahead: 2,
                behind: 0
            }),
            "the later measurement wins"
        );
    }

    #[test]
    fn cache_has_no_state_for_an_unrefreshed_repo() {
        let cache = RepoSyncCache::default();
        assert!(cache.get(REPO).is_none());
    }

    // rule-success.RefreshRepoSyncState — first observation creates the state
    // with the counts that were read.
    #[test]
    fn cache_creates_state_from_a_first_successful_measurement() {
        let mut cache = RepoSyncCache::default();
        cache.apply(measurement(
            REPO,
            Some(AheadBehind {
                ahead: 3,
                behind: 1,
            }),
            None,
        ));
        let state = cache.get(REPO).expect("state created");
        assert_eq!(state.repo_path, REPO);
        assert_eq!(state.base_branch, BASE);
        assert_eq!(
            state.counts,
            Some(AheadBehind {
                ahead: 3,
                behind: 1
            })
        );
        assert_eq!(state.last_fetch_error, None);
    }

    // A first observation whose fetch failed creates an *unmeasured* state
    // carrying the error — never a zero/zero pair.
    #[test]
    fn cache_creates_unmeasured_state_when_the_first_fetch_failed() {
        let mut cache = RepoSyncCache::default();
        cache.apply(measurement(REPO, None, Some("offline")));
        let state = cache.get(REPO).expect("state created");
        assert_eq!(state.counts, None);
        assert!(!state.is_measured());
        assert!(!state.has_drift());
        assert_eq!(state.last_fetch_error.as_deref(), Some("offline"));
    }

    // RefreshRepoSyncState's `else if fetch_error != null` branch: previously
    // known counts survive a failed fetch, so a slow or offline network leaves
    // the drift indicator undisturbed rather than blanking it.
    #[test]
    fn cache_keeps_previous_counts_when_a_later_fetch_fails() {
        let mut cache = RepoSyncCache::default();
        cache.apply(measurement(
            REPO,
            Some(AheadBehind {
                ahead: 3,
                behind: 1,
            }),
            None,
        ));
        cache.apply(measurement(REPO, None, Some("offline")));
        let state = cache.get(REPO).expect("state still present");
        assert_eq!(
            state.counts,
            Some(AheadBehind {
                ahead: 3,
                behind: 1
            }),
            "a failed fetch records only the error"
        );
        assert_eq!(state.last_fetch_error.as_deref(), Some("offline"));
    }

    // The success branch clears a stale fetch error and re-reads base_branch.
    #[test]
    fn cache_clears_the_fetch_error_once_a_fetch_succeeds() {
        let mut cache = RepoSyncCache::default();
        cache.apply(measurement(REPO, None, Some("offline")));
        let mut ok = measurement(
            REPO,
            Some(AheadBehind {
                ahead: 0,
                behind: 2,
            }),
            None,
        );
        ok.base_branch = "master".to_string();
        cache.apply(ok);
        let state = cache.get(REPO).expect("state present");
        assert_eq!(state.last_fetch_error, None);
        assert_eq!(state.base_branch, "master");
        assert_eq!(
            state.counts,
            Some(AheadBehind {
                ahead: 0,
                behind: 2
            })
        );
    }

    // A successful fetch that still cannot be counted blanks the counts back to
    // unknown rather than leaving a stale measurement in place.
    #[test]
    fn cache_marks_unmeasured_when_a_successful_fetch_cannot_be_counted() {
        let mut cache = RepoSyncCache::default();
        cache.apply(measurement(
            REPO,
            Some(AheadBehind {
                ahead: 3,
                behind: 1,
            }),
            None,
        ));
        cache.apply(measurement(REPO, None, None));
        let state = cache.get(REPO).expect("state present");
        assert_eq!(state.counts, None);
        assert!(!state.is_measured());
    }

    // ---------------------------------------------------------------------
    // measure_repo — RefreshRepoSyncState's measurement half
    // ---------------------------------------------------------------------

    // BaseBranchIsTheRepositoryDefault: the branch measured is the repository's
    // own origin/HEAD, so a `master` repository works unchanged.
    #[test]
    fn measure_repo_uses_the_repositorys_own_default_branch() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/master\n"), // symbolic-ref
            MockProcessRunner::ok(),                                            // fetch
            MockProcessRunner::ok_with_stdout(b"2\t0\n"),                       // rev-list
        ]);
        let m = measure_repo(REPO, true, &mock);
        assert_eq!(m.base_branch, "master");
        assert!(mock
            .recorded_calls()
            .iter()
            .any(|(_, args)| args.contains(&"master...origin/master".to_string())));
    }

    // FetchPrecedesCounting: the fetch is issued before the rev-list it makes
    // trustworthy.
    #[test]
    fn measure_repo_fetches_before_counting() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/main\n"),
            MockProcessRunner::ok(),
            MockProcessRunner::ok_with_stdout(b"3\t1\n"),
        ]);
        let m = measure_repo(REPO, true, &mock);
        assert_eq!(
            m.counts,
            Some(AheadBehind {
                ahead: 3,
                behind: 1
            })
        );
        assert_eq!(m.fetch_error, None);
        let calls = mock.recorded_calls();
        let fetch_at = calls
            .iter()
            .position(|(_, a)| a.contains(&"fetch".to_string()))
            .expect("a fetching refresh fetches");
        let count_at = calls
            .iter()
            .position(|(_, a)| a.contains(&"rev-list".to_string()))
            .expect("counts are read");
        assert!(fetch_at < count_at, "counts must follow the fetch");
    }

    // fetch_first = false is a pure local ref read: no network call at all.
    #[test]
    fn measure_repo_skips_the_fetch_when_not_asked_to_fetch() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/main\n"),
            MockProcessRunner::ok_with_stdout(b"0\t2\n"),
        ]);
        let m = measure_repo(REPO, false, &mock);
        assert_eq!(
            m.counts,
            Some(AheadBehind {
                ahead: 0,
                behind: 2
            })
        );
        assert_eq!(m.fetch_error, None);
        assert!(
            !mock
                .recorded_calls()
                .iter()
                .any(|(_, a)| a.contains(&"fetch".to_string())),
            "a non-fetching refresh must not touch the network"
        );
    }

    // A failed fetch yields the error and NO counts — counting against refs the
    // refresh failed to update is what FetchPrecedesCounting forbids.
    #[test]
    fn measure_repo_reports_the_fetch_error_and_reads_no_counts() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/main\n"),
            MockProcessRunner::fail("could not resolve host github.com"),
        ]);
        let m = measure_repo(REPO, true, &mock);
        assert_eq!(m.counts, None);
        let err = m.fetch_error.expect("a failed fetch is reported");
        assert!(
            err.contains("could not resolve host"),
            "expected git's own message, got: {err}"
        );
        assert!(
            !mock
                .recorded_calls()
                .iter()
                .any(|(_, a)| a.contains(&"rev-list".to_string())),
            "no counts may be read after a failed fetch"
        );
    }

    // UnmeasurableIsNotInSync, at the measurement boundary.
    #[test]
    fn measure_repo_yields_no_counts_for_an_unmeasurable_repo() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/main\n"),
            MockProcessRunner::ok(),
            MockProcessRunner::fail("fatal: unknown revision"),
        ]);
        let m = measure_repo(REPO, true, &mock);
        assert_eq!(m.counts, None);
        assert_eq!(m.fetch_error, None, "the fetch itself succeeded");
    }

    #[test]
    fn measure_repo_names_the_repository_it_measured() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/main\n"),
            MockProcessRunner::ok_with_stdout(b"0\t0\n"),
        ]);
        assert_eq!(measure_repo(REPO, false, &mock).repo_path, REPO);
    }

    // RepoStatusCli @guarantee ReadOnly: measuring never merges or pushes.
    #[test]
    fn measure_repo_never_merges_or_pushes() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/main\n"),
            MockProcessRunner::ok(),
            MockProcessRunner::ok_with_stdout(b"3\t1\n"),
        ]);
        measure_repo(REPO, true, &mock);
        for (program, args) in mock.recorded_calls() {
            for forbidden in ["merge", "push", "rebase", "reset"] {
                assert!(
                    !args.iter().any(|a| a == forbidden),
                    "measuring must be read-only, but ran: {program} {args:?}"
                );
            }
        }
    }

    // Every git invocation targets the repo root and nothing else: sync acts on
    // the primary checkout, never on a task worktree.
    #[test]
    fn sync_repo_targets_only_the_primary_checkout() {
        let mock = MockProcessRunner::new(responses(vec![
            MockProcessRunner::ok(),                      // fetch
            MockProcessRunner::ok_with_stdout(b"0\t2\n"), // rev-list: behind 2
            MockProcessRunner::ok(),                      // merge
            MockProcessRunner::ok_with_stdout(b"0\t0\n"), // recount
        ]));
        sync_repo(REPO, BASE, &mock).expect("a behind repo fast-forwards");
        for (program, args) in mock.recorded_calls() {
            assert_eq!(program, "git", "sync shells out to git and nothing else");
            assert_eq!(
                args.first().map(String::as_str),
                Some("-C"),
                "every call must be scoped with -C: {args:?}"
            );
            assert_eq!(
                args.get(1).map(String::as_str),
                Some("/repo"),
                "every call must target the repo root: {args:?}"
            );
        }
    }

    // repo-sync.allium's engine guidance claims every subprocess it issues is
    // bounded by a timeout. This is the test that makes the claim true rather than
    // aspirational, and it covers the preflight reads reached through `crate::git`
    // as well as the engine's own calls.
    #[test]
    fn sync_repo_bounds_every_subprocess_it_runs() {
        let mock = MockProcessRunner::new(responses(vec![
            MockProcessRunner::ok(),                      // fetch
            MockProcessRunner::ok_with_stdout(b"0\t2\n"), // rev-list: behind 2
            MockProcessRunner::ok(),                      // merge
            MockProcessRunner::ok_with_stdout(b"0\t0\n"), // recount
        ]));

        sync_repo(REPO, BASE, &mock).expect("a behind repo fast-forwards");

        let timeouts = mock.recorded_timeouts();
        assert_eq!(
            timeouts.len(),
            7,
            "expected 7 subprocesses on this path (3 preconditions + fetch + rev-list + merge + recount), got {}: {:?}",
            timeouts.len(),
            mock.recorded_calls()
        );
        assert!(
            timeouts.iter().all(|t| *t == Some(SUBPROCESS_TIMEOUT)),
            "every subprocess on the sync path must be bounded, got: {timeouts:?}"
        );
    }

    // The happy path never reaches the conflict branch, so the porcelain read and
    // the merge abort need their own gate — the same split as finish_task's
    // conflict path.
    #[test]
    fn sync_repo_bounds_the_conflict_abort_path() {
        let mock = MockProcessRunner::new(responses(vec![
            MockProcessRunner::ok(),                           // fetch
            MockProcessRunner::ok_with_stdout(b"0\t2\n"),      // rev-list: behind 2
            MockProcessRunner::fail("CONFLICT"),               // merge fails
            MockProcessRunner::ok_with_stdout(b"UU lib.rs\n"), // status --porcelain
            MockProcessRunner::ok(),                           // merge --abort
        ]));

        let err = sync_repo(REPO, BASE, &mock).expect_err("a conflicted merge fails");
        assert!(
            matches!(err, SyncError::MergeConflict { .. }),
            "expected a merge conflict, got: {err}"
        );

        let timeouts = mock.recorded_timeouts();
        assert_eq!(
            timeouts.len(),
            8,
            "expected 8 subprocesses on this path (3 preconditions + fetch + rev-list + merge + status read + abort), got {}: {:?}",
            timeouts.len(),
            mock.recorded_calls()
        );
        assert!(
            timeouts.iter().all(|t| *t == Some(SUBPROCESS_TIMEOUT)),
            "the conflict read and the abort must be bounded too, got: {timeouts:?}"
        );
    }
}
