use anyhow::{Context, Result};
use std::fs;

use crate::models::{DispatchResult, ResumeResult, Task, TaskId, slugify};
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

/// Errors from the finish (merge + cleanup) operation.
#[derive(Debug)]
pub enum FinishError {
    NotOnMain(String),
    MergeConflict(String),
    Other(String),
}

impl std::fmt::Display for FinishError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FinishError::NotOnMain(branch) => write!(
                f,
                "Repo root is not on main (currently on {branch}) — checkout main first"
            ),
            FinishError::MergeConflict(branch) => {
                write!(f, "Merge conflict on {branch} — resolve and try again")
            }
            FinishError::Other(msg) => write!(f, "{msg}"),
        }
    }
}

/// Merge the task branch into main (from the repo root) and kill the tmux window.
/// The worktree is preserved — it will be cleaned up when the task is archived.
pub fn finish_task(
    repo_path: &str,
    branch: &str,
    tmux_window: Option<&str>,
    runner: &dyn ProcessRunner,
) -> std::result::Result<(), FinishError> {
    let repo_path = &expand_tilde(repo_path);

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

    // 3. Merge with --no-ff
    let output = runner
        .run("git", &["-C", repo_path, "merge", "--no-ff", branch])
        .map_err(|e| FinishError::Other(format!("Failed to run git merge: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let is_conflict = stderr.contains("CONFLICT")
            || stdout.contains("CONFLICT")
            || stderr.contains("Automatic merge failed");

        // Abort the merge
        let _ = runner.run("git", &["-C", repo_path, "merge", "--abort"]);

        if is_conflict {
            return Err(FinishError::MergeConflict(branch.to_string()));
        }
        return Err(FinishError::Other(format!(
            "Merge failed: {}",
            stderr.trim()
        )));
    }

    // 4. Kill tmux window (worktree is preserved for later archival)
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
query and update tasks (tool: task-orchestrator). Use update_task to rename \
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
    use crate::process::{MockProcessRunner, exit_fail};
    use crate::models::{Task, TaskId, TaskStatus};
    use chrono::Utc;
    use std::process::Output;

    fn make_task(repo_path: &str) -> Task {
        Task {
            id: TaskId(42),
            title: "Fix bug".to_string(),
            description: "A nasty crash".to_string(),
            repo_path: repo_path.to_string(),
            status: TaskStatus::Ready,
            worktree: None,
            tmux_window: None,
            plan: None,
            epic_id: None,
            needs_input: false,
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
        dispatch_agent(&task, &mock).unwrap();

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

        resume_agent(TaskId(42), &worktree_path, &mock).unwrap();

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
    fn finish_task_happy_path() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"main\n"),       // rev-parse HEAD
            MockProcessRunner::ok_with_stdout(b"git@github.com:org/repo.git\n"), // remote get-url origin
            MockProcessRunner::ok(),                             // git pull origin main
            MockProcessRunner::ok(),                             // git merge --no-ff
            // Only tmux kill (worktree preserved for archival):
            MockProcessRunner::ok_with_stdout(b"task-42\n"),     // tmux list-windows (has_window)
            MockProcessRunner::ok(),                             // tmux kill-window
        ]);

        finish_task("/repo", "42-fix-bug", Some("task-42"), &mock).unwrap();

        let calls = mock.recorded_calls();
        assert!(calls.iter().any(|c| c.1.contains(&"merge".to_string())));
        assert!(calls.iter().any(|c| c.1.contains(&"--no-ff".to_string())));
        // No worktree removal
        assert!(!calls.iter().any(|c| c.1.contains(&"remove".to_string())));
    }

    #[test]
    fn finish_task_not_on_main() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"feature-branch\n"),
        ]);

        let result = finish_task("/repo", "42-fix-bug", None, &mock);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, FinishError::NotOnMain(_)));
        assert!(err.to_string().contains("feature-branch"));
        assert_eq!(mock.recorded_calls().len(), 1);
    }

    #[test]
    fn finish_task_merge_conflict() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"main\n"),
            MockProcessRunner::fail(""),                         // remote get-url (no remote)
            Ok(Output {
                status: exit_fail(),
                stdout: b"CONFLICT (content): Merge conflict in src/main.rs\n".to_vec(),
                stderr: b"Automatic merge failed; fix conflicts and then commit the result.\n".to_vec(),
            }),
            MockProcessRunner::ok(),                             // git merge --abort
        ]);

        let result = finish_task("/repo", "42-fix-bug", None, &mock);
        assert!(matches!(result.unwrap_err(), FinishError::MergeConflict(_)));
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

        let result = finish_task("/repo", "42-fix-bug", None, &mock);
        assert!(matches!(result.unwrap_err(), FinishError::Other(_)));
    }

    #[test]
    fn finish_task_no_remote_skips_pull() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"main\n"),       // rev-parse HEAD
            MockProcessRunner::fail(""),                         // remote get-url (no remote)
            MockProcessRunner::ok(),                             // git merge --no-ff
            // No tmux window, no cleanup
        ]);

        finish_task("/repo", "42-fix-bug", None, &mock).unwrap();
        let calls = mock.recorded_calls();
        // Should not have a "pull" call
        assert!(!calls.iter().any(|c| c.1.contains(&"pull".to_string())));
    }
}
