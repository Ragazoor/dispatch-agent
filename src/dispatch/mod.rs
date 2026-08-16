use anyhow::{Context, Result};

use crate::models::ReviewDecision;
use crate::process::ProcessRunner;

mod agents;
mod finish;
pub(crate) mod git_output;
/// Shared `MockProcessRunner` scripts for the dispatch call sequence. Lives here
/// rather than in a test module because the sequence it declares is this
/// module's own, and three other test suites drive it.
#[cfg(test)]
pub(crate) mod mock_sequence;
// `pub(crate)` (not plain-private) because `parse_tmux_window_task_id` is
// also the resolver `TaskService::record_peer_message_sent`
// (src/service/tasks/crud.rs) uses for a SendMessage `to` field — every
// other item in this module keeps its own `pub(super)` cap regardless.
pub(crate) mod prompts;
mod split_panes;
mod trust;
mod worktree;

pub use agents::{
    agent_tree_pane_id, companion_pane_ids, create_main_session, dispatch_agent,
    fetch_verify_command, is_wrappable, main_session_window_alive, prepare_inputs,
    prepare_inputs_with_epic_ctx, quick_dispatch_agent, research_agent, resume_agent,
    resync_agent_tree_pane, toggle_agent_tree_pane, DispatchInputs, MAIN_SESSION_WINDOW,
};
pub use finish::{finish_task, FinishContext, FinishError};
pub use prompts::{build_and_record_injections, EpicContext, LearningInjections};
pub use split_panes::{join_task_window_into_pane, swap_task_window_into_pane};
pub use trust::{is_repo_trusted, trust_repo};
pub use worktree::{branch_from_worktree, teardown_task, validate_repo_path, TeardownFailure};

/// Extract stderr from a process `Output` as a trimmed `String`.
pub(super) fn stderr_str(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_string()
}

/// Extract stdout from a process `Output` as a trimmed `String`.
pub(super) fn stdout_str(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

// ---------------------------------------------------------------------------
// PR types
// ---------------------------------------------------------------------------
//
// PR creation moved to the agent /wrap-up skill — see
// plugin/skills/wrap-up/SKILL.md and the WrapUpPr rule in
// docs/specs/tasks.allium. The dispatch-side `create_pr` and `merge_pr`
// helpers have been removed; only the status-check helper remains here.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrState {
    Open,
    Merged,
    Closed,
}

#[derive(Debug)]
pub struct PrStatus {
    pub state: PrState,
    pub review_decision: Option<ReviewDecision>,
}

// ---------------------------------------------------------------------------
// PR functions
// ---------------------------------------------------------------------------

/// Check the current status of a PR using `gh pr view`.
pub fn check_pr_status(pr_url: &str, runner: &dyn ProcessRunner) -> Result<PrStatus> {
    let output = runner
        .run(
            "gh",
            &[
                "pr",
                "view",
                pr_url,
                "--json",
                "state,reviewDecision",
                "-q",
                r#"[.state, .reviewDecision] | join("\n")"#,
            ],
        )
        .context("Failed to run gh pr view")?;
    if !output.status.success() {
        anyhow::bail!("gh pr view failed: {}", stderr_str(&output));
    }

    let stdout = stdout_str(&output);
    let mut lines = stdout.lines();
    let state_str = lines
        .next()
        .ok_or_else(|| anyhow::anyhow!("gh pr view: no output"))?
        .to_uppercase();
    // review_decision is optional — repos without branch-protection rules omit it.
    let review_str = lines.next().unwrap_or("").to_uppercase();

    let state = match state_str.as_str() {
        "OPEN" => PrState::Open,
        "MERGED" => PrState::Merged,
        "CLOSED" => PrState::Closed,
        other => anyhow::bail!("gh pr view: unknown PR state {:?}", other),
    };

    let review_decision = ReviewDecision::parse(&review_str);

    Ok(PrStatus {
        state,
        review_decision,
    })
}

/// Resolve a PR's head (source) branch via `gh pr view`, so a review worktree
/// can be based on the PR's code.
///
/// Returns `None` — so callers fall back to the task's base branch — on any
/// command failure, empty output, or a cross-repository (fork) PR. A fork PR's
/// head branch lives on the fork, not `origin`, so it cannot be fetched by name;
/// basing on it would fail `git worktree add`, hence the fork fall-back.
pub fn pr_head_branch(pr_url: &str, runner: &dyn ProcessRunner) -> Option<String> {
    let output = runner
        .run(
            "gh",
            &[
                "pr",
                "view",
                pr_url,
                "--json",
                "headRefName,isCrossRepository",
                "-q",
                r#"[.headRefName, (.isCrossRepository|tostring)] | join("\n")"#,
            ],
        )
        .ok()?;
    if !output.status.success() {
        tracing::warn!(
            pr_url,
            "gh pr view headRefName failed; falling back to base branch"
        );
        return None;
    }

    let stdout = stdout_str(&output);
    let mut lines = stdout.lines();
    let head = lines.next().unwrap_or("").trim().to_string();
    let is_fork = lines.next().unwrap_or("").trim() == "true";
    if head.is_empty() || is_fork {
        return None;
    }
    Some(head)
}

/// Fallback group name used by `repo_name_from_url` when the URL is not a
/// recognisable GitHub URL (empty, non-GitHub host, or malformed).
pub const UNKNOWN_REPO_GROUP: &str = "other";

/// Extract the short repo name (e.g. `"my-repo"`) from a GitHub URL.
///
/// Returns [`UNKNOWN_REPO_GROUP`] for non-GitHub URLs, empty strings, and malformed input.
pub fn repo_name_from_url(url: &str) -> String {
    extract_github_repo(url)
        .and_then(|s| s.split('/').next_back())
        .unwrap_or(UNKNOWN_REPO_GROUP)
        .to_string()
}

/// Derive a repo-grouping key from a filesystem repo path: the final path
/// component (basename). Empty / root paths fall back to `UNKNOWN_REPO_GROUP`,
/// matching the URL-based grouping used by feeds.
pub fn repo_name_from_path(repo_path: &str) -> String {
    std::path::Path::new(repo_path.trim_end_matches('/'))
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(UNKNOWN_REPO_GROUP)
        .to_string()
}

/// Extract `"org/repo"` from a GitHub URL.
///
/// Handles `https://github.com/org/repo`, `.../pull/N`, `.../issues/N`,
/// `.../tree/...`, and similar paths — any URL whose host is `github.com`.
/// Returns `None` for non-GitHub URLs, empty strings, and single-segment paths.
pub fn extract_github_repo(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("https://github.com/")?;
    let rest = rest.trim_end_matches('/');
    // Need at least two path segments: "org/repo[/...]"
    let slash = rest.find('/')?;
    let after_org = &rest[slash + 1..];
    if after_org.is_empty() {
        return None;
    }
    let end = after_org.find('/').unwrap_or(after_org.len());
    let repo = &after_org[..end];
    if repo.is_empty() {
        return None;
    }
    Some(&rest[..slash + 1 + end])
}

/// Resolve the local repo path for each feed item from its URL.
///
/// For each item, attempts `extract_github_repo(url)` → `resolve_repo_path(...)`.
/// Items whose URL cannot be resolved get an empty-string sentinel (`""`).
pub fn resolve_feed_item_repo_paths(
    items: &[crate::models::FeedItem],
    known_paths: &[String],
) -> Vec<String> {
    items
        .iter()
        .map(|item| {
            extract_github_repo(&item.url)
                .and_then(|r| resolve_repo_path(r, known_paths))
                .unwrap_or_default()
        })
        .collect()
}

/// Resolve a GitHub repo name (e.g. `"org/repo"`) to a local filesystem path
/// by matching against known repo paths.  Returns the first path whose
/// directory name equals the short repo name.
pub fn resolve_repo_path(github_repo: &str, known_paths: &[String]) -> Option<String> {
    let repo_short = github_repo.split('/').next_back().unwrap_or(github_repo);
    known_paths
        .iter()
        .find(|p| {
            std::path::Path::new(p)
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|dir| dir == repo_short)
        })
        .cloned()
}

#[cfg(test)]
mod prompts_snapshots;
#[cfg(test)]
mod tests;
