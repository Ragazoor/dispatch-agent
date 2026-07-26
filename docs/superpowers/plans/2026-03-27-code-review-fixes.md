# Code Review Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Address all findings from the 2026-03-27 code review: DB reliability, file-based tracing, injectable ProcessRunner for testable dispatch, and several small cleanups.

**Architecture:** Add a `ProcessRunner` trait (like `TaskStore`) so subprocess calls in `tmux.rs` and `dispatch.rs` can be mocked in tests. Init tracing in `main.rs` writing to a log file alongside the DB. Fix all `lock().unwrap()` in `db.rs` to propagate errors, and warn on silent status fallbacks.

**Tech Stack:** Rust 2021, `tracing` + `tracing-subscriber` (new), `tempfile` promoted from dev-dep.

---

## File Map

| File | Change |
|------|--------|
| `Cargo.toml` | Narrow tokio features; add tracing, tracing-subscriber; promote tempfile |
| `src/process.rs` | **New** — `ProcessRunner` trait, `RealProcessRunner`, `MockProcessRunner` |
| `src/lib.rs` | Export `process` module |
| `src/tmux.rs` | All 6 public functions accept `runner: &dyn ProcessRunner` |
| `src/dispatch.rs` | `provision_worktree`, `dispatch_agent`, `brainstorm_agent`, `cleanup_task`, `resume_agent` accept runner; add tracing |
| `src/runtime.rs` | `TuiRuntime` gains `runner` field; all callers updated; tempfile RAII; tracing |
| `src/db.rs` | 14 `lock().unwrap()` → `map_err`; 2 silent fallbacks get `tracing::warn!` |
| `src/main.rs` | Tracing init; `parse_status` helper |
| `src/mcp/handlers.rs` | `tracing::info!` on each tool call |
| `CLAUDE.md` | tmux prereq; hooks/branch naming; MCP curl; dispatch vs resume |

---

## Task 1: Update Cargo.toml

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Apply all dependency changes**

Replace the `[dependencies]` and `[dev-dependencies]` sections with:

```toml
[dependencies]
ratatui = "0.29"
crossterm = "0.28"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync", "time"] }
rusqlite = { version = "0.32", features = ["bundled"] }
clap = { version = "4", features = ["derive", "env"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
axum = "0.8"
chrono = { version = "0.4", features = ["serde"] }
anyhow = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["fmt", "env-filter"] }
tempfile = "3"
```

Remove the `[dev-dependencies]` section entirely (tempfile is now in `[dependencies]`).

- [ ] **Step 2: Verify it compiles**

Run: `cargo build`
Expected: successful compile

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml
git commit -m "chore: narrow tokio features, add tracing + tempfile deps"
```

---

## Task 2: ProcessRunner Trait

**Files:**
- Create: `src/process.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Create `src/process.rs`**

```rust
use anyhow::{Context, Result};
use std::collections::VecDeque;
use std::process::Output;
use std::sync::Mutex;

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

pub trait ProcessRunner: Send + Sync {
    fn run(&self, program: &str, args: &[&str]) -> Result<Output>;
}

// ---------------------------------------------------------------------------
// Real implementation — wraps std::process::Command
// ---------------------------------------------------------------------------

pub struct RealProcessRunner;

impl ProcessRunner for RealProcessRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<Output> {
        std::process::Command::new(program)
            .args(args)
            .output()
            .with_context(|| format!("failed to run {program}"))
    }
}

// ---------------------------------------------------------------------------
// Mock implementation — for tests only
// ---------------------------------------------------------------------------

pub struct MockProcessRunner {
    pub calls: Mutex<Vec<(String, Vec<String>)>>,
    responses: Mutex<VecDeque<Result<Output>>>,
}

impl MockProcessRunner {
    pub fn new(responses: Vec<Result<Output>>) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            responses: Mutex::new(VecDeque::from(responses)),
        }
    }

    pub fn recorded_calls(&self) -> Vec<(String, Vec<String>)> {
        self.calls.lock().unwrap().clone()
    }

    /// Successful Output with empty stdout/stderr.
    pub fn ok() -> Result<Output> {
        Ok(Output {
            status: exit_ok(),
            stdout: vec![],
            stderr: vec![],
        })
    }

    /// Successful Output with specific stdout bytes.
    pub fn ok_with_stdout(stdout: &[u8]) -> Result<Output> {
        Ok(Output {
            status: exit_ok(),
            stdout: stdout.to_vec(),
            stderr: vec![],
        })
    }

    /// Failed Output (non-zero exit) with specific stderr.
    pub fn fail(stderr: &str) -> Result<Output> {
        Ok(Output {
            status: exit_fail(),
            stdout: vec![],
            stderr: stderr.as_bytes().to_vec(),
        })
    }
}

impl ProcessRunner for MockProcessRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<Output> {
        self.calls
            .lock()
            .unwrap()
            .push((program.to_string(), args.iter().map(|s| s.to_string()).collect()));
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| panic!("MockProcessRunner: no response queued for {program} {args:?}"))
    }
}

// ---------------------------------------------------------------------------
// Helpers for constructing ExitStatus in tests (Unix only)
// ---------------------------------------------------------------------------

#[cfg(unix)]
pub fn exit_ok() -> std::process::ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    std::process::ExitStatus::from_raw(0)
}

#[cfg(unix)]
pub fn exit_fail() -> std::process::ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    // Raw status word: exit code 1 = 1 << 8 = 256
    std::process::ExitStatus::from_raw(1 << 8)
}
```

- [ ] **Step 2: Export from `src/lib.rs`**

Add `pub mod process;` to `src/lib.rs`:

```rust
pub mod db;
pub mod dispatch;
pub mod editor;
pub mod mcp;
pub mod models;
pub mod plan;
pub mod process;
pub mod runtime;
pub mod tmux;
pub mod tui;
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build`
Expected: successful compile

- [ ] **Step 4: Commit**

```bash
git add src/process.rs src/lib.rs
git commit -m "feat: add ProcessRunner trait with real and mock implementations"
```

---

## Task 3: Thread ProcessRunner through tmux.rs

**Files:**
- Modify: `src/tmux.rs`

- [ ] **Step 1: Write three new tests (TDD — they will not compile yet)**

Add these to the `#[cfg(test)]` block at the bottom of `src/tmux.rs`, after the existing tests:

```rust
    use crate::process::MockProcessRunner;

    #[test]
    fn new_window_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        new_window("task-42", "/some/path", &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(
            calls[0].1,
            vec!["new-window", "-n", "task-42", "-c", "/some/path"]
        );
    }

    #[test]
    fn capture_pane_returns_trimmed_stdout() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(
            b"  hello from tmux  \n",
        )]);
        let result = capture_pane("task-42", 5, &mock).unwrap();
        assert_eq!(result, "hello from tmux");
    }

    #[test]
    fn has_window_returns_false_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no sessions")]);
        let result = has_window("task-42", &mock).unwrap();
        assert!(!result);
    }
```

- [ ] **Step 2: Run tests — confirm they fail to compile**

Run: `cargo test 2>&1 | head -20`
Expected: compiler error about wrong number of arguments to `new_window`, `capture_pane`, `has_window`

- [ ] **Step 3: Rewrite `src/tmux.rs` with runner parameter**

Replace the entire file content with:

```rust
use anyhow::{Context, Result, bail};

use crate::process::ProcessRunner;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new tmux window with the given name, starting in `working_dir`.
pub fn new_window(name: &str, working_dir: &str, runner: &dyn ProcessRunner) -> Result<()> {
    let output = runner.run("tmux", &["new-window", "-n", name, "-c", working_dir])?;
    if !output.status.success() {
        bail!("tmux new-window failed with status {}", output.status);
    }
    Ok(())
}

/// Send literal text to a tmux window, then press Enter.
///
/// Uses `-l` to prevent tmux from interpreting escape sequences in the text.
/// Enter is sent as a separate `send-keys` call without `-l`.
pub fn send_keys(window: &str, keys: &str, runner: &dyn ProcessRunner) -> Result<()> {
    let output = runner.run("tmux", &["send-keys", "-t", window, "-l", keys])?;
    if !output.status.success() {
        bail!("tmux send-keys -l failed with status {}", output.status);
    }
    let output = runner.run("tmux", &["send-keys", "-t", window, "Enter"])?;
    if !output.status.success() {
        bail!("tmux send-keys Enter failed with status {}", output.status);
    }
    Ok(())
}

/// Capture the last `lines` lines of output from a tmux pane, returned trimmed.
pub fn capture_pane(window: &str, lines: usize, runner: &dyn ProcessRunner) -> Result<String> {
    let lines_arg = format!("-{lines}");
    let output = runner.run(
        "tmux",
        &["capture-pane", "-t", window, "-p", "-S", &lines_arg],
    )?;
    if !output.status.success() {
        bail!("tmux capture-pane failed with status {}", output.status);
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(text)
}

/// Return true if a tmux window with the given name currently exists.
pub fn has_window(window: &str, runner: &dyn ProcessRunner) -> Result<bool> {
    let output = runner
        .run("tmux", &["list-windows", "-F", "#{window_name}"])
        .context("failed to run tmux list-windows")?;
    // list-windows exits non-zero when there are no windows / no session;
    // treat that as "window not found" rather than a hard error.
    if !output.status.success() {
        return Ok(false);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text.lines().any(|line| line.trim() == window))
}

/// Kill the tmux window with the given name.
pub fn kill_window(window: &str, runner: &dyn ProcessRunner) -> Result<()> {
    let output = runner.run("tmux", &["kill-window", "-t", window])?;
    if !output.status.success() {
        bail!("tmux kill-window failed with status {}", output.status);
    }
    Ok(())
}

/// Switch the active tmux window to the one with the given name.
pub fn select_window(window: &str, runner: &dyn ProcessRunner) -> Result<()> {
    let output = runner.run("tmux", &["select-window", "-t", window])?;
    if !output.status.success() {
        bail!("tmux select-window failed with status {}", output.status);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Internal helpers (kept for arg-shape unit tests)
// ---------------------------------------------------------------------------

#[cfg(test)]
fn select_window_args(window: &str) -> Vec<String> {
    vec!["select-window".to_string(), "-t".to_string(), window.to_string()]
}

#[cfg(test)]
fn new_window_args(name: &str, working_dir: &str) -> Vec<String> {
    vec![
        "new-window".to_string(),
        "-n".to_string(),
        name.to_string(),
        "-c".to_string(),
        working_dir.to_string(),
    ]
}

#[cfg(test)]
fn capture_pane_args(window: &str, lines: usize) -> Vec<String> {
    vec![
        "capture-pane".to_string(),
        "-t".to_string(),
        window.to_string(),
        "-p".to_string(),
        "-S".to_string(),
        format!("-{lines}"),
    ]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_window_args_correct() {
        let args = new_window_args("task-42", "/some/path");
        assert_eq!(
            args,
            vec!["new-window", "-n", "task-42", "-c", "/some/path"]
        );
    }

    #[test]
    fn capture_pane_args_correct() {
        let args = capture_pane_args("task-42", 5);
        assert_eq!(
            args,
            vec!["capture-pane", "-t", "task-42", "-p", "-S", "-5"]
        );
    }

    #[test]
    fn capture_pane_args_different_line_count() {
        let args = capture_pane_args("my-window", 100);
        assert_eq!(args[5], "-100");
    }

    #[test]
    fn has_window_finds_match_in_output() {
        let fake_output = "main\ntask-42\nother-window\n";
        let target = "task-42";
        let found = fake_output.lines().any(|line| line.trim() == target);
        assert!(found);
    }

    #[test]
    fn has_window_no_match() {
        let fake_output = "main\nother-window\n";
        let target = "task-42";
        let found = fake_output.lines().any(|line| line.trim() == target);
        assert!(!found);
    }

    #[test]
    fn has_window_exact_match_not_prefix() {
        let fake_output = "task-42\n";
        let target = "task-4";
        let found = fake_output.lines().any(|line| line.trim() == target);
        assert!(!found);
    }

    #[test]
    fn select_window_args_correct() {
        let args = select_window_args("task-42");
        assert_eq!(args, vec!["select-window", "-t", "task-42"]);
    }

    // --- ProcessRunner-based tests ---

    use crate::process::MockProcessRunner;

    #[test]
    fn new_window_issues_correct_tmux_args() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        new_window("task-42", "/some/path", &mock).unwrap();
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(
            calls[0].1,
            vec!["new-window", "-n", "task-42", "-c", "/some/path"]
        );
    }

    #[test]
    fn capture_pane_returns_trimmed_stdout() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(
            b"  hello from tmux  \n",
        )]);
        let result = capture_pane("task-42", 5, &mock).unwrap();
        assert_eq!(result, "hello from tmux");
    }

    #[test]
    fn has_window_returns_false_on_nonzero_exit() {
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no sessions")]);
        let result = has_window("task-42", &mock).unwrap();
        assert!(!result);
    }
}
```

- [ ] **Step 4: Attempt to compile — expect errors only in dispatch.rs and runtime.rs**

Run: `cargo build 2>&1 | grep "^error" | head -20`
Expected: errors only about `dispatch.rs` and `runtime.rs` passing wrong arg count — that's correct

- [ ] **Step 5: Do not commit yet — proceed to Task 4**

---

## Task 4: Thread ProcessRunner through dispatch.rs + Tests

**Files:**
- Modify: `src/dispatch.rs`

- [ ] **Step 1: Update imports at top of `src/dispatch.rs`**

Replace:
```rust
use anyhow::{Context, Result};
use std::fs;
use std::process::Command;

use crate::models::{DispatchResult, ResumeResult, Task, slugify};
use crate::tmux;
```
with:
```rust
use anyhow::{Context, Result};
use std::fs;

use crate::models::{DispatchResult, ResumeResult, Task, slugify};
use crate::process::ProcessRunner;
use crate::tmux;
```

- [ ] **Step 2: Replace `provision_worktree`**

Replace the `provision_worktree` function:

```rust
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
```

- [ ] **Step 3: Replace `dispatch_agent`**

```rust
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
```

- [ ] **Step 4: Replace `brainstorm_agent`**

```rust
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
```

- [ ] **Step 5: Replace `cleanup_task`**

```rust
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
```

- [ ] **Step 6: Replace `resume_agent`**

```rust
pub fn resume_agent(task_id: i64, worktree_path: &str, runner: &dyn ProcessRunner) -> Result<ResumeResult> {
    let tmux_window = build_tmux_window_name(task_id);

    tmux::new_window(&tmux_window, worktree_path, runner)
        .context("failed to create tmux window for resume")?;

    tmux::send_keys(&tmux_window, "claude --continue", runner)
        .context("failed to send resume keys to tmux window")?;

    tracing::info!(task_id, %tmux_window, "agent resumed");

    Ok(ResumeResult { tmux_window })
}
```

- [ ] **Step 7: Add the five new tests to the `#[cfg(test)]` module in `dispatch.rs`**

Add these imports to the existing test module:

```rust
    use crate::process::MockProcessRunner;
    use crate::models::{Task, TaskStatus};
    use chrono::Utc;
```

Add this helper function inside the test module:

```rust
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
```

Add these five test functions:

```rust
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
```

- [ ] **Step 8: Compile — only runtime.rs should have errors now**

Run: `cargo build 2>&1 | grep "^error" | head -20`
Expected: errors only from `runtime.rs` (wrong arg counts on dispatch/tmux calls)

- [ ] **Step 9: Do not commit yet — proceed to Task 5**

---

## Task 5: Wire ProcessRunner into runtime.rs

**Files:**
- Modify: `src/runtime.rs`

- [ ] **Step 1: Add import and runner field to `TuiRuntime`**

Add to the imports at the top of `src/runtime.rs`:

```rust
use crate::process::{ProcessRunner, RealProcessRunner};
```

Replace the `TuiRuntime` struct definition:

```rust
struct TuiRuntime {
    database: Arc<dyn db::TaskStore>,
    msg_tx: mpsc::UnboundedSender<Message>,
    port: u16,
    input_paused: Arc<AtomicBool>,
    runner: Arc<dyn ProcessRunner>,
}
```

In `run_tui`, replace the `TuiRuntime { ... }` construction:

```rust
    let runtime = TuiRuntime {
        database,
        msg_tx,
        port,
        input_paused,
        runner: Arc::new(RealProcessRunner),
    };
```

- [ ] **Step 2: Update `exec_dispatch`**

Replace `exec_dispatch`:

```rust
    fn exec_dispatch(&self, task: models::Task) {
        let tx = self.msg_tx.clone();
        let port = self.port;
        let runner = self.runner.clone();

        tokio::task::spawn_blocking(move || {
            let id = task.id;
            tracing::info!(task_id = id, "dispatching task");
            match dispatch::dispatch_agent(&task, port, &*runner) {
                Ok(result) => {
                    let _ = tx.send(Message::Dispatched {
                        id,
                        worktree: result.worktree_path,
                        tmux_window: result.tmux_window,
                    });
                }
                Err(e) => {
                    let _ = tx.send(Message::Error(format!("Dispatch failed: {e:#}")));
                }
            }
        });
    }
```

- [ ] **Step 3: Update `exec_brainstorm`**

Replace `exec_brainstorm`:

```rust
    fn exec_brainstorm(&self, task: models::Task) {
        let tx = self.msg_tx.clone();
        let port = self.port;
        let runner = self.runner.clone();

        tokio::task::spawn_blocking(move || {
            let id = task.id;
            tracing::info!(task_id = id, "brainstorming task");
            match dispatch::brainstorm_agent(&task, port, &*runner) {
                Ok(result) => {
                    let _ = tx.send(Message::Dispatched {
                        id,
                        worktree: result.worktree_path,
                        tmux_window: result.tmux_window,
                    });
                }
                Err(e) => {
                    let _ = tx.send(Message::Error(format!("Brainstorm dispatch failed: {e:#}")));
                }
            }
        });
    }
```

- [ ] **Step 4: Update `exec_capture_tmux`**

Replace `exec_capture_tmux`:

```rust
    fn exec_capture_tmux(&self, id: i64, window: String) {
        let tx = self.msg_tx.clone();
        let runner = self.runner.clone();

        tokio::task::spawn_blocking(move || {
            if let Ok(false) = tmux::has_window(&window, &*runner) {
                let _ = tx.send(Message::WindowGone(id));
                return;
            }

            match tmux::capture_pane(&window, 5, &*runner) {
                Ok(output) => {
                    let _ = tx.send(Message::TmuxOutput { id, output });
                }
                Err(e) => {
                    let _ = tx.send(Message::Error(format!(
                        "tmux capture failed for window {window}: {e}"
                    )));
                }
            }
        });
    }
```

- [ ] **Step 5: Update `exec_cleanup`**

Replace `exec_cleanup`:

```rust
    fn exec_cleanup(&self, repo_path: String, worktree: String, tmux_window: Option<String>) {
        let tx = self.msg_tx.clone();
        let runner = self.runner.clone();

        tokio::task::spawn_blocking(move || {
            if let Err(e) = dispatch::cleanup_task(&repo_path, &worktree, tmux_window.as_deref(), &*runner) {
                let _ = tx.send(Message::Error(format!("Cleanup failed: {e:#}")));
            }
        });
    }
```

- [ ] **Step 6: Update `exec_resume`**

Replace `exec_resume`:

```rust
    fn exec_resume(&self, task: models::Task) {
        let tx = self.msg_tx.clone();
        let id = task.id;
        let worktree_path = task.worktree.clone().unwrap_or_default();
        let runner = self.runner.clone();

        tokio::task::spawn_blocking(move || {
            tracing::info!(task_id = id, "resuming task");
            match dispatch::resume_agent(id, &worktree_path, &*runner) {
                Ok(result) => {
                    let _ = tx.send(Message::Resumed {
                        id,
                        tmux_window: result.tmux_window,
                    });
                }
                Err(e) => {
                    let _ = tx.send(Message::Error(format!("Resume failed: {e:#}")));
                }
            }
        });
    }
```

- [ ] **Step 7: Update `exec_jump_to_tmux`**

Replace `exec_jump_to_tmux`:

```rust
    fn exec_jump_to_tmux(&self, app: &mut App, window: String) {
        if let Err(e) = tmux::select_window(&window, &*self.runner) {
            app.update(Message::Error(format!("Jump failed: {e:#}")));
        }
    }
```

- [ ] **Step 8: Add MCP server tracing**

In `run_tui`, after the `tokio::spawn` for the MCP server, add:

```rust
    tracing::info!(port, db = %db_path.display(), "TUI started, MCP server on port {port}");
```

- [ ] **Step 9: Run all tests**

Run: `cargo test`
Expected: all tests pass

- [ ] **Step 10: Commit tasks 3–5 together**

```bash
git add src/tmux.rs src/dispatch.rs src/runtime.rs src/process.rs src/lib.rs
git commit -m "feat: inject ProcessRunner into tmux/dispatch/runtime for testable subprocess calls"
```

---

## Task 6: DB Reliability, Tracing Init, and Cleanup

### 6a — Tracing init + parse_status

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Add imports**

Add to the top of `src/main.rs`:

```rust
use tracing::Level;
use tracing_subscriber::EnvFilter;
```

- [ ] **Step 2: Add tracing init to the `Tui` arm**

Replace:
```rust
        Commands::Tui { port } => {
            runtime::run_tui(&cli.db, port).await?;
        }
```
with:
```rust
        Commands::Tui { port } => {
            let data_dir = cli.db.parent().unwrap_or(std::path::Path::new("."));
            std::fs::create_dir_all(data_dir)?;
            let log_path = data_dir.join("app.log");
            let log_file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)?;
            tracing_subscriber::fmt()
                .with_writer(log_file)
                .with_ansi(false)
                .with_env_filter(
                    EnvFilter::from_default_env().add_directive(Level::INFO.into()),
                )
                .init();
            runtime::run_tui(&cli.db, port).await?;
        }
```

- [ ] **Step 3: Extract `parse_status` helper**

Add above `fn default_db_path()`:

```rust
fn parse_status(s: &str) -> anyhow::Result<models::TaskStatus> {
    models::TaskStatus::parse(s).ok_or_else(|| {
        anyhow::anyhow!(
            "Unknown status: {s}. Valid values: backlog, ready, running, review, done"
        )
    })
}
```

- [ ] **Step 4: Replace duplicate status parsing in `Commands::Update`**

Replace:
```rust
            let new_status = models::TaskStatus::parse(&status)
                .ok_or_else(|| anyhow::anyhow!("Unknown status: {}", status))?;
```
with:
```rust
            let new_status = parse_status(&status)?;
```

- [ ] **Step 5: Replace duplicate status parsing in `Commands::List`**

Replace:
```rust
                    let filter = models::TaskStatus::parse(&s)
                        .ok_or_else(|| anyhow::anyhow!("Unknown status: {}", s))?;
```
with:
```rust
                    let filter = parse_status(&s)?;
```

- [ ] **Step 6: Run tests**

Run: `cargo test`
Expected: all pass

- [ ] **Step 7: Commit**

```bash
git add src/main.rs
git commit -m "feat: initialize file-based tracing to app.log, extract parse_status helper"
```

### 6b — Fix DB mutex panics and silent fallbacks

**Files:**
- Modify: `src/db.rs`

- [ ] **Step 8: Replace all 14 `lock().unwrap()` calls**

Use replace-all on the string `self.conn.lock().unwrap()` — it appears exactly 14 times in the `impl Database` block (lines 118, 128, 141, 158, 175, 194, 209, 223, 242, 256, 270, 281, 296, 309, 320).

Replace every occurrence of:
```rust
let conn = self.conn.lock().unwrap();
```
with:
```rust
let conn = self.conn.lock().map_err(|_| anyhow::anyhow!("db lock poisoned"))?;
```

- [ ] **Step 9: Fix silent status fallback in `row_to_task` (line ~339)**

Replace:
```rust
    let status = TaskStatus::parse(&status_str).unwrap_or(TaskStatus::Backlog);
```
with:
```rust
    let status = TaskStatus::parse(&status_str).unwrap_or_else(|| {
        tracing::warn!(raw = %status_str, "unrecognised task status, defaulting to Backlog");
        TaskStatus::Backlog
    });
```

- [ ] **Step 10: Fix silent source fallback in `row_to_note` (line ~360)**

Replace:
```rust
    let source = NoteSource::parse(&source_str).unwrap_or(NoteSource::User);
```
with:
```rust
    let source = NoteSource::parse(&source_str).unwrap_or_else(|| {
        tracing::warn!(raw = %source_str, "unrecognised note source, defaulting to User");
        NoteSource::User
    });
```

- [ ] **Step 11: Run tests**

Run: `cargo test`
Expected: all pass

- [ ] **Step 12: Commit**

```bash
git add src/db.rs
git commit -m "fix: propagate db lock poison errors, warn on unrecognised status/source"
```

### 6c — Add tracing to MCP handlers

**Files:**
- Modify: `src/mcp/handlers.rs`

- [ ] **Step 13: Add tracing info calls to each tool handler**

In `handle_update_task`, add after `let parsed = ...` is obtained (after the `Ok(a)` match):

```rust
    tracing::info!(task_id = parsed.task_id, status = %parsed.status, "MCP update_task");
```

In `handle_add_note`, add after `let parsed = ...`:

```rust
    tracing::info!(task_id = parsed.task_id, "MCP add_note");
```

In `handle_create_task`, add after `let parsed = ...`:

```rust
    tracing::info!(title = %parsed.title, "MCP create_task");
```

- [ ] **Step 14: Run tests**

Run: `cargo test`
Expected: all pass

- [ ] **Step 15: Commit**

```bash
git add src/mcp/handlers.rs
git commit -m "feat: add tracing to MCP tool handlers"
```

### 6d — Tempfile RAII

**Files:**
- Modify: `src/runtime.rs`

- [ ] **Step 16: Add tempfile import**

Add to the imports at the top of `src/runtime.rs`:

```rust
use tempfile::Builder as TempfileBuilder;
```

- [ ] **Step 17: Replace temp file handling in `exec_edit_in_editor`**

Find the block starting at `let task_id = task.id;` in `exec_edit_in_editor`.

Replace:
```rust
        let task_id = task.id;
        let tmp = std::env::temp_dir().join(format!("task-{task_id}.txt"));
        let content = format_editor_content(&task.title, &task.description, &task.repo_path, task.status.as_str(), task.plan.as_deref().unwrap_or(""));
        std::fs::write(&tmp, &content)?;
```
with:
```rust
        let task_id = task.id;
        let mut tmp = TempfileBuilder::new()
            .prefix(&format!("task-{task_id}-"))
            .suffix(".md")
            .tempfile()?;
        let content = format_editor_content(
            &task.title,
            &task.description,
            &task.repo_path,
            task.status.as_str(),
            task.plan.as_deref().unwrap_or(""),
        );
        std::io::Write::write_all(tmp.as_file_mut(), content.as_bytes())?;
```

Replace the editor invocation to use `tmp.path()`:
```rust
        let status = std::process::Command::new(&editor)
            .arg(tmp.path())
            .status();
```

Replace the read-back:
```rust
                if let Ok(edited) = std::fs::read_to_string(tmp.path()) {
```

Remove the final cleanup line entirely:
```rust
        let _ = std::fs::remove_file(&tmp);  // DELETE THIS LINE
```

- [ ] **Step 18: Run tests**

Run: `cargo test`
Expected: all pass

- [ ] **Step 19: Commit**

```bash
git add src/runtime.rs
git commit -m "fix: use tempfile crate for RAII cleanup of editor temp files"
```

### 6e — CLAUDE.md updates

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 20: Add tmux prerequisite note after line 14**

After the line:
```
Runtime dependencies: `tmux`, `git` (checked at startup). The TUI must be launched from within a tmux session for agent dispatch to work.
```
add:
```markdown

> **Tmux prerequisite:** If you see `not running inside a tmux session`, run `tmux new-session -d -s dev` first, then re-run `cargo run -- tui`.
```

- [ ] **Step 21: Replace the TODO comment and add dispatch/resume clarification**

Replace:
```markdown
- Closing a tmux session preserves the worktree; press `d` to resume with `claude --continue`
- Status transitions (running/review) are handled by project-level Claude Code hooks in `.claude/settings.local.json` that extract the task ID from the git branch name
- Press `g` to jump to an agent's tmux window

> **TODO:** Project-level hooks assume worktree branches follow the `{id}-{slug}` naming convention and that `task-orchestrator` is in PATH. For the general case (multi-project dispatch, non-worktree setups), consider MCP-based status reporting or a dedicated CLI subcommand that infers context.
```
with:
```markdown
- **Dispatch** (`d` on a Ready task): creates a fresh git worktree + tmux window and launches Claude with the task prompt
- **Resume** (`d` on a Running task whose window is gone): re-opens a tmux window in the existing worktree and runs `claude --continue`. Closing a tmux window does **not** delete the worktree.
- Status transitions (running/review) are handled by hooks in `.claude/settings.json` that extract the task ID from the git branch name (`{id}-{slug}` pattern)
- Press `g` to jump to an agent's tmux window
```

- [ ] **Step 22: Add Hooks & Branch Naming section before MCP Server**

Add this new section between `## Kanban Columns` and `## MCP Server`:

```markdown
## Hooks & Branch Naming

Status update hooks in `.claude/settings.json` run when Claude Code starts or stops in a worktree. They parse the branch name, extract the task ID, and call `task-orchestrator update <id> <status>`.

**Requirements:**
- Worktree branches must follow `{id}-{slug}` (e.g. `42-fix-login-bug`). Non-conforming names silently skip status updates.
- `task-orchestrator` must be in `PATH`. Add the debug binary: `export PATH="$PATH:$(pwd)/target/debug"`
```

- [ ] **Step 23: Add MCP curl snippet**

In the `## MCP Server` section, after `Tools: \`update_task\`, \`add_note\`, \`get_task\`.`, add:

```markdown

To test tools manually (while TUI is running):
```bash
curl -s -X POST http://localhost:3142/mcp \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_task","arguments":{"task_id":1}}}'
```
```

- [ ] **Step 24: Update Conventions section**

Replace:
```
- All subprocess calls go through `src/tmux.rs` or `std::process::Command` in `src/dispatch.rs`
```
with:
```
- All subprocess calls go through `src/tmux.rs` or `src/dispatch.rs`, injected with a `ProcessRunner` (`src/process.rs`). Use `MockProcessRunner` in tests.
```

- [ ] **Step 25: Run tests one final time**

Run: `cargo test`
Expected: all pass

Run: `cargo clippy`
Expected: no warnings

- [ ] **Step 26: Final commit**

```bash
git add CLAUDE.md
git commit -m "docs: tmux prereq, hooks/branch naming, MCP curl snippet, dispatch vs resume"
```

---

## Verification

After all tasks, run:

```bash
cargo test
cargo clippy
```

Expected: all tests pass, zero clippy warnings. On next `cargo run -- tui`, a log file will be created at `~/.local/share/task-orchestrator/app.log`.
