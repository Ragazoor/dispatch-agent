use crate::models::expand_tilde;
use crate::process::ProcessRunner;
use crate::tmux;

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
                if files.is_empty() {
                    write!(f, "Rebase conflict on {branch} — resolve and try again")
                } else {
                    write!(
                        f,
                        "Rebase conflict on {branch} in {} — resolve and try again",
                        files.join(", ")
                    )
                }
            }
            FinishError::Other(msg) => write!(f, "{msg}"),
        }
    }
}

/// Parses non-empty `git status --porcelain` lines into their file paths
/// (stripping the two-character status code prefix and the following space).
/// Takes the raw `Output` rather than a pre-trimmed string: the leading
/// status-code column can itself be a space (e.g. `" M"`), which a
/// whole-buffer `.trim()` on the first line would incorrectly eat.
fn parse_porcelain_files(output: &std::process::Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| line.get(3..).unwrap_or(line).trim_end().to_string())
        .collect()
}

/// Parses conflicted file paths out of a failed rebase's combined
/// stdout+stderr, e.g. "CONFLICT (content): Merge conflict in src/main.rs".
/// Returns an empty vec if no parseable conflict line is found — callers
/// fall back to a generic message in that case.
fn parse_conflicted_files(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if !line.starts_with("CONFLICT") {
                return None;
            }
            line.rsplit_once(" in ")
                .map(|(_, path)| path.trim().to_string())
        })
        .collect()
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
    /// The tmux window to kill once the rebase succeeds, if any.
    pub tmux_window: Option<&'a str>,
}

/// Rebase the task branch onto `base_branch` and fast-forward it, then kill the tmux window.
/// The worktree is preserved — it will be cleaned up when the task is archived.
pub fn finish_task(
    ctx: &FinishContext,
    runner: &dyn ProcessRunner,
) -> std::result::Result<(), FinishError> {
    let FinishContext {
        repo_path,
        worktree,
        branch,
        base_branch,
        tmux_window,
    } = *ctx;
    let repo_path = &expand_tilde(repo_path);
    let worktree = &expand_tilde(worktree);

    // 1. Verify we're on the base branch
    let output = runner
        .run(
            "git",
            &["-C", repo_path, "rev-parse", "--abbrev-ref", "HEAD"],
        )
        .map_err(|e| FinishError::Other(format!("Failed to check current branch: {e}")))?;
    let current_branch = stdout_str(&output);
    if current_branch != base_branch {
        return Err(FinishError::NotOnDefaultBranch {
            current: current_branch,
            expected: base_branch.to_string(),
        });
    }

    // 2. Check the primary worktree (repo root) is clean before touching it.
    let output = runner
        .run("git", &["-C", repo_path, "status", "--porcelain"])
        .map_err(|e| FinishError::Other(format!("Failed to check working tree status: {e}")))?;
    let dirty_files = parse_porcelain_files(&output);
    if !dirty_files.is_empty() {
        return Err(FinishError::DirtyPrimaryWorktree {
            path: repo_path.to_string(),
            files: dirty_files,
        });
    }

    // 3. Pull latest base branch (skip if no remote configured)
    let has_remote = runner
        .run("git", &["-C", repo_path, "remote", "get-url", "origin"])
        .map(|o| o.status.success())
        .unwrap_or(false);

    if has_remote {
        let output = runner
            .run(
                "git",
                &[
                    "-C",
                    repo_path,
                    "pull",
                    "--no-rebase",
                    "origin",
                    base_branch,
                ],
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
        .run("git", &["-C", worktree, "rebase", base_branch])
        .map_err(|e| FinishError::Other(format!("Failed to run git rebase: {e}")))?;
    if !output.status.success() {
        let stderr = stderr_str(&output);
        let stdout = stdout_str(&output);
        let is_conflict = is_rebase_conflict(&stdout, &stderr);

        let _ = runner.run("git", &["-C", worktree, "rebase", "--abort"]);

        if is_conflict {
            let files = parse_conflicted_files(&format!("{stdout}\n{stderr}"));
            return Err(FinishError::RebaseConflict {
                branch: branch.to_string(),
                files,
            });
        }
        return Err(FinishError::Other(format!("Rebase failed: {}", stderr)));
    }

    // 5. Fast-forward base branch to the rebased branch
    let output = runner
        .run("git", &["-C", repo_path, "merge", "--ff-only", branch])
        .map_err(|e| FinishError::Other(format!("Failed to fast-forward {base_branch}: {e}")))?;
    if !output.status.success() {
        return Err(FinishError::Other(format!(
            "Fast-forward failed after rebase: {}",
            stderr_str(&output)
        )));
    }

    // 6. Kill tmux window (worktree is preserved for later archival)
    if let Some(window) = tmux_window {
        tmux::kill_window_if_present(window, runner)
            .map_err(|e| FinishError::Other(format!("Failed to kill tmux window: {e}")))?;
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

    /// Build a `FinishContext` with the standard test repo/worktree/branch,
    /// varying only the fields the individual tests care about.
    fn fctx<'a>(base_branch: &'a str, tmux_window: Option<&'a str>) -> FinishContext<'a> {
        FinishContext {
            repo_path: "/repo",
            worktree: "/repo/.worktrees/42-fix-bug",
            branch: "42-fix-bug",
            base_branch,
            tmux_window,
        }
    }

    // The has_window path when tmux itself can't be executed (runner returns
    // Err).  finish_task should warn and still return Ok(()) — a missing tmux
    // window is not a fatal error at this stage.
    #[test]
    fn finish_task_has_window_runner_error_warns_and_succeeds() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"main\n"), // rev-parse HEAD
            MockProcessRunner::ok_with_stdout(b""),       // status --porcelain (clean)
            MockProcessRunner::fail(""),                  // remote get-url (no remote)
            MockProcessRunner::ok(),                      // git rebase main
            MockProcessRunner::ok(),                      // git merge --ff-only
            Err(anyhow::anyhow!("tmux: command not found")), // tmux list-windows (has_window Err)
        ]);

        finish_task(&fctx("main", Some("task-42")), &mock)
            .expect("should succeed despite has_window runner error");
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

        let err = finish_task(&fctx("main", None), &mock).unwrap_err();

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

        let err = finish_task(&fctx("main", None), &mock).unwrap_err();

        assert!(
            matches!(err, FinishError::Other(ref m) if m.contains("Failed to fast-forward")),
            "ff-only runner error should map to FinishError::Other, got: {err}"
        );
    }

    // Rebase detects conflict via stdout CONFLICT marker (stderr is empty),
    // and the conflicted file name is parsed into the error.
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
            MockProcessRunner::ok(), // git rebase --abort
        ]);

        let err = finish_task(&fctx("main", None), &mock).unwrap_err();

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

        let err = finish_task(&fctx("main", None), &mock).unwrap_err();

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
}
