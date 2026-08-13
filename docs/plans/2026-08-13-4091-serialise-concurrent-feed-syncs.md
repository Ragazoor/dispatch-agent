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

## WP0 — get #3989 into this branch — ALREADY DONE

**This is satisfied. Do not redo it.** Merge commit `00798712` on this branch
brought #3989 in; `cargo test` and `cargo clippy --all-targets -- -D warnings`
are green on the result. Both predicted conflicts resolved the way the plan
wanted, verified in the tree:

- `src/runtime/epics.rs:276` uses `crate::feed::parse_feed_items` (`main`'s
  shared parse), not `serde_json::from_slice`.
- `src/tui/messages/feed.rs:19` has the two-field
  `Refreshed { epic_title, count }` (#3989's form); `wrote_stderr` is gone from
  the message, its handler and its tests.

The rest of this section is the reasoning behind *how* it was merged, kept
because it explains why this branch carries #3989's 13 commits and what that
means at wrap-up time. Skip to WP1 unless that matters to you.

**Direction mattered. Rebasing this branch onto `51e28f9e` would have been
wrong.** The #3989
branch is 3 commits *behind* `main`, and one of those three is `effb09e9`
("unify feed-stdout parsing across all three entry points") — the very commit
that makes the manual path use the shared `parse_feed_items`. Rebasing 4091 onto
the #3989 tip would land this work on a tree where that unification does not
exist, so WP2's shared cycle would be built on the older `serde_json::from_slice`
glue and the conflict resolution below would be unsatisfiable without a
cherry-pick.

What was done instead: `git merge --no-ff` of the #3989 branch **into** this one.
That brings its 13 commits in while keeping `main`'s 3, and puts the conflict
resolution in this branch's history where it can be reviewed. Git merged both
files cleanly, so the two "conflicts" never surfaced as conflict markers — which
is exactly why they were verified by reading the result rather than trusted.

**Wrap-up consequence, do not let this land silently:** this branch now carries
#3989's commits, so it cannot merge to `main` before #3989 does without taking
that work with it. Say so on the PR. If #3989 lands on `main` independently
first (it is finished work — tip commit `fix(feeds): close the final-review
findings` — it just has no PR open), the shared history collapses and this
becomes a non-issue.

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

1. **`feed_command`, `feed_role` and `group_by_repo` are all read inside the
   cycle**, from one `db.get_epic(epic_id)` after the claim, instead of the
   auto-poll path using its in-hand `Epic` and the manual path mixing a fresh
   `get_epic` read for `feed_role` with `feed_command` and `group_by_repo` passed
   down from the TUI's cached board. That mixed sourcing is a live drift:
   `App::handle_trigger_epic_feed` (`src/tui/update/feeds.rs:9`) pulls all three
   of title, `feed_command` and `group_by_repo` off `self.find_epic(id)`, i.e.
   the last-loaded board snapshot. So a manual `r` today can sync with a stale
   `group_by_repo` **and can execute a stale `feed_command`** — the second is
   strictly worse (wrong command runs) and it would be incoherent to fix the
   grouping flag while leaving it. One read, one source of truth.

   Consequences: `feed_command` and `group_by_repo` drop out of
   `FeedCommand::TriggerEpic` (`src/tui/commands/feed.rs:11`) and out of
   `exec_trigger_epic_feed`'s signature — only `epic_id` and `epic_title`
   remain, the title purely for status-bar presentation. `handle_trigger_epic_feed`
   keeps reading the cached `feed_command` as the *gate* for whether `r` does
   anything at all (that is `ManualFeedTrigger`'s `requires:` clause and it is
   fine on cached data); it just stops passing the value down. An epic that lost
   its `feed_command`, or was deleted, between keypress and cycle becomes a
   `Failed` outcome instead of a flat upsert onto a missing epic.

   Cost, stated plainly: one extra `get_epic` per eligible auto-poll cycle that
   does not exist today — `FeedRunner::tick` already has the `Epic` in hand from
   its single `list_epics()`. It is a pure read, so it goes to the read pool via
   `db_call_read` and does not queue behind the writer (docs/conventions.md, "DB
   access"). One read per epic per interval is not a real cost, but it is a
   change in the access pattern and should not be waved through as free.
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

**This does NOT close the destructive-teardown bug class — only the feed-vs-feed
half of it.** The claim serialises feed cycles against each other. It does
nothing about a concurrent **non-feed** writer. `delete_stale_subtree_feed_tasks`
filters on the task's current `epic_id` and its keep-set is the emission's
`external_id`s, so any writer that moves a task's `epic_id` *into* the reconciled
subtree mid-cycle — an MCP `update_task` with `epic_id`, an epic reparent, a
`flatten_epic`, a TUI move — can land a feed task under a role sub-epic while it
is absent from the in-flight emission, and the stale delete will remove it and
tear down its worktree. Same mechanism, same consequence, one leg simply is not a
feed cycle. Nothing in this plan detects that, and the spec's new invariant is
deliberately worded "another *cycle*" rather than "another writer" so it does not
overclaim. A reader must not walk away thinking the class is closed. Whether the
stale delete should refuse to remove a task carrying live agent state is the real
fix for that half, and feeds.allium already flags it as an open question under
`DegradedEmptyEmission`'s RESIDUAL RISK paragraph — worth its own task.

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
- **The handshake MUST be deadline-bounded.** A blocking FIFO open that never
  unblocks is worse than a failing test: it wedges CI silently, and the way it
  gets wedged is precisely a regression in the code this plan adds — if
  `try_claim`, the `get_epic`, or the role guard bails before `exec`, `cat` never
  opens the FIFO for reading and the writer-side open blocks forever. So
  `tokio::time::timeout` the `JoinHandle`, and fail the test on elapse with a
  message naming the likely cause. Note honestly what that does and does not buy:
  `spawn_blocking` work is **not cancellable**, so the timeout frees the *test*,
  not the thread — the blocked thread leaks until the process exits. That is
  acceptable in a test binary and is the standard trade; it is not acceptable to
  leave the deadline off. This is why the two claim-held tests below are not
  merely "cheaper" — they are the ones that will still be diagnosable if the FIFO
  test ever goes red.
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
    /// Presentation only — the status-bar strings on the manual path and the
    /// log fields on both. Never used to decide behaviour, so a stale title is
    /// harmless; `feed_command`, `feed_role` and `group_by_repo` are NOT fields
    /// here, they are read fresh inside `run` (behaviour change 1).
    pub(crate) epic_title: String,
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

`Failed(String)` is a bare string because both consumers render it directly
(`FeedMessage::Failed` → status bar, or a `tracing::warn!`). Follow-up worth
noting, not doing here: the repo prefers typed failure vocabularies for anything
a caller branches on (see repo-sync.allium), so if a future caller ever needs to
*distinguish* these failures, promote it to an enum then rather than
string-matching.

Body, in order — this is the whole of today's two glue blocks, merged:

1. `let Some(_claim) = self.guard.try_claim(self.epic_id) else { return Busy };`
2. `get_epic` → `feed_command`, `feed_role`, `group_by_repo` (behaviour change 1;
   missing epic, or an epic whose `feed_command` is now `None` → `Failed`).
3. `exec_feed_command` with the `feed_command` from step 2 → `Err` → `Failed`.
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
  Worth writing down next to that comment, because it is not obvious: the claim
  is taken **inside the spawned job**, not synchronously in `tick()`. So `tick()`
  never blocks and never itself observes contention — whichever of the two
  spawned jobs reaches `try_claim` first wins, and the loser returns `Busy`. The
  outcome is order-dependent but the *assertion* is not, which is why the test is
  race-free either way and must not be rewritten to expect one specific arm.
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
  `epic_invalidate_tx()`. Then a new `TuiRuntime.feed_sync_guard` field, set from
  it. **Both call sites must share one guard or the fix is inert**, and there is
  no compiler check for that — a `FeedSyncGuard::default()` minted at a second
  site type-checks and silently disables the serialisation.

  The real inventory of `TuiRuntime` construction sites — verified, and *not* the
  "eight in `editor.rs`" an earlier draft of this plan claimed:

  | Site | Count | Note |
  |---|---|---|
  | `src/runtime/mod.rs:495` (`bootstrap`) | 1 | production; the only one that matters at runtime |
  | `src/runtime/tests.rs::make_runtime` | 1 | the canonical test fixture — already builds a `FeedRunner` and is what `todos.rs`, `learnings.rs` and the other `src/runtime/*` test modules delegate to |
  | `src/runtime/editor.rs` | 9 | lines 708, 837, 900, 957, 1022, 1074, 1142, 1202, 1263 — the outlier: each builds its own literal instead of using the fixture |

  So the correct shape is a `fn wire_feed(db, notify, runner) -> (FeedRunner,
  Arc<FeedSyncGuard>)` factory applied at **11** sites, of which the one that
  earns the flagship test is `tests.rs::make_runtime` — fixing that fixture gives
  every delegating test module a correctly-wired runtime for free, and is where
  the FIFO test should get its runtime from. `editor.rs`'s 9 literals are
  mechanical follow-through; none of them exercises a feed path, but they must
  take the guard from their own `FeedRunner` rather than minting one, so the
  wrong pattern is not left in the tree to be copied.

### WP4 — spec alignment and the doc sweep

- `allium:weed` over `feeds.allium` against the new code; `allium check`.
- When replacing the "Teardown-vs-notification ordering … open question"
  paragraph, keep the file's register. `feeds.allium` is unusually good at naming
  its own residual risks ("The cost, stated honestly: …"), so the replacement
  states the new normative order *and* what it costs (auto-poll board updates now
  wait on `git worktree remove`) rather than collapsing to a bare rule. Same for
  the new `SerialisedFeedCycle` rule: it must carry the non-feed-writer residual
  risk from the "does NOT close the bug class" paragraph above, or the spec will
  read as a stronger guarantee than the code gives.
- Status-bar snapshots: a new transient message should not touch
  `src/tui/tests/snapshots/`, but if any snapshot does move, accept it with
  `INSTA_UPDATE=always cargo test tui::tests::snapshots` and **delete the
  `*.snap.new` files afterwards**.
- `CLAUDE.md` needs no change (no new subsystem, no new external dependency).
- Record a learning: the per-epic single-flight claim is the third decision that
  had to be identical across the two feed paths, and the reason `src/feed/cycle.rs`
  now exists — so the next agent adds step four *there*, not twice.

### WP5 — bound `exec_feed_command` — DECIDED: not now, document instead

**Decision (user, 2026-08-13): accept the window; do not bound the exec in this
change. Document the resulting starvation in `feeds.allium` so it is specified
behaviour rather than a surprise.** The reasoning for accepting it: a feed command
that hangs forever is itself a bug, it is rare, and the alternative — stealing a
claim past some age — would reintroduce exactly the interleave this task exists
to remove.

So the obligation moves from WP5 into WP4's spec work, and it is not optional:

- `SerialisedFeedCycle`'s `@guidance` must state that the claim is held for the
  whole cycle including an **unbounded** `exec_feed_command`, so a feed command
  that never exits holds its epic's claim for the life of the process. Every
  later tick and every manual `r` for that epic is then dropped, and there is no
  in-app recovery — only a restart. Name the contrast with today explicitly (an
  unserialised manual refresh currently still works while a poll is stuck), so a
  future reader sees it as a known accepted cost and not a regression to
  "helpfully" undo by loosening the claim.
- Point at `src/feed/exec.rs::exec_feed_command`'s lack of a timeout as the
  precise reason, and record bounding it as the sanctioned fix if the window ever
  bites — noting that `run_bounded` (`src/process.rs`) is the repo's
  kill-on-timeout primitive for **synchronous** `ProcessRunner` children and so
  is not a drop-in for this `tokio::process` call site.
- File it as a follow-up task rather than leaving it only in the spec.

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
- **A hung feed command becomes an availability regression, and this needs your
  sign-off.** Today, a stuck auto-poll does not block a manual `r`: the two run
  unserialised, so the user always has a way to force a sync. Under single-flight
  the stuck cycle holds the claim, and then *every* subsequent tick and *every*
  `r` press for that epic is dropped — with no user-facing recovery short of
  restarting the app, because `exec_feed_command` has no timeout and nothing
  reaps the claim. That converts a data-corruption bug into a
  liveness/availability bug for that one epic. The status line at least says why,
  which is more than today's silent pile-up, but "for as long as it hangs" can be
  forever. **Decided: accept it, and specify it** (see WP5) — a feed command that
  hangs forever is itself a bug, and the alternative of letting a manual `r` steal
  an aged claim would reintroduce the interleave this task removes. Bounding the
  exec stays available as the sanctioned fix and is filed as a follow-up.
- **Auto-poll board updates lag teardown** (behaviour change 2). Bounded by the
  `git worktree remove` fan-out, which is already parallel across repos.
- **Merge order with #3989** (see WP0). If #3989 is revised before it lands,
  WP3's caller bodies shift. Nothing else in the plan depends on its internals.
  The live hazard is direction: rebasing onto the #3989 tip silently drops
  `main`'s parse unification, which WP2 builds on.
