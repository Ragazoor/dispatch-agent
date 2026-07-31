# Local-first repo sync — design

**Task**: #3783 · **Date**: 2026-07-31

## Problem

Dispatch's dominant integration path is `wrap_up(rebase)`, which never pushes.
`finish_task` (`src/dispatch/finish.rs:108`) pulls `origin/<base>` into the
primary checkout, rebases the task branch onto local base, then
`git merge --ff-only` — so local `main` gains commits on every finished task
and **nothing ever publishes them**. Local `main` drifts ahead of
`origin/main` without bound, and drifts *behind* whenever a PR merges on
GitHub or another machine pushes.

Nothing in dispatch measures, displays, or closes that gap. There is no
repo-level surface at all beyond `dispatch repo set-verify` / `repo list`.

The drift is not cosmetic. `resolve_start_point`
(`src/dispatch/worktree.rs:105`) bases every new worktree on `origin/<base>`,
so each dispatched task starts behind local `main` by exactly the drift —
the root cause behind knowledge-base entries #288, #326 and #149. That
specific fix is **out of scope** here (see Non-goals) but it is why closing
the gap matters.

## Goal

Keep local `main`/`master` in sync with `origin` — make drift visible where
it cannot be ignored, and give one explicit action that closes it.

## Decisions

Settled during brainstorming, recorded so the plan does not relitigate them:

1. **Explicit action, not auto-push.** `wrap_up` already bundles commit,
   pull, rebase, fast-forward, retro, `exit_session` and epic auto-dispatch
   chaining. Adding a network write to a shared branch would make the one
   already-irreversible step worse, with an unclear failure path (abort a
   wrap_up that has already fast-forwarded local main?). It would also
   publish unverified work — a clean rebase can still leave a broken tree
   (#233, #314) — help only the *ahead* direction, and push once per task
   instead of once per batch.
2. **Merge, then push, on divergence.** When local is both ahead and behind,
   sync merges `origin/<base>` into local and pushes. Never rewrites local
   `main` history, so live worktrees branched off it stay valid. Cost is a
   merge commit, which matches what `finish_task` already does today.
3. **Event-driven refresh, no timer.** Every git-network operation dispatch
   already performs becomes a refresh point (see Refresh triggers). No poll
   interval, no new timing constant.
4. **Status bar, not a popup.** The purpose is a nag; a popup you must open
   is a nag you will not see. A repos popup is deferred.
5. **`o` for sync** — the only unbound board-mode key with a clean mnemonic
   (**o**rigin) and no hazardous neighbour. `y`/`n` were rejected despite
   being free on the board: they are the yes/no keys in every confirm
   prompt, so `y` would both open and accept a push to shared main. `w` was
   rejected for sitting next to `W` (wrap up), the other irreversible git
   action.

## Architecture

Three units, each independently testable.

### 1. `src/repo_sync.rs` — the engine

New module. Synchronous, `ProcessRunner`-driven, no async and no TUI
coupling — structured and tested like `src/dispatch/finish.rs`.

```
AheadBehind { ahead: u32, behind: u32 }

ahead_behind(repo, base, runner) -> Option<AheadBehind>
fetch_base(repo, base, runner) -> Result<()>
sync_repo(repo, base, runner) -> Result<SyncOutcome, SyncError>
```

**`ahead_behind`** runs
`git -C <repo> rev-list --count --left-right <base>...origin/<base>`.
Output `"3\t1"` means 3 commits reachable only from local `<base>` (ahead)
and 1 only from `origin/<base>` (behind). Returns `None` — not
`AheadBehind { 0, 0 }` — when `origin/<base>` does not resolve (no remote,
never fetched) or the output does not parse, so the indicator hides instead
of claiming "in sync" about a repo it cannot measure.

**`sync_repo`** checks all preconditions before any write, each as its own
error variant, so a dirty tree can never masquerade as a conflict (the
discipline `FinishError` already follows):

| Precondition | Error |
|---|---|
| `git remote get-url origin` succeeds | `NoRemote` |
| repo root is on `<base>` | `NotOnBaseBranch { current, expected }` |
| repo root is clean | `DirtyPrimaryWorktree { path, files }` |
| anything else fails to run | `Other(String)` |

The branch check matters because both the merge and the push act on the
checked-out branch. The clean check matters because merging into a dirty
tree is how work is lost.

Then:

1. `git fetch origin <base>`
2. recount ahead/behind from the refreshed refs
3. if `behind > 0`: `git merge --no-edit origin/<base>`. This fast-forwards
   when `ahead == 0` and creates a merge commit when diverged — one command
   covers both. On conflict: read unmerged paths from the repo's own
   `git status --porcelain` **before** `git merge --abort` clears them,
   then abort and return `MergeConflict { files }`.
4. if `ahead > 0` (recounted after the merge): `git push origin <base>`.
   Rejection means origin moved between fetch and push — return
   `PushRejected { stderr }`, which is retryable.
5. return `SyncOutcome`, an enum: `Synced { pulled, pushed }`, or
   `AlreadyInSync` when both counts are zero after the fetch. The
   `AlreadyInSync` path performs no merge and no push — the fetch in step 1
   always runs, since it is what makes the counts trustworthy.

**Shared helper move.** `parse_porcelain_files` and `parse_unmerged_files`
are private in `finish.rs` (`src/dispatch/finish.rs:67`, `:79`).
`sync_repo` needs byte-identical porcelain and conflict parsing, so both
move to `src/git.rs`, which exists for exactly this kind of shared git
plumbing. Both callers then use one implementation — no second copy of
conflict detection.

**Base branch resolution.** The repo's base branch comes from
`git::detect_default_branch` (`src/git.rs:9`), which reads
`refs/remotes/origin/HEAD` and falls back to `"main"`. That is the repo's
actual default branch, so `master` repos work unchanged. Deliberately *not*
the `repo_base_branches` MRU table — that records per-task dispatch bases,
which include PR-review feature branches.

### 2. Measurement — event-driven, cached in memory only

`App` gains a cache keyed by repo path:

```
RepoSyncEntry { base_branch: String, counts: Option<AheadBehind>, last_fetch_error: Option<String> }
App.repo_sync: HashMap<String, RepoSyncEntry>
```

**Not persisted.** The CLI and `doctor` are separate processes and compute
on demand, so nothing needs to cross a process boundary. No DB migration,
no schema change.

Refresh triggers:

| Trigger | Network cost |
|---|---|
| TUI startup | one `git fetch` per `repo_paths` row — the only genuinely new network call |
| Task dispatched | none — `provision_worktree` already fetched `origin/<base>` |
| `wrap_up(rebase)` succeeds | none — `finish_task`'s pull already fetched, and local `<base>` provably just moved ahead |
| Sync completes | none — recount to clear the indicator |

`ahead_behind` is a local ref read, so triggers 2–4 add no network traffic at
all. New plumbing: `Command::Repo(RepoCommand::{RefreshSyncState, Sync { repo_path, base_branch }})`
and `Message::Repo(RepoMessage::{SyncStateRefreshed(..), SyncCompleted(..)})`.

Both commands run their git work inside `spawn_blocking` — conventions
forbid process and `std::fs` work on the async path, and neither may touch
the render path. Startup refresh is fire-and-forget: results arrive as a
Message, so a slow or offline network never delays TUI startup.

A failed fetch is non-fatal: keep the last known counts, store the error in
`last_fetch_error` for the CLI and doctor to report, and leave the status bar
undisturbed.

### 3. Surfaces

**Status bar** (`src/tui/ui/kanban/status_bar.rs`) — a `main ↑3↓1` segment
for the repo owning the currently selected task, rendered only when the
counts are known and `ahead > 0 || behind > 0`. No selection, unknown
counts, or a clean repo means no segment. Ahead-only is neutral-coloured;
any `behind` is a warning, since that is the case that will bite a rebase.

Note the honest consequence: `ahead > 0` is the normal state right after
every `wrap_up(rebase)`, so the segment will be lit most of the time. That
is the design working — it is a nag that clears when you press `o` — but
"hidden when clean" buys less quiet than it sounds.

**Keybinding** — `o` enters
`AppMode::ConfirmRepoSync { repo_path, base_branch, ahead, behind }`. On
confirm, `Command::Repo(RepoCommand::Sync { .. })`. The prompt states
exactly what will happen and names only the halves that apply: "Merge
origin/main into main (1 commit) and push 3 commits to origin/main?" when
diverged, the push alone when `behind == 0`, the merge alone when
`ahead == 0`. `SyncOutcome` goes to the status bar; `SyncError` goes to the
error popup (`src/tui/ui/kanban/popups/error.rs`). `O` is left unbound for a
future sync-all.

**CLI** — two new `RepoAction` variants (`src/main.rs:164`):

- `dispatch repo status [--no-fetch]` — a table over `repo_paths`: path,
  base branch, ahead/behind, last fetch error. Fetches first unless
  `--no-fetch`. This is the multi-repo view the deferred popup would have
  given.
- `dispatch repo sync [<path>]` — sync one repo, or every known repo when
  the path is omitted. Non-zero exit if any repo fails.

**Doctor** — a `repo-sync` check reporting drift per repo as a **warning,
not a failure**, using doctor's existing severity vocabulary. `ahead > 0` is
normal in this workflow and must not turn `dispatch doctor` permanently red.

## Testing

TDD throughout — behaviour first, then the code that satisfies it. Nothing
sleeps: every test is mock- or event-driven, per the no-test-sleep rule.

| What | Where |
|---|---|
| `sync_repo` state machine: each precondition failure; all four ahead/behind quadrants; merge-conflict abort-and-report; push rejected; `AlreadyInSync` issues no merge or push. Exact-argv assertions. | inline `mod tests` in `src/repo_sync.rs`, `MockProcessRunner` |
| `ahead_behind` parsing: `"3\t1"`, `"0\t0"`, missing `origin/<base>` → `None`, malformed output → `None`, exact argv | inline in `src/repo_sync.rs` |
| Porcelain helpers still behave identically after the move | inline in `src/git.rs`; existing `finish.rs` tests must stay green unchanged |
| Each refresh trigger emits `RefreshSyncState`; a failed fetch preserves prior counts | `src/tui/tests/` |
| Status-bar segment present when drifted, absent when clean / unknown / unselected | `src/tui/tests/snapshots` (120×40 — do not change the backend size) |
| `o` → confirm → sync flow, and the prompt naming only the applicable halves | `src/tui/tests/scenarios` |
| `repo status` and `repo sync` argument handling and exit codes | `tests/cli.rs` |
| `repo-sync` check output and severity | inline in `src/cli/doctor.rs` |

## Spec changes

- **New `docs/specs/repo-sync.allium`** — owns `AheadBehind`, the
  `sync_repo` contract with its typed failures, the refresh triggers, and
  all four surfaces.
- **`docs/specs/doctor.allium`** — the new `repo-sync` check.
- **`CLAUDE.md`** — mention `src/repo_sync.rs` under subsystem entry points.

No `core.allium` config entry: the timer is gone, so there is nothing to
configure.

## Non-goals

Explicitly excluded, each either decided against or deferred to its own task:

- **Auto-push from `wrap_up`** — decided against (Decision 1).
- **Rewriting local `main`** (rebase-onto-origin) — decided against
  (Decision 2).
- **`resolve_start_point` preferring local `<base>`** — deferred to a
  follow-up task. Real, and the concrete cost of the drift, but it changes
  dispatch semantics for every task in every repo and deserves its own
  design pass.
- **Post-rebase verify gate** (#233, #314) — separate concern.
- **Per-card worktree staleness badges** — separate concern.
- **Repos popup** — deferred until the engine is proven.
- **Any DB schema change** — the cache is in-memory; the CLI computes on
  demand.
