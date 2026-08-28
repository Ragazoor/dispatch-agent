# Setup Hooks Harness Dedup

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make `hook_dispatches_user_prompt_submit_event` call the `spawn_hook_harness` helper instead of re-inlining its entire body.

## Context

This work package addresses findings from the whole-repository codebase review of 2026-08-28 (`docs/plans/2026-08-28-codebase-review.md`, section 5.5).

The smallest item in the review. It is worth its own work package only because it is genuinely five minutes and needs no coordination with anything else — take it as a warm-up or fold it into another session.

## Findings

### 💡 Test harness setup duplicated instead of called (`src/setup/hooks.rs:278`)

**Issue:** `src/setup/hooks.rs` has a helper, `spawn_hook_harness` (around line 177), that builds the whole fixture a hook test needs:

1. `tempfile::tempdir()`, then `git init -q -b <branch>` with `user.email` / `user.name` set
2. A `README`, `git add .`, `git commit -q -m init`
3. Writes the embedded `hook_script()` to a real file and `chmod 0o755` so bash can execute it
4. Writes a `dispatch` shim onto `PATH` that appends its arguments to a log file, so the test can observe the call without invoking the real binary or touching the live database
5. Returns `(tmp, repo, script_path, observed, path)`

`hook_resolves_task_id_from_worktree_subdirectory` (around line 250) uses it correctly:

```rust
let (_tmp, repo, script_path, observed, path) = spawn_hook_harness("567-foo");
```

`hook_dispatches_user_prompt_submit_event` (around line 278) does not. It repeats all five steps inline — roughly 30 duplicated lines — with the branch name `"789-bar"` hardcoded instead of passed as the helper's argument. It even re-imports `PermissionsExt`, `Write`, `Command` and `Stdio` locally, which the helper's module scope already covers.

The risk is drift, not breakage: the two copies are identical today, and the moment someone fixes the harness (a new git config the test env needs, a different shim shape) they will fix one and not the other. The inlined copy is also why the review's duplication scan flagged `src/setup/hooks.rs` at lines 177–193 and 278–291 as a matching pair.

**Fix:** Replace the inlined body with a call:

```rust
let (_tmp, repo, script_path, observed, path) = spawn_hook_harness("789-bar");
```

Then delete the now-unused local `use` statements and keep only what the remainder of the test body needs — the payload construction, the `invoke_hook` call, and the assertion on the observed log.

Check the two bodies line by line before deleting. If the inlined copy differs from the helper in any way that matters to this test — a different branch name is fine and is what the argument is for, but a different `git config`, an extra file, or a differently-shaped shim is not — then the helper needs a parameter rather than the test needing a copy. Widen the helper in that case; do not force the call site to match.

Both tests are `#[cfg(unix)]`. Keep that attribute.

## Changes

| File | Change |
|------|--------|
| `src/setup/hooks.rs` | Replace the ~30 inlined harness lines in `hook_dispatches_user_prompt_submit_event` with a `spawn_hook_harness("789-bar")` call |
| `src/setup/hooks.rs` | Remove the local `use` statements the inlined body needed and the call site does not |

## Verification

- [ ] `cargo test` — all pass. In particular both hook tests: `hook_resolves_task_id_from_worktree_subdirectory` and `hook_dispatches_user_prompt_submit_event`
- [ ] Confirm `hook_dispatches_user_prompt_submit_event` still genuinely asserts on the observed log — the point of the test is that the hook invokes `dispatch hook 789 user_prompt_submit`. An extraction that accidentally drops the assertion leaves a green test that checks nothing
- [ ] `cargo clippy --all-targets -- -D warnings` — clean. Unused-import warnings from the removed `use` statements surface here
- [ ] `cargo fmt` before committing
- [ ] `git diff --stat` shows a net deletion of roughly 25–30 lines in one file, and nothing else
- [ ] Confirm `#[cfg(unix)]` is still on the test
