# Shell Visibility Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a live backgrounded shell (Bash tool with `run_in_background: true`) visible on the kanban card, and stop the Running → Review transition from firing while one is still alive — mirroring the existing `task_subagents`/`live_subagents`/`stop_pending` machinery with a new, parallel `task_shells`/`live_shells` mechanism.

**Architecture:** A new `task_shells` table (mirroring `task_subagents` exactly: `task_id, shell_id, session_id, started_at`) tracks live background shells, counted into a new `Task.live_shells` column. `HookStop`'s two conditional `UPDATE`s widen to check this counter alongside `live_subagents`; the shared drain predicate (`apply_pending_stop_if_drained`) widens the same way. Detection rides on existing hook events already reaching the shell script (`PostToolUse` for `Bash`/`KillBash`/`BashOutput`), forwarded via a new `dispatch hook-shell` CLI subcommand mirroring `hook-subagent`. Staleness gets a new, much longer threshold (4h) and a distinct `SubStatus`/card label, so an abandoned shell doesn't look identical to a healthy long-running one forever.

**Tech Stack:** Rust, rusqlite, tokio, ratatui, bash (hook script), insta (snapshot tests).

**Spec:** `docs/superpowers/specs/2026-08-15-shell-visibility-design.md` — read this first. It documents two rounds of correction against the real codebase (an adversarial review found real bugs in the first draft; a further research pass found the review's own "Good Practices" claim about `ExitSession` was itself wrong). This plan implements the design's final, verified state. Task 1 also updates `docs/specs/agent-health.allium`, `docs/specs/split-pane.allium`, `docs/specs/dispatch.allium`, and `docs/specs/core.allium` per this repo's spec-first convention — those are the durable domain spec; the design doc above is the one-time design record for *why*.

## Global Constraints

- Every inline `#[cfg(test)] mod tests` block needs `#[allow(clippy::unwrap_used, clippy::expect_used)]` at the top (workspace `-D warnings` policy).
- Never `git add`/`git commit` anything under `docs/plans/` (not applicable here — this plan lives under `docs/superpowers/plans/`, which IS committed per this repo's convention).
- No `tokio::time::sleep` anywhere under `src/`/`tests/`; no `std::thread::sleep` in test files.
- Run `cargo fmt` and `cargo clippy --all-targets -- -D warnings` before any push (the pre-push hook enforces this, but running it per-task catches problems earlier).
- The real subagent machinery this design mirrors is NOT what a first read of the design doc's headline claims might suggest — every task below cites the exact real file:line, verified against source, not against the design doc's prose.

---

## Task 1: Allium spec updates

**Files:**
- Modify: `docs/specs/agent-health.allium`
- Modify: `docs/specs/split-pane.allium`
- Modify: `docs/specs/dispatch.allium`
- Modify: `docs/specs/core.allium`

**Interfaces:**
- Produces: the domain-level vocabulary (`task_shells`/`live_shells`/`ShellEntry`-equivalent rule names) that every later task's code changes implement. No Rust code in this task — pure spec.

- [x] **Step 1: Add the `task_shells` entity to `docs/specs/core.allium`**

Find the `Task` entity definition and the `SubagentEntry`/`live_subagents` fields it already documents (`docs/specs/core.allium`). Add a parallel entity and field, e.g.:

```
entity ShellEntry {
    task: Task
    shell_id: String
    session_id: String
    started_at: Timestamp
}
```

And on `Task`, alongside the existing `live_subagents: Int` field, add:

```
    live_shells: Int
    oldest_live_shell_started_at: Timestamp?
```

- [x] **Step 2: Update `docs/specs/agent-health.allium`'s `HookStop` rule**

Widen the `ensures` block from:

```
    ensures:
        if task.live_subagents > 0:
            task.stop_pending = true
            task.stop_pending_at = now
        else:
            task.status = review
            ...
```

to:

```
    ensures:
        if task.live_subagents > 0 or task.live_shells > 0:
            task.stop_pending = true
            task.stop_pending_at = now
        else:
            task.status = review
            ...
```

Add a `@guidance` note explaining the real implementation is two separate conditional `UPDATE` statements (`src/db/queries/tasks.rs::try_record_stop`), both of which must check the widened condition — one caller widening only the defer half would silently leave the flip half unfixed.

- [x] **Step 3: Add `HookShellStart`/`HookShellStop` rules to `docs/specs/agent-health.allium`**

Mirror the shape of `HookSubagentStart`/`HookSubagentStop`, but:
- `HookShellStart` fires on `Bash` `PostToolUse` with `run_in_background = true` (not `PreToolUse` — the shell_id doesn't exist yet at PreToolUse).
- `HookShellStop` fires on `KillBash` `PostToolUse`, or `BashOutput` `PostToolUse` when the polled shell's status is no longer running.
- Both apply the same session-fencing behavior as `SubagentSessionFence` (evict `ShellEntry` rows whose `session_id` differs from the incoming event).
- **Unlike** `HookSubagentStop`, there is no `ClearShellsOnSessionStart` rule — document explicitly why (a background shell can survive `/clear`/resume since it's an independent OS process, unlike a subagent; session fencing on the next shell event is the only sweep mechanism, and its absence for a task that never gets another shell event is the accepted "Known limitation").
- Note the four real structural clear points shells mirror: `DetectCrashedAgent`, `DetachTmux`, and `DispatchTask`'s two claim paths (`ClaimNextBacklogSubtask`/whatever `docs/specs/dispatch.allium` calls the by-id claim) — cite `docs/specs/dispatch.allium` and `docs/specs/split-pane.allium` by name since this rule's guidance references them.

- [x] **Step 4: Update `ClassifyAgentActivity` in `docs/specs/agent-health.allium`**

Add a new `shell_stale_threshold: Duration = 4.hours` to the `config` block (alongside `active_threshold`). Add a new branch: `live_shells > 0` forces `active` unless `now - task.oldest_live_shell_started_at > config.shell_stale_threshold`, in which case a new sub-status `stale_shell` applies. Order this **after** the `live_subagents > 0` check (a genuinely live subagent wins over a stale-looking old shell) and **before** the plain time-threshold branch. Document the `stale_shell` card label convention: `"stale · shell Xh"` or similar, distinct from plain `"stale · Xm"`.

- [x] **Step 5: Update `DetachTmux` in `docs/specs/split-pane.allium`**

Widen its `ensures` block — currently clears `SubagentEntries` and checks `live_subagents = 0 and stop_pending and status = running` to flip to Review. Add: also clears `ShellEntry` rows and resets `live_shells`/`oldest_live_shell_started_at`, and the flip condition becomes `live_subagents = 0 and live_shells = 0 and stop_pending and status = running`.

- [x] **Step 6: Update `DispatchTask` in `docs/specs/dispatch.allium`**

Wherever it documents clearing `SubagentEntry`/`live_subagents`/`stop_pending` on claim, add the equivalent `ShellEntry`/`live_shells` clear, non-draining (same reasoning already given there: guards against entries left over from a prior run of the same task).

- [x] **Step 7: Run `allium check` / `allium analyse` per this repo's convention**

Run: `allium check docs/specs/agent-health.allium docs/specs/split-pane.allium docs/specs/dispatch.allium docs/specs/core.allium`
Expected: no validation errors. Fix any syntax issues before proceeding.

- [x] **Step 8: Commit**

```bash
git add docs/specs/agent-health.allium docs/specs/split-pane.allium docs/specs/dispatch.allium docs/specs/core.allium
git commit -m "spec(4187): add task_shells/live_shells to the Allium domain spec"
```

---

## Task 2: Migration — `task_shells` table and `tasks` columns

**Files:**
- Modify: `src/db/migrations.rs`
- Test: `src/db/tests/migrations.rs`

**Interfaces:**
- Consumes: nothing from prior tasks (pure schema).
- Produces: `task_shells(task_id, shell_id, session_id, started_at)` table, `tasks.live_shells INTEGER NOT NULL DEFAULT 0`, `tasks.oldest_live_shell_started_at TEXT` (nullable) — read by every later task.

- [x] **Step 1: Write the failing migration test**

In `src/db/tests/migrations.rs`, following the exact shape of `migration_81_creates_task_subagents_and_columns` (lines 64-93):

```rust
#[tokio::test]
async fn migration_85_creates_task_shells_and_columns() {
    let db = in_memory_db().await;
    let has = db
        .db_call(|conn| {
            let table: i64 = conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='task_shells'",
                [],
                |r| r.get(0),
            )?;
            let live: i64 = conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('tasks') WHERE name='live_shells'",
                [],
                |r| r.get(0),
            )?;
            let oldest: i64 = conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('tasks') WHERE name='oldest_live_shell_started_at'",
                [],
                |r| r.get(0),
            )?;
            Ok((table, live, oldest))
        })
        .await
        .expect("query schema");
    assert_eq!(
        has,
        (1, 1, 1),
        "migration 85 must create task_shells and both tasks columns"
    );
}
```

- [x] **Step 2: Run it to verify it fails**

Run: `cargo test --lib migration_85_creates_task_shells_and_columns`
Expected: FAIL — table/columns don't exist yet.

- [x] **Step 3: Add the migration function**

In `src/db/migrations.rs`, following `migrate_v81_create_task_subagents`'s exact shape:

```rust
pub(super) fn migrate_v85_create_task_shells(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS task_shells (
             task_id    INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
             shell_id   TEXT    NOT NULL,
             session_id TEXT    NOT NULL,
             started_at TEXT    NOT NULL,
             PRIMARY KEY (task_id, shell_id)
         );
         CREATE INDEX IF NOT EXISTS idx_task_shells_task ON task_shells(task_id);",
    )?;
    if !column_exists(conn, "tasks", "live_shells") {
        conn.execute_batch("ALTER TABLE tasks ADD COLUMN live_shells INTEGER NOT NULL DEFAULT 0")?;
    }
    if !column_exists(conn, "tasks", "oldest_live_shell_started_at") {
        conn.execute_batch("ALTER TABLE tasks ADD COLUMN oldest_live_shell_started_at TEXT")?;
    }
    Ok(())
}
```

Register it in the `MIGRATIONS` array (the tail currently ends `(84, migrate_v84_drop_tips_state)`):

```rust
    (85, migrate_v85_create_task_shells),
```

- [x] **Step 4: Run test to verify it passes**

Run: `cargo test --lib migration_85_creates_task_shells_and_columns`
Expected: PASS

- [x] **Step 5: Commit**

```bash
git add src/db/migrations.rs src/db/tests/migrations.rs
git commit -m "feat(4187): add task_shells table and tasks.live_shells/oldest_live_shell_started_at (migration 85)"
```

---

## Task 3: Query layer — `task_shells` CRUD, `TaskCrud` trait methods, and the shared drain predicate

**Files:**
- Create: `src/db/queries/shells.rs` (raw row-level functions, `pub(super) fn`, taking `&Connection`/`&mut Connection` — mirrors `src/db/queries/subagents.rs`'s shape exactly)
- Modify: `src/db/queries/mod.rs` (register the module; move `apply_pending_stop_if_drained` here and widen it)
- Modify: `src/db/queries/subagents.rs` (widen `subagent_clear`, `finish_drain`)
- Modify: `src/db/mod.rs` (add `shell_start`/`shell_stop`/`shell_clear_no_drain` to the `TaskCrud` trait, alongside the existing `subagent_start`/`subagent_stop`/`subagent_clear`/`subagent_clear_and_void_pending_stop` declarations at lines 223-260)
- Modify: `src/db/queries/tasks.rs` (the `TaskCrud` impl block — add the three new async methods, mirroring `subagent_start`/`subagent_stop`/`subagent_clear` at lines 520-557 exactly, each a thin `self.db_call(move |conn| super::shells::...)` wrapper)
- Create: `src/db/tests/shells.rs` (CRUD tests calling the trait methods through `Database`, mirroring `src/db/tests/subagents.rs` exactly — per this repo's own convention, "DB schema, CRUD" tests live under `src/db/tests/`, not inline in the query module)
- Modify: `src/db/tests/mod.rs` (register `mod shells;` alongside the existing `mod subagents;`)

**Interfaces:**
- Consumes: `task_shells` table + `tasks.live_shells`/`oldest_live_shell_started_at` columns (Task 2), `crate::models::ShellDrain` (Task 5 — **do Task 5 before this task**, since `ShellDrain` is a model type this task's function signatures return).
- Produces: `Database::shell_start(&self, id: TaskId, shell_id: &str, session_id: &str, now: DateTime<Utc>) -> Result<i64>`, `Database::shell_stop(&self, id: TaskId, shell_id: &str, session_id: &str) -> Result<ShellDrain>`, `Database::shell_clear_no_drain(&self, id: TaskId) -> Result<()>` (all via the `TaskCrud` trait) — consumed by Task 6's service layer.

- [x] **Step 1: Write the failing CRUD tests in `src/db/tests/shells.rs`**

Register the module first: in `src/db/tests/mod.rs`, add `mod shells;` alongside the existing `mod subagents;` (line 10). Then, following `src/db/tests/subagents.rs` verbatim (its `make_task` helper, its `in_memory_db()`/`create_task_returning` usage, its exact assertion style):

```rust
#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use chrono::Utc;

async fn make_task(db: &Database, title: &str) -> Task {
    create_task_returning(db, title, "desc", "/repo", None, TaskStatus::Backlog)
        .await
        .unwrap()
}

#[tokio::test]
async fn start_then_stop_returns_to_zero() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    let now = Utc::now();

    assert_eq!(db.shell_start(task.id, "bash_1", "s1", now).await.unwrap(), 1);
    assert_eq!(db.shell_start(task.id, "bash_2", "s1", now).await.unwrap(), 2);
    assert_eq!(db.shell_stop(task.id, "bash_1", "s1").await.unwrap().live, 1);
    assert_eq!(db.shell_stop(task.id, "bash_2", "s1").await.unwrap().live, 0);
}

#[tokio::test]
async fn duplicate_start_is_idempotent() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    let now = Utc::now();

    assert_eq!(db.shell_start(task.id, "bash_1", "s1", now).await.unwrap(), 1);
    assert_eq!(
        db.shell_start(task.id, "bash_1", "s1", now).await.unwrap(),
        1,
        "a replayed shell start must not double-count"
    );
}

#[tokio::test]
async fn unknown_stop_is_a_noop_not_an_underflow() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;

    assert_eq!(
        db.shell_stop(task.id, "never-started", "s1").await.unwrap().live,
        0,
        "an unrecognised shell_id must not drive the count negative"
    );
    let reread = db.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(reread.live_shells, 0);
}

#[tokio::test]
async fn new_session_id_evicts_the_previous_sessions_entries() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    let now = Utc::now();

    db.shell_start(task.id, "bash_1", "old-session", now).await.unwrap();
    let count = db.shell_start(task.id, "bash_2", "new-session", now).await.unwrap();
    assert_eq!(
        count, 1,
        "the old session's row must be fenced out, leaving only the new one"
    );
}

#[tokio::test]
async fn shell_stop_drains_a_deferred_stop_when_both_counters_reach_zero() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    let now = Utc::now();
    db.shell_start(task.id, "bash_1", "s1", now).await.unwrap();
    db.db_call(move |conn| {
        conn.execute(
            "UPDATE tasks SET status = 'running', stop_pending = 1 WHERE id = ?1",
            rusqlite::params![task.id.0],
        )
        .map_err(anyhow::Error::from)
    })
    .await
    .unwrap();

    let drain = db.shell_stop(task.id, "bash_1", "s1").await.unwrap();
    assert!(
        drain.applied_pending_stop,
        "draining the only live shell with stop_pending set must flip to Review"
    );
    let reread = db.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(reread.status, TaskStatus::Review);
}

#[tokio::test]
async fn shell_stop_does_not_drain_while_a_subagent_is_still_live() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    let now = Utc::now();
    db.shell_start(task.id, "bash_1", "s1", now).await.unwrap();
    db.db_call(move |conn| {
        conn.execute(
            "UPDATE tasks SET status = 'running', stop_pending = 1, live_subagents = 1 WHERE id = ?1",
            rusqlite::params![task.id.0],
        )
        .map_err(anyhow::Error::from)
    })
    .await
    .unwrap();

    let drain = db.shell_stop(task.id, "bash_1", "s1").await.unwrap();
    assert!(
        !drain.applied_pending_stop,
        "live_subagents > 0 must block the flip even though live_shells reached 0 \
         -- this is the regression test for the shared-drain-predicate finding"
    );
}
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib db::tests::shells`
Expected: FAIL to compile — `Database::shell_start`/`shell_stop` don't exist yet.

- [x] **Step 3: Move `apply_pending_stop_if_drained` to `src/db/queries/mod.rs` and widen it**

In `src/db/queries/subagents.rs`, delete the `apply_pending_stop_if_drained` function (currently private, right after the module doc comment). In `src/db/queries/mod.rs`, add it (near `STOP_FLIP_SET`, which it uses):

```rust
/// Applies a deferred `Stop` once both live-subagent and live-shell counts
/// have reached zero. Shared by every drain path (`subagents::finish_drain`,
/// `shells::shell_stop`, `shells::shell_clear_no_drain`'s draining sibling if
/// one exists) so a later edit to the condition cannot fix one caller and
/// silently leave another racy — see the "shared drain predicate" finding in
/// docs/superpowers/specs/2026-08-15-shell-visibility-design.md.
pub(super) fn apply_pending_stop_if_drained(tx: &Connection, task_id: i64) -> Result<bool> {
    let review = TaskStatus::Review;
    let rows = tx
        .execute(
            &format!(
                "UPDATE tasks {} \
                 WHERE id = ?3 AND status = ?4 AND stop_pending = 1 \
                   AND live_subagents = 0 AND live_shells = 0",
                STOP_FLIP_SET
            ),
            params![
                review.as_str(),
                SubStatus::default_for(review).as_str(),
                task_id,
                TaskStatus::Running.as_str(),
            ],
        )
        .context("Failed to apply pending stop")?;
    Ok(rows == 1)
}
```

Add the needed imports to `mod.rs` (`rusqlite::{params, Connection}` if not already present — check first, `mod.rs` already imports some rusqlite types for other functions).

In `src/db/queries/subagents.rs`, update `finish_drain` to call `super::apply_pending_stop_if_drained` instead of the now-deleted local one, and to also resync shell state (harmless no-op when this call site didn't touch `task_shells`, but keeps both counters guaranteed-fresh before the shared predicate runs):

```rust
fn finish_drain(tx: rusqlite::Transaction<'_>, task_id: i64, what: &str) -> Result<SubagentDrain> {
    let live = sync_count(&tx, task_id)?;
    super::shells::sync_shell_state(&tx, task_id)?;
    let applied_pending_stop = super::apply_pending_stop_if_drained(&tx, task_id)?;
    tx.commit()?;
    Ok(SubagentDrain { live, applied_pending_stop })
}
```

Also widen `subagent_clear` (DetachTmux's sole caller, per the verified cleanup-site research — safe to widen directly, no other caller needs the old behavior) to also clear `task_shells`:

```rust
pub(super) fn subagent_clear(conn: &mut Connection, task_id: i64) -> Result<SubagentDrain> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM task_subagents WHERE task_id = ?1", params![task_id])?;
    tx.execute("DELETE FROM task_shells WHERE task_id = ?1", params![task_id])?;
    finish_drain(tx, task_id, "subagent_clear")
}
```

Do **NOT** widen `subagent_clear_and_void_pending_stop` — it is shared by `SessionStart`'s CLI clear action (which must NOT clear shells, per the design's session-fencing section) and `DispatchTask`'s two claim functions (which SHOULD clear shells, handled separately in Task 6 via an explicit additional call).

- [x] **Step 4: Write `src/db/queries/shells.rs`**

```rust
//! `task_shells` CRUD — the storage half of the live background-shell count.
//!
//! Mirrors `src/db/queries/subagents.rs`. Every operation rewrites
//! `tasks.live_shells`/`tasks.oldest_live_shell_started_at` from the table in
//! the same transaction.
//!
//! Session fencing (evicting rows whose `session_id` differs from the
//! incoming one) is the only sweep for a dead session's leftover shells —
//! there is deliberately no clear-on-`SessionStart` for shells, unlike
//! subagents. See the "Session fencing" section of
//! docs/superpowers/specs/2026-08-15-shell-visibility-design.md for why: a
//! background shell is an independent OS process that can survive `/clear`
//! or `resume`, where a subagent cannot.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

use crate::models::ShellDrain;

pub(super) fn sync_shell_state(conn: &Connection, task_id: i64) -> Result<i64> {
    let (count, oldest): (i64, Option<String>) = conn
        .query_row(
            "SELECT COUNT(*), MIN(started_at) FROM task_shells WHERE task_id = ?1",
            params![task_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .context("Failed to count task_shells")?;
    conn.execute(
        "UPDATE tasks SET live_shells = ?2, oldest_live_shell_started_at = ?3 WHERE id = ?1",
        params![task_id, count, oldest],
    )
    .context("Failed to sync live_shells")?;
    Ok(count)
}

fn fence_session(conn: &Connection, task_id: i64, session_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM task_shells WHERE task_id = ?1 AND session_id != ?2",
        params![task_id, session_id],
    )
    .context("Failed to fence stale shell session rows")?;
    Ok(())
}

pub(super) fn shell_start(
    conn: &mut Connection,
    task_id: i64,
    shell_id: &str,
    session_id: &str,
    now: DateTime<Utc>,
) -> Result<i64> {
    let tx = conn.unchecked_transaction()?;
    fence_session(&tx, task_id, session_id)?;
    tx.execute(
        "INSERT OR REPLACE INTO task_shells (task_id, shell_id, session_id, started_at) \
         VALUES (?1, ?2, ?3, ?4)",
        params![task_id, shell_id, session_id, now.to_rfc3339()],
    )?;
    let count = sync_shell_state(&tx, task_id)?;
    tx.commit()?;
    Ok(count)
}

pub(super) fn shell_stop(
    conn: &mut Connection,
    task_id: i64,
    shell_id: &str,
    session_id: &str,
) -> Result<ShellDrain> {
    let tx = conn.unchecked_transaction()?;
    fence_session(&tx, task_id, session_id)?;
    tx.execute(
        "DELETE FROM task_shells WHERE task_id = ?1 AND shell_id = ?2",
        params![task_id, shell_id],
    )?;
    let live = sync_shell_state(&tx, task_id)?;
    let applied_pending_stop = super::apply_pending_stop_if_drained(&tx, task_id)?;
    tx.commit()?;
    Ok(ShellDrain {
        live,
        applied_pending_stop,
    })
}

/// Non-draining clear: deletes every `task_shells` row for the task and
/// resyncs the count, but leaves `stop_pending`/status alone. Used by
/// `DetectCrashedAgent` and `DispatchTask`'s two claim functions — see
/// `TaskService::clear_shells_no_drain`. Deliberately NOT called from
/// `SessionStart`'s handler; see the module doc comment.
pub(super) fn shell_clear_no_drain(conn: &mut Connection, task_id: i64) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM task_shells WHERE task_id = ?1", params![task_id])?;
    sync_shell_state(&tx, task_id)?;
    tx.commit()?;
    Ok(())
}
```

Register the module in `src/db/queries/mod.rs`: add `pub(super) mod shells;` alongside the existing `pub(super) mod subagents;`.

- [x] **Step 5: Add the `TaskCrud` trait methods**

In `src/db/mod.rs`, add to the `TaskCrud` trait (alongside `subagent_start`/`subagent_stop`/`subagent_clear`/`subagent_clear_and_void_pending_stop` at lines 223-260):

```rust
    /// Record a live background shell starting for `id` (a Bash tool call
    /// with `run_in_background: true`). Rows belonging to any session other
    /// than `session_id` are evicted first — see the session-fencing section
    /// of `docs/superpowers/specs/2026-08-15-shell-visibility-design.md` for
    /// why shells use fencing alone, with no SessionStart-driven clear.
    /// Returns the resulting live count.
    async fn shell_start(
        &self,
        id: TaskId,
        shell_id: &str,
        session_id: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<i64>;
    /// Record a live background shell stopping for `id`. If this drains the
    /// last shell of a task carrying a deferred `Stop` (and no subagent is
    /// still live), the flip to `Review` is applied in the same transaction.
    async fn shell_stop(
        &self,
        id: TaskId,
        shell_id: &str,
        session_id: &str,
    ) -> Result<ShellDrain>;
    /// Remove every live-shell row for `id` and zero `live_shells`, without
    /// draining. For `DetectCrashedAgent` and `DispatchTask`'s claim
    /// functions — deliberately NOT called from `SessionStart`.
    async fn shell_clear_no_drain(&self, id: TaskId) -> Result<()>;
```

Add `ShellDrain` to this file's imports from `crate::models` if not already covered by a glob import.

In `src/db/queries/tasks.rs`, add the impls (alongside `subagent_start`/`subagent_stop`/`subagent_clear` at lines 520-557):

```rust
    async fn shell_start(
        &self,
        id: TaskId,
        shell_id: &str,
        session_id: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<i64> {
        let shell_id = shell_id.to_string();
        let session_id = session_id.to_string();
        self.db_call(move |conn| super::shells::shell_start(conn, id.0, &shell_id, &session_id, now))
            .await
    }

    async fn shell_stop(&self, id: TaskId, shell_id: &str, session_id: &str) -> Result<ShellDrain> {
        let shell_id = shell_id.to_string();
        let session_id = session_id.to_string();
        self.db_call(move |conn| super::shells::shell_stop(conn, id.0, &shell_id, &session_id))
            .await
    }

    async fn shell_clear_no_drain(&self, id: TaskId) -> Result<()> {
        self.db_call(move |conn| super::shells::shell_clear_no_drain(conn, id.0))
            .await
    }
```

- [x] **Step 6: Run tests to verify they pass**

Run: `cargo test --lib db::tests::shells`
Expected: PASS (6 tests)

- [x] **Step 7: Run the existing subagent tests to confirm no regression**

Run: `cargo test --lib db::tests::subagents`
Expected: PASS — the widened `finish_drain`/`subagent_clear` must not break existing subagent-only behavior (a task with no `task_shells` rows syncs `live_shells = 0`/`oldest_live_shell_started_at = NULL`, which is a no-op against the existing predicate since `live_shells = 0` was already the default).

- [x] **Step 8: Commit**

```bash
git add src/db/queries/shells.rs src/db/queries/mod.rs src/db/queries/subagents.rs src/db/mod.rs src/db/queries/tasks.rs src/db/tests/shells.rs src/db/tests/mod.rs
git commit -m "feat(4187): task_shells CRUD, widen the shared drain predicate for live_shells"
```

---

## Task 4: Widen `try_record_stop`

**Files:**
- Modify: `src/db/queries/tasks.rs:559-629` (`try_record_stop`)
- Test: `src/db/tests/subagents.rs` — this is where `try_record_stop` is already tested today (search `// try_record_stop` around line 534), despite the file's name; add the new test alongside the existing ones there rather than starting a new file.

**Interfaces:**
- Consumes: `tasks.live_shells` column (Task 2).
- Produces: the fixed `try_record_stop` — the actual bug fix for this task's headline scenario.

- [x] **Step 1: Write the failing test**

In `src/db/tests/subagents.rs`, near the existing `try_record_stop` tests (which use the `set_running(db, task)` helper at line 537-547, built on `TaskPatch`). `live_shells` is **not** exposed via `TaskPatch` — same reasoning as `live_subagents`'s deliberate exclusion (see `src/db/mod.rs`'s doc comment on `TaskPatch`: it's a denormalised count owned exclusively by its query module's transactional writes) — so seed it with a direct `db_call`:

```rust
#[tokio::test]
async fn try_record_stop_defers_when_a_shell_is_live_and_no_subagents_are() {
    let db = in_memory_db().await;
    let task = create_task_returning(&db, "t", "d", "/r", None, TaskStatus::Backlog)
        .await
        .unwrap();
    set_running(&db, &task).await;
    db.db_call({
        let task_id = task.id;
        move |conn| {
            conn.execute(
                "UPDATE tasks SET live_shells = 1 WHERE id = ?1",
                rusqlite::params![task_id.0],
            )
            .map_err(anyhow::Error::from)
        }
    })
    .await
    .unwrap();

    let outcome = db.try_record_stop(task.id, Utc::now()).await.unwrap();
    assert_eq!(
        outcome,
        StopOutcome::Deferred,
        "a live background shell with no subagents must defer the Stop, not flip to Review \
         -- this is the regression test for #4187's core bug"
    );
    let reread = db.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(reread.status, TaskStatus::Running, "must stay Running, not flip to review");
}
```

- [x] **Step 2: Run it to verify it fails**

Run: `cargo test --lib try_record_stop_defers_when_a_shell_is_live_and_no_subagents_are`
Expected: FAIL — `outcome` is `Flipped`, not `Deferred` (the bug this task fixes).

- [x] **Step 3: Widen both conditional `UPDATE`s in `try_record_stop`**

```rust
async fn try_record_stop(
    &self,
    id: TaskId,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<StopOutcome> {
    let deferred_at = super::format_datetime_millis(now);
    self.db_call(move |conn| {
        let tx = conn.unchecked_transaction()?;
        let review = TaskStatus::Review;
        let flipped = tx.execute(
            &format!(
                "UPDATE tasks {} \
                 WHERE id = ?3 AND status = ?4 AND live_subagents = 0 AND live_shells = 0",
                super::STOP_FLIP_SET
            ),
            params![review.as_str(), SubStatus::default_for(review).as_str(), id.0, TaskStatus::Running.as_str()],
        )?;
        let outcome = if flipped == 1 {
            StopOutcome::Flipped
        } else {
            let deferred = tx.execute(
                "UPDATE tasks \
                 SET stop_pending = 1, stop_pending_at = ?3, updated_at = datetime('now') \
                 WHERE id = ?1 AND status = ?2 AND (live_subagents > 0 OR live_shells > 0)",
                params![id.0, TaskStatus::Running.as_str(), deferred_at],
            )?;
            if deferred == 1 { StopOutcome::Deferred } else { StopOutcome::NoOp }
        };
        tx.commit()?;
        Ok(outcome)
    }).await
}
```

- [x] **Step 4: Run test to verify it passes**

Run: `cargo test --lib try_record_stop_defers_when_a_shell_is_live_and_no_subagents_are`
Expected: PASS

- [x] **Step 5: Run existing `try_record_stop`/subagent-focused tests to confirm no regression**

Run: `cargo test --lib db::tests::subagents`
Expected: PASS — a task with `live_shells = 0` (the default) behaves exactly as before.

- [x] **Step 6: Commit**

```bash
git add src/db/queries/tasks.rs src/db/tests/subagents.rs
git commit -m "fix(4187): try_record_stop defers Running->Review while a background shell is live"
```

---

## Task 5: Models — `Task` fields, `ShellEvent`, `ShellDrain`, `SubStatus::StaleShell`, `AgentActivity::StaleShell`

**Files:**
- Modify: `src/models/tasks.rs`
- Modify: `src/db/queries/mod.rs` (`TASK_COLUMNS`, `row_to_task`)

**Interfaces:**
- Produces: `Task.live_shells: i64`, `Task.oldest_live_shell_started_at: Option<DateTime<Utc>>`; `ShellEvent { Start { shell_id: String, session_id: String }, Stop { shell_id: String, session_id: String } }`; `ShellDrain { live: i64, applied_pending_stop: bool }`; `SubStatus::StaleShell`; `AgentActivity::StaleShell`; `classify_agent_activity`'s new signature — consumed by Task 3 (already forward-referenced there), Task 6 (service layer), Task 9-10 (hook CLI/card rendering).

- [x] **Step 1: Write the failing model tests**

In `src/models/tasks.rs`'s existing `#[cfg(test)] mod activity_tests` (right after the existing `classify_agent_activity` tests, following their exact style — e.g. the test using `at(N, now)`/`past`/`long_ago` helpers already defined there):

```rust
#[test]
fn classify_agent_activity_stays_active_with_a_fresh_live_shell() {
    let now = Utc::now();
    let recent = now - Duration::minutes(30);
    assert_eq!(
        classify_agent_activity(None, None, 0, 1, Some(recent), now),
        AgentActivity::Active,
        "a live shell younger than the shell-stale threshold must read Active, \
         not Stale -- this is #4187's staleness-exemption fix"
    );
}

#[test]
fn classify_agent_activity_flags_a_shell_running_past_the_stale_threshold() {
    let now = Utc::now();
    let ancient = now - SHELL_STALE_THRESHOLD - Duration::minutes(1);
    assert_eq!(
        classify_agent_activity(None, None, 0, 1, Some(ancient), now),
        AgentActivity::StaleShell,
        "a live shell older than shell_stale_threshold must surface distinctly, \
         not render identically to a healthy long-running one forever"
    );
}

#[test]
fn classify_agent_activity_prefers_live_subagents_over_a_stale_shell() {
    let now = Utc::now();
    let ancient = now - SHELL_STALE_THRESHOLD - Duration::minutes(1);
    assert_eq!(
        classify_agent_activity(None, None, 1, 1, Some(ancient), now),
        AgentActivity::Active,
        "a genuinely live subagent must win over an old-looking shell"
    );
}
```

(Note: existing tests in this module call `classify_agent_activity` with 4 args — those will need updating to the new 6-arg signature as part of Step 3 below, or they'll fail to compile. Update every existing call site in this file to pass `0, None` for the two new params where the test doesn't care about shells.)

- [x] **Step 2: Run tests to verify they fail (compile error)**

Run: `cargo test --lib models::tasks::activity_tests`
Expected: FAIL to compile — wrong arg count.

**Do NOT add `live_shells`/`oldest_live_shell_started_at` to `TaskPatch`** (`src/db/mod.rs`, the `patch_struct!` block around line 77-111). `live_subagents` is deliberately excluded there today, with a doc comment explaining why: it's a denormalised count owned exclusively by its query module's transactional writes, and excluding it from the generic patch surface makes "no handler can desync the count" a compile-time property. `live_shells`/`oldest_live_shell_started_at` follow the identical reasoning — they're owned exclusively by `src/db/queries/shells.rs` (Task 3).

- [x] **Step 3: Add `Task` fields**

In the `Task` struct (`src/models/tasks.rs:292-327`), add after `stop_pending: bool`:

```rust
    /// Number of currently-live backgrounded shells (Bash tool with
    /// `run_in_background: true`). Denormalised `COUNT(*)` over
    /// `task_shells`. See `classify_agent_activity` and the running card's
    /// "· N shells" label.
    pub live_shells: i64,
    /// Timestamp of the oldest currently-live `task_shells` row for this
    /// task, used to detect an abandoned shell past `SHELL_STALE_THRESHOLD`.
    /// `None` when `live_shells == 0`.
    pub oldest_live_shell_started_at: Option<DateTime<Utc>>,
```

- [x] **Step 4: Add `ShellEvent` and `ShellDrain`**

Right after `SubagentEvent` (`src/models/tasks.rs:740-761`):

```rust
/// A Claude Code background-shell lifecycle event, forwarded by
/// `task-status-hook` via `dispatch hook-shell`. Mirrors [`SubagentEvent`]
/// but has no `Clear` variant: DetachTmux's shell-clearing rides on the
/// existing `subagent_clear` DB function (widened to also touch
/// `task_shells`), and there is deliberately no SessionStart-driven clear
/// for shells — see `docs/superpowers/specs/2026-08-15-shell-visibility-design.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellEvent {
    /// A backgrounded Bash call was launched (`PostToolUse`, not
    /// `PreToolUse` — the shell_id doesn't exist until the call returns).
    Start { shell_id: String, session_id: String },
    /// `KillBash`, or `BashOutput` reporting the shell is no longer running.
    Stop { shell_id: String, session_id: String },
}
```

Right after `SubagentDrain` (find its closing brace, around line 840):

```rust
/// Result of a shell mutation that can drain the last live shell. Mirrors
/// [`SubagentDrain`] exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellDrain {
    pub live: i64,
    pub applied_pending_stop: bool,
}
```

- [x] **Step 5: Add `SubStatus::StaleShell` — all five touch points**

In `SubStatus` enum (line 138-148), add `StaleShell,` after `Stale,`.

In `ALL` (line 151-161), add `SubStatus::StaleShell,` after `SubStatus::Stale,`.

In `is_valid_for`'s `Running` arm (line 167-174), add `| SubStatus::StaleShell` to the `matches!` list.

In `properties()` (line 210-251), add an arm reusing the existing staleness priority slot (both are staleness variants, and `PRIORITY_ACTIVE_SLOT`'s precedent already shows multiple variants legitimately sharing one priority):

```rust
            SubStatus::StaleShell => SubStatusProperties {
                priority: PRIORITY_STALE,
                header_label: "shell stale",
            },
```

In the `define_str_enum!` macro invocation (line 272-282), add `StaleShell => "stale_shell",`.

- [x] **Step 6: Add `AgentActivity::StaleShell` and `SHELL_STALE_THRESHOLD`**

Right after `ACTIVE_THRESHOLD` (line 914):

```rust
/// Time a background shell may stay live before it's flagged distinctly as
/// possibly-abandoned rather than exempted from staleness forever. Much
/// longer than `ACTIVE_THRESHOLD` because a legitimate dev server or long
/// build can run for hours; see the "ClassifyAgentActivity change" section of
/// docs/superpowers/specs/2026-08-15-shell-visibility-design.md.
pub const SHELL_STALE_THRESHOLD: chrono::Duration = chrono::Duration::hours(4);
```

Update `AgentActivity` (line 920-924):

```rust
pub enum AgentActivity {
    Active,
    Waiting,
    Stale,
    StaleShell,
}
```

Update `to_sub_status` (line 928-934):

```rust
    pub fn to_sub_status(self) -> SubStatus {
        match self {
            AgentActivity::Active => SubStatus::Active,
            AgentActivity::Waiting => SubStatus::NeedsInput,
            AgentActivity::Stale => SubStatus::Stale,
            AgentActivity::StaleShell => SubStatus::StaleShell,
        }
    }
```

- [x] **Step 7: Widen `classify_agent_activity`'s signature**

```rust
pub fn classify_agent_activity(
    last_pre_tool_use_at: Option<chrono::DateTime<chrono::Utc>>,
    last_notification_at: Option<chrono::DateTime<chrono::Utc>>,
    live_subagents: i64,
    live_shells: i64,
    oldest_live_shell_started_at: Option<chrono::DateTime<chrono::Utc>>,
    now: chrono::DateTime<chrono::Utc>,
) -> AgentActivity {
    if let Some(notif) = last_notification_at {
        let notif_is_newer = last_pre_tool_use_at.is_none_or(|p| notif > p);
        if notif_is_newer {
            return AgentActivity::Waiting;
        }
    }
    if live_subagents > 0 {
        return AgentActivity::Active;
    }
    if live_shells > 0 {
        let stale_shell = oldest_live_shell_started_at
            .is_some_and(|ts| now.signed_duration_since(ts) > SHELL_STALE_THRESHOLD);
        return if stale_shell {
            AgentActivity::StaleShell
        } else {
            AgentActivity::Active
        };
    }
    match last_pre_tool_use_at {
        Some(ts) if now.signed_duration_since(ts) <= ACTIVE_THRESHOLD => AgentActivity::Active,
        _ => AgentActivity::Stale,
    }
}
```

Update every existing call site in this file's test modules to pass the two new params (`0, None` where a test doesn't care about shells).

- [x] **Step 8: Add `live_shells`/`oldest_live_shell_started_at` to `TASK_COLUMNS` and `row_to_task`**

In `src/db/queries/mod.rs`, append to `TASK_COLUMNS` (line 111-115):

```rust
pub(super) const TASK_COLUMNS: &str =
    "id, title, description, repo_path, status, worktree, tmux_window, \
     plan_path, epic_id, sub_status, url, url_type, tag, sort_order, base_branch, external_id, \
     created_at, updated_at, labels, last_pre_tool_use_at, last_notification_at, \
     wrap_up_mode, auto_run_plan, live_subagents, stop_pending, \
     live_shells, oldest_live_shell_started_at";
```

In `row_to_task` (line 158-191), add after `stop_pending: row.get("stop_pending")?,`:

```rust
        live_shells: row.get("live_shells")?,
        oldest_live_shell_started_at: read_optional_datetime(row, "oldest_live_shell_started_at")?,
```

- [x] **Step 9: Fix every construction site of `Task` that lists fields explicitly**

Run: `cargo build 2>&1 | grep "missing field"` and add `live_shells: 0, oldest_live_shell_started_at: None,` (or the appropriate test value) to every struct literal the compiler flags — likely test helpers like `make_task` in `src/tui/tests/helpers.rs` and any fixture builders under `src/db/tests/`.

- [x] **Step 10: Run tests to verify they pass**

Run: `cargo test --lib models::tasks`
Expected: PASS

- [x] **Step 11: Run the full lib test suite to catch any remaining compile fallout**

Run: `cargo build --all-targets 2>&1 | tee /tmp/build.log; grep -E "error" /tmp/build.log`
Expected: no errors. Fix any remaining call sites the earlier grep missed.

- [x] **Step 12: Commit**

```bash
git add src/models/tasks.rs src/db/queries/mod.rs
git commit -m "feat(4187): Task.live_shells, ShellEvent/ShellDrain, SubStatus::StaleShell"
```

---

## Task 6: Service layer — wire shells into every real structural clear point

**Files:**
- Modify: `src/service/tasks/crud.rs` (`record_hook_event`, `claim_backlog_task`, `claim_next_backlog_task`, new `record_shell_event`/`clear_shells_no_drain`)
- Modify: `src/service/api.rs` (the `TaskServiceApi`/`TaskServiceApiStub` macro-generated trait — register the two new `TaskService` methods here too, so mocked/stubbed callers elsewhere see them)
- Modify: `src/runtime/tasks.rs` (`exec_clear_subagents`)
- Modify: `src/tui/update/agent.rs` (`handle_agent_crashed`, `tick_sub_status`)

**Interfaces:**
- Consumes: `Database::shell_start`/`shell_stop`/`shell_clear_no_drain` (Task 3, exposed via the `TaskCrud` trait), `ShellEvent`/`ShellDrain` (Task 5).
- Produces: `TaskService::record_shell_event(id, ShellEvent) -> Result<(), ServiceError>`, `TaskService::clear_shells_no_drain(id) -> Result<(), ServiceError>` — consumed by Task 7 (CLI).

- [x] **Step 1: Register the two new methods in the `TaskServiceApi` mock/stub macro**

`src/service/api.rs` generates a `TaskServiceApi` trait (and a `TaskServiceApiStub`) from a macro invocation that lists `TaskService`'s hook-facing methods — `record_subagent_event`/`clear_subagents_no_drain` are declared there at lines 253-267. Add matching entries for the two new methods, in the same macro invocation, right after `clear_subagents_no_drain`:

```rust
            /// Record a shell lifecycle event and, when it drains the last
            /// live shell for a task carrying a deferred Stop (with no
            /// subagent still live either), apply that Stop. See
            /// `HookShellStart`/`HookShellStop` in
            /// `docs/specs/agent-health.allium`.
            async fn record_shell_event(
                &self,
                id: $crate::models::TaskId,
                event: $crate::models::ShellEvent
            ) -> Result<(), $crate::service::ServiceError>;

            /// Clear a task's shell entries without draining — for
            /// `DetectCrashedAgent` and `DispatchTask`'s claim functions.
            /// Deliberately NOT called from `SessionStart`; see
            /// `docs/superpowers/specs/2026-08-15-shell-visibility-design.md`.
            async fn clear_shells_no_drain(
                &self,
                id: $crate::models::TaskId
            ) -> Result<(), $crate::service::ServiceError>;
```

- [x] **Step 2: Write the failing service-layer tests**

Real helpers, verified against `src/service/tasks/tests.rs`: `test_db().await` builds an in-memory DB, `task_svc(&db)` builds a `TaskService`, `create_running_task(&svc, sub_status: SubStatus).await -> TaskId` seeds a Running task, and `svc.get_task(id).await.unwrap()` returns `Task` directly (not `Option<Task>`). Following `detach_clear_drains_and_performs_a_pending_flip` (lines 2987-3018) and `clear_no_drain_voids_a_pending_stop_without_flipping_to_review` (lines 3023-3049) as exact templates:

```rust
#[tokio::test]
async fn record_shell_event_start_increments_live_shells() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = create_running_task(&svc, SubStatus::Active).await;

    svc.record_shell_event(
        id,
        ShellEvent::Start { shell_id: "bash_1".into(), session_id: "sess_1".into() },
    )
    .await
    .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.live_shells, 1);
}

#[tokio::test]
async fn shell_stop_drains_a_deferred_stop_to_review() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = create_running_task(&svc, SubStatus::Active).await;

    svc.record_shell_event(
        id,
        ShellEvent::Start { shell_id: "bash_1".into(), session_id: "sess_1".into() },
    )
    .await
    .unwrap();
    svc.record_hook_event(id, HookEventKind::Stop).await.unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Running,
        "Stop must defer, not flip, while a shell is live -- #4187's core bug"
    );

    svc.record_shell_event(
        id,
        ShellEvent::Stop { shell_id: "bash_1".into(), session_id: "sess_1".into() },
    )
    .await
    .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Review);
}

#[tokio::test]
async fn clear_shells_no_drain_zeroes_live_shells_without_touching_status() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = create_running_task(&svc, SubStatus::Active).await;

    svc.record_shell_event(
        id,
        ShellEvent::Start { shell_id: "bash_1".into(), session_id: "sess_1".into() },
    )
    .await
    .unwrap();

    svc.clear_shells_no_drain(id).await.unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.live_shells, 0);
    assert_eq!(task.status, TaskStatus::Running);
}
```

- [x] **Step 3: Run tests to verify they fail**

Run: `cargo test --lib service::tasks::tests::record_shell_event`
Expected: FAIL to compile — methods don't exist.

- [x] **Step 4: Add `record_shell_event`/`clear_shells_no_drain` to `TaskService`**

In `src/service/tasks/crud.rs`, near `record_subagent_event`/`clear_subagents_no_drain`:

```rust
pub async fn record_shell_event(&self, id: TaskId, event: ShellEvent) -> Result<(), ServiceError> {
    if !self.db.task_exists(id).await? {
        return Err(Self::task_not_found(id));
    }
    let applied_pending_stop = match event {
        ShellEvent::Start { shell_id, session_id } => {
            self.db.shell_start(id, &shell_id, &session_id, self.clock.now()).await?;
            false
        }
        ShellEvent::Stop { shell_id, session_id } => {
            self.db.shell_stop(id, &shell_id, &session_id).await?.applied_pending_stop
        }
    };
    if applied_pending_stop {
        self.recalculate_epic_for_task(id).await;
    }
    Ok(())
}

/// Non-draining clear. Called from `DetectCrashedAgent`'s no-drain path and
/// `DispatchTask`'s two claim functions. Deliberately NOT called from
/// `SessionStart` handling — see `docs/superpowers/specs/2026-08-15-shell-visibility-design.md`.
pub async fn clear_shells_no_drain(&self, id: TaskId) -> Result<(), ServiceError> {
    if !self.db.task_exists(id).await? {
        return Err(Self::task_not_found(id));
    }
    self.db.shell_clear_no_drain(id).await?;
    Ok(())
}
```

- [x] **Step 5: Wire the two `DispatchTask` claim functions**

In `claim_next_backlog_task` (`src/service/tasks/crud.rs:890-913`), right after the existing `self.clear_subagents_no_drain(claimed_id).await?;`:

```rust
        // No-drain: mirrors the subagent clear above, guarding against shell
        // entries left over from a prior run of this task.
        self.clear_shells_no_drain(claimed_id).await?;
```

In `claim_backlog_task` (`:935-948`), same pattern right after its `self.clear_subagents_no_drain(task_id).await?;`:

```rust
        self.clear_shells_no_drain(task_id).await?;
```

- [x] **Step 6: Wire `exec_clear_subagents`'s `NoDrain` branch**

In `src/runtime/tasks.rs:206-218`:

```rust
    pub(super) async fn exec_clear_subagents(&self, id: models::TaskId, mode: models::DrainMode) {
        let result = match mode {
            models::DrainMode::Drain => {
                self.task_svc
                    .record_subagent_event(id, models::SubagentEvent::Clear)
                    .await
            }
            models::DrainMode::NoDrain => self.task_svc.clear_subagents_no_drain(id).await,
        };
        if let Err(e) = result {
            tracing::warn!(task_id = id.0, error = %e, "failed to clear subagent entries");
        }
        // Shells: the Drain branch above clears them for free (subagent_clear
        // is widened at the DB layer to also touch task_shells). NoDrain
        // (crash detection) needs its own call, since clear_subagents_no_drain
        // is also reached by SessionStart, which must NOT clear shells.
        if mode == models::DrainMode::NoDrain {
            if let Err(e) = self.task_svc.clear_shells_no_drain(id).await {
                tracing::warn!(task_id = id.0, error = %e, "failed to clear shell entries");
            }
        }
    }
```

(`DrainMode` needs `PartialEq` — it already derives `PartialEq, Eq` per Task 5's read of its definition, so `mode == models::DrainMode::NoDrain` compiles as-is.)

- [x] **Step 7: Wire `handle_agent_crashed`'s in-memory bookkeeping**

In `src/tui/update/agent.rs:453-481`, add alongside the existing `task.live_subagents = 0;`:

```rust
            task.live_subagents = 0;
            task.live_shells = 0;
```

- [x] **Step 8: Wire `record_hook_event`'s `PreToolUse` branch and `tick_sub_status` to the new `classify_agent_activity` signature**

In `src/service/tasks/crud.rs` (`record_hook_event`, around line 733), change:

```rust
            let activity = classify_agent_activity(Some(now), task.last_notification_at, task.live_subagents, now);
```

to:

```rust
            let activity = classify_agent_activity(
                Some(now),
                task.last_notification_at,
                task.live_subagents,
                task.live_shells,
                task.oldest_live_shell_started_at,
                now,
            );
```

In `src/tui/update/agent.rs`'s `tick_sub_status` (line 295-330), the call at lines 304-309 currently reads:

```rust
                let activity = crate::models::classify_agent_activity(
                    t.last_pre_tool_use_at,
                    t.last_notification_at,
                    t.live_subagents,
                    now,
                );
```

Change it to:

```rust
                let activity = crate::models::classify_agent_activity(
                    t.last_pre_tool_use_at,
                    t.last_notification_at,
                    t.live_subagents,
                    t.live_shells,
                    t.oldest_live_shell_started_at,
                    now,
                );
```

- [x] **Step 9: Run tests to verify they pass**

Run: `cargo test --lib service::tasks`
Expected: PASS

- [x] **Step 10: Run the full lib suite**

Run: `cargo build --all-targets 2>&1 | tee /tmp/build2.log; grep -E "error" /tmp/build2.log`
Expected: no errors.

- [x] **Step 11: Commit**

```bash
git add src/service/tasks/crud.rs src/service/api.rs src/runtime/tasks.rs src/tui/update/agent.rs
git commit -m "feat(4187): wire live_shells through record_hook_event, tick_sub_status, and every real cleanup site"
```

---

## Task 7: CLI — `dispatch hook-shell` subcommand

**Files:**
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: `TaskService::record_shell_event` (Task 6).
- Produces: `dispatch hook-shell <id> <start|stop> --shell-id <id> --session-id <id>` — consumed by Task 8 (hook script).

- [x] **Step 1: Write the failing CLI test**

`tests/cli.rs` already covers `hook-subagent` as a full-binary integration test (search `// hook-subagent`, lines 628-700) using `binary()` (spawns the compiled `dispatch` binary), `seed_running_task(db.path(), title, sub_status)`, and `Database::open(db.path())` to re-read state. Add an analogous `// hook-shell` section following `hook_subagent_start_then_stop_round_trips` (lines 640-700) exactly:

```rust
#[tokio::test]
async fn hook_shell_start_then_stop_round_trips() {
    let db = NamedTempFile::new().unwrap();
    let db_path = db.path().to_str().unwrap();
    let id = seed_running_task(db.path(), "Shell Test", SubStatus::Active).await;

    let out = binary()
        .args([
            "--db", db_path, "hook-shell", &id.0.to_string(), "start",
            "--shell-id", "bash_1", "--session-id", "s1",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

    let conn = Database::open(db.path()).await.unwrap();
    let task = conn.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.live_shells, 1, "expected live_shells to be 1 after start");
    drop(conn);

    let out = binary()
        .args([
            "--db", db_path, "hook-shell", &id.0.to_string(), "stop",
            "--shell-id", "bash_1", "--session-id", "s1",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

    let conn = Database::open(db.path()).await.unwrap();
    let task = conn.get_task(id).await.unwrap().unwrap();
    assert_eq!(task.live_shells, 0, "expected live_shells to be 0 after the matching stop");
}
```

- [x] **Step 2: Run it to verify it fails**

Run: `cargo test --test cli hook_shell_start_then_stop_round_trips`
Expected: FAIL — subcommand doesn't exist.

- [x] **Step 3: Add the `HookShell` clap variant**

In `src/main.rs`, alongside `Hook`/`HookSubagent`/`HookFileEvent`:

```rust
    HookShell {
        id: i64,
        action: String, // start | stop
        #[arg(long = "shell-id")]
        shell_id: Option<String>,
        #[arg(long = "session-id")]
        session_id: Option<String>,
    },
```

- [x] **Step 4: Add `cmd_hook_shell`**

```rust
async fn cmd_hook_shell(
    db: &Path,
    id: i64,
    action: String,
    shell_id: Option<String>,
    session_id: Option<String>,
) -> Result<()> {
    let (Some(shell_id), Some(session_id)) = (shell_id, session_id) else {
        return Ok(());
    };
    let event = match action.as_str() {
        "start" => models::ShellEvent::Start { shell_id, session_id },
        "stop" => models::ShellEvent::Stop { shell_id, session_id },
        other => anyhow::bail!("Invalid shell action: {other}. Valid: start, stop"),
    };
    let svc = open_hook_service(db).await?;
    let outcome = svc.record_shell_event(models::TaskId(id), event).await;
    report_hook_outcome(id, outcome)
}
```

Wire the match arm dispatching to it, following the existing `HookSubagent { .. } => cmd_hook_subagent(...)` pattern.

- [x] **Step 5: Run test to verify it passes**

Run: the test from Step 1
Expected: PASS

- [x] **Step 6: Commit**

```bash
git add src/main.rs
git commit -m "feat(4187): dispatch hook-shell CLI subcommand"
```

---

## Task 8: Hook script — Bash/KillBash/BashOutput branches

**Files:**
- Modify: `plugin/hooks/scripts/task-status-hook`

**Interfaces:**
- Consumes: `dispatch hook-shell` (Task 7).
- Produces: the actual detection wiring — consumed by Task 9's tests.

**Implementation-time risk (per the design doc — resolve BEFORE writing the parsing logic below):** capture real Claude Code hook payloads for a backgrounded `Bash` call, a `KillBash` call, and a `BashOutput` call, to confirm the exact field names for `tool_input.run_in_background`, the assigned shell_id (likely in `tool_response`, not `tool_input`, for the `Bash` case — the ID doesn't exist until the call returns), `tool_input.shell_id` for `KillBash`/`BashOutput`, and whatever field `BashOutput`'s `tool_response` uses to report status. The field names below (`run_in_background`, `shell_id`, `status`) are the design's best guess, not confirmed — adjust them to match real payloads before trusting the script.

- [x] **Step 1: Update `docs/superpowers/specs/2026-08-15-shell-visibility-design.md`'s Open Questions section with the confirmed field names once captured, before writing code**

This is a documentation-only step but it's TDD-load-bearing: the tests in Task 9 assert on these exact field names, so get them right here first.

- [x] **Step 2: Add the Bash/KillBash/BashOutput branches to the `PostToolUse` case**

In `plugin/hooks/scripts/task-status-hook`, extend the existing `case "$TOOL" in Read|Write|Edit) ... esac` block (inside the `if [[ "$EVENT" == "PostToolUse" ]]` branch) with new cases for shell detection, run *before* or alongside the file-event switch:

```bash
        case "$TOOL" in
            Bash)
                RUN_IN_BACKGROUND=$(echo "$INPUT" | jq -r '.tool_input.run_in_background // false')
                if [[ "$RUN_IN_BACKGROUND" == "true" ]]; then
                    SHELL_ID=$(echo "$INPUT" | jq -r '.tool_response.shell_id // empty')
                    SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // empty')
                    if [[ -n "$SHELL_ID" ]]; then
                        dispatch hook-shell "$ID" start --shell-id "$SHELL_ID" --session-id "$SESSION_ID"
                    fi
                fi
                ;;
            KillBash)
                SHELL_ID=$(echo "$INPUT" | jq -r '.tool_input.shell_id // empty')
                SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // empty')
                if [[ -n "$SHELL_ID" ]]; then
                    dispatch hook-shell "$ID" stop --shell-id "$SHELL_ID" --session-id "$SESSION_ID"
                fi
                ;;
            BashOutput)
                STATUS=$(echo "$INPUT" | jq -r '.tool_response.status // empty')
                if [[ -n "$STATUS" && "$STATUS" != "running" ]]; then
                    SHELL_ID=$(echo "$INPUT" | jq -r '.tool_input.shell_id // empty')
                    SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // empty')
                    if [[ -n "$SHELL_ID" ]]; then
                        dispatch hook-shell "$ID" stop --shell-id "$SHELL_ID" --session-id "$SESSION_ID"
                    fi
                fi
                ;;
        esac
```

Place this as an additional `case "$TOOL" in ... esac` block alongside (not replacing) the existing `Read|Write|Edit) ... NotebookEdit) ... esac` file-event block — both switches run independently on the same `$TOOL`/`$INPUT`.

- [x] **Step 3: Run `bash -n` to sanity-check the script parses**

Run: `bash -n plugin/hooks/scripts/task-status-hook`
Expected: no syntax errors.

- [x] **Step 4: Commit**

```bash
git add plugin/hooks/scripts/task-status-hook
git commit -m "feat(4187): forward backgrounded Bash/KillBash/BashOutput to dispatch hook-shell"
```

(Tests come in Task 9 — this task's script change is verified there, matching this repo's convention of testing the embedded script via `src/setup/hooks.rs`.)

---

## Task 9: Hook script tests

**Files:**
- Modify: `src/setup/hooks.rs`

**Interfaces:**
- Consumes: Task 8's script changes.
- Produces: regression coverage for the detection logic.

- [x] **Step 1: Write the failing tests**

Following `hook_forwards_subagent_start_and_stop` (`src/setup/hooks.rs:474-495`) and `hook_does_not_forward_file_event_for_untracked_tool` (`:420-436`) as exact templates:

```rust
#[cfg(unix)]
#[test]
fn hook_forwards_backgrounded_bash_as_shell_start() {
    let (_tmp, repo, script_path, observed, path) = spawn_hook_harness("231-bash-bg");
    let payload = format!(
        r#"{{"cwd":"{}","hook_event_name":"PostToolUse","tool_name":"Bash","session_id":"sess_9","tool_input":{{"command":"npm run dev","run_in_background":true}},"tool_response":{{"shell_id":"bash_1"}}}}"#,
        repo.display()
    );
    invoke_hook(&script_path, &repo, &path, &payload);

    let log = std::fs::read_to_string(&observed).unwrap_or_default();
    assert!(
        log.contains("hook-shell 231 start")
            && log.contains("--shell-id bash_1")
            && log.contains("--session-id sess_9"),
        "expected a backgrounded Bash call to forward as hook-shell start; got: {log:?}"
    );
}

#[cfg(unix)]
#[test]
fn hook_does_not_forward_shell_start_for_a_foreground_bash_call() {
    let (_tmp, repo, script_path, observed, path) = spawn_hook_harness("232-bash-fg");
    let payload = format!(
        r#"{{"cwd":"{}","hook_event_name":"PostToolUse","tool_name":"Bash","session_id":"sess_9","tool_input":{{"command":"cargo test"}},"tool_response":{{}}}}"#,
        repo.display()
    );
    invoke_hook(&script_path, &repo, &path, &payload);

    let log = std::fs::read_to_string(&observed).unwrap_or_default();
    assert!(
        !log.contains("hook-shell"),
        "a plain (non-backgrounded) Bash call must not forward as a shell event; got: {log:?}"
    );
}

#[cfg(unix)]
#[test]
fn hook_forwards_kill_bash_as_shell_stop() {
    let (_tmp, repo, script_path, observed, path) = spawn_hook_harness("233-killbash");
    let payload = format!(
        r#"{{"cwd":"{}","hook_event_name":"PostToolUse","tool_name":"KillBash","session_id":"sess_9","tool_input":{{"shell_id":"bash_1"}}}}"#,
        repo.display()
    );
    invoke_hook(&script_path, &repo, &path, &payload);

    let log = std::fs::read_to_string(&observed).unwrap_or_default();
    assert!(
        log.contains("hook-shell 233 stop")
            && log.contains("--shell-id bash_1")
            && log.contains("--session-id sess_9"),
        "expected KillBash to forward as hook-shell stop; got: {log:?}"
    );
}

#[cfg(unix)]
#[test]
fn hook_forwards_bash_output_as_shell_stop_only_when_no_longer_running() {
    let (_tmp, repo, script_path, observed, path) = spawn_hook_harness("234-bashoutput");
    let still_running = format!(
        r#"{{"cwd":"{}","hook_event_name":"PostToolUse","tool_name":"BashOutput","session_id":"sess_9","tool_input":{{"shell_id":"bash_1"}},"tool_response":{{"status":"running"}}}}"#,
        repo.display()
    );
    invoke_hook(&script_path, &repo, &path, &still_running);
    let log = std::fs::read_to_string(&observed).unwrap_or_default();
    assert!(
        !log.contains("hook-shell"),
        "a BashOutput poll reporting status=running must not forward a stop; got: {log:?}"
    );

    let completed = format!(
        r#"{{"cwd":"{}","hook_event_name":"PostToolUse","tool_name":"BashOutput","session_id":"sess_9","tool_input":{{"shell_id":"bash_1"}},"tool_response":{{"status":"completed"}}}}"#,
        repo.display()
    );
    invoke_hook(&script_path, &repo, &path, &completed);
    let log = std::fs::read_to_string(&observed).unwrap_or_default();
    assert!(
        log.contains("hook-shell 234 stop") && log.contains("--shell-id bash_1"),
        "a BashOutput poll reporting status=completed must forward a stop; got: {log:?}"
    );
}
```

- [x] **Step 2: Run tests to verify they fail**

These tests are numbered as Task 9 for review/commit granularity (script-change and script-tests are separately reviewable diffs), but they must be *written and run red* before Task 8's script edit lands, to get genuine TDD red-green rather than a same-commit rubber stamp. Concretely: write this task's Step 1 tests, temporarily stash or skip Task 8's script edit (or simply do these tests' Step 1 and Step 2 before touching the script at all), and confirm:

Run: `cargo test --lib hook_forwards_backgrounded_bash_as_shell_start hook_does_not_forward_shell_start_for_a_foreground_bash_call hook_forwards_kill_bash_as_shell_stop hook_forwards_bash_output_as_shell_stop_only_when_no_longer_running`
Expected: FAIL — the unmodified script has no Bash/KillBash/BashOutput branches yet, so none of these forward anything.

Then perform Task 8's Step 2 (the script edit) before returning to this task's Step 3.

- [x] **Step 3: Run tests to verify they pass**

Run: same command as Step 2
Expected: PASS (assuming Task 8's script change is in place)

- [x] **Step 4: Run the full `src/setup/hooks.rs` suite to confirm no regression**

Run: `cargo test --lib setup::hooks`
Expected: PASS

- [x] **Step 5: Commit**

```bash
git add src/setup/hooks.rs
git commit -m "test(4187): cover Bash/KillBash/BashOutput forwarding to hook-shell"
```

---

## Task 10: Card rendering — `CardIndicator::Running { shells }` and a `StaleShell` variant

**Files:**
- Modify: `src/tui/ui/kanban/cards.rs`

**Interfaces:**
- Consumes: `Task.live_shells`, `SubStatus::StaleShell` (Task 5).
- Produces: the visible feature — "running · N shells" label and a distinct stale-shell indicator.

- [ ] **Step 1: Write the failing inline unit tests**

`cards.rs`'s own `mod tests` (starting line 497) already has everything needed: `make_task(id, status)` (a plain sync helper, no DB), `App::new(vec![...])`, and a `label_of(indicator: CardIndicator) -> String` helper (line 635-641) that renders and flattens the label text — reuse it rather than re-deriving the span extraction. Following `running_card_shows_subagent_count`/`running_card_uses_the_singular_for_one_subagent`/`running_card_omits_the_suffix_at_zero` (lines 643-663) as exact templates:

```rust
#[test]
fn running_card_shows_shell_count() {
    let mut task = make_task(1, TaskStatus::Running);
    task.live_shells = 2;
    let app = App::new(vec![]);
    let indicator = classify_card_indicator(&task, task.status, &app, Utc::now());
    assert_eq!(indicator, CardIndicator::Running { subagents: 0, shells: 2 });
}

#[test]
fn running_card_composes_subagents_and_shells() {
    let mut task = make_task(1, TaskStatus::Running);
    task.live_subagents = 1;
    task.live_shells = 1;
    let app = App::new(vec![]);
    let indicator = classify_card_indicator(&task, task.status, &app, Utc::now());
    assert_eq!(indicator, CardIndicator::Running { subagents: 1, shells: 1 });
}

#[test]
fn stale_shell_sub_status_produces_a_distinct_indicator() {
    let mut task = make_task(1, TaskStatus::Running);
    task.sub_status = SubStatus::StaleShell;
    task.live_shells = 1;
    let app = App::new(vec![]);
    let indicator = classify_card_indicator(&task, task.status, &app, Utc::now());
    assert!(matches!(indicator, CardIndicator::StaleShell { .. }), "got {indicator:?}");
}

#[test]
fn running_label_shows_shell_count() {
    let text = label_of(CardIndicator::Running { subagents: 0, shells: 1 });
    assert!(text.contains("running \u{00b7} 1 shell"), "got: {text:?}");
}

#[test]
fn running_label_uses_the_plural_for_multiple_shells() {
    let text = label_of(CardIndicator::Running { subagents: 0, shells: 3 });
    assert!(text.contains("running \u{00b7} 3 shells"), "got: {text:?}");
}

#[test]
fn running_label_omits_shell_suffix_at_zero() {
    let text = label_of(CardIndicator::Running { subagents: 0, shells: 0 });
    assert!(!text.contains("shell"), "zero shells must render no suffix; got: {text:?}");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib tui::ui::kanban::cards`
Expected: FAIL to compile — `CardIndicator::Running` doesn't have a `shells` field yet, `StaleShell` variant doesn't exist. This will also break five EXISTING tests that construct `CardIndicator::Running { subagents: N }` without a `shells` field (lines 592, 631, 645, 651, 658) — fixed in Step 3a below, in the same commit.

- [ ] **Step 3: Extend `CardIndicator`**

```rust
enum CardIndicator {
    // ... existing variants unchanged ...
    Stale {
        inactive_mins: Option<u64>,
    },
    StaleShell {
        inactive_hours: Option<u64>,
    },
    Blocked,
    Running {
        subagents: u32,
        shells: u32,
    },
    // ... rest unchanged ...
}
```

- [ ] **Step 3a: Fix the five existing `CardIndicator::Running` construction sites broken by the new field**

At lines 592, 631, 645, 651, and 658 (pre-Task-10 line numbers — re-`grep -n "CardIndicator::Running"` after Step 3 to get current numbers), each currently reads `CardIndicator::Running { subagents: N }`. Add `, shells: 0` to each, e.g. line 592 becomes:

```rust
            CardIndicator::Running { subagents: 0, shells: 0 },
```

and line 645 becomes:

```rust
        let text = label_of(CardIndicator::Running { subagents: 3, shells: 0 });
```

Do this for all five sites — none of them are testing shell behavior, so `shells: 0` preserves their original intent exactly.

- [ ] **Step 4: Add the `StaleShell` branch to `classify_card_indicator`, right after the plain `Stale` check**

```rust
    if task.sub_status == SubStatus::Stale {
        let inactive_mins = task.last_pre_tool_use_at.map(|ts| {
            now.signed_duration_since(ts).num_minutes().max(0).unsigned_abs()
        });
        return CardIndicator::Stale { inactive_mins };
    }
    if task.sub_status == SubStatus::StaleShell {
        let inactive_hours = task.oldest_live_shell_started_at.map(|ts| {
            now.signed_duration_since(ts).num_hours().max(0).unsigned_abs()
        });
        return CardIndicator::StaleShell { inactive_hours };
    }
    if status == TaskStatus::Running && task.sub_status == SubStatus::NeedsInput {
        return CardIndicator::Blocked;
    }
    if status == TaskStatus::Running {
        return CardIndicator::Running {
            subagents: task.live_subagents.max(0) as u32,
            shells: task.live_shells.max(0) as u32,
        };
    }
```

- [ ] **Step 5: Update `render_card_indicator`**

```rust
        CardIndicator::Stale { inactive_mins } => {
            let label = match inactive_mins {
                Some(m) => format!("\u{25c9} stale \u{00b7} {m}m"),
                None => "\u{25c9} stale".to_string(),
            };
            (label, YELLOW)
        }
        CardIndicator::StaleShell { inactive_hours } => {
            let label = match inactive_hours {
                Some(h) => format!("\u{25c9} shell stale \u{00b7} {h}h"),
                None => "\u{25c9} shell stale".to_string(),
            };
            (label, YELLOW)
        }
        CardIndicator::Blocked => ("\u{25c9} blocked".to_string(), YELLOW),
        CardIndicator::Running { subagents, shells } => {
            let icon = status_icon(TaskStatus::Running);
            let mut label = match subagents {
                0 => format!("{icon} running"),
                1 => format!("{icon} running \u{00b7} 1 agent"),
                n => format!("{icon} running \u{00b7} {n} agents"),
            };
            match shells {
                0 => {}
                1 => label.push_str(" \u{00b7} 1 shell"),
                n => label.push_str(&format!(" \u{00b7} {n} shells")),
            }
            (label, CYAN)
        }
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test --lib tui::ui::kanban::cards`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add src/tui/ui/kanban/cards.rs
git commit -m "feat(4187): render running · N shells and a distinct shell-stale card indicator"
```

---

## Task 11: Card snapshot tests

**Files:**
- Modify: `src/tui/tests/snapshots.rs`

**Interfaces:**
- Consumes: Task 10's card rendering.
- Produces: locked visual regression coverage.

- [ ] **Step 1: Write the new snapshot tests**

Following `snapshot_card_running_with_subagents` (`src/tui/tests/snapshots.rs:295-306`) exactly:

```rust
/// Locks the live-shell-count suffix on a running card's second line.
#[test]
fn snapshot_card_running_with_shells() {
    let mut t = make_task(1, TaskStatus::Running);
    t.title = "running with shells".to_string();
    t.live_shells = 2;

    let mut app = App::new(vec![t]);
    app.spinner_tick = 0;
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

/// Locks composed subagent + shell suffixes.
#[test]
fn snapshot_card_running_with_subagents_and_shells() {
    let mut t = make_task(1, TaskStatus::Running);
    t.title = "running with both".to_string();
    t.live_subagents = 1;
    t.live_shells = 1;

    let mut app = App::new(vec![t]);
    app.spinner_tick = 0;
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}

/// Locks the shell-stale card indicator.
#[test]
fn snapshot_card_stale_shell() {
    let mut t = make_task(1, TaskStatus::Running);
    t.title = "abandoned shell".to_string();
    t.sub_status = SubStatus::StaleShell;
    t.live_shells = 1;
    t.oldest_live_shell_started_at = Some(chrono::Utc::now() - chrono::Duration::hours(5));

    let mut app = App::new(vec![t]);
    app.spinner_tick = 0;
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}
```

- [ ] **Step 2: Run tests to generate `.snap.new` files**

Run: `cargo test tui::tests::snapshots::snapshot_card_running_with_shells tui::tests::snapshots::snapshot_card_running_with_subagents_and_shells tui::tests::snapshots::snapshot_card_stale_shell`
Expected: FAIL (no baseline snapshot exists yet) — this produces `.snap.new` files under `src/tui/tests/snapshots/`.

- [ ] **Step 3: Review and accept the new snapshots**

Run: `INSTA_UPDATE=always cargo test tui::tests::snapshots::snapshot_card_running_with_shells tui::tests::snapshots::snapshot_card_running_with_subagents_and_shells tui::tests::snapshots::snapshot_card_stale_shell`
Then manually inspect the accepted `.snap` files under `src/tui/tests/snapshots/` to confirm the rendered text matches what Task 10 intended (e.g. `running · 2 shells`, `running · 1 agent · 1 shell`, `shell stale · 5h`).

- [ ] **Step 4: Clean up any stray `.snap.new` files**

Run: `rm -f src/tui/tests/snapshots/*.snap.new`

- [ ] **Step 5: Run the full snapshot suite to confirm no unrelated regressions**

Run: `cargo test tui::tests::snapshots`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/tui/tests/snapshots.rs src/tui/tests/snapshots/
git commit -m "test(4187): lock running · N shells and shell-stale card snapshots"
```

---

## Final verification

- [ ] **Run the full suite**

Run: `cargo test > /tmp/full_test.txt 2>&1; echo $?`
Then: `grep -E "^(test result|failures:)" /tmp/full_test.txt`
Expected: all green, including the real-tmux integration targets if tmux is on `PATH`.

- [ ] **Run the pre-push checks**

Run: `cargo fmt --check`
Run: `cargo clippy --all-targets -- -D warnings`
Run: `./scripts/check-doc-paths.sh`
Run: `./scripts/check-doc-symbols.sh`
Expected: all clean.

- [ ] **Run `allium:weed` to check spec/code alignment before wrap-up**, since this plan touches four Allium spec files across eleven Rust-code tasks — drift between them is exactly what that skill catches.
