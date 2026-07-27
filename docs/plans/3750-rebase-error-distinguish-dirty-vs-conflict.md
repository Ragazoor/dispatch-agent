# 3750: distinguish dirty-tree from real rebase conflicts in wrap_up's rebase action

## Problem

`finish_task` (`src/dispatch/finish.rs`) only has two failure buckets:
`FinishError::RebaseConflict(branch)` (generic "Rebase conflict on {branch} —
resolve and try again") and `FinishError::Other(String)`. When the primary
worktree (the repo root that owns `task.base_branch`) has uncommitted
changes, that ends up looking indistinguishable from a genuine textual
rebase conflict once it surfaces — the agent has to manually inspect the
primary worktree to figure out which situation applies.

## Fix

1. Add a preflight check, run right after verifying the repo root is on
   `base_branch` and before any pull/rebase/merge is attempted: `git -C
   <repo_path> status --porcelain`. Non-empty output → new
   `FinishError::DirtyPrimaryWorktree { path, files }`, naming the repo path
   and the dirty file(s) up front, with no rebase attempted at all.
2. Enrich `FinishError::RebaseConflict` from a bare `String` (branch) to
   `{ branch: String, files: Vec<String> }`, parsing `CONFLICT (...): ...
   in <path>` lines out of the failed rebase's combined stdout+stderr. The
   Display message names the conflicted file(s) when present, falling back
   to the original generic wording when git's output doesn't yield a
   parseable path.

Neither change alters `finish_task`'s success path, the `is_conflict`
detection heuristic, or the `Conflict` sub_status semantics — a dirty
primary worktree is a distinct, non-conflict error and must not flip
`sub_status` to `Conflict` (there is nothing to resolve via conflict
markers, just a tree to clean up).

## Steps (TDD: test, then implementation)

1. Update `docs/specs/pr-workflow.allium` (done) — `WrapUpRebase` guidance
   documents the new preflight step and the enriched conflict message;
   `FinishTaskConflict` guidance clarifies it only fires for genuine
   conflicts, not a dirty primary worktree.
2. `src/dispatch/finish.rs`:
   - Add failing tests: a new `DirtyPrimaryWorktree` case (porcelain output
     non-empty → error before any rebase call happens), and update the
     existing conflict test to assert the parsed file name appears.
   - Change the enum, `Display` impl, and the git-command sequence to match
     (insert the `status --porcelain` step; parse conflicted files from the
     rebase failure text via a small helper, e.g. `parse_conflicted_files`).
   - Update all existing `MockProcessRunner` sequences in this file's
     inline test module to include the new `status --porcelain` call
     (clean, i.e. empty stdout) so previously-passing tests keep passing.
3. `src/dispatch/tests.rs` — same mechanical update to its (partially
   duplicate) `finish_task` test suite, plus a new dirty-worktree test and
   an updated conflict-file-name assertion.
4. `src/mcp/handlers/tasks/wrap_up.rs` and `src/runtime/tasks.rs` — update
   the two `matches!(e, FinishError::RebaseConflict(_))` call sites to the
   new struct-variant pattern (`RebaseConflict { .. }`); no behavioural
   change needed since `DirtyPrimaryWorktree` already falls through to "not
   a conflict" by not matching that arm.
5. `src/mcp/handlers/tests/tasks/wrap_up.rs` and `src/runtime/tests.rs` —
   same mechanical mock-sequence update, plus one new MCP-level test
   exercising `wrap_up(action="rebase")` against a dirty primary worktree,
   asserting the error names the primary worktree (not "Rebase conflict").
6. Run `cargo test` and `./scripts/check-doc-paths.sh`; fix fallout.

## Out of scope

- Not touching the task's own worktree dirty-check — wrap_up already
  expects the agent to have committed their own work; this task is
  specifically about the *primary* worktree (repo root).
- Not changing `is_conflict`'s substring-matching heuristic itself, only
  enriching what happens once it fires.
