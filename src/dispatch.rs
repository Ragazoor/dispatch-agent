use anyhow::{Context, Result};
use std::fs;

use crate::models::{DispatchResult, ResumeResult, Task, slugify};
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

    tracing::info!(task_id = task.id, %worktree_path, "provisioning worktree");

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

    Ok(ProvisionResult { worktree_path, tmux_window })
}

/// Provision a git worktree, open a tmux window, and launch the Claude agent
/// with a structured prompt.
///
/// This function is **synchronous** and should be called via
/// `tokio::task::spawn_blocking` from async contexts.
pub fn dispatch_agent(task: &Task, mcp_port: u16, runner: &dyn ProcessRunner) -> Result<DispatchResult> {
    let provision = provision_worktree(task, runner)?;

    let prompt = build_prompt(task.id, &task.title, &task.description, mcp_port, task.plan.as_deref());
    let prompt_file = format!("{}/.claude-prompt", provision.worktree_path);
    fs::write(&prompt_file, &prompt)
        .with_context(|| format!("failed to write {prompt_file}"))?;
    tmux::send_keys(&provision.tmux_window, "claude \"$(cat .claude-prompt)\"", runner)
        .context("failed to send keys to tmux window")?;

    tracing::info!(task_id = task.id, worktree = %provision.worktree_path, "agent dispatched");

    Ok(DispatchResult {
        worktree_path: provision.worktree_path,
        tmux_window: provision.tmux_window,
    })
}

/// Provision a worktree and launch a brainstorming session.
///
/// Same infrastructure as `dispatch_agent` but with a brainstorming-focused prompt.
pub fn brainstorm_agent(task: &Task, mcp_port: u16, runner: &dyn ProcessRunner) -> Result<DispatchResult> {
    let provision = provision_worktree(task, runner)?;

    let prompt = build_brainstorm_prompt(task.id, &task.title, &task.description, mcp_port);
    let prompt_file = format!("{}/.claude-prompt", provision.worktree_path);
    fs::write(&prompt_file, &prompt)
        .with_context(|| format!("failed to write {prompt_file}"))?;
    tmux::send_keys(&provision.tmux_window, "claude \"$(cat .claude-prompt)\"", runner)
        .context("failed to send keys to tmux window")?;

    tracing::info!(task_id = task.id, worktree = %provision.worktree_path, "brainstorm dispatched");

    Ok(DispatchResult {
        worktree_path: provision.worktree_path,
        tmux_window: provision.tmux_window,
    })
}

/// Provision a worktree and launch a quick dispatch session.
///
/// Same infrastructure as `dispatch_agent` but with a prompt that instructs
/// the agent to rename the placeholder task after understanding user intent.
pub fn quick_dispatch_agent(task: &Task, mcp_port: u16, runner: &dyn ProcessRunner) -> Result<DispatchResult> {
    let provision = provision_worktree(task, runner)?;

    let prompt = build_quick_dispatch_prompt(task.id, &task.title, &task.description, mcp_port);
    let prompt_file = format!("{}/.claude-prompt", provision.worktree_path);
    fs::write(&prompt_file, &prompt)
        .with_context(|| format!("failed to write {prompt_file}"))?;
    tmux::send_keys(&provision.tmux_window, "claude \"$(cat .claude-prompt)\"", runner)
        .context("failed to send keys to tmux window")?;

    tracing::info!(task_id = task.id, worktree = %provision.worktree_path, "quick dispatch agent launched");

    Ok(DispatchResult {
        worktree_path: provision.worktree_path,
        tmux_window: provision.tmux_window,
    })
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
// resume_agent
// ---------------------------------------------------------------------------

/// Re-open a tmux window for an existing worktree and resume the most recent
/// Claude conversation with `claude --continue`.
///
/// This function is **synchronous** and should be called via
/// `tokio::task::spawn_blocking` from async contexts.
pub fn resume_agent(task_id: i64, worktree_path: &str, runner: &dyn ProcessRunner) -> Result<ResumeResult> {
    let tmux_window = build_tmux_window_name(task_id);

    tmux::new_window(&tmux_window, worktree_path, runner)
        .context("failed to create tmux window for resume")?;

    tmux::send_keys(&tmux_window, "claude --continue", runner)
        .context("failed to send resume keys to tmux window")?;

    tracing::info!(task_id, %tmux_window, "agent resumed");

    Ok(ResumeResult { tmux_window })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn build_tmux_window_name(task_id: i64) -> String {
    format!("task-{task_id}")
}

fn build_prompt(task_id: i64, title: &str, description: &str, _mcp_port: u16, plan: Option<&str>) -> String {
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

fn build_quick_dispatch_prompt(task_id: i64, title: &str, description: &str, mcp_port: u16) -> String {
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
post notes as you work (tool: task-orchestrator, tool name: add_note) and \
rename this task (tool: task-orchestrator, tool name: update_task — set the title field)."
    )
}

fn build_brainstorm_prompt(task_id: i64, title: &str, description: &str, mcp_port: u16) -> String {
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
attach the plan (tool: task-orchestrator, tool name: update_task — set the plan field)."
    )
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
    use crate::process::MockProcessRunner;
    use crate::models::{Task, TaskStatus};
    use chrono::Utc;

    fn make_task(repo_path: &str) -> Task {
        Task {
            id: 42,
            title: "Fix bug".to_string(),
            description: "A nasty crash".to_string(),
            repo_path: repo_path.to_string(),
            status: TaskStatus::Ready,
            worktree: None,
            tmux_window: None,
            plan: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn build_prompt_contains_task_info() {
        let prompt = build_prompt(42, "Fix bug", "A nasty crash", 3142, None);
        assert!(prompt.contains("42"));
        assert!(prompt.contains("Fix bug"));
        assert!(prompt.contains("A nasty crash"));
        assert!(prompt.contains("automatically via hooks"));
    }

    #[test]
    fn build_prompt_mentions_automatic_hooks() {
        let prompt = build_prompt(7, "Title", "Desc", 3142, None);
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
        assert_eq!(build_tmux_window_name(42), "task-42");
    }

    #[test]
    fn build_prompt_includes_plan_path() {
        let prompt = build_prompt(1, "Task", "Desc", 3142, Some("docs/plans/my-plan.md"));
        assert!(prompt.contains("Plan: docs/plans/my-plan.md"));
    }

    #[test]
    fn build_prompt_without_plan_omits_plan_section() {
        let prompt = build_prompt(1, "Task", "Desc", 3142, None);
        assert!(!prompt.contains("Plan:"));
    }

    #[test]
    fn build_quick_dispatch_prompt_contains_rename_instruction() {
        let prompt = build_quick_dispatch_prompt(42, "Quick task", "", 3142);
        assert!(prompt.contains("42"));
        assert!(prompt.contains("Quick task"));
        assert!(prompt.contains("update_task"));
        assert!(prompt.contains("title"));
        assert!(prompt.contains("placeholder"));
    }

    #[test]
    fn build_quick_dispatch_prompt_mentions_mcp() {
        let prompt = build_quick_dispatch_prompt(1, "Quick task", "", 3142);
        assert!(prompt.contains("3142"));
        assert!(prompt.contains("add_note"));
    }

    #[test]
    fn build_quick_dispatch_prompt_differs_from_regular() {
        let regular = build_prompt(1, "Task", "Desc", 3142, None);
        let quick = build_quick_dispatch_prompt(1, "Task", "Desc", 3142);
        assert!(quick.contains("placeholder"));
        assert!(!regular.contains("placeholder"));
    }

    #[test]
    fn build_brainstorm_prompt_contains_task_info() {
        let prompt = build_brainstorm_prompt(7, "Design auth", "Rework the auth flow", 3142);
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
            MockProcessRunner::ok(), // tmux send-keys -l
            MockProcessRunner::ok(), // tmux send-keys Enter
        ]);

        let task = make_task(&repo_path);
        dispatch_agent(&task, 3142, &mock).unwrap();

        let calls = mock.recorded_calls();
        assert_eq!(calls[0].0, "git", "first call should be git");
        assert!(calls[0].1.contains(&"worktree".to_string()));
        assert!(calls[0].1.contains(&"add".to_string()));
        assert_eq!(calls[1].0, "tmux");
        assert_eq!(calls[1].1[0], "new-window");
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
            MockProcessRunner::ok(), // tmux send-keys -l (the claude command)
            MockProcessRunner::ok(), // tmux send-keys Enter
        ]);

        let task = make_task(&repo_path);
        dispatch_agent(&task, 3142, &mock).unwrap();

        let calls = mock.recorded_calls();
        // The literal send-keys call (index 2) carries the claude invocation
        assert!(
            calls[2].1.iter().any(|a| a.contains("claude")),
            "send-keys should include claude"
        );
    }

    #[test]
    fn resume_skips_git_issues_tmux_continue() {
        let dir = tempfile::TempDir::new().unwrap();
        let worktree_path = dir.path().to_str().unwrap().to_string();

        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // tmux new-window
            MockProcessRunner::ok(), // tmux send-keys -l
            MockProcessRunner::ok(), // tmux send-keys Enter
        ]);

        resume_agent(42, &worktree_path, &mock).unwrap();

        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(calls[0].1[0], "new-window");
        assert!(calls.iter().all(|(prog, _)| prog != "git"), "resume should make no git calls");
        assert!(calls[1].1.iter().any(|a| a.contains("--continue")));
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
        let result = dispatch_agent(&task, 3142, &mock);
        assert!(result.is_err());
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1, "only the git call should have been made");
    }
}
