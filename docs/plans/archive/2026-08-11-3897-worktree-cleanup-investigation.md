# Task #3897 — Are worktrees cleaned up on archive? Investigation + plan

Date: 2026-08-11

## Answer

**Yes — the ordinary archive path does remove the worktree.** Archiving a task
emits a `Cleanup` command that runs `git worktree remove --force` plus a
best-effort `git branch -D`. This holds for the single-task archive, batch
archive, and epic archive, and for delete.

- `handle_archive_task` (`src/tui/update/retry.rs:108`) calls
  `App::take_cleanup` (`src/tui/mod.rs:1884`) and emits
  `TaskCommand::Cleanup` before `Persist`.
- `handle_archive_epic` (`src/tui/update/epics.rs:166`) walks
  `descendant_epic_ids` and archives every subtask through the same handler.
- `handle_batch_archive_tasks` (`src/tui/update/selection.rs:172`) delegates to
  the same handler.
- `exec_cleanup` (`src/runtime/tasks.rs:748`) → `dispatch::cleanup_task`
  (`src/dispatch/worktree.rs:501`) does the tmux kill + `git worktree remove
  --force` + `git branch -D`.
- Existing coverage: `archive_task_with_worktree_emits_cleanup`
  (`src/tui/tests/archive.rs:278`).

Empirical checks that back this up:

- `app.log` records 1200 `cleaning up task` events, and none of the recently
  cleaned paths (e.g. `.worktrees/3203-wp3-dispatch-tmux-test-coverage`,
  `.worktrees/3192-quick-task`) still exist on disk.
- `git worktree remove --force` was exercised against a scratch repo for the
  states a dispatch worktree actually reaches (untracked files, a `target/`
  build dir, a nested independent git repo, a dirty tracked file). All four
  removed cleanly, exit 0. Only a *locked* worktree fails (needs `-f -f`), and
  dispatch never locks a worktree.
- `tasks` rows with `status='archived' AND worktree IS NOT NULL`: **0**.

So the reported symptom is not the main archive path. It is one of the three
holes below. All three are confirmed, not inferred.

## Confirmed defects

### 1. Deleting an epic leaks every sub-epic subtask's worktree

`EpicCrud::delete_epic` deletes the **whole subtree**:
`delete_epic_recursive` (`src/db/queries/epics.rs:347`) walks
`parent_epic_id` depth-first and runs `DELETE FROM tasks WHERE epic_id = ?1`
for each descendant epic (covered by `delete_epic_multi_level_sub_epics`,
`src/db/tests/epics.rs:180`).

But `handle_delete_epic` (`src/tui/update/epics.rs:79`) only collects
`t.epic_id == Some(id)` — **direct children only**. A sub-epic's subtasks have
their rows deleted with no `Cleanup` emitted, so their worktree directory,
branch and tmux window survive with nothing left in the DB that references
them. They are unreachable from the UI forever.

`handle_delete_epic` also only retains-out direct children from
`board.tasks`/`board.epics`, so the in-memory board briefly holds tasks
pointing at deleted epics (self-heals on the next `RefreshFromDb`).

Confirmed with this test (run against `HEAD`, then reverted so the tree stays
green):

```rust
#[test]
fn delete_epic_cleans_up_worktrees_of_sub_epic_subtasks() {
    let mut app = App::new(vec![]);
    let mut child_epic = make_epic(20);
    child_epic.parent_epic_id = Some(EpicId(10));
    app.board.epics = vec![make_epic(10), child_epic];

    let mut direct = make_task(1, TaskStatus::Running);
    direct.epic_id = Some(EpicId(10));
    direct.worktree = Some("/repo/.worktrees/1-direct".to_string());
    let mut nested = make_task(2, TaskStatus::Running);
    nested.epic_id = Some(EpicId(20));
    nested.worktree = Some("/repo/.worktrees/2-nested".to_string());
    app.board.tasks = vec![direct, nested];

    let cmds = app.update(Message::Epic(crate::tui::messages::EpicMessage::Delete(
        EpicId(10),
    )));

    let cleaned: Vec<&str> = cmds
        .iter()
        .filter_map(|c| match c {
            Command::Task(crate::tui::commands::TaskCommand::Cleanup { worktree, .. }) => {
                Some(worktree.as_str())
            }
            _ => None,
        })
        .collect();
    assert!(
        cleaned.contains(&"/repo/.worktrees/2-nested"),
        "sub-epic subtask worktree must be cleaned up, got: {cleaned:?}"
    );
}
```

Observed failure:

```
sub-epic subtask worktree must be cleaned up, got: ["/repo/.worktrees/1-direct"]
```

Note: `docs/specs/epics.allium:272-279` (`DeleteEpic` guidance) already
*describes* this asymmetry ("cleanup for nested sub-epic descendants is not
performed by the TUI"). So this is a documented gap, not spec drift — but it is
still an unbounded resource leak with no recovery path, and the spec offers no
rationale for it. `ArchiveEpic` recurses correctly; `DeleteEpic` should too.

### 2. A dispatch that fails *after* `git worktree add` leaves an orphan

`provision_worktree` (`src/dispatch/worktree.rs:340`) creates the worktree at
line 463, then does `tmux new-window`, `set_window_dispatch_dir` and
`ensure_split_hook`; `dispatch_with_prompt` then writes `.claude-prompt` and
sends keys (`src/dispatch/agents.rs:159-192`). Any failure from that point on
returns `Err` with **no rollback**, and the task's `worktree` column is only
written once dispatch returns `Ok` (`src/mcp/handlers/tasks/dispatch.rs:126`,
`handle_dispatched` in `src/tui/update/lifecycle.rs:255`). The claim is
released and the task goes back to Backlog with `worktree = null`, while the
directory and branch stay on disk. Archiving that task cleans nothing —
`take_cleanup` returns `None`.

Two consequences:

- If the task is later re-dispatched with the **same** title, provisioning
  silently takes the reuse path (`reused_worktree = path.exists()`, line 357),
  which also downgrades the fetch from `Required` to `BestEffort`. So a failed
  dispatch quietly changes the next dispatch's semantics. That contradicts
  `docs/specs/dispatch.allium:183-186`, which claims the release leaves the task
  "dispatchable exactly as before".
- If the title is edited first, the slug changes and the old directory is
  orphaned permanently.

Confirmed with this test (also reverted):

```rust
#[test]
fn provision_worktree_rolls_back_the_worktree_when_a_later_step_fails() {
    let (_dir, repo_path) = make_test_repo();

    let mock = MockProcessRunner::new(vec![
        MockProcessRunner::ok(),                      // git worktree add
        MockProcessRunner::fail("no server running"), // tmux new-window
    ]);

    let task = make_task(&repo_path);
    let result = provision_worktree(&task, &mock, None, SUBPROCESS_TIMEOUT);
    assert!(result.is_err(), "tmux failure must abort provisioning");

    let calls = mock.recorded_calls();
    assert!(
        calls.iter().any(|(prog, args)| prog == "git"
            && args.contains(&"worktree".to_string())
            && args.contains(&"remove".to_string())),
        "the created worktree must be removed on the failure path, got: {calls:?}"
    );
}
```

Observed failure — only the `add` and the failed `new-window` are recorded, no
removal is attempted:

```
the created worktree must be removed on the failure path, got: [("git", ["-C",
"…", "worktree", "add", "…/.worktrees/42-fix-bug", "-B", "42-fix-bug"]),
("tmux", ["new-window", "-d", "-n", "task-42", "-c", "…"])]
```

### 3. A failed cleanup is invisible and unrecoverable

`take_cleanup` clears `task.worktree` in memory and `exec_persist_task`
(`src/runtime/tasks.rs:185`) writes that `NULL` to the DB unconditionally — the
`Persist` is not gated on the `Cleanup` succeeding, and the two run
independently (`exec_cleanup` does its work in a detached `spawn_blocking`,
`src/runtime/tasks.rs:777`).

If `git worktree remove` fails, `cleanup_task` bails *before* `git branch -D`
(`src/dispatch/worktree.rs:527`), so both the directory and the branch survive,
while the DB has already forgotten the path. The only signal is a
`SystemMessage::Error` in the status bar, which auto-clears after
`STATUS_MESSAGE_TTL` (5s) — and nothing is written to `app.log`, so there is no
post-hoc trace at all. `docs/specs/tasks.allium:481-483` states
`ArchiveTask` ensures `not exists task.worktree` unconditionally; the
implementation can violate that postcondition silently.

This is the mechanism that turns any transient failure into a permanent orphan,
and it is consistent with what `.worktrees/` looks like in this repo today: 37
directories on disk vs 25 registered worktrees, with several of the unregistered
leftovers containing only a `.claude/settings.local.json`.

Caveat, stated explicitly: those specific leftovers date from March 2026 and
their directory slugs no longer match the task ids in the current DB (which has
`.bak` files from at least three resets), so I cannot attribute any individual
orphan to a specific code path. The three defects above are confirmed from code
and tests; the on-disk residue is corroborating, not proof of provenance.

## Non-defects (checked, working as specified)

- Review→Done (`ConfirmDone`) kills only the tmux window and keeps the
  worktree, so the task stays resumable — `docs/specs/tasks.allium:309`. The
  later archive cleans it. (Knowledge-base entry #298 confirmed.)
- `wrap_up`/`exit_session` keep the worktree: `close_session`
  (`src/service/tasks/crud.rs:264`) clears `tmux_window` only.
- MCP cannot archive at all (`src/mcp/handlers/tasks/crud.rs:30`), so there is
  no agent-driven archive path that bypasses cleanup.
- Shared worktrees detach instead of removing (`exec_cleanup`), as specified.
  `has_other_tasks_with_worktree` (`src/db/queries/tasks.rs:217`) excludes
  `done` rows, so a Done task holding the same worktree does not block removal;
  its stale `worktree` pointer is then handled by the `WORKTREE_ALREADY_REMOVED`
  branch in `cleanup_task`.

## Implementation plan

TDD throughout: each step writes the test first, watches it fail, then makes it
pass. Spec changes land before the tests they describe.

### Step 1 — `DeleteEpic` recurses (defect 1)

1. **Spec** (`allium:tend`): rewrite `DeleteEpic`'s guidance in
   `docs/specs/epics.allium` so cleanup covers the full subtree, matching
   `ArchiveEpic`. Remove the "cleanup for nested sub-epic descendants is not
   performed by the TUI" sentence. Run `allium:weed`.
2. **Test** (`src/tui/tests/epics.rs`): add
   `delete_epic_cleans_up_worktrees_of_sub_epic_subtasks` exactly as above.
   Add a second test asserting the whole subtree leaves `board.tasks` and
   `board.epics`.
3. **Code** (`src/tui/update/epics.rs:79`): collect
   `descendant_epic_ids(id, &self.board.epics)` (already used by
   `handle_archive_epic`), take cleanup for every task in that set, and retain
   out both the subtree's epics and their tasks.

### Step 2 — roll back a half-provisioned worktree (defect 2)

1. **Spec**: in `docs/specs/dispatch.allium`, state that a provisioning failure
   after the worktree is created removes what it created, so "dispatchable
   exactly as before" is literally true. Keep the reuse path untouched — a
   *reused* worktree is pre-existing and must never be removed on failure.
2. **Test** (`src/dispatch/tests.rs`): add
   `provision_worktree_rolls_back_the_worktree_when_a_later_step_fails` as
   above, plus the negative twin —
   `provision_worktree_does_not_remove_a_reused_worktree_on_failure` (build it
   with `make_test_repo_with_worktree`, fail `tmux new-window`, assert no `git
   worktree remove` is issued). Extend to the `dispatch_with_prompt` layer for
   the `.claude-prompt` write and `send_keys` failures.
3. **Code**: in `provision_worktree`, wrap everything after a *fresh*
   `git worktree add` so that on `Err` it issues a best-effort
   `git worktree remove --force` + `git branch -D` (reuse `cleanup_task` with
   `tmux_window: None`, or a small `rollback_fresh_worktree` helper) and returns
   the original error with the rollback outcome attached to the log line. Do the
   same for the failure paths in `dispatch_with_prompt` that occur after
   provisioning returned `Ok`.

### Step 3 — make a failed cleanup visible (defect 3)

1. **Spec**: `docs/specs/tasks.allium` — `ArchiveTask`/`DeleteTask` currently
   promise `not exists task.worktree` unconditionally. Either weaken it to
   best-effort with a stated observable consequence, or (preferred) keep the
   promise and specify that a failed removal is retried/reported. Decide with
   the user before writing — this is a behaviour question, not a wording one.
2. **Test**: in `src/runtime/tests.rs`, script a runner whose `git worktree
   remove` fails and assert the failure is surfaced durably (not only via the
   5s status message). Mechanism depends on the choice in (1).
3. **Code** (`src/dispatch/worktree.rs`, `src/runtime/tasks.rs:777`): at
   minimum add `tracing::error!(worktree_path, error = …)` on the failure branch
   so `app.log` keeps a record — the current silence is why the existing
   leftovers cannot be attributed. If the spec keeps the strong postcondition,
   gate the `worktree` column clear on the removal succeeding so the task
   retains a retryable pointer.

### Step 4 — reap existing orphans (optional, needs user sign-off)

A `dispatch worktrees prune <repo>` CLI that lists `.worktrees/*` directories
with no live task row and no git registration, and removes them only after
explicit confirmation. Out of scope for this task unless the user asks — it
deletes user data, so it must not be implicit.

## Verification

`cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh` for each step,
plus `cargo clippy --all-targets -- -D warnings` (pre-push hook) and
`allium:weed` after any spec edit.
