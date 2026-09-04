# Log-warning triage: the cards that need code

Task #4691, epic #319 (Log Warnings). Triage of all 106,433 WARN lines in
`app.log` assigned each of the epic's 37 cards to one of four buckets. 32 are
archive-only (no code); this plan covers the 5 that need code.

Archive-only IDs, for the record — dead message heads: 4653 4654 4656 4657 4658
4659 4662 4665 4666 4667 4671 4672 4673 4674 4677 4680 4681. Fixed since:
4652 4655. Correct warnings, keep the level: 4661 4663 4664 4668 4676 4678 4679
4682 4683 4684 4686 4687. Plus 4670, moved here mid-plan — see its
section.

## Card 4651 — PR polling never gives up

**63,000 of the 106,000 warnings, and still live.** Five tasks (2910, 3145,
3147, 3148, 3150) each logged the identical failure 693 times in the last 30
days. Root cause is permanent and outside dispatch: the `gh` account has no
access to those `annotell/*` repos (a feed script says so directly —
`warning: skipping annotell/scala-common: no account has access`).

The defect is that dispatch cannot tell a permanent failure from a transient
one, so it retries a hopeless call every 30s forever and tells the user
nothing. `PollPrStatus` (`docs/specs/pr-workflow.allium:59`) has no failure
branch at all — `check_pr_status` failing is unspecified.

### Spec changes

`core.allium` — one new `SubStatus` variant next to `pr_closed`:

    pr_unreachable  -- gh could not read the PR (repo not resolvable, bad
                    -- credentials, no PR at that url); polling for this task
                    -- has stopped. Task stays in review so a human can act.

`pr-workflow.allium` — `PollPrStatus` gains a failure branch:

- `check_pr_status` failures classify as **permanent** or **transient**.
  Permanent: repo not resolvable, bad credentials, requires authentication, no
  PR found at that url, unknown PR state. Transient: everything else —
  connection errors, TLS/read/dial timeouts, 5xx, rate limits, stream resets.
- Permanent, `permanent_failure_threshold` (3) consecutive times: set
  `sub_status = pr_unreachable` and stop polling that task for the rest of the
  session.
- Transient: exponential backoff from `pr_poll_interval` (30s), capped at
  `pr_poll_backoff_max` (30 min). Reset on any success.
- A successful poll clears `pr_unreachable` via the existing open/merged/closed
  branches. Unlike `conflict`, `pr_unreachable` is **not** guarded in the open
  branch — clearing it on success is the recovery path.

### Design decision: the give-up is in-memory, the sub_status is the marker

The consecutive-failure count and the backoff deadline live in memory beside
`agents.last_pr_poll`. The persisted `sub_status` is only what the user sees.

Consequence, and the reason for it: a dispatch restart retries a
`pr_unreachable` task once (costing ~3 polls per affected task per restart,
i.e. ~15 calls today). That is deliberate. The failure mode driving this card
is repo access, which the user fixes on GitHub, not on the task — so there is
nothing on the task to edit to trigger a retry. Without the restart path the
status would be a roach motel. The poll filter therefore keys on
`status = review and url.url_type = pr` as it does today, and must NOT exclude
`pr_unreachable`.

### Implementation

1. `src/models/tasks.rs` — `SubStatus::PrUnreachable`: `as_str`, `parse`,
   `SubStatusProperties` (label + colour), and the review-status validity
   lists at lines ~181, ~205, ~222, ~229.
2. `src/models/epics.rs` — decide whether `PrUnreachable` joins the
   `[Conflict, Crashed]` set at line ~448 and the line ~221 match. It is an
   attention state like `Conflict`, so it should.
3. `src/dispatch/mod.rs` — `check_pr_status` returns a classified error, not a
   flat `anyhow::Error`. New `PrCheckError { Permanent(String), Transient(String) }`
   or equivalent, classified from `gh`'s stderr.
4. `src/tui/update/agent.rs::tick_pr_poll` — consult the in-memory failure
   state; skip tasks that have given up, and honour the backoff deadline.
5. `src/runtime/pr.rs::exec_check_pr_status` — send a new failure message
   carrying the classification instead of logging and dropping.
6. New `PrMessage` failure variant + its `update` handler: bump the counter,
   set `pr_unreachable` at the threshold, log the give-up **once**.

## Card 4660 — `slow db_call` names the victim, not the cause

141 hits, live to 2026-08-25. The top offender is `task_exists`
(`src/db/queries/tasks.rs:224`) — `SELECT 1 FROM tasks WHERE id = ?1` — at
200–300ms, and the worst single call was 5.2s. A single-row lookup on the
primary key cannot cost that. The time is queue wait, not query cost, so the
`#[track_caller]` location in the warning points at an innocent query and is
actively misleading for diagnosis.

The spec already knows. `observability.allium`'s `DbCallSlowWarning` carries a
"DIAGNOSTIC-QUALITY CAVEAT" describing this exact ambiguity and accepting it,
noting that correlating neighbouring warn lines by timestamp is the only way to
tell contention from query cost. So this card resolves a documented limitation
rather than inventing behaviour.

Change `db_call_timed` (`src/db/mod.rs:1055`) to measure the two phases
separately — time waiting to acquire a connection versus time the closure ran —
and report both as their own fields.

The firing threshold stays on **total** elapsed time, not execute time: a call
that really took five seconds mattered to whoever waited on it, however the time
was spent. The split changes what the warning explains, not which calls it fires
for.

## Cards 4669, 4675 — a typed predicate, conditionally demoting

Attempted first as a swap to `src/tmux.rs::kill_window_if_present`, which
already handles an absent window. Reverted: that wrapper checks `has_window`
first, adding a tmux query the whole suite must then stub, and it broke three
unrelated tests. Refactoring `window_target` to return the not-found case
typed was also rejected — it has ten-plus callers.

Landed instead as `src/tmux.rs::is_window_absent_error`, a predicate beside the
code that produces the message, with the two call sites branching on it:

- `src/runtime/split.rs::exec_kill_tmux_window`
- `src/service/tasks/wrap_up.rs::kill_session_window`

No extra tmux round-trip, no test churn, genuine kill failures still at WARN.
Matching on the message text is defensible here and not for `gh`: dispatch
produces this wording itself, and `window_target_treats_a_failed_lookup_as_not_found`
pins it.

Leave `wrap_up.rs`'s `worker died` line at WARN — a dead worker is real.

## Card 4670 — do NOT demote; archive instead

Reversed on reading the spec. `observability.allium`'s
`TrajectoryWriteFailureIsSilent` deliberately warns when a trajectory entry is
lost, and argues the case explicitly. The `open` call at
`src/mcp/trajectory.rs:37` passes `.create(true)`, so `NotFound` does not mean
"no file yet" — it means the trajectories directory vanished between the
`create_dir_all` above it and the open. That is a genuinely lost entry, which is
exactly what the spec says must be reported.

12 hits on one day in May. Correct warning; archive the card. This moves the
archive-only count from 31 to 32.

## Card 4685 — demote, conditionally

`src/runtime/tasks.rs::exec_clear_subagents`, `failed to clear subagent/shell
entries: Task N not found`. One hit. The task was deleted while its hook was in
flight — clearing entries for a task that no longer exists is moot. The same
`Err` also covers a real DB failure, so only the typed
`ServiceError::NotFound` is demoted.

## Order of work

Spec first, then tests, then code, per CLAUDE.md.

1. `allium:tend` — `core.allium` (the enum variant), `pr-workflow.allium`
   (the failure branch), `observability.allium` (the two-phase timing).
2. `allium:propagate` — generate tests; confirm they fail.
3. Implement 4651, then 4660, then the two tmux call-site swaps and the one
   conditional demotion.
4. `allium:weed` — confirm spec and code agree.
5. Verify: `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`.

## Explicitly out of scope

- `base_branch` validation before dispatch (cards 4676, 4687 — `invalid
  reference: main`). 5 hits in five months; not worth a code path.
- Feed rate-limit backoff. Same shape as 4651, but the feed already degrades
  safely — it syncs additively and skips removals when a command wrote to
  stderr.
- Archiving the 31 archive-only cards. Neither MCP (`update_task`'s status enum
  omits `done`/`archived`) nor the CLI can set it; it is a TUI action.

## What landed

Spec, then tests, then code, per CLAUDE.md. All 4610 tests pass; `cargo fmt
--check`, `cargo clippy --all-targets -- -D warnings`, `check-doc-paths.sh`,
`check-doc-symbols.sh`, `check-no-test-sleep.sh` and `allium check` are clean.

**Spec** — `core.allium`: `SubStatus::pr_unreachable`, `PrCheckOutcome`,
`PrPollState`, two config params, four doc tables. `pr-workflow.allium`:
`PollPrStatus` failure branch, `PrPollGaveUp`. `observability.allium`:
`execute_ms`/`queued_ms` on `SlowDbCallWarning`, plus
`AlreadyDoneTeardownIsNotAFailure` / `TeardownFailureIsWarned` for the
demotions. `mcp-task-tools.allium`: the advertised-enum omission.

**Code** — migration v96 (the `(status, sub_status)` CHECK constraint);
`PrCheckFailure` and its classifier in `src/dispatch/mod.rs`;
`PrMessage::CheckFailed` and `handle_pr_check_failed`; poll gating in
`tick_pr_poll`; two-phase timing in `src/db/mod.rs`;
`tmux::is_window_absent_error` and its two call sites; the typed not-found
demotion in `exec_clear_subagents`.

### Two things worth knowing next time

`allium check` reports errors under `diagnostics`, not `findings`, and needs
every spec in one invocation or cross-module references fail to resolve.
Recorded as knowledge-base entry #619.

`crate::test_log::logged_during` installs a **thread-local** subscriber, so it
cannot see anything logged inside `spawn_blocking`. Two tests written against
the tmux teardown passed vacuously because of it, and were replaced with direct
tests of `is_window_absent_error`. Assert on a log only from the same thread
that writes it.
