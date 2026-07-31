# Plan: conditional rebase preamble (task #3804)

Design: `docs/superpowers/specs/2026-07-31-conditional-rebase-preamble-design.md`

Goal: stop emitting the start-of-task fetch-then-rebase preamble when
provisioning has already guaranteed the branch is at `origin/<base>`. Emit it
only when the worktree was reused or the fetch failed. Scope is the preamble
only — the origin-vs-local base-ref defect is a deliberate non-goal.

Each step is test-first: write the test, watch it fail for the right reason, then
implement.

---

## Step 1 — Spec first

Per repo convention (spec → tests → code), update the Allium spec before any
code.

`docs/specs/dispatch.allium`:

- `:200-204` — replace the unconditional "The agent prompt is prepended with a
  fetch-then-rebase preamble" claim with the conditional rule and its decision
  table (fresh+ok → none; fresh+fetch-failed → plain preamble + Note;
  reused → reuse-aware preamble; reused+fetch-failed → reuse preamble + Note).
- `:217-219` — mark `{rebase_preamble}` in the prompt skeleton as optional,
  noting it is absent on the common fresh-dispatch path.
- `:172-173` — extend the existing "if the worktree directory already exists,
  only the `git worktree add` step is skipped" note to record that reuse now also
  selects the reuse-aware preamble.
- State the preserved invariant explicitly: the preamble target is always the
  resolved base branch (`task.base_branch`, else the detected default), never a
  literal `main`.
- State the never-drop invariant: any row carrying a fetch warning emits a
  preamble containing it.

Pre-existing staleness in the block being edited, **out of scope** — note it, do
not fix it, so `allium:weed` in Step 7 is not confused about what changed:
`dispatch.allium:214` lists `build_epic_planning_prompt` as one of the prompt
builders, but no such function exists anywhere in `src/` (only that spec line
mentions it). Likewise `src/dispatch/worktree.rs:129` still claims
`provision_worktree` is "shared by both `dispatch_agent` and `brainstorm_agent`",
and there is no `brainstorm_agent`. Both are separate cleanups.

Use the `allium:tend` skill. Then `./scripts/check-doc-paths.sh`, since the spec
cites `src/…` paths and `file:NN` line numbers.

**Checkpoint**: `./scripts/check-doc-paths.sh` passes.

---

## Step 2 — `reused_rebase_preamble` (test → code)

**Test** (`src/dispatch/prompts.rs`, inline `mod tests`):

- `reused_rebase_preamble_names_reuse_and_base_branch` — for base `develop`, the
  output mentions reuse of a previous attempt, tells the agent to check
  `git status`/`git log` first, contains `git fetch origin develop` and
  `git rebase origin/develop`, warns about unstaged changes, and contains no
  literal `main`.

**Code**: add `reused_rebase_preamble(base: &str) -> String` next to
`rebase_preamble` (`prompts.rs:52`) and `pr_rebase_preamble` (`:68`), matching
their `pub(super)` visibility and doc-comment style. Text per the design doc.

**Checkpoint**: `cargo test dispatch::prompts`.

---

## Step 3 — `select_preamble` decision table (test → code)

This is the heart of the change and the only place the rule lives.

**Tests** (`src/dispatch/prompts.rs`, inline `mod tests`) — one per table row plus
the two invariants:

| test | assertion |
|---|---|
| `select_preamble_empty_for_fresh_worktree_with_successful_fetch` | returns `""` |
| `select_preamble_plain_rebase_when_fetch_failed_on_fresh_worktree` | contains `git rebase origin/main`, the warning text, and no reuse wording |
| `select_preamble_reuse_wording_for_reused_worktree` | contains reuse wording; does *not* contain the plain preamble's opening line |
| `select_preamble_reuse_wording_and_note_when_reused_and_fetch_failed` | contains both reuse wording and warning |
| `select_preamble_pr_branch_ignores_reuse` | PR text for both `reused = true` and `false` |
| `select_preamble_pr_branch_still_carries_fetch_warning` | PR text plus the warning |
| `select_preamble_never_drops_fetch_warning` | loops all `(pr_branch, reused)` combinations with `fetch_warning = Some(...)`; every result contains the warning |
| `select_preamble_targets_given_base_branch_not_main` | base `develop`, non-empty rows name `develop` and contain no literal `main` — locks the invariant from the design |

**Code**: add to `prompts.rs`:

```rust
pub(super) fn select_preamble(
    pr_branch: Option<&str>,
    base: &str,
    reused: bool,
    fetch_warning: Option<&str>,
) -> String
```

Body: pick the wording (PR → `pr_rebase_preamble`; else reused →
`reused_rebase_preamble`; else warning present → `rebase_preamble`; else `""`),
then append `\n\nNote: {warning}` when a warning exists and the wording is
non-empty. The `""`-with-warning combination is unreachable by construction —
the warning branch produces a preamble — and the never-drop test proves it.

**Checkpoint**: `cargo test dispatch::prompts`.

---

## Step 4 — `ProvisionResult.reused_worktree` (test → code)

**Tests** (`src/dispatch/tests.rs`, ProcessRunner section):

- `provision_worktree_reports_reuse_for_existing_dir` — `make_test_repo_with_worktree("42-fix-bug")`, mock `git fetch` + the three
  tmux calls; assert `reused_worktree == true`.
- `provision_worktree_reports_fresh_when_worktree_created` —
  `make_test_repo()`, mock `git fetch` + `git worktree add` + the tmux calls;
  assert `reused_worktree == false`.

Both are drivable because `provision_worktree` returns before any prompt is
written — the mock never needs to create a directory. Match the existing mock
call-sequence comment style at `tests.rs:727-737`.

**Code**: add `pub(super) reused_worktree: bool` to `ProvisionResult`
(`worktree.rs:67-74`) with a doc comment explaining why the caller cares (it
selects the reuse-aware preamble). Set it from the `if
Path::new(&worktree_path).exists()` branch at `:165` — `true` in the reuse arm,
`false` in the `worktree add` arm.

**Checkpoint**: `cargo test dispatch::` — compilation forces every
`ProvisionResult` construction site to be updated.

---

## Step 5 — Wire `dispatch_with_prompt` (test → code)

**Test** (`src/dispatch/tests.rs`):

- `dispatch_reused_worktree_prompt_carries_reuse_preamble` — dispatch through
  `dispatch_agent` on a pre-created worktree, read
  `<worktree>/.claude-prompt`, assert it contains the reuse wording and
  `Always work from this worktree folder`.

Also assert the empty-preamble formatting has no leading blank lines. Since the
fresh path is not reachable through `dispatch_agent` with a mock (see the design
doc's testing note), cover that by asserting `select_preamble`'s empty output
composes correctly — either via a small extracted `compose_prompt` helper or by
asserting the reuse-path prompt starts with the preamble while a
`select_preamble`-empty composition starts with `Always work from`.

**Code** (`src/dispatch/agents.rs:150-174`), reorder to:

1. Resolve `effective_base` from `pr_branch` / `resolved` — no preamble built
   here.
2. Call `provision_worktree(task, runner, Some(&effective_base), SUBPROCESS_TIMEOUT)`.
3. `let preamble = select_preamble(pr_branch.as_deref(), &effective_base,
   provision.reused_worktree, provision.fetch_warning.as_deref());`
4. Delete the now-redundant `match &provision.fetch_warning` block at `:163-166`
   — `select_preamble` owns warning appending.
5. Change prompt assembly so an empty preamble contributes nothing: build the
   head as `""` when the preamble is empty, else `format!("{preamble}\n\n")`,
   then `format!("{head}Always work from this worktree folder — …")`.

**The borrow fix, concretely.** Today `match pr_branch { Some(branch) => … }`
*moves* `pr_branch`, but `select_preamble` needs it after provisioning. Match on
`&pr_branch` and clone only in the `Some` arm, letting the `None` arm move
`resolved` (unused thereafter):

```rust
let effective_base: String = match &pr_branch {
    Some(branch) => branch.clone(),
    None => resolved,
};
let provision =
    provision_worktree(task, runner, Some(&effective_base), SUBPROCESS_TIMEOUT)?;
let preamble = select_preamble(
    pr_branch.as_deref(),
    &effective_base,
    provision.reused_worktree,
    provision.fetch_warning.as_deref(),
);
```

This compiles because `pr_branch` is only ever borrowed. Do not "clone the whole
`Option`" — that changes ownership semantics for no benefit.

**Checkpoint**: `cargo test dispatch::`.

---

## Step 5a — Real-git coverage of the fresh row (test only)

The fresh row fires on every normal dispatch and rests on a factual claim about
git, so it must not be covered by unit tests alone.

**Test** (`tests/tmux_lifecycle.rs`) —
`fresh_dispatch_leaves_branch_at_origin_base`:

- Use the existing fixture: `seed_repo` (`tmux_lifecycle.rs:111-125`) already
  builds a real repo with a real local `origin` and pushes `main`.
- `Fixture::dispatch(<unused id>)` (`:184-194`) for a task id whose worktree does
  not exist, so real `git worktree add` runs and really creates the directory.
- Assert `git rev-parse <branch>` == `git rev-parse origin/main` in the repo.

This asserts the **premise** — the fact that makes the preamble a no-op — so if
provisioning ever stops leaving the branch at `origin/<base>`, the no-preamble
row becomes wrong and this test fails. Reuse the file's existing `git()` helper
(`:132-150`), which sanitises the git environment.

**Do not** assert the absence of preamble text in `.claude-prompt` here. The
launch command is `bash -c 'prompt=$(cat .claude-prompt) && rm -f .claude-prompt
&& claude …'` (`src/dispatch/agents.rs:183`); under real tmux that shell runs and
deletes the file, so reading it back races, and `tokio::time::sleep` is banned in
tests (`./scripts/check-no-test-sleep.sh`). Prompt text for the fresh row is
covered by Step 3.

**Checkpoint**: `cargo test --test tmux_lifecycle` (needs a running tmux server).

---

## Step 6 — Remove the misleading test

Delete `rebase_preamble_prepended_to_all_prompts`
(`src/dispatch/tests.rs:541-561`). It hand-assembles the preamble and body rather
than calling `dispatch_with_prompt`, so it would still pass unchanged after this
work while asserting nothing about dispatch. Step 3 and Step 5 replace it.

Drop the now-unused `rebase_preamble` import from the `tests.rs:5` use-list if
nothing else in the file references it (`tests.rs:1417` and `:1434` do, so it
likely stays — check rather than assume).

Keep `tests.rs:1417` (`"99-prev-task"`) and `:1434` (`"develop"`) —
`rebase_preamble` itself is unchanged.

**No other existing test should break — audited, and this is the expected
result.** Every fixture pre-creates the worktree dir, so every existing
`dispatch_agent` / `research_agent` / `quick_dispatch_agent` test now lands on the
*reuse* row and receives the reuse wording. They survive because every
prompt-content assertion uses `.contains(...)`, never `.starts_with(...)` — the
sole `.starts_with("Before starting work")` is at `tests.rs:559`, inside the test
this step deletes. Specifically:

- `tests.rs:1015-1048` (reuse + fetch failed) asserts `contains("origin/main")`
  and `contains("Note:")` — both hold for `reused_rebase_preamble` + `Note:`.
- `tests.rs:1050-1076` (reuse + fetch ok) asserts only `!contains("Note:")`.
- `tests.rs:984-1012` (PR review) asserts `contains("git rebase origin/feature-x")`
  and `!contains("git rebase main")` — holds, since PR rows ignore `reused`.
- `src/mcp/handlers/tests/tasks/dispatch.rs:1966` (dependabot, pre-created dir)
  asserts `contains("Your task is:")` — unaffected.
- Mock call-order/count assertions (`tests.rs:722-755`, `:2710-2738`) are
  unaffected: `select_preamble` is pure and issues no subprocess calls.

If any of these *does* fail, treat it as a signal the decision table was
mis-implemented, not as a test to relax.

**Checkpoint**: `cargo test`.

---

## Step 7 — Verify and align

1. `cargo fmt` — note that scoped `cargo fmt -- <files>` can still touch
   unrelated files; diff-check afterwards.
2. `cargo clippy --all-targets -- -D warnings` — the pre-push gate; a plain
   `cargo build` will not catch it.
3. `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh` — the task's
   verify command.
4. `allium:weed` on `docs/specs/dispatch.allium` to confirm spec and code agree.
5. Confirm no `src/dispatch/snapshots/*.snap.new` or
   `src/tui/tests/snapshots/*.snap.new` were left behind. No snapshot churn is
   expected — the snapshots cover `build_*_prompt`, which is upstream of the
   preamble — so any `.snap.new` here is a signal something unintended changed.

---

## Risks

- **A reused worktree is not guaranteed to be behind.** The reuse preamble tells
  the agent to rebase, which on a dirty tree fails. The wording handles this by
  telling the agent to inspect first and commit or stash — it does not try to
  resolve it automatically.
- **Losing the preamble loses a nudge.** The preamble also implicitly told agents
  the branch tracks a base branch. Accepted: the fresh-dispatch guarantee makes
  it noise, and the reuse and fetch-failure rows keep it exactly where it carries
  information.
- **Ordering regression.** Moving preamble construction after provisioning means a
  future edit could reintroduce a pre-provision preamble that ignores the
  provisioning outcome. The `select_preamble` table tests make the rule the single
  source of truth, so such an edit fails tests rather than silently regressing.
