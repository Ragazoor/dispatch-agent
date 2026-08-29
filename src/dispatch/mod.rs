use anyhow::{Context, Result};

use crate::models::{extract_github_repo, ReviewDecision};
use crate::process::{ProcessRunner, SUBPROCESS_TIMEOUT};

mod agents;
mod allium_specs;
mod caller_identity;
mod finish;
pub(crate) mod git_output;
/// Shared `MockProcessRunner` scripts for the dispatch call sequence. Lives here
/// rather than in a test module because the sequence it declares is this
/// module's own, and three other test suites drive it.
#[cfg(test)]
pub(crate) mod mock_sequence;
mod prompts;
mod split_panes;
mod trust;
mod worktree;

pub use agents::{
    agent_tree_pane_id, companion_pane_ids, create_main_session, dispatch_agent,
    fetch_verify_command, main_session_window_alive, prepare_inputs, prepare_inputs_with_epic_ctx,
    quick_dispatch_agent, research_agent, resume_agent, resync_agent_tree_pane, run_agent_for_mode,
    toggle_agent_tree_pane, DispatchInputs, MAIN_SESSION_WINDOW,
};
pub use finish::{finish_task, FinishContext, FinishError};
// Test-only re-export: `prompts` is private, and the routing tests in
// `src/runtime/tests/` and `src/service/tasks/tests/` assert on this marker to
// prove `DispatchMode::Research` reached `build_research_prompt`.
#[cfg(test)]
pub(crate) use prompts::RESEARCH_AGENT_INTRO;
pub use prompts::{build_and_record_injections, EpicContext, LearningInjections};
pub use split_panes::{join_task_window_into_pane, swap_task_window_into_pane};
pub(crate) use trust::{claude_json_path, is_trusted_at, trust_at};
pub(crate) use worktree::PROVISION_MAX_SUBPROCESS_CALLS;
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
        .run_with_timeout(
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
            SUBPROCESS_TIMEOUT,
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
/// `pub(crate)` only so the shared fixture builders in here
/// (`make_test_repo_with_worktree`) can be reached from other modules' tests
/// instead of being copied into them.
#[cfg(test)]
pub(crate) mod tests;
