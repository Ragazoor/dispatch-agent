use anyhow::{Context, Result};
use std::fs;
use std::process::Command;

use crate::models::{DispatchResult, ResumeResult, Task, slugify};
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
fn provision_worktree(task: &Task) -> Result<ProvisionResult> {
    let repo_path = expand_tilde(&task.repo_path);
    let slug = slugify(&task.title);
    let worktree_name = format!("{}-{slug}", task.id);
    let worktree_path = format!("{repo_path}/.worktrees/{worktree_name}");
    let tmux_window = build_tmux_window_name(task.id);

    // 1. Ensure the .worktrees directory exists.
    fs::create_dir_all(format!("{repo_path}/.worktrees"))
        .context("failed to create .worktrees directory")?;

    // 2. Create git worktree (-B resets the branch if it already exists).
    let output = Command::new("git")
        .args(["-C", &repo_path, "worktree", "add", &worktree_path, "-B", &worktree_name])
        .output()
        .context("failed to spawn git worktree add")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let msg = stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or(stderr.trim());
        anyhow::bail!("git worktree add failed: {msg}");
    }

    // 3. Open a new tmux window rooted at the worktree.
    tmux::new_window(&tmux_window, &worktree_path)
        .context("failed to create tmux window")?;

    Ok(ProvisionResult { worktree_path, tmux_window })
}

/// Provision a git worktree, open a tmux window, and launch the Claude agent
/// with a structured prompt.
///
/// This function is **synchronous** and should be called via
/// `tokio::task::spawn_blocking` from async contexts.
pub fn dispatch_agent(task: &Task, mcp_port: u16) -> Result<DispatchResult> {
    let provision = provision_worktree(task)?;


    let prompt = build_prompt(task.id, &task.title, &task.description, mcp_port, task.plan.as_deref());
    let prompt_file = format!("{}/.claude-prompt", provision.worktree_path);
    fs::write(&prompt_file, &prompt)
        .with_context(|| format!("failed to write {prompt_file}"))?;
    tmux::send_keys(&provision.tmux_window, "claude \"$(cat .claude-prompt)\"")
        .context("failed to send keys to tmux window")?;

    Ok(DispatchResult {
        worktree_path: provision.worktree_path,
        tmux_window: provision.tmux_window,
    })
}

/// Provision a worktree and launch a brainstorming session.
///
/// Same infrastructure as `dispatch_agent` but with a brainstorming-focused prompt.
pub fn brainstorm_agent(task: &Task, mcp_port: u16) -> Result<DispatchResult> {
    let provision = provision_worktree(task)?;

    let prompt = build_brainstorm_prompt(task.id, &task.title, &task.description, mcp_port);
    let prompt_file = format!("{}/.claude-prompt", provision.worktree_path);
    fs::write(&prompt_file, &prompt)
        .with_context(|| format!("failed to write {prompt_file}"))?;
    tmux::send_keys(&provision.tmux_window, "claude \"$(cat .claude-prompt)\"")
        .context("failed to send keys to tmux window")?;

    Ok(DispatchResult {
        worktree_path: provision.worktree_path,
        tmux_window: provision.tmux_window,
    })
}

/// Provision a worktree and launch a quick dispatch session.
///
/// Same infrastructure as `dispatch_agent` but with a prompt that instructs
/// the agent to rename the placeholder task after understanding user intent.
pub fn quick_dispatch_agent(task: &Task, mcp_port: u16) -> Result<DispatchResult> {
    let provision = provision_worktree(task)?;

    let prompt = build_quick_dispatch_prompt(task.id, &task.title, &task.description, mcp_port);
    let prompt_file = format!("{}/.claude-prompt", provision.worktree_path);
    fs::write(&prompt_file, &prompt)
        .with_context(|| format!("failed to write {prompt_file}"))?;
    tmux::send_keys(&provision.tmux_window, "claude \"$(cat .claude-prompt)\"")
        .context("failed to send keys to tmux window")?;

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
pub fn cleanup_task(repo_path: &str, worktree_path: &str, tmux_window: Option<&str>) -> Result<()> {
    // Kill the tmux window if it is still alive.
    if let Some(window) = tmux_window {
        match tmux::has_window(window) {
            Ok(true) => {
                tmux::kill_window(window)
                    .context("failed to kill tmux window during cleanup")?;
            }
            Ok(false) => {}
            Err(e) => {
                eprintln!("warning: could not check tmux window during cleanup: {e}");
            }
        }
    }

    // Remove the git worktree.
    let output = Command::new("git")
        .args(["worktree", "remove", "--force", worktree_path])
        .output()
        .context("failed to spawn git worktree remove")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "git worktree remove failed for path {}: {}",
            worktree_path,
            stderr.trim()
        );
    }

    // Delete the local branch created for this worktree.
    // Derive branch name from the worktree path's last component.
    if let Some(branch) = std::path::Path::new(worktree_path)
        .file_name()
        .and_then(|n| n.to_str())
    {
        // Best-effort: ignore errors (branch may not exist if worktree creation
        // failed partway through).
        let _ = Command::new("git")
            .args(["-C", repo_path, "branch", "-D", branch])
            .output();
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
pub fn resume_agent(
    task_id: i64,
    worktree_path: &str,
) -> Result<ResumeResult> {
    let tmux_window = build_tmux_window_name(task_id);

    // 1. Create a new tmux window at the existing worktree.
    tmux::new_window(&tmux_window, worktree_path)
        .context("failed to create tmux window for resume")?;

    // 2. Launch Claude in continue mode (picks up most recent conversation).
    tmux::send_keys(&tmux_window, "claude --continue")
        .context("failed to send resume keys to tmux window")?;

    Ok(ResumeResult { tmux_window })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn build_tmux_window_name(task_id: i64) -> String {
    format!("task-{task_id}")
}

fn build_prompt(task_id: i64, title: &str, description: &str, mcp_port: u16, plan: Option<&str>) -> String {
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
Do not call update_task for status changes. \
An MCP server is available at http://localhost:{mcp_port}/mcp — use it to \
post notes as you work (tool: task-orchestrator, tool name: add_note)."
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
post notes as you work (tool: task-orchestrator, tool name: add_note) and \
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

    #[test]
    fn build_prompt_contains_task_info() {
        let prompt = build_prompt(42, "Fix bug", "A nasty crash", 3142, None);
        assert!(prompt.contains("42"));
        assert!(prompt.contains("Fix bug"));
        assert!(prompt.contains("A nasty crash"));
        assert!(prompt.contains("3142"));
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


}
