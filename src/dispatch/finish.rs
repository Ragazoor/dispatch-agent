use std::time::Duration;

use crate::git::parse_unmerged_files;
use crate::models::expand_tilde;
use crate::process::ProcessRunner;

use super::git_output::is_rebase_conflict;
use super::{stderr_str, stdout_str};

/// Errors from the finish (rebase + cleanup) operation.
#[derive(Debug)]
pub enum FinishError {
    NotOnDefaultBranch {
        current: String,
        expected: String,
    },
    /// The primary worktree (repo root) has uncommitted changes. Detected as
    /// a preflight check before any pull/rebase/merge is attempted, so it
    /// never masquerades as a rebase conflict.
    DirtyPrimaryWorktree {
        path: String,
        files: Vec<String>,
    },
    RebaseConflict {
        branch: String,
        files: Vec<String>,
    },
    Other(String),
}

impl std::fmt::Display for FinishError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FinishError::NotOnDefaultBranch { current, expected } => write!(
                f,
                "Repo root is not on {expected} (currently on {current}) — checkout {expected} first"
            ),
            FinishError::DirtyPrimaryWorktree { path, files } => write!(
                f,
                "Primary worktree at {path} has uncommitted changes ({}) — commit or stash them before wrap_up can rebase",
                files.join(", ")
            ),
            FinishError::RebaseConflict { branch, files } => {
                let location = if files.is_empty() {
                    String::new()
                } else {
                    format!(" in {}", files.join(", "))
                };
                write!(f, "Rebase conflict on {branch}{location} — resolve and try again")
            }
            FinishError::Other(msg) => write!(f, "{msg}"),
        }
    }
}

/// The git-orchestration inputs for [`finish_task`], grouped to avoid a long
/// list of same-typed positional `&str` arguments that are easy to transpose.
pub struct FinishContext<'a> {
    /// Repo root path (where the base branch is checked out).
    pub repo_path: &'a str,
    /// The task's worktree path (where the task branch is checked out).
    pub worktree: &'a str,
    /// The task branch to rebase and fast-forward onto `base_branch`.
    pub branch: &'a str,
    /// The branch the repo root must be on; rebase/fast-forward target.
    pub base_branch: &'a str,
    /// Bound for every git subprocess `finish_task` issues. Use
    /// [`crate::process::SUBPROCESS_TIMEOUT`] in production; pass a short
    /// duration in tests, mirroring `provision_worktree` in
    /// [`crate::dispatch::worktree`]. Without this seam a test proving the
    /// bound exists would have to wait out the real 60s bound to go red.
    pub timeout: Duration,
}

/// Rebase the task branch onto `base_branch` and fast-forward it. The git half
/// of a finish and nothing else: no tmux teardown, no task write.
///
/// Killing the session is the caller's job, and deliberately so — the caller
/// gates the teardown on the task's terminal write landing first, so a task
/// whose Done write failed keeps its live window (`ExitSession` in
/// `docs/specs/pr-workflow.allium`). The worktree is preserved
/// — it will be cleaned up when the task is archived.
pub fn finish_task(
    ctx: &FinishContext,
    runner: &dyn ProcessRunner,
) -> std::result::Result<(), FinishError> {
    let FinishContext {
        repo_path,
        worktree,
        branch,
        base_branch,
        timeout,
    } = *ctx;
    let repo_path = &expand_tilde(repo_path);
    let worktree = &expand_tilde(worktree);

    // 1. Verify we're on the base branch
    let current_branch =
        crate::git::current_branch(repo_path, runner).map_err(FinishError::Other)?;
    if current_branch != base_branch {
        return Err(FinishError::NotOnDefaultBranch {
            current: current_branch,
            expected: base_branch.to_string(),
        });
    }

    // 2. Check the primary worktree (repo root) is clean before touching it.
    let dirty_files = crate::git::dirty_files(repo_path, runner).map_err(FinishError::Other)?;
    if !dirty_files.is_empty() {
        return Err(FinishError::DirtyPrimaryWorktree {
            path: repo_path.to_string(),
            files: dirty_files,
        });
    }

    // 3. Pull latest base branch (skip if no remote configured). A probe that
    //    could not be run at all is *not* "no remote": it means git could not be
    //    spawned, which is a failure worth naming rather than a licence to skip
    //    the pull and rebase anyway.
    let has_remote =
        crate::git::has_origin_remote(repo_path, runner).map_err(FinishError::Other)?;

    if has_remote {
        let output = runner
            .run_with_timeout(
                "git",
                &[
                    "-C",
                    repo_path,
                    "pull",
                    "--no-rebase",
                    "origin",
                    base_branch,
                ],
                timeout,
            )
            .map_err(|e| FinishError::Other(format!("Failed to pull: {e}")))?;
        if !output.status.success() {
            return Err(FinishError::Other(format!(
                "Failed to pull {base_branch}: {}",
                stderr_str(&output)
            )));
        }
    }

    // 4. Rebase branch onto base branch (from worktree, where branch is checked out)
    let output = runner
        .run_with_timeout("git", &["-C", worktree, "rebase", base_branch], timeout)
        .map_err(|e| FinishError::Other(format!("Failed to run git rebase: {e}")))?;
    if !output.status.success() {
        let stderr = stderr_str(&output);
        let stdout = stdout_str(&output);
        let is_conflict = is_rebase_conflict(&stdout, &stderr);

        // Read the conflicted file(s) out of the worktree's own status
        // while the rebase is still mid-flight — `rebase --abort` below
        // clears this state, so it must be gathered first.
        let conflicted_files = if is_conflict {
            runner
                .run_with_timeout(
                    "git",
                    &["-C", worktree, "status", "--porcelain"],
                    timeout,
                )
                .map(|o| parse_unmerged_files(&o))
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        let _ = runner.run_with_timeout(
            "git",
            &["-C", worktree, "rebase", "--abort"],
            timeout,
        );

        if is_conflict {
            return Err(FinishError::RebaseConflict {
                branch: branch.to_string(),
                files: conflicted_files,
            });
        }
        return Err(FinishError::Other(format!("Rebase failed: {}", stderr)));
    }

    // 5. Fast-forward base branch to the rebased branch
    let output = runner
        .run_with_timeout(
            "git",
            &["-C", repo_path, "merge", "--ff-only", branch],
            timeout,
        )
        .map_err(|e| FinishError::Other(format!("Failed to fast-forward {base_branch}: {e}")))?;
    if !output.status.success() {
        return Err(FinishError::Other(format!(
            "Fast-forward failed after rebase: {}",
            stderr_str(&output)
        )));
    }

    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::process::MockProcessRunner;
    use std::process::Output;

    fn exit_fail() -> std::process::ExitStatus {
        // UNIX only, but tests only run on Linux/macOS anyway.
        use std::os::unix::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(1)
    }

    /// A bound short enough that no test ever waits on it.
    /// `MockProcessRunner::run_with_timeout` bails *without* sleeping once a
    /// scripted delay reaches the timeout, so a bounded call is instant; and on
    /// the unbounded path — which does sleep — 50ms is unnoticeable.
    const TEST_TIMEOUT: Duration = Duration::from_millis(50);

    /// Build a `FinishContext` with the standard test repo/worktree/branch,
    /// varying only the base branch the individual tests care about.
    fn fctx(base_branch: &str) -> FinishContext<'_> {
        FinishContext {
            repo_path: "/repo",
            worktree: "/repo/.worktrees/42-fix-bug",
            branch: "42-fix-bug",
            base_branch,
            timeout: TEST_TIMEOUT,
        }
    }

    // finish_task is the git half only: a successful rebase + fast-forward
    // issues no tmux call at all. The teardown belongs to the caller, gated on
    // the task's terminal write landing (ExitSession in
    // docs/specs/pr-workflow.allium).
    #[test]
    fn finish_task_issues_no_tmux_command() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"main\n"), // rev-parse HEAD
            MockProcessRunner::ok_with_stdout(b""),       // status --porcelain (clean)
            MockProcessRunner::fail(""),                  // remote get-url (no remote)
            MockProcessRunner::ok(),                      // git rebase main
            MockProcessRunner::ok(),                      // git merge --ff-only
        ]);

        finish_task(&fctx("main"), &mock).expect("rebase + fast-forward succeeds");

        assert!(
            mock.recorded_calls()
                .iter()
                .all(|(program, _)| program != "tmux"),
            "finish_task must not touch tmux: {:?}",
            mock.recorded_calls()
        );
    }

    // Pull runner returns Err (process could not be spawned) rather than a
    // non-zero exit — maps to FinishError::Other via map_err.
    #[test]
    fn finish_task_pull_runner_error_returns_other() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"main\n"), // rev-parse HEAD
            MockProcessRunner::ok_with_stdout(b""),       // status --porcelain (clean)
            MockProcessRunner::ok_with_stdout(b"git@github.com:org/repo.git\n"), // remote get-url
            Err(anyhow::anyhow!("git: command not found")), // git pull
        ]);

        let err = finish_task(&fctx("main"), &mock).unwrap_err();

        assert!(
            matches!(err, FinishError::Other(ref m) if m.contains("Failed to pull")),
            "pull runner error should map to FinishError::Other, got: {err}"
        );
    }

    // FF-only runner returns Err (process could not be spawned) — maps to
    // FinishError::Other via map_err with "Failed to fast-forward" prefix.
    #[test]
    fn finish_task_ff_only_runner_error_returns_other() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"main\n"), // rev-parse HEAD
            MockProcessRunner::ok_with_stdout(b""),       // status --porcelain (clean)
            MockProcessRunner::fail(""),                  // remote get-url (no remote)
            MockProcessRunner::ok(),                      // git rebase
            Err(anyhow::anyhow!("git: command not found")), // git merge --ff-only
        ]);

        let err = finish_task(&fctx("main"), &mock).unwrap_err();

        assert!(
            matches!(err, FinishError::Other(ref m) if m.contains("Failed to fast-forward")),
            "ff-only runner error should map to FinishError::Other, got: {err}"
        );
    }

    // A remote probe that could not be *run* is a git failure, not a finding
    // that there is no remote — so it stops the operation rather than silently
    // skipping the pull and rebasing against a base that was never refreshed.
    #[test]
    fn finish_task_reports_a_remote_probe_that_could_not_be_run() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"main\n"), // rev-parse HEAD
            MockProcessRunner::ok_with_stdout(b""),       // status --porcelain (clean)
            Err(anyhow::anyhow!("git: command not found")), // remote get-url
        ]);

        let err = finish_task(&fctx("main"), &mock).unwrap_err();

        assert!(
            matches!(err, FinishError::Other(ref m) if m.contains("command not found")),
            "a probe that cannot run must carry why, got: {err}"
        );
        let calls = mock.recorded_calls();
        assert_eq!(
            calls.len(),
            3,
            "nothing beyond the remote probe may run: {calls:?}"
        );
    }

    // Rebase detects conflict via stdout CONFLICT marker (stderr is empty),
    // and the conflicted file name is read from the worktree's own porcelain
    // status (queried before the abort clears it) rather than parsed out of
    // git's English rebase prose.
    #[test]
    fn finish_task_rebase_conflict_in_stdout_returns_rebase_conflict() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"main\n"), // rev-parse HEAD
            MockProcessRunner::ok_with_stdout(b""),       // status --porcelain (clean)
            MockProcessRunner::fail(""),                  // remote get-url (no remote)
            Ok(Output {
                status: exit_fail(),
                stdout: b"CONFLICT (content): Merge conflict in lib.rs\n".to_vec(),
                stderr: vec![],
            }),
            MockProcessRunner::ok_with_stdout(b"UU lib.rs\n"), // status --porcelain (mid-rebase, conflicted)
            MockProcessRunner::ok(),                           // git rebase --abort
        ]);

        let err = finish_task(&fctx("main"), &mock).unwrap_err();

        assert!(
            matches!(err, FinishError::RebaseConflict { ref files, .. } if files == &["lib.rs".to_string()]),
            "CONFLICT in stdout should map to RebaseConflict naming lib.rs, got: {err}"
        );
        assert!(
            err.to_string().contains("lib.rs"),
            "error message should name the conflicted file, got: {err}"
        );
    }

    // A dirty primary worktree is detected before any pull/rebase call, and
    // reported as its own distinct error — not conflated with a rebase
    // conflict.
    #[test]
    fn finish_task_dirty_primary_worktree_returns_error_before_pull() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"main\n"), // rev-parse HEAD
            MockProcessRunner::ok_with_stdout(b" M src/unrelated.rs\n?? scratch.txt\n"), // status --porcelain (dirty)
        ]);

        let err = finish_task(&fctx("main"), &mock).unwrap_err();

        assert!(
            matches!(err, FinishError::DirtyPrimaryWorktree { ref path, ref files }
                if path == "/repo" && files == &["src/unrelated.rs".to_string(), "scratch.txt".to_string()]),
            "dirty primary worktree should be reported before any rebase attempt, got: {err}"
        );
        assert!(
            err.to_string().contains("/repo") && err.to_string().contains("uncommitted"),
            "error message should name the primary worktree and mention uncommitted changes, got: {err}"
        );

        // No pull, rebase, or merge call should have been attempted.
        let calls = mock.recorded_calls();
        assert_eq!(
            calls.len(),
            2,
            "should stop after rev-parse + status --porcelain, got: {calls:?}"
        );
    }

    // --- subprocess bounding ---

    // The pull is the only network call on the finish path. Unbounded, an origin
    // that accepts the connection and then stalls hangs wrap_up forever: no exit
    // token is ever minted, so the agent cannot close its session at all.
    #[test]
    fn finish_task_bounds_the_pull() {
        let mock = MockProcessRunner::new_with_delays(vec![
            (None, MockProcessRunner::ok_with_stdout(b"main\n")), // rev-parse HEAD
            (None, MockProcessRunner::ok_with_stdout(b"")),       // status --porcelain (clean)
            (
                None,
                MockProcessRunner::ok_with_stdout(b"git@github.com:org/repo.git\n"),
            ), // remote get-url
            (Some(TEST_TIMEOUT), MockProcessRunner::ok()), // git pull — stalls past the bound
        ]);

        let err = finish_task(&fctx("main"), &mock).unwrap_err();

        assert!(
            matches!(err, FinishError::Other(ref m) if m.contains("Failed to pull") && m.contains("timed out")),
            "a stalled pull must surface as a timed-out pull, got: {err}"
        );
    }

    // `git rebase` takes the worktree index lock, which another git process in the
    // same checkout can hold indefinitely.
    #[test]
    fn finish_task_bounds_the_rebase() {
        let mock = MockProcessRunner::new_with_delays(vec![
            (None, MockProcessRunner::ok_with_stdout(b"main\n")), // rev-parse HEAD
            (None, MockProcessRunner::ok_with_stdout(b"")),       // status --porcelain (clean)
            (None, MockProcessRunner::fail("")),                  // remote get-url (no remote)
            (Some(TEST_TIMEOUT), MockProcessRunner::ok()), // git rebase — blocked on the lock
        ]);

        let err = finish_task(&fctx("main"), &mock).unwrap_err();

        assert!(
            matches!(err, FinishError::Other(ref m) if m.contains("Failed to run git rebase") && m.contains("timed out")),
            "a rebase blocked on the index lock must surface as a timeout, got: {err}"
        );
    }

    // Same for the fast-forward, which takes the repo root's index lock.
    #[test]
    fn finish_task_bounds_the_fast_forward() {
        let mock = MockProcessRunner::new_with_delays(vec![
            (None, MockProcessRunner::ok_with_stdout(b"main\n")), // rev-parse HEAD
            (None, MockProcessRunner::ok_with_stdout(b"")),       // status --porcelain (clean)
            (None, MockProcessRunner::fail("")),                  // remote get-url (no remote)
            (None, MockProcessRunner::ok()),                      // git rebase
            (Some(TEST_TIMEOUT), MockProcessRunner::ok()), // git merge --ff-only — blocked
        ]);

        let err = finish_task(&fctx("main"), &mock).unwrap_err();

        assert!(
            matches!(err, FinishError::Other(ref m) if m.contains("Failed to fast-forward") && m.contains("timed out")),
            "a blocked fast-forward must surface as a timeout, got: {err}"
        );
    }

    // The test that pins the convention rather than three instances of it: every
    // subprocess reachable on a successful finish carries the bound, including the
    // three preflight reads reached through `crate::git`. A future unbounded call
    // added anywhere on this path fails here.
    //
    // The preflight reads (rev-parse, status --porcelain, remote get-url) go
    // through `crate::git` helpers bounded in Task 2 with the production
    // `SUBPROCESS_TIMEOUT` constant, not the injected `TEST_TIMEOUT` — only the
    // pull/rebase/merge calls `finish_task` issues directly carry the context's
    // `timeout` field. So the recorded timeouts are a mix of two different
    // `Some(_)` durations, not all equal to one value. What matters here is that
    // none of them is `None` — i.e. nothing on the path is unbounded.
    #[test]
    fn finish_task_bounds_every_subprocess_it_runs() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"main\n"), // rev-parse HEAD
            MockProcessRunner::ok_with_stdout(b""),       // status --porcelain (clean)
            MockProcessRunner::ok_with_stdout(b"git@github.com:org/repo.git\n"), // remote get-url
            MockProcessRunner::ok(),                      // git pull origin main
            MockProcessRunner::ok(),                      // git rebase main
            MockProcessRunner::ok(),                      // git merge --ff-only
        ]);

        finish_task(&fctx("main"), &mock).expect("rebase + fast-forward succeeds");

        let timeouts = mock.recorded_timeouts();
        assert_eq!(
            timeouts.len(),
            mock.recorded_calls().len(),
            "every recorded call must have a timeout slot"
        );
        assert!(
            timeouts.iter().all(|t| t.is_some()),
            "every subprocess on the finish path must be bounded, got: {timeouts:?}"
        );
    }

    // The happy path above never reaches the conflict branch, so the abort and the
    // porcelain read that precedes it need their own gate. Both are best-effort,
    // so a timeout there degrades exactly as any other failure does — but neither
    // may hang, which is what an unbounded call on a lock-taking abort would do.
    //
    // As above, the preflight reads carry the production `SUBPROCESS_TIMEOUT`
    // (via the bounded `crate::git` helpers) while the conflict-path status read
    // and the abort carry the injected `TEST_TIMEOUT` — a mix of `Some(_)`
    // values is expected; only `None` would indicate an unbounded call.
    #[test]
    fn finish_task_bounds_the_conflict_abort_path() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"main\n"), // rev-parse HEAD
            MockProcessRunner::ok_with_stdout(b""),       // status --porcelain (clean)
            MockProcessRunner::fail(""),                  // remote get-url (no remote)
            Ok(Output {
                status: exit_fail(),
                stdout: b"CONFLICT (content): Merge conflict in lib.rs\n".to_vec(),
                stderr: vec![],
            }), // git rebase — conflicts
            MockProcessRunner::ok_with_stdout(b"UU lib.rs\n"), // status --porcelain (mid-rebase)
            MockProcessRunner::ok(),                           // git rebase --abort
        ]);

        let err = finish_task(&fctx("main"), &mock).unwrap_err();
        assert!(
            matches!(err, FinishError::RebaseConflict { .. }),
            "expected a rebase conflict, got: {err}"
        );

        let timeouts = mock.recorded_timeouts();
        assert_eq!(timeouts.len(), mock.recorded_calls().len());
        assert!(
            timeouts.iter().all(|t| t.is_some()),
            "the conflict read and the abort must be bounded too, got: {timeouts:?}"
        );
    }
}
