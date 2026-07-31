# Plan: card indicator for Running-without-worktree tasks (#3822)

## Problem

A task that is `Running` (or `Review`) with `worktree = NULL` and `tmux_window = NULL`
renders as an ordinary **running** card — indistinguishable from a healthy live agent.
Nothing ever re-classifies it:

- `Task::is_detached()` (`src/models/tasks.rs:322`) requires `worktree.is_some()`, so
  `classify_card_indicator` (`src/tui/ui/kanban/cards.rs:68`) falls through to the plain
  `status == Running` arm.
- `tick_sub_status` / `tick_window_checks` (`src/tui/update/agent.rs`) both filter on
  `tmux_window.is_some()`, so it never becomes `Stale` or `Crashed`.
- Because it is never Stale/Crashed, the kill-and-retry dialog is unreachable:
  `Space` only offers it for a windowless task with `sub_status` Stale or Crashed
  (`src/tui/input.rs:318-345`). The user gets the status hint
  "No worktree to resume, move to Backlog and re-dispatch".

Reachable via a manual `L` (forward) move out of Backlog, via a crash mid-dispatch, and —
since the atomic claim writes `Running` before provisioning — via a dispatch worker that
dies without reporting.

## Design

### 1. New derived predicate: `is_unprovisioned`

`status in {running, review} and worktree = null and tmux_window = null`.

Deliberately requires *both* to be null, mirroring the description of the defect. It is
the exact complement of `is_detached` (which requires a worktree), so the two are
mutually exclusive and the classifier ordering between them is not load-bearing.

### 2. New `CardIndicator::Unprovisioned`

Rendered as `⚠ no worktree` in `Color::Red` (matching `Conflict` / `Crashed`, the other
"this needs a human" indicators).

**Priority: directly below `Dispatching`, above everything else.** Rationale — every
other indicator below it describes a state that presupposes a provisioned worktree
(`Conflict` = a rebase in the worktree, `Detached` = worktree present by definition,
`Crashed`/`Stale` = window-activity classifications, `ReviewPr` = a review task, which
in a healthy flow always retains its worktree until it reaches Done). So no *reachable*
healthy state loses its current indicator.

`Dispatching` staying on top is the load-bearing part: a task mid-dispatch is *also*
Running-without-worktree once the claim lands, and it must keep showing `dispatching…`
rather than flipping to "broken" for the duration of every in-flight dispatch.

### 3. Relax the `Dispatching` debug assert

`classify_card_indicator` currently `debug_assert_eq!`s that anything in `app.dispatching`
is `Backlog`. That predates the atomic claim: the claim writes `Running` before
provisioning, so a `RefreshFromDb` during an in-flight dispatch legitimately yields
`Running` + no worktree while the ID is still in `dispatching`. Widen the assert to
"Backlog, or Running-without-worktree (claimed, not yet provisioned)".

### 4. Make recovery reachable in place (deliberate keybinding change)

Widen `Space`'s kill-and-retry branch (`src/tui/input.rs:337-359`) so a **Running** task
with no worktree offers the retry dialog regardless of `sub_status`, instead of the
dead-end status hint. Scoped to `Running` only — `RetryFresh` already accepts any
`Running` task (`src/tui/update/retry.rs:70`) and rejects everything else, so widening to
Review/Done would open a dialog that silently no-ops. Review/Done without a worktree keep
the existing status hint.

### 5. Stale `m`-key references

`m` opens the move-to-epic tree picker; forward/backward movement is `L`/`H`. Two places
still claim `m` moves a task forward:

- `docs/specs/dispatch.allium:943` (the `RunningTaskHasWorktree` invariant)
- `src/tui/input.rs:387` (doc comment on `handle_key_move`)

Both corrected as part of this change.

## Spec changes (first)

`docs/specs/core.allium`
- Add the `is_unprovisioned` derived alongside `is_detached` on entity `Task`.

`docs/specs/dispatch.allium`
- New surface `UnprovisionedIndicator` with guarantees:
  - `ShownForWindowlessRunning` — running/review + no worktree + no window renders the
    distinct warning indicator, not the ordinary running one.
  - `DispatchingOutranksIt` — while the task is in the dispatching set it renders
    `dispatching…`; the unprovisioned indicator only appears once dispatch feedback ends.
  - `NotShownWhenProvisioned` — a running task with a worktree is unaffected, whether or
    not it still has a window (that is `detached`).
  - `RetryReachableInPlace` — the go-to-session key offers kill-and-retry for a
    windowless running task regardless of sub_status.
- `DispatchingTimeout` guarantee: record that the watchdog only clears the *feedback*,
  and that a claimed-but-unprovisioned row is thereafter visible via
  `UnprovisionedIndicator` rather than releasing the claim.
- `RunningTaskHasWorktree` invariant: replace "(m key)" with the correct `L` key, and
  point at the new indicator as the surfacing mechanism.

## Tests (before code)

`src/tui/ui/kanban/cards.rs` (inline `mod tests`) — classifier unit tests:
1. `running_without_worktree_classifies_unprovisioned`
2. `review_without_worktree_classifies_unprovisioned`
3. `dispatching_outranks_unprovisioned` — task in `app.dispatching`, Running, no worktree
   → `Dispatching` (guards `@guarantee DispatchingOutranksIt`)
4. `running_with_worktree_no_window_still_detached` — regression guard on §2's ordering
5. `running_with_worktree_and_window_still_running`

`src/tui/tests/dispatch.rs` — render-level:
6. `unprovisioned_running_card_shows_no_worktree` — `render_to_buffer`, asserts the card
   shows `⚠ no worktree` and not `▶ running`.

`src/tui/tests/input_handlers.rs` — keybinding:
7. `space_on_windowless_running_task_opens_retry_dialog` — `sub_status = Active`,
   no worktree, no window → `InputMode::ConfirmRetry`.
8. `space_on_windowless_review_task_still_shows_hint` — unchanged behaviour.

Snapshot: add the unprovisioned card to no existing snapshot fixture (avoids churn on
unrelated diffs); the render-level test above covers the visual output.

## Implementation

1. `src/models/tasks.rs` — `Task::is_unprovisioned()`.
2. `src/tui/ui/kanban/cards.rs` — `CardIndicator::Unprovisioned` variant, classifier arm
   below `Dispatching`, render arm, widened debug assert, updated priority doc comment.
3. `src/tui/input.rs` — widen the retry branch; fix the `handle_key_move` doc comment.

## Deviations found during implementation

- **The TUI test fixture was itself unprovisioned.** `make_task` left `worktree` and
  `tmux_window` null for every status, so the standard `make_app` Running task started
  rendering `⚠ no worktree` and 23 tests (mostly snapshots) failed. `make_task` now
  provisions Running/Review tasks — which is what a dispatched task actually looks like.
  Tests that genuinely want a detached or unprovisioned task clear the fields explicitly.
  Four `flat_view_*` snapshots legitimately gain a `[Space] session` hint as a result.
- **Action hints needed the same widening.** With `Space` now offering retry, an
  unprovisioned Running card showed no `[Space]` hint at all — the recovery was reachable
  but invisible. `action_hints` (`src/tui/ui/kanban/mod.rs`) now emits `[Space] retry`
  for that case, and `RetryReachableInPlace` records it.
- **`docs/reference.md`** keybinding table updated for the new `Space` behaviour.

### From the `allium:weed` spec-alignment pass

- **`DispatchingOutranksIt` had to govern the key, not just the label.** The first cut
  guarded only `classify_card_indicator`. A task mid-dispatch is unprovisioned by
  construction, so its card correctly showed `dispatching…` while the hint bar advertised
  `[Space] retry` and pressing Space ran `RetryFresh` — returning the row to Backlog and
  firing a *second* `DispatchAgent` beside the one still in flight. `is_problematic`
  (`src/tui/input.rs`) and `action_hints` are now both gated on the dispatching set, and
  Space reports "Dispatch in progress…" instead.
- **`action_hints` gained a `dispatch_in_flight` parameter**, replacing the dead
  `_selected_column` one it never read.
- **The retry dialog was mislabelled for the new entry path.** It announced
  `Agent stale - [r] Resume  [f] Fresh start` for a task that is neither stale nor
  crashed, and `[r]` dead-ended on "Cannot resume: task has no worktree" — precisely the
  message this change set out to remove. It now reads
  `Agent never started - [f] Fresh start`.
- **Spec wording:** `is_unprovisioned` is *mutually exclusive with* `is_detached`, not its
  complement (a conflict-flagged task with a worktree and no window is neither); the
  `UnprovisionedIndicator` surface takes a `task` context rather than reading
  `selected_task`, since it is evaluated per rendered card; the unprovisioned-over-conflict
  ordering is now stated; `DispatchingTimeout` says error popup, not toast.

### From the `/simplify` pass

The important one: **the in-flight guard was keyed on TUI-process-local state and missed
the case it was written for.** `App.dispatching` only holds dispatches the board itself
started. The epic auto-dispatch chain claims its next subtask inside `auto_dispatch_next`
(`src/mcp/handlers/tasks/dispatch.rs`) and never enters that map, and a board restart
mid-dispatch empties it. Either way a genuinely-provisioning task rendered `⚠ no worktree`
for its whole provisioning window and was one `Space` away from a duplicate agent.

Fixed by keying off the row instead: `App::dispatch_may_be_in_flight` ORs the in-flight set
with the claim's own `last_pre_tool_use_at` stamp, treating a claim younger than
`DISPATCH_WATCHDOG_TIMEOUT` as alive — the same 60s line `DispatchingTimeout` already draws
between "slow" and "dead". Works across processes and across restarts. A row with no stamp
counts as not-in-flight, so it surfaces immediately rather than hiding for a minute.

Also applied: `make_unprovisioned_task` in `tests/helpers.rs` (replacing a local fixture and
six copy-paste field clears); `app.is_dispatching()` in place of raw map access; a duplicated
doc-comment header on `action_hints` removed; the retry-dialog guard hoisted to an early
return instead of a diverging `let` arm; the widened `debug_assert`'s message realigned with
what it actually checks.

Skipped deliberately: unifying the `Space` routing ladder that `handle_key_activate` and
`action_hints` each encode separately — real duplication, but the refactor spans the
Backlog/Review/Done arms and the split-pane priority branch this change doesn't touch.
Also skipped promoting "unprovisioned" to a real `SubStatus` via `tick_sub_status`; the
row-based freshness gate gets the same cross-process correctness without new DB semantics.

## Verification

`cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`, plus
`cargo clippy --all-targets -- -D warnings` (pre-push).
