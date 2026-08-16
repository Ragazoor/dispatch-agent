# 3848 — the "load-bearing ordering inside `handle_tick`"

## What the task assumed

Task #3848 (follow-up from #3755's final review) states that
`tick_window_checks` must run before `tick_stranded_pending_stop` in
`handle_tick` (`src/tui/update/agent.rs`), because the crash handler clears
`stop_pending` in memory and running it first stops a crashed-and-stranded task
producing both a Crashed patch and a Review flip in the same tick.

## What is actually true

The ordering is **not** load-bearing. Two facts settle it:

- `tick_window_checks` takes `&self`. It mutates nothing; it only collects
  windowed task ids into a single `BatchCheckWindows` command.
- The runtime executes that command as `drop(rt.exec_batch_check_windows(...))`
  (`src/runtime/commands.rs`) — a fire-and-forget `spawn_blocking`. The
  resulting `WindowGone` message reaches `handle_window_gone` on a later
  iteration of the event loop, never inside the tick that requested the check.

So no tick can both detect a crash and reconcile the same task, and swapping
the two calls changes only the position of one command in the returned `Vec`.
There is no dependency to pin, and pinning a false one would mislead the next
reader.

## What *is* load-bearing and was undocumented

`handle_agent_crashed` sets `task.stop_pending = false` on the in-memory task
(`src/tui/update/agent.rs`). That line reads as bookkeeping next to
`live_subagents = 0`, but it is the guard: after a crash the task still has
`status = Running` and `live_subagents = 0`, which are two of
`tick_stranded_pending_stop`'s three predicates. Leave `stop_pending` set and
the *next* tick emits `ApplyPendingStop` for a task the crash handler just
marked Crashed — the Crashed-and-in-Review state
`docs/specs/agent-health.allium` forbids.

The DB half of the same clear is `clear_subagents_no_drain`
(`src/service/tasks/crud.rs`), reached via the `ClearSubagents { drain: false }`
command; that one was already documented, this one was not.

Verified by removing the line: the new test fails, and passes again once it is
restored.

## Changes

1. **Test** — `a_crash_suppresses_the_reconciler_instead_of_racing_it_into_review`
   in `src/tui/tests/dispatch.rs`, alongside the existing
   `ReconcileStrandedPendingStop` tests. Crashes a Running task that carries a
   deferred Stop and one live subagent, then ticks and asserts no
   `ApplyPendingStop` is emitted. Fails if the in-memory clear is removed.
2. **Comment at the real dependency** — on `task.stop_pending = false` in
   `handle_agent_crashed`, naming the reconciler, the spec rule, the DB half,
   and the test.
3. **Comment on `handle_tick`** — records that the sub-step order is *not*
   load-bearing and why, so the false dependency is not re-derived and written
   down by a future reader.
4. **Spec** — a guidance addition to `DetectCrashedAgent` in
   `docs/specs/agent-health.allium` tying the `stop_pending = false` ensure to
   `ReconcileStrandedPendingStop`, and noting that rule-evaluation order
   protects nothing here because the window check is asynchronous.

No behaviour changes.
