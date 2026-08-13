# 4091 — Serialise concurrent feed syncs of the same epic

## Status of the premise

The task description is written against code that is **not in `main`**. The
`RemovedFeedTask` / `cleanup_removed_feed_tasks` teardown, the `FeedSyncOutcome`
return type, the `DegradedEmptyEmission` guard, and all three tests the
description names as pinning the invariant
(`src/feed/ingest/tests.rs::moved_task_is_never_reported_as_removed`,
`src/feed/mod.rs::tick_removed_task_tears_down_its_worktree`,
`src/runtime/tests.rs::exec_trigger_epic_feed_removed_task_tears_down_its_worktree`)
live on the unmerged local branch
`3989-an-empty-feed-emission-silently-deletes-every-feed-task-in-a-reviews-parent-subtree`
(tip `51e28f9e`, 13 commits ahead of `main`, 3 behind, no PR open). So does the
design doc that scoped this task out.

**Decision (agreed with the user): plan against `main` + #3989.** The guard must
cover the teardown step, because teardown is what makes this race destructive
rather than merely lossy.

**Consequence for the implementing agent: do not start on plain `main`.** See
WP0 below — it is a blocking prerequisite, not a footnote. Everything after it
assumes the merge has happened. Line references are to the post-merge tree;
symbol references (`path::symbol`) are stable either way.

## WP0 — get #3989 into this branch (blocking prerequisite)

Do this first and do not start WP1 until `cargo test` is green on the result.

**Direction matters. Do not rebase this branch onto `51e28f9e`.** The #3989
branch is 3 commits *behind* `main`, and one of those three is `effb09e9`
("unify feed-stdout parsing across all three entry points") — the very commit
that makes the manual path use the shared `parse_feed_items`. Rebasing 4091 onto
the #3989 tip would land this work on a tree where that unification does not
exist, so WP2's shared cycle would be built on the older `serde_json::from_slice`
glue and the conflict resolution below would be unsatisfiable without a
cherry-pick.

So, in preference order:

1. **Best: #3989 lands on `main` on its own** (it is finished work — tip commit
   is `fix(feeds): close the final-review findings` — but has no PR open, so it
   needs whoever owns it to wrap it up). Then `git merge origin/main` into this
   branch and carry on. Whoever merges #3989 resolves the two conflicts below;
   verify they resolved them as stated before building on top.
2. **Otherwise: merge, don't rebase.** From this branch, `git merge` the #3989
   branch. That brings its 13 commits in while keeping `main`'s 3, and puts the
   conflict resolution in this branch's history where it can be reviewed. Note
   for the wrap-up: this branch then carries #3989's commits, so it cannot merge
   to `main` before #3989 does without taking that work with it — flag it on the
   PR rather than letting it land silently.

Either way, the two known `main` ↔ #3989 conflicts resolve as:

- `src/runtime/epics.rs::exec_trigger_epic_feed` — #3989 still calls
  `serde_json::from_slice`; `main`'s `effb09e9` replaced that with the shared
  `crate::feed::parse_feed_items`. **Keep `main`'s shared parse** — the whole
  point of WP2 is that both paths share this layer.
- `FeedMessage::Refreshed` — `main` carries `wrote_stderr`; #3989 deletes it
  (a degraded zero-item emission is now a hard failure, so the zero-item stderr
  hint is unreachable). **Keep #3989's two-field form**, and check that
  `App::handle_feed_refreshed` and any status-bar test or snapshot referencing
  `wrote_stderr` came along with the deletion.

Both `git log --oneline main..<3989-branch>` and the reverse should be re-checked
at the time — the counts quoted here (13 ahead, 3 behind) are as of 2026-08-13
and will drift.

## The bug

Nothing serialises the two paths that run a feed cycle for the same epic:

- auto-poll — `src/feed/mod.rs::FeedRunner::tick` spawns a `FeedJob` per eligible
  epic, fire-and-forget (`tokio::task::spawn(job.run())`).
- manual `r` — `src/runtime/epics.rs::exec_trigger_epic_feed` spawns its own
  task, and deliberately does **not** touch `FeedRunner`'s `last_run`
  (feeds.allium `ManualFeedTrigger`), so it can land at any point inside an
  in-flight poll.

`src/feed/ingest/role_routed.rs::run_role_routed_feed_sync` then runs as a
sequence of separate `db_call`s with `await` points between them:

```
ensure_role_sub_epics          -> list_sub_epics / create_epic / patch_epic
build_existing_task_index      -> list_tasks_for_epic × N
route_and_group_entries        -> set_task_epic_id + patch_task   (the MOVE)
upsert_role_groups             -> upsert_feed_tasks per group     (reports removed)
delete_stale_subtree           -> delete_stale_subtree_feed_tasks (reports removed)
clear_parent_stranded_tasks    -> upsert_feed_tasks(parent, &[])  (reports removed)
```

Every one of those statements filters on the task's **current** `epic_id`. The
ordering (move before the deletes) is load-bearing and is pinned within a single
pass by `moved_task_is_never_reported_as_removed`. It is not pinned *across*
passes: cycle A can move a task to `team_reviews` and cycle B — whose keep-set
was computed from its own, older emission — can reach `delete_stale_subtree`
before A does, see the task under a keep-set that does not contain it, and delete
it. Post-#3989 that `DELETE ... RETURNING` hands the row to
`src/feed/mod.rs::cleanup_removed_feed_tasks`, which does
`git worktree remove --force`, `git branch -D` and `tmux kill-window` on it,
unconditionally on `status`/`sub_status`. So the interleave now destroys a live
review agent's uncommitted work, not just a DB row.

Secondary cost, independent of correctness: two concurrent cycles mean two
`scripts/fetch-reviews.sh` runs, i.e. double the GitHub search API spend, and a
hung feed command lets `tick` pile up one orphaned task per interval forever.

## Chosen approach

Per-epic **single-flight claim** held across the *whole* cycle —
exec → parse → degraded-guard → sync → teardown — with the later attempt
**dropped**, not queued. Agreed with the user; the reasoning:

- Holding the claim across `exec` (not just the sync) is what removes the double
  API spend, and it also removes the *stale-snapshot* variant of the bug, where
  the two cycles serialise on the sync but the second one reconciles against an
  emission fetched before the first one wrote.
- Dropping rather than queueing: `src/feed/exec.rs::exec_feed_command` has **no
  timeout** (it is a bare `tokio::process::Command::output()`, not
  `run_bounded`), so a queue behind a hung command is unbounded. Dropping caps
  in-flight work at one cycle per epic and turns today's silent task pile-up
  into a debug log line. Repeated `r` presses cannot queue either.
- The claim is released by `Drop`, so it survives every early `return` and a
  panicking spawned task alike.

Rejected: making `run_role_routed_feed_sync`'s steps one transaction.
`tokio_rusqlite` cannot span a transaction across separate `db_call` closures
(each takes `FnOnce(&mut Connection)`), so the whole five-module sync would have
to collapse into one synchronous closure inside the `db` layer — a large rewrite
that inverts the service/db layering, and it would still leave the two paths
double-fetching.

### Scope: one shared feed-cycle function

Agreed with the user, and required by the repo's own rule (knowledge base #284:
a decision that must stay identical across the auto-poll and manual paths lives
in one shared function, never duplicated). The two paths already share `exec`,
`parse`, the degraded guard and `run_feed_sync_by_role` — but each re-implements
the ~40 lines of glue between them, and #3989 had to add its teardown call twice
for exactly that reason. The claim would be the third such duplication.

So: extract `src/feed/cycle.rs::run_feed_cycle`, owning everything from claiming
the epic to finishing teardown, returning an outcome the two callers only
*present* differently.

### Two deliberate behaviour changes this extraction forces

Both are improvements, both are called out in the spec, both get their own test.
Neither is required by the race fix — if review pushes back, WP4 can be dropped
without affecting WP1–WP3.

1. **`feed_role` and `group_by_repo` are read inside the cycle**, from one
   `db.get_epic(epic_id)` after the claim, instead of the auto-poll path using
   its in-hand `Epic` and the manual path mixing a fresh `get_epic` read for
   `feed_role` with a `group_by_repo` value passed down from the TUI's cached
   board. That mixed sourcing is a live drift: a manual `r` today can sync with a
   stale `group_by_repo`. One read, one source of truth, one fewer parameter on
   both callers. An epic deleted between spawn and cycle becomes a `Failed`
   outcome rather than a flat upsert onto a missing epic.
2. **Both paths now notify *after* teardown.** feeds.allium currently records the
   divergence as an open question — `FeedJob::run` sends `EpicChanged` then
   awaits cleanup; `exec_trigger_epic_feed` awaits cleanup then sends
   `Refreshed`. A shared function that owns teardown has to pick one. Pick the
   manual path's stronger guarantee ("reconciled AND cleaned up") for both, and
   close the open question in the spec. Cost: the board's `EpicChanged` for an
   auto-poll now lands after `git worktree remove`, so a mass-merge removing a
   dozen tasks lags the board by a beat.

The role-sub-epic provisioning guard (the `debug_assert!` block in
`FeedJob::run`) also moves into the shared cycle, at its current position — after
parse, before sync — so failure precedence is unchanged. The manual path gains
it, which it should have had all along: a manual `r` on a misconfigured role
sub-epic must not flat-upsert into it either.

### Keying and scope of the claim

Keyed by the **polled epic's id** — the `reviews_parent`, never a role sub-epic.
Only the parent carries a `feed_command` (enforced at provisioning), and only the
parent's cycle writes the subtree, so one key per polled epic covers the whole
subtree it reconciles. Distinct epics never contend.

**Out of scope, noted for honesty:** two *nested* feed epics whose subtrees
overlap would still contend — `delete_stale_subtree_feed_tasks(P, …)` reaches any
child of `P`, including a child that is itself a feed epic. Not reachable today
(the role-routed path requires `feed_role = reviews_parent`, whose children carry
no `feed_command`; the flat/grouped paths delete per-epic), but a
`group_by_repo` feed epic with a hand-nested feed child would expose it. Also out
of scope: giving `exec_feed_command` a `run_bounded` timeout, which would bound
how long a claim can be held. Both worth follow-up tasks.

## Spec first

`docs/specs/feeds.allium`. Use the `allium:tend` skill; verify with
`allium:weed` and `allium check`.

1. **New rule `SerialisedFeedCycle`**, in a new `-- == Serialisation ==` section
   placed immediately after `FeedTick` (it governs the entry into a cycle, from
   either path).

   - `when: FeedCycleRequested(epic)` — a new event, raised per eligible epic by
     `FeedTick`'s `Spawn` and by `ManualFeedRefresh`. Document that these are the
     only two raisers (the `verify-feed` CLI execs a feed command but performs no
     sync and shares no process with the TUI, so it is outside this rule —
     knowledge base #387).
   - `ensures`: if a cycle for `epic` is in flight, the request is **dropped** —
     no exec, no parse, no sync, no teardown, no notification — and the outcome
     is surfaced per path (silent debug log for the poll; a distinct status-bar
     line for the manual trigger). Otherwise the request claims `epic` for the
     whole cycle and releases it on every exit path, success or failure.
   - State explicitly: the claim spans exec → parse → degraded-guard → sync →
     teardown; it is keyed by the polled epic's id, so the reviews_parent's claim
     covers its whole subtree and distinct epics never contend; and it is
     symmetric — whichever path claims first wins, so an auto-poll tick is
     dropped by an in-flight manual refresh exactly as the reverse.
   - State the interaction with `FeedTick`: a dropped tick does **not** retry
     sooner, because `last_run` is bumped before the spawn. The epic waits one
     full interval.
2. **New invariant `OneFeedCycleAtATimePerEpic`** — at most one in-flight feed
   cycle per epic; therefore the non-transactional steps of
   `RoleRoutedFeedSync` can never interleave with another cycle's steps for the
   same epic. Cross-reference the `TaskTeardown` fan-out paragraph in
   `RoleRoutedFeedSync`'s `@guidance`, whose "each statement filters on the
   task's CURRENT `epic_id`, so the move must land before the deletes" argument
   is only sound under this invariant.
3. **`ManualFeedTrigger`** — add the third outcome to its status-bar contract:
   `"Feed for '<title>' is already refreshing…"` when the request is dropped.
   Note it is neither a success nor a failure.
4. **`RoleRoutedFeedSync` `@guidance`** — the teardown paragraph's
   "Teardown-vs-notification ordering differs between the two feed paths and no
   clause governs it … open question" is resolved by behaviour change 2. Replace
   it with the normative order: both paths await teardown, then notify.
5. Point the `@guidance` of every touched rule at
   `src/feed/cycle.rs::run_feed_cycle` and `src/feed/guard.rs::FeedSyncGuard`,
   and update `FeedTick` / `ManualFeedTrigger` / `DegradedEmptyEmission` /
   `FeedCommandStderrOnSuccess` guidance where they name `FeedJob::run` or
   `exec_trigger_epic_feed` as the place a step happens — after the extraction
   most of those steps happen in the shared cycle, and the two named functions
   become presentation-only. `./scripts/check-doc-paths.sh` and
   `check-doc-symbols.sh` will catch any reference left dangling; prefer the
   `path::symbol` form over `file:NN`.

## Work packages

WP0 (above) is a blocking prerequisite and must be green before WP1 starts.

TDD throughout: each package writes its tests first and watches them fail for
the right reason before the implementation lands.

### WP1 — `FeedSyncGuard` (the primitive)

**Tests first**, inline `mod tests` in `src/feed/guard.rs` (with the mandatory
`#![allow(clippy::unwrap_used, clippy::expect_used)]`):

- `claim_is_exclusive_per_epic` — a second `try_claim` for the same id yields
  `None` while the first claim is alive.
- `claim_is_released_on_drop` — dropping the claim lets the next `try_claim`
  succeed.
- `different_epics_claim_independently`.
- `claim_is_released_when_the_holder_panics` — `tokio::spawn` a task that panics
  while holding a claim, `await` its `JoinHandle` (the `Err` is the deterministic
  completion signal — no sleep), then assert the epic can be claimed again.
- `a_poisoned_registry_still_claims` — the internal mutex is poison-tolerant, so
  a panic *inside* the critical section does not wedge the feed forever.

**Then implement** `src/feed/guard.rs`:

```rust
pub(crate) struct FeedSyncGuard { in_flight: std::sync::Mutex<HashSet<EpicId>> }
pub(crate) struct FeedClaim { guard: Arc<FeedSyncGuard>, epic_id: EpicId }

impl FeedSyncGuard {
    pub(crate) fn try_claim(self: &Arc<Self>, epic_id: EpicId) -> Option<FeedClaim>;
}
impl Drop for FeedClaim { /* remove epic_id */ }
```

Notes:

- A `std::sync::Mutex` is correct here and must **not** be a `tokio::sync::Mutex`:
  the lock is held only for a `HashSet` insert/remove and is never held across an
  `await`. This mirrors `TuiRuntime.editor_session`.
- Poison handling without `unwrap`/`expect` (the pre-push hook applies
  `-D warnings`): `match self.in_flight.lock() { Ok(g) => g, Err(p) => p.into_inner() }`.
  Recovering rather than propagating is deliberate — a poisoned registry must not
  make an epic permanently unpollable.
- `Drop`, not an explicit release call, so the `?`/early-`return`-heavy cycle
  body and a panicking spawned task both release.

### WP2 — `run_feed_cycle` (the shared sequence)

**Tests first.** The cheap, robust pair — hold a claim from the test, drive each
path, assert it did nothing:

- `src/feed/mod.rs` tests: `tick_skips_an_epic_whose_cycle_is_already_in_flight`
  — take a claim on the epic, `runner.tick().await`, assert no `McpEvent`
  arrives within the timeout window (the established absence idiom in this file)
  and that a pre-existing feed task is untouched.
- `src/runtime/tests.rs`:
  `exec_trigger_epic_feed_reports_already_refreshing_while_a_cycle_is_in_flight`
  — take a claim, trigger, assert `FeedMessage::AlreadyRefreshing`, and assert a
  pre-existing task that carries a worktree still exists.

The flagship test the task asks for — **both paths against one epic, with a real
cycle in flight**. Deterministic without any sleep, using a FIFO as the
handshake:

- `manual_refresh_is_dropped_while_a_real_auto_poll_cycle_is_in_flight`
  (`src/runtime/tests.rs`, since it needs both a `TuiRuntime` and a
  `FeedRunner` sharing one guard).
- `mkfifo` into the scratch dir; the epic's `feed_command` is
  `cat <fifo>; echo '[]'`.
- Test calls `runner.tick()` (spawns the cycle, which blocks in `exec` on the
  `cat`), then — from `spawn_blocking` — opens the FIFO **for writing**. That
  open blocks until the reader opens, so it *is* the signal that the exec is in
  flight; no polling, no sleep.
- With the cycle provably mid-exec, trigger the manual path and assert
  `AlreadyRefreshing` plus zero writes.
- Then write + close the FIFO to let the cycle finish, and drain events.
- If the FIFO handshake proves awkward on the implementation pass, the two
  claim-held tests above are the fallback coverage — they pin the same contract,
  just one layer in. Do not silently drop the flagship test without saying so.

**Then implement** `src/feed/cycle.rs`:

```rust
pub(crate) struct FeedCycle {
    pub(crate) db: Arc<dyn TaskStore>,
    pub(crate) runner: Arc<dyn ProcessRunner>,
    pub(crate) guard: Arc<FeedSyncGuard>,
    pub(crate) epic_id: EpicId,
    pub(crate) epic_title: String,
    pub(crate) cmd: String,
    /// `Some` from the auto-poll path, which fetches once per tick for all
    /// epics; `None` from the manual path, resolved inside after the claim so
    /// a dropped request does no DB work.
    pub(crate) known_paths: Option<Arc<Vec<String>>>,
}

pub(crate) enum FeedCycleOutcome {
    Synced { count: usize, affected_epics: Vec<EpicId> },
    Busy,
    Failed(String),
}

impl FeedCycle { pub(crate) async fn run(self) -> FeedCycleOutcome }
```

Body, in order — this is the whole of today's two glue blocks, merged:

1. `let Some(_claim) = self.guard.try_claim(self.epic_id) else { return Busy };`
2. `get_epic` → `feed_role`, `group_by_repo` (behaviour change 1; missing epic →
   `Failed`).
3. `exec_feed_command` → `Err` → `Failed`.
4. `parse_feed_items` → `Err` → `Failed`.
5. `degraded_empty_emission` → `Some(reason)` → `Failed(reason)`.
6. role-sub-epic provisioning guard (`debug_assert!` + `Failed`).
7. `known_paths` / `resolve_feed_item_repo_paths` / `resolve_base_branches` /
   `FeedItemWithTarget::zip`.
8. `run_feed_sync_by_role` → `Err` → `Failed`.
9. `recalculate_epic_status_after_feed`, then
   `cleanup_removed_feed_tasks(runner, outcome.removed).await`.
10. `Synced { count, affected_epics }`.

The cycle **logs** every failure itself (it holds `epic_id` and `epic_title`), so
callers only present. That removes `FeedJob::run`'s `warn_on_err` duplication and
keeps a `Failed` from being logged twice.

### WP3 — rewire the two callers, and the wiring itself

**Tests first**: the existing `tick_*` and `exec_trigger_epic_feed_*` suites are
the behaviour-preservation harness — every one of them must stay green
**unmodified**, in particular:

- `tick_two_ticks_lose_nothing` — two back-to-back zero-interval ticks. Its
  assertion (exactly one feed task in the subtree) holds whether the second tick
  serialises or is dropped as `Busy`. Keep the test; update only its comment to
  say the property is now structurally enforced rather than incidental.
- `tick_twice_is_idempotent` and `tick_interval_not_elapsed_skips_command` both
  await the first refresh before the second tick, so neither hits `Busy`.
- `tick_removed_task_tears_down_its_worktree` and
  `exec_trigger_epic_feed_removed_task_tears_down_its_worktree` (#3989) pin that
  teardown is still wired on both paths after the extraction.

New: `exec_trigger_epic_feed_honours_a_group_by_repo_change_made_after_load`
pins behaviour change 1 — the manual path syncs against the epic's current
`group_by_repo`, not the value the board was holding.

**Then implement:**

- `FeedJob::run` collapses to: build a `FeedCycle`, `match` it. `Synced` → one
  `McpEvent::EpicChanged` per affected epic (now after teardown — behaviour
  change 2). `Busy` → `tracing::debug!` "a feed cycle for this epic is already
  in flight; skipping". `Failed` → nothing (already logged).
- `exec_trigger_epic_feed` collapses to the same `match`. `Synced` →
  `FeedMessage::Refreshed { epic_title, count }`. `Failed(e)` →
  `FeedMessage::Failed`. `Busy` → new
  `FeedMessage::AlreadyRefreshing { epic_title }`. Both `feed_role` and
  `group_by_repo` drop out of the function; `group_by_repo` can then also drop
  out of the `Command`/`Message` that carries the trigger, if nothing else reads
  it — check before removing.
- `src/tui/messages/feed.rs`: add `AlreadyRefreshing { epic_title }` and route it
  to a new `App::handle_feed_already_refreshing`, which sets the status message
  `"Feed for '<title>' is already refreshing…"` (a normal transient status, so it
  expires on `STATUS_MESSAGE_TTL` like the other two).
- Wiring: `FeedRunner::new` constructs the guard; add
  `pub(crate) fn sync_guard(&self) -> Arc<FeedSyncGuard>`, mirroring the existing
  `epic_invalidate_tx()`. `runtime::bootstrap` (`src/runtime/mod.rs:492`) binds
  the runner to a local, pulls `sync_guard()`, and stores it on a new
  `TuiRuntime.feed_sync_guard` field.
  **Both call sites must share one guard or the fix is inert**, and there is no
  compiler check for that. The eight test constructors of `TuiRuntime` in
  `src/runtime/editor.rs` (lines ~715–1156) build their `FeedRunner` inline in the
  struct literal; each must be changed to bind it first and take the guard from
  it — *not* to mint a fresh `FeedSyncGuard::default()`, which would silently
  make those runtimes unable to observe the poll path. Consider a small
  `fn wire_feed(db, notify, runner) -> (FeedRunner, Arc<FeedSyncGuard>)` factory
  used by bootstrap and all eight test sites so the pairing cannot be got wrong.

### WP4 — spec alignment and the doc sweep

- `allium:weed` over `feeds.allium` against the new code; `allium check`.
- Status-bar snapshots: a new transient message should not touch
  `src/tui/tests/snapshots/`, but if any snapshot does move, accept it with
  `INSTA_UPDATE=always cargo test tui::tests::snapshots` and **delete the
  `*.snap.new` files afterwards**.
- `CLAUDE.md` needs no change (no new subsystem, no new external dependency).
- Record a learning: the per-epic single-flight claim is the third decision that
  had to be identical across the two feed paths, and the reason `src/feed/cycle.rs`
  now exists — so the next agent adds step four *there*, not twice.

## Verification

```
cargo test
cargo clippy --all-targets -- -D warnings
allium check docs/specs/feeds.allium
```

Plus the pre-push hook in full (`cargo fmt`, the doc-path/doc-symbol checkers and
their self-tests, `check-no-test-sleep.sh` — the FIFO test must not introduce a
sleep, and if a bounded poll step turns out unavoidable it needs an
`// allow-test-sleep: <why>` marker with a real justification).

## Risks

- **Silent inertness.** If the two call sites end up on different guard
  instances, everything passes and nothing is serialised. The claim-held tests in
  WP2 exercise the guard directly and would *also* pass under a mis-wire — only
  the FIFO test crosses both paths through one runtime. That is the argument for
  landing it, and for the `wire_feed` factory.
- **A hung feed command now blocks its epic's manual refresh** for as long as it
  hangs, instead of piling up parallel cycles. Better failure mode, but a
  visible change; the status line says why. Bounding `exec_feed_command` with
  `run_bounded` is the real fix and is a follow-up.
- **Auto-poll board updates lag teardown** (behaviour change 2). Bounded by the
  `git worktree remove` fan-out, which is already parallel across repos.
- **Merge order with #3989** (see WP0). If #3989 is revised before it lands,
  WP3's caller bodies shift. Nothing else in the plan depends on its internals.
  The live hazard is direction: rebasing onto the #3989 tip silently drops
  `main`'s parse unification, which WP2 builds on.
