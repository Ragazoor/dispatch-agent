# Staging Pipeline & Scheduled Agents — Design

Date: 2026-08-16
Task: #4199 "main pipeline"

## Problem

Running the full test/lint/validation suite on every task is slow and CPU
intensive. The goal is a two-tier flow:

- A task finishes its own work, runs a **cheap**, fast check, and integrates
  into a shared **staging** branch (not `main` directly).
- A separate, **scheduled** agent periodically wakes up, looks at whatever has
  landed on staging, runs the **full** (expensive) suite, fixes anything
  broken, and promotes validated work from staging into `main` — with no
  human in the loop.
- The scheduling itself should be a dispatch-native capability, not an
  in-session `claude /loop`, so it survives independently of any one
  conversation, is visible on the board, and doesn't hold an idle agent
  process/context open between runs.

## Scope split

This is two subsystems, built in this order:

1. **A generic scheduling primitive.** The ability for dispatch to
   periodically (re)dispatch a task, whose worktree can optionally track a
   fixed, pre-existing branch (rather than a fresh per-task branch). Nothing
   about this primitive is staging-specific.
2. **The staging pipeline**, which is one configuration of (1): a task with
   `pinned_branch = "staging"`, `base_branch = "main"`, a schedule interval,
   and a standardized "run the full suite and promote" prompt. Ordinary
   feature tasks need **zero changes** — they already support an arbitrary
   `base_branch`, so pointing them at `"staging"` instead of `"main"` works
   today.

## Part A — Generic scheduling primitive

### New `Task` fields (`core.allium`)

```
schedule_interval_secs: Integer?  -- null = not scheduled. When set, the
                                  -- scheduler redispatches this task on this
                                  -- cadence whenever it is idle.
pinned_branch: String?           -- null = normal per-task branch. When set,
                                  -- the task's worktree checks out this
                                  -- EXISTING branch literally, instead of
                                  -- creating "<id>-<slug>". Independent of
                                  -- schedule_interval_secs -- a task can pin
                                  -- a branch without being scheduled, or vice
                                  -- versa, though the pipeline use case sets
                                  -- both.
last_processed_sha: String?      -- null = nothing successfully promoted yet.
                                  -- Set to pinned_branch's tip only when a
                                  -- wrap_up(merge) for this task SUCCEEDS
                                  -- (see BranchMerged below). Used by the
                                  -- scheduler to skip a tick when nothing
                                  -- new has landed since the last success.
last_scheduled_check_at: Timestamp? -- wallclock of the scheduler's last
                                  -- look at this task, whether or not it
                                  -- resulted in a dispatch. Drives the
                                  -- elapsed-time gate, mirroring
                                  -- Epic.last_run for feeds.
```

Migration: four new nullable columns on `tasks`, all defaulting to null/no-op
for every existing row. No behavior change for a task that never sets
`schedule_interval_secs`.

`WrapUpMode` gains a fourth variant: `{rebase | pr | done | merge}` (see
Part A's wrap-up section below). A scheduled/pinned-branch task will
typically set `wrap_up_mode = merge` at creation so its prompt doesn't need
to ask.

### Worktree carve-out for `pinned_branch`

`DispatchTask`'s worktree provisioning already special-cases PR-review tasks
to check out an existing branch (`origin/<headRefName>`) instead of creating
`"<id>-<slug>"`. `pinned_branch` is a second, structurally similar carve-out:

- The worktree **path** stays keyed by task id, as today:
  `<repo_path>/.worktrees/<id>-<slug>`.
- The worktree's checked-out **branch** is `pinned_branch` verbatim, created
  from `origin/<pinned_branch>` (fetched first) if it doesn't exist locally,
  or reused as-is if the directory already exists on disk (the existing
  REUSED path already covers this — a pinned-branch task's worktree is
  reused on every subsequent tick, not recreated).
- No id-slug branch is ever created for a pinned-branch task.

### Scheduled (re)dispatch

A task with `schedule_interval_secs` set is dispatched by a **new rule**,
not by loosening `DispatchTask`'s own precondition (which stays
`status = backlog` for every other caller). Call it `DispatchScheduledTask`:

```
rule DispatchScheduledTask {
    when: SchedulerTick()

    requires: task.schedule_interval_secs != null
    requires: task.status in {backlog, done}      -- idle; done->running is
                                                    -- already a valid graph
                                                    -- edge (see ResumeTask)
    requires: task.tmux_window = null              -- no live agent already
                                                    -- running this tick

    let due = elapsed_since(task.last_scheduled_check_at) >= task.schedule_interval_secs

    ...
}
```

This keeps `DispatchTask`, `ExitSession`, and every existing lifecycle rule
completely untouched — scheduled dispatch is purely additive.

**Skip-if-unchanged, done in the scheduler, not the agent.** When `due` and
`task.pinned_branch` is set: fetch `origin/<pinned_branch>` (a single
lightweight git call, the same style as `repo_sync.rs`'s `ahead_behind`) and
compare its tip against `task.last_processed_sha`.

- Unchanged (and `last_processed_sha` is not null): bump
  `last_scheduled_check_at` only. No agent is dispatched — no tmux window, no
  Claude process, no cost. This is the direct fix for the CPU complaint:
  idle staging costs one `git fetch` per interval, not a full suite run.
- Changed, or `last_processed_sha` is null (first run ever): dispatch,
  reusing/creating the worktree as described above.

If `pinned_branch` is not set (a generic scheduled task with a fresh worktree
each run), there is no branch to diff — it simply redispatches every
interval unconditionally, like a cron job.

**Retry semantics fall out for free.** `last_processed_sha` is written only
on a *successful* promotion (see `BranchMerged` below), never speculatively.
So: a tick that fixed everything and promoted moves `last_processed_sha`
forward, and the next tick sees no drift and skips. A tick where the agent
got stuck and never completed a merge leaves `last_processed_sha` stale, so
the *next* tick still sees the branch as unprocessed and retries — no
separate "paused"/"stuck" state or backoff logic is needed; it's a natural
consequence of only recording success.

### New wrap-up action: `merge`

Available to **any** task (not only pinned-branch ones) as a fourth
`wrap_up` action, alongside `rebase`/`pr`/`done`. It exists because
`wrap_up(rebase)` is unsafe when the worktree's own branch is a *shared*
branch other worktrees depend on (see "Why not just use rebase" below).

Mechanics (`WrapUpMerge`, mirroring `WrapUpRebase`'s shape in
`pr-workflow.allium`):

1. Verify the repo root is on `task.base_branch` and clean (same preflight
   as rebase).
2. `git fetch origin <base_branch>`; `git pull origin <base_branch>` if a
   remote exists (same non-fatal-vs-fatal fetch-failure handling as
   `WrapUpRebase` step 3).
3. In the repo root: **squash-merge** the worktree's branch into
   `base_branch` — `git merge --squash <branch>` then commit. This creates
   exactly one new commit on `base_branch`. Critically, this **never
   rewrites either branch's existing history**: `base_branch` only gains a
   new commit at its tip (safe even when other worktrees are branched off
   it), and the worktree's own branch (`branch`) is untouched (safe even
   when `branch` is a long-lived shared branch like staging).
4. On conflict: abort the merge, report the conflicted paths (read before
   the abort, same discipline as `ConflictFilesCapturedBeforeAbort` in
   `repo-sync.allium`), set `task.sub_status = conflict` (reusing the
   existing sub_status, same as `RebaseConflict`).
5. **No push.** Exactly like `wrap_up(rebase)`, this is a local-only write.
   Publishing `base_branch` to origin remains an explicit, separate act —
   see "Auto-push is scoped to the pipeline, not the tool" below.

```
ensures: BranchMerged(branch: branch, onto: task.base_branch, repo_path: task.repo_path)
```

`BranchMerged` mirrors `BranchRebased` and feeds two consumers:

- `RefreshRepoSyncStateAfterRebase`'s existing rule gains `BranchMerged` as a
  second trigger (repo-sync.allium) — a merge also moves local `base_branch`
  ahead of origin, so the drift indicator must refresh the same way it does
  after a rebase.
- A **new** rule, `RecordPipelineProgress`, fires on `BranchMerged` when
  `task.pinned_branch != null`: it sets `task.last_processed_sha` to
  `pinned_branch`'s tip at merge time. This is what closes the loop described
  in "Retry semantics" above.

Everything else about `wrap_up(merge)` mirrors `wrap_up(rebase)`/`wrap_up(pr)`
exactly: it performs no task-status write itself (mints an in-memory exit
token recording `action = merge`, same as the other three), the verify-command
reminder line appears in its response the same way, and `exit_session(token,
action="merge")` performs the terminal `status = done` transition — no new
code needed in `ExitSession` beyond adding `merge` alongside `rebase`/`done`
in whichever branch of that match arm produces the terminal
`done`-with-no-url outcome (same as `rebase`/`done` today; `merge` is not a
review-producing action, so it does not take the `pr` branch).

**Why not just use rebase (squash or otherwise) for everything?** Considered
and rejected. `wrap_up(rebase)` is safe *today* specifically because it only
ever rewrites the disposable, per-task `"<id>-<slug>"` branch — nobody else
depends on that branch's SHAs, so rewriting it is free. Changing that
mechanism globally (e.g. to always squash) would be a behavior change to
every existing task's wrap-up, trading away per-commit history (bisect,
blame, review granularity) for a benefit (safety on a shared branch) that the
existing mechanism never needed in the first place — the hazard only exists
for a worktree whose own branch is *itself* long-lived and shared, which is
new with `pinned_branch`. Keeping `rebase` unchanged and adding `merge`
alongside it confines the new mechanics to where they're actually needed,
while making the safer option available to any task that wants it (e.g. a
task whose `base_branch` happens to be another in-flight task's branch).

**Auto-push is scoped to the pipeline, not the tool.** `repo-sync.allium`'s
`SyncNeverAutomatic` invariant says publishing a shared branch to origin is
always an explicit operator action — no timer, no wrap-up step, no agent
tool may do it automatically. `wrap_up(merge)` honours that: it never
pushes, exactly like `wrap_up(rebase)`. But the whole point of the staging
pipeline is unattended promotion to `main` with no human step. That
automation lives **one layer up**, in the scheduled-dispatch completion flow
for pinned-branch tasks specifically (Part B) — not in the generic
`wrap_up(merge)` tool every task can call. This is a deliberate, narrowly
scoped exception, recorded the same way `feeds.allium` calls out its own
role-routed-path exception: the exception is contained to
`schedule_interval_secs != null and pinned_branch != null` tasks, and no
other caller may push a shared branch automatically.

### Scheduler mechanics

A new background loop, structurally parallel to `FeedRunner`
(`src/feed/mod.rs`): ticks on its own interval (independent of the TUI tick
and the feed poll, same reasoning as those two being kept separate), walks
tasks with `schedule_interval_secs != null`, and applies the elapsed-time
gate per task. Command/agent-launch work is spawned as background tokio
tasks so a slow git fetch or a live dispatch never blocks the event loop —
same discipline as `FeedTick`.

### Config surface (v1: MCP/CLI only, no new TUI)

- `create_task` / `update_task` gain `schedule_interval_secs` and
  `pinned_branch` as settable fields.
- No new TUI forms or pickers in this iteration — configuring a scheduled
  task is an `update_task` call, not a board interaction. (Revisit if this
  proves painful in practice.)

## Part B — Staging pipeline (an application of Part A)

### Verify-command tiering

`SavedRepoPath` gains a second command, parallel to the existing one:

```
verify_command: String?       -- existing: the cheap, fast check a task's
                               -- own agent runs before its own wrap-up.
full_verify_command: String?  -- new: the exhaustive suite (all tests, all
                               -- lints, everything CPU-intensive) that only
                               -- the scheduled pipeline agent runs.
```

Same single-line, no-newline validation as `verify_command`. New MCP tool
`set_full_verify_command` (and a clear variant), mirroring `set_verify_command`
exactly; new CLI form `dispatch repo set-verify-full <path> <command>` /
`dispatch repo clear-verify-full <path>`, mirroring the existing `repo
set-verify` / `clear-verify`.

Surfaced to the agent the same way `verify_command` is today — **not** in
the dispatch prompt, but through `get_task` (a "Full verify command" line,
alongside the existing "Verify command" line) and through `wrap_up(merge)`'s
response, the same two-surface pattern documented in `dispatch.allium`.

### The pipeline task's prompt

A pinned-branch, scheduled task gets its own prompt variant (a new
`build_pipeline_prompt`, alongside `build_prompt` /
`build_quick_dispatch_prompt` / `build_research_prompt`), used whenever
`task.pinned_branch != null`. Shape:

- States plainly that this is a recurring pipeline task on `<pinned_branch>`,
  and that new commits have landed since it last ran (the scheduler only
  dispatches when that's true, so this is always accurate).
- Points at `full_verify_command` via `get_task` (as above) — run it; if it
  fails, fix the failures with ordinary commits directly on the worktree's
  branch (no PR, no rebase — this is forward development on staging, same as
  any other commit).
- Once green, call `wrap_up(action="merge")` then `exit_session(token,
  action="merge")`.
- Skips the plan/brainstorm addendum entirely (there's no "plan" for a
  pipeline run) and skips the epic/sibling-communication block (a
  pinned-branch task is not epic-decomposition work). Keeps the shared TDD /
  Allium / MCP-tools / learning-tools / wrap-up trailing instructions,
  mirroring the existing unified-skeleton discipline in `dispatch.allium`.

### Example configuration (how a user actually sets this up)

```
set_full_verify_command(repo_path, "cargo test && cargo clippy --all-targets -- -D warnings && ...")

create_task(
    title: "staging pipeline",
    repo_path: <repo>,
    base_branch: "main",
    pinned_branch: "staging",
    schedule_interval_secs: 600,
    wrap_up_mode: merge,
)
```

Ordinary feature tasks need no new fields at all — they just pick `staging`
as their `base_branch` in the existing `BaseBranchPicker` (it already accepts
any typed branch name), run their own cheap `verify_command`, and
`wrap_up(rebase)` onto `staging` exactly as they would onto `main` today.
`staging` only ever moves forward by fast-forward from those rebases and by
the pipeline's own fix-up commits — its history is never rewritten by
anything in this design, which is what makes `wrap_up(merge)`'s "never
rewrites either branch" property hold for it specifically.

### Auto-push completes the loop

The scheduled-dispatch completion path for a pinned-branch task (Part A's
scheduler, specifically the branch that handles `SessionClosed` for a task
with both `schedule_interval_secs` and `pinned_branch` set) pushes
`base_branch` to origin immediately after a successful `wrap_up(merge)` +
`exit_session`. This is the one place in the whole design that performs an
automatic push to a shared branch, and it is deliberately narrow: gated on
both fields being set (i.e. only a task the user explicitly configured as a
recurring pipeline), never reachable from `wrap_up(merge)` itself, from a
one-off task, or from any manual wrap-up path.

## Open items / deferred

- No TUI surfaces for configuring scheduled/pinned-branch tasks in v1
  (MCP/CLI only — see Part A).
- No "paused after repeated failure" state — retry-forever falls out of the
  `last_processed_sha`-on-success-only design and needs no extra code (see
  "Retry semantics" above). Revisit only if repeated-failure cost becomes a
  real problem in practice.
- Merge-conflict recovery for `wrap_up(merge)` reuses the existing `conflict`
  sub_status rather than introducing a new one; if this proves confusing to
  distinguish from a rebase conflict on the board, a distinct sub_status can
  be added later.
