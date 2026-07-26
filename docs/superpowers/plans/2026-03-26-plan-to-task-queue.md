# Plan-to-Task Queue — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `task-orchestrator create --from-plan` CLI subcommand and a `/queue-plan` Claude Code skill so that plan files can be queued as Ready tasks in the kanban board without immediately dispatching agents.

**Architecture:** A new `src/plan.rs` module parses plan markdown files to extract title and description. A `find_task_by_plan` method on `Database` enables idempotency. The `Create` CLI subcommand wires these together. A project-level Claude Code custom command (`.claude/commands/queue-plan.md`) invokes the CLI.

**Tech Stack:** Rust, clap, rusqlite, Claude Code custom commands

**Spec:** `docs/superpowers/specs/2026-03-26-plan-to-task-queue-design.md`

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `src/plan.rs` | Create | Parse plan markdown: extract H1 title and Goal line |
| `src/lib.rs` | Modify | Add `pub mod plan;` |
| `src/db.rs` | Modify | Add `find_task_by_plan` method to `Database` and `TaskStore` trait |
| `src/main.rs` | Modify | Add `Create` variant to `Commands` enum and match arm |
| `.claude/commands/queue-plan.md` | Create | `/queue-plan` custom command |

---

### Task 1: Plan file parser

**Files:**
- Create: `src/plan.rs`
- Modify: `src/lib.rs:1-8`

- [ ] **Step 1: Write the failing tests**

Create `src/plan.rs` with tests only:

```rust
use anyhow::Result;

/// Metadata extracted from a plan markdown file.
#[derive(Debug, PartialEq)]
pub struct PlanMetadata {
    pub title: String,
    pub description: String,
}

/// Parse a plan markdown file and extract title and description.
///
/// - Title: first H1 heading, with trailing " — Implementation Plan" stripped
/// - Description: content of the first `**Goal:**` line, prefix removed
pub fn parse_plan(content: &str) -> Result<PlanMetadata> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_standard_plan() {
        let content = "\
# Automatic Task Status Hooks — Implementation Plan

> **For agentic workers:** ...

**Goal:** Automatically manage task status transitions via Claude Code hooks.

**Architecture:** Dispatch writes settings.json...
";
        let meta = parse_plan(content).unwrap();
        assert_eq!(meta.title, "Automatic Task Status Hooks");
        assert_eq!(
            meta.description,
            "Automatically manage task status transitions via Claude Code hooks."
        );
    }

    #[test]
    fn parse_title_without_suffix() {
        let content = "\
# Simple Feature

**Goal:** Do something simple.
";
        let meta = parse_plan(content).unwrap();
        assert_eq!(meta.title, "Simple Feature");
        assert_eq!(meta.description, "Do something simple.");
    }

    #[test]
    fn parse_missing_h1_is_error() {
        let content = "\
**Goal:** No heading here.
";
        let result = parse_plan(content);
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("H1"), "Error should mention missing H1 heading");
    }

    #[test]
    fn parse_missing_goal_gives_empty_description() {
        let content = "\
# Feature Without Goal

**Architecture:** Some architecture.
";
        let meta = parse_plan(content).unwrap();
        assert_eq!(meta.title, "Feature Without Goal");
        assert_eq!(meta.description, "");
    }

    #[test]
    fn parse_h1_with_extra_whitespace() {
        let content = "\
#   Padded Title — Implementation Plan

**Goal:**   Spaced out goal.
";
        let meta = parse_plan(content).unwrap();
        assert_eq!(meta.title, "Padded Title");
        assert_eq!(meta.description, "Spaced out goal.");
    }
}
```

- [ ] **Step 2: Register the module**

Add to `src/lib.rs` after line 1 (`pub mod db;`):

```rust
pub mod plan;
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test plan::tests`
Expected: FAIL — `todo!()` panics

- [ ] **Step 4: Implement `parse_plan`**

Replace the `todo!()` body in `src/plan.rs`:

```rust
pub fn parse_plan(content: &str) -> Result<PlanMetadata> {
    let title = content
        .lines()
        .find(|line| line.starts_with("# "))
        .ok_or_else(|| anyhow::anyhow!("No H1 heading found. Use --title to provide a title manually."))?;

    let title = title
        .trim_start_matches('#')
        .trim()
        .trim_end_matches("— Implementation Plan")
        .trim()
        .to_string();

    let description = content
        .lines()
        .find(|line| line.contains("**Goal:**"))
        .map(|line| {
            line.split("**Goal:**")
                .nth(1)
                .unwrap_or("")
                .trim()
                .to_string()
        })
        .unwrap_or_default();

    Ok(PlanMetadata { title, description })
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test plan::tests`
Expected: All 5 tests PASS

- [ ] **Step 6: Commit**

```bash
git add src/plan.rs src/lib.rs
git commit -m "feat: add plan file parser for extracting title and goal"
```

---

### Task 2: Database idempotency — `find_task_by_plan`

**Files:**
- Modify: `src/db.rs:13-26` (TaskStore trait)
- Modify: `src/db.rs:298-334` (TaskStore impl for Database)
- Modify: `src/db.rs:389-548` (tests module)

- [ ] **Step 1: Write the failing test**

Add to the end of `src/db.rs` tests module (before the final `}`):

```rust
    #[test]
    fn find_task_by_plan_returns_match() {
        let db = in_memory_db();
        let id = db.create_task("Planned", "desc", "/repo", Some("/plans/my-plan.md"), TaskStatus::Ready).unwrap();

        let found = db.find_task_by_plan("/plans/my-plan.md").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, id);
    }

    #[test]
    fn find_task_by_plan_returns_none_when_no_match() {
        let db = in_memory_db();
        db.create_task("Other", "desc", "/repo", Some("/plans/other.md"), TaskStatus::Ready).unwrap();

        let found = db.find_task_by_plan("/plans/nonexistent.md").unwrap();
        assert!(found.is_none());
    }

    #[test]
    fn find_task_by_plan_ignores_tasks_without_plan() {
        let db = in_memory_db();
        db.create_task("No Plan", "desc", "/repo", None, TaskStatus::Backlog).unwrap();

        let found = db.find_task_by_plan("/plans/any.md").unwrap();
        assert!(found.is_none());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test find_task_by_plan`
Expected: FAIL — `find_task_by_plan` method does not exist

- [ ] **Step 3: Add `find_task_by_plan` to the `TaskStore` trait**

Add to `src/db.rs` in the `TaskStore` trait (after the `save_repo_path` line, line 25):

```rust
    fn find_task_by_plan(&self, plan: &str) -> Result<Option<Task>>;
```

- [ ] **Step 4: Implement on `Database`**

Add to `src/db.rs` after the `save_repo_path` method (after line 271, before the closing `}` of `impl Database`):

```rust
    pub fn find_task_by_plan(&self, plan: &str) -> Result<Option<Task>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, title, description, repo_path, status, worktree, tmux_window,
                    plan, created_at, updated_at
             FROM tasks WHERE plan = ?1",
            params![plan],
            row_to_task,
        )
        .optional()
        .context("Failed to find task by plan")
    }
```

- [ ] **Step 5: Add the trait delegation**

Add to the `impl TaskStore for Database` block (after the `save_repo_path` delegation, line 334):

```rust
    fn find_task_by_plan(&self, plan: &str) -> Result<Option<Task>> {
        Database::find_task_by_plan(self, plan)
    }
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test find_task_by_plan`
Expected: All 3 tests PASS

- [ ] **Step 7: Commit**

```bash
git add src/db.rs
git commit -m "feat: add find_task_by_plan for idempotent task creation"
```

---

### Task 3: CLI `create --from-plan` subcommand

**Files:**
- Modify: `src/main.rs:1-5` (imports)
- Modify: `src/main.rs:19-40` (Commands enum)
- Modify: `src/main.rs:54-90` (main match)

- [ ] **Step 1: Add the `Create` variant to `Commands`**

In `src/main.rs`, add after the `List` variant (after line 39, before the closing `}` of the enum):

```rust
    /// Create a task from a plan file
    Create {
        /// Path to the plan markdown file
        #[arg(long)]
        from_plan: PathBuf,

        /// Target repository path (defaults to current directory)
        #[arg(long)]
        repo_path: Option<PathBuf>,

        /// Override the title extracted from the plan
        #[arg(long)]
        title: Option<String>,

        /// Override the description extracted from the plan
        #[arg(long)]
        description: Option<String>,
    },
```

- [ ] **Step 2: Add the import for `plan` module**

In `src/main.rs`, change line 5 from:

```rust
use task_orchestrator::{db, models, runtime};
```

to:

```rust
use task_orchestrator::{db, models, plan, runtime};
```

- [ ] **Step 3: Add the match arm**

In `src/main.rs`, add after the `Commands::List` match arm (after line 86, before the closing `}` of the match):

```rust
        Commands::Create { from_plan, repo_path, title, description } => {
            let content = std::fs::read_to_string(&from_plan)
                .map_err(|e| anyhow::anyhow!("Failed to read plan file {}: {}", from_plan.display(), e))?;

            let metadata = plan::parse_plan(&content)?;

            let title = title.unwrap_or(metadata.title);
            let description = description.unwrap_or(metadata.description);

            let repo_path = repo_path
                .or_else(|| std::env::current_dir().ok())
                .ok_or_else(|| anyhow::anyhow!("Could not determine repo path. Use --repo-path."))?;
            let repo_path_str = repo_path.to_string_lossy();

            let plan_path = std::fs::canonicalize(&from_plan)
                .map_err(|e| anyhow::anyhow!("Failed to resolve plan path {}: {}", from_plan.display(), e))?;
            let plan_str = plan_path.to_string_lossy();

            let db = db::Database::open(&cli.db)?;

            if let Some(existing) = db.find_task_by_plan(&plan_str)? {
                println!("Task #{} already exists for this plan [{}]", existing.id, existing.status.as_str());
                return Ok(());
            }

            let id = db.create_task(&title, &description, &repo_path_str, Some(&plan_str), models::TaskStatus::Ready)?;
            println!("Created task #{}: \"{}\" [ready]", id, title);
        }
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build`
Expected: Compiles without errors

- [ ] **Step 5: Manual smoke test**

Run against one of the existing plan files:

```bash
cargo run -- create --from-plan docs/superpowers/plans/2026-03-26-automatic-task-status-hooks.md
```

Expected output: `Created task #N: "Automatic Task Status Hooks" [ready]`

Run again to test idempotency:

```bash
cargo run -- create --from-plan docs/superpowers/plans/2026-03-26-automatic-task-status-hooks.md
```

Expected output: `Task #N already exists for this plan [ready]`

- [ ] **Step 6: Run full test suite**

Run: `cargo test`
Expected: All tests pass (no regressions)

- [ ] **Step 7: Commit**

```bash
git add src/main.rs
git commit -m "feat: add create --from-plan CLI subcommand"
```

---

### Task 4: `/queue-plan` custom command

**Files:**
- Create: `.claude/commands/queue-plan.md`

- [ ] **Step 1: Create the command directory**

```bash
mkdir -p .claude/commands
```

- [ ] **Step 2: Write the custom command**

Create `.claude/commands/queue-plan.md`:

```markdown
---
description: Queue a plan file as a Ready task in the task orchestrator kanban board
allowed-tools: Bash, Glob, Read
---

Queue a plan as a task in the task orchestrator.

## Instructions

1. **Find the plan file:**
   - If an argument was provided ("$ARGUMENTS"), use that as the plan file path
   - Otherwise, use Glob to find the most recently modified `.md` file in `docs/superpowers/plans/` or `docs/plans/`
   - If no plan file is found, ask the user for the path

2. **Run the CLI to create the task:**
   ```
   task-orchestrator create --from-plan <absolute-path> --repo-path <current-working-directory>
   ```

3. **Report the result to the user.** Show the task ID and title from the CLI output.
```

- [ ] **Step 3: Verify the command is recognized**

Start a new Claude Code session in this project and check that `/queue-plan` appears in the slash command list.

- [ ] **Step 4: Commit**

```bash
git add .claude/commands/queue-plan.md
git commit -m "feat: add /queue-plan custom command for plan-to-task creation"
```
