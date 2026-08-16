# Feed Empty-Emission Guard and Task Teardown Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop a degraded feed command (exit 0, empty array, error on stderr) from deleting every feed task in a `reviews_parent` subtree, and make every feed-driven task removal tear down the task's worktree and tmux window instead of orphaning them.

**Architecture:** Two independent halves. (1) A shared predicate in `src/feed/exec.rs` classifies a zero-item emission that wrote to stderr as a feed-command failure, applied after parse and before sync in both the auto-poll and manual-refresh paths. (2) Both feed stale-delete SQL statements switch to `DELETE ... RETURNING`, so the rows they removed flow back up through the ingest pipeline to a cleanup helper that runs the same teardown `ArchiveTask`/`DeleteTask` use.

**Tech Stack:** Rust 2021, tokio, rusqlite 0.32 (bundled SQLite 3.46), `tokio_rusqlite`, ratatui, Allium specs.

## Global Constraints

- **Spec first, then tests, then code.** `docs/specs/*.allium` is the source of truth. Task 1 lands the spec before any code task runs.
- **TDD throughout.** Every code task writes a failing test, runs it to confirm the failure, then implements.
- **Inline test modules need `#![allow(clippy::unwrap_used, clippy::expect_used)]`** at the top — the workspace `-D warnings` policy rejects bare `unwrap()`/`expect()` otherwise. See `src/db/tests/mod.rs`.
- **No `tokio::time::sleep` anywhere under `src/` or `tests/`**, and no `std::thread::sleep` in test files. `./scripts/check-no-test-sleep.sh` enforces this in the pre-push hook.
- **Mutation boundary:** the feed subsystem is a *sanctioned* direct-mutation consumer, so `db.patch_task(...)` and friends from `src/feed/` are not violations.
- **Verify command** (run before declaring work complete):
  `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`
- **Clippy is only a hard error via the pre-push hook** (`cargo clippy --all-targets -- -D warnings`). A green `cargo build` does not imply clippy-clean.
- Prefer the `path::symbol` form over `file:NN` in doc comments and specs — line citations rot silently.

---

### Task 1: Spec — `DegradedEmptyEmission` and the named teardown concept

Spec-first. No code changes in this task.

**Files:**
- Modify: `docs/specs/feeds.allium` (`FeedCommandStderrOnSuccess`, `FeedCommandFailure`, `ManualFeedTrigger`, `RoleRoutedFeedSync`, `GroupedFeedUpsert`, `FlatFeedReconcile`)
- Modify: `docs/specs/tasks.allium` (`ArchiveTask`, `DeleteTask`)
- Reference: `docs/superpowers/specs/2026-08-12-feed-empty-emission-guard-design.md`

**Interfaces:**
- Consumes: nothing.
- Produces: the rule name `DegradedEmptyEmission` and the teardown concept name, both cited by doc comments in Tasks 2–7.

- [ ] **Step 1: Read the design doc and the current spec text**

Read `docs/superpowers/specs/2026-08-12-feed-empty-emission-guard-design.md` in full, then the four `feeds.allium` rules named above and the two `tasks.allium` rules.

- [ ] **Step 2: Invoke the `allium:tend` skill for the spec edits**

Use the skill rather than hand-editing — it enforces Allium syntax and structure.

Edits required:

1. **New rule `DegradedEmptyEmission`** in `feeds.allium`, in the "Failure handling" section immediately after `FeedCommandStderrOnSuccess`. It fires when a feed command exits 0, its stdout parses to **zero** items, and its stderr is non-empty. Effect: the emission is treated as `FeedCommandFailed` — logged at warn, `last_run` bumped, **no sync runs at all**, existing tasks untouched. State explicitly that a zero-item emission with *empty* stderr still reconciles normally and still removes merged/closed PRs, and that a non-empty emission syncs normally regardless of stderr.

2. **Amend `FeedCommandStderrOnSuccess`.** Its current claim is now false for the zero-item case:
   > This is diagnostic only. The sync proceeds exactly as it would for a command that wrote nothing to stderr — in particular an empty emission still reconciles, and still removes tasks absent from it.

   Replace with: diagnostic only **when the emission is non-empty**; a zero-item emission with stderr is `DegradedEmptyEmission`. Record that this reverses a deliberate #3900 decision, and record the accepted cost: a script that writes to stderr on every run *and* legitimately emits `[]` is treated as failed indefinitely and never reconciles.

3. **Amend `FeedCommandFailure`** — it enumerates three buckets; add a fourth, "zero items emitted while stderr is non-empty (`DegradedEmptyEmission`)".

4. **Amend `ManualFeedTrigger`** — delete the `" — command wrote to stderr (see app.log)"` suffix clause and the `count = 0 AND wrote to stderr` condition attached to it. That combination now produces the failure status line instead.

5. **New named teardown concept in `tasks.allium`.** `ArchiveTask` and `DeleteTask` each currently carry their own copy of:
   ```
   ensures:
       if task.worktree != null:
           not exists task.worktree
   ```
   plus guidance about the tmux window, best-effort branch deletion, and the shared-worktree detach rule. Factor that into one named concept covering: kill the tmux window if present; remove the git worktree if present **unless another active task shares it**, in which case detach from this task only and leave the worktree on disk; best-effort branch deletion. Have `ArchiveTask` and `DeleteTask` reference it. **Their behaviour must not change** — this is a factoring of already-specified behaviour.

6. **Amend the feed removal clauses** in `RoleRoutedFeedSync`, `GroupedFeedUpsert` and `FlatFeedReconcile` to state that removing a feed task performs that same teardown. Today they say only `not exists task`.

- [ ] **Step 3: Validate the specs**

Run: `allium check docs/specs/feeds.allium` and `allium check docs/specs/tasks.allium`
Expected: both pass with no errors.

- [ ] **Step 4: Verify doc paths**

Run: `./scripts/check-doc-paths.sh`
Expected: PASS. Every `src/…` path cited in the new spec text must exist. Prefer `path::symbol` citations over `file:NN`.

- [ ] **Step 5: Commit**

```bash
git add docs/specs/feeds.allium docs/specs/tasks.allium
git commit -m "spec(feeds): add DegradedEmptyEmission and name task teardown"
```

---

### Task 2: The guard predicate

**Files:**
- Modify: `src/feed/exec.rs` (add `degraded_empty_emission`, and a test in the existing `mod tests`)

**Interfaces:**
- Consumes: nothing.
- Produces: `pub(crate) fn degraded_empty_emission(item_count: usize, stderr: &str) -> Option<String>` — returns `Some(reason)` only when `item_count == 0 && !stderr.is_empty()`. The `reason` embeds the stderr text so the manual path can put it in the status bar. Consumed by Tasks 3 and 4.

- [ ] **Step 1: Write the failing tests**

Add to the existing `mod tests` at the bottom of `src/feed/exec.rs`:

```rust
// --- degraded_empty_emission (feeds.allium: DegradedEmptyEmission) ---

#[test]
fn degraded_when_zero_items_and_stderr_present() {
    let reason = degraded_empty_emission(0, "Invalid search query")
        .expect("zero items plus stderr must be treated as a failure");
    assert!(
        reason.contains("Invalid search query"),
        "reason must carry the stderr so the status bar can show it, got: {reason}"
    );
}

#[test]
fn not_degraded_when_zero_items_and_no_stderr() {
    // A genuinely-empty clean run must still reconcile — this is the
    // false-positive boundary the guard must never cross.
    assert!(degraded_empty_emission(0, "").is_none());
}

#[test]
fn not_degraded_when_items_present_despite_stderr() {
    // Chatter alongside a real emission stays diagnostic-only.
    assert!(degraded_empty_emission(3, "some warning").is_none());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test feed::exec::tests::degraded -- --include-ignored`
Expected: FAIL to compile — `cannot find function degraded_empty_emission`.

- [ ] **Step 3: Write the implementation**

Add to `src/feed/exec.rs`, directly below `exec_feed_command`:

```rust
/// Classify a zero-exit emission that produced no items while writing to
/// stderr. That combination is the signature of a script which failed
/// internally and soft-failed to an empty array — syncing it would delete
/// every feed task in the epic's subtree.
///
/// Returns `Some(reason)` to suppress the sync, `None` to proceed.
///
/// A genuinely-empty clean run (`stderr` empty) returns `None` and reconciles
/// normally, so merged and closed PRs are still removed. A non-empty emission
/// returns `None` regardless of stderr — there, stderr is chatter and the warn
/// line from `exec_feed_command` is enough.
///
/// See feeds.allium: DegradedEmptyEmission.
pub(crate) fn degraded_empty_emission(item_count: usize, stderr: &str) -> Option<String> {
    if item_count == 0 && !stderr.is_empty() {
        Some(format!(
            "command emitted no items but wrote to stderr: {stderr}"
        ))
    } else {
        None
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test feed::exec::tests::degraded`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
git add src/feed/exec.rs
git commit -m "feat(feeds): add degraded_empty_emission guard predicate"
```

---

### Task 3: Wire the guard into the auto-poll path

**Files:**
- Modify: `src/feed/mod.rs` (re-export in the `pub(crate) use exec::{…}` line; apply in `FeedJob::run`; add a test in the existing `mod tests`)

**Interfaces:**
- Consumes: `degraded_empty_emission(item_count: usize, stderr: &str) -> Option<String>` from Task 2.
- Produces: nothing new. `FeedJob::run` returns early without syncing when the guard fires.

- [ ] **Step 1: Write the failing test**

Add to `mod tests` in `src/feed/mod.rs`. This is the direct regression test for the reported bug — a pre-existing feed task must survive a degraded poll:

```rust
// Regression for #3989 (feeds.allium: DegradedEmptyEmission). A command that
// soft-fails to `[]` while reporting the reason on stderr must NOT reconcile —
// syncing it would delete every feed task already in the epic.
#[tokio::test]
async fn tick_degraded_empty_emission_does_not_delete_existing_tasks() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let epic = db.create_epic("Degraded Epic", "", None).await.unwrap();

    // Seed one feed task, as a previous healthy poll would have.
    db.upsert_feed_tasks(
        epic.id,
        &[crate::models::FeedItem {
            external_id: "pr-1".to_string(),
            title: "Existing PR".to_string(),
            ..Default::default()
        }],
        &["".to_string()],
        &["main".to_string()],
    )
    .await
    .unwrap();
    assert_eq!(db.list_tasks_for_epic(epic.id).await.unwrap().len(), 1);

    db.patch_epic(
        epic.id,
        &EpicPatch::new()
            .feed_command(Some("echo 'Invalid search query' >&2; echo '[]'")),
    )
    .await
    .unwrap();

    let (mut runner, _rx) = make_runner(db.clone());
    runner.tick().await;

    let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    assert_eq!(
        tasks.len(),
        1,
        "a degraded empty emission must not delete existing feed tasks"
    );
    assert_eq!(tasks[0].external_id.as_deref(), Some("pr-1"));
}
```

> **Note on `FeedItem` construction:** if `FeedItem` does not derive `Default`, build it with every field spelled out instead — copy the field list from an existing `FeedItem` literal in `src/feed/mod.rs`'s test module rather than adding a `Default` impl.

> **Note on synchronisation:** `tick` spawns the job; the existing tests in this module await an `McpEvent` on `rx` to know it finished. The guard path sends **no** event, so that handle is unavailable here. Do **not** add a sleep — `./scripts/check-no-test-sleep.sh` rejects it. Instead, make `FeedJob::run` awaitable in tests, or assert after `tick()` returns if `tick` awaits its spawned jobs. Read `FeedRunner::tick` first and pick whichever deterministic signal it already offers; if neither exists, add a test-only completion `Notify` to `FeedRunner` rather than sleeping.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test feed::tests::tick_degraded_empty_emission_does_not_delete_existing_tasks`
Expected: FAIL — `assertion left == right failed: left: 0, right: 1`. The task was deleted. This is the bug.

- [ ] **Step 3: Implement**

In `src/feed/mod.rs`, extend the re-export:

```rust
pub(crate) use exec::{degraded_empty_emission, exec_feed_command, resolve_base_branches};
```

In `FeedJob::run`, immediately after the `parse::parse_feed_items` match block that binds `items`, and before `resolve_feed_item_repo_paths`:

```rust
// A zero-item emission that also wrote to stderr is a degraded run, not an
// empty one: syncing it would delete every feed task in this epic's subtree.
// exec_feed_command already logged the stderr; the auto-poll path adds
// nothing else, per feeds.allium FeedCommandFailure ("the TUI is NOT
// notified"). last_run was bumped by `tick` before this job was spawned.
// See feeds.allium: DegradedEmptyEmission.
if let Some(reason) = exec::degraded_empty_emission(items.len(), &output.stderr) {
    tracing::warn!(
        epic_id = self.epic.id.0,
        epic_title = %self.epic.title,
        "FeedRunner: skipping sync: {reason}"
    );
    return;
}
```

`output.stderr` must still be in scope — the existing code destructures `let stdout = output.stdout;`. Change that to keep `output` alive (e.g. `let stdout = &output.stdout;` and adjust the parse call, or bind `let stderr = output.stderr.clone();` before the move).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test feed::tests`
Expected: all pass, including the new test.

**`tick_stderr_on_zero_exit_does_not_suppress_sync` must still pass unmodified.** Its command emits **one** item alongside stderr, so the guard does not fire. If it now fails, the guard is wrongly keying on stderr alone rather than on zero items — fix the guard, not the test.

- [ ] **Step 5: Commit**

```bash
git add src/feed/mod.rs
git commit -m "fix(feeds): skip sync on a degraded empty emission in the auto-poll path"
```

---

### Task 4: Wire the guard into the manual path and retire `wrote_stderr`

Once the guard is in, `count == 0 && wrote_stderr` can no longer reach `Refreshed`, so the hint it fed becomes dead code.

**Files:**
- Modify: `src/runtime/epics.rs` (`exec_trigger_epic_feed`)
- Modify: `src/tui/messages/feed.rs` (`FeedMessage::Refreshed`, `route`)
- Modify: `src/tui/update/feeds.rs` (`handle_feed_refreshed`)
- Modify: `src/runtime/tests.rs` (invert one test, delete one)
- Modify: `src/tui/tests/epics.rs` (5 `FeedMessage::Refreshed` literals lose a field)

**Interfaces:**
- Consumes: `degraded_empty_emission` from Task 2.
- Produces: `FeedMessage::Refreshed { epic_title: String, count: usize }` — the `wrote_stderr: bool` field is gone. `handle_feed_refreshed(&mut self, epic_title: String, count: usize) -> Vec<Command>`.

- [ ] **Step 1: Write the failing test**

In `src/runtime/tests.rs`, **invert** `exec_trigger_epic_feed_reports_stderr_written_on_zero_exit`. It currently drives `echo 'Invalid search query' >&2; echo '[]'` and asserts `FeedMessage::Refreshed { count: 0, wrote_stderr: true, .. }`. Replace the assertion so it expects a failure, and rename it:

```rust
// feeds.allium: DegradedEmptyEmission. A zero-item emission that wrote to
// stderr is a failure, not a refresh — the sync is skipped entirely so the
// epic's existing tasks survive. Inverted from the #3900 behaviour, which
// reported it as a successful zero-task refresh AFTER the delete had run.
#[tokio::test]
async fn exec_trigger_epic_feed_fails_on_degraded_empty_emission() {
    let db = test_db().await;
    let epic = db.create_epic("Degraded Feed", "", None).await.unwrap();

    let (tx, mut rx) = mpsc::unbounded_channel();
    let rt = make_runtime(db, tx, Arc::new(MockProcessRunner::new(vec![]))).await;

    rt.exec_trigger_epic_feed(
        epic.id,
        "Degraded Feed".to_string(),
        "echo 'Invalid search query' >&2; echo '[]'".to_string(),
        false,
    );

    let msg = recv_feed_message(&mut rx).await;
    match msg {
        Message::Feed(crate::tui::messages::FeedMessage::Failed { error, .. }) => {
            assert!(
                error.contains("Invalid search query"),
                "failure must carry the stderr, got: {error}"
            );
        }
        other => panic!("expected FeedMessage::Failed, got: {other:?}"),
    }
}
```

> Match the surrounding tests' helper for draining `rx` — reuse whatever `exec_trigger_epic_feed_reports_stderr_written_on_zero_exit` used rather than inventing `recv_feed_message` if no such helper exists.

Then **delete** `exec_trigger_epic_feed_quiet_command_reports_no_stderr` — it exists solely to assert `wrote_stderr: false`, which is the field being removed. Its surviving value (a quiet `echo '[]'` still reports a successful refresh) is already covered by `exec_trigger_epic_feed_zero_items`.

**`exec_trigger_epic_feed_zero_items` must stay green, with only the `wrote_stderr` field dropped from its expected literal.** It drives a quiet `echo '[]'` and is the false-positive boundary: a genuinely-empty clean run must still report a successful refresh.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --lib runtime::tests::exec_trigger_epic_feed_fails_on_degraded_empty_emission`
Expected: FAIL — receives `FeedMessage::Refreshed`, panics with "expected FeedMessage::Failed".

- [ ] **Step 3: Implement**

In `src/runtime/epics.rs::exec_trigger_epic_feed`, replace the `let wrote_stderr = !output.stderr.is_empty();` line and apply the guard after the items parse:

```rust
let items: Vec<models::FeedItem> = match serde_json::from_slice(&output.stdout) {
    Ok(i) => i,
    Err(e) => return fail(e.to_string()),
};

// feeds.allium: DegradedEmptyEmission — a zero-item emission that wrote to
// stderr is a failed run, not an empty one. Skip the sync entirely so the
// epic's existing tasks are left alone, and surface the stderr.
if let Some(reason) = crate::feed::degraded_empty_emission(items.len(), &output.stderr) {
    return fail(reason);
}
```

Then drop `wrote_stderr` from the `FeedMessage::Refreshed` construction further down.

In `src/tui/messages/feed.rs`, remove the `wrote_stderr: bool` field from `Refreshed`, update its doc comment (it currently cites `FeedCommandStderrOnSuccess`), and simplify the `route` arm to `app.handle_feed_refreshed(epic_title, count)`.

In `src/tui/update/feeds.rs`, drop the `wrote_stderr` parameter and the whole `hint` block:

```rust
pub(in crate::tui) fn handle_feed_refreshed(
    &mut self,
    epic_title: String,
    count: usize,
) -> Vec<Command> {
    self.set_status(format!("Feed for '{epic_title}': {count} task(s) synced"));
    vec![Command::Task(
        crate::tui::commands::TaskCommand::RefreshFromDb,
    )]
}
```

In `src/tui/tests/epics.rs`, remove `wrote_stderr` from all 5 `FeedMessage::Refreshed` literals.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib runtime::tests && cargo test --lib tui::tests::epics && cargo test --lib feed::`
Expected: all pass. Then `cargo build` to confirm no `wrote_stderr` references remain.

- [ ] **Step 5: Commit**

```bash
git add src/runtime/epics.rs src/runtime/tests.rs src/tui/messages/feed.rs src/tui/update/feeds.rs src/tui/tests/epics.rs
git commit -m "fix(feeds): fail the manual refresh on a degraded empty emission"
```

---

### Task 5: DB layer returns the rows it deleted

**Files:**
- Modify: `src/db/mod.rs` (add `RemovedFeedTask`; change two `TaskCrud` signatures)
- Modify: `src/db/queries/tasks.rs` (`upsert_feed_tasks`, `delete_stale_subtree_feed_tasks`)
- Test: `src/db/tests/tasks.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  ```rust
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct RemovedFeedTask {
      pub id: TaskId,
      pub repo_path: String,
      pub worktree: Option<String>,
      pub tmux_window: Option<String>,
  }
  ```
  `async fn upsert_feed_tasks(...) -> Result<Vec<RemovedFeedTask>>` and
  `async fn delete_stale_subtree_feed_tasks(...) -> Result<Vec<RemovedFeedTask>>`.
  Both return **only** rows that have something to tear down. Consumed by Tasks 6 and 7.

> **Two footguns, both load-bearing:**
>
> 1. **Do not add `AND (worktree IS NOT NULL OR tmux_window IS NOT NULL)` to the `DELETE` predicate.** That would change *which rows get deleted* and strand merged PRs forever. The `DELETE` predicate stays byte-identical; `RETURNING` yields every deleted row, and the filter for "has something to clean" is applied in Rust afterwards.
> 2. **`RETURNING` in rusqlite only executes the statement as rows are stepped.** The iterator must be fully drained (`.collect::<rusqlite::Result<Vec<_>>>()?`) or the delete silently does not happen. The `Statement` borrows the transaction, so it must be dropped before `tx.commit()`.

- [ ] **Step 1: Write the failing tests**

Add to `src/db/tests/tasks.rs`:

```rust
/// The subtree delete must hand back the rows it removed so the caller can
/// tear down their worktrees (feeds.allium: RoleRoutedFeedSync). Only rows
/// carrying a worktree or tmux window are returned — a plain card has nothing
/// to clean up.
#[tokio::test]
async fn delete_stale_subtree_feed_tasks_returns_removed_rows_with_state() {
    let db = Database::open_in_memory().await.unwrap();
    let parent = db.create_epic("Reviews", "", None).await.unwrap();
    let sub = db.create_epic("My Reviews", "", Some(parent.id)).await.unwrap();

    db.upsert_feed_tasks(
        sub.id,
        &[feed_item("stale-1"), feed_item("plain-2"), feed_item("keep-3")],
        &["/repo/a".to_string(), "/repo/a".to_string(), "/repo/a".to_string()],
        &["main".to_string(), "main".to_string(), "main".to_string()],
    )
    .await
    .unwrap();

    // Give only `stale-1` an in-flight worktree and tmux window.
    let tasks = db.list_tasks_for_epic(sub.id).await.unwrap();
    let stale = tasks.iter().find(|t| t.external_id.as_deref() == Some("stale-1")).unwrap();
    db.patch_task(
        stale.id,
        &TaskPatch::new()
            .worktree(Some("/repo/a/.worktrees/stale-1"))
            .tmux_window(Some("dispatch:stale-1")),
    )
    .await
    .unwrap();

    let removed = db
        .delete_stale_subtree_feed_tasks(parent.id, &["keep-3".to_string()])
        .await
        .unwrap();

    // Both stale-1 and plain-2 are deleted from the DB...
    let left = db.list_tasks_for_epic(sub.id).await.unwrap();
    assert_eq!(left.len(), 1, "only the kept item survives");
    assert_eq!(left[0].external_id.as_deref(), Some("keep-3"));

    // ...but only stale-1 needs teardown.
    assert_eq!(removed.len(), 1, "only rows with state are returned");
    assert_eq!(removed[0].repo_path, "/repo/a");
    assert_eq!(removed[0].worktree.as_deref(), Some("/repo/a/.worktrees/stale-1"));
    assert_eq!(removed[0].tmux_window.as_deref(), Some("dispatch:stale-1"));
}

/// The same contract for the flat/grouped path's stale-delete, which runs
/// inside upsert_feed_tasks (feeds.allium: UpsertFeedTasks).
#[tokio::test]
async fn upsert_feed_tasks_returns_removed_rows_with_state() {
    let db = Database::open_in_memory().await.unwrap();
    let epic = db.create_epic("CVE Feed", "", None).await.unwrap();

    db.upsert_feed_tasks(
        epic.id,
        &[feed_item("gone-1")],
        &["/repo/b".to_string()],
        &["main".to_string()],
    )
    .await
    .unwrap();

    let task = db.list_tasks_for_epic(epic.id).await.unwrap().remove(0);
    db.patch_task(
        task.id,
        &TaskPatch::new().worktree(Some("/repo/b/.worktrees/gone-1")),
    )
    .await
    .unwrap();

    // An empty emission clears the epic and reports what it removed.
    let removed = db.upsert_feed_tasks(epic.id, &[], &[], &[]).await.unwrap();

    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0].worktree.as_deref(), Some("/repo/b/.worktrees/gone-1"));
    assert_eq!(removed[0].tmux_window, None, "no tmux window was set");
}

/// A manual task (external_id IS NULL) is never deleted and never reported,
/// even when it carries a worktree.
#[tokio::test]
async fn delete_stale_subtree_feed_tasks_never_reports_manual_tasks() {
    let db = Database::open_in_memory().await.unwrap();
    let parent = db.create_epic("Reviews", "", None).await.unwrap();
    let sub = db.create_epic("My Reviews", "", Some(parent.id)).await.unwrap();

    let manual = db
        .create_task(CreateTaskRequest {
            title: "Manual",
            repo_path: "/repo/a",
            epic_id: Some(sub.id),
            ..Default::default()
        })
        .await
        .unwrap();
    db.patch_task(
        manual,
        &TaskPatch::new().worktree(Some("/repo/a/.worktrees/manual")),
    )
    .await
    .unwrap();

    let removed = db
        .delete_stale_subtree_feed_tasks(parent.id, &[])
        .await
        .unwrap();

    assert!(removed.is_empty(), "manual tasks are neither deleted nor reported");
    assert_eq!(db.list_tasks_for_epic(sub.id).await.unwrap().len(), 1);
}
```

> Reuse the existing `feed_item(...)` / `CreateTaskRequest` helpers in `src/db/tests/tasks.rs` — match their exact shape rather than the sketch above, which may not compile verbatim against the current field lists.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib db::tests::tasks::delete_stale_subtree_feed_tasks_returns`
Expected: FAIL to compile — `Result<()>` has no `len()`; the methods return unit.

- [ ] **Step 3: Implement**

In `src/db/mod.rs`, add the struct near the other public DB types and change both `TaskCrud` signatures to return `Result<Vec<RemovedFeedTask>>`.

In `src/db/queries/tasks.rs::delete_stale_subtree_feed_tasks`, replace the `conn.execute(...)` with:

```rust
self.db_call(move |conn| {
    let mut stmt = conn.prepare(
        "DELETE FROM tasks
         WHERE epic_id IN (SELECT id FROM epics WHERE parent_epic_id = ?1)
           AND external_id IS NOT NULL
           AND external_id NOT IN (SELECT value FROM json_each(?2))
         RETURNING id, repo_path, worktree, tmux_window",
    )?;
    let removed = stmt
        .query_map(params![parent_id.0, keep], |row| {
            Ok(RemovedFeedTask {
                id: TaskId(row.get(0)?),
                repo_path: row.get(1)?,
                worktree: row.get(2)?,
                tmux_window: row.get(3)?,
            })
        })
        .context("Failed to delete stale subtree feed tasks")?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(needs_teardown(removed))
})
.await
```

Apply the identical `RETURNING` treatment to the `DELETE` inside `upsert_feed_tasks`, keeping it inside the existing `tx` and dropping the `Statement` before `tx.commit()`.

Add the shared filter as a free function in the same module:

```rust
/// Keep only the rows that actually own something to tear down. The DELETE
/// predicate is deliberately untouched — every stale feed task is still
/// removed; this only narrows what the caller has to clean up afterwards.
fn needs_teardown(rows: Vec<RemovedFeedTask>) -> Vec<RemovedFeedTask> {
    rows.into_iter()
        .filter(|r| r.worktree.is_some() || r.tmux_window.is_some())
        .collect()
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib db::tests::tasks`
Expected: all pass. The existing `upsert_feed_tasks_*` and `delete_stale_subtree_feed_tasks_scopes_to_subtree_and_keeps_set` tests should compile untouched — every call site discards the `Ok` payload.

- [ ] **Step 5: Commit**

```bash
git add src/db/mod.rs src/db/queries/tasks.rs src/db/tests/tasks.rs
git commit -m "feat(db): return deleted rows from the feed stale-delete statements"
```

---

### Task 6: The teardown fan-out helper

**Files:**
- Modify: `src/feed/mod.rs` (add `cleanup_removed_feed_tasks` and its tests)

**Interfaces:**
- Consumes: `RemovedFeedTask` from Task 5; `crate::dispatch::cleanup_task(repo_path, worktree_path, tmux_window, runner)`; `crate::tmux::kill_window_if_present(window, runner)`; `db.has_other_tasks_with_worktree(worktree, exclude_id)`.
- Produces: `pub(crate) async fn cleanup_removed_feed_tasks(db: &dyn TaskStore, runner: Arc<dyn ProcessRunner>, removed: Vec<RemovedFeedTask>)`. Consumed by Task 7.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `src/feed/mod.rs`:

```rust
// --- cleanup_removed_feed_tasks ---

#[tokio::test]
async fn cleanup_removes_worktree_and_kills_window() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let runner = Arc::new(MockProcessRunner::new(vec![]));

    cleanup_removed_feed_tasks(
        &*db,
        runner.clone(),
        vec![RemovedFeedTask {
            id: TaskId(1),
            repo_path: "/repo/a".to_string(),
            worktree: Some("/repo/a/.worktrees/pr-1".to_string()),
            tmux_window: Some("dispatch:pr-1".to_string()),
        }],
    )
    .await;

    let calls = runner.calls();
    assert!(
        calls.iter().any(|c| c.contains("worktree") && c.contains("/repo/a/.worktrees/pr-1")),
        "must remove the worktree, got: {calls:?}"
    );
    assert!(
        calls.iter().any(|c| c.contains("kill-window")),
        "must kill the tmux window, got: {calls:?}"
    );
}

// A worktree shared with a live task must survive — only the window goes.
#[tokio::test]
async fn cleanup_skips_worktree_removal_when_shared() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let epic = db.create_epic("E", "", None).await.unwrap();
    let other = db
        .create_task(/* a task in /repo/a/.worktrees/shared — see helpers */)
        .await
        .unwrap();
    db.patch_task(
        other,
        &crate::db::TaskPatch::new().worktree(Some("/repo/a/.worktrees/shared")),
    )
    .await
    .unwrap();
    let _ = epic;

    let runner = Arc::new(MockProcessRunner::new(vec![]));
    cleanup_removed_feed_tasks(
        &*db,
        runner.clone(),
        vec![RemovedFeedTask {
            id: TaskId(999),
            repo_path: "/repo/a".to_string(),
            worktree: Some("/repo/a/.worktrees/shared".to_string()),
            tmux_window: Some("dispatch:pr-1".to_string()),
        }],
    )
    .await;

    let calls = runner.calls();
    assert!(
        !calls.iter().any(|c| c.contains("worktree remove")),
        "a shared worktree must not be removed, got: {calls:?}"
    );
    assert!(
        calls.iter().any(|c| c.contains("kill-window")),
        "the window still goes, got: {calls:?}"
    );
}

// Two removals in the same repo must not run git concurrently — git takes a
// lock on the repo's worktree metadata and index.
#[tokio::test]
async fn cleanup_serialises_same_repo_removals() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let runner = Arc::new(MockProcessRunner::new(vec![]));

    cleanup_removed_feed_tasks(
        &*db,
        runner.clone(),
        vec![
            RemovedFeedTask {
                id: TaskId(1),
                repo_path: "/repo/a".to_string(),
                worktree: Some("/repo/a/.worktrees/pr-1".to_string()),
                tmux_window: None,
            },
            RemovedFeedTask {
                id: TaskId(2),
                repo_path: "/repo/a".to_string(),
                worktree: Some("/repo/a/.worktrees/pr-2".to_string()),
                tmux_window: None,
            },
        ],
    )
    .await;

    let calls = runner.calls();
    let pr1 = calls.iter().position(|c| c.contains("pr-1")).expect("pr-1 cleaned");
    let pr2 = calls.iter().position(|c| c.contains("pr-2")).expect("pr-2 cleaned");
    assert_ne!(pr1, pr2, "both removals ran");
}
```

> `MockProcessRunner`'s recorded-call accessor may not be named `calls()` — read `src/process.rs` and use whatever the existing `MockProcessRunner` tests in `src/tmux.rs` use to assert argv. Fill in the `create_task` call in the second test from the helpers already used in `src/feed/mod.rs`'s test module.
>
> The serialisation test above only proves both removals happened. To prove *ordering*, have the helper collect per-repo work into one `spawn_blocking` and assert that the two `git` calls for `/repo/a` are adjacent in the recorded call list — a concurrent implementation would interleave them with calls from another repo. Add a third `RemovedFeedTask` in a different `repo_path` to make that assertion meaningful.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib feed::tests::cleanup_`
Expected: FAIL to compile — `cannot find function cleanup_removed_feed_tasks`.

- [ ] **Step 3: Implement**

Add to `src/feed/mod.rs`:

```rust
/// Tear down the worktree and tmux window of every feed task a sync removed.
///
/// A feed-driven removal is a deletion like any other and owes the same
/// teardown ArchiveTask and DeleteTask perform (tasks.allium): kill the tmux
/// window, remove the worktree unless another active task shares it, and
/// delete the branch best-effort.
///
/// Removals are grouped by `repo_path` and run sequentially within a repo.
/// `cleanup_task` shells `git -C <repo> worktree remove --force` and
/// `git branch -D` against the shared checkout, and a Reviews epic's tasks
/// overwhelmingly share one repo — running those concurrently contends on the
/// repo's index lock. Different repos still proceed in parallel.
///
/// Failures are logged, not surfaced: feed reconciliation is background work.
pub(crate) async fn cleanup_removed_feed_tasks(
    db: &dyn TaskStore,
    runner: Arc<dyn ProcessRunner>,
    removed: Vec<RemovedFeedTask>,
) {
    if removed.is_empty() {
        return;
    }

    // The shared-worktree question needs the DB, so resolve it before the
    // blocking fan-out. The row itself is already gone, so `exclude_id` only
    // matters for defence in depth.
    let mut by_repo: HashMap<String, Vec<(RemovedFeedTask, bool)>> = HashMap::new();
    for task in removed {
        let shared = match &task.worktree {
            Some(wt) => db
                .has_other_tasks_with_worktree(wt, task.id)
                .await
                .unwrap_or(false),
            None => false,
        };
        by_repo.entry(task.repo_path.clone()).or_default().push((task, shared));
    }

    let mut handles = Vec::new();
    for (_repo, tasks) in by_repo {
        let runner = runner.clone();
        handles.push(tokio::task::spawn_blocking(move || {
            for (task, shared) in tasks {
                match (&task.worktree, shared) {
                    // Nothing but a window to reclaim.
                    (None, _) | (Some(_), true) => {
                        if let Some(window) = &task.tmux_window {
                            if let Err(err) = crate::tmux::kill_window_if_present(window, &*runner) {
                                tracing::warn!(
                                    task_id = task.id.0,
                                    "feed cleanup: kill_window_if_present failed: {err:#}"
                                );
                            }
                        }
                    }
                    (Some(worktree), false) => {
                        if let Err(err) = crate::dispatch::cleanup_task(
                            &task.repo_path,
                            worktree,
                            task.tmux_window.as_deref(),
                            &*runner,
                        ) {
                            tracing::warn!(
                                task_id = task.id.0,
                                "feed cleanup: cleanup_task failed: {err:#}"
                            );
                        }
                    }
                }
            }
        }));
    }
    for handle in handles {
        let _ = handle.await;
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib feed::tests::cleanup_`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/feed/mod.rs
git commit -m "feat(feeds): tear down worktrees for feed-removed tasks, serialised per repo"
```

---

### Task 7: Propagate removed rows through the ingest pipeline and call the fan-out

**Files:**
- Modify: `src/feed/ingest/stale.rs` (`delete_stale_subtree`, `clear_parent_stranded_tasks`)
- Modify: `src/feed/ingest/role_routed.rs` (`run_role_routed_feed_sync`)
- Modify: `src/feed/ingest/grouped.rs` (`upsert_sub_epic_and_recalc`, `upsert_present_groups`, `clear_absent_sub_epics`, `clear_parent_flat_tasks`, `sync_grouped_feed`)
- Modify: `src/feed/ingest/mod.rs` (`FeedSyncOutcome`, `run_feed_sync`, `run_feed_sync_by_role`)
- Modify: `src/feed/mod.rs` (`FeedJob::run` calls the fan-out)
- Modify: `src/runtime/epics.rs` (`exec_trigger_epic_feed` calls the fan-out)
- Test: `src/feed/ingest/tests.rs`

**Interfaces:**
- Consumes: `RemovedFeedTask` (Task 5), `cleanup_removed_feed_tasks` (Task 6).
- Produces:
  ```rust
  pub(crate) struct FeedSyncOutcome {
      pub(crate) affected_epics: Vec<EpicId>,
      pub(crate) removed: Vec<RemovedFeedTask>,
  }
  ```
  `run_feed_sync_by_role(...) -> Result<FeedSyncOutcome>` (was `Result<Vec<EpicId>>`).

- [ ] **Step 1: Write the failing test**

This is the invariant the whole cleanup half rests on. Add to `src/feed/ingest/tests.rs`:

```rust
/// A task moved between role sub-epics during a sync must NEVER appear in the
/// removed set. The ordering that makes this true is not compiler-enforced:
/// apply_move's set_task_epic_id lands before upsert_role_groups,
/// delete_stale_subtree and clear_parent_stranded_tasks, each of whose SQL
/// filters on the task's CURRENT epic_id. Before task teardown existed, a
/// mis-ordering merely deleted a row; now it would force-remove a live agent's
/// worktree. Pin it.
#[tokio::test]
async fn moved_task_is_never_reported_as_removed() {
    let db = Database::open_in_memory().await.unwrap();
    let parent = db.create_epic("Reviews", "", None).await.unwrap();
    db.patch_epic(parent.id, &EpicPatch::new().feed_role(FeedRole::ReviewsParent))
        .await
        .unwrap();

    // First sync: the PR is team-requested, so it lands in Team Reviews.
    let outcome = run_feed_sync_by_role(
        &db,
        parent.id,
        FeedRole::ReviewsParent,
        false,
        vec![entry_with_signals("pr-1", &[Signal::TeamRequest])],
    )
    .await
    .unwrap();
    assert!(outcome.removed.is_empty(), "nothing removed on first sight");

    // Give it an in-flight worktree, as a dispatched review agent would.
    let task = find_feed_task(&db, parent.id, "pr-1").await;
    db.patch_task(
        task.id,
        &TaskPatch::new()
            .worktree(Some("/repo/a/.worktrees/pr-1"))
            .tmux_window(Some("dispatch:pr-1")),
    )
    .await
    .unwrap();

    // Second sync: I have now reviewed it, so it routes to My Reviews — a MOVE
    // across role sub-epics, not a delete-and-reinsert.
    let outcome = run_feed_sync_by_role(
        &db,
        parent.id,
        FeedRole::ReviewsParent,
        false,
        vec![entry_with_signals("pr-1", &[Signal::TeamRequest, Signal::Reviewed])],
    )
    .await
    .unwrap();

    assert!(
        outcome.removed.is_empty(),
        "a moved task must not be reported for teardown, got: {:?}",
        outcome.removed
    );

    let moved = find_feed_task(&db, parent.id, "pr-1").await;
    assert_eq!(
        moved.worktree.as_deref(),
        Some("/repo/a/.worktrees/pr-1"),
        "the move must preserve the in-flight worktree"
    );
}
```

> Reuse the existing helpers in `src/feed/ingest/tests.rs` for building entries and locating a feed task across the subtree — the module already has tests that route by signal and assert cross-role moves. Match their shapes rather than the `entry_with_signals` / `find_feed_task` sketches above.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --lib feed::ingest::tests::moved_task_is_never_reported_as_removed`
Expected: FAIL to compile — `run_feed_sync_by_role` returns `Vec<EpicId>`, which has no `.removed`.

- [ ] **Step 3: Implement the propagation**

`src/feed/ingest/stale.rs` — both functions return what they removed:

```rust
pub(super) async fn delete_stale_subtree(
    db: &dyn TaskStore,
    parent_id: EpicId,
    roles: &RoleSubEpics,
    all_external_ids: &[String],
) -> Vec<RemovedFeedTask>
```

Collect from the parent-rooted call and each of the three role-rooted calls into one `Vec`. `warn_on_err` currently consumes the `Result`; switch to matching on it so the `Ok` payload is kept and the warn behaviour is preserved on `Err` (returning an empty vec in that arm).

`clear_parent_stranded_tasks` returns `Vec<RemovedFeedTask>` the same way.

`src/feed/ingest/role_routed.rs::run_role_routed_feed_sync` returns `Result<FeedSyncOutcome>`, concatenating the vecs from `delete_stale_subtree` and `clear_parent_stranded_tasks` into `removed`, and its existing `all_ids` into `affected_epics`.

`src/feed/ingest/grouped.rs` — thread the same way: `upsert_sub_epic_and_recalc` returns `Vec<RemovedFeedTask>`; `upsert_present_groups`, `clear_absent_sub_epics` and `clear_parent_flat_tasks` accumulate; `sync_grouped_feed` returns `(Vec<EpicId>, Vec<RemovedFeedTask>)`.

`src/feed/ingest/mod.rs` — add `FeedSyncOutcome`, and have `run_feed_sync` / `run_feed_sync_by_role` return it. The flat branch's `db.upsert_feed_tasks(...)` payload becomes `removed`.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --lib feed::ingest::tests`
Expected: all pass, including the new invariant test.

- [ ] **Step 5: Call the fan-out from both feed paths**

In `src/feed/mod.rs::FeedJob::run`, after the sync succeeds:

```rust
if let Ok(outcome) = &sync_result {
    recalculate_epic_status_after_feed(&*self.db, self.epic.id, "FeedRunner").await;
    for id in &outcome.affected_epics {
        let _ = self.notify.send(McpEvent::EpicChanged(*id));
    }
}
if let Ok(outcome) = sync_result {
    cleanup_removed_feed_tasks(&*self.db, self.runner.clone(), outcome.removed).await;
} else {
    warn_on_err(
        sync_result.map(|_| ()),
        self.epic.id,
        None,
        "FeedRunner: upsert_feed_tasks failed",
    );
}
```

> Restructure as needed to satisfy the borrow checker and keep the existing `warn_on_err` behaviour on the error path — the shape above is illustrative, not literal.

In `src/runtime/epics.rs::exec_trigger_epic_feed`, in the `Ok` arm, call `crate::feed::cleanup_removed_feed_tasks(&*db, runner.clone(), outcome.removed).await;` before sending `FeedMessage::Refreshed`, and take `count` from the emission as it does today.

- [ ] **Step 6: Run the full suite**

Run: `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`
Expected: all pass. Then `cargo clippy --all-targets -- -D warnings`.

- [ ] **Step 7: Commit**

```bash
git add src/feed/ src/runtime/epics.rs
git commit -m "fix(feeds): tear down worktrees when a feed sync removes tasks"
```

---

### Task 8: Spec alignment and the deferred follow-up

**Files:**
- Modify: `docs/specs/feeds.allium` (`@guidance` blocks only — implementation pointers)

**Interfaces:**
- Consumes: everything above.
- Produces: nothing.

- [ ] **Step 1: Run `allium:weed` to find spec/code divergence**

Invoke the `allium:weed` skill over `docs/specs/feeds.allium` and `docs/specs/tasks.allium`. Resolve anything it finds that this change introduced.

- [ ] **Step 2: Update the `@guidance` implementation pointers**

`RoleRoutedFeedSync`'s guidance names `delete_stale_subtree_feed_tasks` and describes it as a bare `DELETE`. Update it to mention `RETURNING` and the teardown fan-out
(`src/feed/mod.rs::cleanup_removed_feed_tasks`). Add the new coverage to its `Coverage:` list. Do the same for `FeedCommandStderrOnSuccess`, pointing at `src/feed/exec.rs::degraded_empty_emission`.

- [ ] **Step 3: Record the deferred follow-up as a task**

Nothing serialises a manual `r` refresh against an in-flight poll tick for the same epic, so the two can interleave between the non-transactional steps of `run_role_routed_feed_sync`. Pre-existing, but this change raises its cost from a lost row to a destroyed worktree.

Call `create_task` on the dispatch MCP server describing exactly that, referencing this plan and the "Out of scope" section of
`docs/superpowers/specs/2026-08-12-feed-empty-emission-guard-design.md`.

- [ ] **Step 4: Run the verify command**

Run: `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add docs/specs/feeds.allium docs/specs/tasks.allium
git commit -m "docs(feeds): point the specs at the guard and teardown implementations"
```

---

## Self-Review Notes

**Spec coverage.** Every design section maps to a task: guard predicate → Task 2; guard wiring → Tasks 3–4; `wrote_stderr` retirement → Task 4; teardown concept naming → Task 1; DB `RETURNING` → Task 5; per-repo serialised fan-out → Task 6; pipeline propagation and the ordering invariant → Task 7; spec realignment and the deferred follow-up → Task 8.

**Test citations verified against the code.** `tick_stderr_on_zero_exit_does_not_suppress_sync` (`src/feed/mod.rs`) emits **one** item and must stay green untouched — it is *not* the test that inverts. The inverting test is `exec_trigger_epic_feed_reports_stderr_written_on_zero_exit` (`src/runtime/tests.rs`). `exec_trigger_epic_feed_zero_items` is the false-positive boundary and keeps asserting a successful refresh.

**Type consistency.** `RemovedFeedTask` is defined once in Task 5 and used with identical field names in Tasks 6 and 7. `FeedSyncOutcome` is defined in Task 7 and consumed only there and in the two callers.

**Known sketch sites.** Tasks 5, 6 and 7 contain test code whose helper names (`feed_item`, `MockProcessRunner::calls`, `entry_with_signals`, `find_feed_task`) must be matched against the existing test modules rather than copied verbatim. Each is flagged inline.
