//! `task_subagents` CRUD — the storage half of the live subagent count.
//!
//! Every operation rewrites `tasks.live_subagents` from the table in the same
//! transaction, so the denormalised count and its source cannot disagree.
//! The transaction is **explicit** and load-bearing: each Claude Code hook runs
//! in its own `dispatch` process with its own connection, so without it SQLite
//! would only serialise the individual statements and two hooks could interleave
//! their fence/mutate/count/update steps into a count that disagrees with the
//! table. Session fencing (evicting rows whose `session_id` differs from the
//! incoming one) is the drift bound; there is deliberately no TTL. See
//! `docs/specs/agent-health.allium`.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

/// Recompute `tasks.live_subagents` from `task_subagents` and return it.
fn sync_count(conn: &Connection, task_id: i64) -> Result<i64> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM task_subagents WHERE task_id = ?1",
            params![task_id],
            |r| r.get(0),
        )
        .context("Failed to count task_subagents")?;
    conn.execute(
        "UPDATE tasks SET live_subagents = ?2 WHERE id = ?1",
        params![task_id, count],
    )
    .context("Failed to sync live_subagents")?;
    Ok(count)
}

/// Evict rows belonging to any session other than `session_id`. A new `claude`
/// process means a new session id, so those rows are provably dead.
fn fence_session(conn: &Connection, task_id: i64, session_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM task_subagents WHERE task_id = ?1 AND session_id != ?2",
        params![task_id, session_id],
    )
    .context("Failed to fence stale subagent session rows")?;
    Ok(())
}

pub(super) fn subagent_start(
    conn: &mut Connection,
    task_id: i64,
    agent_id: &str,
    session_id: &str,
    now: DateTime<Utc>,
) -> Result<i64> {
    let tx = conn
        .unchecked_transaction()
        .context("Failed to open subagent_start transaction")?;
    fence_session(&tx, task_id, session_id)?;
    tx.execute(
        "INSERT OR REPLACE INTO task_subagents (task_id, agent_id, session_id, started_at) \
         VALUES (?1, ?2, ?3, ?4)",
        params![task_id, agent_id, session_id, now.to_rfc3339()],
    )
    .context("Failed to insert task_subagents row")?;
    let count = sync_count(&tx, task_id)?;
    tx.commit()
        .context("Failed to commit subagent_start transaction")?;
    Ok(count)
}

pub(super) fn subagent_stop(
    conn: &mut Connection,
    task_id: i64,
    agent_id: &str,
    session_id: &str,
) -> Result<i64> {
    let tx = conn
        .unchecked_transaction()
        .context("Failed to open subagent_stop transaction")?;
    fence_session(&tx, task_id, session_id)?;
    tx.execute(
        "DELETE FROM task_subagents WHERE task_id = ?1 AND agent_id = ?2",
        params![task_id, agent_id],
    )
    .context("Failed to delete task_subagents row")?;
    let count = sync_count(&tx, task_id)?;
    tx.commit()
        .context("Failed to commit subagent_stop transaction")?;
    Ok(count)
}

pub(super) fn subagent_clear(conn: &mut Connection, task_id: i64) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .context("Failed to open subagent_clear transaction")?;
    tx.execute(
        "DELETE FROM task_subagents WHERE task_id = ?1",
        params![task_id],
    )
    .context("Failed to clear task_subagents rows")?;
    sync_count(&tx, task_id)?;
    tx.commit()
        .context("Failed to commit subagent_clear transaction")?;
    Ok(())
}
