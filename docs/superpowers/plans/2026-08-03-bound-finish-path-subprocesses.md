# Bounding the Finish-Path Subprocesses — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bound every git subprocess on the wrap-up rebase path and the repo-sync path with `run_with_timeout(SUBPROCESS_TIMEOUT)`, so a stalled network call or a held index lock can no longer hang `wrap_up` forever.

**Architecture:** Purely a swap of `ProcessRunner::run` for `ProcessRunner::run_with_timeout` at twelve call sites across three files, plus one injectable-timeout field on the existing `FinishContext` struct so the tests are instant. No new error variants, no new constants, no new response shapes. Spec guidance moves first, then tests, then code.

**Tech Stack:** Rust 2021, `MockProcessRunner` (`src/process.rs`) for all tests, Allium specs in `docs/specs/`.

**Design doc:** `docs/superpowers/specs/2026-08-03-wrap-up-call-time-design.md` — read it first. It records why the 1.7 s p50 is *not* being optimised.

## Global Constraints

- **This is a robustness fix, not a latency fix.** A 60 s bound would not have altered any of the 538 recorded rebase calls. Never describe this work — in a commit message, a doc, or a PR — as improving `wrap_up` latency.
- `SUBPROCESS_TIMEOUT` is 60 s, defined in `src/process.rs`. Reuse it. Do **not** introduce a new constant or a config field.
- Do **not** shorten `SUBPROCESS_TIMEOUT`. The slowest genuine recorded call is 28 s of real work; a tighter bound would fail wrap-ups that were about to succeed.
- Inline test modules need `#[allow(clippy::unwrap_used, clippy::expect_used)]` at the top. All three files already have it — don't remove it.
- No `tokio::time::sleep` anywhere under `src/`; no `std::thread::sleep` in test files. `scripts/check-no-test-sleep.sh` enforces this. The tests in this plan never sleep — see the `TEST_TIMEOUT` note in Task 3.
- Every existing test must keep passing **unmodified**, except the two `fctx` helpers in Task 3. `MockProcessRunner` answers `run` and `run_with_timeout` identically when no delay is scripted, so nothing else should need touching. If an existing test breaks, stop and work out why rather than editing it.
- Verification command for the repo: `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`
- Commit after each task. Never `git add` anything under `docs/plans/`.

---

### Task 1: Spec guidance for the bound

**Files:**
- Modify: `docs/specs/pr-workflow.allium` — the `WrapUpRebase` rule's `@guidance` block
- Modify: `docs/specs/repo-sync.allium` — the engine `@guidance` block containing "Every subprocess the engine issues is bounded by a timeout"

**Interfaces:**
- Consumes: nothing.
- Produces: the spec statements that Tasks 2–4 implement. No code symbols.

Behaviour changes start in the spec in this repo. A new failure mode (timeout) is behaviour, even though it adds no new `ensures`.

- [ ] **Step 1: Read both target guidance blocks**

Read the `WrapUpRebase` rule in `docs/specs/pr-workflow.allium` — in particular its `@guidance` block, which enumerates the finish steps 1–5. Then read the engine `@guidance` block in `docs/specs/repo-sync.allium`, the one beginning "Implementation: src/repo_sync.rs, structured and tested like src/dispatch/finish.rs".

Note the irony you are resolving: `repo-sync.allium` cites `src/dispatch/finish.rs` as the model it is structured like, and asserts every subprocess it issues is bounded — while `finish.rs` bounds none of its calls and `repo_sync.rs` leaves six unbounded.

- [ ] **Step 2: Add the bounding note to `WrapUpRebase`**

Use the `allium:tend` skill. Add to the `WrapUpRebase` `@guidance` block, after the existing steps 1–5 enumeration, a note stating in Allium comment style:

- Every subprocess in the finish path is bounded by a timeout: the pull because it touches the network, and the rebase, the conflict-path status read, the rebase abort and the fast-forward because each can block on a repository lock.
- None of them may wedge the caller. An unbounded call here hangs the agent's `wrap_up` tool call indefinitely and mints no exit token, so the session cannot be closed at all.
- A subprocess that times out is simply one that failed, reported through the existing `other` error — no new failure variant.
- The one asymmetric case: if the rebase abort times out the worktree is left mid-conflict, exactly as when the abort fails for any other reason. The rule already reports the underlying conflict rather than the abort's fate.

Match the surrounding comment style: `--` prefixed lines, wrapped at the file's existing width.

- [ ] **Step 3: Extend the `repo-sync.allium` bounding note**

The existing note names the fetch, the push and the rev-list. Extend it so it also covers what the engine actually issues and currently leaves unbounded: the preflight reads it reaches through the shared git helpers (origin-remote probe, current branch, working-tree status, default-branch detection) and the whole merge block (the merge, its conflict-path status read, and its abort).

The claim in that block is currently false for six calls. After Tasks 2 and 4 it becomes true.

- [ ] **Step 4: Validate the specs parse**

Run: `allium check docs/specs/pr-workflow.allium && allium check docs/specs/repo-sync.allium`
Expected: both pass with no errors.

- [ ] **Step 5: Commit**

```bash
git add docs/specs/pr-workflow.allium docs/specs/repo-sync.allium
git commit -m "spec(3757): every finish-path and sync-path subprocess is bounded

Records the timeout bound as guidance on WrapUpRebase, and extends
repo-sync.allium's existing bounding note to cover the preflight reads
and the merge block it claimed but did not bound.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 2: Bound the four shared `src/git.rs` helpers

**Files:**
- Modify: `src/git.rs` — the module `use` at the top, and the four helper bodies
- Test: `src/git.rs`, in the existing `mod tests`

**Interfaces:**
- Consumes: `crate::process::SUBPROCESS_TIMEOUT` (a `pub(crate) const Duration`), `ProcessRunner::run_with_timeout(&self, program: &str, args: &[&str], timeout: Duration) -> Result<Output>`.
- Produces: no signature changes. `detect_default_branch`, `has_origin_remote`, `current_branch` and `dirty_files` keep their exact current signatures and return types. Tasks 3 and 4 rely on that.

These four are shared by `finish_task` and by `repo_sync`'s `sync_repo`/`measure_repo`, so one change fixes both paths. `detect_default_branch` is included because `measure_repo` issues it, and leaving it unbounded would keep the `repo-sync.allium` claim false.

- [ ] **Step 1: Write the four failing tests**

Add to the existing `mod tests` in `src/git.rs`. `SUBPROCESS_TIMEOUT` reaches these through the module's `use super::*`, which Step 3 makes available.

```rust
    // --- subprocess bounding ---
    //
    // These four helpers are issued on both the wrap-up rebase path
    // (finish_task) and the repo-sync path (sync_repo / measure_repo), and every
    // one of them can block on a repository lock — routinely, since a human often
    // has the same checkout open while an agent wraps up. A bare `run` wedges both
    // callers with no way out, so each must carry the bound.

    #[test]
    fn detect_default_branch_bounds_its_subprocess() {
        let runner = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(
            b"refs/remotes/origin/main\n",
        )]);
        let _ = detect_default_branch("/repo", &runner);
        assert_eq!(runner.recorded_timeouts(), vec![Some(SUBPROCESS_TIMEOUT)]);
    }

    #[test]
    fn has_origin_remote_bounds_its_subprocess() {
        let runner = MockProcessRunner::new(vec![MockProcessRunner::ok()]);
        let _ = has_origin_remote("/repo", &runner);
        assert_eq!(runner.recorded_timeouts(), vec![Some(SUBPROCESS_TIMEOUT)]);
    }

    #[test]
    fn current_branch_bounds_its_subprocess() {
        let runner = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"main\n")]);
        let _ = current_branch("/repo", &runner);
        assert_eq!(runner.recorded_timeouts(), vec![Some(SUBPROCESS_TIMEOUT)]);
    }

    #[test]
    fn dirty_files_bounds_its_subprocess() {
        let runner = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"")]);
        let _ = dirty_files("/repo", &runner);
        assert_eq!(runner.recorded_timeouts(), vec![Some(SUBPROCESS_TIMEOUT)]);
    }
```

These assert the timeout was *passed*, not that it fires. That is deliberate: no delay is scripted, so nothing sleeps and the tests are instant.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test git::tests -- bounds_its_subprocess`
Expected: 4 FAIL. Each reports `left: [None]`, `right: [Some(60s)]` — `MockProcessRunner::run` records `None` in the timeout slot, `run_with_timeout` records `Some(timeout)`.

If instead you get a compile error on `SUBPROCESS_TIMEOUT` being unresolved, do Step 3's `use` line first, then re-run.

- [ ] **Step 3: Widen the module import**

In `src/git.rs`, replace:

```rust
use crate::process::ProcessRunner;
```

with:

```rust
use crate::process::{ProcessRunner, SUBPROCESS_TIMEOUT};
```

- [ ] **Step 4: Bound `detect_default_branch`**

Replace:

```rust
    if let Ok(output) = runner.run(
        "git",
        &["-C", repo_path, "symbolic-ref", "refs/remotes/origin/HEAD"],
    ) {
```

with:

```rust
    if let Ok(output) = runner.run_with_timeout(
        "git",
        &["-C", repo_path, "symbolic-ref", "refs/remotes/origin/HEAD"],
        SUBPROCESS_TIMEOUT,
    ) {
```

- [ ] **Step 5: Bound `has_origin_remote`**

Replace:

```rust
    runner
        .run("git", &["-C", repo_path, "remote", "get-url", "origin"])
        .map(|o| o.status.success())
```

with:

```rust
    runner
        .run_with_timeout(
            "git",
            &["-C", repo_path, "remote", "get-url", "origin"],
            SUBPROCESS_TIMEOUT,
        )
        .map(|o| o.status.success())
```

- [ ] **Step 6: Bound `current_branch`**

Replace:

```rust
    runner
        .run(
            "git",
            &["-C", repo_path, "rev-parse", "--abbrev-ref", "HEAD"],
        )
        .map(|output| crate::process::stdout_str(&output))
```

with:

```rust
    runner
        .run_with_timeout(
            "git",
            &["-C", repo_path, "rev-parse", "--abbrev-ref", "HEAD"],
            SUBPROCESS_TIMEOUT,
        )
        .map(|output| crate::process::stdout_str(&output))
```

- [ ] **Step 7: Bound `dirty_files`**

Replace:

```rust
    runner
        .run("git", &["-C", repo_path, "status", "--porcelain"])
        .map(|output| parse_porcelain_files(&output))
```

with:

```rust
    runner
        .run_with_timeout(
            "git",
            &["-C", repo_path, "status", "--porcelain"],
            SUBPROCESS_TIMEOUT,
        )
        .map(|output| parse_porcelain_files(&output))
```

- [ ] **Step 8: Run the new tests, then the whole `git` module**

Run: `cargo test git::tests`
Expected: all PASS, including the four new ones and every pre-existing test in the module unchanged. The pre-existing `recorded_calls()` assertions (e.g. `has_origin_remote_invokes_remote_get_url_origin`) are unaffected — `record_and_pop` records the program and argv identically on both paths.

- [ ] **Step 9: Run the suites that consume these helpers**

Run: `cargo test dispatch:: && cargo test repo_sync`
Expected: all PASS. These helpers are called from both, and nothing should regress — but this is the cheap check that proves it before moving on.

- [ ] **Step 10: Commit**

```bash
git add src/git.rs
git commit -m "fix(git): bound the four shared git helpers with SUBPROCESS_TIMEOUT

Each can block on a repository lock, and all four are issued on both the
wrap-up rebase path and the repo-sync path, where an unbounded call
wedges the caller with no way out.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 3: Bound `finish_task`, with an injectable timeout

**Files:**
- Modify: `src/dispatch/finish.rs` — imports, `FinishContext`, the destructure in `finish_task`, five call sites, and the `fctx` helper in its `mod tests`
- Modify: `src/mcp/handlers/tasks/wrap_up.rs` — the one production `FinishContext` construction
- Modify: `src/dispatch/tests.rs` — the second `fctx` helper
- Test: `src/dispatch/finish.rs`, in the existing `mod tests`

**Interfaces:**
- Consumes: the four now-bounded `crate::git` helpers from Task 2 (unchanged signatures).
- Produces: `FinishContext` gains a public field `timeout: std::time::Duration`. All three construction sites must set it. `finish_task(ctx: &FinishContext, runner: &dyn ProcessRunner) -> Result<(), FinishError>` keeps its signature — the timeout rides on the context, not a new parameter.

**Why the field exists.** `MockProcessRunner` only short-circuits a scripted delay inside `run_with_timeout`; inside the unbounded `run` it genuinely *sleeps* for it (`src/process.rs`). So a test that proves the bound is missing by scripting a 60 s stall would take 60 s to go red. An injectable bound lets the same test use 50 ms and be instant in both directions. This mirrors `provision_worktree` in `src/dispatch/worktree.rs`, which already takes a timeout documented as "use `SUBPROCESS_TIMEOUT` in production; pass a short duration in tests".

- [ ] **Step 1: Add the field and thread it through all three construction sites**

This is a compile-driven change, so it comes before the tests — the tests cannot be written against a struct that lacks the field.

In `src/dispatch/finish.rs`, add the import:

```rust
use std::time::Duration;
```

Add the field to `FinishContext`, after `base_branch`:

```rust
    /// Bound for every git subprocess `finish_task` issues. Use
    /// [`crate::process::SUBPROCESS_TIMEOUT`] in production; pass a short
    /// duration in tests, mirroring `provision_worktree` in
    /// [`crate::dispatch::worktree`]. Without this seam a test proving the
    /// bound exists would have to wait out the real 60s bound to go red.
    pub timeout: Duration,
```

Add it to the destructure at the top of `finish_task`:

```rust
    let FinishContext {
        repo_path,
        worktree,
        branch,
        base_branch,
        timeout,
    } = *ctx;
```

`Duration` is `Copy`, so the existing `*ctx` deref still works.

In `src/mcp/handlers/tasks/wrap_up.rs`, in `finish_wrap_up_rebase`, add the field to the `dispatch::FinishContext` literal:

```rust
                timeout: crate::process::SUBPROCESS_TIMEOUT,
```

In `src/dispatch/tests.rs`, add to the `fctx` helper's literal:

```rust
        timeout: std::time::Duration::from_millis(50),
```

In `src/dispatch/finish.rs`'s `mod tests`, add the constant and the field. Replace the `fctx` helper with:

```rust
    /// A bound short enough that no test ever waits on it.
    /// `MockProcessRunner::run_with_timeout` bails *without* sleeping once a
    /// scripted delay reaches the timeout, so a bounded call is instant; and on
    /// the unbounded path — which does sleep — 50ms is unnoticeable.
    const TEST_TIMEOUT: Duration = Duration::from_millis(50);

    /// Build a `FinishContext` with the standard test repo/worktree/branch,
    /// varying only the base branch the individual tests care about.
    fn fctx(base_branch: &str) -> FinishContext<'_> {
        FinishContext {
            repo_path: "/repo",
            worktree: "/repo/.worktrees/42-fix-bug",
            branch: "42-fix-bug",
            base_branch,
            timeout: TEST_TIMEOUT,
        }
    }
```

- [ ] **Step 2: Confirm it compiles and every existing test still passes**

Run: `cargo test dispatch::`
Expected: PASS. The field is threaded but not yet read by any call site, so behaviour is unchanged. Clippy may warn that `timeout` is unused — that resolves in Step 5.

- [ ] **Step 3: Write the four failing tests**

Add to the existing `mod tests` in `src/dispatch/finish.rs`.

```rust
    // --- subprocess bounding ---

    // The pull is the only network call on the finish path. Unbounded, an origin
    // that accepts the connection and then stalls hangs wrap_up forever: no exit
    // token is ever minted, so the agent cannot close its session at all.
    #[test]
    fn finish_task_bounds_the_pull() {
        let mock = MockProcessRunner::new_with_delays(vec![
            (None, MockProcessRunner::ok_with_stdout(b"main\n")), // rev-parse HEAD
            (None, MockProcessRunner::ok_with_stdout(b"")),       // status --porcelain (clean)
            (
                None,
                MockProcessRunner::ok_with_stdout(b"git@github.com:org/repo.git\n"),
            ), // remote get-url
            (Some(TEST_TIMEOUT), MockProcessRunner::ok()), // git pull — stalls past the bound
        ]);

        let err = finish_task(&fctx("main"), &mock).unwrap_err();

        assert!(
            matches!(err, FinishError::Other(ref m) if m.contains("Failed to pull") && m.contains("timed out")),
            "a stalled pull must surface as a timed-out pull, got: {err}"
        );
    }

    // `git rebase` takes the worktree index lock, which another git process in the
    // same checkout can hold indefinitely.
    #[test]
    fn finish_task_bounds_the_rebase() {
        let mock = MockProcessRunner::new_with_delays(vec![
            (None, MockProcessRunner::ok_with_stdout(b"main\n")), // rev-parse HEAD
            (None, MockProcessRunner::ok_with_stdout(b"")),       // status --porcelain (clean)
            (None, MockProcessRunner::fail("")),                  // remote get-url (no remote)
            (Some(TEST_TIMEOUT), MockProcessRunner::ok()), // git rebase — blocked on the lock
        ]);

        let err = finish_task(&fctx("main"), &mock).unwrap_err();

        assert!(
            matches!(err, FinishError::Other(ref m) if m.contains("Failed to run git rebase") && m.contains("timed out")),
            "a rebase blocked on the index lock must surface as a timeout, got: {err}"
        );
    }

    // Same for the fast-forward, which takes the repo root's index lock.
    #[test]
    fn finish_task_bounds_the_fast_forward() {
        let mock = MockProcessRunner::new_with_delays(vec![
            (None, MockProcessRunner::ok_with_stdout(b"main\n")), // rev-parse HEAD
            (None, MockProcessRunner::ok_with_stdout(b"")),       // status --porcelain (clean)
            (None, MockProcessRunner::fail("")),                  // remote get-url (no remote)
            (None, MockProcessRunner::ok()),                      // git rebase
            (Some(TEST_TIMEOUT), MockProcessRunner::ok()), // git merge --ff-only — blocked
        ]);

        let err = finish_task(&fctx("main"), &mock).unwrap_err();

        assert!(
            matches!(err, FinishError::Other(ref m) if m.contains("Failed to fast-forward") && m.contains("timed out")),
            "a blocked fast-forward must surface as a timeout, got: {err}"
        );
    }

    // The test that pins the convention rather than three instances of it: every
    // subprocess reachable on a successful finish carries the bound, including the
    // three preflight reads reached through `crate::git`. A future unbounded call
    // added anywhere on this path fails here.
    #[test]
    fn finish_task_bounds_every_subprocess_it_runs() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"main\n"), // rev-parse HEAD
            MockProcessRunner::ok_with_stdout(b""),       // status --porcelain (clean)
            MockProcessRunner::ok_with_stdout(b"git@github.com:org/repo.git\n"), // remote get-url
            MockProcessRunner::ok(),                      // git pull origin main
            MockProcessRunner::ok(),                      // git rebase main
            MockProcessRunner::ok(),                      // git merge --ff-only
        ]);

        finish_task(&fctx("main"), &mock).expect("rebase + fast-forward succeeds");

        let timeouts = mock.recorded_timeouts();
        assert_eq!(
            timeouts.len(),
            mock.recorded_calls().len(),
            "every recorded call must have a timeout slot"
        );
        assert!(
            timeouts.iter().all(|t| *t == Some(TEST_TIMEOUT)),
            "every subprocess on the finish path must be bounded, got: {timeouts:?}"
        );
    }

    // The happy path above never reaches the conflict branch, so the abort and the
    // porcelain read that precedes it need their own gate. Both are best-effort,
    // so a timeout there degrades exactly as any other failure does — but neither
    // may hang, which is what an unbounded call on a lock-taking abort would do.
    #[test]
    fn finish_task_bounds_the_conflict_abort_path() {
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"main\n"), // rev-parse HEAD
            MockProcessRunner::ok_with_stdout(b""),       // status --porcelain (clean)
            MockProcessRunner::fail(""),                  // remote get-url (no remote)
            Ok(Output {
                status: exit_fail(),
                stdout: b"CONFLICT (content): Merge conflict in lib.rs\n".to_vec(),
                stderr: vec![],
            }), // git rebase — conflicts
            MockProcessRunner::ok_with_stdout(b"UU lib.rs\n"), // status --porcelain (mid-rebase)
            MockProcessRunner::ok(),                           // git rebase --abort
        ]);

        let err = finish_task(&fctx("main"), &mock).unwrap_err();
        assert!(
            matches!(err, FinishError::RebaseConflict { .. }),
            "expected a rebase conflict, got: {err}"
        );

        let timeouts = mock.recorded_timeouts();
        assert_eq!(timeouts.len(), mock.recorded_calls().len());
        assert!(
            timeouts.iter().all(|t| *t == Some(TEST_TIMEOUT)),
            "the conflict read and the abort must be bounded too, got: {timeouts:?}"
        );
    }
```

`Output` and `exit_fail` are already in scope in this module — the existing `finish_task_rebase_conflict_in_stdout_returns_rebase_conflict` test uses both.

- [ ] **Step 4: Run the tests to verify they fail**

Run: `cargo test dispatch::finish::tests`
Expected: the five new tests FAIL; every pre-existing test in the module PASSES.

The three `bounds_the_*` tests fail because the unbounded `run` ignores the scripted delay, returns `Ok`, and `finish_task` then runs off the end of the mock script. The two `bounds_every`/`bounds_the_conflict` tests fail on the timeout assertion with `Some(50ms)` expected and `None` recorded.

The whole run should finish in well under a second. If it hangs, you scripted the delay as `SUBPROCESS_TIMEOUT` instead of `TEST_TIMEOUT` — fix that before continuing.

- [ ] **Step 5: Bound the pull**

In `finish_task`, replace:

```rust
        let output = runner
            .run(
                "git",
                &[
                    "-C",
                    repo_path,
                    "pull",
                    "--no-rebase",
                    "origin",
                    base_branch,
                ],
            )
            .map_err(|e| FinishError::Other(format!("Failed to pull: {e}")))?;
```

with:

```rust
        let output = runner
            .run_with_timeout(
                "git",
                &[
                    "-C",
                    repo_path,
                    "pull",
                    "--no-rebase",
                    "origin",
                    base_branch,
                ],
                timeout,
            )
            .map_err(|e| FinishError::Other(format!("Failed to pull: {e}")))?;
```

- [ ] **Step 6: Bound the rebase**

Replace:

```rust
    let output = runner
        .run("git", &["-C", worktree, "rebase", base_branch])
        .map_err(|e| FinishError::Other(format!("Failed to run git rebase: {e}")))?;
```

with:

```rust
    let output = runner
        .run_with_timeout("git", &["-C", worktree, "rebase", base_branch], timeout)
        .map_err(|e| FinishError::Other(format!("Failed to run git rebase: {e}")))?;
```

- [ ] **Step 7: Bound the conflict-path status read and the abort**

Replace:

```rust
        let conflicted_files = if is_conflict {
            runner
                .run("git", &["-C", worktree, "status", "--porcelain"])
                .map(|o| parse_unmerged_files(&o))
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        let _ = runner.run("git", &["-C", worktree, "rebase", "--abort"]);
```

with:

```rust
        let conflicted_files = if is_conflict {
            runner
                .run_with_timeout(
                    "git",
                    &["-C", worktree, "status", "--porcelain"],
                    timeout,
                )
                .map(|o| parse_unmerged_files(&o))
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        let _ = runner.run_with_timeout(
            "git",
            &["-C", worktree, "rebase", "--abort"],
            timeout,
        );
```

- [ ] **Step 8: Bound the fast-forward**

Replace:

```rust
    let output = runner
        .run("git", &["-C", repo_path, "merge", "--ff-only", branch])
        .map_err(|e| FinishError::Other(format!("Failed to fast-forward {base_branch}: {e}")))?;
```

with:

```rust
    let output = runner
        .run_with_timeout(
            "git",
            &["-C", repo_path, "merge", "--ff-only", branch],
            timeout,
        )
        .map_err(|e| FinishError::Other(format!("Failed to fast-forward {base_branch}: {e}")))?;
```

- [ ] **Step 9: Run the tests to verify they pass**

Run: `cargo test dispatch::finish::tests`
Expected: all PASS, in well under a second.

- [ ] **Step 10: Run the wider dispatch and MCP suites**

Run: `cargo test dispatch:: && cargo test mcp::`
Expected: all PASS. `src/dispatch/tests.rs` has 12 `finish_task` call sites, all routed through the `fctx` you updated in Step 1, and `wrap_up.rs` is the only production caller.

- [ ] **Step 11: Commit**

```bash
git add src/dispatch/finish.rs src/dispatch/tests.rs src/mcp/handlers/tasks/wrap_up.rs
git commit -m "fix(dispatch): bound every subprocess finish_task issues

finish_task bounded none of its five git calls. The pull could stall on a
network round-trip and the rebase, abort and fast-forward on a held index
lock — hanging wrap_up with no exit token minted, so the agent could not
close its session at all.

The timeout rides on FinishContext so tests can inject a short bound,
mirroring provision_worktree. Not a latency change: a 60s bound would not
have altered any recorded call.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 4: Bound `sync_repo`'s merge block

**Files:**
- Modify: `src/repo_sync.rs` — the three unbounded calls in the merge block
- Test: `src/repo_sync.rs`, in the existing `mod tests`

**Interfaces:**
- Consumes: `SUBPROCESS_TIMEOUT`, already imported at the top of `src/repo_sync.rs`; the Task 2 helpers.
- Produces: no signature changes. `sync_repo(repo_path: &str, base_branch: &str, runner: &dyn ProcessRunner) -> Result<SyncOutcome, SyncError>` is unchanged.

The fetch, rev-list and push here are already bounded. The merge block is not, and it mirrors `finish_task`'s rebase block site for site. Keep `SUBPROCESS_TIMEOUT` inline — these tests assert the timeout was *passed*, never that it fires, so they script no delay and never sleep. No injectable seam is needed.

- [ ] **Step 1: Write the two failing tests**

Add to the existing `mod tests` in `src/repo_sync.rs`. `REPO`, `BASE` and the `responses` helper are already defined there.

```rust
    // repo-sync.allium's engine guidance claims every subprocess it issues is
    // bounded by a timeout. This is the test that makes the claim true rather than
    // aspirational, and it covers the preflight reads reached through `crate::git`
    // as well as the engine's own calls.
    #[test]
    fn sync_repo_bounds_every_subprocess_it_runs() {
        let mock = MockProcessRunner::new(responses(vec![
            MockProcessRunner::ok(),                      // fetch
            MockProcessRunner::ok_with_stdout(b"0\t2\n"), // rev-list: behind 2
            MockProcessRunner::ok(),                      // merge
            MockProcessRunner::ok_with_stdout(b"0\t0\n"), // recount
        ]));

        sync_repo(REPO, BASE, &mock).expect("a behind repo fast-forwards");

        let timeouts = mock.recorded_timeouts();
        assert_eq!(
            timeouts.len(),
            mock.recorded_calls().len(),
            "every recorded call must have a timeout slot"
        );
        assert!(
            timeouts.iter().all(|t| *t == Some(SUBPROCESS_TIMEOUT)),
            "every subprocess on the sync path must be bounded, got: {timeouts:?}"
        );
    }

    // The happy path never reaches the conflict branch, so the porcelain read and
    // the merge abort need their own gate — the same split as finish_task's
    // conflict path.
    #[test]
    fn sync_repo_bounds_the_conflict_abort_path() {
        let mock = MockProcessRunner::new(responses(vec![
            MockProcessRunner::ok(),                          // fetch
            MockProcessRunner::ok_with_stdout(b"0\t2\n"),     // rev-list: behind 2
            MockProcessRunner::fail("CONFLICT"),              // merge fails
            MockProcessRunner::ok_with_stdout(b"UU lib.rs\n"), // status --porcelain
            MockProcessRunner::ok(),                          // merge --abort
        ]));

        let err = sync_repo(REPO, BASE, &mock).expect_err("a conflicted merge fails");
        assert!(
            matches!(err, SyncError::MergeConflict { .. }),
            "expected a merge conflict, got: {err}"
        );

        let timeouts = mock.recorded_timeouts();
        assert_eq!(timeouts.len(), mock.recorded_calls().len());
        assert!(
            timeouts.iter().all(|t| *t == Some(SUBPROCESS_TIMEOUT)),
            "the conflict read and the abort must be bounded too, got: {timeouts:?}"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test repo_sync::tests -- bounds`
Expected: 2 FAIL on the timeout assertion — the merge, the porcelain read and the abort record `None`.

Note: after Task 2 the three preflight reads inside `responses()` already record `Some(SUBPROCESS_TIMEOUT)`, so the failure isolates exactly the merge block. If these tests unexpectedly *pass*, Task 2 was not applied — stop and check.

- [ ] **Step 3: Bound the merge, the conflict read and the abort**

In `sync_repo`, replace:

```rust
        let output = runner
            .run(
                "git",
                &[
                    "-C",
                    &repo,
                    "merge",
                    "--no-edit",
                    &format!("origin/{base_branch}"),
                ],
            )
            .map_err(|e| SyncError::Other(format!("Failed to run git merge: {e}")))?;
```

with:

```rust
        let output = runner
            .run_with_timeout(
                "git",
                &[
                    "-C",
                    &repo,
                    "merge",
                    "--no-edit",
                    &format!("origin/{base_branch}"),
                ],
                SUBPROCESS_TIMEOUT,
            )
            .map_err(|e| SyncError::Other(format!("Failed to run git merge: {e}")))?;
```

Then replace:

```rust
            let conflicted = runner
                .run("git", &["-C", &repo, "status", "--porcelain"])
                .map(|o| crate::git::parse_unmerged_files(&o))
                .unwrap_or_default();
            let _ = runner.run("git", &["-C", &repo, "merge", "--abort"]);
```

with:

```rust
            let conflicted = runner
                .run_with_timeout(
                    "git",
                    &["-C", &repo, "status", "--porcelain"],
                    SUBPROCESS_TIMEOUT,
                )
                .map(|o| crate::git::parse_unmerged_files(&o))
                .unwrap_or_default();
            let _ = runner.run_with_timeout(
                "git",
                &["-C", &repo, "merge", "--abort"],
                SUBPROCESS_TIMEOUT,
            );
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test repo_sync`
Expected: all PASS, including every pre-existing test unchanged.

- [ ] **Step 5: Commit**

```bash
git add src/repo_sync.rs
git commit -m "fix(repo-sync): bound the merge, conflict read and abort

The fetch, rev-list and push were already bounded; the merge block was
not, so repo-sync.allium's claim that every subprocess the engine issues
is bounded was false for six calls. With src/git.rs already fixed, this
makes it true.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 5: Verify alignment and close out

**Files:**
- No source changes expected. Only spec/doc touch-ups if `allium:weed` finds drift.

**Interfaces:**
- Consumes: everything from Tasks 1–4.
- Produces: nothing.

- [ ] **Step 1: Check spec and code agree**

Use the `allium:weed` skill on `docs/specs/pr-workflow.allium` and `docs/specs/repo-sync.allium`.

Expected: no divergence on the bounding guidance added in Task 1. If weed reports drift, fix the spec to match what Tasks 2–4 actually implemented — the code is the ground truth here, since the spec text was written before the code.

- [ ] **Step 2: Run the full verification command**

Run: `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`
Expected: all three PASS.

If `cargo fmt --check` fails, run `cargo fmt` and re-check the diff before staging — a scoped `cargo fmt` in this repo has been observed reformatting unrelated files, so read the diff rather than trusting the scope.

- [ ] **Step 3: Run clippy at the pre-push gate's strictness**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: PASS. A plain `cargo build` does **not** fail on clippy lints, so this is the only check that matches what the pre-push hook enforces. Pay attention to any `unused` warning on the new `timeout` field — that would mean a call site was missed.

- [ ] **Step 4: Record the learning**

Use the `/learnings` skill (`record_learning`) to capture the measurement, so the next agent asking "why is `wrap_up` slow?" does not redo the trajectory analysis:

> `wrap_up` latency is entirely `action="rebase"`: `done`/`pr` are ~5 ms, `rebase` is p50 1.7 s and that is one `git pull` network round-trip in `finish_task`, not a defect. Local git steps are ~11 ms. Trajectory logs at `~/.local/share/dispatch/trajectories/*.jsonl` carry a `duration_ms` per MCP call — aggregate those before theorising about MCP latency.

Also rate the learnings that were surfaced for this task via `rate_learning`.

- [ ] **Step 5: Commit any weed fixes**

Only if Step 1 produced changes:

```bash
git add docs/specs/pr-workflow.allium docs/specs/repo-sync.allium
git commit -m "spec(3757): align bounding guidance with the implementation

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

## Out of scope

- Reducing the 1.7 s p50. Deliberately not attempted — see the design doc's rejected options.
- Any new timing constant, config flag, or background poll loop.
- Unbounded `runner.run` calls outside the finish and sync paths. Bounding those is defensible but has a different blast radius and belongs to a separate task.
