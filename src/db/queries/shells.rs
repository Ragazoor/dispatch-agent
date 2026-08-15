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

/// Recompute `tasks.live_shells`/`tasks.oldest_live_shell_started_at` from
/// `task_shells` and return the live count.
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

/// Evict rows belonging to any session other than `session_id`. A new
/// `claude` process means a new session id, so those rows are provably dead
/// *for this session* — see the module doc comment for why this is the only
/// sweep mechanism for shells (no SessionStart-driven clear).
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
    let tx = conn
        .unchecked_transaction()
        .context("Failed to open shell_start transaction")?;
    fence_session(&tx, task_id, session_id)?;
    tx.execute(
        "INSERT OR REPLACE INTO task_shells (task_id, shell_id, session_id, started_at) \
         VALUES (?1, ?2, ?3, ?4)",
        params![
            task_id,
            shell_id,
            session_id,
            super::format_datetime_millis(now)
        ],
    )
    .context("Failed to insert task_shells row")?;
    let count = sync_shell_state(&tx, task_id)?;
    tx.commit()
        .context("Failed to commit shell_start transaction")?;
    Ok(count)
}

pub(super) fn shell_stop(
    conn: &mut Connection,
    task_id: i64,
    shell_id: &str,
    session_id: &str,
) -> Result<ShellDrain> {
    let tx = conn
        .unchecked_transaction()
        .context("Failed to open shell_stop transaction")?;
    fence_session(&tx, task_id, session_id)?;
    tx.execute(
        "DELETE FROM task_shells WHERE task_id = ?1 AND shell_id = ?2",
        params![task_id, shell_id],
    )
    .context("Failed to delete task_shells row")?;
    let live = sync_shell_state(&tx, task_id)?;
    let applied_pending_stop = super::apply_pending_stop_if_drained(&tx, task_id)?;
    tx.commit()
        .context("Failed to commit shell_stop transaction")?;
    Ok(ShellDrain {
        live,
        applied_pending_stop,
    })
}

/// Non-draining clear: deletes every `task_shells` row for the task and
/// resyncs the count, but leaves `stop_pending`/status alone. Used by
/// `DetectCrashedAgent` and `DispatchTask`'s two claim functions — see
/// `TaskService::clear_shells_no_drain`. Deliberately NOT called from
/// `SessionStart`; see the module doc comment.
pub(super) fn shell_clear_no_drain(conn: &mut Connection, task_id: i64) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .context("Failed to open shell_clear_no_drain transaction")?;
    tx.execute(
        "DELETE FROM task_shells WHERE task_id = ?1",
        params![task_id],
    )
    .context("Failed to clear task_shells rows")?;
    sync_shell_state(&tx, task_id)?;
    tx.commit()
        .context("Failed to commit shell_clear_no_drain transaction")?;
    Ok(())
}
