use anyhow::{Context, Result};
use std::fs;

use crate::models::{DispatchResult, EpicId, ResumeResult, Task, TaskId, slugify};
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
fn provision_worktree(task: &Task, runner: &dyn ProcessRunner) -> Result<ProvisionResult> {
    let repo_path = expand_tilde(&task.repo_path);
    let slug = slugify(&task.title);
    let worktree_name = format!("{}-{slug}", task.id);
    let worktree_path = format!("{repo_path}/.worktrees/{worktree_name}");
    let tmux_window = build_tmux_window_name(task.id);

    tracing::info!(task_id = task.id.0, %worktree_path, "provisioning worktree");

    fs::create_dir_all(format!("{repo_path}/.worktrees"))
        .context("failed to create .worktrees directory")?;

    let output = runner
        .run(
            "git",
            &["-C", &repo_path, "worktree", "add", &worktree_path, "-B", &worktree_name],
        )
        .context("failed to run git worktree add")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let msg = stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or(stderr.trim());
        anyhow::bail!("git worktree add failed: {msg}");
    }

    tmux::new_window(&tmux_window, &worktree_path, runner)
        .context("failed to create tmux window")?;

    tmux::set_after_split_hook(&tmux_window, &worktree_path, runner)
        .context("failed to set tmux split hook")?;

    Ok(ProvisionResult { worktree_path, tmux_window })
}

/// Provision worktree, write prompt file, launch Claude via tmux.
/// Shared by all dispatch variants.
fn dispatch_with_prompt(
    task: &Task,
    prompt: &str,
    runner: &dyn ProcessRunner,
) -> Result<DispatchResult> {
    let provision = provision_worktree(task, runner)?;

    let prompt_file = format!("{}/.claude-prompt", provision.worktree_path);
    fs::write(&prompt_file, prompt)
        .with_context(|| format!("failed to write {prompt_file}"))?;
    tmux::send_keys(
        &provision.tmux_window,
        "claude \"$(cat .claude-prompt)\"",
        runner,
    )
    .context("failed to send keys to tmux window")?;

    tracing::info!(task_id = task.id.0, worktree = %provision.worktree_path, "agent dispatched");

    Ok(DispatchResult {
        worktree_path: provision.worktree_path,
        tmux_window: provision.tmux_window,
    })
}

pub fn dispatch_agent(task: &Task, runner: &dyn ProcessRunner) -> Result<DispatchResult> {
    let prompt = build_prompt(task.id, &task.title, &task.description, task.plan.as_deref());
    dispatch_with_prompt(task, &prompt, runner)
}

pub fn brainstorm_agent(task: &Task, mcp_port: u16, runner: &dyn ProcessRunner) -> Result<DispatchResult> {
    let prompt = build_brainstorm_prompt(task.id, &task.title, &task.description, mcp_port);
    dispatch_with_prompt(task, &prompt, runner)
}

pub fn quick_dispatch_agent(task: &Task, mcp_port: u16, runner: &dyn ProcessRunner) -> Result<DispatchResult> {
    let prompt = build_quick_dispatch_prompt(task.id, &task.title, &task.description, mcp_port);
    dispatch_with_prompt(task, &prompt, runner)
}

pub fn epic_planning_agent(task: &Task, epic_id: EpicId, epic_title: &str, epic_description: &str, mcp_port: u16, runner: &dyn ProcessRunner) -> Result<DispatchResult> {
    let prompt = build_epic_planning_prompt(epic_id, epic_title, epic_description, mcp_port);
    dispatch_with_prompt(task, &prompt, runner)
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

    let output = runner
        .run("git", &["worktree", "remove", "--force", worktree_path])
        .context("failed to run git worktree remove")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "git worktree remove failed for path {worktree_path}: {}",
            stderr.trim()
        );
    }

    if let Some(branch) = std::path::Path::new(worktree_path)
        .file_name()
        .and_then(|n| n.to_str())
    {
        // Best-effort: ignore errors (branch may not exist).
        let _ = runner.run("git", &["-C", repo_path, "branch", "-D", branch]);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// finish_task
// ---------------------------------------------------------------------------

/// Errors from the finish (rebase + cleanup) operation.
#[derive(Debug)]
pub enum FinishError {
    NotOnMain(String),
    RebaseConflict(String),
    Other(String),
}

impl std::fmt::Display for FinishError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FinishError::NotOnMain(branch) => write!(
                f,
                "Repo root is not on main (currently on {branch}) — checkout main first"
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

    // 1. Verify we're on main
    let output = runner
        .run("git", &["-C", repo_path, "rev-parse", "--abbrev-ref", "HEAD"])
        .map_err(|e| FinishError::Other(format!("Failed to check current branch: {e}")))?;
    let current_branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if current_branch != "main" {
        return Err(FinishError::NotOnMain(current_branch));
    }

    // 2. Pull latest main (skip if no remote configured)
    let has_remote = runner
        .run("git", &["-C", repo_path, "remote", "get-url", "origin"])
        .map(|o| o.status.success())
        .unwrap_or(false);

    if has_remote {
        let output = runner
            .run("git", &["-C", repo_path, "pull", "origin", "main"])
            .map_err(|e| FinishError::Other(format!("Failed to pull: {e}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(FinishError::Other(format!(
                "Failed to pull main: {}",
                stderr.trim()
            )));
        }
    }

    // 3. Rebase branch onto main (from worktree, where branch is checked out)
    let output = runner
        .run("git", &["-C", worktree, "rebase", "main"])
        .map_err(|e| FinishError::Other(format!("Failed to run git rebase: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
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
            stderr.trim()
        )));
    }

    // 4. Fast-forward main to the rebased branch
    let output = runner
        .run("git", &["-C", repo_path, "merge", "--ff-only", branch])
        .map_err(|e| FinishError::Other(format!("Failed to fast-forward main: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(FinishError::Other(format!(
            "Fast-forward failed after rebase: {}",
            stderr.trim()
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

    tmux::set_after_split_hook(&tmux_window, worktree_path, runner)
        .context("failed to set tmux split hook")?;

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
Do not call update_task for status changes."
    )
}

fn build_quick_dispatch_prompt(task_id: TaskId, title: &str, description: &str, mcp_port: u16) -> String {
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
An MCP server is available at http://localhost:{mcp_port}/mcp — use it to \
query and update tasks (tool: dispatch). Use update_task to rename \
this task with a descriptive title, and get_task to check current state."
    )
}

fn build_brainstorm_prompt(task_id: TaskId, title: &str, description: &str, mcp_port: u16) -> String {
    format!(
        "You are an autonomous coding agent starting a brainstorming session.\n\
\n\
Task:\n\
  ID: {task_id}\n\
  Title: {title}\n\
  Description: {description}\n\
\n\
Your goal is to explore the codebase, brainstorm approaches, and write an \
implementation plan. When done, save the plan and attach it to the task:\n\
\n\
1. Write the plan to docs/plans/ (or docs/superpowers/specs/ if using the brainstorming skill)\n\
2. Call update_task via MCP to set the plan field to the plan file path\n\
\n\
After planning, ask whether to continue implementing or stop.\n\
\n\
An MCP server is available at http://localhost:{mcp_port}/mcp — use it to \
attach the plan (tool: dispatch, tool name: update_task — set the plan field)."
    )
}

fn build_epic_planning_prompt(epic_id: EpicId, title: &str, description: &str, mcp_port: u16) -> String {
    format!(
        "You are an autonomous coding agent starting a brainstorming session.\n\
\n\
Epic:\n\
  ID: {epic_id}\n\
  Title: {title}\n\
  Description: {description}\n\
\n\
Your goal is to explore the codebase, brainstorm approaches, and write an \
implementation plan for this epic. When done, save the plan to docs/plans/.\n\
\n\
After planning, ask whether to continue creating subtasks or stop.\n\
\n\
An MCP server is available at http://localhost:{mcp_port}/mcp — use it to \
query tasks and epics (tool: dispatch).\n\
\n\
IMPORTANT: Do NOT start implementing. Your job ends after planning.",
        epic_id = epic_id,
        title = title,
        description = description,
        mcp_port = mcp_port,
    )
}

// ---------------------------------------------------------------------------
// PR types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PrResult {
    pub pr_url: String,
    pub pr_number: i64,
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

    // 1. Push the branch
    let output = runner
        .run("git", &["-C", repo_path, "push", "-u", "origin", branch])
        .map_err(|e| PrError::PushFailed(format!("Failed to run git push: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(PrError::PushFailed(stderr.trim().to_string()));
    }

    // 2. Get the repo slug from git remote
    let remote_output = runner
        .run("git", &["-C", repo_path, "remote", "get-url", "origin"])
        .map_err(|e| PrError::Other(format!("Failed to get remote URL: {e}")))?;
    let remote_url = String::from_utf8_lossy(&remote_output.stdout).trim().to_string();
    let repo_slug = parse_repo_slug(&remote_url)
        .ok_or_else(|| PrError::Other(format!("Could not parse repo slug from: {remote_url}")))?;

    // 3. Create the PR
    let output = runner
        .run("gh", &[
            "pr", "create",
            "--title", title,
            "--body", description,
            "--head", branch,
            "--base", "main",
            "--repo", &repo_slug,
        ])
        .map_err(|e| PrError::Other(format!("Failed to run gh: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(PrError::CreateFailed(stderr.trim().to_string()));
    }

    // 4. Parse the PR URL from stdout
    let pr_url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let pr_number = pr_url
        .rsplit('/')
        .next()
        .and_then(|s| s.parse::<i64>().ok())
        .ok_or_else(|| PrError::Other(format!("Could not parse PR number from: {pr_url}")))?;

    Ok(PrResult { pr_url, pr_number })
}

/// Check the current status of a PR using `gh pr view`.
pub fn check_pr_status(
    pr_number: i64,
    repo_path: &str,
    runner: &dyn ProcessRunner,
) -> Result<PrState> {
    let repo_path = &expand_tilde(repo_path);

    // Get repo slug from git remote
    let remote_output = runner
        .run("git", &["-C", repo_path, "remote", "get-url", "origin"])
        .context("Failed to get remote URL")?;
    let remote_url = String::from_utf8_lossy(&remote_output.stdout).trim().to_string();
    let repo_slug = parse_repo_slug(&remote_url)
        .ok_or_else(|| anyhow::anyhow!("Could not parse repo slug from: {remote_url}"))?;

    let pr_num_str = pr_number.to_string();
    let output = runner
        .run("gh", &[
            "pr", "view", &pr_num_str,
            "--repo", &repo_slug,
            "--json", "state",
            "-q", ".state",
        ])
        .context("Failed to run gh pr view")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("gh pr view failed: {}", stderr.trim());
    }

    let state = String::from_utf8_lossy(&output.stdout).trim().to_uppercase();
    match state.as_str() {
        "MERGED" => Ok(PrState::Merged),
        "CLOSED" => Ok(PrState::Closed),
        _ => Ok(PrState::Open),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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
            needs_input: false,
            pr_url: None,
            pr_number: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
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
        let prompt = build_quick_dispatch_prompt(TaskId(42), "Quick task", "", 3142);
        assert!(prompt.contains("42"));
        assert!(prompt.contains("Quick task"));
        assert!(prompt.contains("update_task"));
        assert!(prompt.contains("title"));
        assert!(prompt.contains("placeholder"));
    }

    #[test]
    fn build_quick_dispatch_prompt_mentions_mcp() {
        let prompt = build_quick_dispatch_prompt(TaskId(1), "Quick task", "", 3142);
        assert!(prompt.contains("3142"));
        assert!(prompt.contains("update_task"));
        assert!(!prompt.contains("add_note"));
    }

    #[test]
    fn build_quick_dispatch_prompt_differs_from_regular() {
        let regular = build_prompt(TaskId(1), "Task", "Desc", None);
        let quick = build_quick_dispatch_prompt(TaskId(1), "Task", "Desc", 3142);
        assert!(quick.contains("placeholder"));
        assert!(!regular.contains("placeholder"));
    }

    #[test]
    fn build_brainstorm_prompt_contains_task_info() {
        let prompt = build_brainstorm_prompt(TaskId(7), "Design auth", "Rework the auth flow", 3142);
        assert!(prompt.contains("7"));
        assert!(prompt.contains("Design auth"));
        assert!(prompt.contains("Rework the auth flow"));
        assert!(prompt.contains("3142"));
        assert!(prompt.contains("brainstorm"));
        assert!(prompt.contains("update_task"));
    }

    // --- ProcessRunner-based tests ---

    #[test]
    fn dispatch_creates_worktree_then_opens_tmux() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo_path = dir.path().to_str().unwrap().to_string();
        // Pre-create worktree dir so fs::write for the prompt succeeds
        // (git is mocked and won't create it).
        let worktree_dir = dir.path().join(".worktrees").join("42-fix-bug");
        std::fs::create_dir_all(&worktree_dir).unwrap();

        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // git worktree add
            MockProcessRunner::ok(), // tmux new-window
            MockProcessRunner::ok(), // tmux set-hook (after-split-window)
            MockProcessRunner::ok(), // tmux send-keys -l
            MockProcessRunner::ok(), // tmux send-keys Enter
        ]);

        let task = make_task(&repo_path);
        dispatch_agent(&task, &mock).unwrap();

        let calls = mock.recorded_calls();
        assert_eq!(calls[0].0, "git", "first call should be git");
        assert!(calls[0].1.contains(&"worktree".to_string()));
        assert!(calls[0].1.contains(&"add".to_string()));
        assert_eq!(calls[1].0, "tmux");
        assert_eq!(calls[1].1[0], "new-window");
        assert_eq!(calls[2].0, "tmux");
        assert_eq!(calls[2].1[0], "set-hook");
    }

    #[test]
    fn dispatch_sends_claude_command() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo_path = dir.path().to_str().unwrap().to_string();
        let worktree_dir = dir.path().join(".worktrees").join("42-fix-bug");
        std::fs::create_dir_all(&worktree_dir).unwrap();

        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // git worktree add
            MockProcessRunner::ok(), // tmux new-window
            MockProcessRunner::ok(), // tmux set-hook (after-split-window)
            MockProcessRunner::ok(), // tmux send-keys -l (the claude command)
            MockProcessRunner::ok(), // tmux send-keys Enter
        ]);

        let task = make_task(&repo_path);
        dispatch_agent(&task, &mock).unwrap();

        let calls = mock.recorded_calls();
        // The literal send-keys call (index 3) carries the claude invocation
        assert!(
            calls[3].1.iter().any(|a| a.contains("claude")),
            "send-keys should include claude"
        );
    }

    #[test]
    fn resume_skips_git_issues_tmux_continue() {
        let dir = tempfile::TempDir::new().unwrap();
        let worktree_path = dir.path().to_str().unwrap().to_string();

        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // tmux new-window
            MockProcessRunner::ok(), // tmux set-hook (after-split-window)
            MockProcessRunner::ok(), // tmux send-keys -l
            MockProcessRunner::ok(), // tmux send-keys Enter
        ]);

        resume_agent(TaskId(42), &worktree_path, &mock).unwrap();

        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 4);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(calls[0].1[0], "new-window");
        assert_eq!(calls[1].1[0], "set-hook");
        assert!(calls.iter().all(|(prog, _)| prog != "git"), "resume should make no git calls");
        assert!(calls[2].1.iter().any(|a| a.contains("--continue")));
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
        assert!(calls[2].1.contains(&"remove".to_string()));
    }

    #[test]
    fn dispatch_fails_fast_if_git_fails() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo_path = dir.path().to_str().unwrap().to_string();

        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::fail("not a git repo"), // git worktree add fails
        ]);

        let task = make_task(&repo_path);
        let result = dispatch_agent(&task, &mock);
        assert!(result.is_err());
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1, "only the git call should have been made");
    }

    #[test]
    fn brainstorm_creates_worktree_then_opens_tmux() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo_path = dir.path().to_str().unwrap().to_string();
        let worktree_dir = dir.path().join(".worktrees").join("42-fix-bug");
        std::fs::create_dir_all(&worktree_dir).unwrap();

        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // git worktree add
            MockProcessRunner::ok(), // tmux new-window
            MockProcessRunner::ok(), // tmux set-hook (after-split-window)
            MockProcessRunner::ok(), // tmux send-keys -l
            MockProcessRunner::ok(), // tmux send-keys Enter
        ]);

        let task = make_task(&repo_path);
        brainstorm_agent(&task, 3142, &mock).unwrap();

        let calls = mock.recorded_calls();
        assert_eq!(calls[0].0, "git", "first call should be git");
        assert!(calls[0].1.contains(&"worktree".to_string()));
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
            MockProcessRunner::ok(), // git worktree add
            MockProcessRunner::ok(), // tmux new-window
            MockProcessRunner::ok(), // tmux set-hook (after-split-window)
            MockProcessRunner::ok(), // tmux send-keys -l
            MockProcessRunner::ok(), // tmux send-keys Enter
        ]);

        let task = make_task(&repo_path);
        brainstorm_agent(&task, 3142, &mock).unwrap();

        // Verify the prompt file was written with brainstorm content
        let prompt_file = worktree_dir.join(".claude-prompt");
        let prompt = std::fs::read_to_string(prompt_file).unwrap();
        assert!(prompt.contains("brainstorm"), "prompt should mention brainstorming");
        assert!(prompt.contains("implementation plan"), "prompt should mention planning");
    }

    #[test]
    fn quick_dispatch_creates_worktree_then_opens_tmux() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo_path = dir.path().to_str().unwrap().to_string();
        let worktree_dir = dir.path().join(".worktrees").join("42-fix-bug");
        std::fs::create_dir_all(&worktree_dir).unwrap();

        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // git worktree add
            MockProcessRunner::ok(), // tmux new-window
            MockProcessRunner::ok(), // tmux set-hook (after-split-window)
            MockProcessRunner::ok(), // tmux send-keys -l
            MockProcessRunner::ok(), // tmux send-keys Enter
        ]);

        let task = make_task(&repo_path);
        quick_dispatch_agent(&task, 3142, &mock).unwrap();

        let calls = mock.recorded_calls();
        assert_eq!(calls[0].0, "git");
        assert!(calls[0].1.contains(&"worktree".to_string()));
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
            MockProcessRunner::ok(), // git worktree add
            MockProcessRunner::ok(), // tmux new-window
            MockProcessRunner::ok(), // tmux set-hook (after-split-window)
            MockProcessRunner::ok(), // tmux send-keys -l
            MockProcessRunner::ok(), // tmux send-keys Enter
        ]);

        let task = make_task(&repo_path);
        quick_dispatch_agent(&task, 3142, &mock).unwrap();

        let prompt_file = worktree_dir.join(".claude-prompt");
        let prompt = std::fs::read_to_string(prompt_file).unwrap();
        assert!(prompt.contains("placeholder"), "prompt should mention placeholder title");
        assert!(prompt.contains("update_task"), "prompt should mention update_task for rename");
    }

    // --- finish_task tests ---

    #[test]
    fn epic_planning_prompt_contains_epic_context() {
        let prompt = build_epic_planning_prompt(EpicId(42), "Redesign auth", "Rework the login flow", 3142);
        assert!(prompt.contains("42"));
        assert!(prompt.contains("Redesign auth"));
        assert!(prompt.contains("Rework the login flow"));
        assert!(prompt.contains("Do NOT start implementing"));
        assert!(prompt.contains("3142"));
    }

    #[test]
    fn finish_task_happy_path() {
        let mock = MockProcessRunner::new(vec![
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
    fn finish_task_not_on_main() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"feature-branch\n"),
        ]);

        let result = finish_task("/repo", "/repo/.worktrees/42-fix-bug", "42-fix-bug", None, &mock);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, FinishError::NotOnMain(_)));
        assert!(err.to_string().contains("feature-branch"));
        assert_eq!(mock.recorded_calls().len(), 1);
    }

    #[test]
    fn finish_task_rebase_conflict() {
        let mock = MockProcessRunner::new(vec![
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
            MockProcessRunner::ok(),  // git push
            MockProcessRunner::ok_with_stdout(b"git@github.com:org/repo.git\n"),  // git remote get-url origin
            MockProcessRunner::ok_with_stdout(b"https://github.com/org/repo/pull/42\n"),  // gh pr create
        ]);

        let result = create_pr("/repo", "42-fix-bug", "Fix bug", "A nasty crash", &mock).unwrap();
        assert_eq!(result.pr_url, "https://github.com/org/repo/pull/42");
        assert_eq!(result.pr_number, 42);

        let calls = mock.recorded_calls();
        assert_eq!(calls[0].0, "git");
        assert!(calls[0].1.contains(&"push".to_string()));
        assert!(calls[0].1.contains(&"-u".to_string()));
        assert_eq!(calls[1].0, "git");
        assert!(calls[1].1.contains(&"get-url".to_string()));
        assert_eq!(calls[2].0, "gh");
        assert!(calls[2].1.contains(&"create".to_string()));
        assert!(calls[2].1.contains(&"org/repo".to_string()));
    }

    #[test]
    fn create_pr_push_fails() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::fail("fatal: no remote"),
        ]);

        let result = create_pr("/repo", "42-fix-bug", "Fix bug", "desc", &mock);
        assert!(matches!(result, Err(PrError::PushFailed(_))));
    }

    #[test]
    fn create_pr_gh_create_fails() {
        let mock = MockProcessRunner::new(vec![
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
            MockProcessRunner::ok_with_stdout(b"git@github.com:org/repo.git\n"),  // git remote get-url
            MockProcessRunner::ok_with_stdout(b"OPEN\n"),  // gh pr view
        ]);
        let result = check_pr_status(42, "/repo", &mock).unwrap();
        assert_eq!(result, PrState::Open);
    }

    #[test]
    fn check_pr_status_merged() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"git@github.com:org/repo.git\n"),
            MockProcessRunner::ok_with_stdout(b"MERGED\n"),
        ]);
        let result = check_pr_status(42, "/repo", &mock).unwrap();
        assert_eq!(result, PrState::Merged);
    }

    #[test]
    fn check_pr_status_closed() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"git@github.com:org/repo.git\n"),
            MockProcessRunner::ok_with_stdout(b"CLOSED\n"),
        ]);
        let result = check_pr_status(42, "/repo", &mock).unwrap();
        assert_eq!(result, PrState::Closed);
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
}
