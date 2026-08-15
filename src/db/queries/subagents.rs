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

use crate::models::SubagentDrain;

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
) -> Result<SubagentDrain> {
    let tx = conn
        .unchecked_transaction()
        .context("Failed to open subagent_stop transaction")?;
    fence_session(&tx, task_id, session_id)?;
    tx.execute(
        "DELETE FROM task_subagents WHERE task_id = ?1 AND agent_id = ?2",
        params![task_id, agent_id],
    )
    .context("Failed to delete task_subagents row")?;
    finish_drain(tx, task_id, "subagent_stop")
}

/// Clear every entry and **void** any deferred Stop, without the drain path.
///
/// For the three callers that already own the task's resulting status — crash,
/// dispatch-claim, and `SessionStart`, where a Stop deferred by the previous
/// turn is stale by definition. See `ClearSubagentsOnSessionStart` in
/// `docs/specs/agent-health.allium`.
///
/// One transaction, and `stop_pending` is cleared in it rather than by a
/// follow-up patch: otherwise a `SubagentStart` landing between the two writes
/// could be counted against a task whose bit is about to be wiped.
pub(super) fn subagent_clear_and_void_pending_stop(
    conn: &mut Connection,
    task_id: i64,
) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .context("Failed to open subagent_clear_and_void_pending_stop transaction")?;
    tx.execute(
        "DELETE FROM task_subagents WHERE task_id = ?1",
        params![task_id],
    )
    .context("Failed to clear task_subagents rows")?;
    sync_count(&tx, task_id)?;
    tx.execute(
        "UPDATE tasks SET stop_pending = 0, updated_at = datetime('now') WHERE id = ?1",
        params![task_id],
    )
    .context("Failed to void pending stop")?;
    tx.commit()
        .context("Failed to commit subagent_clear_and_void_pending_stop transaction")?;
    Ok(())
}

/// Clear every entry and, if that drained a task carrying a deferred Stop,
/// apply it. Reached only from detach, whose rule owns no status of its own —
/// see `DetachTmux` in `docs/specs/split-pane.allium`.
///
/// Also clears `task_shells` in the same transaction: `DetachTmux` drops the
/// tmux window any backgrounded shell was running in too, so there is nothing
/// left to eventually report it stopped. Without this, a task with a live
/// shell and no subagents would flip to Review here checking only
/// `live_subagents = 0`, reproducing the bug this design fixes via detach
/// instead of via a bare Stop. See
/// docs/superpowers/specs/2026-08-15-shell-visibility-design.md.
pub(super) fn subagent_clear(conn: &mut Connection, task_id: i64) -> Result<SubagentDrain> {
    let tx = conn
        .unchecked_transaction()
        .context("Failed to open subagent_clear transaction")?;
    tx.execute(
        "DELETE FROM task_subagents WHERE task_id = ?1",
        params![task_id],
    )
    .context("Failed to clear task_subagents rows")?;
    tx.execute(
        "DELETE FROM task_shells WHERE task_id = ?1",
        params![task_id],
    )
    .context("Failed to clear task_shells rows")?;
    // Resync live_shells here, not in finish_drain: this is the only draining
    // call site that ever touches task_shells (plain subagent_stop never
    // does), so folding it into the shared tail would pay for a recount on
    // every ordinary SubagentStop for no reason.
    super::shells::sync_shell_state(&tx, task_id)?;
    finish_drain(tx, task_id, "subagent_clear")
}

/// Recount live-subagent state, apply any drained Stop, and commit — the
/// shared tail of every draining mutation.
///
/// Held in one place deliberately: "the count and the flip commit together"
/// is the invariant that makes the stranded `Running` + `stop_pending` +
/// both-counters-zero state unreachable, and stating it twice is how a later
/// edit fixes one caller and silently leaves the other racy.
fn finish_drain(tx: rusqlite::Transaction<'_>, task_id: i64, what: &str) -> Result<SubagentDrain> {
    let live = sync_count(&tx, task_id)?;
    let applied_pending_stop = super::apply_pending_stop_if_drained(&tx, task_id)?;
    tx.commit()
        .with_context(|| format!("Failed to commit {what} transaction"))?;
    Ok(SubagentDrain {
        live,
        applied_pending_stop,
    })
}
