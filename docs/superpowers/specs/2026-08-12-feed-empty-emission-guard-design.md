# Feed empty-emission guard and task teardown on feed removal

Task #3989. Follow-up to #3900, which made this failure visible but deliberately
left the behaviour unchanged.

## Problem

When a feed command exits 0 and emits `[]`, `run_role_routed_feed_sync`
(`src/feed/ingest/role_routed.rs::run_role_routed_feed_sync`) calls
`delete_stale_subtree` with an empty keep-list. The SQL is
`external_id NOT IN (SELECT value FROM json_each(?2))`
(`src/db/queries/tasks.rs::delete_stale_subtree_feed_tasks`), and `NOT IN` against
an empty JSON array matches every feed task in the subtree. A feed script that
fails internally but still exits 0 with an empty array therefore does not merely
fail to fill My/Team/Bots — it deletes everything already in them.

Two things make it worse:

- It is a raw `DELETE`, with no worktree or tmux teardown. A review task with an
  in-flight agent has its row deleted and its worktree orphaned on disk.
- This is exactly how the #3900 incident went: the wrong `gh` account made all
  four repo-scoped queries fail, `scripts/fetch-reviews.sh` soft-failed each to
  `[]`, and the subtree was wiped.

#3900 made the failure visible — stderr now reaches `app.log` and the status bar —
but the hint fires *after* the delete has already run, so today dispatch reports a
wipe rather than preventing one.

## What is a bugfix and what is new

The cleanup half is a bugfix: feed-driven removal should always have torn down the
worktree, exactly as `ArchiveTask` and `DeleteTask` do.

The guard half is **not** a bugfix. The code matches the spec exactly.
`feeds.allium`'s `FeedCommandStderrOnSuccess` currently states:

> This is diagnostic only. The sync proceeds exactly as it would for a command
> that wrote nothing to stderr — in particular an empty emission still
> reconciles, and still removes tasks absent from it.

That sentence was written deliberately by #3900, having seen this failure. This
design reverses it. It is recorded here as a reversal, with its cost, rather than
presented as a fix.

## Decisions

1. **Trigger: stderr-gated skip only.** Zero items AND non-empty stderr → treat as
   a `FeedCommandFailure` and skip the sync entirely. A delete blast-radius cap and
   a "never delete in-flight tasks" rule were both considered and rejected: the cap
   needs a threshold and would block legitimate mass cleanup, and refusing to
   delete in-flight tasks would leave merged-PR cards lingering forever.
2. **Cleanup: delete, then tear down.** The delete returns the rows it removed; the
   feed layer runs the same teardown `ArchiveTask`/`DeleteTask` use. Rejected
   alternative: refusing to delete tasks that carry state, which changes normal
   behaviour visibly.
3. **All feed delete paths**, not just `reviews_parent`.
4. **Surfacing matches the existing rules.** Auto-poll logs and bumps `last_run`,
   silent in the TUI. Manual `r` refresh shows
   `Feed for '<title>' failed: <stderr>`.

### Accepted cost of decision 1

A feed script that writes progress chatter to stderr on *every* run and also
legitimately emits `[]` on a day with genuinely zero items will be treated as
failed indefinitely. That feed never reconciles, so genuinely merged or closed PRs
never disappear from it. There is no escape hatch short of fixing the script.

This is the cost #3900 avoided by keeping stderr diagnostic-only. We accept it
because the alternative is unbounded data loss, and because the script contract in
`feeds.allium` already requires errors on stderr to come with a non-zero exit — a
script that chatters on stderr while exiting 0 is already outside contract.

`scripts/fetch-reviews.sh` is clean here: only its explicit failure branches write
to stderr. The residual risk applies to any future feed script and is documented in
the spec rather than left implicit.

## Design

### 1. `DegradedEmptyEmission` — the guard

One shared predicate in `src/feed/exec.rs`, next to `FeedOutput`, so the two feed
paths cannot drift — the same rationale that put the role dispatch in the single
shared `run_feed_sync_by_role`:

```rust
/// Some(reason) when a zero-exit command emitted no items but wrote to stderr.
pub(crate) fn degraded_empty_emission(item_count: usize, stderr: &str) -> Option<String>
```

Both callers apply it **after** parse, **before** sync:

- `src/feed/mod.rs::FeedJob::run` — warn to `app.log`, return. `last_run` is already
  bumped by `FeedRunner::tick` before the job is spawned, so this lands exactly on
  `FeedCommandFailure`'s existing contract.
- `src/runtime/epics.rs::exec_trigger_epic_feed` — `fail(reason)`, so the status bar
  shows `Feed for '<title>' failed: <stderr>`.

Placing the guard after the parse is deliberate: malformed stdout with non-empty
stderr already fails as a parse error, which is bucket 3 of `FeedCommandFailure`.

The guard applies to every feed epic uniformly, not only `reviews_parent`.

**Consequence.** The `count == 0 && wrote_stderr` hint in
`src/tui/update/feeds.rs` becomes unreachable — a zero-item run that wrote to
stderr can no longer produce a `Refreshed` message at all. So `wrote_stderr` comes
off `FeedMessage::Refreshed` and the hint is deleted, superseded by the harder
failure. This rolls back one piece of #3900's UI work.

### 2. Task teardown, named once

`tasks.allium` already specifies teardown, but as duplicated prose in two rules —
`ArchiveTask` and `DeleteTask` each carry their own
`if task.worktree != null: not exists task.worktree` plus guidance about the tmux
window and the shared-worktree detach rule. `feeds.allium` restates none of it: its
removal steps say only `not exists task`.

Factor it into one named concept in `tasks.allium` capturing:

- kill the tmux window if present;
- remove the git worktree if present, **unless** another active task shares it, in
  which case detach from this task only and leave the worktree on disk;
- best-effort branch deletion.

`ArchiveTask`, `DeleteTask`, and the feed removal clauses in `RoleRoutedFeedSync`,
`GroupedFeedUpsert` and `FlatFeedReconcile` then reference the concept instead of
restating it. This is a factoring of existing specified behaviour, not a new
definition — the behaviour of Archive and Delete does not change.

### 3. Cleanup plumbing

The DB layer returns what it removed instead of discarding it, via `RETURNING`
rather than a separate `SELECT` — one statement, so the two can never disagree.
rusqlite 0.32 bundles SQLite 3.46; `RETURNING` needs 3.35.

```rust
pub struct RemovedFeedTask {
    id: TaskId,
    repo_path: String,
    worktree: Option<String>,
    tmux_window: Option<String>,
}
```

Applied to both delete sites: `delete_stale_subtree_feed_tasks`, and the
stale-delete inside `upsert_feed_tasks` — the latter covers
`clear_parent_stranded_tasks` and the grouped path's sub-epic clears, satisfying
decision 3.

Only rows with something to tear down are returned (`worktree IS NOT NULL OR
tmux_window IS NOT NULL`).

The rows propagate up through `delete_stale_subtree` →
`run_role_routed_feed_sync` / `sync_grouped_feed` → `run_feed_sync_by_role`, whose
return type becomes:

```rust
pub(crate) struct FeedSyncOutcome {
    affected_epics: Vec<EpicId>,
    removed: Vec<RemovedFeedTask>,
}
```

`RemovedFeedTask` and `FeedSyncOutcome` are plumbing. They carry no domain meaning
and do not appear in any spec.

### 4. Cleanup fan-out, serialised per repo

One shared async helper in `src/feed/mod.rs`, called by both feed paths (each
already holds an `Arc<dyn ProcessRunner>`). Per removed task it mirrors
`src/runtime/tasks.rs::exec_cleanup`: check `has_other_tasks_with_worktree` first —
a shared worktree is detached, never removed — then run
`src/dispatch/worktree.rs::cleanup_task`. Failures warn to `app.log` rather than
surfacing; feed reconciliation is background work.

**Removed tasks are grouped by `repo_path` and torn down sequentially within a
repo**, with different repos free to run in parallel. `cleanup_task` shells
`git -C <repo> worktree remove --force` followed by a best-effort `git branch -D`
against the shared checkout. Today that fires once per cleanup; here it fires once
per removed task, and a Reviews epic's tasks overwhelmingly share one `repo_path`.
A mass-merge deleting a dozen tasks at once would otherwise contend on
`.git/index.lock` and produce spurious failures that do not exist today.

## Testing

Spec first, then tests, then code.

**The guard predicate** — inline unit tests in `src/feed/exec.rs`: fires on zero
items with stderr; does not fire on zero items without stderr; does not fire on a
non-empty emission regardless of stderr.

**The two feed paths:**

- `src/runtime/tests.rs::exec_trigger_epic_feed_reports_stderr_written_on_zero_exit`
  **inverts**. It drives `echo 'Invalid search query' >&2; echo '[]'` and today
  asserts `FeedMessage::Refreshed { count: 0, wrote_stderr: true }`. It must now
  assert `FeedMessage::Failed`.
- A new auto-poll counterpart in `src/feed/mod.rs`: a zero-item emission with
  stderr leaves pre-existing subtree tasks untouched. This is the direct
  regression test for the reported bug.

**Must stay green, unmodified — the false-positive boundary:**

- `src/runtime/tests.rs::exec_trigger_epic_feed_zero_items` and
  `exec_trigger_epic_feed_quiet_command_reports_no_stderr` both drive a quiet
  `echo '[]'`. A genuinely-empty clean run must still reconcile.
- `src/feed/mod.rs::tick_stderr_on_zero_exit_does_not_suppress_sync` emits **one**
  item alongside stderr. The guard fires only at zero items, so this test is
  unaffected. Inverting it would itself be a regression.

**The DB layer** — `src/db/tests/tasks.rs`: both delete sites return the rows they
removed, restricted to rows carrying a worktree or tmux window; manual tasks
(`external_id IS NULL`) are still never touched.

**The cleanup fan-out** — `MockProcessRunner` tests asserting `git worktree remove`
and `tmux kill-window` argv, the shared-worktree skip, and that two removed tasks
sharing a `repo_path` are torn down sequentially rather than concurrently.

**The ordering invariant** — a task moved between role sub-epics during a sync
never appears in `removed`. The current ordering is safe: `apply_move`'s
`set_task_epic_id` lands before `upsert_role_groups`, `delete_stale_subtree` and
`clear_parent_stranded_tasks`, each of whose SQL filters on the task's *current*
`epic_id`. But that safety rests on hand-verified ordering across five
non-transactional `await` points with nothing pinning it. Before this change a
mis-ordering deleted a row; after it, it force-removes a live agent's worktree.
That warrants a test, not a doc comment.

## Out of scope

**Concurrent syncs of the same epic.** Nothing serialises a manual `r` refresh
against an in-flight poll tick for the same epic, so the two can interleave between
the non-transactional steps above. Pre-existing, but this change raises its cost
from a lost row to a destroyed worktree. Follow-up task.

**Parse drift between the two paths.** `exec_trigger_epic_feed` uses
`serde_json::from_slice` while `FeedJob::run` uses `parse::parse_feed_items`, which
warns on unknown tags. Real drift, inherited from #3900's scope note, unchanged
here.
