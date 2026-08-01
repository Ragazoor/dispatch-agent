# Worktree start point: local `<base>` vs `origin/<base>`

**Task**: #3810 — Worktrees start from `origin/<base>`, not local base branch
**Date**: 2026-08-01
**Absorbs**: #3804 (conditional rebase preamble) — design
`docs/superpowers/specs/2026-07-31-conditional-rebase-preamble-design.md`,
plan `docs/plans/3804-conditional-rebase-preamble.md`. That task was closed
after landing docs only; no implementation reached `src/`. It rewrites the same
function this task must rewrite, so the two are done as one change.

## The defect

`resolve_start_point` (`src/dispatch/worktree.rs:105`) creates every worktree
from `origin/<base>`, falling back to local `<base>` only when the fetch fails.

The rebase wrap-up path — `finish_task` (`src/dispatch/finish.rs`), reached from
the `/wrap-up` skill — rebases the task branch onto **local** `<base>`,
fast-forwards local `<base>`, and never pushes. Local `<base>` therefore
accumulates every finished dispatch while `origin/<base>` lags. Dispatch reads
the ref that does *not* have the landed work, so each new task starts behind by
the accumulated drift.

Four agents recorded this independently as knowledge-base entries #288, #326,
#149 and #233. It reproduced during this task's own session: local `main` was
`5 ahead, 0 behind` `origin/main`, and this worktree was missing all five
commits — including `b516aad9`, which removed the `dispatch doctor` CLI that
`CLAUDE.md` still documented in the stale context.

## Root cause, and why the fix lives in dispatch

Two coherent models exist:

- **Remote-first** — `origin/<base>` is truth; the root cause is that wrap-up
  never pushes. The fix would be to sync (merge + push) before dispatching.
- **Local-first** — local `<base>` is truth; the root cause is that dispatch
  reads the wrong ref.

`docs/specs/repo-sync.allium` already commits the repo to local-first: sync
closes divergence by *merging into* local base and never rewriting it
(`LocalBaseHistoryIsNeverRewritten`), precisely so worktrees branched off it stay
valid.

**Decision: dispatch never pushes and never mutates the primary checkout.**
Sync stays a deliberate, human-triggered action (board key / `dispatch repo
sync`). An implicit sync-before-dispatch was rejected on two grounds: it turns
"dispatch a task" into an outward-facing publish, and `sync_repo`
(`src/repo_sync.rs:194`) is deliberately strict — `NotOnBaseBranch`,
`DirtyPrimaryWorktree`, `MergeConflict` — so on a checkout that is mid-feature or
dirty it would decline and silently leave the very drift it was meant to remove.

Dispatch may therefore only *fetch*, and must choose correctly from what is on
disk.

## Change 1 — fetch failures are classified, not blanket-tolerated

Today a failed fetch is soft: after 3 blind retries, provisioning falls back to
local `<base>`, returns `Ok`, and the sole signal is a `Note:` line appended to a
prompt the agent may never act on. A dispatch with no network silently produces a
worktree off whatever local `<base>` happened to be.

The failure classes are not alike, so they stop being treated alike:

- **404-class** — there is no `origin/<base>` to prefer. Not an error: local
  `<base>` is the only ref that exists. Use it, keep the `Note:`.
- **infra-class** — origin exists and has the branch, but we could not reach it.
  **Abort the dispatch** with `Err`. No worktree, no tmux window, no agent.

Classification uses exit codes only — no git stderr pattern-matching, which the
comment at `worktree.rs:21-24` deliberately avoids. `git fetch` exits `128` for
missing-ref, unresolvable-host, and unreadable-remote alike (verified), so it
cannot classify. `git ls-remote --exit-code` can (verified):

| `git ls-remote --exit-code origin refs/heads/<base>` | meaning |
|---|---|
| `0` | ref exists on origin |
| `2` | ref absent from origin — **404-class** |
| `128` | origin unreachable — **infra-class** |

Combined with `git::has_origin_remote` (`src/git.rs:33`, exit-code only):

```
git fetch origin <base>
  ok      -> Fetched
  failed  -> classify ONCE:
        !has_origin_remote(repo)          -> NoOriginRef("no origin remote configured")
        ls-remote exit 2                  -> NoOriginRef("origin has no branch <base>")
        ls-remote exit 0 or 128, or spawn failure
                                          -> infra: retry the fetch up to
                                             FETCH_MAX_ATTEMPTS, then Err
```

Classification runs **once**, after the first failure — a 404 is not retried,
because retrying a branch that does not exist cannot succeed. Retries then mean
what `worktree.rs:21-24` claims: smoothing transient infra, and nothing else.

The `ls-remote` probe is itself a network call, so it uses `run_with_timeout`.

**Unchanged:** `pr_head_branch` (`src/dispatch/agents.rs:143`) keeps its
soft-fallback to the base branch. Its fallback is semantically meaningful — a
cross-repository PR's head genuinely is not on origin — and `gh` offers no clean
exit-code contract to classify on. Separate concern.

## Change 2 — start-point selection

With a successful fetch, both refs are current, so the choice is decidable:

```
ahead_behind(repo, base)          // <base>...origin/<base>, src/repo_sync.rs:125
  Some(ab) if ab.ahead > 0  ->  Local  { base }    // local carries commits origin lacks — FIXED
  Some(ab)                  ->  Remote { base }    // ahead == 0: today's behaviour, unchanged
  None                      ->  Remote { base }    // local <base> absent — origin is the only ref
```

Local is preferred **only** on a positive `ahead > 0` reading. This polarity is
load-bearing: `ahead_behind` returns `None` whenever local `<base>` does not
resolve, which is the normal case for a base branch the human never checked out
locally (`base_branch = develop`). Treating `None` as "prefer local" would hand
`git worktree add` a ref that does not exist.

Divergence (`ahead > 0 && behind > 0`) takes the local branch **silently**. The
board's drift indicator (#3783) is the human-facing signal; a second warning in
the agent's prompt would be noise the agent cannot act on.

`sync_repo` merges rather than rebases, so a local `<base>` that is ahead already
contains everything origin has once the human syncs. Choosing local can only ever
lose commits that the human has not yet merged — which the drift indicator is
there to surface.

**PR-review worktrees skip the measurement entirely** and always use
`origin/<headRefName>`. A review must see exactly the PR's code; a stale local
branch sharing that name and carrying extra commits would silently poison it.

### Types

```rust
/// Which ref a worktree branch was created from — and therefore what the agent
/// should rebase onto if it ever needs to.
pub(super) enum StartPoint {
    /// `origin/<base>`: origin is at least as new as local `<base>`.
    Remote { base: String },
    /// Bare local `<base>`: it carries commits `origin/<base>` does not, or
    /// origin has no such branch.
    Local { base: String },
}
```

`git_ref()` yields `origin/<base>` or `<base>`; `base()` yields the bare branch
name for the `git fetch origin <base>` line, which is identical in both arms.

`provision_worktree`'s `base_branch: Option<&str>` becomes
`Option<BaseRef<'_>>` with arms `Branch(&str)` (measured, may prefer local) and
`PrHead(&str)` (never measured). The `None` arm stays — six tests in
`src/dispatch/tests.rs` pass it, and it means "create the branch with no explicit
start point".

`ProvisionResult` gains `start_point: Option<StartPoint>` and — from #3804 —
`reused_worktree: bool`. `fetch_warning` keeps its name; only its doc comment
narrows, since it is now exclusively the 404-class message.

## Change 3 — conditional preamble, keyed on the resolved ref (absorbs #3804)

#3804's finding was that on a fresh dispatch the preamble is a guaranteed no-op:
the branch *is* `origin/<base>`, so `git rebase origin/<base>` can only report
"up to date". That claim generalises rather than breaks — the branch is the
**resolved start point**, so the rebase is still a no-op. #3804's decision table
survives intact; only the ref it names changes.

This matters because a preamble that ignored the resolution would actively undo
it. From a branch sitting at local `<base>`'s tip, `git rebase origin/<base>`
replays local `<base>`'s unpushed commits onto `origin/<base>` — no data loss,
but they return with **new SHAs**. The worktree branch then holds duplicates of
commits already in local `<base>`, and `finish_task`'s rebase onto local `<base>`
conflicts or double-applies. That is knowledge entry #288 described exactly.

Change 1 also **collapses the fetch dimension out of #3804's table**. #3804 had
four rows because a failed fetch left the branch possibly-stale, so it emitted a
preamble telling the agent to retry the fetch. That is no longer true of either
class: infra-class aborts the dispatch, and 404-class leaves the branch at
exactly local `<base>` — where `git rebase <base>` is a guaranteed no-op and the
fetch can never succeed on retry, because the branch does not exist on origin.
Only reuse survives as a discriminator:

| worktree | preamble |
|---|---|
| fresh | **none** — branch *is* the start point |
| reused | `reused_rebase_preamble(start_point)` |
| PR (any reuse state) | `pr_rebase_preamble(pr_branch)` |

The preamble builders take `&StartPoint` and render:

```
git fetch origin <base>          # always the bare branch name
git rebase <start_point.git_ref()>   # <base> or origin/<base>
```

`rebase_preamble` therefore loses its last caller and is **deleted**;
`reused_rebase_preamble` and `pr_rebase_preamble` are the only two builders left.

**The `Note:` is independent of the preamble.** #3804 appended the warning to the
preamble text, which forced a "never drop the warning" invariant coupling the
two. Here the warning is emitted whenever `fetch_warning` is `Some`, whether or
not a preamble accompanies it — so the fresh + 404-class case correctly yields a
`Note:` and no preamble. Decoupling them is what lets the table shrink.

**Invariant: a fetch warning is never dropped** — now structural rather than
table-wide, since the `Note:` no longer depends on which preamble row was taken.

**Invariant: the preamble target is always the resolved base branch**, never a
literal `main`. Already true via `dispatch_with_prompt`'s `resolved`
(`agents.rs:136`); preserved and re-tested.

### `select_preamble`

```rust
pub(super) fn select_preamble(
    pr_branch: Option<&str>,
    start_point: &StartPoint,
    reused: bool,
) -> String
```

Pure — no `ProcessRunner`, no filesystem — so every row is unit-testable
directly. It is the only place the rule lives. Returns `""` for the no-preamble
row. It takes no `fetch_warning`: the `Note:` is composed separately, which is
what keeps this function a three-row table rather than a six-row one.

### `dispatch_with_prompt` rewiring

`src/dispatch/agents.rs:152-176` builds the preamble *before* provisioning and
patches the warning on afterwards. It must resolve `effective_base`, provision,
then call `select_preamble` with the provisioning outcome.

The borrow detail from #3804's plan carries over unchanged: today
`match pr_branch { … }` *moves* `pr_branch`, but `select_preamble` needs it
afterwards. Match on `&pr_branch`, clone only in the `Some` arm, let the `None`
arm move `resolved`.

Prompt assembly must stop hardcoding `"{preamble}\n\n"` — an empty preamble would
otherwise leave two blank lines above "Always work from this worktree folder".

## Testing

Test-first throughout, per repo convention (spec → tests → code).

**Pure units** (`src/dispatch/prompts.rs`, inline `mod tests`) — one test per
preamble row (fresh → `""`, reused → reuse wording, PR → PR wording for both
reuse states), the never-literal-`main` invariant, and the ref-mirroring claim:
`reused_rebase_preamble` renders `git rebase develop` for
`Local { base: "develop" }` and `git rebase origin/develop` for `Remote`, with
`git fetch origin develop` in both.

**Note composition** (`dispatch_with_prompt` level) — the warning survives on a
row that emits no preamble (fresh + 404-class). This is the case the old
preamble-coupled invariant made unrepresentable, so it gets an explicit test.

**Classification** (`src/dispatch/worktree.rs` or `tests.rs`, `MockProcessRunner`)
— one test per class: no origin remote → local + warning, no retry;
`ls-remote` exit 2 → local + warning, no retry; `ls-remote` exit 128 → retried
then `Err`; `ls-remote` exit 0 → retried then `Err`; fetch succeeds on retry 2 →
`Remote`, no warning. Assert the **call sequence**, since "no retry on 404" is a
claim about how many fetches were issued.

**Selection** (`MockProcessRunner`) — `rev-list` yielding `3\t0` → `Local`;
`0\t2` → `Remote`; `3\t2` → `Local`; failure → `Remote`; `BaseRef::PrHead` →
`Remote` with **no `rev-list` call issued at all**.

**Real git** (`tests/tmux_lifecycle.rs`) — the mock cannot answer these, because
it never runs `git worktree add` for real. `seed_repo` (`:111-125`) already
builds a real repo with a real local `origin`:

1. `fresh_dispatch_with_synced_base_starts_from_origin` — unchanged fixture
   (local `main` == `origin/main`, so `ahead = 0`): assert
   `git rev-parse <branch>` == `git rev-parse origin/main`. This is #3804's
   premise test, and it still holds.
2. `fresh_dispatch_prefers_local_base_when_ahead_of_origin` — **the regression
   test for this whole task.** Add a commit to local `main` *without* pushing,
   dispatch, and assert `git rev-parse <branch>` == `git rev-parse main` and
   `!=` `git rev-parse origin/main`.

Both gate on `tmux_available_or_skip()`, which skips locally when tmux is missing
but hard-fails under `CI` (`docs/conventions.md`), so they cannot quietly stop
running. Comment both to record that they ask a *git* question and live in this
file only because it is the one place with a real repo, a real `origin`, and a
real dispatch — so a later reader does not "simplify" them onto a mock.

Do **not** assert prompt text there: the launch command deletes `.claude-prompt`
shortly after dispatch, reading it back races, and `tokio::time::sleep` is banned
in tests (`./scripts/check-no-test-sleep.sh`).

**Existing tests that must change** — these assert the behaviour being replaced,
so they are rewritten, not relaxed:

- `src/dispatch/tests.rs:1286` and `:2639` assert the fallback-to-local on fetch
  failure. Under Change 1 an infra-class failure aborts; they are re-pointed at
  the 404-class path.
- Every `provision_worktree` test asserting `origin/<base>` as start point
  (`:1099`, `:1120`, `:1143`, `:1223`, `:1262`, `:1343`, `:1376`) now needs its
  mock to answer the new `rev-list` probe. Their assertions stay valid — a mock
  yielding no ahead-commits still selects `Remote`.
- `rebase_preamble_prepended_to_all_prompts` (`:541-561`) is deleted per #3804:
  it hand-assembles the preamble instead of calling `dispatch_with_prompt`, so it
  would pass unchanged after this work and asserts nothing about dispatch.
- `rebase_preamble` is deleted outright (no callers survive the table collapse),
  so its two direct tests — `:1424` (`"99-prev-task"`) and `:1441`
  (`"develop"`) — are re-pointed at `reused_rebase_preamble`. #3804's plan said
  to keep them untouched; that no longer holds, because #3804 kept
  `rebase_preamble` alive for its fetch-failed rows and those rows are gone.
- Every existing `dispatch_agent` fixture pre-creates the worktree dir, so all of
  them land on the *reuse* row. They survive because every prompt assertion uses
  `.contains(...)`; the sole `.starts_with("Before starting work")` is at `:559`,
  inside the deleted test. Audited in #3804's plan; re-verify rather than assume.

**Zero snapshot churn expected** — `src/dispatch/snapshots/` covers
`build_*_prompt`, which is upstream of the preamble. Any `.snap.new` here signals
something unintended changed.

## Spec updates

`docs/specs/dispatch.allium:196-237` is rewritten via `allium:tend`, verified
with `allium:weed`:

- `:196-206` — the fetch no longer always yields `origin/<base>`. Record the
  classification and the selection rule.
- `:208-217` — the blanket soft-fallback becomes 404-class only; infra-class
  aborts the dispatch.
- `:219-230` — record that PR-based worktrees skip the measurement.
- `:233-237` and the `{rebase_preamble}` line in the prompt skeleton
  (`:250-252`) — the preamble becomes conditional and targets the resolved ref.

Pre-existing staleness in this block, **out of scope** (noted so `allium:weed`
is not confused): `dispatch.allium:214` lists `build_epic_planning_prompt`, which
exists nowhere in `src/`.

## Risks

- **A repo whose local base is stale and never synced regresses.** If the human
  never touches local `<base>`, `ahead = 0` and behaviour is unchanged — the risk
  only materialises for a local base that is *ahead* yet unwanted, which is the
  case this task exists to prefer. Accepted.
- **Aborting on infra-class failure blocks offline dispatch.** This is the
  intended change: an offline dispatch today produces a silently stale worktree.
  The error names the cause and the retry count.
- **One extra `git rev-list` per dispatch.** Local-only, no network.
- **Ordering regression.** Moving preamble construction after provisioning means
  a future edit could reintroduce a pre-provision preamble that ignores the
  resolution. The `select_preamble` table tests make the rule the single source
  of truth, so such an edit fails tests rather than regressing silently.

## Verification

```
cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh
```

Plus `cargo clippy --all-targets -- -D warnings` (the pre-push gate; a plain
`cargo build` will not catch it).
