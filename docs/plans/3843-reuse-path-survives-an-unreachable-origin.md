# 3843 — Unreachable origin must not abort a reuse dispatch

Follow-up from #3810's whole-branch review (finding I2), deliberately deferred there.

## Problem

`provision_worktree` (`src/dispatch/worktree.rs`) calls `fetch_origin(...)?` **before**
measuring `reused_worktree`. When the worktree directory already exists, `git worktree add`
is skipped entirely — no ref is consumed to *create* anything. The resolved start point
feeds only the reuse rebase preamble and a tracing field.

So on the reuse path an unreachable origin currently aborts a dispatch that needs no
network at all: no tmux window, no agent, task bounced to Backlog.

### The cost is not just the abort

The task description frames the fix as "downgrade the abort to a warning". That is
necessary but **not sufficient** — it changes *what* happens at the end without changing
*how long* the user waits. On a blackholing network (slow timeout rather than fast DNS
failure) the current budget is:

| step | worst case |
|---|---|
| `git fetch` attempt 1 | `SUBPROCESS_TIMEOUT` = 60 s |
| `git ls-remote` classification probe | 60 s |
| `FETCH_RETRY_DELAY` | 0.5 s |
| `git fetch` attempt 2 | 60 s |
| `FETCH_RETRY_DELAY` | 0.5 s |
| `git fetch` attempt 3 | 60 s |
| **total** | **≈ 4 minutes of blocking** |

Downgrading abort → warning alone leaves all 4 minutes in place. The fix has to address
the budget too.

### Why the retries and the probe can go on the reuse path

Both exist to serve the abort decision, and only that decision:

- **The `ls-remote` probe** exists to tell 404-class from infra-class, because the two
  outcomes differ: 404 → fall back to local (fine), infra → abort. On the reuse path both
  classes produce the *same* outcome — warn, and use a start point that isn't
  `origin/<base>`. The probe would buy nothing but a slightly nicer warning string, at the
  cost of a full network timeout.
- **The retry budget** exists to smooth transient failures (e.g. ref-lock contention)
  *before an abort*. With no abort to smooth, a second and third attempt cost two more
  timeouts to improve a warning that is already non-fatal.

Remove the abort and both lose their purpose. The single fetch attempt stays, because
`dispatch.allium` wants `origin/<base>` kept fresh and the repo-sync drift indicator
depends on it — one attempt preserves that intent on the happy path while bounding the
offline cost to one `SUBPROCESS_TIMEOUT` (60 s, down from ~4 min).

### Facts verified in the code

- `git fetch` and `git ls-remote` are the only network calls in this path.
- `crate::git::has_origin_remote` runs `git remote get-url origin` — **local**, no network.
- `crate::repo_sync::ahead_behind` runs `git rev-list --count --left-right` — **local**,
  no network. It compares against the *remote-tracking* ref, so after a failed fetch its
  reading is stale, which is why the reuse-failure path skips it (below).

## Design

### 1. Measure reuse before fetching

Hoist `let reused_worktree = Path::new(&worktree_path).exists();` above the fetch block.
Note that `fs::create_dir_all(".worktrees")` must stay where it is — inside the
`else` branch, below the fetch — or an aborted fresh dispatch starts leaving an empty
directory behind again (a #3845 fix).

### 2. A fetch policy, not a bare bool

```rust
/// Whether provisioning still needs origin to be reachable.
enum FetchPolicy {
    /// A fresh worktree is about to be created from the resolved ref, so an
    /// unreachable origin must abort rather than branch off a stale local ref.
    Required,
    /// The worktree directory already exists: `git worktree add` is skipped and
    /// no ref is consumed to create anything. One attempt keeps origin fresh;
    /// failure is a warning, not an abort.
    BestEffort,
}
```

`fetch_origin` takes the policy and gains a third outcome:

```rust
enum FetchOutcome {
    Fetched,
    NoOriginRef(String),
    /// Reuse path only: origin could not be reached. Carries the `Note:` text.
    Unreachable(String),
}
```

Under `BestEffort`: one attempt, no `classify_fetch_failure` call, no retry sleep.
Under `Required`: today's behaviour, unchanged.

### 3. Start point on a failed reuse fetch

| base | start point | why |
|---|---|---|
| `BaseRef::Branch(b)` | `StartPoint::Local { b }` | Local `<base>` is the only ref whose freshness we can vouch for. It is also what wrap-up rebases onto, so the preamble and wrap-up agree. Choosing `origin/<b>` off a stale tracking ref is exactly the SHA-duplication hazard #3810 removed. |
| `BaseRef::PrHead(b)` | `StartPoint::Remote { b }` | A review must **never** get a `Local` start point — `BaseRef::PrHead`'s whole contract. The existing worktree already holds the PR's code from the previous attempt, so nothing is poisoned; the preamble's `git rebase origin/<b>` may fail visibly, and the `Note:` explains why. |

Both carry a `fetch_warning`, so the agent is told in its own prompt rather than only in a
server-side log.

### 4. Error string gains a next step

Current: `Could not reach origin to fetch {base} after {N} attempts: {err}`.
Add what to do about it, and make it explicit that this is the fresh-worktree path.

## Test plan (TDD — tests first, then implementation)

All in `src/dispatch/tests.rs` unless noted. Pre-create the worktree dir with the existing
`make_test_repo_with_worktree("42-fix-bug")` helper to hit the reuse path.

1. `provision_worktree_reuse_survives_an_unreachable_origin`
   — dir exists, every fetch fails ⇒ `Ok`, `start_point == StartPoint::Local { base: "main" }`,
   `fetch_warning.is_some()`, and a tmux window **is** created.
2. `provision_worktree_reuse_does_not_retry_or_probe_an_unreachable_origin`
   — exactly **one** `git fetch` call and **zero** `ls-remote` calls. This is the test that
   locks the 4-min → 60-s budget; without it the abort could be downgraded while leaving
   the cost in place.
3. `provision_worktree_reuse_of_a_pr_head_keeps_the_remote_start_point`
   — `BaseRef::PrHead("feature-x")` + reuse + failed fetch ⇒ `Ok`,
   `StartPoint::Remote { base: "feature-x" }`, never `Local`.
4. `provision_worktree_fresh_still_aborts_on_an_unreachable_origin`
   — regression guard for the fresh path. `provision_worktree_kills_git_fetch_on_timeout_and_aborts`
   already covers the timeout flavour; assert the retry/probe budget is still spent there so
   step 2's change cannot silently leak onto the fresh path.
5. Error-string test — the abort message names a next step, not just cause + attempt count.

Existing tests that must stay green unchanged (they all use `make_test_repo()`, i.e. the
fresh path): `provision_worktree_fetch_failure_falls_back_to_local_without_retry`,
`provision_worktree_pr_head_missing_from_origin_aborts_rather_than_using_local`,
`provision_worktree_retries_fetch_before_falling_back`,
`provision_worktree_kills_git_fetch_on_timeout_and_aborts`.

Note the ordering hazard flagged by learning #351: pre-creating the worktree dir is exactly
what flips `reused_worktree` to true, so tests 1–3 get the reuse path for free.

## Spec update

`docs/specs/dispatch.allium`, the "Fetch failure is classified, not blanket-tolerated"
block (~lines 204–227), currently states the infra-class abort without qualification. It
needs to say the abort is conditional on a **fresh** worktree, and that the reuse path
runs a single best-effort fetch with no classification probe and no retries. The
start-point selection block (~lines 229–246) needs the reuse-failure rows from the table
above. Apply via `allium:tend`, verify with `allium:weed`.

## Verification

```
cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh
```

## Base-branch note

This work sits on top of #3810 and #3845, which are **not on `origin/main`** — they are
unpushed commits on local `main` (tip `66d95cde`). This branch is rebased onto local
`main`, not `origin/main`. Rebasing onto `origin/main` would erase the code this task
exists to change.
