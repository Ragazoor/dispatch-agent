# Code Review Fixes Design

**Date:** 2026-03-27
**Scope:** Address all findings from the 2026-03-27 code review

---

## Overview

Four independent workstreams to improve reliability, observability, testability, and cleanliness:

1. Database reliability — mutex poison propagation + silent fallback warnings
2. Observability — file-based tracing with `tracing` crate
3. ProcessRunner trait — injectable subprocess abstraction enabling dispatch/tmux tests
4. Cleanup — temp file RAII, duplicate parsing, tokio features, CLAUDE.md

---

## 1. Database Reliability

### Mutex Poison Propagation

All 14 `lock().unwrap()` calls in `src/db.rs` are replaced with error propagation:

```rust
let conn = self.conn.lock().map_err(|_| anyhow!("db lock poisoned"))?;
```

No method signature changes required — all `Database` methods already return `anyhow::Result`. If a thread panics while holding the lock, subsequent DB calls return an error rather than panicking, allowing the TUI to surface the failure via the existing status bar error display.

### Silent Status/Source Fallback

The two silent fallbacks in row mapping:

```rust
// before
.unwrap_or(TaskStatus::Backlog)
.unwrap_or(NoteSource::Agent)

// after
.unwrap_or_else(|| {
    tracing::warn!(raw = %value, "unrecognised task status, defaulting to Backlog");
    TaskStatus::Backlog
})
```

Behaviour is unchanged (fallback still occurs), but the raw DB value is logged at WARN level so data corruption is detectable.

---

## 2. Observability

### Dependencies

```toml
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["fmt", "env-filter"] }
```

### Initialization

In `src/main.rs`, inside the `tui` subcommand arm before `run_tui()`:

```rust
let log_path = data_dir.join("app.log");
let log_file = std::fs::OpenOptions::new()
    .create(true)
    .append(true)
    .open(&log_path)?;
tracing_subscriber::fmt()
    .with_writer(log_file)
    .with_ansi(false)
    .with_env_filter(
        tracing_subscriber::EnvFilter::from_default_env()
            .add_directive(tracing::Level::INFO.into()),
    )
    .init();
```

Log file location: `~/.local/share/task-orchestrator/app.log` (same directory as the database). Log level defaults to `INFO`, overridable via `RUST_LOG`.

### Instrumented Call Sites

| File | Events logged |
|------|--------------|
| `db.rs` | Lock poison error, silent status/source fallback |
| `dispatch.rs` | Worktree creation start/success/failure, Claude invocation command |
| `runtime.rs` | Each `exec_*` command entry, MCP server bind address |
| `mcp/handlers.rs` | Each tool call with task ID and tool name |

Use `tracing::info!` for normal operations, `tracing::warn!` for unexpected-but-recoverable situations, `tracing::error!` for failures.

---

## 3. ProcessRunner Trait

### New File: `src/process.rs`

```rust
pub trait ProcessRunner: Send + Sync {
    fn run(&self, program: &str, args: &[&str]) -> anyhow::Result<std::process::Output>;
}

pub struct RealProcessRunner;

impl ProcessRunner for RealProcessRunner {
    fn run(&self, program: &str, args: &[&str]) -> anyhow::Result<std::process::Output> {
        std::process::Command::new(program)
            .args(args)
            .output()
            .with_context(|| format!("failed to run {program}"))
    }
}
```

`MockProcessRunner` for tests:

```rust
pub struct MockProcessRunner {
    pub calls: Mutex<Vec<(String, Vec<String>)>>,
    responses: Mutex<VecDeque<anyhow::Result<std::process::Output>>>,
}

impl MockProcessRunner {
    pub fn new(responses: Vec<anyhow::Result<std::process::Output>>) -> Self { ... }
    pub fn recorded_calls(&self) -> Vec<(String, Vec<String>)> { ... }
    pub fn success() -> std::process::Output { ... }  // helper
}
```

### Threading Through Existing Code

**`src/tmux.rs`** — all 6 public functions gain a `runner: &dyn ProcessRunner` parameter. Internal `std::process::Command` calls are replaced with `runner.run(...)`.

**`src/dispatch.rs`** — `dispatch_agent`, `resume_agent`, and `cleanup_task` gain `runner: &dyn ProcessRunner`. All subprocess calls (git, tmux via tmux.rs) route through it.

**`src/runtime.rs`** — `TuiRuntime` gains `runner: Arc<dyn ProcessRunner>`. Constructed with `Arc::new(RealProcessRunner)` in `run_tui()`. Passed into dispatch/tmux calls in the relevant `exec_*` methods.

### New Tests

In `src/dispatch.rs` (`#[cfg(test)]` module):

| Test | Verifies |
|------|---------|
| `dispatch_creates_worktree_then_opens_tmux` | git worktree add called before tmux new-window |
| `dispatch_launches_claude_with_correct_args` | claude send-keys command includes task prompt |
| `resume_skips_worktree_creation` | no git commands issued, correct tmux commands |
| `cleanup_removes_worktree_and_window` | both git worktree remove and kill-window issued |
| `dispatch_fails_if_git_fails` | git error propagates, no tmux commands issued |

In `src/tmux.rs` (`#[cfg(test)]` module):

| Test | Verifies |
|------|---------|
| `new_window_issues_correct_args` | tmux new-window with expected flags |
| `capture_pane_returns_output` | stdout from mock returned as string |
| `has_window_returns_false_on_nonzero_exit` | non-zero exit → Ok(false) |

---

## 4. Cleanup

### Temp File RAII

Add `tempfile = "3"` to `Cargo.toml`.

In `src/runtime.rs` `exec_edit_task()`, replace manual temp file creation and `std::fs::remove_file` with:

```rust
let tmp = tempfile::Builder::new().suffix(".md").tempfile()?;
// write content to tmp.as_file()
// open editor with tmp.path()
// read back content
// tmp dropped here → file deleted automatically
```

### Duplicate Status Parsing

In `src/main.rs`, extract:

```rust
fn parse_status(s: &str) -> anyhow::Result<TaskStatus> {
    s.parse::<TaskStatus>()
        .map_err(|_| anyhow::anyhow!("unknown status: {s}"))
}
```

Called from both the `update` and `list` subcommand arms.

### Tokio Features

Replace `features = ["full"]` with explicit list:

```toml
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync", "time"] }
```

### CLAUDE.md Additions

Add the following to the **Build & Test** section:

> **Tmux prerequisite:** The TUI must run inside a tmux session. If you see `Error: not running inside a tmux session`, run `tmux new-session -d -s dev` first, then re-run `cargo run -- tui`.

Add a new **Hooks & Branch Naming** section:

> Status update hooks in `.claude/settings.json` extract the task ID from the current git branch name. Branches **must** follow the `{id}-{slug}` pattern (e.g. `42-fix-login-bug`) for hooks to work. Branches with non-conforming names silently skip status updates.

Add to the **MCP Server** section:

> To test MCP tools manually:
> ```bash
> curl -s -X POST http://localhost:3142/mcp \
>   -H 'Content-Type: application/json' \
>   -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_task","arguments":{"id":1}}}'
> ```

Add to the **Kanban Columns** section, under the dispatch/resume bullet:

> **Dispatch vs resume:** Dispatch creates a fresh worktree and tmux window. Closing the tmux window does not delete the worktree — the task stays Running. Press `d` again to resume: this re-opens a tmux window in the existing worktree and runs `claude --continue` to pick up where the agent left off.

---

## Constraints & Non-Goals

- No changes to the `TaskStore` trait or database schema
- No changes to the MCP protocol or tool definitions
- `MockProcessRunner` is test-only; not exported from `lib.rs`
- Logging is append-only; no log rotation (out of scope)

---

## File Change Summary

| File | Change type |
|------|------------|
| `Cargo.toml` | Add tracing, tempfile deps; narrow tokio features |
| `src/main.rs` | Add tracing init, extract parse_status helper |
| `src/process.rs` | New file — ProcessRunner trait + impls |
| `src/lib.rs` | Export process module |
| `src/db.rs` | Propagate lock errors, warn on fallbacks |
| `src/dispatch.rs` | Accept ProcessRunner, add tests |
| `src/tmux.rs` | Accept ProcessRunner, add tests |
| `src/runtime.rs` | Hold Arc<dyn ProcessRunner>, use tempfile |
| `src/mcp/handlers.rs` | Add tracing calls |
| `CLAUDE.md` | Four documentation additions |
