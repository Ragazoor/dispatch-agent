//! Small git plumbing helpers shared across the crate.

use crate::process::ProcessRunner;

/// Detect the default branch for a repo by inspecting `origin/HEAD`.
///
/// Falls back to `"main"` when the remote ref is missing or the command
/// fails (no remote, fresh clone without `git remote set-head`, etc.).
pub fn detect_default_branch(repo_path: &str, runner: &dyn ProcessRunner) -> String {
    if let Ok(output) = runner.run(
        "git",
        &["-C", repo_path, "symbolic-ref", "refs/remotes/origin/HEAD"],
    ) {
        if output.status.success() {
            let refname = String::from_utf8_lossy(&output.stdout).trim().to_string();
            // e.g. "refs/remotes/origin/master" → "master"
            if let Some(branch) = refname.rsplit('/').next() {
                if !branch.is_empty() {
                    return branch.to_string();
                }
            }
        }
    }
    "main".to_string()
}

/// Whether the repo has an `origin` remote configured.
///
/// Three outcomes, not two: `Ok(true)` and `Ok(false)` are the probe's own
/// answers, while `Err` means the probe could not be run at all and so answered
/// nothing. Collapsing the third into `Ok(false)` here would report "no origin
/// remote configured" as a positive finding on the strength of a failure to
/// look — which is exactly the wrong direction for callers that branch on
/// absence.
///
/// Callers decide what each outcome *means*, and all three answer differently:
///
/// - [`crate::dispatch::finish::finish_task`] skips its pull on `Ok(false)` but
///   fails outright on `Err`, because a git it cannot spawn is a real failure it
///   should name rather than rebase past.
/// - `classify_fetch_failure` (`src/dispatch/worktree.rs`) grants the
///   local-branch fallback only on `Ok(false)`; an `Err` is unreachable-origin,
///   since a failure to look identifies nothing.
/// - [`crate::repo_sync::sync_repo`] deliberately treats both as `NoRemote` —
///   for that operation "nothing to sync against" is the same fact either way,
///   a carve-out stated in `docs/specs/repo-sync.allium` under
///   `PreconditionsPrecedeEveryWrite`.
pub(crate) fn has_origin_remote(
    repo_path: &str,
    runner: &dyn ProcessRunner,
) -> std::result::Result<bool, String> {
    runner
        .run("git", &["-C", repo_path, "remote", "get-url", "origin"])
        .map(|o| o.status.success())
        .map_err(|e| format!("Failed to check for an origin remote: {e}"))
}

/// The repo's currently checked-out branch name.
///
/// One of the three preflight reads shared by the rebase path
/// ([`crate::dispatch::finish::finish_task`]) and the repo-sync path
/// ([`crate::repo_sync::sync_repo`]). Both need to know they are on the base
/// branch before writing, because rebase, merge and push all act on whatever is
/// checked out. Returns the branch rather than a yes/no so each caller can name
/// the actual branch in its own error variant.
pub(crate) fn current_branch(
    repo_path: &str,
    runner: &dyn ProcessRunner,
) -> std::result::Result<String, String> {
    runner
        .run(
            "git",
            &["-C", repo_path, "rev-parse", "--abbrev-ref", "HEAD"],
        )
        .map(|output| crate::process::stdout_str(&output))
        .map_err(|e| format!("Failed to check current branch: {e}"))
}

/// Every dirty or untracked path in the repo's working tree, empty when clean.
///
/// The second shared preflight read: both the rebase and the repo-sync path
/// refuse to touch a dirty checkout, because rebasing or merging into one is
/// how work gets lost. Returns the paths so each caller can list them in its
/// own error variant.
pub(crate) fn dirty_files(
    repo_path: &str,
    runner: &dyn ProcessRunner,
) -> std::result::Result<Vec<String>, String> {
    runner
        .run("git", &["-C", repo_path, "status", "--porcelain"])
        .map(|output| parse_porcelain_files(&output))
        .map_err(|e| format!("Failed to check working tree status: {e}"))
}

/// Splits every `git status --porcelain` line into its two-character status
/// code and the path that follows (after the status code and its separating
/// space). Operates on the raw `Output` rather than a pre-trimmed string:
/// the leading status-code column can itself be a space (e.g. `" M"`), which
/// a whole-buffer `.trim()` on the first line would incorrectly eat.
fn porcelain_entries(output: &std::process::Output) -> Vec<(String, String)> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.len() >= 3)
        .map(|line| (line[0..2].to_string(), line[3..].trim_end().to_string()))
        .collect()
}

/// Every dirty/untracked path from a `git status --porcelain` run.
///
/// Shared by the rebase path ([`crate::dispatch::finish::finish_task`]) and the
/// repo-sync path ([`crate::repo_sync::sync_repo`]) so that "is this checkout
/// dirty?" has exactly one answer.
pub(crate) fn parse_porcelain_files(output: &std::process::Output) -> Vec<String> {
    porcelain_entries(output)
        .into_iter()
        .map(|(_, path)| path)
        .collect()
}

/// Just the unmerged (conflicted) paths from a `git status --porcelain` run
/// — status codes `UU`, `AA`, `DD`, or any code containing `U` (added/deleted
/// by us/them). Structural and locale-independent, unlike parsing conflict
/// file names out of rebase's English stdout/stderr prose (which breaks on
/// rename/delete conflicts, whose message doesn't end in "... in <path>").
///
/// Shared by the rebase path and the repo-sync merge path, so conflict
/// detection is not duplicated.
pub(crate) fn parse_unmerged_files(output: &std::process::Output) -> Vec<String> {
    porcelain_entries(output)
        .into_iter()
        .filter(|(code, _)| code == "AA" || code == "DD" || code.contains('U'))
        .map(|(_, path)| path)
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::process::MockProcessRunner;

    fn porcelain(stdout: &[u8]) -> std::process::Output {
        std::process::Output {
            status: crate::process::exit_ok(),
            stdout: stdout.to_vec(),
            stderr: vec![],
        }
    }

    // --- porcelain helpers (moved here from dispatch::finish) ---

    #[test]
    fn parse_porcelain_files_keeps_leading_space_status_codes() {
        let out = porcelain(b" M src/unrelated.rs\n?? scratch.txt\n");
        assert_eq!(
            parse_porcelain_files(&out),
            vec!["src/unrelated.rs".to_string(), "scratch.txt".to_string()]
        );
    }

    #[test]
    fn parse_porcelain_files_is_empty_for_a_clean_tree() {
        assert!(parse_porcelain_files(&porcelain(b"")).is_empty());
    }

    #[test]
    fn parse_porcelain_files_skips_lines_too_short_to_carry_a_path() {
        assert!(parse_porcelain_files(&porcelain(b"M\n")).is_empty());
    }

    #[test]
    fn parse_unmerged_files_selects_only_conflict_codes() {
        let out = porcelain(
            b"UU lib.rs\nAA added.rs\nDD gone.rs\nAU theirs.rs\n M clean.rs\n?? new.rs\n",
        );
        assert_eq!(
            parse_unmerged_files(&out),
            vec![
                "lib.rs".to_string(),
                "added.rs".to_string(),
                "gone.rs".to_string(),
                "theirs.rs".to_string(),
            ]
        );
    }

    #[test]
    fn parse_unmerged_files_is_empty_when_only_dirty_paths_exist() {
        assert!(parse_unmerged_files(&porcelain(b" M src/a.rs\n?? b.txt\n")).is_empty());
    }

    // --- detect_default_branch ---

    #[test]
    fn detect_default_branch_returns_remote_head_when_set() {
        let runner = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(
            b"refs/remotes/origin/master\n",
        )]);
        assert_eq!(detect_default_branch("/repo", &runner), "master");
    }

    #[test]
    fn detect_default_branch_falls_back_when_origin_head_missing() {
        let runner = MockProcessRunner::new(vec![MockProcessRunner::fail(
            "fatal: ref refs/remotes/origin/HEAD is not a symbolic ref",
        )]);
        assert_eq!(detect_default_branch("/repo", &runner), "main");
    }

    #[test]
    fn detect_default_branch_falls_back_when_runner_errors() {
        let runner = MockProcessRunner::new(vec![Err(anyhow::anyhow!("git not on PATH"))]);
        assert_eq!(detect_default_branch("/repo", &runner), "main");
    }

    // --- has_origin_remote ---

    #[test]
    fn has_origin_remote_reports_a_configured_remote() {
        let runner = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(
            b"git@github.com:org/repo.git\n",
        )]);
        assert_eq!(has_origin_remote("/repo", &runner), Ok(true));
    }

    #[test]
    fn has_origin_remote_reports_a_repo_without_one() {
        let runner = MockProcessRunner::new(vec![MockProcessRunner::fail(
            "error: No such remote 'origin'",
        )]);
        assert_eq!(has_origin_remote("/repo", &runner), Ok(false));
    }

    // The point of the Result: a probe that could not be *run* is not a positive
    // finding that there is no remote, and callers must be able to tell the two
    // apart rather than have `git.rs` collapse them on their behalf.
    #[test]
    fn has_origin_remote_distinguishes_a_probe_that_could_not_be_run() {
        let runner = MockProcessRunner::new(vec![Err(anyhow::anyhow!("git not on PATH"))]);
        let err = has_origin_remote("/repo", &runner)
            .expect_err("a probe that cannot be run is not an answer");
        assert!(
            err.contains("git not on PATH"),
            "the failure must carry why the probe could not run, got: {err}"
        );
    }

    #[test]
    fn has_origin_remote_invokes_remote_get_url_origin() {
        let runner = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        let _ = has_origin_remote("/some/repo", &runner);
        let calls = runner.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "git");
        assert_eq!(
            calls[0].1,
            vec![
                "-C".to_string(),
                "/some/repo".to_string(),
                "remote".to_string(),
                "get-url".to_string(),
                "origin".to_string(),
            ]
        );
    }

    #[test]
    fn detect_default_branch_invokes_correct_git_command() {
        let runner = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(
            b"refs/remotes/origin/main\n",
        )]);
        let _ = detect_default_branch("/some/repo", &runner);
        let calls = runner.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "git");
        assert_eq!(
            calls[0].1,
            vec![
                "-C".to_string(),
                "/some/repo".to_string(),
                "symbolic-ref".to_string(),
                "refs/remotes/origin/HEAD".to_string(),
            ]
        );
    }
}
