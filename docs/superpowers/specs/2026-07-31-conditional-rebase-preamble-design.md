# Conditional rebase preamble

**Task**: #3804 — Do we need git rebase at start
**Date**: 2026-07-31

## Question

Every dispatched agent's prompt opens with a fetch-then-rebase preamble:

```
Before starting work, fetch and rebase your branch onto the latest main:
git fetch origin main
git rebase origin/main
```

Does the agent need this, given the TUI provisions the worktree itself?

## Finding: on a fresh dispatch it is a guaranteed no-op

`provision_worktree` (`src/dispatch/worktree.rs:157-188`) runs
`git fetch origin <base>` — retried up to `FETCH_MAX_ATTEMPTS` (3) times — and
then creates the branch with
`git worktree add <path> -B <branch> origin/<base>`.

The branch therefore *is* `origin/<base>` at the moment the agent starts. The
preamble's rebase can only report "up to date". Confirmed empirically: the agent
executing this task ran the preamble and got
`Current branch 3804-do-we-need-git-rebase-at-start is up to date.`

Three cases are the exception, all knowable server-side at provision time:

1. **Reused worktree directory** — `worktree.rs:165` skips `git worktree add`
   entirely when the path already exists (redispatch / retry after a crash), so
   the branch keeps whatever state the previous attempt left it in. The fetch
   still runs, so a rebase here does real work.
2. **Fetch failed** — `resolve_start_point` (`worktree.rs:105-126`) falls back to
   the bare local `<base>` and returns a warning, which
   `dispatch_with_prompt` already appends to the prompt as a `Note:` line.
3. PR-based review worktrees, which are a separate preamble
   (`pr_rebase_preamble`) and a separate concern — see Non-goals.

## Non-goals

### The origin-vs-local base-ref staleness

`wrap_up(rebase)` (`src/dispatch/finish.rs:152-215`) pulls `origin/<base>` into
the repo root, rebases the task branch onto **local** `<base>`, fast-forwards
local `<base>` with `merge --ff-only` — and never pushes. Local `main` therefore
accumulates every finished dispatch's work while `origin/main` lags behind. Both
the provisioning start point and the preamble target `origin/<base>`, the ref
that does *not* have that landed work. Four separate agents have recorded this as
a learning (#288, #326, #149, #233).

This is a real defect and it is **explicitly out of scope** for this task, by
decision: the scope here is the preamble only. Fixing the start point is a
follow-up.

### The PR-review preamble

`pr_rebase_preamble` (`src/dispatch/prompts.rs:68-78`) is framed as an on-demand
refresh — "do this whenever you want to refresh the PR's code" — rather than a
start-of-task step, because commits can land on the PR after dispatch. It is not
redundant and is left unconditional.

## Invariant carried forward

The preamble target is **always the resolved base branch, never a literal
`main`**. This already holds: `dispatch_with_prompt`
(`src/dispatch/agents.rs:134-136`) resolves `task.base_branch` first and falls
back to `detect_default_branch(repo)` only when the task has none, then passes
that to `rebase_preamble(&resolved)`. `main` appears only when it *is* the
resolved base branch. `src/dispatch/tests.rs:1417` locks the epic-chaining case
(`"99-prev-task"`) and `:1434` locks `"develop"`.

The design must preserve this, and adds a regression test asserting a non-`main`
base branch survives preamble selection.

## Design

### Behaviour rule

Non-PR dispatch, evaluated *after* provisioning:

| worktree | fetch | preamble |
|---|---|---|
| fresh | ok | **none** |
| fresh | failed | `rebase_preamble(base)` + `Note: <warning>` |
| reused | ok | `reused_rebase_preamble(base)` |
| reused | failed | `reused_rebase_preamble(base)` + `Note: <warning>` |

PR-based review worktrees emit `pr_rebase_preamble(pr_branch)` unconditionally.
Reuse and fetch outcome do not change *which* wording they get — but a fetch
warning still appends its `Note:` line, as it does today, so the invariant below
holds for PR rows too.

Reuse wins over fetch-failure for the *wording* — it is the superset instruction.
The `Note:` is orthogonal and appends in every row where a warning exists.

**Invariant: a fetch warning is never dropped.** Every row where
`fetch_warning` is `Some` emits a preamble that carries it. The
no-preamble row is reachable only when `fetch_warning` is `None`.

### Reuse wording

A reused worktree is reused precisely because a previous attempt left something
there, so it may hold uncommitted edits or commits from that run. A bare
`git rebase origin/<base>` on a dirty tree fails with `cannot rebase: You have
unstaged changes` — handing the agent a confusing error as its very first
action. The reuse preamble therefore names the situation:

```
This worktree was reused from a previous attempt and may contain uncommitted
changes or commits from that run. Check `git status` and `git log` first, then
bring the branch up to date:

git fetch origin <base>
git rebase origin/<base>

If the rebase reports unstaged changes, commit or stash them first.
```

### Components

**`ProvisionResult.reused_worktree: bool`** — `src/dispatch/worktree.rs:67-74`.
Set at the existing `if Path::new(&worktree_path).exists()` branch (`:165`). The
fact is already known there; it is simply not reported today. Sits alongside the
existing `fetch_warning` field, which serves the same purpose for the other
condition.

**`select_preamble(pr_branch: Option<&str>, base: &str, reused: bool,
fetch_warning: Option<&str>) -> String`** — new pure function in
`src/dispatch/prompts.rs`. This is the decision table and nothing else. Returns
`""` for the no-preamble row. Takes no `ProcessRunner`, touches no filesystem, so
every row is unit-testable directly.

**`reused_rebase_preamble(base: &str) -> String`** — new, in `prompts.rs`,
alongside the existing two preamble builders.

**`dispatch_with_prompt` rewiring** — `src/dispatch/agents.rs:150-174`. Today it
builds the preamble *before* `provision_worktree` and patches the warning on
afterwards. It must instead resolve `effective_base`, provision, then call
`select_preamble` with the provisioning outcome. Prompt assembly must also stop
hardcoding `"{preamble}\n\n"`: an empty preamble would otherwise leave two blank
lines above "Always work from this worktree folder".

`rebase_preamble` and `pr_rebase_preamble` keep their current text and
signatures.

## Testing

Tests are written before the implementation, per TDD.

**`select_preamble` (unit, `prompts.rs`)** — one test per table row:

- fresh + fetch ok → empty string
- fresh + fetch failed → contains `git rebase origin/<base>` and the warning
- reused + fetch ok → contains the reuse wording, not the plain wording
- reused + fetch failed → reuse wording *and* the warning
- PR branch set → `pr_rebase_preamble` text regardless of `reused`; with a
  warning present, that text plus the `Note:` line
- warning-never-dropped invariant: for every combination where `fetch_warning`
  is `Some`, the output contains the warning text
- `base = "develop"` → output names `develop` and contains no literal `main`

**`provision_worktree` (`worktree.rs`)** — `reused_worktree` is `true` when the
worktree dir pre-exists and `false` when `git worktree add` runs. Both are
drivable with the existing `make_test_repo()` and
`make_test_repo_with_worktree()` helpers, because provisioning returns before any
prompt is written.

**`dispatch_agent` (integration, `tests.rs`)** — the reuse path only: read
`.claude-prompt` from the worktree and assert the reuse wording is present.

Note the mock's limit, which is why the decision table is a pure function: every
existing `dispatch_agent` test uses `make_test_repo_with_worktree` because
`fs::write` of `.claude-prompt` needs the directory to exist — but "directory
pre-exists" *is* the reuse trigger, and `MockProcessRunner` does not create
directories for `git worktree add`. The fresh path is therefore not drivable
end-to-end through `dispatch_agent` with a mock.

**Deletion**: `rebase_preamble_prepended_to_all_prompts`
(`src/dispatch/tests.rs:541-561`) is removed. It hand-assembles the preamble and
body itself instead of calling `dispatch_with_prompt`, so it asserts nothing
about real dispatch behaviour — it would still pass unchanged after this work,
which makes it actively misleading. The `select_preamble` tests replace it with
real coverage.

`tests.rs:1417` and `:1434` stay as-is; `rebase_preamble` is unchanged.

**Zero snapshot churn.** `src/dispatch/snapshots/` contains no preamble text —
the preamble is added in `dispatch_with_prompt`, downstream of the
`build_*_prompt` functions the snapshots cover.

## Spec updates

`docs/specs/dispatch.allium`:

- `:200-204` asserts the preamble is prepended unconditionally — becomes
  conditional, with the behaviour table as the rule.
- `:217-219` lists `{rebase_preamble}` in the prompt skeleton as always present —
  becomes optional.
- `:163-184` (fetch, retry, fallback, warning threading) stays accurate; the
  reuse case at `:172-173` gains a note that reuse now also selects a distinct
  preamble.

Applied with `allium:tend`, verified with `allium:weed`.

## Verification

```
cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh
```
