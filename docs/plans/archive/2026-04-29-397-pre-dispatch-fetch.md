# Pre-Dispatch Fetch Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Before creating a task worktree, fetch `origin/<base_branch>` so the agent always starts from the freshest code on the remote rather than a potentially stale local branch.

**Architecture:** Add a `git fetch origin <base_branch>` step inside `provision_worktree()` in `src/dispatch/worktree.rs`, gated on the worktree-not-yet-existing check (same gate as `git worktree add`). On success, pass `origin/<base_branch>` as the start point to `git worktree add`; on failure (no origin, network error), log a warning and fall back to the local branch. Update the Allium spec to match.

**Tech Stack:** Rust, `ProcessRunner` trait + `MockProcessRunner` for tests, `git fetch` + `git worktree add`.

---

### Task 1: Write failing tests for fetch behaviour in `provision_worktree()`

**Files:**
- Modify: `src/dispatch/tests.rs`

- [ ] **Step 1: Add four new tests after the existing `provision_worktree_*` tests (around line 730)**

Add these tests to `src/dispatch/tests.rs`:

```rust
#[test]
fn provision_worktree_fetches_origin_before_create() {
    // Fetch succeeds → worktree add should use origin/<base> as start point
    let (_dir, repo_path) = make_test_repo();

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // git fetch origin main
        MockProcessRunner::ok(), // git worktree add
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook (after-split-window)
    ]);

    let task = make_task(&repo_path);
    provision_worktree(&task, &mock, Some("main")).unwrap();

    let calls = mock.recorded_calls();
    // call[0] = git fetch origin main
    assert_eq!(calls[0].0, "git");
    assert!(calls[0].1.contains(&"fetch".to_string()), "expected fetch, got: {:?}", calls[0].1);
    assert!(calls[0].1.contains(&"origin".to_string()));
    assert!(calls[0].1.contains(&"main".to_string()));
    // call[1] = git worktree add ... origin/main
    assert_eq!(calls[1].0, "git");
    assert!(calls[1].1.contains(&"worktree".to_string()));
    assert_eq!(
        calls[1].1.last().unwrap(),
        "origin/main",
        "worktree add should use origin/main as start point, got: {:?}", calls[1].1
    );
}

#[test]
fn provision_worktree_fetch_failure_falls_back_to_local() {
    // Fetch fails → worktree add should use local branch (no error propagated)
    let (_dir, repo_path) = make_test_repo();

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::fail("fatal: 'origin' does not appear to be a git repository"), // git fetch fails
        MockProcessRunner::ok(), // git worktree add
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook (after-split-window)
    ]);

    let task = make_task(&repo_path);
    // Should NOT return an error — soft fail
    provision_worktree(&task, &mock, Some("main")).unwrap();

    let calls = mock.recorded_calls();
    // call[0] = fetch (failed)
    assert_eq!(calls[0].0, "git");
    assert!(calls[0].1.contains(&"fetch".to_string()));
    // call[1] = worktree add using local "main" (not "origin/main")
    assert_eq!(calls[1].0, "git");
    assert!(calls[1].1.contains(&"worktree".to_string()));
    assert_eq!(
        calls[1].1.last().unwrap(),
        "main",
        "fallback should use local main, got: {:?}", calls[1].1
    );
}

#[test]
fn provision_worktree_fetch_uses_custom_base_branch() {
    // Custom base_branch is used in both fetch and worktree add
    let (_dir, repo_path) = make_test_repo();

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // git fetch origin develop
        MockProcessRunner::ok(), // git worktree add
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook (after-split-window)
    ]);

    let task = make_task(&repo_path);
    provision_worktree(&task, &mock, Some("develop")).unwrap();

    let calls = mock.recorded_calls();
    // fetch uses "develop"
    assert!(calls[0].1.contains(&"develop".to_string()), "fetch should use 'develop', got: {:?}", calls[0].1);
    // worktree add uses "origin/develop"
    assert_eq!(
        calls[1].1.last().unwrap(),
        "origin/develop",
        "worktree add should use origin/develop, got: {:?}", calls[1].1
    );
}

#[test]
fn provision_worktree_skips_fetch_when_dir_exists() {
    // Pre-existing worktree dir → no git calls at all (fetch + worktree add both skipped)
    let (_dir, repo_path, _worktree_dir) = make_test_repo_with_worktree("42-fix-bug");

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook (after-split-window)
    ]);

    let task = make_task(&repo_path);
    provision_worktree(&task, &mock, Some("main")).unwrap();

    let calls = mock.recorded_calls();
    assert!(
        calls.iter().all(|(prog, _)| prog != "git"),
        "no git calls expected when worktree dir already exists, got: {calls:?}"
    );
}
```

- [ ] **Step 2: Run the new tests to confirm they fail**

```bash
cargo test provision_worktree_fetches_origin_before_create provision_worktree_fetch_failure_falls_back_to_local provision_worktree_fetch_uses_custom_base_branch provision_worktree_skips_fetch_when_dir_exists -- --test-threads=1
```

Expected: 4 failures (fetch not yet added to implementation; mock panics on unexpected calls or assertion fails).

---

### Task 2: Update the existing `provision_worktree_with_base_branch_passes_start_point` test

The current test expects the local branch name as the last git arg. After the change it will be `origin/<base>`, and the mock needs an extra response for the fetch call.

**Files:**
- Modify: `src/dispatch/tests.rs` — around line 706

- [ ] **Step 1: Replace the existing test**

Find and replace `provision_worktree_with_base_branch_passes_start_point` (lines 705–731) with:

```rust
#[test]
fn provision_worktree_with_base_branch_passes_start_point() {
    let (_dir, repo_path) = make_test_repo();

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(), // git fetch origin 99-prev-task  ← new
        MockProcessRunner::ok(), // git worktree add (with origin/99-prev-task)
        MockProcessRunner::ok(), // tmux new-window
        MockProcessRunner::ok(), // tmux set-option @dispatch_dir
        MockProcessRunner::ok(), // tmux set-hook (after-split-window)
    ]);

    let task = make_task(&repo_path);
    let result = provision_worktree(&task, &mock, Some("99-prev-task")).unwrap();

    let calls = mock.recorded_calls();
    // call[0] = fetch
    assert_eq!(calls[0].0, "git");
    assert!(calls[0].1.contains(&"fetch".to_string()));
    assert!(calls[0].1.contains(&"99-prev-task".to_string()));
    // call[1] = worktree add — start point is now origin/<base>
    assert_eq!(calls[1].0, "git");
    let git_args = &calls[1].1;
    assert_eq!(
        git_args.last().unwrap(),
        "origin/99-prev-task",
        "base branch should be origin/99-prev-task as last git arg, got: {git_args:?}"
    );

    let expected_path = format!("{repo_path}/.worktrees/42-fix-bug");
    assert_eq!(result.worktree_path, expected_path);
}
```

- [ ] **Step 2: Run to confirm it fails**

```bash
cargo test provision_worktree_with_base_branch_passes_start_point
```

Expected: FAIL (implementation not yet changed).

---

### Task 3: Implement the fetch step in `provision_worktree()`

**Files:**
- Modify: `src/dispatch/worktree.rs` — the `else` block starting at line 36

- [ ] **Step 1: Replace the `else` block in `provision_worktree()`**

Replace this section (lines 36–57):

```rust
    } else {
        let mut args = vec![
            "-C",
            &repo_path,
            "worktree",
            "add",
            &*worktree_path,
            "-B",
            &*worktree_name,
        ];
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
```

With:

```rust
    } else {
        // Fetch origin/<base_branch> so the new branch starts from the latest
        // remote state rather than a potentially stale local branch.
        // Soft-fail: if fetch is unavailable (no origin, no network), fall
        // back to the local branch and continue — dispatch is not blocked.
        let start_point: Option<String> = if let Some(base) = base_branch {
            let fetch_ok = runner
                .run("git", &["-C", &repo_path, "fetch", "origin", base])
                .map(|o| o.status.success())
                .unwrap_or(false);
            if fetch_ok {
                Some(format!("origin/{base}"))
            } else {
                tracing::warn!(base, "git fetch origin failed, falling back to local branch");
                Some(base.to_string())
            }
        } else {
            None
        };

        let mut args = vec![
            "-C",
            &repo_path,
            "worktree",
            "add",
            &*worktree_path,
            "-B",
            &*worktree_name,
        ];
        if let Some(ref sp) = start_point {
            args.push(sp.as_str());
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
```

- [ ] **Step 2: Run all provision_worktree tests**

```bash
cargo test provision_worktree
```

Expected: all pass (5 old + 4 new = 9 tests).

- [ ] **Step 3: Run the full dispatch test suite**

```bash
cargo test dispatch
```

Expected: all pass. If any dispatch test panics with "no response queued", it means a test that creates a fresh worktree now needs a fetch mock response — add `MockProcessRunner::ok()` as the first response in that test's vec.

- [ ] **Step 4: Run the full test suite**

```bash
cargo test
```

Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/dispatch/worktree.rs src/dispatch/tests.rs
git commit -m "feat: fetch origin/<base_branch> before worktree creation"
```

---

### Task 4: Update the Allium spec

**Files:**
- Modify: `docs/specs/tasks.allium` — `@guidance` block of `DispatchTask` rule (lines 119–130)

- [ ] **Step 1: Update the guidance comment**

Find the `@guidance` block in `DispatchTask` (around line 119) that reads:

```
    @guidance
        -- The worktree branch is created from task.base_branch.
        -- Specifically: `git worktree add -b <branch> <path> <base_branch>`.
        -- If the worktree directory already exists on disk (pre-created),
        -- the git worktree add step is skipped.
        -- The launched Claude command includes a plugin-dir flag so
        -- the agent discovers dispatch plugin skills (e.g. /wrap-up).
        -- The agent prompt is prepended with a `git rebase <base>`
        -- preamble so the agent picks up latest base changes on first
        -- run. All task dispatches use --permission-mode plan
        -- (see PlanTask guidance for the full rationale).
```

Replace with:

```
    @guidance
        -- Before creating the worktree, `git fetch origin <base_branch>` is
        -- run in the repo root to update the remote-tracking ref. On success,
        -- the worktree branch is created from `origin/<base_branch>` so the
        -- agent starts from the latest remote state. On fetch failure (no
        -- origin remote, network error), the step is skipped silently and the
        -- worktree falls back to the local base branch.
        -- Specifically: `git worktree add -B <branch> <path> origin/<base_branch>`.
        -- If the worktree directory already exists on disk (pre-created),
        -- the fetch and git worktree add steps are both skipped.
        -- The launched Claude command includes a plugin-dir flag so
        -- the agent discovers dispatch plugin skills (e.g. /wrap-up).
        -- The agent prompt is prepended with a `git rebase <base>`
        -- preamble so the agent picks up latest base changes on first
        -- run (also useful when an agent is resumed after time passes).
        -- All task dispatches use --permission-mode plan
        -- (see PlanTask guidance for the full rationale).
```

- [ ] **Step 2: Run allium check**

```bash
allium check docs/specs/tasks.allium
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add docs/specs/tasks.allium
git commit -m "spec: document pre-dispatch origin fetch in DispatchTask guidance"
```

---

### Task 5: Attach plan to task and verify

- [ ] **Step 1: Call `update_task` via MCP to attach the plan**

Use the `update_task` MCP tool:
```json
{
  "id": 397,
  "plan_path": "docs/plans/2026-04-29-pre-dispatch-fetch.md"
}
```

(The plan file at this path is this document. Copy it from `docs/superpowers/plans/` if needed.)

- [ ] **Step 2: Run the full test suite one final time**

```bash
cargo test
```

Expected: all pass, no warnings from clippy about unused variables.

- [ ] **Step 3: Run clippy**

```bash
cargo clippy --all-targets -- -D warnings
```

Expected: no warnings.
