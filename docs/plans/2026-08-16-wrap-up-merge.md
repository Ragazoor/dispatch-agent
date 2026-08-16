# `wrap_up(action="merge")` — Implementation Plan

> **For agentic workers:** Use TDD throughout — write the failing test before the code that makes it pass. Update `docs/specs/pr-workflow.allium`, `docs/specs/core.allium` (WrapUpMode enum), and `docs/specs/repo-sync.allium` (BranchMerged as a second RefreshRepoSyncState trigger) via `allium:tend`, then verify with `allium:weed`.

**Goal:** Add a fourth `wrap_up` action, `merge`, that squash-merges the task's worktree branch into `base_branch` without rewriting either branch's history — safe for a worktree whose own branch is itself a long-lived, shared branch (unlike `rebase`, which is only safe because it rewrites a disposable per-task branch).

**Architecture:** A new `finish_task_merge` function in `src/dispatch/finish.rs`, mirroring `finish_task`'s shape (preflight checks, then `git merge --squash <branch>` + commit in the repo root instead of rebase+ff, with its own conflict handling); a new `WrapUpAction::Merge` variant threaded through the existing `handle_wrap_up`/`handle_exit_session` match arms; a `BranchMerged` event mirroring `BranchRebased`.

**Tech Stack:** Rust, the existing `ProcessRunner`/`MockProcessRunner` test harness, the existing MCP `wrap_up`/`exit_session` handlers.

**Spec:** `docs/superpowers/specs/2026-08-16-staging-pipeline-scheduled-agents-design.md` ("New wrap-up action: merge" section)

## Global Constraints

- `wrap_up(action="rebase")` and `finish_task` must NOT be modified by this plan — `merge` is additive, a sibling to `rebase`, not a replacement or a refactor of it.
- `wrap_up(action="merge")` never pushes to origin. This plan does not add any `git push` call anywhere. (Auto-push for the scheduled-pipeline case is a separate, narrowly-scoped hook in the scheduling-primitive work, not part of this action's own mechanics.)
- Squash-merge only, never a `--no-ff` merge commit (per the design doc's confirmed decision) — `git merge --squash <branch>` followed by an explicit `git commit`.
- Every subprocess added must go through `ProcessRunner`, be bounded by a timeout, and use `MockProcessRunner` for tests — same discipline as every other call in `finish.rs`.
- Conflict-path files must be read (`git status --porcelain` / `parse_unmerged_files`) **before** the abort (`git merge --abort`), exactly like `finish_task`'s rebase-conflict handling reads unmerged files before `git rebase --abort` — never after.

---

## File Structure

- Modify `src/mcp/mod.rs:91-119` — `WrapUpAction` enum (`Rebase, Done, Pr` -> `+ Merge`), `ExitToken`'s action field (already generic over `WrapUpAction`, no change needed beyond the enum itself).
- Modify `src/dispatch/finish.rs` — new `finish_task_merge(ctx: &FinishContext, runner: &dyn ProcessRunner) -> Result<(), FinishError>`, new `FinishError::MergeConflict { branch: String, files: Vec<String> }` variant.
- Modify `src/dispatch/git_output.rs` — verify (and if needed, extend) `is_rebase_conflict`-equivalent conflict detection works for `merge --squash` output; add `is_merge_conflict` if the marker differs (check before assuming).
- Modify `src/mcp/handlers/tasks/wrap_up.rs` — `handle_wrap_up`'s match (line 237-260) gains a `Merge` arm calling `finish_wrap_up_merge` (new function, alongside `finish_wrap_up_rebase`); `wrap_up_verify_line` (38-56), `exit_instruction` (16-25) gain `Merge` arms; `handle_exit_session`'s terminal-outcome match (343-353) adds `Merge` to the `CloseSessionOutcome::Done` arm.
- Modify `src/models/tasks.rs:614-625` — `WrapUpMode` enum (`Rebase, Pr, Done` -> `+ Merge`), bump the `ALL.len() == 3` test assertion (line 1274) to 4.
- Modify `src/mcp/handlers/dispatch.rs:102-107, 185, 249` — `wrap_up_action_enum_values()` picks up `Merge` automatically from `WrapUpAction::ALL`/`WrapUpMode::ALL`; hand-edit the two hardcoded description strings at those lines to mention `merge`.
- Modify TUI wrap_up_mode picker: `src/tui/input.rs:662`, `src/tui/ui/input_form.rs:299`, `src/tui/update/forms.rs:142`, `src/editor.rs` (~813-860) — add a 4th keybinding (e.g. `m`) and line.
- Modify `plugin/skills/wrap-up/SKILL.md` — add the `merge` path to the argument parsing, the Step 4 choice prompt, and the mechanics table.
- Modify `docs/specs/repo-sync.allium` — `RefreshRepoSyncStateAfterRebase`'s `when: BranchRebased(_, _, repo_path)` trigger gains `BranchMerged` as a second trigger (a merge also moves local `base_branch` ahead of origin, same as a rebase does).
- Test: `src/mcp/handlers/tests/tasks/wrap_up.rs` (new `wrap_up_merge_*`/`exit_session_full_flow_merge` tests, mirroring the existing `wrap_up_rebase_*` ones), `src/dispatch/finish.rs`'s inline tests (new `finish_task_merge_*` tests mirroring `finish_task`'s), `src/models/tasks.rs`'s `WrapUpMode::ALL` test, `src/tui/tests/input_handlers.rs` (new `wrap_up_mode_m_selects_merge_and_creates_task` test mirroring the `_r_`/`_p_`/`_d_` ones at lines 2540-2679), `src/tui/tests/usage.rs:526` (keybinding telemetry coverage for the new key).

---

## Task 1: `WrapUpAction`/`WrapUpMode` enum + exhaustive call sites

**Files:**
- Modify: `src/mcp/mod.rs:91-110`, `src/models/tasks.rs:614-625` and `:1274`.
- Test: the `ALL.len()` assertion becomes the first failing test.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn wrap_up_mode_all_has_four_variants() {
    assert_eq!(WrapUpMode::ALL.len(), 4);
    assert!(WrapUpMode::ALL.contains(&WrapUpMode::Merge));
}
```

- [ ] **Step 2: Run test, confirm it fails** (compile error: no `Merge` variant).

- [ ] **Step 3: Add the variant to both enums**

```rust
// src/mcp/mod.rs
pub(crate) enum WrapUpAction { Rebase, Done, Pr, Merge }
```
```rust
// src/models/tasks.rs
pub enum WrapUpMode { Rebase, Pr, Done, Merge }
```

(Both use whatever derive/macro machinery is already there — `#[serde(rename_all = "lowercase")]` for `WrapUpAction`, `define_str_enum!` for `WrapUpMode` — add `Merge` in the same style, nothing new invented.)

- [ ] **Step 4: `cargo build`, fix every compile error the compiler surfaces** from the now-4-variant enums. Expect errors in: `src/mcp/handlers/dispatch.rs` (JSON schema builders at lines 102-107, 185, 249 — these are likely non-exhaustive `Vec`/`.map()` builders over `ALL`, so may just work once `ALL` includes `Merge`, but check), `src/mcp/handlers/tasks/wrap_up.rs` (multiple exhaustive `match action { .. }` sites — `handle_wrap_up`'s dispatch, `wrap_up_verify_line`, `exit_instruction`, `handle_exit_session`'s terminal-outcome match), TUI picker code (`input.rs:662`, `input_form.rs:299`, `forms.rs:142`, `editor.rs` ~813-860).

  For THIS task, add `Merge` to every exhaustive match with a placeholder-free but temporary behavior: route it identically to `Rebase` everywhere for now (this task's job is just "the enum has 4 variants and compiles"; Task 2 below implements `merge`'s actual distinct mechanics). Do not use `_ => unimplemented!()` — use a real, working temporary branch (e.g. `Merge => /* same as Rebase for now, replaced in Task 2 */ ...`).

- [ ] **Step 5: Run test, confirm it passes; run the full existing wrap_up/wrap_up_mode test suites to confirm nothing broke** (`cargo test wrap_up`, `cargo test tui::tests::input_handlers`).

- [ ] **Step 6: Hand-edit the two description strings** at `dispatch.rs:184`/wherever the `create_task`/`update_task` tool-schema description text lists `'rebase' ... 'pr' ... or 'done'` — add `merge`.

- [ ] **Step 7: Commit**

```bash
git add src/mcp/mod.rs src/models/tasks.rs src/mcp/handlers/dispatch.rs src/mcp/handlers/tasks/wrap_up.rs src/tui/
git commit -m "feat(wrap-up): add Merge variant to WrapUpAction/WrapUpMode (routes as Rebase temporarily)"
```

---

## Task 2: `finish_task_merge` mechanics

**Files:**
- Modify: `src/dispatch/finish.rs` (new `finish_task_merge`, new `FinishError::MergeConflict`).
- Modify: `src/dispatch/git_output.rs` (conflict detection, if needed).
- Test: inline `mod tests` in `finish.rs`, using `MockProcessRunner`.

**Interfaces:**
- Consumes: `FinishContext { repo_path, worktree, branch, base_branch, timeout }` (existing struct, `finish.rs:58-73` — unchanged).
- Produces: `pub fn finish_task_merge(ctx: &FinishContext, runner: &dyn ProcessRunner) -> Result<(), FinishError>`; `FinishError::MergeConflict { branch: String, files: Vec<String> }`.

- [ ] **Step 1: Write the failing happy-path test**

```rust
#[test]
fn finish_task_merge_squash_merges_cleanly() {
    let runner = MockProcessRunner::new(vec![
        ok_with_stdout("main"),        // current_branch(repo_path) == base_branch
        ok_with_stdout(""),            // dirty_files(repo_path) empty
        ok(),                          // git pull --no-rebase origin main
        ok(),                          // git -C repo_path merge --squash staging
        ok(),                          // git -C repo_path commit -m "..."
    ]);
    let ctx = test_finish_context(); // existing helper, base_branch="main", branch="staging"
    finish_task_merge(&ctx, &runner).expect("should succeed");
}
```

- [ ] **Step 2: Run test, confirm it fails** (function doesn't exist).

- [ ] **Step 3: Implement**, reusing `finish_task`'s preflight steps 1-3 verbatim (same `current_branch`/`dirty_files`/pull calls, same `NotOnDefaultBranch`/`DirtyPrimaryWorktree` errors) and replacing steps 4-5 (rebase + ff-merge) with:

```rust
pub fn finish_task_merge(ctx: &FinishContext, runner: &dyn ProcessRunner) -> Result<(), FinishError> {
    // Steps 1-3: identical to finish_task — verify on base_branch, clean, pull.
    // (Extract these three into a shared preflight helper if finish_task's
    // current structure makes that clean; otherwise duplicate the three
    // calls exactly, since they're only ~10 lines and finish_task must not
    // be refactored by this plan.)
    preflight_checks(ctx, runner)?;

    // Squash-merge in the REPO ROOT (never the worktree) — this is what
    // guarantees the worktree's own branch is never rewritten.
    let merge_result = run_bounded(
        runner,
        &ctx.repo_path,
        &["merge", "--squash", &ctx.branch],
        ctx.timeout,
    );
    if let Err(e) = merge_result {
        if is_merge_conflict(&e) {
            // Read unmerged paths BEFORE aborting — aborting clears them.
            let files = parse_unmerged_files(&status_porcelain(&ctx.repo_path, runner)?);
            let _ = run_bounded(runner, &ctx.repo_path, &["merge", "--abort"], ctx.timeout);
            return Err(FinishError::MergeConflict { branch: ctx.branch.clone(), files });
        }
        return Err(FinishError::Other(e.to_string()));
    }

    run_bounded(
        runner,
        &ctx.repo_path,
        &["commit", "-m", &format!("Merge {} into {}", ctx.branch, ctx.base_branch)],
        ctx.timeout,
    )
    .map_err(|e| FinishError::Other(e.to_string()))?;

    Ok(())
}
```

(Read `finish_task`'s actual current code at `finish.rs:83-194` before writing this — match `run_bounded`/`status_porcelain`/`parse_unmerged_files`'s real signatures exactly; the above is illustrative shape, not verbatim. Confirm whether `is_rebase_conflict` (`git_output.rs`) already matches git's `--squash`-conflict stdout/stderr markers — git uses the same `CONFLICT (content): Merge conflict in <file>` marker for both rebase and merge conflicts, so it likely can be reused directly as `is_merge_conflict` (a rename/alias) rather than needing new detection logic; verify this against the function's actual implementation before assuming.)

Add `MergeConflict { branch: String, files: Vec<String> }` to `FinishError` alongside `RebaseConflict`.

- [ ] **Step 4: Run test, confirm it passes.**

- [ ] **Step 5: Write and pass the conflict test**

```rust
#[test]
fn finish_task_merge_conflict_reports_files_and_aborts() {
    let runner = MockProcessRunner::new(vec![
        ok_with_stdout("main"),
        ok_with_stdout(""),
        ok(),
        fail_with_stderr("CONFLICT (content): Merge conflict in src/foo.rs"), // git merge --squash
        ok_with_stdout(" UU src/foo.rs"),  // git status --porcelain, read before abort
        ok(),                              // git merge --abort
    ]);
    let ctx = test_finish_context();
    let err = finish_task_merge(&ctx, &runner).unwrap_err();
    match err {
        FinishError::MergeConflict { files, .. } => assert_eq!(files, vec!["src/foo.rs".to_string()]),
        other => panic!("expected MergeConflict, got {other:?}"),
    }
}
```

- [ ] **Step 6: Write and pass the dirty-primary-worktree test and the not-on-base-branch test**, mirroring `finish_task`'s existing equivalents exactly (same preflight, so same failure modes).

- [ ] **Step 7: Commit**

```bash
git add src/dispatch/finish.rs src/dispatch/git_output.rs
git commit -m "feat(dispatch): add finish_task_merge — squash-merge without rewriting either branch"
```

---

## Task 3: Wire `merge` into `handle_wrap_up`/`handle_exit_session`

**Files:**
- Modify: `src/mcp/handlers/tasks/wrap_up.rs` — replace Task 1's temporary "route Merge as Rebase" with real behavior.
- Test: `src/mcp/handlers/tests/tasks/wrap_up.rs`.

**Interfaces:**
- Consumes: `finish_task_merge` (Task 2); existing `finish_wrap_up_rebase`/`finish_wrap_up_simple` shapes (`wrap_up.rs:237-260`) — read these directly to match `finish_wrap_up_merge`'s signature to them.

- [ ] **Step 1: Write the failing test — full merge flow**

```rust
#[tokio::test]
async fn wrap_up_merge_returns_started_and_exit_token() {
    let state = make_merge_state(); // new helper, mirroring make_rebase_state (wrap_up.rs:1275)
    let runner = merge_ok_runner();  // new helper, mirroring rebase_ok_runner (wrap_up.rs:716)
    let result = handle_wrap_up(&state, wrap_up_args(task_id, WrapUpAction::Merge), &runner).await;
    assert!(result.is_ok());
    // assert an exit token was minted recording action = Merge, same shape as the rebase test
}

#[tokio::test]
async fn exit_session_full_flow_merge() {
    // mirrors exit_session_full_flow_rebase / _rebase_closes_session_in_one_call:
    // wrap_up(merge) -> exit_session(token, action="merge") -> task.status == Done, tmux_window cleared
}
```

- [ ] **Step 2: Run tests, confirm they fail** (currently routed as Rebase, which calls `finish_task`/`finish_wrap_up_rebase` instead of the merge path — tests should fail on wrong subprocess calls or wrong exit-token bookkeeping if the test asserts merge-specific mock calls).

- [ ] **Step 3: Implement `finish_wrap_up_merge`**, mirroring `finish_wrap_up_rebase`'s shape exactly but calling `finish_task_merge` instead of `finish_task`, and emitting `BranchMerged { branch, onto: base_branch, repo_path }` on success (new event, mirroring `BranchRebased`). Wire `handle_wrap_up`'s match:

```rust
match parsed.action {
    WrapUpAction::Done | WrapUpAction::Pr => finish_wrap_up_simple(...).await,
    WrapUpAction::Rebase => finish_wrap_up_rebase(state, id, task).await,
    WrapUpAction::Merge => finish_wrap_up_merge(state, id, task).await,
}
```

Update `handle_exit_session`'s terminal-outcome match (`:343-353`):

```rust
let outcome = match (action, pr_url) {
    (WrapUpAction::Pr, Some(pr_url)) => CloseSessionOutcome::Review { pr_url: TaskUrl::new(pr_url, UrlType::Pr) },
    (WrapUpAction::Pr, None) | (WrapUpAction::Rebase, _) | (WrapUpAction::Done, _) | (WrapUpAction::Merge, _) => CloseSessionOutcome::Done,
};
```

Update `wrap_up_verify_line` (`:38-56`) — `merge` joins `Rebase`'s branch (it performs a real git operation on `base_branch` after the skill's pre-check, same as rebase), but needs its own wording since "this rebase" would misdescribe a merge:

```rust
WrapUpAction::Rebase => format!("Verify before exiting: this rebase may have pulled in changes since you last checked — run `{cmd}` and confirm it passes."),
WrapUpAction::Merge => format!("Verify before exiting: this merge may have changed `{base_branch}` since you last checked — run `{cmd}` and confirm it passes."),
WrapUpAction::Pr | WrapUpAction::Done => format!("Verify before exiting: if you haven't already run `{cmd}` and confirmed it passes earlier in this wrap-up, do so now."),
```

Update `exit_instruction` (`:16-25`) — `Merge` joins the no-extra-arg arm (`Rebase | Done`), since it never needs `pr_url`.

- [ ] **Step 4: Run tests, confirm they pass.**

- [ ] **Step 5: Write and pass a merge-conflict-surfaces-as-error test**, mirroring `wrap_up_rebase_conflict_returns_error`.

- [ ] **Step 6: Commit**

```bash
git add src/mcp/handlers/tasks/wrap_up.rs src/mcp/handlers/tests/tasks/wrap_up.rs
git commit -m "feat(wrap-up): implement merge action mechanics in handle_wrap_up/handle_exit_session"
```

---

## Task 4: TUI wrap-up-mode picker + `/wrap-up` skill doc

**Files:**
- Modify: `src/tui/input.rs:662`, `src/tui/ui/input_form.rs:299`, `src/tui/update/forms.rs:142`, `src/editor.rs` (~813-860).
- Modify: `plugin/skills/wrap-up/SKILL.md`.
- Test: `src/tui/tests/input_handlers.rs` (new `wrap_up_mode_m_selects_merge_and_creates_task`), `src/tui/tests/usage.rs:526`.

- [ ] **Step 1: Write the failing test**, mirroring `wrap_up_mode_r_selects_rebase_and_creates_task` (lines 2540-2679) exactly, substituting `m`/`Merge`.

- [ ] **Step 2: Run test, confirm it fails.**

- [ ] **Step 3: Add the `m` keybinding** to `handle_key_wrap_up_mode` (`input.rs:662`, same `handle_char_picker` mechanism already used for `r`/`p`/`d`), and the corresponding line in `input_wrap_up_mode_lines` (`input_form.rs:299`) and the editor's wrap_up_mode section (`editor.rs` ~813-860).

- [ ] **Step 4: Run test, confirm it passes.**

- [ ] **Step 5: Update `plugin/skills/wrap-up/SKILL.md`** — argument parsing (`/wrap-up rebase|pr|done|merge`), the Step 4 choice prompt, and the mechanics table, adding merge's row: performs a squash-merge into `base_branch` (never rewrites the task's own branch), never pushes, same verify-before-exit discipline as rebase.

- [ ] **Step 6: Commit**

```bash
git add src/tui/ src/editor.rs plugin/skills/wrap-up/SKILL.md
git commit -m "feat(tui): add merge keybinding to wrap-up-mode picker; document in /wrap-up skill"
```

---

## Task 5: `docs/specs` alignment

- [ ] Use `allium:tend` to add a `WrapUpMerge` rule to `pr-workflow.allium` (mirroring `WrapUpRebase`'s shape — preconditions, `BranchMerged` emission, the "why squash not rebase" rationale from the design doc), add `merge` to `core.allium`'s `WrapUpMode` enum doc, and add `BranchMerged` as a second trigger on `repo-sync.allium`'s `RefreshRepoSyncStateAfterRebase` rule (rename/broaden it if the spec-writing convention prefers, e.g. to `RefreshRepoSyncStateAfterIntegration` — use judgement, but keep it minimal).
- [ ] Run `allium:weed` to confirm alignment; fix any drift found.
- [ ] Commit spec changes separately (`docs: add WrapUpMerge to pr-workflow.allium, update core.allium/repo-sync.allium`).

---

## Self-Review Notes

- Task 1's "route Merge as Rebase temporarily" step is a deliberate, short-lived scaffolding step to keep the build green while wiring the enum through every exhaustive match — Task 3 replaces it with real behavior in the same plan. Do not leave Task 1's temporary routing in place after Task 3 lands; grep for any leftover comment referencing it before the final commit of this plan.
- `is_merge_conflict`/`is_rebase_conflict` sharing: confirmed only by re-reading `git_output.rs` in Task 2 — don't assume without checking; if the markers differ, write `is_merge_conflict` as its own function rather than forcing reuse.
