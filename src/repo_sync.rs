//! Local-first repo sync: keeping a repository's primary checkout in step with
//! origin on its own default branch.
//!
//! Spec: `docs/specs/repo-sync.allium` (the `RepoSyncEngine` contract and the
//! `SyncRepo` rule). Structured like [`crate::dispatch::finish`] — synchronous,
//! [`ProcessRunner`]-driven, with no TUI or database coupling, so the same three
//! operations back the board action and the CLI.

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
pub fn ahead_behind(
    repo_path: &str,
    base_branch: &str,
    runner: &dyn ProcessRunner,
) -> Option<AheadBehind> {
    let repo_path = expand_tilde(repo_path);
    let output = runner
        .run(
            "git",
            &[
                "-C",
                &repo_path,
                "rev-list",
                "--count",
                "--left-right",
                &count_range(base_branch),
            ],
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

    // 1. An origin remote must exist. Both a spawn failure and a non-zero exit
    //    mean the same thing here: nothing to sync against.
    let has_remote = runner
        .run("git", &["-C", &repo, "remote", "get-url", "origin"])
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !has_remote {
        return Err(SyncError::NoRemote);
    }

    // 2. The checkout must be on the base branch — the merge and the push both
    //    act on whatever is checked out.
    let output = runner
        .run("git", &["-C", &repo, "rev-parse", "--abbrev-ref", "HEAD"])
        .map_err(|e| SyncError::Other(format!("Failed to check current branch: {e}")))?;
    let current = stdout_str(&output);
    if current != base_branch {
        return Err(SyncError::NotOnBaseBranch {
            current,
            expected: base_branch.to_string(),
        });
    }

    // 3. The checkout must be clean — merging into a dirty tree loses work.
    let output = runner
        .run("git", &["-C", &repo, "status", "--porcelain"])
        .map_err(|e| SyncError::Other(format!("Failed to check working tree status: {e}")))?;
    let dirty = crate::git::parse_porcelain_files(&output);
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
            .run(
                "git",
                &[
                    "-C",
                    &repo,
                    "merge",
                    "--no-edit",
                    &format!("origin/{base_branch}"),
                ],
            )
            .map_err(|e| SyncError::Other(format!("Failed to run git merge: {e}")))?;
        if !output.status.success() {
            // Read the conflicted paths from the repo's own status *before*
            // aborting — the abort clears them
            // (`ConflictFilesCapturedBeforeAbort`).
            let conflicted = runner
                .run("git", &["-C", &repo, "status", "--porcelain"])
                .map(|o| crate::git::parse_unmerged_files(&o))
                .unwrap_or_default();
            let _ = runner.run("git", &["-C", &repo, "merge", "--abort"]);
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
}
