# Bounding the subprocesses on the finish path

Task #3757 — "wrap-up call time"

## Why the task premise does not hold

The task asks why `wrap_up` takes seconds and proposes optimising that path. Measured
against the real trajectory logs (`~/.local/share/dispatch/trajectories/`, which record
`duration_ms` per MCP call), 677 `wrap_up` calls split cleanly by action:

| action | n | mean | p50 | p90 | p99 | max |
|---|---|---|---|---|---|---|
| `done` | 37 | 5 ms | 3 | 10 | 14 | 18 |
| `pr` | 102 | 6 ms | 6 | 12 | 17 | 33 |
| `rebase` | 538 | 1923 ms | 1698 | 2246 | 7026 | 28245 |

For scale, every other MCP tool is single-digit milliseconds (`exit_session` 3.2,
`get_task` 4.4, `update_task` 3.6).

So "seconds" belongs exclusively to `action="rebase"`, and it is essentially one network
round-trip: `git pull --no-rebase origin <base>` in `finish_task`. `git ls-remote origin
main` in this repo measures 1.82 s, against a 1.70 s p50. The four local git steps in the
same function are ~11 ms each.

The tail is legitimate work, not a stall. The slowest recorded calls are a rebase conflict
(28.2 s: detect, read porcelain status, `rebase --abort`), three large successful rebases
(21.7 / 18.8 / 10.5 s), and one pull that genuinely transferred. All 538 completed.

Two options were considered and rejected:

- **Warm `origin/<base>` in the background** so the pull becomes a local
  `merge --ff-only origin/<base>`. Reuses the existing `measure_repo(fetch_first = true)`
  path; only a periodic trigger is missing. Rejected: it reverses an explicit spec
  position (`docs/specs/repo-sync.allium` states measurement is event-driven with no poll
  interval and no new timing constant), adds permanent background network traffic, and —
  decisively — makes staleness silent and unbounded. A warming fetch that has been failing
  (offline, VPN, expired credentials) is deliberately non-fatal and keeps the previous
  state, so the rebase would silently target an arbitrarily old base with no signal to the
  agent. Given that sibling epic work landing on the base branch between `wrap_up` and
  `exit_session` is already a known hazard, trading freshness away on the one operation
  whose correctness depends on it is the wrong direction. The gain is 1.65 s, once per
  task, immediately before the agent runs its verify command (tens of seconds to minutes).
- **Shorten the timeout enough to clip the tail.** Rejected: the slowest genuine call is
  28 s of real work, so any bound tight enough to matter would start failing wrap-ups that
  were about to succeed.

**Conclusion: `wrap_up` is not slow for a fixable reason.** What the investigation did find
on that path is one real latent bug, which is what this design addresses.

## The defect

`finish_task` (`src/dispatch/finish.rs`) issues 5–8 git subprocesses through
`runner.run(...)`, which has no timeout. One is a network call; three take a git index
lock. Any of them can block indefinitely — an origin that completes the TCP handshake and
then stalls, or a stale `index.lock` held by another git process in the same repository
(routine, since the human often works in that checkout while an agent wraps up).

When that happens `wrap_up` never returns, the agent's tool call hangs forever, and no exit
token is ever minted — so the session cannot be closed at all.

The convention it departs from is already established: `run_with_timeout(SUBPROCESS_TIMEOUT)`
bounds the fetch, rev-list and push in `src/repo_sync.rs`, and the fetch and worktree-add in
`src/dispatch/worktree.rs`. `finish_task` does not bound a single one of its calls.

The same gap exists in the three preflight helpers in `src/git.rs` —
`has_origin_remote`, `current_branch`, `dirty_files` — which are shared by `finish_task`
and `repo_sync::sync_repo`. And `sync_repo`'s merge block mirrors `finish_task`'s rebase
block site for site: its `git merge --no-edit origin/<base>`, its conflict-path
`git status --porcelain`, and its `git merge --abort` are all unbounded too, even though the
fetch, rev-list and push around them are bounded.

That matters beyond consistency: `docs/specs/repo-sync.allium` already asserts that *every*
subprocess the sync engine issues is bounded by a timeout. **That claim is currently
false** — six of the engine's calls are not.

## The change

Replace `run` with `run_with_timeout(..., SUBPROCESS_TIMEOUT)` at every site below.

| file | call | why it can block |
|---|---|---|
| `src/dispatch/finish.rs` | `git pull --no-rebase origin <base>` | network |
| `src/dispatch/finish.rs` | `git rebase <base>` | worktree index lock |
| `src/dispatch/finish.rs` | `git status --porcelain` (conflict read) | lock, mid-rebase |
| `src/dispatch/finish.rs` | `git rebase --abort` | lock |
| `src/dispatch/finish.rs` | `git merge --ff-only <branch>` | repo-root index lock |
| `src/git.rs` | `git remote get-url origin` | lock |
| `src/git.rs` | `git rev-parse --abbrev-ref HEAD` | lock |
| `src/git.rs` | `git status --porcelain` | lock |
| `src/git.rs` | `git symbolic-ref refs/remotes/origin/HEAD` | lock |
| `src/repo_sync.rs` | `git merge --no-edit origin/<base>` | repo-root index lock |
| `src/repo_sync.rs` | `git status --porcelain` (conflict read) | lock, mid-merge |
| `src/repo_sync.rs` | `git merge --abort` | lock |

`detect_default_branch` is in the list for the same reason as the other three: it is not
called by `finish_task`, but it *is* issued by `measure_repo` on the sync path, so leaving it
unbounded would keep `repo-sync.allium`'s claim false.

The `src/git.rs` helpers already take `&dyn ProcessRunner`, so they need no signature
changes.

`SUBPROCESS_TIMEOUT` (60 s, in `src/process.rs`) is reused — no new constant, and no new
configuration surface. The value is deliberately generous for the reason given above.

### One testability seam

`finish_task` gains a `timeout: Duration` field on its existing `FinishContext` struct,
which `wrap_up`'s only production call site sets to `SUBPROCESS_TIMEOUT`. This follows the
established precedent in `src/dispatch/worktree.rs`, whose `provision_worktree` already
takes a timeout parameter documented as "use `SUBPROCESS_TIMEOUT` in production; pass a
short duration in tests".

It exists for a concrete reason rather than symmetry. `MockProcessRunner` only short-circuits
a scripted delay inside `run_with_timeout`; in the unbounded `run` it *sleeps* for it. So a
test that proves the bound is missing by scripting a 60 s stall would take 60 s to go red.
With an injectable timeout the same test uses a 50 ms bound and is instant in both
directions. There are three `FinishContext` construction sites — two test helpers and
`wrap_up`'s — so the change is three lines.

`sync_repo` and the `src/git.rs` helpers keep the constant inline: their tests assert the
timeout was *passed* (via `recorded_timeouts()`) rather than that it fires, so they never
script a delay and never sleep. No signature changes there.

### What this does and does not claim

It prevents an indefinite hang and makes an existing spec claim true. It does **not**
improve p50 or p99: a 60 s bound would not have altered a single one of the 538 recorded
calls. Any implementation or review that reports this as a latency improvement is
misreporting it.

## Error surface

No new error variants and no new response shapes. `run_with_timeout` returns `Err` on
timeout, and every site already maps `Err` to an existing error:

- the pull to `FinishError::Other` with its "Failed to pull" prefix,
- the rebase to `FinishError::Other` with "Failed to run git rebase",
- the fast-forward to `FinishError::Other` with "Failed to fast-forward",
- the `src/git.rs` helpers to `FinishError::Other` / `SyncError::Other` via their existing
  `String` errors.

A timeout therefore reaches the agent as, for example,
`wrap_up failed: Failed to pull: git timed out after 60s`.

`git rebase --abort` and the conflict-path `git status --porcelain` read are already
best-effort (`let _ =` and `unwrap_or_default()` respectively), so a timeout there degrades
exactly as a failure does today. Same for the corresponding pair in `sync_repo`'s merge
block. Its merge itself already maps `Err` to `SyncError::Other` with a "Failed to run git
merge" prefix.

One case deserves naming because it is the only one where a timeout is not simply a
failure: if `git merge --abort` times out in either function, the checkout is left
mid-conflict. That is already true today when the abort fails for any other reason, and
both functions already report the underlying conflict rather than the abort's fate, so the
behaviour is unchanged — but it is the reason the abort keeps a generous bound rather than a
tight one.

## Spec changes

`WrapUpRebase` in `docs/specs/pr-workflow.allium` states its `ensures` in terms of task
state and describes the git steps in `@guidance` prose; it does not enumerate a failure
vocabulary. So no `ensures` changes. But a new failure mode is behaviour, so the spec moves
first, via `allium:tend`:

1. **`WrapUpRebase`** — add a bounding note mirroring the existing one in
   `docs/specs/repo-sync.allium`: every subprocess on the finish path is bounded, the
   network call because it touches the network and the rebase/merge/abort/status calls
   because they can block on a repository lock; a subprocess that times out is simply one
   that failed, which the surrounding logic already reports.
2. **`docs/specs/repo-sync.allium`** — extend the existing bounding note so it covers the
   preflight reads and the whole merge block it currently claims but does not bound.

Then `allium:weed` to confirm spec and code agree.

## Tests

TDD: tests first, and each new test must be seen to fail against today's code.

The seam is `MockProcessRunner::new_with_delays`. Its `run_with_timeout` bails when the
scripted delay is greater than or equal to the timeout **without sleeping** (see
`src/process.rs`), so the tests are deterministic and do not trip
`scripts/check-no-test-sleep.sh`. Its `recorded_timeouts()` accessor returns the timeout
passed per call, positionally aligned with `recorded_calls()`.

In `src/dispatch/finish.rs`'s `mod tests`:

1. `finish_task_bounds_the_pull` — script a delay at or beyond `SUBPROCESS_TIMEOUT` on the
   pull; expect `FinishError::Other` whose message contains both "Failed to pull" and
   "timed out". Fails today: bare `run` ignores the scripted delay and returns `Ok`.
2. `finish_task_bounds_the_rebase` — same, expecting "Failed to run git rebase".
3. `finish_task_bounds_the_fast_forward` — same, expecting "Failed to fast-forward".
4. `finish_task_bounds_every_subprocess_it_runs` — the test that actually pins the
   convention rather than three instances of it: on a successful run, assert every entry
   of `recorded_timeouts()` is `Some(SUBPROCESS_TIMEOUT)` and that its length equals
   `recorded_calls().len()`. A future unbounded call added to this function fails here.

In `src/git.rs`'s `mod tests`: one test per helper asserting `recorded_timeouts()` is
`[Some(SUBPROCESS_TIMEOUT)]`.

In `src/repo_sync.rs`'s `mod tests`: a `sync_repo_bounds_every_subprocess_it_runs` guard of
the same shape as (4), plus a timeout test on the merge itself expecting `SyncError::Other`
containing "Failed to run git merge" and "timed out".

No existing test needs to change. `MockProcessRunner` answers `run` and `run_with_timeout`
identically when no delay is scripted, and the other `ProcessRunner` implementations
(`SocketRunner` in `tests/tmux_harness/mod.rs`, and the feed runners) implement only `run`,
inheriting the trait's delegating default.

## Out of scope

- Reducing the 1.7 s p50. Deliberately not attempted; see the rejected options above.
- Any new timing constant, configuration flag, or poll loop.
- The other unbounded `runner.run` calls in the codebase that are not on the finish or sync
  paths. Bounding those is defensible but is a different task with a different blast
  radius.

## Follow-up

Record the per-action latency breakdown as a knowledge-base learning, so the next agent
asking "why is `wrap_up` slow?" does not repeat the trajectory analysis: `done` and `pr`
are ~5 ms; `rebase` is a p50 1.7 s network round-trip that is inherent to the operation,
not a defect.
