use anyhow::{Context, Result};
use std::fs;
use std::process::Command;

use crate::models::{DispatchResult, slugify};
use crate::tmux;

// ---------------------------------------------------------------------------
// dispatch_agent
// ---------------------------------------------------------------------------

/// Provision a git worktree, write MCP config, open a tmux window, and
/// launch the Claude agent with a structured prompt.
///
/// This function is **synchronous** and should be called via
/// `tokio::task::spawn_blocking` from async contexts.
pub fn dispatch_agent(
    task_id: i64,
    title: &str,
    description: &str,
    repo_path: &str,
    mcp_port: u16,
) -> Result<DispatchResult> {
    let repo_path = expand_tilde(repo_path);
    let slug = slugify(title);
    let worktree_name = format!("{task_id}-{slug}");
    let worktree_path = format!("{repo_path}/.worktrees/{worktree_name}");
    let tmux_window = format!("task-{task_id}");

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

    // 4. Open a new tmux window rooted at the worktree.
    tmux::new_window(&tmux_window, &worktree_path)
        .context("failed to create tmux window")?;

    // 5. Write the prompt file and launch Claude in print mode.
    let prompt = build_prompt(task_id, title, description, mcp_port);
    let prompt_file = format!("{worktree_path}/.claude-prompt");
    fs::write(&prompt_file, &prompt)
        .with_context(|| format!("failed to write {prompt_file}"))?;
    tmux::send_keys(&tmux_window, "claude -p < .claude-prompt")
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
pub fn cleanup_task(repo_path: &str, worktree_path: &str, tmux_window: &str) -> Result<()> {
    // Kill the tmux window if it is still alive.
    match tmux::has_window(tmux_window) {
        Ok(true) => {
            tmux::kill_window(tmux_window)
                .context("failed to kill tmux window during cleanup")?;
        }
        Ok(false) => {} // already gone
        Err(e) => {
            // Non-fatal: window state unknown, proceed with worktree removal.
            eprintln!("warning: could not check tmux window during cleanup: {e}");
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
// Helpers
// ---------------------------------------------------------------------------

fn build_prompt(task_id: i64, title: &str, description: &str, mcp_port: u16) -> String {
    format!(
        "You are an autonomous coding agent. \
Your task is:\n\
  ID: {task_id}\n\
  Title: {title}\n\
  Description: {description}\n\
\n\
An MCP server is available at http://localhost:{mcp_port}/mcp — use it to \
update task status and post notes as you work (tool: task-orchestrator). \
When your work is complete, update the task status to 'review' via the MCP \
server. If MCP is unavailable, run: \
task-orchestrator update {task_id} review"
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
        let prompt = build_prompt(42, "Fix bug", "A nasty crash", 3142);
        assert!(prompt.contains("42"));
        assert!(prompt.contains("Fix bug"));
        assert!(prompt.contains("A nasty crash"));
        assert!(prompt.contains("3142"));
        assert!(prompt.contains("review"));
    }

    #[test]
    fn build_prompt_contains_mcp_fallback() {
        let prompt = build_prompt(7, "Title", "Desc", 3142);
        assert!(prompt.contains("task-orchestrator update 7 review"));
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
}
