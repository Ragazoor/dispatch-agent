use anyhow::{Context, Result};
use std::fs;
use std::process::Command;

use crate::models::{DispatchResult, ResumeResult, Task, slugify};
use crate::tmux;

// ---------------------------------------------------------------------------
// dispatch_agent
// ---------------------------------------------------------------------------

/// Provision a git worktree, write MCP config, open a tmux window, and
/// launch the Claude agent with a structured prompt.
///
/// This function is **synchronous** and should be called via
/// `tokio::task::spawn_blocking` from async contexts.
pub fn dispatch_agent(task: &Task, mcp_port: u16, db_path: &str) -> Result<DispatchResult> {
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
        .args([
            "-C",
            &repo_path,
            "worktree",
            "add",
            &worktree_path,
            "-B",
            &worktree_name,
        ])
        .output()
        .context("failed to spawn git worktree add")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Show the last meaningful line (git prints progress first, error last)
        let msg = stderr
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or(stderr.trim());
        anyhow::bail!("git worktree add failed: {msg}");
    }

    // 3. Write .mcp.json into the worktree so Claude picks up the MCP server.
    let mcp_config = format!(
        r#"{{"mcpServers":{{"task-orchestrator":{{"url":"http://localhost:{mcp_port}/mcp"}}}}}}"#
    );
    fs::write(format!("{worktree_path}/.mcp.json"), &mcp_config)
        .with_context(|| format!("failed to write {worktree_path}/.mcp.json"))?;

    // 3b. Write .claude/settings.json with status hooks.
    let claude_dir = format!("{worktree_path}/.claude");
    fs::create_dir_all(&claude_dir)
        .with_context(|| format!("failed to create {claude_dir}"))?;
    let hooks_config = build_hooks_config(task.id, db_path);
    fs::write(format!("{claude_dir}/settings.json"), &hooks_config)
        .with_context(|| format!("failed to write {claude_dir}/settings.json"))?;

    // 4. Open a new tmux window rooted at the worktree.
    tmux::new_window(&tmux_window, &worktree_path)
        .context("failed to create tmux window")?;

    // 5. Write the prompt file and launch Claude in interactive mode.
    let prompt = build_prompt(task.id, &task.title, &task.description, mcp_port, task.plan.as_deref());
    let prompt_file = format!("{worktree_path}/.claude-prompt");
    fs::write(&prompt_file, &prompt)
        .with_context(|| format!("failed to write {prompt_file}"))?;
    tmux::send_keys(&tmux_window, "claude \"$(cat .claude-prompt)\"")
        .context("failed to send keys to tmux window")?;

    Ok(DispatchResult {
        worktree_path,
        tmux_window,
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

fn build_hooks_config(task_id: i64, db_path: &str) -> String {
    format!(
        r#"{{"hooks":{{"Stop":[{{"type":"command","command":"task-orchestrator --db {db_path} update {task_id} review"}}],"UserPromptSubmit":[{{"type":"command","command":"task-orchestrator --db {db_path} update {task_id} running"}}]}}}}"#
    )
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
    fn build_hooks_config_contains_task_id_and_db_path() {
        let config = build_hooks_config(42, "/home/user/.local/share/task-orchestrator/tasks.db");
        let parsed: serde_json::Value = serde_json::from_str(&config)
            .expect("hooks config should be valid JSON");

        let stop_cmd = parsed["hooks"]["Stop"][0]["command"].as_str().unwrap();
        assert!(stop_cmd.contains("update 42 review"), "Stop hook should update to review");
        assert!(stop_cmd.contains("--db /home/user/.local/share/task-orchestrator/tasks.db"));

        let submit_cmd = parsed["hooks"]["UserPromptSubmit"][0]["command"].as_str().unwrap();
        assert!(submit_cmd.contains("update 42 running"), "UserPromptSubmit hook should update to running");
        assert!(submit_cmd.contains("--db /home/user/.local/share/task-orchestrator/tasks.db"));
    }
}
