# Auto-Dispatch Moves Into `exit_session` — Design Spec

**Date:** 2026-07-26
**Task:** #3744 — automatic dispatch
**Status:** Approved 2026-07-26 (revised after adversarial review; the atomic claim in Component 1 is approved scope)

> Tracked in git: `main` gained `90903bf5` ("docs: track docs/plans/ and
> docs/superpowers/ in git") on 2026-07-26, removing both paths from
> `.gitignore`. Earlier revisions of this doc said it was ignored; that is no
> longer true.

## Problem

Epic subtask chaining is currently agent-initiated and fires too early.

`/wrap-up` Step 5 opens with (`plugin/skills/wrap-up/SKILL.md:101-105`):

> ### Dispatch next epic subtask
> If the task has an `epic_id`, call the `dispatch` MCP tool `dispatch_next` with that `epic_id`. This fires the next agent in the epic immediately, **while you complete the steps below**.

The steps below are the ones that actually land the work: `wrap_up(action="rebase")` fast-forwards `base_branch`, `/retro` runs, then `exit_session` applies the terminal status patch. So the ordering today is:

1. Agent A calls `dispatch_next` → subtask B's worktree is cut from `origin/<base_branch>`.
2. Agent A calls `wrap_up(rebase)` → A's branch is rebased and `base_branch` is fast-forwarded.

B branched from a base that does not contain A's commits. For a plan-decomposed epic — where subtask B is usually the direct continuation of subtask A — B starts blind to its predecessor's work: it re-derives context that already exists, re-implements code A already wrote, and hits avoidable conflicts when its own branch is later rebased.

Because the trigger lives in a markdown skill rather than in the server, the ordering is also unenforceable. An agent that skips the section, or reorders it, produces silently different behaviour.

## Goal

Make automatic epic dispatch the **last thing that happens when `exit_session` is called**, server-side, so the next subtask's worktree is always cut from a `base_branch` that already contains the finished subtask's work.

## Approach

Move the chain from the skill into `handle_exit_session`, and delete the `dispatch_next` MCP tool.

Deleting rather than keeping the tool is deliberate: its only caller is the `/wrap-up` skill. Left in place alongside automatic dispatch it becomes a double-dispatch footgun — an agent that calls `dispatch_next` and then exits fires *two* subtasks, one of them from a stale base, reintroducing exactly the bug this change removes.

### Ordering guarantee

The fix is structural rather than a matter of timing or retries:

| Step | Where | Synchronous? |
|---|---|---|
| Rebase + fast-forward `base_branch` | `wrap_up(action="rebase")` | yes — awaited before the exit token is issued |
| Terminal patch (`Done` / `Review` + `pr` url), `tmux_window = null` | `exit_session` | yes — `update_task` is awaited |
| Claim next backlog subtask | `exit_session` | yes |
| Create next subtask's worktree + tmux window | background task spawned by `exit_session` | no |

Every **database write** that determines the next subtask's starting base completes before the next subtask is claimed.

One thing genuinely is still in flight: the closing task's `tmux kill-window` is a detached, never-joined `spawn_blocking` (`src/mcp/handlers/tasks/wrap_up.rs:379-383`) and can overlap the new window's creation. That is harmless because window names are keyed by task id (`build_tmux_window_name` → `task-<id>`, `src/dispatch/prompts.rs:41-43`), so the closing and opening windows can never collide. This design does not tighten it — but the guarantee above is about DB state, not about OS-level teardown, and future work must not read it as more than that.

## Components

### 1. Atomic claim — `src/db/`, `src/service/tasks/crud.rs`

`next_backlog_task` (`src/service/tasks/crud.rs:672-688`) is a pure read: it lists the epic's tasks, filters `status == Backlog`, sorts, returns the first. Nothing removes the returned task from contention until the *background* dispatch later patches it to `Running`. Two `exit_session` calls on the same epic that interleave across that `.await` therefore both select the same subtask B and both spawn a dispatch for it — two worktrees and two tmux windows for one task, with the losing pair silently orphaned.

That TOCTOU exists today in `handle_dispatch_next`, but this change makes chaining an unconditional side effect of *every* close on an `auto_dispatch` epic instead of one opt-in call per session, and epics can legitimately have several subtasks running in parallel after manual dispatch. Shipping automatic chaining on top of a racy selection is not acceptable, so the claim becomes atomic:

- **DB layer**: `try_claim_backlog_task(&self, id: TaskId, now: DateTime<Utc>) -> Result<bool>` — a single statement, `UPDATE tasks SET status = 'running', sub_status = <default_for(running)>, last_pre_tool_use_at = ?, updated_at = ? WHERE id = ? AND status = 'backlog'`, returning `rows_affected == 1`. No schema change, so no migration.
- **Service layer** (the mutation boundary — task writes never bypass it): `TaskService::claim_next_backlog_task(&self, epic_id) -> Result<Option<Task>>`. Loops at most a small bounded number of times: `next_backlog_task` → `try_claim_backlog_task`. A `false` return means a concurrent caller won the row, so it re-selects; `None` from the selection ends the loop. On a successful claim it fires `recalculate_epic_status` exactly as the normal patch path does, and returns the task with the claimed status applied.

All writes serialise through the single writer connection (`db_call`, see `docs/conventions.md`), so the conditional `UPDATE` is atomic with respect to every other mutation — the `WHERE status = 'backlog'` clause is what makes the claim exclusive, not a lock.

Claiming moves the `Running` transition earlier, ahead of worktree provisioning. Three consequences, all verified benign:

- A claimed-but-unprovisioned task is `Running` with `tmux_window = None`. The tick's window check filters on `tmux_window.is_some()` (`src/tui/update/agent.rs:248-255`) and activity classification filters on `Running && tmux_window.is_some()` (`src/tui/update/agent.rs:277`), so no spurious `Crashed` or `Stale` during the provisioning window.
- `is_wrappable` requires a worktree (`src/dispatch/agents.rs:218-221`), so a claimed task cannot be wrapped up before it is provisioned.
- Seeding `last_pre_tool_use_at` at claim time subsumes the seed the current dispatch tail applies for the same reason (keeping the fresh agent classified `Active` until its first `PreToolUse` hook).

If the dispatch subsequently fails, the background tail patches the task back to `Backlog` with `sub_status = default_for(backlog)` and logs a warning, so a failed chain leaves the subtask dispatchable exactly as it is today.

`handle_dispatch_task` gains protection for free: it rejects a task whose status is not `Backlog`, so a task claimed by a chain can no longer be double-dispatched through that tool either.

**Residual, out of scope**: the TUI `d` dispatch path does not claim, so a human pressing `d` on the exact subtask a chain is claiming, in the same instant, can still double-dispatch. That window is pre-existing, human-timing-dependent, and closing it means converting the TUI dispatch path onto the same claim — a separate change. Also pre-existing and untouched: `provision_worktree`'s `git fetch` / `git worktree add` against the shared repo root (`src/dispatch/worktree.rs:150-187`) has no contention retry against a sibling task's concurrent `git pull` / `merge --ff-only` on that same root; only the fetch step retries.

### 2. `auto_dispatch_next` helper — `src/mcp/handlers/tasks/dispatch.rs`

Replace `handle_dispatch_next` (`src/mcp/handlers/tasks/dispatch.rs:68-196`) with:

```rust
/// Dispatches the next backlog subtask of `epic_id`, if any. Returns
/// `Some((id, title))` when a dispatch was started, `None` when the chain
/// stops here (auto_dispatch off, no backlog subtask, or a lookup failure).
/// Never returns an error: a chain problem must not fail the caller.
pub(super) async fn auto_dispatch_next(
    state: &McpState,
    epic_id: EpicId,
) -> Option<(TaskId, String)>
```

`pub(super)` suffices — `wrap_up` and `dispatch` are both descendants of the private `tasks` module (`src/mcp/handlers/tasks/mod.rs:16`).

Behaviour, carried over from `handle_dispatch_next` apart from the claim and the return type:

- Reads the epic. `auto_dispatch = false` → `None`. Epic missing → warn, `None`. DB error → warn and **continue** (the existing fail-open read: a DB hiccup must not silently stall an epic).
- `claim_next_backlog_task(epic_id)` → `None` when no backlog subtask remains or the claim errors (warn).
- Builds `EpicContext`, learning injections, and the verify command, then spawns the existing background task: `spawn_blocking(do_dispatch)` → on success patch the worktree and tmux window (status is already `Running` from the claim), on failure revert to `Backlog` → send `TaskChanged` / `EpicChanged` on `notify_tx`.
- Returns `Some((next_id, next_title))` for the caller's response text.

The five `dispatch_next:`-prefixed log messages (`:78`, `:101`, `:166`, `:171`, `:176`) are renamed to `auto_dispatch_next:`.

### 3. Delete the `dispatch_next` tool

- The `async "dispatch_next" => tasks::handle_dispatch_next` entry and its schema (`src/mcp/handlers/dispatch.rs:347-358`). `TOOL_NAMES`, `tool_definitions()`, and `dispatch_tool()` are macro-generated from that list, so removing the entry is sufficient.
- `DispatchNextArgs` (`src/mcp/handlers/tasks/mod.rs:173-177`), its mention in the `use super::{...}` list in `tasks/dispatch.rs:8-11`, the `handle_dispatch_next` re-export (`tasks/mod.rs:24-26`), and the test-module import at `src/mcp/handlers/tests/mod.rs:31`.

While in `src/mcp/handlers/dispatch.rs`, fix the stale claim in the `mcp_tools!` doc comment at `:38` that `TOOL_NAMES` is "used by setup.rs" — nothing in `src/setup/` reads it.

### 4. `handle_exit_session` — `src/mcp/handlers/tasks/wrap_up.rs`

After the existing terminal patch, the `notify_task_changed` / `notify_epic_changed` calls, and the `spawn_blocking` tmux kill, add:

```rust
let next = match task.epic_id {
    Some(epic_id) => auto_dispatch_next(state, epic_id).await,
    None => None,
};
```

`task` is partially moved at `:377` (`let tmux_window = task.tmux_window;`), but `epic_id` is a distinct `Copy` field that was never moved, so this compiles.

Response text:

- no chain: `"Session closed."` (unchanged)
- chained: `"Session closed. Dispatching next epic subtask #<id> '<title>'."`

Placement is last on purpose — after teardown, after every task mutation, and after the epic-status recalculation triggered by the patch flow.

The `exit_session` tool description in `mcp_tools!` gains a sentence stating that closing a subtask automatically dispatches the epic's next backlog subtask when `auto_dispatch` is enabled, so an agent reading `tools/list` does not go looking for a tool to call.

### 5. Skill and docs

- `plugin/skills/wrap-up/SKILL.md`: delete lines **101-106** (the `### Dispatch next epic subtask` heading and both paragraphs), leaving `:99` adjacent to `### If rebase:`. Step 5 has three branches (rebase, pr, done), each with its own `exit_session` description — each gains one line stating that `exit_session` chains the epic's next subtask automatically and the agent must not try to dispatch it.
- `CLAUDE.md:122`, Finishing bullet: drop `dispatch_next`, state that `exit_session` chains automatically.
- `README.md:120`: reword "Agents can call `dispatch_next` to trigger the next subtask themselves" to describe the automatic chain.
- `docs/module-map.md:75`: drop `dispatch_next` from the `tasks/dispatch.rs` row.

Historical artifacts under `docs/plans/` (`300-epic-dispatch-next.md`, `330-dispatch-task.md`) are tracked despite the current `.gitignore` rule; they are dated working artifacts and are deliberately left stale, as is `docs/superpowers/specs/2026-04-14-auto-dispatch-epic-design.md`.

## Error handling

### The closing patch itself (decided 2026-07-26)

`exit_session`'s terminal patch — the single write that sets the status *and* clears
`tmux_window` — currently has its failure swallowed at `warn` (`src/mcp/handlers/tasks/wrap_up.rs:367-372`),
after which the handler tears down the window, chains, and reports `"Session closed."` The specs
(`pr-workflow.allium` `ExitSession`, `mcp-task-tools.allium` `ExitSessionViaMcp`) state that
mutation unconditionally, so the code is wrong. The swallow predates this change, but chaining on
top of it compounds a broken close into a second dispatch.

Resolution: **gate the chain on the patch succeeding.**

- ~~The tmux window is still killed — the agent is exiting regardless, and a surviving window is a
  dead prompt in the user's tmux.~~ **Superseded 2026-07-27** — the teardown is gated on the patch
  too; see "Failed-close visibility" below. The "dead prompt" objection does not hold: the window
  still hosts a live agent, because nothing kills it.
- The chain does **not** fire. A failed close plus a freshly launched successor is strictly harder
  to notice than a failed close alone.
- The response reports the failure instead of `"Session closed."`, so the agent (and the human
  reading the board) learns the close did not take effect. The task stays visible in its current
  status for a manual retry.
- `SessionClosed(task)` is therefore emitted only when the terminal mutation succeeded; the specs
  must model that condition rather than asserting it unconditionally.

Not chosen: returning a JSON-RPC error and skipping teardown. The exit token is consumed before
this point, so an error strands the agent with no retry path and a live window.

### Failed-close visibility (decided 2026-07-27, revised same day)

**First decision, now superseded.** With the teardown still unconditional, a failed close left the
task at its old status holding a `tmux_window` that named a just-killed window. That was accepted
on the grounds that `DetectCrashedAgent` would match it, producing `sub_status = crashed` plus an
urgent notification — mislabelled, but not silent.

**Why it was revised.** That rationale only holds from `running`. `exit_session` is reachable from
`review` too: `is_wrappable` accepts `Review || Running` (`src/dispatch/agents.rs:218-221`),
`handle_exit_session` only re-checks `tmux_window != null`, and the Stop hook moves
`running → review` whenever the agent's turn ends — which can happen during `/retro`, between
`wrap_up` and `exit_session`. `DetectCrashedAgent` requires `status = running`, so from `review`
`handle_window_gone` (`src/tui/update/agent.rs:21-36`) takes its non-running branch: it clears
`tmux_window` with **no sub-status change and no notification**. The task then satisfies
`is_detached` (`src/models/tasks.rs:296-301`) and renders in the awaiting-merge section — looking
*more* finished than it is. Silent, and in the most misleading possible direction.

Resolution: **gate the tmux teardown on the patch succeeding as well.**

- The window is killed only when the close actually took effect.
- On failure the task keeps its `tmux_window`, so it never satisfies `is_detached` and never
  drifts into awaiting-merge — from either status.
- Nothing looks finished. A `running` task stays in Running and, receiving no further `PreToolUse`
  hooks, is classified `stale` by the normal tick path. A `review` task stays in Review with a live
  window. Either way the board still shows work outstanding.
- The window hosts a live agent rather than a dead prompt, so the human can attach to it and retry
  the close directly.

This makes the `crashed`-signal reasoning moot: with the window alive, `WindowGone` never fires and
`DetectCrashedAgent` is never reached. The spec text asserting a failed close surfaces as `crashed`
(and `agent-health.allium`'s cross-reference naming it a second cause reaching that rule) must be
removed rather than merely amended, and the `FailedCloseFromReviewVisibility` open question is
resolved by this decision. Note that iteration 2 modelled the teardown as an unconditional
`not exists window`; that too becomes conditional.

Still not chosen: a dedicated `close_failed` sub-status, a separate failure-time notification,
widening `DetectCrashedAgent` to `review` (it would break the deliberate quiet detach of review
tasks that legitimately lose their window), and rejecting `exit_session` from `review` (the Stop
hook can put a task there through no fault of the agent, which would then be unable to close).

### `wrap_up` and `updated_at` (decided 2026-07-27)

`WrapUpDone`, `WrapUpPr`, and `WrapUpRebase` each assert `ensures: task.updated_at = now`
(`docs/specs/pr-workflow.allium:172`, `:242`, `:293`), but `finish_wrap_up_simple`
(`src/mcp/handlers/tasks/wrap_up.rs:98-131`) performs no DB write at all for `done` and `pr` — it
mints an in-memory exit token and returns prose. `finish_wrap_up_rebase` writes only when clearing
a `conflict` sub-status.

Resolution: **fix the spec, not the code.** Drop or qualify the `updated_at` assertions to match
what the handlers actually do. This is pre-existing drift, unrelated to the chaining change, and
adding a DB write to `wrap_up` would be a real behaviour change to a path this task does not
otherwise touch.

### The chain

No failure in the chain may fail the close. `exit_session` has already killed the tmux window and moved the task by the time `auto_dispatch_next` runs; returning an error there would tell the agent its session failed to close when it did.

Every failure path — epic missing, DB error, claim error, dispatch panic, dispatch error — is logged at `warn` and the response is the plain `"Session closed."`. The epic simply stops chaining and the human dispatches the next subtask from the TUI, which is the same recovery path `auto_dispatch = false` already produces.

## Scope boundaries

**The TUI finish path is unchanged.** `W` then `r` in the TUI completes via the `FinishTaskSuccess` rule (`docs/specs/pr-workflow.allium:402`; `TaskMessage::FinishComplete` / `handle_finish_complete` in `src/runtime/tasks.rs:661`), not `ExitSession`, and does not chain today. Adding chaining to human-driven finishes is a separate decision.

**`dispatch_task` is unchanged.** It stays the explicit "dispatch this specific task" tool.

**The `auto_dispatch` flag and its `U` toggle are unchanged.** The flag keeps its exact meaning; only the code path that reads it moves.

## Accepted behaviour changes

1. **`/retro` follow-ups become chain candidates.** `/retro` runs between `wrap_up` and `exit_session` and may create follow-up tasks on the same epic. Under the old ordering those tasks did not exist yet when `dispatch_next` fired; now they are visible to the selection. `/retro`'s `create_task` call never passes `sort_order` (`plugin/skills/retro/SKILL.md:47-62`), and `next_backlog_task` sorts by `(sort_order.unwrap_or(id), id)` (`src/service/tasks/crud.rs:685`), so a follow-up's fallback key is its own — always larger — id. It therefore sorts after every planned subtask and is only ever chosen once the planned subtasks are exhausted, at which point dispatching it is the right outcome.
2. **The chain now starts strictly later.** Subtask B previously started while A was still rebasing and running `/retro`; it now starts after A's session is fully closed. Wall-clock latency between subtasks grows by roughly the duration of `/retro`. This is the cost of the correctness the change buys, and it is the explicit intent of the task.
3. **All three wrap-up actions chain**, matching today's behaviour (the skill fires `dispatch_next` regardless of action). For `action = "pr"` the finished work is on a PR branch rather than `base_branch`, so subtask B still will not see it — but that is inherent to choosing the PR path, not a regression introduced here, and stalling an epic until a human merges the PR would be a larger behaviour change than this task asks for.
4. **A dispatching subtask now shows as `Running` a few seconds earlier**, from claim rather than from successful provisioning (see Component 1).

## Allium spec updates

`DispatchNextViaMcp` is currently duplicated, in `docs/specs/epics.allium:452-488` and `docs/specs/mcp-task-tools.allium:312-340`, with different `ensures` bodies. Both are deleted.

Replacement, keyed on a new `SessionClosed(task)` event so the epic-chaining semantics stay in the file that owns the `auto_dispatch` flag. This mirrors the established `AgentLaunched` pattern — `ensures`'d from rules in `dispatch.allium`, `epics.allium`, and `tasks.allium`, consumed via `when: AgentLaunched(task, mode)` in `learnings.allium:300`.

- `epics.allium` gains `AutoDispatchNextSubtask`:
  - `when: SessionClosed(task)`
  - No `requires`; guarded inside `ensures` so a stopped chain is a normal outcome rather than a violated precondition. No epic, `auto_dispatch = false`, or no backlog subtask each leave the epic untouched; otherwise `AgentLaunched(task: next, mode: standard)` with `next = first_by_order(epic.subtasks where status = backlog)`.
  - Guidance carries over the surviving notes from `DispatchNextViaMcp`: only direct `Task` children are candidates (sub-epics are never auto-dispatched), `sort_order` ascending with an id fallback and id tiebreaker, the fail-open `auto_dispatch` read, background dispatch with a TUI refresh notification, the atomic claim that makes selection exclusive, and the fact that failure cannot fail the close.
- `mcp-task-tools.allium` `ExitSessionViaMcp` (`:510`) and `pr-workflow.allium` `ExitSession` (`:323`) each gain a trailing `ensures: SessionClosed(task)`, plus guidance that it fires last and cannot fail the close.
- Three stale cross-references fixed: `mcp-task-tools.allium:354` (`-- Unlike DispatchNextViaMcp, this call is synchronous`, inside `DispatchTaskViaMcp`), `epics.allium:634` (`see DispatchNextViaMcp`, inside `RegroupEpic`), and `epics.allium:543`, where `ToggleAutoDispatch` (rule at `:532`) says the flag makes "the MCP dispatch_next tool" return an informational message.
- The claim itself is a mechanism, not domain behaviour, so it is documented in guidance rather than modelled as a rule — but `AutoDispatchNextSubtask` must state that at most one agent is ever launched per closed session even under concurrent closes.

Applied with the `allium:tend` skill, verified with `allium check` and `allium:weed`.

## Testing

TDD: every test below is written and failing before the corresponding code change.

### Rewritten — `src/mcp/handlers/tests/tasks/dispatch.rs:662-1024`

The five `dispatch_next_*` tests are rewritten to drive `wrap_up(action="done")` → `exit_session` instead of `dispatch_next`, preserving each property. The exit path never branches on `task.tag` — `handle_wrap_up`, `finish_wrap_up_*`, and `handle_exit_session` never read it, and `is_wrappable` gates on worktree plus `Running`/`Review` only — so tag-tagged fixtures survive the move unchanged.

| Existing test | Becomes |
|---|---|
| `dispatch_next_picks_first_backlog_subtask` (`:735`) | closing a subtask dispatches the epic's first backlog subtask |
| `dispatch_next_respects_sort_order` (`:841`) | selection honours `sort_order` ascending |
| `dispatch_next_respects_tag_routing` (`:939`) | the chained dispatch honours `DispatchMode::for_task` |
| `dispatch_next_no_backlog_returns_success_noop` (`:680`) | closing the last subtask closes cleanly and dispatches nothing |
| `dispatch_next_epic_not_found_returns_error` (`:665`) | a task whose `epic_id` does not resolve still closes successfully (warn-and-skip, **not** an error — this inverts the old assertion) |

`dispatch_next_returns_disabled_when_auto_dispatch_off` (`src/mcp/handlers/tests/tasks/crud.rs:2707-2766`) moves to the same file as an `exit_session` variant: `auto_dispatch = false` → task closes, no subtask dispatched.

### New

- **No epic**: closing a task with `epic_id = None` returns `"Session closed."` and dispatches nothing.
- **Ordering** (the regression guard for this task): with subtasks A and B on one epic, close A and assert that when B reaches `Running` with a worktree, A is already `Done` with `tmux_window = None`. This is the property the old flow violated.
- **All three actions chain**: `rebase`, `done`, and `pr` each dispatch the next subtask.
- **Response text**: the chained response names the dispatched subtask's id and title.
- **Claim exclusivity** (service level, `src/service/tasks/tests.rs`): two concurrent `claim_next_backlog_task` calls on an epic with two backlog subtasks return two *different* tasks, and a third returns `None`. Plus a DB-level test that `try_claim_backlog_task` returns `false` for a task already out of `Backlog`.
- **Claim revert**: a failing dispatch leaves the claimed subtask back in `Backlog` with no worktree.
- **Tool list**: the registry test at `src/mcp/handlers/tests/mod.rs:600-605` drops its `dispatch_next` case, and the payload-deserialisation arm at `:752-754` goes with it — leaving that arm is a compile error once `DispatchNextArgs` is gone, not merely a stale case. The "Tool list mismatch" assertion at `:694-699` is the gate that fails until both are removed.
- **Embedded plugin copy**: a content assertion in `src/setup/plugins.rs` (alongside `wrap_up_skill_uses_simplify_not_code_simplifier` at `:498`) that the embedded wrap-up skill no longer instructs the agent to call `dispatch_next`. This is the only mechanism that catches a regression in the embedded copy.

### Test mechanics

Background completion is awaited by draining `McpEvent::TaskChanged(next_id)` from an `mpsc::unbounded_channel` passed as `notify_tx` to `McpState::new` (knowledge base #108). No `tokio::time::sleep` — `./scripts/check-no-test-sleep.sh` rejects it in the pre-push hook.

Tests asserting that *nothing* was dispatched cannot wait on an event that will never arrive. They assert on state instead: after `exit_session` returns, the sibling backlog task is still `Backlog` with `worktree = None`. Because the claim is synchronous and completes before `exit_session` returns, a task that was never claimed can never later transition — so this assertion is not merely a timing snapshot.

## Verification

```
cargo test && ./scripts/check-doc-paths.sh
```

`check-doc-paths.sh` only validates `src/**.rs` references inside six hardcoded docs (`scripts/check-doc-paths.sh:15-22`), which does not include `docs/superpowers/`. This doc's own `src/` references are therefore checked by passing it explicitly:

```
./scripts/check-doc-paths.sh docs/superpowers/specs/2026-07-26-exit-session-auto-dispatch-design.md
```

Plus `cargo clippy --all-targets -- -D warnings` and `allium check` before the work is called done.
