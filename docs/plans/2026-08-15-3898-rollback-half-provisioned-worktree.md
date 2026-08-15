# Roll back a half-provisioned worktree when dispatch fails after `git worktree add` (#3898)

## Problem

`provision_worktree` (`src/dispatch/worktree.rs::provision_worktree`) creates a
fresh worktree with `git worktree add`, then runs `tmux new-window`,
`set_window_dispatch_dir`, and `ensure_split_hook`. `dispatch_with_prompt`
(`src/dispatch/agents.rs`) then writes `.claude-prompt` and sends the launch
keystroke. A failure at any of these later steps returns `Err` with no
rollback — the worktree directory and branch are left on disk, unreferenced
by the task row (which only gets `worktree` written on full success).

Re-dispatching the same task then silently takes the REUSE path
(`reused_worktree = path.exists()`), which downgrades the fetch policy from
`Required` to `BestEffort` — contradicting `docs/specs/dispatch.allium`'s
claim that a failed dispatch leaves the task "dispatchable exactly as
before". If the title is edited before re-dispatch, the slug changes and the
old directory is orphaned permanently.

## Approach

Spec → tests → code, per repo convention.

1. **Spec**: `docs/specs/dispatch.allium` — done. Added a "Provisioning-failure
   rollback" subsection to `DispatchTask`'s guidance stating that any failure
   after a FRESH `git worktree add` rolls the worktree (and its branch) back
   before the error propagates, and that a REUSED worktree is never removed
   on failure.

2. **Tests** (`src/dispatch/worktree.rs`'s `gitignore_tests`-style module, or a
   new `mod tests` in that file — whichever fits the existing test
   organization for `provision_worktree`):
   - `provision_worktree_rolls_back_the_worktree_when_a_later_step_fails` —
     given verbatim in the task description. Fresh repo (`make_test_repo`),
     mock: `git worktree add` ok, `tmux new-window` fails. Asserts the result
     is `Err` and that a `git worktree remove` call was recorded.
   - `provision_worktree_does_not_remove_a_reused_worktree_on_failure` — the
     negative twin. `make_test_repo_with_worktree` (pre-existing directory),
     mock: `tmux new-window` fails (only one call needed — reuse path skips
     `git worktree add`). Asserts `Err` and that NO `git worktree remove` call
     was recorded.
   - `dispatch_agent_rolls_back_a_fresh_worktree_when_the_prompt_write_fails`
     (in `src/dispatch/tests.rs`, alongside the other `dispatch_agent_*`
     tests) — exercises the `dispatch_with_prompt` extension. Uses
     `make_test_repo` (no pre-created directory) so `git worktree add` is
     mocked as fresh-and-successful but the real directory never actually
     exists on disk (per KB #351) — the subsequent `.claude-prompt` write
     therefore fails naturally, without needing to fake anything. Asserts the
     dispatch errors and that a `git worktree remove` call was recorded.
   - Extend the two existing reuse-path failure tests
     (`dispatch_agent_propagates_tmux_new_window_failure`,
     `dispatch_agent_propagates_send_keys_failure`, both already on the reuse
     path via `make_test_repo_with_worktree`) with an assertion that no
     `git worktree remove` call was recorded — pins down that
     `dispatch_with_prompt`'s new rollback branch never fires for a reused
     worktree.

3. **Code**:
   - `src/dispatch/worktree.rs`: add `pub(super) fn rollback_fresh_worktree(repo_path, worktree_path, runner)`,
     a thin best-effort wrapper around the existing `remove_worktree_and_branch`
     (logs a warning on failure, never returns `Err`).
   - `provision_worktree`: wrap the three post-`git worktree add` tmux calls
     (`tmux::new_window`, `set_window_dispatch_dir`, `ensure_split_hook`) in a
     single fallible block. On `Err`, call `rollback_fresh_worktree` iff
     `!reused_worktree`, then return the original error unchanged.
   - `src/dispatch/agents.rs::dispatch_with_prompt`: wrap the `.claude-prompt`
     write and `tmux::send_keys` call the same way — on `Err`, call
     `rollback_fresh_worktree` iff `!provision.reused_worktree`, then return
     the original error.

## Out of scope

- Cleaning up the tmux window itself on a rollback (the task and spec focus
  on the worktree/branch leak; window lifecycle is a separate concern already
  handled by `teardown_task` elsewhere).
- Retroactively cleaning the one already-orphaned instance mentioned in the
  task description — that's a manual follow-up, not something this code
  change can reach.
