# 3852 — Make `UserPromptSubmit`'s `stop_pending` clear conditional

## Problem

`record_hook_event`'s `UserPromptSubmit` arm (`src/service/tasks/crud.rs`) builds a
`TaskPatch` from a `get_task` snapshot and force-sets `stop_pending(false)`.
Every Claude Code hook is a separate `dispatch` process, so this can void a
`Stop` that a *concurrent* process legitimately deferred:

1. `t0` — the human submits a prompt; the `UserPromptSubmit` hook process starts.
2. `t1` — the agent's turn ends with background subagents still live.
3. `t2` — the `Stop` hook process commits `stop_pending = 1` (deferred flip).
4. `t3` — the slow `UserPromptSubmit` write lands and clears the bit.

The deferred Stop is gone. `apply_pending_stop_if_drained` is gated on
`stop_pending = 1`, so the eventual drain never fires and the task stays
`Running` forever with no agent left to move it.

This is the mirror image of the stranded triple #3849 fixed: the bit ends up
`false`, not `true`, so the retired `ReconcileStrandedPendingStop` reconciler
never caught it either.

## Why the obvious fixes don't work

**A generation/turn counter does not fix it.** A counter incremented by
`UserPromptSubmit` and read by `Stop`'s defer branch derives its ordering from
*write* order, and a late `UserPromptSubmit` write is exactly the failure. At
`t2` the Stop reads the pre-increment counter, so its recorded turn looks stale
and the `t3` clear voids it anyway.

**`live_subagents = 0` as the predicate is not right either.** #3849 established
that `stop_pending = 1` implies `live_subagents > 0` (the stranded triple is
unreachable), so that predicate would make the clear unreachable — equivalent to
deleting it. A stale previous-turn bit would then survive into the new turn and
its drain would flip the task to `Review` mid-turn, which
`HookUserPromptSubmit`'s guidance explicitly rejects.

**Folding the write into a transaction is not sufficient on its own.** The
snapshot only feeds the status gate; the void is unconditional regardless. See
learning #355 — a multi-statement closure still needs an explicit
`unchecked_transaction()`, but atomicity alone does not decide *whether* to
clear.

## Design: compare event times, not write order

Each hook knows when *its own event* fired, independent of when its write lands.

- `Stop`'s defer branch stamps `tasks.stop_pending_at = <stop event time>`.
- `UserPromptSubmit` clears `stop_pending` only where
  `stop_pending_at IS NULL OR stop_pending_at < <prompt event time>`.

| Scenario | Times | Outcome |
|---|---|---|
| Previous turn deferred, human resumes | `t_stop < t_prompt` | voided — `HookUserPromptSubmit`'s stated intent |
| Same turn's Stop races a slow UPS write | `t_stop > t_prompt` | preserved — the drain still fires |
| Row predating the column | `NULL` | voided — preserves legacy behaviour |

Strict `<` means a tie preserves the bit. That is the safer tie-break: a
wrongly-preserved bit self-corrects at the next drain or prompt, a wrongly-voided
one strands the task `Running` forever.

Decisions taken with the user:

- `stop_pending_at` is stored at **millisecond** precision
  (`%Y-%m-%d %H:%M:%S%.3f`), not via `format_datetime`'s second granularity —
  sub-second ordering is the entire point. The column is only ever compared
  against a value formatted the same way, so lexicographic TEXT comparison is
  correct.
- `stop_pending_at` is a **DB-only column**: not in `TASK_COLUMNS`,
  `row_to_task`, or `TaskPatch`. Nothing outside the two SQL statements needs
  it, and keeping it off the model avoids churn in every `Task` fixture. It is
  meaningful only when `stop_pending = 1`; clear points do not need to null it,
  because every reader gates on the bit.

## Steps

Spec first, then tests, then code (per `CLAUDE.md`).

### 1. Spec (`docs/specs/`)

- `core.allium` — add `stop_pending_at: timestamp?` to `Task`, documenting that
  it is only meaningful while `stop_pending` holds.
- `agent-health.allium`:
  - `HookStop` defer branch: `ensures task.stop_pending_at = now` alongside
    `task.stop_pending = true`.
  - `HookUserPromptSubmit`: replace the unconditional
    `ensures: task.stop_pending = false` with a conditional on
    `task.stop_pending_at = null or task.stop_pending_at < now`, and extend the
    guidance with the ordering argument above (including why a turn counter
    fails).
  - The "stranded" invariant guidance lists `HookUserPromptSubmit` among the
    writers that "only ever set it to false" — still true (it either clears or
    leaves it), but reword so the conditional is visible.
- Run `allium check` / `allium:weed` at the end.

### 2. Tests (fail first)

`src/db/tests/subagents.rs`:

- `record_stop_stamps_the_defer_time` — deferring stamps `stop_pending_at`
  (read via `db_call` raw SQL, since the column is DB-only).
- `user_prompt_submit_voids_a_stop_deferred_before_the_prompt` — defer at `T`,
  then `record_user_prompt_submit(id, T + 1s)` → bit cleared, status Running.
- `user_prompt_submit_preserves_a_stop_deferred_after_the_prompt` — defer at
  `T`, then `record_user_prompt_submit(id, T - 1s)` → bit **kept**, status
  Running (the late-write race).
- `user_prompt_submit_voids_a_legacy_pending_stop_with_no_defer_time` — plant
  `stop_pending = 1, stop_pending_at = NULL` via `db_call` → cleared.
- `user_prompt_submit_resumes_a_review_task` / `..._refreshes_a_running_task` /
  `..._is_a_noop_outside_running_or_review` — the outcome enum, exercising the
  status/sub_status/`last_pre_tool_use_at` writes at DB level.

`src/service/tasks/tests.rs`:

- Keep `user_prompt_submit_voids_a_pending_stop` (real clock: the Stop's
  `clock.now()` precedes the UPS one, so it still voids).
- Add a `FixedClock`-driven pair asserting the preserve case end to end:
  inject a clock, defer a Stop at `T`, then run `UserPromptSubmit` with the
  clock reading `T - 1s`, assert the bit survives and the subsequent
  `SubagentStop` drain still flips to `Review`.
- Existing epic-recalculation coverage for the Review → Running transition must
  keep passing through the new code path.

`src/db/tests/migrations.rs`:

- v83 adds the column (`pragma_table_info`), and is a no-op on a synthetic
  partial `tasks` table lacking `stop_pending`.

### 3. Code

- `src/db/migrations.rs` — `migrate_v83_add_stop_pending_at`, registered as
  `(83, …)`. Pure `ALTER TABLE tasks ADD COLUMN stop_pending_at TEXT` behind a
  `column_exists` guard. No backfill: `NULL` already means "voidable", which is
  the pre-existing behaviour.
- `src/db/queries/mod.rs` — a `format_datetime_millis` helper (or a local const
  format string in `queries/tasks.rs` if nothing else wants it).
- `src/db/mod.rs` — `try_record_stop(&self, id, now)` gains the event time; new
  `record_user_prompt_submit(&self, id, now) -> Result<UserPromptOutcome>` on the
  same trait, with the `{ Resumed, Refreshed, NoOp }` enum next to `StopOutcome`.
- `src/db/queries/tasks.rs`:
  - `try_record_stop` — defer branch also sets
    `stop_pending_at = <millis-formatted now>`.
  - `record_user_prompt_submit` — one `unchecked_transaction()` holding:
    1. `UPDATE … SET status = running, sub_status = default, last_pre_tool_use_at = ?now WHERE id = ? AND status = 'review'` → 1 row ⇒ `Resumed`.
    2. if 0 rows: the same minus `status`, `WHERE … status = 'running'` → 1 row ⇒ `Refreshed`.
    3. on either hit: `UPDATE … SET stop_pending = 0 WHERE id = ? AND stop_pending = 1 AND (stop_pending_at IS NULL OR stop_pending_at < ?now)`.
    Two statements rather than one so the `Resumed` / `Refreshed` distinction
    comes from `WHERE status = …` rowcounts, avoiding a read-then-write for
    `was_review`. The first statement is a write, so the deferred transaction
    takes the write lock immediately — same argument as `try_record_stop`.
- `src/service/tasks/crud.rs` — route `HookEventKind::UserPromptSubmit` before
  the patch match, next to `Stop`: call `record_user_prompt_submit(id,
  self.clock.now())` and recalculate the epic only on `Resumed`. Drop the UPS
  arm and the `was_review` snapshot read from the patch match.

### 4. Docs / verification

- `docs/conventions.md` sub-status-TOCTOU section: add the event-time-vs-write-order
  argument as the third instance of the conditional-write pattern.
- `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`, plus
  `cargo clippy --all-targets -- -D warnings` (pre-push gate).
- `record_learning`: "a generation counter cannot order two racing hook
  processes' writes — stamp the *event* time and compare against it".

## Risk

The main behaviour change beyond the fix is that a `Stop` and a
`UserPromptSubmit` whose event times fall in the same millisecond now preserve
the bit instead of voiding it. Reachable only from a machine-fast turnaround, and
the milder failure by construction (self-corrects at the next drain or prompt).
