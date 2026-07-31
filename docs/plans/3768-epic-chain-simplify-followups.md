# 3768 — Follow-ups from the epic-chain simplify pass

Five refactors deferred from #3744. Items 1–4 are implemented in this worktree;
item 5 is a behaviour change with a much wider blast radius and ships as a
follow-up task (see §5).

Order matters: item 1 changes the claim's shape, item 4 introduces the service
call that item 2's callers and the TUI finish path both lean on, so 1 → 4 → 2 → 3.
Each item is TDD-first: spec (where domain behaviour moves) → tests → code.

---

## 1. Replace the claim retry loop with one atomic select-and-claim

**Chosen shape** (of the three proposed): the single statement. It removes the
loop, the constant and the spurious `Ok(None)` outright rather than making them
smaller, and it lets `next_backlog_task` — now only reachable from the claim —
disappear with it.

### Spec first

`docs/specs/epics.allium`, `AutoDispatchNextSubtask` guidance. Two edits:

- Delete the bounded-retry sentences ("Re-selection after a lost claim is
  bounded (currently 5 attempts) … exhausting the budget just stops the chain,
  which is one of the normal outcomes above.") and the "a caller that loses the
  claim re-selects" clause. Replace with: selection and claim are **one**
  conditional write — the ordering predicate lives inside the statement, so
  there is no select-then-claim window to lose and no re-selection.
- The "Three ways the chain stops" list stays as-is and becomes exhaustive
  again: a contended claim is no longer a fourth, undocumented way to stop.

Leave the `ensures` block alone — `first_by_order(epic.subtasks where status =
backlog)` already describes the new statement exactly.

Run `allium check docs/specs/epics.allium` after editing; `allium:weed` on the
epics spec at the end of the item.

### Tests

DB layer, `src/db/tests/tasks.rs` — new `try_claim_next_backlog_task` block,
absorbing the ordering coverage the two deleted `next_backlog_task_*` service
tests carried:

- `…claims_the_lowest_sort_order_subtask` — three backlog subtasks with
  sort_orders 30/10/20; returns the 10 one and leaves the others in Backlog.
- `…falls_back_to_id_when_sort_order_is_null` — mixed null/non-null sort_order,
  asserting the `COALESCE(sort_order, id)` key and the id tiebreaker match the
  old Rust `(sort_order.unwrap_or(id), id)` ordering.
- `…skips_non_backlog_subtasks` — running/review/done subtasks are invisible.
- `…returns_none_for_an_epic_with_no_backlog_subtask`.
- `…ignores_tasks_of_other_epics` — the `epic_id` predicate.
- `…applies_running_sub_status_and_activity_stamp` — the claimed row carries
  `status = running`, `sub_status = default_for(running)`, a non-null
  `last_pre_tool_use_at`, and a bumped `updated_at`.
- `…claims_each_subtask_at_most_once` — two sequential calls on a two-subtask
  epic return two different ids; a third returns `None`.

Service layer, `src/service/tasks/tests.rs`:

- Keep `claim_next_backlog_task_marks_the_claimed_task_running`,
  `…returns_none_when_no_backlog_remains`, `…is_exclusive_under_concurrency`,
  `…epic_not_found` unchanged — they are the behavioural contract and must pass
  untouched across the rewrite. `…is_exclusive_under_concurrency` is the one
  that proves the new statement still buys exclusivity.
- Delete the three `next_backlog_task_*` tests along with the method. The
  epic-not-found case is already covered by
  `claim_next_backlog_task_epic_not_found`.

### Code

`src/db/mod.rs` + `src/db/queries/tasks.rs`:

- Replace `try_claim_backlog_task(id, now) -> bool` with
  `try_claim_next_backlog_task(epic_id, now) -> Option<TaskId>`:

  ```sql
  UPDATE tasks
     SET status = ?1, sub_status = ?2, last_pre_tool_use_at = ?3,
         updated_at = datetime('now')
   WHERE id = (SELECT id FROM tasks
                WHERE epic_id = ?4 AND status = ?5
                ORDER BY COALESCE(sort_order, id), id
                LIMIT 1)
  RETURNING id
  ```

  Executed with `query_row`/`optional()`; `None` means no backlog subtask
  remained. Statuses come from `TaskStatus::as_str()` / `SubStatus::default_for`
  as the old statement did, not string literals.
- Delete `try_claim_backlog_task` and its three DB tests
  (`…claims_a_backlog_task_once`, `…is_false_for_task_out_of_backlog`,
  `…is_false_for_missing_task`). They restate `update_status_if_matching` /
  `_not_matching` / `_nonexistent` verbatim, and the new coverage above
  supersedes the first. `try_release_backlog_claim` stays — the release is still
  by-id.

`src/service/tasks/crud.rs`:

- Delete `CLAIM_MAX_ATTEMPTS` and `next_backlog_task`.
- `claim_next_backlog_task` becomes: `get_epic` (kept solely for the `NotFound`
  contract, which a test pins) → `try_claim_next_backlog_task` → on `Some(id)`,
  `recalculate_epic` then re-read via `get_task`; on `None`, `Ok(None)`. No
  loop, no `warn!`. Update the doc comment: exclusivity now comes from the
  statement, not from re-selection.
- The trait doc in `src/service/api.rs` for `claim_next_backlog_task` mentions
  nothing about retries and needs no change; re-read it to confirm.

`docs/mcp.md:43` says "a lost claim … is logged at `warn`". A claim can no
longer be lost — only fail with a DB error. Reword to "a claim error".

---

## 4. Service-level `close_session` (+ the TUI finish ordering it exposes)

Ahead of item 2 because item 2's `handle_dispatch_task` neighbours this code and
the TUI finish fix depends on the new call.

### Why

`handle_exit_session` treats `update_task(...) == Ok` as "the terminal write
landed". That is true only because the close patch happens to set just
`status`/`url`/`tmux_window`, so `update_task`'s fallible follow-ups
(`set_task_epic_id`, `reroute_on_repo_change`) are unreachable. Add `epic_id` or
`repo_path` to the patch later and `Err` starts meaning "patch landed, follow-up
failed" — the handler then leaves a live window on a task that IS done, exactly
the disagreement `ExitSession`'s `close_persisted` gate exists to prevent.

### Spec

`docs/specs/pr-workflow.allium`:

- `ExitSession` already models this correctly via
  `close_persisted = terminal_close_persisted(task)`; no rule change. Add one
  guidance sentence recording that the close is a single purpose-built
  persistence call whose `Result` *is* `close_persisted`, so no unrelated
  follow-up step can make the two disagree.
- `FinishTaskSuccess`: its guidance currently says the implementation "rebases …
  then fast-forwards … then kills the tmux window", i.e. the window dies before
  Done is persisted — the opposite of the rule #3744 introduced for the MCP
  path. Reorder the guidance to: rebase, fast-forward, persist Done + clear
  `tmux_window`, **then** kill the window, and state that the teardown is gated
  on that write landing. The `ensures` block already asserts the end state and
  does not change.

### Tests

`src/service/tasks/tests.rs`:

- `close_session_done_moves_task_to_done_and_clears_window` — returns the window
  it cleared; task is Done with the default Done sub_status, `tmux_window` null,
  worktree untouched, `sort_order` set by the Done-transition recency rank.
- `close_session_pr_moves_task_to_review_and_records_url` — Review + `TaskUrl`
  of type `Pr`, window cleared.
- `close_session_missing_task_is_not_found` — `ServiceError::NotFound`, nothing
  written.
- `close_session_recalculates_the_parent_epic` — an epic whose only subtask
  closes reaches its recalculated status.

`src/mcp/handlers/tests/tasks/wrap_up.rs` / `…/dispatch.rs`: the existing
`ChainFixture::with_failing_close` coverage keeps working, but
`FailingUpdateTaskService` must now fail `close_session` rather than
`update_task` — the failing-close branch is what pins "no teardown, no chain, no
Done". Rename it `FailingCloseTaskService` for accuracy.

TUI finish, `src/tui/tests/` (scenario) + `src/runtime/tests.rs`:

- `finish_kills_the_window_only_after_done_is_persisted` — assert no
  `tmux kill-window` is recorded on the mock runner before the Done write, and
  one after.
- `finish_leaves_the_window_alive_when_the_done_write_fails` — a failing
  `close_session` leaves the window and the task's `tmux_window` intact,
  mirroring `exit_session`'s failed-close path.

### Code

`src/service/tasks/crud.rs` + `src/service/api.rs` (one macro list, so the trait,
the stub bridge and the mocks all follow from a single entry):

```rust
pub enum CloseSessionOutcome { Done, Review { pr_url: TaskUrl } }
pub struct ClosedSession { pub window: Option<String> }

async fn close_session(&self, task_id: TaskId, outcome: CloseSessionOutcome)
    -> Result<ClosedSession, ServiceError>;
```

Implementation: read the task (`NotFound` if absent), remember its
`tmux_window`, build the one terminal patch (status + sub_status + optional url +
cleared `tmux_window`, plus the Done-transition `sort_order`), `patch_task`, then
`recalculate_epic_for_task`. Only the patch is fallible, so `Err` means exactly
"the terminal write did not land".

`src/mcp/handlers/tasks/wrap_up.rs`: `handle_exit_session` maps
`(action, pr_url)` to a `CloseSessionOutcome` and calls `close_session`. The
window it tears down comes from the returned `ClosedSession`, not from the
pre-read task. Everything downstream of `close_result` is unchanged.

TUI finish ordering:

- `src/dispatch/finish.rs`: drop `tmux_window` from `FinishContext` and delete
  step 6. `finish_task` becomes git-only, which is what the MCP caller already
  wanted (it passes `None`). Its tests lose the `tmux_window` argument; the
  `has_window_runner_error` test loses its reason to exist and goes.
- `src/runtime/tasks.rs::exec_finish`: on `Ok(())`, emit `FinishComplete` as
  today. On the shared-worktree fast path, unchanged.
- `src/tui/update/wrap_up.rs::handle_finish_complete`: keep the optimistic
  in-memory update, but replace the `Persist` command with
  `TaskCommand::CloseSession(Task)` — it carries the task, like `Persist`, so the
  runtime can splice the service-computed `sort_order` back into the board.
- New `exec_close_session` in `src/runtime/tasks.rs`: `close_session(id, Done)`,
  and **only on `Ok`** spawn `tmux::kill_window_if_present`. On `Err`, surface a
  `SystemMessage::Error` and leave the window alone — the same fail-visible
  shape `exit_session` uses. It returns the teardown's `JoinHandle` (`None` when
  the close failed or there was no window), mirroring `exec_check_window`, so
  the tests can await the teardown instead of sleeping.
- `ClosedSession` therefore carries `sort_order_after_write` alongside `window`,
  with the same contract as `UpdateTaskResult::sort_order_after_write`. Without
  it a freshly finished task renders at the bottom of Done until the next
  refresh, the exact bug `exec_persist_task`'s comment warns about.

Commands are drained sequentially and awaited (`src/runtime/commands.rs`), so
the gate is the `Result`, not command ordering.

---

## 2. Extract the dispatch prologue

Four sites run `EpicContext` → `build_and_record_injections` →
`fetch_verify_command`: `auto_dispatch_next` and `handle_dispatch_task`
(`src/mcp/handlers/tasks/dispatch.rs`), and `src/runtime/tasks.rs:156-158`
(quick dispatch) and `:305-308` (`exec_dispatch_agent`).

No spec change: this is pure code motion, no observable behaviour.

### Tests

The four call sites are already covered end-to-end (chain tests, the
`dispatch_task` handler tests, `src/runtime/tests.rs` dispatch tests) and those
must pass untouched — that is the refactor's safety net. Two new focused tests.
They live in `src/runtime/tests.rs` rather than `src/dispatch/tests.rs`: the
prologue needs a DB with an epic, learnings-with-embeddings and a verify command,
and that scaffolding (plus `test_runtime`) already exists there, next to the
`build_learning_injections_partitions_and_records_retrievals` test it extends.

- `prepare_inputs_reads_epic_context_injections_and_verify_command` — a task on
  an epic in a repo with a verify command and one matching learning yields all
  three fields populated, and the learning's retrieval is recorded (the side
  effect `build_and_record_injections` owns).
- `prepare_inputs_with_epic_ctx_skips_the_epic_read` — passing a pre-fetched
  `EpicContext` returns it verbatim.

### Code

`src/dispatch/mod.rs` (or `agents.rs`, next to `fetch_verify_command`):

```rust
pub struct DispatchInputs {
    pub epic_ctx: Option<EpicContext>,
    pub injected: Vec<Learning>,
    pub verify_command: Option<String>,
}

pub async fn prepare_inputs(db: &dyn TaskReadStore, task: &Task,
                            emb_svc: &Arc<EmbeddingService>) -> DispatchInputs;

/// For callers that already hold the epic row (the chain) — skips the re-read.
pub async fn prepare_inputs_with_epic_ctx(db, task, emb_svc,
                                          epic_ctx: Option<EpicContext>) -> DispatchInputs;
```

`prepare_inputs` delegates to the second with `EpicContext::from_db(task, db)`.

The two real differences between the sites survive extraction untouched: the
chain's claim has already applied `Running` (so its post-dispatch patch sets
only worktree/tmux_window), and the chain is fire-and-forget while
`dispatch_task` awaits. Both live outside the prologue.

---

## 3. Hoist `ChainFixture` into the shared test module

`ChainFixture` (`src/mcp/handlers/tests/tasks/dispatch.rs:693`) duplicates
`create_running_task_with_window`, `test_state_with_db` +
`state_with_mock_task_svc`, and — in `close` — an 11th copy of the
seed-`exit_tokens`-then-call-`exit_session` boilerplate that
`src/mcp/handlers/tests/tasks/wrap_up.rs` repeats inline at 14 sites.

### Code (tests only; no spec, no production change)

`src/mcp/handlers/tests/mod.rs`, beside `create_running_task_with_window`:

- `seed_exit_token(state, task_id, action) -> String` — inserts the `ExitToken`
  and returns it.
- `close_session_via_mcp(state, task_id, action)` — `seed_exit_token` then the
  `exit_session` `tools/call`, adding `TEST_PR_URL` for the `Pr` action. This is
  `ChainFixture::close` verbatim.
- `test_state_with_overrides(runner, notify_tx, task_svc)` — the one `McpState`
  constructor, which `test_state`, `test_state_with_db`,
  `state_with_mock_task_svc` and `ChainFixture::build` all delegate to.
- `create_running_task_with_window_in(state, repo_path, epic_id)` — the
  parameterised form; `create_running_task_with_window` becomes a wrapper for
  `("/repo", None)`. Worktree and window are derived from the task id so a
  fixture holding several stays distinct.

Then:

- `ChainFixture::build` calls the shared constructor and gains `with_runner` for
  tests that need their own command script; `ChainFixture::close` delegates to
  `close_session_via_mcp`; `closing_subtask` becomes one delegating line.
- Replace all 14 inline token-seeding blocks in `wrap_up.rs` with
  `seed_exit_token` / `close_session_via_mcp`. This is the point of the item:
  otherwise the next change to the exit-token shape is a 15-site edit.
- Repoint the `dispatch_task` handler tests at the fixture, dropping their
  per-test temp-dir/worktree/db/state scaffolding. The exact runner script they
  assert through moves into a shared `dispatch_runner_script()` rather than
  being replaced by uniform successes — the command order is itself an
  assertion, and `with_runner` preserves it.

Verification is the suite itself: the same assertions must pass, with the
scaffolding gone. No test's *assertions* may change in this item — if one has
to, that is a signal the hoist changed behaviour and it stops here.

---

## 5. Route every dispatch entry point through the claim — follow-up task #3802

Deliberately not in this session. The atomic claim protects chain-vs-chain only:
`handle_dispatch_task` and the TUI `d` path both read status, then provision,
then write Running, so a claim landing in that window is invisible to them.
#3744 scoped the spec's exclusivity claim down to match (the "Scope of that
exclusivity" paragraph in `AutoDispatchNextSubtask`).

Sketch for the follow-up:

- Re-introduce a **by-id** conditional claim. Item 1 deletes
  `try_claim_backlog_task`; the replacement should be the sanctioned existing
  shape rather than a new bespoke statement — `update_status_if` for the
  Backlog→Running transition plus a `TaskPatch` for `last_pre_tool_use_at`, the
  "conditional transition, then patch the extras" pattern `cli_update_task`
  already uses. The second write is uncontended, since the row has already left
  Backlog.
- `handle_dispatch_task`: claim before provisioning; a lost claim becomes a
  "task is not in backlog" style error instead of the current pre-read check.
- The TUI path is the bulk: `src/runtime/tasks.rs` (`exec_dispatch_agent`,
  `exec_quick_dispatch`), `src/tui/update/lifecycle.rs`
  (`handle_dispatch_task`, `handle_trust_and_dispatch`,
  `handle_trust_check_untrusted`) and the `dispatching` map, whose
  render-time invariant is "only pre-dispatch (Backlog) tasks"
  (`src/tui/ui/kanban/cards.rs:78`) — a claim that flips the task to Running
  *before* dispatch completes breaks that assertion, so the marker and the
  `DispatchFailed`/release path have to be reworked together.
- Spec: replace the "Scope of that exclusivity" paragraph with unconditional
  exclusivity across all entry points; `dispatch.allium`'s `DispatchTask`
  postcondition gains the claim.

---

## Verification

`cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh` after each
item, plus `cargo clippy --all-targets -- -D warnings` before wrap-up (the
pre-push hook runs it and a green `cargo build` does not imply clippy-clean).
`allium check` on both edited specs, and `allium:weed` over `epics.allium` and
`pr-workflow.allium` at the end.
