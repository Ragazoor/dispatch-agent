use anyhow::{Context, Result};
use std::fs;

use crate::models::{DispatchResult, EpicId, ResumeResult, Task, TaskId, TaskStatus, slugify};
use crate::process::ProcessRunner;
use crate::tmux;

// ---------------------------------------------------------------------------
// dispatch_agent
// ---------------------------------------------------------------------------

struct ProvisionResult {
    worktree_path: String,
    tmux_window: String,
}

/// Create a git worktree and open a tmux window.
/// Shared by both `dispatch_agent` and `brainstorm_agent`.
fn provision_worktree(
    task: &Task,
    runner: &dyn ProcessRunner,
    base_branch: Option<&str>,
) -> Result<ProvisionResult> {
    let repo_path = expand_tilde(&task.repo_path);
    let slug = slugify(&task.title);
    let worktree_name = format!("{}-{slug}", task.id);
    let worktree_path = format!("{repo_path}/.worktrees/{worktree_name}");
    let tmux_window = build_tmux_window_name(task.id);

    tracing::info!(task_id = task.id.0, %worktree_path, ?base_branch, "provisioning worktree");

    fs::create_dir_all(format!("{repo_path}/.worktrees"))
        .context("failed to create .worktrees directory")?;

    if std::path::Path::new(&worktree_path).exists() {
        tracing::info!(task_id = task.id.0, %worktree_path, "worktree already exists, reusing");
    } else {
        let mut args = vec!["-C", &repo_path, "worktree", "add", &*worktree_path, "-B", &*worktree_name];
        if let Some(base) = base_branch {
            args.push(base);
        }
        let output = runner
            .run("git", &args)
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
    tmux::ensure_split_hook(runner)
        .context("failed to ensure tmux split hook")?;

    Ok(ProvisionResult { worktree_path, tmux_window })
}

fn rebase_preamble(target: &str) -> String {
    format!(
        "Before starting work, rebase your branch from {target}:\n\
         ```\n\
         git fetch origin && git rebase {target}\n\
         ```"
    )
}

#[derive(Clone, Copy)]
enum ClaudeMode {
    AcceptEdits,
    Plan,
}

impl ClaudeMode {
    fn as_flag(self) -> &'static str {
        match self {
            ClaudeMode::AcceptEdits => "acceptEdits",
            ClaudeMode::Plan => "plan",
        }
    }
}

/// Provision worktree, write prompt file, launch Claude via tmux.
/// The prompt file is deleted after Claude reads it.
/// Shared by all dispatch variants.
fn dispatch_with_prompt(
    task: &Task,
    prompt: &str,
    mode: ClaudeMode,
    runner: &dyn ProcessRunner,
    base_branch: Option<&str>,
) -> Result<DispatchResult> {
    let repo_path = expand_tilde(&task.repo_path);

    // Resolve the start-point once; reuse in both provision_worktree and rebase_preamble.
    let detected;
    let resolved = match base_branch {
        Some(b) => b,
        None => {
            detected = format!("origin/{}", detect_default_branch(&repo_path, runner));
            &detected
        }
    };

    let provision = provision_worktree(task, runner, Some(resolved))?;

    let full_prompt = format!("{}\n\n{prompt}", rebase_preamble(resolved));
    let prompt_file = format!("{}/.claude-prompt", provision.worktree_path);
    fs::write(&prompt_file, &full_prompt)
        .with_context(|| format!("failed to write {prompt_file}"))?;
    let claude_cmd = format!(
        "bash -c 'prompt=$(cat .claude-prompt) && rm -f .claude-prompt \
         && claude --permission-mode {} \"$prompt\"'",
        mode.as_flag()
    );
    tmux::send_keys(&provision.tmux_window, &claude_cmd, runner)
        .context("failed to send keys to tmux window")?;

    tracing::info!(task_id = task.id.0, worktree = %provision.worktree_path, "agent dispatched");

    Ok(DispatchResult {
        worktree_path: provision.worktree_path,
        tmux_window: provision.tmux_window,
    })
}

pub fn dispatch_agent(task: &Task, runner: &dyn ProcessRunner) -> Result<DispatchResult> {
    let prompt = build_prompt(task.id, &task.title, &task.description, task.plan.as_deref());
    dispatch_with_prompt(task, &prompt, ClaudeMode::AcceptEdits, runner, None)
}

pub fn brainstorm_agent(task: &Task, runner: &dyn ProcessRunner) -> Result<DispatchResult> {
    let prompt = build_brainstorm_prompt(task.id, &task.title, &task.description);
    dispatch_with_prompt(task, &prompt, ClaudeMode::Plan, runner, None)
}

pub fn plan_agent(task: &Task, runner: &dyn ProcessRunner) -> Result<DispatchResult> {
    let prompt = build_plan_prompt(task.id, &task.title, &task.description);
    dispatch_with_prompt(task, &prompt, ClaudeMode::Plan, runner, None)
}

pub fn quick_dispatch_agent(task: &Task, runner: &dyn ProcessRunner) -> Result<DispatchResult> {
    let prompt = build_quick_dispatch_prompt(task.id, &task.title, &task.description);
    dispatch_with_prompt(task, &prompt, ClaudeMode::AcceptEdits, runner, None)
}

pub fn epic_planning_agent(task: &Task, epic_id: EpicId, epic_title: &str, epic_description: &str, runner: &dyn ProcessRunner) -> Result<DispatchResult> {
    let prompt = build_epic_planning_prompt(epic_id, epic_title, epic_description);
    dispatch_with_prompt(task, &prompt, ClaudeMode::Plan, runner, None)
}

/// Dispatch a task from main (epic auto-dispatch).
pub fn dispatch_chained_agent(task: &Task, runner: &dyn ProcessRunner) -> Result<DispatchResult> {
    let prompt = build_prompt(task.id, &task.title, &task.description, task.plan.as_deref());
    dispatch_with_prompt(task, &prompt, ClaudeMode::AcceptEdits, runner, None)
}

/// Plan a task from main (epic auto-dispatch).
pub fn plan_chained_agent(task: &Task, runner: &dyn ProcessRunner) -> Result<DispatchResult> {
    let prompt = build_plan_prompt(task.id, &task.title, &task.description);
    dispatch_with_prompt(task, &prompt, ClaudeMode::Plan, runner, None)
}

/// Brainstorm a task from main (epic auto-dispatch).
pub fn brainstorm_chained_agent(task: &Task, runner: &dyn ProcessRunner) -> Result<DispatchResult> {
    let prompt = build_brainstorm_prompt(task.id, &task.title, &task.description);
    dispatch_with_prompt(task, &prompt, ClaudeMode::Plan, runner, None)
}

// ---------------------------------------------------------------------------
// cleanup_task
// ---------------------------------------------------------------------------

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
        .run("git", &["-C", &repo, "worktree", "remove", "--force", worktree_path])
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

// ---------------------------------------------------------------------------
// finish_task
// ---------------------------------------------------------------------------

/// Detect the default branch for a repo by inspecting `origin/HEAD`.
/// Falls back to `"main"` when there is no remote or the ref is missing.
fn detect_default_branch(repo_path: &str, runner: &dyn ProcessRunner) -> String {
    if let Ok(output) = runner.run(
        "git",
        &["-C", repo_path, "symbolic-ref", "refs/remotes/origin/HEAD"],
    ) {
        if output.status.success() {
            let refname = stdout_str(&output);
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

/// Errors from the finish (rebase + cleanup) operation.
#[derive(Debug)]
pub enum FinishError {
    NotOnDefaultBranch { current: String, expected: String },
    RebaseConflict(String),
    Other(String),
}

impl std::fmt::Display for FinishError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FinishError::NotOnDefaultBranch { current, expected } => write!(
                f,
                "Repo root is not on {expected} (currently on {current}) — checkout {expected} first"
            ),
            FinishError::RebaseConflict(branch) => {
                write!(f, "Rebase conflict on {branch} — resolve and try again")
            }
            FinishError::Other(msg) => write!(f, "{msg}"),
        }
    }
}

/// Rebase the task branch onto main and fast-forward main, then kill the tmux window.
/// The worktree is preserved — it will be cleaned up when the task is archived.
pub fn finish_task(
    repo_path: &str,
    worktree: &str,
    branch: &str,
    tmux_window: Option<&str>,
    runner: &dyn ProcessRunner,
) -> std::result::Result<(), FinishError> {
    let repo_path = &expand_tilde(repo_path);
    let worktree = &expand_tilde(worktree);
    let default_branch = detect_default_branch(repo_path, runner);

    // 1. Verify we're on the default branch
    let output = runner
        .run("git", &["-C", repo_path, "rev-parse", "--abbrev-ref", "HEAD"])
        .map_err(|e| FinishError::Other(format!("Failed to check current branch: {e}")))?;
    let current_branch = stdout_str(&output);
    if current_branch != default_branch {
        return Err(FinishError::NotOnDefaultBranch {
            current: current_branch,
            expected: default_branch,
        });
    }

    // 2. Pull latest default branch (skip if no remote configured)
    let has_remote = runner
        .run("git", &["-C", repo_path, "remote", "get-url", "origin"])
        .map(|o| o.status.success())
        .unwrap_or(false);

    if has_remote {
        let output = runner
            .run(
                "git",
                &["-C", repo_path, "pull", "origin", &default_branch],
            )
            .map_err(|e| FinishError::Other(format!("Failed to pull: {e}")))?;
        if !output.status.success() {
            return Err(FinishError::Other(format!(
                "Failed to pull {default_branch}: {}",
                stderr_str(&output)
            )));
        }
    }

    // 3. Rebase branch onto default branch (from worktree, where branch is checked out)
    let output = runner
        .run("git", &["-C", worktree, "rebase", &default_branch])
        .map_err(|e| FinishError::Other(format!("Failed to run git rebase: {e}")))?;
    if !output.status.success() {
        let stderr = stderr_str(&output);
        let stdout = stdout_str(&output);
        let is_conflict = stderr.contains("CONFLICT")
            || stdout.contains("CONFLICT")
            || stderr.contains("could not apply")
            || stderr.contains("Merge conflict");

        let _ = runner.run("git", &["-C", worktree, "rebase", "--abort"]);

        if is_conflict {
            return Err(FinishError::RebaseConflict(branch.to_string()));
        }
        return Err(FinishError::Other(format!(
            "Rebase failed: {}",
            stderr
        )));
    }

    // 4. Fast-forward default branch to the rebased branch
    let output = runner
        .run("git", &["-C", repo_path, "merge", "--ff-only", branch])
        .map_err(|e| FinishError::Other(format!("Failed to fast-forward {default_branch}: {e}")))?;
    if !output.status.success() {
        return Err(FinishError::Other(format!(
            "Fast-forward failed after rebase: {}",
            stderr_str(&output)
        )));
    }

    // 5. Kill tmux window (worktree is preserved for later archival)
    if let Some(window) = tmux_window {
        match tmux::has_window(window, runner) {
            Ok(true) => {
                tmux::kill_window(window, runner)
                    .map_err(|e| FinishError::Other(format!("Failed to kill tmux window: {e}")))?;
            }
            Ok(false) => {}
            Err(e) => {
                tracing::warn!("could not check tmux window during finish: {e}");
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// resume_agent
// ---------------------------------------------------------------------------

/// Re-open a tmux window for an existing worktree and resume the most recent
/// Claude conversation with `claude --continue`.
///
/// This function is **synchronous** and should be called via
/// `tokio::task::spawn_blocking` from async contexts.
pub fn resume_agent(task_id: TaskId, worktree_path: &str, runner: &dyn ProcessRunner) -> Result<ResumeResult> {
    let tmux_window = build_tmux_window_name(task_id);

    tmux::new_window(&tmux_window, worktree_path, runner)
        .context("failed to create tmux window for resume")?;

    tmux::set_window_dispatch_dir(&tmux_window, worktree_path, runner)
        .context("failed to set tmux window dispatch dir")?;
    tmux::ensure_split_hook(runner)
        .context("failed to ensure tmux split hook")?;

    tmux::send_keys(&tmux_window, "claude --continue", runner)
        .context("failed to send resume keys to tmux window")?;

    tracing::info!(task_id = task_id.0, %tmux_window, "agent resumed");

    Ok(ResumeResult { tmux_window })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn build_tmux_window_name(task_id: TaskId) -> String {
    format!("task-{task_id}")
}

fn build_prompt(task_id: TaskId, title: &str, description: &str, plan: Option<&str>) -> String {
    let plan_section = match plan {
        Some(path) => format!(
            "\n\nPlan: {path}\nRead this file for the full implementation plan. Follow it step by step."
        ),
        None => String::new(),
    };

    format!(
        "You are an autonomous coding agent. \
Your task is:\n\
  ID: {task_id}\n\
  Title: {title}\n\
  Description: {description}\
{plan_section}\n\
\n\
Task status transitions (running/review) are managed automatically via hooks. \
Do not call update_task for status changes.\n\
\n\
The dispatch MCP tools are available — use them to query and update this task \
(get_task, update_task).\n\
\n\
When implementation is complete, use the /wrap-up skill to commit remaining \
changes and ask the user whether to rebase onto main or create a PR."
    )
}

fn build_quick_dispatch_prompt(task_id: TaskId, title: &str, description: &str) -> String {
    format!(
        "You are an autonomous coding agent working interactively with the user.\n\
\n\
Task:\n\
  ID: {task_id}\n\
  Title: {title}\n\
  Description: {description}\n\
\n\
This is a quick-dispatched task with a placeholder title. After you understand what \
the user wants, call `update_task` with a descriptive `title` (and optionally \
`description`) to rename the task on the kanban board.\n\
\n\
Task status transitions (running/review) are managed automatically via hooks. \
Do not call update_task for status changes.\n\
The dispatch MCP tools are available — use them to query and update tasks. \
Use update_task to rename this task with a descriptive title, and get_task to check current state."
    )
}

fn build_brainstorm_prompt(task_id: TaskId, title: &str, description: &str) -> String {
    format!(
        "You are an autonomous coding agent starting a brainstorming session.\n\
\n\
Task:\n\
  ID: {task_id}\n\
  Title: {title}\n\
  Description: {description}\n\
\n\
Before diving in, ask the user any clarifying questions needed to ensure you \
fully understand the requirements and intended behaviour.\n\
\n\
Your goal is to explore the codebase, brainstorm approaches, and write an \
implementation plan. When done, save the plan and attach it to the task:\n\
\n\
1. Write the plan to docs/plans/ (or docs/superpowers/specs/ if using the brainstorming skill)\n\
2. Call update_task via the dispatch MCP tools to set the plan field to the plan file path\n\
\n\
After planning, ask whether to continue implementing or stop.\n\
\n\
Use the dispatch MCP tools to attach the plan (update_task — set the plan field)."
    )
}

fn build_plan_prompt(task_id: TaskId, title: &str, description: &str) -> String {
    format!(
        "You are an autonomous coding agent starting a planning session.\n\
\n\
Task:\n\
  ID: {task_id}\n\
  Title: {title}\n\
  Description: {description}\n\
\n\
Your goal is to explore the codebase and write a focused implementation plan. \
Use /plan mode for a structured planning session. When done, save the plan \
and attach it to the task:\n\
\n\
1. Write the plan to docs/plans/\n\
2. Call update_task via the dispatch MCP tools to set the plan field to the plan file path\n\
\n\
After planning, ask whether to continue implementing or stop.\n\
\n\
Use the dispatch MCP tools to attach the plan (update_task — set the plan field)."
    )
}

fn build_epic_planning_prompt(epic_id: EpicId, title: &str, description: &str) -> String {
    format!(
        "You are an autonomous coding agent starting a planning session.\n\
\n\
Epic:\n\
  ID: {epic_id}\n\
  Title: {title}\n\
  Description: {description}\n\
\n\
Your goal is to explore the codebase, write an implementation plan, and break \
it into work packages on the kanban board.\n\
\n\
Steps:\n\
1. Explore the codebase to understand what needs to change.\n\
2. Use the /brainstorm skill to write the plan. When done, attach it to the epic:\n\
   Call update_epic with epic_id={epic_id} and plan=<absolute path to plan file>\n\
3. Create work packages from the plan using create_task. Work packages are kanban \
tasks — do not confuse them with subtasks inside the plan document itself:\n\
   - Set epic_id={epic_id} on every work package\n\
   - Use sort_order to control execution order (1, 2, 3, \u{2026})\n\
   - Work packages at the same sort_order in different repositories run in parallel\n\
   - Work packages in the same repository must have different sort_order values\n\
   - Set repo_path to the absolute path of the repository each work package targets\n\
\n\
After creating the work packages, confirm with the user before doing anything further.\n\
\n\
Use the dispatch MCP tools to query tasks and epics. Relevant tools: create_task, update_epic, list_tasks.\n\
\n\
IMPORTANT: Do NOT start implementing. Your job ends after creating the work packages.",
        epic_id = epic_id,
        title = title,
        description = description,
    )
}

// ---------------------------------------------------------------------------
// PR types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PrResult {
    pub pr_url: String,
}

#[derive(Debug)]
pub enum PrError {
    PushFailed(String),
    CreateFailed(String),
    Other(String),
}

impl std::fmt::Display for PrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PrError::PushFailed(msg) => write!(f, "Push failed: {msg}"),
            PrError::CreateFailed(msg) => write!(f, "PR creation failed: {msg}"),
            PrError::Other(msg) => write!(f, "{msg}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrState {
    Open,
    Merged,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrReviewDecision {
    Approved,
    ChangesRequested,
    ReviewRequired,
}

#[derive(Debug)]
pub struct PrStatus {
    pub state: PrState,
    pub review_decision: Option<PrReviewDecision>,
}

// ---------------------------------------------------------------------------
// PR functions
// ---------------------------------------------------------------------------

/// Extract "owner/repo" from a git remote URL.
/// Handles both SSH (git@github.com:owner/repo.git) and HTTPS (https://github.com/owner/repo.git).
fn parse_repo_slug(remote_url: &str) -> Option<String> {
    let url = remote_url.trim().trim_end_matches(".git");
    // SSH: git@github.com:owner/repo
    if let Some(path) = url.strip_prefix("git@github.com:") {
        return Some(path.to_string());
    }
    // HTTPS: https://github.com/owner/repo
    if let Some(rest) = url.strip_prefix("https://github.com/") {
        return Some(rest.to_string());
    }
    None
}

/// Push the branch to origin and create a GitHub PR using `gh`.
pub fn create_pr(
    repo_path: &str,
    branch: &str,
    title: &str,
    description: &str,
    runner: &dyn ProcessRunner,
) -> std::result::Result<PrResult, PrError> {
    let repo_path = &expand_tilde(repo_path);
    let default_branch = detect_default_branch(repo_path, runner);

    // 1. Push the branch
    let output = runner
        .run("git", &["-C", repo_path, "push", "-u", "origin", branch])
        .map_err(|e| PrError::PushFailed(format!("Failed to run git push: {e}")))?;
    if !output.status.success() {
        return Err(PrError::PushFailed(stderr_str(&output)));
    }

    // 2. Get the repo slug from git remote
    let remote_output = runner
        .run("git", &["-C", repo_path, "remote", "get-url", "origin"])
        .map_err(|e| PrError::Other(format!("Failed to get remote URL: {e}")))?;
    let remote_url = stdout_str(&remote_output);
    let repo_slug = parse_repo_slug(&remote_url)
        .ok_or_else(|| PrError::Other(format!("Could not parse repo slug from: {remote_url}")))?;

    // 3. Create the PR
    let output = runner
        .run("gh", &[
            "pr", "create",
            "--draft",
            "--title", title,
            "--body", description,
            "--head", branch,
            "--base", &default_branch,
            "--repo", &repo_slug,
        ])
        .map_err(|e| PrError::Other(format!("Failed to run gh: {e}")))?;
    if !output.status.success() {
        return Err(PrError::CreateFailed(stderr_str(&output)));
    }

    // 4. Parse the PR URL from stdout
    let pr_url = stdout_str(&output);
    Ok(PrResult { pr_url })
}

/// Check the current status of a PR using `gh pr view`.
pub fn check_pr_status(
    pr_url: &str,
    runner: &dyn ProcessRunner,
) -> Result<PrStatus> {
    let output = runner
        .run("gh", &[
            "pr", "view", pr_url,
            "--json", "state,reviewDecision",
            "-q", r#"[.state, .reviewDecision] | join("\n")"#,
        ])
        .context("Failed to run gh pr view")?;
    if !output.status.success() {
        anyhow::bail!("gh pr view failed: {}", stderr_str(&output));
    }

    let stdout = stdout_str(&output);
    let mut lines = stdout.lines();
    let state_str = lines.next().unwrap_or("").to_uppercase();
    let review_str = lines.next().unwrap_or("").to_uppercase();

    let state = match state_str.as_str() {
        "MERGED" => PrState::Merged,
        "CLOSED" => PrState::Closed,
        _ => PrState::Open,
    };

    let review_decision = match review_str.as_str() {
        "APPROVED" => Some(PrReviewDecision::Approved),
        "CHANGES_REQUESTED" => Some(PrReviewDecision::ChangesRequested),
        "REVIEW_REQUIRED" => Some(PrReviewDecision::ReviewRequired),
        _ => None,
    };

    Ok(PrStatus { state, review_decision })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve a GitHub repo name (e.g. `"org/repo"`) to a local filesystem path
/// by matching against known repo paths.  Returns the first path whose
/// directory name equals the short repo name.
pub fn resolve_repo_path(github_repo: &str, known_paths: &[String]) -> Option<String> {
    let repo_short = github_repo.split('/').next_back().unwrap_or(github_repo);
    known_paths.iter().find(|p| {
        std::path::Path::new(p)
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|dir| dir == repo_short)
    }).cloned()
}

/// Dispatch a Claude agent to review a PR in an isolated worktree.
pub fn dispatch_review_agent(
    repo_path: &str,
    number: i64,
    title: &str,
    body: &str,
    head_ref: &str,
    runner: &dyn ProcessRunner,
) -> Result<DispatchResult> {
    let repo_path = expand_tilde(repo_path);
    let repo_short = repo_path.split('/').next_back().unwrap_or(&repo_path);
    let worktree_name = format!("review-{number}");
    let worktree_path = format!("{repo_path}/.worktrees/{worktree_name}");
    let tmux_window = format!("review-{repo_short}-{number}");

    // Check if tmux window already exists — focus it instead
    if tmux::has_window(&tmux_window, runner).unwrap_or(false) {
        return Ok(DispatchResult {
            worktree_path,
            tmux_window,
        });
    }

    std::fs::create_dir_all(format!("{repo_path}/.worktrees"))
        .context("failed to create .worktrees directory")?;

    // Fetch the PR branch and create worktree tracking it
    let fetch_output = runner
        .run("git", &["-C", &repo_path, "fetch", "origin", head_ref])
        .context("failed to fetch PR branch")?;
    anyhow::ensure!(
        fetch_output.status.success(),
        "git fetch failed: {}",
        stderr_str(&fetch_output)
    );

    if !std::path::Path::new(&worktree_path).exists() {
        let output = runner
            .run(
                "git",
                &[
                    "-C",
                    &repo_path,
                    "worktree",
                    "add",
                    &worktree_path,
                    &format!("origin/{head_ref}"),
                ],
            )
            .context("failed to create review worktree")?;
        anyhow::ensure!(
            output.status.success(),
            "git worktree add failed: {}",
            stderr_str(&output)
        );
    }

    tmux::new_window(&tmux_window, &worktree_path, runner)
        .context("failed to create tmux window")?;

    // Write prompt and launch Claude
    let prompt = format!(
        "You are reviewing PR #{number}: {title}\n\n\
         ## PR Description\n\n{body}\n\n\
         ## Instructions\n\n\
         1. Read the diff: `gh pr diff {number}`\n\
         2. Review the changes for correctness, style, bugs, and security issues\n\
         3. Submit your review using `gh pr review {number}` with appropriate comments\n\
         4. If changes are needed, use `--request-changes`. If it looks good, use `--approve`.\n\n\
         Focus on substantive issues. Don't nitpick style unless it affects readability."
    );
    let prompt_file = format!("{worktree_path}/.claude-prompt");
    fs::write(&prompt_file, &prompt)
        .with_context(|| format!("failed to write {prompt_file}"))?;
    let claude_cmd = "bash -c 'prompt=$(cat .claude-prompt) && rm -f .claude-prompt && claude --permission-mode acceptEdits \"$prompt\"'";
    tmux::send_keys(&tmux_window, claude_cmd, runner)
        .context("failed to send keys to tmux window")?;

    Ok(DispatchResult {
        worktree_path,
        tmux_window,
    })
}

/// A task can be wrapped up if it has a worktree and is either Running or Review.
pub fn is_wrappable(task: &Task) -> bool {
    task.worktree.is_some()
        && (task.status == TaskStatus::Review || task.status == TaskStatus::Running)
}

/// Extract the branch name from a worktree path (its last path component).
pub fn branch_from_worktree(worktree: &str) -> Option<String> {
    std::path::Path::new(worktree)
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
}

/// Extract stderr from a process `Output` as a trimmed `String`.
fn stderr_str(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_string()
}

/// Extract stdout from a process `Output` as a trimmed `String`.
fn stdout_str(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Expand a leading `~` or `~/` to the user's home directory.
fn expand_tilde(path: &str) -> String {
    if path == "~" || path.starts_with("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return format!("{}{}", home.to_string_lossy(), &path[1..]);
        }
    }
    path.to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use crate::process::{MockProcessRunner, exit_fail};
    use crate::models::{EpicId, Task, TaskId, TaskStatus};
    use chrono::Utc;
    use std::process::Output;

    fn make_task(repo_path: &str) -> Task {
        Task {
            id: TaskId(42),
            title: "Fix bug".to_string(),
            description: "A nasty crash".to_string(),
            repo_path: repo_path.to_string(),
            status: TaskStatus::Backlog,
            worktree: None,
            tmux_window: None,
            plan: None,
            epic_id: None,
            sub_status: crate::models::SubStatus::None,
            pr_url: None,
            tag: None,
            sort_order: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn resolve_repo_path_matches_directory_name() {
        let paths = vec![
            "/home/user/projects/frontend".to_string(),
            "/home/user/projects/backend".to_string(),
        ];
        assert_eq!(
            resolve_repo_path("org/backend", &paths),
            Some("/home/user/projects/backend".to_string()),
        );
    }

    #[test]
    fn resolve_repo_path_returns_none_when_no_match() {
        let paths = vec!["/home/user/projects/frontend".to_string()];
        assert_eq!(resolve_repo_path("org/backend", &paths), None);
    }

    #[test]
    fn resolve_repo_path_handles_empty_paths() {
        assert_eq!(resolve_repo_path("org/repo", &[]), None);
    }

    #[test]
    fn build_prompt_contains_task_info() {
        let prompt = build_prompt(TaskId(42), "Fix bug", "A nasty crash", None);
        assert!(prompt.contains("42"));
        assert!(prompt.contains("Fix bug"));
        assert!(prompt.contains("A nasty crash"));
        assert!(prompt.contains("automatically via hooks"));
    }

    #[test]
    fn build_prompt_mentions_automatic_hooks() {
        let prompt = build_prompt(TaskId(7), "Title", "Desc", None);
        assert!(prompt.contains("automatically via hooks"));
        assert!(!prompt.contains("update the task status to 'review'"));
    }

    #[test]
    fn build_prompt_mentions_wrap_up_skill() {
        let prompt = build_prompt(TaskId(7), "Title", "Desc", None);
        assert!(prompt.contains("/wrap-up"), "prompt should tell agent to use /wrap-up skill");
        assert!(prompt.contains("rebase") || prompt.contains("PR"), "prompt should mention rebase/PR choice");
    }

    #[test]
    fn build_prompt_mentions_mcp_tools() {
        let prompt = build_prompt(TaskId(1), "Task", "Desc", None);
        assert!(prompt.contains("dispatch MCP tools"), "standard dispatch prompt should mention MCP tools");
    }

    #[test]
    fn is_wrappable_running_with_worktree() {
        let task = Task {
            status: TaskStatus::Running,
            worktree: Some("/tmp/wt".to_string()),
            ..make_task("/repo")
        };
        assert!(is_wrappable(&task));
    }

    #[test]
    fn is_wrappable_review_with_worktree() {
        let task = Task {
            status: TaskStatus::Review,
            worktree: Some("/tmp/wt".to_string()),
            ..make_task("/repo")
        };
        assert!(is_wrappable(&task));
    }

    #[test]
    fn is_wrappable_running_without_worktree() {
        let task = Task {
            status: TaskStatus::Running,
            worktree: None,
            ..make_task("/repo")
        };
        assert!(!is_wrappable(&task));
    }

    #[test]
    fn is_wrappable_backlog_with_worktree() {
        let task = Task {
            status: TaskStatus::Backlog,
            worktree: Some("/tmp/wt".to_string()),
            ..make_task("/repo")
        };
        assert!(!is_wrappable(&task));
    }

    #[test]
    fn expand_tilde_with_path() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(expand_tilde("~/projects/foo"), format!("{home}/projects/foo"));
    }

    #[test]
    fn expand_tilde_bare() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(expand_tilde("~"), home);
    }

    #[test]
    fn expand_tilde_absolute_unchanged() {
        assert_eq!(expand_tilde("/home/user/foo"), "/home/user/foo");
    }

    #[test]
    fn resume_window_name_matches_dispatch() {
        // The resume window name should use the same naming convention as dispatch
        assert_eq!(build_tmux_window_name(TaskId(42)), "task-42");
    }

    #[test]
    fn build_prompt_includes_plan_path() {
        let prompt = build_prompt(TaskId(1), "Task", "Desc", Some("docs/plans/my-plan.md"));
        assert!(prompt.contains("Plan: docs/plans/my-plan.md"));
    }

    #[test]
    fn build_prompt_without_plan_omits_plan_section() {
        let prompt = build_prompt(TaskId(1), "Task", "Desc", None);
        assert!(!prompt.contains("Plan:"));
    }

    #[test]
    fn build_quick_dispatch_prompt_contains_rename_instruction() {
        let prompt = build_quick_dispatch_prompt(TaskId(42), "Quick task", "");
        assert!(prompt.contains("42"));
        assert!(prompt.contains("Quick task"));
        assert!(prompt.contains("update_task"));
        assert!(prompt.contains("title"));
        assert!(prompt.contains("placeholder"));
    }

    #[test]
    fn build_quick_dispatch_prompt_mentions_mcp() {
        let prompt = build_quick_dispatch_prompt(TaskId(1), "Quick task", "");
        assert!(prompt.contains("dispatch MCP tools"));
        assert!(prompt.contains("update_task"));
        assert!(!prompt.contains("add_note"));
    }

    #[test]
    fn build_quick_dispatch_prompt_differs_from_regular() {
        let regular = build_prompt(TaskId(1), "Task", "Desc", None);
        let quick = build_quick_dispatch_prompt(TaskId(1), "Task", "Desc");
        assert!(quick.contains("placeholder"));
        assert!(!regular.contains("placeholder"));
    }

    #[test]
    fn rebase_preamble_prepended_to_all_prompts() {
        let body = build_prompt(TaskId(1), "Task", "Desc", None);
        let full = format!("{}\n\n{body}", rebase_preamble("origin/main"));
        assert!(full.contains("rebase your branch from origin/main"));
        assert!(full.starts_with("Before starting work"));
    }

    #[test]
    fn build_brainstorm_prompt_contains_task_info() {
        let prompt = build_brainstorm_prompt(TaskId(7), "Design auth", "Rework the auth flow");
        assert!(prompt.contains("7"));
        assert!(prompt.contains("Design auth"));
        assert!(prompt.contains("Rework the auth flow"));
        assert!(prompt.contains("clarifying questions"), "brainstorm prompt should ask for clarifying questions");
        assert!(prompt.contains("brainstorm"));
        assert!(prompt.contains("update_task"));
    }

    #[test]
    fn build_plan_prompt_contains_task_info() {
        let prompt = build_plan_prompt(TaskId(8), "Add feature", "Small improvement");
        assert!(prompt.contains("8"));
        assert!(prompt.contains("Add feature"));
        assert!(prompt.contains("Small improvement"));
        assert!(prompt.contains("/plan"));
        assert!(prompt.contains("update_task"));
    }

    #[test]
    fn build_plan_prompt_differs_from_brainstorm() {
        let plan = build_plan_prompt(TaskId(1), "T", "D");
        let brainstorm = build_brainstorm_prompt(TaskId(1), "T", "D");
        assert_ne!(plan, brainstorm);
        assert!(plan.contains("planning"));
        assert!(brainstorm.contains("brainstorm"));
    }

    // --- ProcessRunner-based tests ---

    #[test]
    fn dispatch_reuses_existing_worktree() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo_path = dir.path().to_str().unwrap().to_string();
        // Pre-create worktree dir — simulates a re-dispatch where the worktree
        // already exists on disk from a previous dispatch cycle.
        let worktree_dir = dir.path().join(".worktrees").join("42-fix-bug");
        std::fs::create_dir_all(&worktree_dir).unwrap();

        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::fail("not a git repo"), // detect_default_branch (fallback to "main")
            // git worktree add is skipped (dir exists)
            MockProcessRunner::ok(), // tmux new-window
            MockProcessRunner::ok(), // tmux set-option @dispatch_dir
            MockProcessRunner::ok(), // tmux set-hook (after-split-window)
            MockProcessRunner::ok(), // tmux send-keys -l
            MockProcessRunner::ok(), // tmux send-keys Enter
        ]);

        let task = make_task(&repo_path);
        dispatch_agent(&task, &mock).unwrap();

        let calls = mock.recorded_calls();
        assert!(calls.iter().all(|(prog, args)| !(prog == "git" && args.iter().any(|a| a == "worktree"))), "git worktree add should be skipped for existing worktree");
        assert_eq!(calls[1].0, "tmux");
        assert_eq!(calls[1].1[0], "new-window");
        assert_eq!(calls[2].0, "tmux");
        assert_eq!(calls[2].1[0], "set-option");
        assert_eq!(calls[3].0, "tmux");
        assert_eq!(calls[3].1[0], "set-hook");
    }

    #[test]
    fn dispatch_sends_claude_command() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo_path = dir.path().to_str().unwrap().to_string();
        let worktree_dir = dir.path().join(".worktrees").join("42-fix-bug");
        std::fs::create_dir_all(&worktree_dir).unwrap();

        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::fail("not a git repo"), // detect_default_branch (fallback to "main")
            MockProcessRunner::ok(), // tmux new-window
            MockProcessRunner::ok(), // tmux set-option @dispatch_dir
            MockProcessRunner::ok(), // tmux set-hook (after-split-window)
            MockProcessRunner::ok(), // tmux send-keys -l (the claude command)
            MockProcessRunner::ok(), // tmux send-keys Enter
        ]);

        let task = make_task(&repo_path);
        dispatch_agent(&task, &mock).unwrap();

        let calls = mock.recorded_calls();
        // The literal send-keys call (index 4) carries the claude invocation
        assert!(
            calls[4].1.iter().any(|a| a.contains("claude")),
            "send-keys should include claude"
        );
        assert!(
            calls[4].1.iter().any(|a| a.contains("--permission-mode acceptEdits")),
            "dispatch_agent send-keys should use acceptEdits mode"
        );
    }

    #[test]
    fn dispatch_agent_uses_accept_edits_mode() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo_path = dir.path().to_str().unwrap().to_string();
        let worktree_dir = dir.path().join(".worktrees").join("42-fix-bug");
        std::fs::create_dir_all(&worktree_dir).unwrap();

        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::fail("not a git repo"), // detect_default_branch
            MockProcessRunner::ok(),                   // tmux new-window
            MockProcessRunner::ok(),                   // tmux set-option @dispatch_dir
            MockProcessRunner::ok(),                   // tmux set-hook
            MockProcessRunner::ok(),                   // tmux send-keys -l
            MockProcessRunner::ok(),                   // tmux send-keys Enter
        ]);

        let task = make_task(&repo_path);
        dispatch_agent(&task, &mock).unwrap();

        let calls = mock.recorded_calls();
        let send_keys_arg = calls[4].1.iter().find(|a| a.contains("claude")).unwrap();
        assert!(
            send_keys_arg.contains("--permission-mode acceptEdits"),
            "dispatch_agent should use acceptEdits mode, got: {send_keys_arg}"
        );
    }

    #[test]
    fn plan_agent_uses_plan_mode() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo_path = dir.path().to_str().unwrap().to_string();
        let worktree_dir = dir.path().join(".worktrees").join("42-fix-bug");
        std::fs::create_dir_all(&worktree_dir).unwrap();

        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::fail("not a git repo"), // detect_default_branch
            MockProcessRunner::ok(),                   // tmux new-window
            MockProcessRunner::ok(),                   // tmux set-option @dispatch_dir
            MockProcessRunner::ok(),                   // tmux set-hook
            MockProcessRunner::ok(),                   // tmux send-keys -l
            MockProcessRunner::ok(),                   // tmux send-keys Enter
        ]);

        let task = make_task(&repo_path);
        plan_agent(&task, &mock).unwrap();

        let calls = mock.recorded_calls();
        let send_keys_arg = calls[4].1.iter().find(|a| a.contains("claude")).unwrap();
        assert!(
            send_keys_arg.contains("--permission-mode plan"),
            "plan_agent should use plan mode, got: {send_keys_arg}"
        );
    }

    #[test]
    fn provision_worktree_creates_new_when_dir_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo_path = dir.path().to_str().unwrap().to_string();
        // Do NOT pre-create the worktree dir — test the "create" path

        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // git worktree add
            MockProcessRunner::ok(), // tmux new-window
            MockProcessRunner::ok(), // tmux set-option @dispatch_dir
            MockProcessRunner::ok(), // tmux set-hook (after-split-window)
        ]);

        let task = make_task(&repo_path);
        let result = provision_worktree(&task, &mock, None).unwrap();

        let calls = mock.recorded_calls();
        assert_eq!(calls[0].0, "git", "first call should be git worktree add");
        assert!(calls[0].1.contains(&"worktree".to_string()));
        assert!(calls[0].1.contains(&"add".to_string()));
        assert_eq!(calls[1].0, "tmux");
        assert_eq!(calls[1].1[0], "new-window");

        let expected_path = format!("{repo_path}/.worktrees/42-fix-bug");
        assert_eq!(result.worktree_path, expected_path);
    }

    #[test]
    fn provision_worktree_skips_git_when_dir_exists() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo_path = dir.path().to_str().unwrap().to_string();
        let worktree_dir = dir.path().join(".worktrees").join("42-fix-bug");
        std::fs::create_dir_all(&worktree_dir).unwrap();

        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // tmux new-window
            MockProcessRunner::ok(), // tmux set-option @dispatch_dir
            MockProcessRunner::ok(), // tmux set-hook (after-split-window)
        ]);

        let task = make_task(&repo_path);
        let result = provision_worktree(&task, &mock, None).unwrap();

        let calls = mock.recorded_calls();
        assert!(calls.iter().all(|(prog, _)| prog != "git"), "git should be skipped");
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(calls[0].1[0], "new-window");
        assert_eq!(result.worktree_path, worktree_dir.to_str().unwrap());
    }

    #[test]
    fn provision_worktree_with_base_branch_passes_start_point() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo_path = dir.path().to_str().unwrap().to_string();

        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // git worktree add (with base branch)
            MockProcessRunner::ok(), // tmux new-window
            MockProcessRunner::ok(), // tmux set-option @dispatch_dir
            MockProcessRunner::ok(), // tmux set-hook (after-split-window)
        ]);

        let task = make_task(&repo_path);
        let result = provision_worktree(&task, &mock, Some("99-prev-task")).unwrap();

        let calls = mock.recorded_calls();
        assert_eq!(calls[0].0, "git");
        // The base branch should be the last arg to git worktree add
        let git_args = &calls[0].1;
        assert_eq!(git_args.last().unwrap(), "99-prev-task",
            "base branch should be last git arg, got: {git_args:?}");

        let expected_path = format!("{repo_path}/.worktrees/42-fix-bug");
        assert_eq!(result.worktree_path, expected_path);
    }

    #[test]
    fn rebase_preamble_with_base_branch() {
        let preamble = rebase_preamble("99-prev-task");
        assert!(preamble.contains("99-prev-task"), "should reference the base branch");
        assert!(!preamble.contains("origin/main"), "should not reference origin/main");
    }

    #[test]
    fn rebase_preamble_uses_given_target() {
        let preamble = rebase_preamble("origin/develop");
        assert!(preamble.contains("origin/develop"), "should use given target, got: {preamble}");
        assert!(!preamble.contains("origin/main"), "should not contain origin/main");
    }

    #[test]
    fn resume_skips_git_issues_tmux_continue() {
        let dir = tempfile::TempDir::new().unwrap();
        let worktree_path = dir.path().to_str().unwrap().to_string();

        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // tmux new-window
            MockProcessRunner::ok(), // tmux set-option @dispatch_dir
            MockProcessRunner::ok(), // tmux set-hook (after-split-window)
            MockProcessRunner::ok(), // tmux send-keys -l
            MockProcessRunner::ok(), // tmux send-keys Enter
        ]);

        resume_agent(TaskId(42), &worktree_path, &mock).unwrap();

        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 5);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(calls[0].1[0], "new-window");
        assert_eq!(calls[1].1[0], "set-option");
        assert_eq!(calls[2].1[0], "set-hook");
        assert!(calls.iter().all(|(prog, _)| prog != "git"), "resume should make no git calls");
        assert!(calls[3].1.iter().any(|a| a.contains("--continue")));
    }

    #[test]
    fn cleanup_kills_window_and_removes_worktree() {
        let mock = MockProcessRunner::new(vec![
            // has_window: list-windows returns the window name in stdout
            MockProcessRunner::ok_with_stdout(b"task-42\n"),
            MockProcessRunner::ok(), // tmux kill-window
            MockProcessRunner::ok(), // git worktree remove
            MockProcessRunner::ok(), // git branch -D (best-effort)
        ]);

        cleanup_task("/repo", "/repo/.worktrees/42-fix-bug", Some("task-42"), &mock).unwrap();

        let calls = mock.recorded_calls();
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(calls[0].1[0], "list-windows");
        assert_eq!(calls[1].0, "tmux");
        assert_eq!(calls[1].1[0], "kill-window");
        assert_eq!(calls[2].0, "git");
        // git worktree remove is invoked with -C <repo>
        assert!(calls[2].1.contains(&"-C".to_string()));
        assert!(calls[2].1.contains(&"remove".to_string()));
    }

    #[test]
    fn cleanup_succeeds_when_worktree_already_removed() {
        // When git says "not a working tree" the archive should still succeed,
        // not surface an error to the user.
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::fail("fatal: '/repo/.worktrees/42-fix-bug' is not a working tree"),
            MockProcessRunner::ok(), // git branch -D (best-effort)
        ]);

        cleanup_task("/repo", "/repo/.worktrees/42-fix-bug", None, &mock).unwrap();
    }

    #[test]
    fn dispatch_uses_detected_default_branch() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo_path = dir.path().to_str().unwrap().to_string();
        let worktree_dir = dir.path().join(".worktrees").join("42-fix-bug");
        std::fs::create_dir_all(&worktree_dir).unwrap();

        let mock = MockProcessRunner::new(vec![
            // detect_default_branch returns "master"
            MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/master\n"),
            // git worktree add is skipped (dir exists), but provision_worktree
            // receives Some("origin/master") from dispatch_with_prompt
            MockProcessRunner::ok(), // tmux new-window
            MockProcessRunner::ok(), // tmux set-option @dispatch_dir
            MockProcessRunner::ok(), // tmux set-hook
            MockProcessRunner::ok(), // tmux send-keys -l
            MockProcessRunner::ok(), // tmux send-keys Enter
        ]);

        let task = make_task(&repo_path);
        dispatch_agent(&task, &mock).unwrap();

        // Verify the prompt uses the detected branch
        let prompt_file = worktree_dir.join(".claude-prompt");
        let prompt = std::fs::read_to_string(prompt_file).unwrap();
        assert!(prompt.contains("origin/master"), "prompt should reference origin/master, got: {prompt}");
        assert!(!prompt.contains("origin/main"), "prompt should not reference origin/main");
    }

    #[test]
    fn dispatch_fails_fast_if_git_fails() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo_path = dir.path().to_str().unwrap().to_string();

        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::fail("not a git repo"), // detect_default_branch: symbolic-ref fails → fallback to "main"
            MockProcessRunner::fail("not a git repo"), // git worktree add fails
        ]);

        let task = make_task(&repo_path);
        let result = dispatch_agent(&task, &mock);
        assert!(result.is_err());
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 2, "detect_default_branch and git worktree add should have been called");
    }

    #[test]
    fn brainstorm_reuses_existing_worktree() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo_path = dir.path().to_str().unwrap().to_string();
        let worktree_dir = dir.path().join(".worktrees").join("42-fix-bug");
        std::fs::create_dir_all(&worktree_dir).unwrap();

        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::fail("not a git repo"), // detect_default_branch (fallback to "main")
            // git worktree add is skipped (dir exists)
            MockProcessRunner::ok(), // tmux new-window
            MockProcessRunner::ok(), // tmux set-option @dispatch_dir
            MockProcessRunner::ok(), // tmux set-hook (after-split-window)
            MockProcessRunner::ok(), // tmux send-keys -l
            MockProcessRunner::ok(), // tmux send-keys Enter
        ]);

        let task = make_task(&repo_path);
        brainstorm_agent(&task, &mock).unwrap();

        let calls = mock.recorded_calls();
        assert!(calls.iter().all(|(prog, args)| !(prog == "git" && args.iter().any(|a| a == "worktree"))), "git worktree add should be skipped for existing worktree");
        assert_eq!(calls[1].0, "tmux");
        assert_eq!(calls[1].1[0], "new-window");
    }

    #[test]
    fn brainstorm_sends_brainstorm_prompt() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo_path = dir.path().to_str().unwrap().to_string();
        let worktree_dir = dir.path().join(".worktrees").join("42-fix-bug");
        std::fs::create_dir_all(&worktree_dir).unwrap();

        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::fail("not a git repo"), // detect_default_branch (fallback to "main")
            MockProcessRunner::ok(), // tmux new-window
            MockProcessRunner::ok(), // tmux set-option @dispatch_dir
            MockProcessRunner::ok(), // tmux set-hook (after-split-window)
            MockProcessRunner::ok(), // tmux send-keys -l
            MockProcessRunner::ok(), // tmux send-keys Enter
        ]);

        let task = make_task(&repo_path);
        brainstorm_agent(&task, &mock).unwrap();

        // Verify the prompt file was written with brainstorm content
        let prompt_file = worktree_dir.join(".claude-prompt");
        let prompt = std::fs::read_to_string(prompt_file).unwrap();
        assert!(prompt.contains("brainstorm"), "prompt should mention brainstorming");
        assert!(prompt.contains("implementation plan"), "prompt should mention planning");
    }

    #[test]
    fn quick_dispatch_reuses_existing_worktree() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo_path = dir.path().to_str().unwrap().to_string();
        let worktree_dir = dir.path().join(".worktrees").join("42-fix-bug");
        std::fs::create_dir_all(&worktree_dir).unwrap();

        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::fail("not a git repo"), // detect_default_branch (fallback to "main")
            // git worktree add is skipped (dir exists)
            MockProcessRunner::ok(), // tmux new-window
            MockProcessRunner::ok(), // tmux set-option @dispatch_dir
            MockProcessRunner::ok(), // tmux set-hook (after-split-window)
            MockProcessRunner::ok(), // tmux send-keys -l
            MockProcessRunner::ok(), // tmux send-keys Enter
        ]);

        let task = make_task(&repo_path);
        quick_dispatch_agent(&task, &mock).unwrap();

        let calls = mock.recorded_calls();
        assert!(calls.iter().all(|(prog, args)| !(prog == "git" && args.iter().any(|a| a == "worktree"))), "git worktree add should be skipped for existing worktree");
        assert_eq!(calls[1].0, "tmux");
        assert_eq!(calls[1].1[0], "new-window");
    }

    #[test]
    fn quick_dispatch_sends_rename_prompt() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo_path = dir.path().to_str().unwrap().to_string();
        let worktree_dir = dir.path().join(".worktrees").join("42-fix-bug");
        std::fs::create_dir_all(&worktree_dir).unwrap();

        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::fail("not a git repo"), // detect_default_branch (fallback to "main")
            MockProcessRunner::ok(), // tmux new-window
            MockProcessRunner::ok(), // tmux set-option @dispatch_dir
            MockProcessRunner::ok(), // tmux set-hook (after-split-window)
            MockProcessRunner::ok(), // tmux send-keys -l
            MockProcessRunner::ok(), // tmux send-keys Enter
        ]);

        let task = make_task(&repo_path);
        quick_dispatch_agent(&task, &mock).unwrap();

        let prompt_file = worktree_dir.join(".claude-prompt");
        let prompt = std::fs::read_to_string(prompt_file).unwrap();
        assert!(prompt.contains("placeholder"), "prompt should mention placeholder title");
        assert!(prompt.contains("update_task"), "prompt should mention update_task for rename");
    }

    // --- finish_task tests ---

    #[test]
    fn epic_planning_prompt_contains_epic_context() {
        let prompt = build_epic_planning_prompt(EpicId(42), "Redesign auth", "Rework the login flow");
        assert!(prompt.contains("42"));
        assert!(prompt.contains("Redesign auth"));
        assert!(prompt.contains("Rework the login flow"));
        assert!(prompt.contains("Do NOT start implementing"));
        // Work package instructions
        assert!(prompt.contains("create_task"), "prompt should instruct using create_task");
        assert!(prompt.contains("sort_order"), "prompt should explain sort_order for ordering");
        assert!(prompt.contains("update_epic"), "prompt should instruct attaching plan to epic");
        assert!(prompt.contains("repo_path"), "prompt should explain repo_path for parallelization");
        assert!(prompt.contains("epic_id=42"), "update_epic call should include the resolved epic id");
        assert!(prompt.contains("/brainstorm"), "prompt should direct agent to use the brainstorm skill");
        assert!(prompt.contains("work package"), "prompt should use 'work package' terminology");
    }

    #[test]
    fn finish_task_happy_path() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/main\n"), // symbolic-ref (detect default branch)
            MockProcessRunner::ok_with_stdout(b"main\n"),       // rev-parse HEAD
            MockProcessRunner::ok_with_stdout(b"git@github.com:org/repo.git\n"), // remote get-url origin
            MockProcessRunner::ok(),                             // git pull origin main
            MockProcessRunner::ok(),                             // git rebase main (from worktree)
            MockProcessRunner::ok(),                             // git merge --ff-only (fast-forward main)
            // Only tmux kill (worktree preserved for archival):
            MockProcessRunner::ok_with_stdout(b"task-42\n"),     // tmux list-windows (has_window)
            MockProcessRunner::ok(),                             // tmux kill-window
        ]);

        finish_task("/repo", "/repo/.worktrees/42-fix-bug", "42-fix-bug", Some("task-42"), &mock).unwrap();

        let calls = mock.recorded_calls();
        assert!(calls.iter().any(|c| c.1.contains(&"rebase".to_string())));
        assert!(calls.iter().any(|c| c.1.contains(&"--ff-only".to_string())));
        // No worktree removal
        assert!(!calls.iter().any(|c| c.1.contains(&"remove".to_string())));
    }

    #[test]
    fn finish_task_with_master_default_branch() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/master\n"), // symbolic-ref → master
            MockProcessRunner::ok_with_stdout(b"master\n"),      // rev-parse HEAD
            MockProcessRunner::ok_with_stdout(b"git@github.com:org/repo.git\n"), // remote get-url origin
            MockProcessRunner::ok(),                              // git pull origin master
            MockProcessRunner::ok(),                              // git rebase master (from worktree)
            MockProcessRunner::ok(),                              // git merge --ff-only
        ]);

        finish_task("/repo", "/repo/.worktrees/42-fix-bug", "42-fix-bug", None, &mock).unwrap();

        let calls = mock.recorded_calls();
        // pull should reference "master" not "main"
        let pull_call = calls.iter().find(|c| c.1.contains(&"pull".to_string())).unwrap();
        assert!(pull_call.1.contains(&"master".to_string()));
        // rebase should reference "master"
        let rebase_call = calls.iter().find(|c| c.1.contains(&"rebase".to_string())).unwrap();
        assert!(rebase_call.1.contains(&"master".to_string()));
    }

    #[test]
    fn finish_task_not_on_default_branch() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/main\n"), // symbolic-ref
            MockProcessRunner::ok_with_stdout(b"feature-branch\n"),            // rev-parse HEAD
        ]);

        let result = finish_task("/repo", "/repo/.worktrees/42-fix-bug", "42-fix-bug", None, &mock);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, FinishError::NotOnDefaultBranch { .. }));
        assert!(err.to_string().contains("feature-branch"));
    }

    #[test]
    fn finish_task_rebase_conflict() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/main\n"), // symbolic-ref
            MockProcessRunner::ok_with_stdout(b"main\n"),
            MockProcessRunner::fail(""),                         // remote get-url (no remote)
            Ok(Output {
                status: exit_fail(),
                stdout: b"".to_vec(),
                stderr: b"CONFLICT (content): Merge conflict in src/main.rs\nerror: could not apply abc1234\n".to_vec(),
            }),
            MockProcessRunner::ok(),                             // git rebase --abort
        ]);

        let result = finish_task("/repo", "/repo/.worktrees/42-fix-bug", "42-fix-bug", None, &mock);
        assert!(matches!(result.unwrap_err(), FinishError::RebaseConflict(_)));
        let calls = mock.recorded_calls();
        assert!(calls.last().unwrap().1.contains(&"--abort".to_string()));
    }

    #[test]
    fn finish_task_pull_fails() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/main\n"), // symbolic-ref
            MockProcessRunner::ok_with_stdout(b"main\n"),
            MockProcessRunner::ok_with_stdout(b"git@github.com:org/repo.git\n"), // remote get-url origin
            MockProcessRunner::fail("fatal: unable to access remote"),            // git pull fails
        ]);

        let result = finish_task("/repo", "/repo/.worktrees/42-fix-bug", "42-fix-bug", None, &mock);
        assert!(matches!(result.unwrap_err(), FinishError::Other(_)));
    }

    // --- create_pr tests ---

    #[test]
    fn create_pr_happy_path() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/main\n"), // symbolic-ref
            MockProcessRunner::ok(),  // git push
            MockProcessRunner::ok_with_stdout(b"git@github.com:org/repo.git\n"),  // git remote get-url origin
            MockProcessRunner::ok_with_stdout(b"https://github.com/org/repo/pull/42\n"),  // gh pr create
        ]);

        let result = create_pr("/repo", "42-fix-bug", "Fix bug", "A nasty crash", &mock).unwrap();
        assert_eq!(result.pr_url, "https://github.com/org/repo/pull/42");

        let calls = mock.recorded_calls();
        assert_eq!(calls[1].0, "git");
        assert!(calls[1].1.contains(&"push".to_string()));
        assert!(calls[1].1.contains(&"-u".to_string()));
        assert_eq!(calls[2].0, "git");
        assert!(calls[2].1.contains(&"get-url".to_string()));
        assert_eq!(calls[3].0, "gh");
        assert!(calls[3].1.contains(&"create".to_string()));
        assert!(calls[3].1.contains(&"--draft".to_string()));
        assert!(calls[3].1.contains(&"org/repo".to_string()));
    }

    #[test]
    fn create_pr_with_master_base() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/master\n"), // symbolic-ref → master
            MockProcessRunner::ok(),  // git push
            MockProcessRunner::ok_with_stdout(b"git@github.com:org/repo.git\n"),  // git remote get-url origin
            MockProcessRunner::ok_with_stdout(b"https://github.com/org/repo/pull/42\n"),  // gh pr create
        ]);

        create_pr("/repo", "42-fix-bug", "Fix bug", "desc", &mock).unwrap();

        let calls = mock.recorded_calls();
        let gh_call = calls.iter().find(|c| c.0 == "gh").unwrap();
        assert!(gh_call.1.contains(&"master".to_string()));
    }

    #[test]
    fn create_pr_push_fails() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/main\n"), // symbolic-ref
            MockProcessRunner::fail("fatal: no remote"),
        ]);

        let result = create_pr("/repo", "42-fix-bug", "Fix bug", "desc", &mock);
        assert!(matches!(result, Err(PrError::PushFailed(_))));
    }

    #[test]
    fn create_pr_gh_create_fails() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/main\n"), // symbolic-ref
            MockProcessRunner::ok(),  // git push succeeds
            MockProcessRunner::ok_with_stdout(b"git@github.com:org/repo.git\n"),  // git remote get-url
            MockProcessRunner::fail("error: pull request already exists"),  // gh pr create
        ]);

        let result = create_pr("/repo", "42-fix-bug", "Fix bug", "desc", &mock);
        assert!(matches!(result, Err(PrError::CreateFailed(_))));
    }

    // --- check_pr_status tests ---

    #[test]
    fn check_pr_status_open() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"OPEN\nREVIEW_REQUIRED\n"),
        ]);
        let result = check_pr_status("https://github.com/org/repo/pull/42", &mock).unwrap();
        assert_eq!(result.state, PrState::Open);
        assert_eq!(result.review_decision, Some(PrReviewDecision::ReviewRequired));
    }

    #[test]
    fn check_pr_status_merged() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"MERGED\n"),
        ]);
        let result = check_pr_status("https://github.com/org/repo/pull/42", &mock).unwrap();
        assert_eq!(result.state, PrState::Merged);
        assert_eq!(result.review_decision, None);
    }

    #[test]
    fn check_pr_status_closed() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"CLOSED\n"),
        ]);
        let result = check_pr_status("https://github.com/org/repo/pull/42", &mock).unwrap();
        assert_eq!(result.state, PrState::Closed);
        assert_eq!(result.review_decision, None);
    }

    #[test]
    fn check_pr_status_open_approved() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"OPEN\nAPPROVED\n"),
        ]);
        let result = check_pr_status("https://github.com/org/repo/pull/42", &mock).unwrap();
        assert_eq!(result.state, PrState::Open);
        assert_eq!(result.review_decision, Some(PrReviewDecision::Approved));
    }

    #[test]
    fn check_pr_status_open_changes_requested() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"OPEN\nCHANGES_REQUESTED\n"),
        ]);
        let result = check_pr_status("https://github.com/org/repo/pull/42", &mock).unwrap();
        assert_eq!(result.state, PrState::Open);
        assert_eq!(result.review_decision, Some(PrReviewDecision::ChangesRequested));
    }

    // --- parse_repo_slug tests ---

    #[test]
    fn parse_repo_slug_ssh() {
        assert_eq!(
            parse_repo_slug("git@github.com:org/repo.git"),
            Some("org/repo".to_string()),
        );
    }

    #[test]
    fn parse_repo_slug_https() {
        assert_eq!(
            parse_repo_slug("https://github.com/org/repo.git"),
            Some("org/repo".to_string()),
        );
    }

    #[test]
    fn parse_repo_slug_no_git_suffix() {
        assert_eq!(
            parse_repo_slug("https://github.com/org/repo"),
            Some("org/repo".to_string()),
        );
    }

    #[test]
    fn finish_task_no_remote_skips_pull() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::fail(""),                         // symbolic-ref (no remote → fallback to "main")
            MockProcessRunner::ok_with_stdout(b"main\n"),       // rev-parse HEAD
            MockProcessRunner::fail(""),                         // remote get-url (no remote)
            MockProcessRunner::ok(),                             // git rebase main (from worktree)
            MockProcessRunner::ok(),                             // git merge --ff-only (fast-forward)
            // No tmux window, no cleanup
        ]);

        finish_task("/repo", "/repo/.worktrees/42-fix-bug", "42-fix-bug", None, &mock).unwrap();
        let calls = mock.recorded_calls();
        // Should not have a "pull" call
        assert!(!calls.iter().any(|c| c.1.contains(&"pull".to_string())));
    }

    // --- dispatch_review_agent tests ---

    #[test]
    fn review_agent_returns_early_when_window_exists() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo_path = dir.path().to_str().unwrap().to_string();
        let repo_short = dir.path().file_name().unwrap().to_str().unwrap();
        let tmux_window = format!("review-{repo_short}-99");

        let mock = MockProcessRunner::new(vec![
            // has_window: list-windows returns the window name
            MockProcessRunner::ok_with_stdout(tmux_window.as_bytes()),
        ]);

        let result = dispatch_review_agent(&repo_path, 99, "Fix it", "body", "feature-branch", &mock).unwrap();

        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1, "only list-windows should be called");
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(calls[0].1[0], "list-windows");
        assert_eq!(result.tmux_window, tmux_window);
        let expected_worktree = format!("{repo_path}/.worktrees/review-99");
        assert_eq!(result.worktree_path, expected_worktree);
    }

    #[test]
    fn review_agent_skips_worktree_add_when_dir_exists() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo_path = dir.path().to_str().unwrap().to_string();
        // Pre-create the review worktree directory
        let worktree_dir = dir.path().join(".worktrees").join("review-99");
        std::fs::create_dir_all(&worktree_dir).unwrap();

        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"other-window\n"), // has_window: no match
            MockProcessRunner::ok(),                              // git fetch origin feature-branch
            // git worktree add is skipped (dir exists)
            MockProcessRunner::ok(),                              // tmux new-window
            MockProcessRunner::ok(),                              // tmux send-keys -l
            MockProcessRunner::ok(),                              // tmux send-keys Enter
        ]);

        let result = dispatch_review_agent(&repo_path, 99, "Fix it", "desc", "feature-branch", &mock).unwrap();

        let calls = mock.recorded_calls();
        assert!(
            calls.iter().all(|(prog, args)| !(prog == "git" && args.iter().any(|a| a == "worktree"))),
            "git worktree add should be skipped when dir exists"
        );
        // git fetch should still happen
        assert_eq!(calls[1].0, "git");
        assert!(calls[1].1.contains(&"fetch".to_string()));
        assert!(calls[1].1.contains(&"feature-branch".to_string()));
        assert_eq!(result.worktree_path, worktree_dir.to_str().unwrap());
    }

    #[test]
    fn review_agent_happy_path_writes_prompt_and_launches_claude() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo_path = dir.path().to_str().unwrap().to_string();
        // Pre-create worktree dir (simulates a previous fetch or existing
        // worktree — the mock git worktree add can't create dirs on disk)
        let worktree_dir = dir.path().join(".worktrees").join("review-99");
        std::fs::create_dir_all(&worktree_dir).unwrap();

        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"other-window\n"), // has_window: no match
            MockProcessRunner::ok(),                              // git fetch origin feature-branch
            // git worktree add skipped (dir exists)
            MockProcessRunner::ok(),                              // tmux new-window
            MockProcessRunner::ok(),                              // tmux send-keys -l
            MockProcessRunner::ok(),                              // tmux send-keys Enter
        ]);

        let result = dispatch_review_agent(&repo_path, 99, "Fix it", "PR body here", "feature-branch", &mock).unwrap();

        let calls = mock.recorded_calls();
        // Verify git fetch
        assert_eq!(calls[1].0, "git");
        assert!(calls[1].1.contains(&"fetch".to_string()));
        assert!(calls[1].1.contains(&"feature-branch".to_string()));
        // Verify tmux new-window
        assert_eq!(calls[2].0, "tmux");
        assert_eq!(calls[2].1[0], "new-window");
        // Verify send-keys includes claude command
        assert!(
            calls[3].1.iter().any(|a| a.contains("claude")),
            "send-keys should include claude command"
        );

        // Verify prompt file content
        let prompt_file = worktree_dir.join(".claude-prompt");
        let prompt = std::fs::read_to_string(prompt_file).unwrap();
        assert!(prompt.contains("PR #99"), "prompt should reference PR number");
        assert!(prompt.contains("Fix it"), "prompt should include PR title");
        assert!(prompt.contains("PR body here"), "prompt should include PR body");
        assert!(prompt.contains("gh pr diff 99"), "prompt should instruct reading the diff");
        assert!(prompt.contains("gh pr review 99"), "prompt should instruct submitting review");

        let repo_short = dir.path().file_name().unwrap().to_str().unwrap();
        assert_eq!(result.tmux_window, format!("review-{repo_short}-99"));
    }

    #[test]
    fn review_agent_calls_worktree_add_when_dir_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo_path = dir.path().to_str().unwrap().to_string();
        // Do NOT pre-create the review worktree directory

        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"other-window\n"), // has_window: no match
            MockProcessRunner::ok(),                              // git fetch origin feature-branch
            MockProcessRunner::ok(),                              // git worktree add
            MockProcessRunner::ok(),                              // tmux new-window
            // fs::write will fail (mock worktree add doesn't create dir),
            // but we can still verify the calls made so far
        ]);

        // The function will error at fs::write since mock doesn't create the dir
        let result = dispatch_review_agent(&repo_path, 99, "Fix it", "body", "feature-branch", &mock);
        assert!(result.is_err());

        let calls = mock.recorded_calls();
        // Verify git worktree add was called with correct args
        let wt_call = calls.iter().find(|(prog, args)| prog == "git" && args.contains(&"worktree".to_string()));
        assert!(wt_call.is_some(), "git worktree add should be called when dir is missing");
        let (_, args) = wt_call.unwrap();
        assert!(args.contains(&"add".to_string()));
        assert!(args.iter().any(|a| a == "origin/feature-branch"));
    }

    #[test]
    fn review_agent_fails_when_git_fetch_fails() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo_path = dir.path().to_str().unwrap().to_string();

        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"\n"),             // has_window: no match
            MockProcessRunner::fail("fatal: couldn't find remote ref"), // git fetch fails
        ]);

        let result = dispatch_review_agent(&repo_path, 99, "Fix it", "body", "nonexistent", &mock);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("git fetch failed"));
    }

    #[test]
    fn review_agent_uses_accept_edits_mode() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo_path = dir.path().to_str().unwrap().to_string();
        let worktree_dir = dir.path().join(".worktrees").join("review-99");
        std::fs::create_dir_all(&worktree_dir).unwrap();

        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"\n"),             // has_window: no match
            MockProcessRunner::ok(),                              // git fetch
            // worktree exists, skip add
            MockProcessRunner::ok(),                              // tmux new-window
            MockProcessRunner::ok(),                              // tmux send-keys -l
            MockProcessRunner::ok(),                              // tmux send-keys Enter
        ]);

        dispatch_review_agent(&repo_path, 99, "Fix it", "body", "feature-branch", &mock).unwrap();

        let calls = mock.recorded_calls();
        let send_keys_arg = calls[3].1.iter().find(|a| a.contains("claude")).unwrap();
        assert!(
            send_keys_arg.contains("--permission-mode acceptEdits"),
            "review agent should use acceptEdits mode, got: {send_keys_arg}"
        );
    }
}

#[cfg(test)]
mod branch_tests {
    use super::*;

    #[test]
    fn branch_from_worktree_extracts_last_component() {
        assert_eq!(
            branch_from_worktree("/repo/.worktrees/42-fix-login"),
            Some("42-fix-login".to_string())
        );
    }

    #[test]
    fn branch_from_worktree_returns_none_for_empty() {
        assert_eq!(branch_from_worktree(""), None);
    }
}
