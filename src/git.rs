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
