# #4201 — Size `DISPATCH_WATCHDOG_TIMEOUT` off `provision_worktree`'s real subprocess budget

## Problem

`DISPATCH_WATCHDOG_TIMEOUT` (`src/tui/mod.rs:89`) is documented as "kept in
sync" with `SUBPROCESS_TIMEOUT` (`src/process.rs:9`) at a 1:1 ratio — both
120s. But `provision_worktree` (`src/dispatch/worktree.rs`) can legitimately
issue several `SUBPROCESS_TIMEOUT`-bounded subprocess calls **sequentially**
before a fresh dispatch succeeds or gives up, under `FetchPolicy::Required`:

1. Up to `FETCH_MAX_ATTEMPTS` (3) `git fetch origin <base>` attempts.
2. `classify_fetch_failure`'s two probes (`git remote get-url origin`, then
   `git ls-remote --exit-code origin refs/heads/<base>`), fired once after the
   first failed attempt.
3. `select_start_point`'s ahead/behind measurement (`crate::repo_sync::ahead_behind`,
   `git rev-list --count --left-right`), run once the fetch has succeeded, for
   any branch-based (non-PR-head) dispatch.
4. `git worktree add`, for a fresh (non-reused) worktree.

The task's own writeup estimated "~5×" (3 fetch attempts + 1 classify probe +
1 worktree add), but that undercounts: the actual worst case is a fetch that
only succeeds on its **last** allowed attempt, which still runs the classify
probes (triggered by the first failed attempt) *and then continues* to the
ahead/behind measurement and worktree add rather than aborting. That path is
already exercised today by `provision_worktree_retries_fetch_before_falling_back`
(`src/dispatch/tests.rs:1649`), whose `calls[6]` is the `worktree add` call —
i.e. 7 calls (indices 0–6), not 5:

```
0: git fetch origin <base>       (attempt 1, fails)
1: git remote get-url origin     (classify probe 1)
2: git ls-remote ... <base>      (classify probe 2)
3: git fetch origin <base>       (attempt 2, fails)
4: git fetch origin <base>       (attempt 3, succeeds)
5: git rev-list --count ...      (ahead/behind measurement)
6: git worktree add ...          (worktree add)
```

A network flaky enough to stall two or more of these calls trips the 120s
watchdog while `provision_worktree` is still legitimately working within its
own retry policy — producing the spurious "Dispatch timed out" popup this
task exists to close.

### Explicitly out of scope

Several more subprocess calls happen in and around `provision_worktree` that
this plan does not change:

- `detect_default_branch`, called from `dispatch_with_prompt`
  (`src/dispatch/agents.rs`) before `provision_worktree` even runs, when the
  task has no configured `base_branch` — a local `git symbolic-ref` read with
  no network involved, so its realistic worst-case contribution is negligible
  even under a fully flaky network.
- `pr_head_branch` (`gh pr view`, for PR-review tasks, called from
  `dispatch_with_prompt` before `provision_worktree`) and the three tmux calls
  inside `provision_worktree`'s own `post_add` step — `tmux::new_window`,
  `tmux::set_window_dispatch_dir`, `tmux::ensure_split_hook`
  (`src/dispatch/worktree.rs:473-480`). All four call `runner.run(...)`
  instead of `runner.run_with_timeout(...)`, so all four are genuinely
  **unbounded** in production (`src/process.rs:243-253`; confirmed no path in
  `src/tmux.rs` uses `run_with_timeout`). This does **not** mean the watchdog
  fails to cover them — `tick_dispatching`'s check is pure wall-clock elapsed
  time since the task entered `dispatching`
  (`src/tui/update/agent.rs:232-254`), so the "dispatch timed out" popup still
  fires at the deadline regardless of which subprocess is stuck. What an
  unbounded call actually risks is a `spawn_blocking` OS thread parked forever
  (a resource leak the watchdog's UI backstop does nothing to reclaim) and a
  task left stranded in `Running` with no worktree indefinitely — already an
  accepted failure mode per `DispatchingTimeout`'s own spec text, just one a
  hung `gh`/`tmux` call can trigger without ever hitting a per-call timeout.
  These four call sites share one fix shape (switch to `run_with_timeout`) and
  are worth a single follow-up task; do not fold that fix into this one.
  (Plan: file a follow-up task at wrap-up time.)

This plan fixes the `provision_worktree` **fetch/classify/measure/worktree-add**
gap only, which is what #4201 as written is about, and documents the
unbounded-call findings above as a follow-up rather than silently absorbing
them into this change.

## Design

Size the watchdog off the actual worst-case number of sequential
`SUBPROCESS_TIMEOUT`-bounded calls `provision_worktree` can issue, instead of
mirroring `SUBPROCESS_TIMEOUT` 1:1.

Add a named constant next to `FETCH_MAX_ATTEMPTS` in `src/dispatch/worktree.rs`:

```rust
/// Worst-case number of subprocess calls `provision_worktree` can issue
/// sequentially while provisioning a fresh, branch-based worktree, each
/// independently bounded by `SUBPROCESS_TIMEOUT`. Worst case is a fetch that
/// only succeeds on its last allowed attempt: `FETCH_MAX_ATTEMPTS` fetch
/// attempts, + 2 for `classify_fetch_failure`'s probes (fired once, after the
/// first failed attempt), + 1 for `select_start_point`'s ahead/behind
/// measurement, + 1 for the final `git worktree add`.
///
/// `pub(crate)` so `DISPATCH_WATCHDOG_TIMEOUT` (`src/tui/mod.rs`) can derive
/// its budget from this instead of mirroring the number by hand — see
/// `FETCH_MAX_ATTEMPTS`'s own doc comment for why mirroring is the hazard
/// this avoids (#4201).
pub(crate) const PROVISION_MAX_SUBPROCESS_CALLS: u32 = FETCH_MAX_ATTEMPTS + 4;
```

Re-export it next to the existing `pub(crate) use worktree::{ensure_dispatch_dir_and_gitignore, DISPATCH_DIR};`
line in `src/dispatch/mod.rs`.

Redefine the watchdog in `src/tui/mod.rs`:

```rust
pub(in crate::tui) const DISPATCH_WATCHDOG_TIMEOUT: Duration = Duration::from_secs(
    SUBPROCESS_TIMEOUT.as_secs() * crate::dispatch::PROVISION_MAX_SUBPROCESS_CALLS as u64,
);
```

(`Duration::as_secs`/`from_secs` are both `const fn`, so this stays a
compile-time constant like today; `src/tui/mod.rs` needs a new
`use crate::process::SUBPROCESS_TIMEOUT;` import, since nothing in that file
references it yet.) At current values this is `120s × 7 = 840s` (14 minutes)
— up from 120s.

This is a deliberate trade-off, not a side effect of "getting the arithmetic
right": raising the constant does not change worker behaviour at all (the
dispatch worker runs on a detached `tokio::task::spawn_blocking` thread the
watchdog never cancels either way — see `DispatchingTimeout`'s own "this
surface is a UI backstop and stays one"). Its only effect is *how long a
genuinely stuck worker* (a panic escaping `catch_unwind`, a truly hung
subprocess) sits silently before the user sees anything — from 2 minutes
today to 14 minutes after this change. That is the cost being paid to remove
the spurious-timeout false positive; the spec update below states it as such.

This is a value-only, structurally-minimal change: no behavioural change to
`fetch_origin`'s retry policy, no change to how `tick_dispatching` uses the
constant, no change to the `dispatch_may_be_in_flight` claim-staleness
fallback that reuses the same constant (`src/tui/mod.rs:657-666`) — that
fallback gets correspondingly more generous too, which is the same fix
applied consistently (a claim younger than the new deadline is still "alive"
whichever process made it).

### Why not a separate outer deadline for provisioning?

The alternative floated in the task description — give `provision_worktree`
its own outer deadline distinct from the per-call bound, cutting retries
short once a global budget is spent — would change `fetch_origin`'s retry
behaviour (abandoning retries mid-budget) and needs threading a deadline
through `fetch_origin`/`classify_fetch_failure`/`provision_worktree`, touching
their signatures and every existing test that scripts them via
`DispatchScript`. Deriving the constant is a smaller, lower-risk change that
directly closes the described gap without altering retry semantics, and keeps
the existing "kept in sync" relationship between the two constants (now a
documented multiple instead of a 1:1 mirror) rather than introducing a third,
independent timeout concept.

## Test plan (TDD — tests first)

1. **`src/dispatch/tests.rs`**: a new test asserting
   `PROVISION_MAX_SUBPROCESS_CALLS` matches the real worst-case step count as
   modeled by `DispatchScript`, using the exact shape
   `DispatchScript::provision().fresh_worktree().fetch_succeeds_on_attempt(FETCH_MAX_ATTEMPTS)`
   (the same shape `provision_worktree_retries_fetch_before_falling_back`
   already exercises) via `script.index_of(Step::WorktreeAdd) + 1 ==
   PROVISION_MAX_SUBPROCESS_CALLS as usize`. This fails if `fetch_origin`'s
   retry/classify logic ever grows another call without the constant being
   updated to match. Note in the test that this pins the count only up
   through `WorktreeAdd` (the last `SUBPROCESS_TIMEOUT`-bounded call in the
   sequence) — it does not, and does not need to, say anything about the
   unbounded tmux tail that follows.
2. **`src/tui/tests/dispatch.rs`** (or inline `mod tests` in `src/tui/mod.rs`,
   whichever the existing constants' tests use): a test asserting
   `DISPATCH_WATCHDOG_TIMEOUT == SUBPROCESS_TIMEOUT * PROVISION_MAX_SUBPROCESS_CALLS`,
   and a regression guard that `DISPATCH_WATCHDOG_TIMEOUT > SUBPROCESS_TIMEOUT`
   (the exact bug being fixed — the old code satisfied `==`, which was the
   defect).

Both tests are written first and confirmed to fail against the current 1:1
constant before implementing the change.

## Documentation updates

- `docs/specs/dispatch.allium`: `DispatchingTimeout` guarantee (~line 1134) —
  replace the literal "120 seconds" with a description of the derived budget
  (`SUBPROCESS_TIMEOUT × PROVISION_MAX_SUBPROCESS_CALLS`) and why it's sized
  that way (matching `fetch_origin`'s retry budget, not a 1:1 mirror). Note
  the `pr_head_branch` unbounded-call finding as a known gap this guarantee
  does not cover.
- `docs/reference.md` "Timing Constants" — update the "Dispatch watchdog
  (120s)" line to state the new value and the derivation.
- `src/process.rs:7-9` and `src/tui/mod.rs:86-89` doc comments — replace
  "both kept in sync at 120s" with the new relationship.
- Run `allium:tend`/`allium:weed` after the spec edit to confirm alignment.

## Follow-up (not part of this task)

File a new task: four subprocess call sites reachable from a dispatch use
`runner.run(...)` instead of `runner.run_with_timeout(...)`, so they are
genuinely unbounded in production — `pr_head_branch`'s `gh pr view`
(`src/dispatch/mod.rs:123`, called before `provision_worktree` for PR-review
tasks) and the three tmux calls inside `provision_worktree`'s own `post_add`
step, `tmux::new_window`/`tmux::set_window_dispatch_dir`/`tmux::ensure_split_hook`
(`src/dispatch/worktree.rs:473-480`). None of these hangs defeats the
watchdog itself (its check is wall-clock elapsed time, not per-subprocess),
but each risks a `spawn_blocking` thread parked forever and a task stranded
in `Running` with no worktree, independent of anything this task changes. One
task, one fix shape: give all four the same `run_with_timeout` treatment
already applied to `git fetch`/`git worktree add`.
