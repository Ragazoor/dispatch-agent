# #4096 — Archive/delete leaks the tmux window of a task that has no worktree

## The defect, restated against the current tree

`App::take_cleanup` (`src/tui/mod.rs::take_cleanup`) matches on `task.worktree.take()`:

- `Some(wt)` → emits `TaskCommand::Cleanup { worktree: wt, tmux_window, follow_up }`.
- `None` → drops `task.tmux_window` from the board **and returns `None`**.

Nothing is queued in the `None` arm, so `exec_cleanup` (`src/runtime/tasks.rs::exec_cleanup`)
never runs and `tmux kill-window` is never issued. A task with a **tmux window but no
worktree** therefore keeps its window alive forever after archive, delete, epic-delete, or
retry-fresh, while the board (and, for archive/delete, the row) forgets the window's name —
an unattributable orphan.

`TaskTeardown` (head of the `-- == Archive ==` section of `docs/specs/tasks.allium`) states
step 1 — "kill the task's tmux window if it has one" — unconditionally. Gating the whole
teardown on the worktree's presence violates it.

The feed path added by #3989 (`src/feed/mod.rs::cleanup_removed_feed_tasks`) does handle the
window-only shape: it branches on `worktree` and calls `crate::tmux::kill_window_if_present`
when it is `None`. So the same row shape is torn down correctly by a feed sync and leaks by
an archive/delete. Two wrappers around one primitive have diverged, and the newer one is the
more complete.

**Which rows have this shape.** The two columns are independent — nothing in the schema or the
service layer ties `tmux_window`'s presence to `worktree`'s — so the pair is reachable
whenever the two writes come apart: a dispatch whose window creation succeeded and whose
worktree pointer write did not land, a cleanup that cleared `worktree` while the window
outlived it, a hand-recovered row. I have not enumerated a specific production sequence and
the plan does not depend on one: `TaskTeardown` step 1 is unconditional, and the feed path
already treats the shape as routine enough to carry a dedicated arm and two tests
(`cleanup_kills_window_only_when_there_is_no_worktree`,
`cleanup_of_a_stateless_row_runs_no_commands`). The divergence between the two wrappers is the
defect regardless of the row's provenance.

## Scope note on the related tasks

- **#4092** (`exec_cleanup`'s shared-worktree branch calls `detach_only`) is already gone:
  commit `6e0787ba` ("refactor(cleanup): drop the unreachable shared-worktree machinery")
  removed it — `grep -rn detach_only src/` is empty. Nothing to fix there.
- **#4093** (is the shared-worktree machinery reachable) was answered by the same commit and
  by `WorktreeIsNeverShared` in `docs/specs/tasks.allium`.
- That spec block ends with a standing instruction for this task: *"When the two teardown
  wrappers are unified into one primitive, the tripwires must collapse to one deliberately
  rather than leaving an orphan behind."* This plan does that collapse.

## Design

### One primitive

Replace `src/dispatch/worktree.rs::cleanup_task` with

```rust
pub fn teardown_task(
    repo_path: &str,
    worktree_path: Option<&str>,
    tmux_window: Option<&str>,
    runner: &dyn ProcessRunner,
) -> Result<()>
```

Body, in `TaskTeardown` order:

1. `if let Some(window) = tmux_window { tmux::kill_window_if_present(window, runner)? }` —
   unchanged from today, including the `?` (a kill failure aborts before the removal; pinned
   by `cleanup_task_kill_window_failure_propagates`).
2. `let Some(worktree_path) = worktree_path else { return Ok(()) };` then today's
   `git worktree remove --force` + already-removed tolerance + best-effort `git branch -D`,
   verbatim.

`cleanup_task` has exactly two callers (`exec_cleanup`, `cleanup_removed_feed_tasks`), both
of which become one call each, so this is a replacement, not an added wrapper. Rename rather
than keep both: a second entry point is how the two policies diverged in the first place.

### Where the gate stays

The follow-up gate is keyed on **step 2 only**, which is what both spec rules already say:

- `ArchiveTask`/`DeleteTask` bind `let released = removed = worktree_removed(task)` —
  vacuously true when the task had no worktree.
- `DeleteTask`'s guidance: *"A task with no worktree has nothing to release, so it is deleted
  immediately."*

So a **window-only** teardown must apply its follow-up regardless of whether the kill
succeeded — there is no worktree to strand and nothing to retry, and withholding the follow-up
would newly gate a row delete on a tmux call, which no spec rule asks for. `exec_cleanup`
becomes:

```rust
let msg = match (teardown_task(&repo_path, worktree.as_deref(), tmux_window.as_deref(), &*runner), worktree) {
    (Ok(()), _) => CleanupSucceeded { id, follow_up },
    // Nothing to release: the gate does not apply. Warn-logged, like the feed path.
    (Err(e), None) => { tracing::warn!(...); CleanupSucceeded { id, follow_up } }
    (Err(e), Some(worktree)) => { tracing::error!(...); CleanupFailed { id, worktree, error } }
};
```

`TaskMessage::CleanupFailed { worktree: String }` keeps its non-optional field, because only
the worktree arm can produce it. `handle_cleanup_failed`'s message text stays valid.

### Command plumbing

`TaskCommand::Cleanup.worktree` becomes `Option<String>`; `exec_cleanup`'s `worktree`
parameter likewise. `take_cleanup` emits the command whenever the task owns a worktree **or**
a window, and returns `None` only for a task that owns neither:

```rust
let worktree = task.worktree.take();
let tmux_window = task.tmux_window.take();
if worktree.is_none() && tmux_window.is_none() { return None; }
Some(Command::Task(TaskCommand::Cleanup { id: task.id, repo_path: task.repo_path.clone(), worktree, tmux_window, follow_up }))
```

Consequences at the four call sites, all desirable and all already correct as written:

- `handle_archive_task` — pushes the cleanup, still persists the pre-clear snapshot; the
  `ClearPointer` follow-up now arrives via `CleanupSucceeded` for the window-only case too.
- `handle_delete_task` — the `Some(_)` arm now covers window-only rows, so the row delete
  becomes the cleanup's `DeleteRow` follow-up instead of an immediate `Delete`. Behaviourally
  identical in outcome, because the window-only path always reports `CleanupSucceeded` (see
  the gate above). Its doc comment needs rewording: "with no worktree there is nothing to
  release and the delete is immediate" becomes "a task owning neither worktree nor window is
  deleted immediately".
- `handle_delete_epic` (`Nothing`) and `handle_retry_fresh` (`ClearPointer`) — unchanged
  code, now reached for window-only rows.

### Feed path

`cleanup_removed_feed_tasks` loses its `match &task.worktree` and calls
`teardown_task(&task.repo_path, task.worktree.as_deref(), task.tmux_window.as_deref(), &*runner)`,
warn-logging any error. The "row owning neither runs no commands" property is preserved by
the primitive itself (both `if let`s skip), and is what
`cleanup_of_a_stateless_row_runs_no_commands` asserts.

## Steps (TDD — spec, then tests, then code)

### Step 1 — Spec (`allium:tend`)

`docs/specs/tasks.allium`, `-- == Archive ==` header block:

1. State the missing clause explicitly, as a named rule so tests can cite it —
   **`TeardownIsOwedWheneverThereIsSomethingToRelease`**: `TaskTeardown` is performed when the
   task owns a worktree **or** a tmux window; only a task owning neither runs no commands. A
   window without a worktree gets step 1 and nothing else; a worktree without a window gets
   steps 2–3.
2. Extend `WorktreeReleaseIsGated`: the gate is keyed on step 2. When there is no worktree,
   `released` is vacuously true, so the requesting operation's follow-up is applied even if
   step 1 failed; that failure is warn-logged, not surfaced, and never withholds the row
   delete or the pointer clear.
3. Rewrite the "How the feed path reads step 3" paragraph as a path-independent statement
   about the shared primitive (step 3 is reachable only through the worktree arm, on every
   path, because that is where `git branch -D` lives).
4. Repoint every implementation reference: `src/dispatch/worktree.rs::cleanup_task` →
   `::teardown_task` (lines ~513, ~557, ~564, ~569), and note that `exec_cleanup` and
   `cleanup_removed_feed_tasks` are now policy wrappers (gating vs warn-logging) over it.
5. Collapse the `WorktreeIsNeverShared` tripwire pair as that block instructs: keep the
   `exec_cleanup` tripwire (`exec_cleanup_tears_down_even_if_another_row_names_the_worktree`,
   the path with a store handle in scope, i.e. the only one where a guard could be
   reintroduced cheaply), drop the feed twin, and update the coverage prose to say so.
6. `docs/specs/feeds.allium` — check its removal clauses for `cleanup_task` mentions and
   repoint.

Run `allium check` on both files. Do **not** change `ArchiveTask`/`DeleteTask` `ensures`
clauses; they already specify the desired behaviour.

### Step 2 — Primitive tests (`src/dispatch/tests.rs`)

Rename existing `cleanup_task_*` tests to `teardown_task_*` and adapt their call sites to the
`Option` worktree. New tests (red before Step 3):

- `teardown_task_kills_window_when_there_is_no_worktree` — `worktree: None`,
  `tmux_window: Some("task-42")` → asserts a `kill-window` argv and **no** `git worktree
  remove`.
- `teardown_task_with_neither_worktree_nor_window_runs_no_commands`.
- Keep `teardown_task_kill_window_failure_propagates` (worktree present) and add
  `teardown_task_window_only_kill_failure_is_an_error` — the primitive still returns `Err`;
  deciding what to do with it is the caller's job.

### Step 3 — Implement the primitive

`src/dispatch/worktree.rs`: `cleanup_task` → `teardown_task` with the `Option` worktree and
the early return; update the `pub use` in `src/dispatch/mod.rs`. Update the two call sites
mechanically so the tree compiles (real behaviour changes land in Steps 5 and 7). Step 2
green.

### Step 4 — Runtime + TUI tests (red)

`src/runtime/tests.rs`:

- `exec_cleanup_kills_the_window_of_a_task_with_no_worktree` — `worktree: None`, window set →
  `kill-window` issued, no `git worktree remove`, and `CleanupSucceeded` reported with the
  follow-up intact.
- `exec_cleanup_window_only_kill_failure_still_applies_the_follow_up` — scripted kill failure
  → `CleanupSucceeded`, **not** `CleanupFailed` (this is the gate clause from Step 1.2).
- Keep the existing worktree-present failure test asserting `CleanupFailed`.

`src/tui/tests/archive.rs`:

- `archive_task_with_window_but_no_worktree_emits_cleanup` — asserts the emitted command
  carries `worktree: None` and `tmux_window: Some(..)` (destructure; a `matches!(.., ..)`
  would pass before the fix if only presence were checked).
- `delete_task_with_window_but_no_worktree_tears_down_the_window` — asserts a `Cleanup` with
  `follow_up: DeleteRow` and **no** sibling `Delete`, then feeds `CleanupSucceeded` and
  asserts the `Delete` arrives.
- `archive_task_with_neither_worktree_nor_window_emits_no_cleanup` — keeps the existing
  `archive_task_without_worktree_no_cleanup` guarantee for the truly stateless row (rename or
  keep both; `make_task` sets neither field, so today's test already covers it).

`src/tui/tests/epics.rs` / `navigation.rs`: one window-only case each for the epic-delete
(`Nothing`) and delete-from-archive paths, mirroring the existing tests.

`tests/tmux_lifecycle.rs`: `teardown_task_window_only_removes_the_real_window` — create a
real window via the harness, call `teardown_task` with `worktree: None`, assert
`list-windows` no longer names it. Per learning #327 a `MockProcessRunner` argv assertion can
pin a broken command string and stay green; this is the layer that proves the window is
actually gone.

### Step 5 — Implement the command plumbing

`TaskCommand::Cleanup.worktree: Option<String>` (`src/tui/commands/task.rs`, doc comment
updated for the gate wording), `src/runtime/commands.rs` dispatch arm, `exec_cleanup`
signature + the `(result, worktree)` match with its `warn!`/`error!` split, and
`take_cleanup`'s new body + doc comment. Reword `handle_delete_task`'s doc comment. Step 4
green.

### Step 6 — Feed test adjustment (red/green)

Drop `cleanup_removes_the_worktree_even_if_another_row_names_it` (the collapsed tripwire from
Step 1.5) and keep `cleanup_removes_worktree_and_kills_window`,
`cleanup_kills_window_only_when_there_is_no_worktree`,
`cleanup_of_a_stateless_row_runs_no_commands`, `cleanup_continues_after_a_failure`,
`cleanup_serialises_same_repo_removals` — they must all stay green through Step 7, which is
the point: the feed path's behaviour is unchanged, only its implementation collapses.

### Step 7 — Route the feed path through the primitive

Replace the `match &task.worktree` in `cleanup_removed_feed_tasks` with the single
`teardown_task` call; rewrite its doc comment (the "How the feed path reads step 3" prose and
the "no shared-worktree check here" argument move to referencing the primitive and the spec).

### Step 8 — Verify

- `cargo test` (full suite; needs `tmux` on `PATH` — confirm the `tmux_*` targets did not
  print `skipping`).
- `cargo clippy --all-targets -- -D warnings`, `cargo fmt`.
- `./scripts/check-doc-paths.sh` and `./scripts/check-doc-symbols.sh` — both will fail on any
  surviving `cleanup_task` reference in `docs/specs/*.allium`, `docs/*.md`, or a doc comment,
  which is the safety net for Step 1.4.
- `allium:weed` over `tasks.allium` + `feeds.allium` to confirm spec/code alignment.
- `git log --oneline HEAD..main`; merge `main` and re-run if non-empty.

## Out of scope

- Any change to `ArchiveTask`/`DeleteTask` postconditions.
- Surfacing a window-only kill failure in the status bar (warn log only, matching the feed
  path). If we later want it, it is an additive `SystemMessage`, not a gate.
- `ConfirmDone`'s window-only kill (`take_detach` → `KillTmuxWindow`) — a different rule
  (worktree deliberately retained), already correct.
