# 3850 — Code-quality cleanups left over from subagent counting

Follow-up to #3755. Five findings from that branch's simplify review, deferred at
the final gate. None is a correctness issue, so **every step below must be
behaviour-preserving**: no Allium spec change is expected, and `allium:weed` at
the end must report no new divergence.

## Disposition of the five findings

| # | Finding | Disposition |
|---|---|---|
| 1 | Duplicated hook-CLI boilerplate in `cmd_hook`/`cmd_hook_subagent`/`cmd_hook_file_event` | Apply — extract a shared prologue + outcome reporter |
| 2 | Two writer round trips in `clear_subagents_no_drain` | Apply — fold `stop_pending = 0` into the clear transaction, via a **second named DB method**, not a flag |
| 3 | Unconditional `get_task` on `SubagentStart` | Apply — `TaskRead::task_exists` + an early-returning `Start` arm |
| 4 | `drains` redundant in the drain predicate | Apply, but **structurally**: the `Start` arm returns before the drain predicate exists, so the "a Start can never drain" invariant is enforced by control flow rather than encoded as a condition or silently relied upon. Step 3 and step 4 are therefore one change. |
| 5 | `ClearSubagents { drain: bool }` is boolean-blind | Apply — named `DrainMode { Drain, NoDrain }` |

Two notes on where this deviates from the gate's recorded reasoning:

- Finding 2's own suggestion of "a `clear_stop_pending` flag" is rejected: it
  would reintroduce exactly the boolean-blindness finding 5 is about, and
  `subagent_clear`'s two callers want genuinely different things.
- The gate suggested finding 2 could just clear `stop_pending` unconditionally
  inside `subagent_clear` ("both callers want it cleared"). That is **not**
  behaviour-preserving. `DetachTmux` (`docs/specs/split-pane.allium:34`) clears
  `stop_pending` only `if task.stop_pending and task.status = running`, and it
  reaches the DB through the same `subagent_clear`. Folding the clear in
  unconditionally would silently widen that rule. Hence two named methods.
- Finding 4's gate note ("removing it couples correctness to the non-obvious
  invariant *Start can never yield zero*") is correct and is the reason the fix
  here is a restructure rather than a deletion. After step 3/4 the invariant is
  not relied on at all — the `Start` path never reaches the predicate.

## Step 1 — `TaskRead::task_exists`

Two places read a whole task row and use it only to answer "does this exist":
`clear_subagents_no_drain` (`src/service/tasks/crud.rs:826`, discards the row
outright) and the `Start` arm of `record_subagent_event`.

**Test first** (`src/db/tests/` — the module that already covers task CRUD):

- `task_exists_is_true_for_a_created_task`
- `task_exists_is_false_for_an_unknown_id`
- `task_exists_is_false_after_delete`

**Then implement**: add `async fn task_exists(&self, id: TaskId) -> Result<bool>`
to `TaskRead` (`src/db/mod.rs:157`) and implement it on `Database`
(`src/db/queries/tasks.rs:140`) as a `SELECT 1 FROM tasks WHERE id = ?1` via
`db_call_read` — a pure read, so it must not queue behind the writer. `Database`
is the only implementor of `TaskRead`, so no mock needs updating.

## Step 2 — restructure `record_subagent_event` (findings 3 + 4)

Target shape in `src/service/tasks/crud.rs`:

```rust
match event {
    SubagentEvent::Start { agent_id, session_id } => {
        // Existence check, not a row read: a Start inserts before the
        // recount, so it can never drain and never needs the task's status
        // or stop_pending. Highest-frequency of the three events.
        if !self.db.task_exists(id).await? { return Err(not_found(id)); }
        self.db.subagent_start(id, &agent_id, &session_id, self.clock.now()).await?;
        Ok(())
    }
    SubagentEvent::Stop { agent_id, session_id } => {
        let task = self.require_task(id).await?;
        let live = self.db.subagent_stop(id, &agent_id, &session_id).await?;
        self.drain_if_settled(&task, live).await
    }
    SubagentEvent::Clear => {
        let task = self.require_task(id).await?;
        self.db.subagent_clear(id).await?;
        self.drain_if_settled(&task, 0).await
    }
}
```

with two small private helpers: `require_task` (the existing `get_task` +
`NotFound`) and `drain_if_settled(&task, live)` holding the unchanged predicate
`live == 0 && task.stop_pending && task.status == Running` plus the `TaskPatch`
and `recalculate_epic_for_task`. The `drains` local disappears; no `unreachable!`
is introduced.

Ordering invariants that must survive: the existence/`NotFound` check stays
**before** any mutation on every arm, and the `task` snapshot for Stop/Clear is
still read before the count-mutating call (the drain decision is deliberately
made against the pre-mutation row).

**Test first.** The existing tests at `src/service/tasks/tests.rs:2811+` already
cover the drain matrix; they must pass untouched — that is the primary evidence
of behaviour preservation. Add:

- `subagent_start_does_not_drain_a_pending_stop_even_at_count_zero` — asserts
  the structural invariant directly: a task Running with `stop_pending` and no
  entries stays Running after a `Start`.
- `subagent_start_on_unknown_task_returns_not_found` — pins the `NotFound`
  contract now that the `Start` arm no longer goes through `get_task`
  (`record_subagent_event_unknown_task_returns_not_found` at
  `src/service/tasks/tests.rs:3159` may already cover this; extend it to all
  three event variants rather than duplicating).

## Step 3 — one writer round trip for the no-drain clear (finding 2)

**Test first**, in `src/db/tests/subagents.rs` (alongside
`subagent_clear_rolls_back_the_delete_when_the_count_write_fails` at line 203):

- `subagent_clear_and_void_pending_stop_clears_entries_count_and_the_bit`
- `subagent_clear_and_void_pending_stop_leaves_status_alone` — it must not
  touch `status`/`sub_status`; only `subagent_clear`'s columns plus
  `stop_pending`.
- `subagent_clear_leaves_stop_pending_alone` — the guard that keeps the
  `DetachTmux` drain path's behaviour from drifting.
- extend the existing rollback test to the new method: a failing count write
  must roll back the delete *and* the `stop_pending` write.

**Then implement**:

- `src/db/queries/subagents.rs`: give the existing body a private
  `void_pending_stop: bool` parameter (private to this one file, so the boolean
  never reaches an API surface) and add `UPDATE tasks SET stop_pending = 0` to
  the same `unchecked_transaction` when set. Keep the module doc's explanation
  of *why* the transaction is explicit.
- `src/db/mod.rs` + `src/db/queries/tasks.rs`: expose
  `subagent_clear_and_void_pending_stop(&self, id)` next to `subagent_clear`,
  documenting which caller each is for.
- `src/service/tasks/crud.rs`: `clear_subagents_no_drain` becomes the
  `task_exists` check (step 1) plus one `subagent_clear_and_void_pending_stop`
  call — two writer round trips become one, and the read stops materialising a
  row it discards.

The three `clear_subagents_no_drain` callers (SessionStart via
`cmd_hook_subagent`, crash, dispatch-claim) are unchanged.

## Step 4 — `DrainMode` (finding 5)

**Test first**: the two existing command assertions —
`src/tui/tests/dispatch.rs:875` (`drain: false`) and
`src/tui/tests/input_handlers.rs:1801` (`drain: true`) — get updated to match on
`DrainMode::NoDrain` / `DrainMode::Drain`. They are the coverage; the change is a
compile-enforced rename, so no new test is warranted.

**Then implement**:

- Add `pub enum DrainMode { Drain, NoDrain }` in `src/tui/commands/task.rs`
  beside the command, with the doc comment explaining that exactly one clear
  point drains (`DetachTmux`) and why.
- `ClearSubagents { id: TaskId, mode: DrainMode }`.
- Update emitters `src/tui/mod.rs:1734` (`Drain`) and
  `src/tui/update/agent.rs:505` (`NoDrain`), the dispatcher
  `src/runtime/commands.rs:209`, and `exec_clear_subagents`
  (`src/runtime/tasks.rs:219`) which matches on the mode instead of an `if`.

## Step 5 — shared hook-CLI prologue (finding 1)

`cmd_hook` (`src/main.rs:304`), `cmd_hook_subagent` (`:341`) and
`cmd_hook_file_event` (`:399`) all resolve `data_dir` from the DB path and call
`init_app_log_subscriber`; the first two then open the `Database`, build
`TaskService::new_with_real_runner`, and run the same three-arm
`Ok / NotFound → "Task {id} not found, skipping" / Err` match.

**Then implement** two helpers in `src/main.rs`:

- `fn hook_data_dir(db: &Path) -> Result<&Path>` — resolve the parent and
  install the log subscriber; used by all three, including the service-free
  `cmd_hook_file_event`.
- `async fn open_hook_service(db: &Path) -> Result<service::TaskService>` — the
  above plus `Database::open` and `new_with_real_runner`.
- `fn report_hook_outcome(id: i64, outcome: Result<(), ServiceError>) -> Result<()>`
  — the three-arm match, with the "a missing task is a silent skip, because a
  hook fires from a session whose task may have been archived" rationale stated
  once instead of twice.

`cmd_update`'s superficially similar match stays put: it is a
`Result<bool, _>` with two success arms and a stringly `"not found"` test, not
the same shape.

**Tests**: `tests/cli.rs` already covers this surface end to end —
`hook_initialises_app_log_in_data_dir` (`:572`), `hook_unknown_task_skips`
(`:602`), `hook_subagent_on_missing_task_exits_zero` (`:753`). Add the missing
symmetric case so the extracted reporter is pinned from both callers rather than
one:

- `hook_subagent_initialises_app_log_in_data_dir` — mirrors `:572` for
  `hook-subagent`, and would have caught a prologue extraction that dropped the
  subscriber on one path.

## Step 6 — docs and verification

- `src/db/mod.rs` doc comments for the two clear methods and `task_exists`;
  `src/service/tasks/crud.rs` doc comment on `record_subagent_event` updated to
  describe the three-arm structure (and to say that Start's inability to drain
  is now structural).
- No `CLAUDE.md` or `docs/module-map.md` change expected — no new module, no new
  subsystem. Re-check once the code is in.
- Run `allium:weed` on `agent-health.allium` and `split-pane.allium` to confirm
  no divergence was introduced.
- Verify: `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`,
  then `cargo clippy --all-targets -- -D warnings` (the pre-push gate; a plain
  build does not fail on `unwrap`).

## Risks

- **Step 3 is the only step that touches SQL under a transaction.** The rollback
  test is the guard; do not merge it without extending that test to the new
  method.
- **Step 2 changes the read shape of the hottest hook path.** The win is small
  (each hook is already paying process startup + `Database::open`), so it is
  taken for the control-flow clarity in finding 4, not for the microseconds. If
  the restructure turns out to need an `unreachable!` or a second match on
  `event`, abandon it and keep `drains` — the gate's original judgement stands
  in that case.
