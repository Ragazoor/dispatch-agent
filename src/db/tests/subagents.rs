#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use chrono::Utc;

#[tokio::test]
async fn start_then_stop_returns_to_zero() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    let now = Utc::now();

    assert_eq!(
        db.subagent_start(task.id, "a1", "s1", now).await.unwrap(),
        1
    );
    assert_eq!(
        db.subagent_start(task.id, "a2", "s1", now).await.unwrap(),
        2
    );
    assert_eq!(db.subagent_stop(task.id, "a1", "s1").await.unwrap().live, 1);
    assert_eq!(db.subagent_stop(task.id, "a2", "s1").await.unwrap().live, 0);
}

#[tokio::test]
async fn duplicate_start_is_idempotent() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    let now = Utc::now();

    assert_eq!(
        db.subagent_start(task.id, "a1", "s1", now).await.unwrap(),
        1
    );
    assert_eq!(
        db.subagent_start(task.id, "a1", "s1", now).await.unwrap(),
        1,
        "a replayed SubagentStart must not double-count"
    );
}

#[tokio::test]
async fn unknown_stop_is_a_noop_not_an_underflow() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;

    assert_eq!(
        db.subagent_stop(task.id, "never-started", "s1")
            .await
            .unwrap()
            .live,
        0,
        "an unrecognised agent_id must not drive the count negative"
    );
    let reread = db.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(reread.live_subagents, 0);
}

#[tokio::test]
async fn new_session_id_evicts_the_previous_sessions_entries() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    let now = Utc::now();

    db.subagent_start(task.id, "a1", "old-session", now)
        .await
        .unwrap();
    db.subagent_start(task.id, "a2", "old-session", now)
        .await
        .unwrap();

    // A start from a new session fences the stale rows: only the new one survives.
    assert_eq!(
        db.subagent_start(task.id, "a3", "new-session", now)
            .await
            .unwrap(),
        1,
        "entries from a dead session must be evicted"
    );
}

#[tokio::test]
async fn live_subagents_column_tracks_the_table() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    let now = Utc::now();

    db.subagent_start(task.id, "a1", "s1", now).await.unwrap();
    let reread = db.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(
        reread.live_subagents, 1,
        "denormalised count must match the table"
    );
}

#[tokio::test]
async fn clear_removes_everything() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    let now = Utc::now();

    db.subagent_start(task.id, "a1", "s1", now).await.unwrap();
    db.subagent_start(task.id, "a2", "s1", now).await.unwrap();
    db.subagent_clear(task.id).await.unwrap();

    let reread = db.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(reread.live_subagents, 0);
}

/// Install a trigger that aborts the `live_subagents` write — the *last* step
/// of every subagent mutation. Without an enclosing transaction the earlier
/// fence/insert/delete statements have already committed as their own implicit
/// transactions, so their effects survive the failure; with one, the whole
/// operation rolls back. That difference is what these tests assert.
async fn arm_sync_count_abort(db: &Database) {
    db.db_call(|conn| {
        conn.execute_batch(
            "CREATE TRIGGER abort_sync_count BEFORE UPDATE OF live_subagents ON tasks \
             BEGIN SELECT RAISE(ABORT, 'sync_count failed'); END;",
        )?;
        Ok(())
    })
    .await
    .unwrap();
}

async fn subagent_rows(db: &Database, task_id: i64) -> Vec<String> {
    db.db_call(move |conn| {
        let mut stmt = conn
            .prepare("SELECT agent_id FROM task_subagents WHERE task_id = ?1 ORDER BY agent_id")?;
        let rows = stmt
            .query_map([task_id], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn subagent_start_rolls_back_the_insert_when_the_count_write_fails() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    let now = Utc::now();

    arm_sync_count_abort(&db).await;

    assert!(
        db.subagent_start(task.id, "a1", "s1", now).await.is_err(),
        "the armed trigger must make the operation fail"
    );
    assert!(
        subagent_rows(&db, task.id.0).await.is_empty(),
        "a failed subagent_start must leave no row behind — insert and count \
         write are one transaction"
    );
}

#[tokio::test]
async fn subagent_start_rolls_back_the_session_fence_when_the_count_write_fails() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    let now = Utc::now();

    db.subagent_start(task.id, "old", "old-session", now)
        .await
        .unwrap();
    arm_sync_count_abort(&db).await;

    assert!(db
        .subagent_start(task.id, "new", "new-session", now)
        .await
        .is_err());
    assert_eq!(
        subagent_rows(&db, task.id.0).await,
        vec!["old".to_string()],
        "a failed subagent_start must not leave the session fence applied"
    );
}

#[tokio::test]
async fn subagent_stop_rolls_back_the_delete_when_the_count_write_fails() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    let now = Utc::now();

    db.subagent_start(task.id, "a1", "s1", now).await.unwrap();
    arm_sync_count_abort(&db).await;

    assert!(db.subagent_stop(task.id, "a1", "s1").await.is_err());
    assert_eq!(
        subagent_rows(&db, task.id.0).await,
        vec!["a1".to_string()],
        "a failed subagent_stop must not leave the row deleted — that is the \
         count-says-0-but-a-subagent-is-live state this branch exists to prevent"
    );
}

#[tokio::test]
async fn subagent_clear_rolls_back_the_delete_when_the_count_write_fails() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    let now = Utc::now();

    db.subagent_start(task.id, "a1", "s1", now).await.unwrap();
    arm_sync_count_abort(&db).await;

    assert!(db.subagent_clear(task.id).await.is_err());
    assert_eq!(subagent_rows(&db, task.id.0).await, vec!["a1".to_string()]);
}

// ---------------------------------------------------------------------------
// subagent_clear_and_void_pending_stop — the non-draining clear's single write
// ---------------------------------------------------------------------------

/// Pins exactly which columns the non-draining clear writes: entries, the count
/// and `stop_pending` — and, just as importantly, not `status`/`sub_status`.
/// Voiding a deferred Stop is not applying it; the caller owns the status.
#[tokio::test]
async fn clear_and_void_pending_stop_writes_the_count_and_the_bit_only() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    let now = Utc::now();

    db.subagent_start(task.id, "a1", "s1", now).await.unwrap();
    db.subagent_start(task.id, "a2", "s1", now).await.unwrap();
    set_running_with_pending_stop(&db, &task).await;
    let before = db.get_task(task.id).await.unwrap().unwrap();

    db.subagent_clear_and_void_pending_stop(task.id)
        .await
        .unwrap();

    assert!(subagent_rows(&db, task.id.0).await.is_empty());
    let reread = db.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(reread.live_subagents, 0);
    assert!(
        !reread.stop_pending,
        "the non-draining clear must void the deferred Stop in the same write"
    );
    assert_eq!(reread.status, TaskStatus::Running);
    assert_eq!(reread.sub_status, before.sub_status);
}

/// The guard that keeps `DetachTmux`'s drain path from drifting: `split-pane.allium`
/// clears `stop_pending` there only when the task is Running, as part of the flip.
/// The draining clear must therefore leave the bit alone on a task that is not
/// Running — replacing its conditional write with an unconditional
/// `stop_pending = 0` would silently widen that rule.
#[tokio::test]
async fn the_draining_clear_leaves_stop_pending_alone_on_a_non_running_task() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    db.patch_task(
        task.id,
        &crate::db::TaskPatch::new()
            .status(TaskStatus::Done)
            .stop_pending(true),
    )
    .await
    .unwrap();

    let drain = db.subagent_clear(task.id).await.unwrap();

    assert!(!drain.applied_pending_stop);
    let reread = db.get_task(task.id).await.unwrap().unwrap();
    assert!(
        reread.stop_pending,
        "status is part of the flip's WHERE, so a non-Running task keeps its bit"
    );
    assert_eq!(reread.status, TaskStatus::Done);
}

#[tokio::test]
async fn clear_and_void_pending_stop_rolls_back_both_writes_when_the_count_write_fails() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    let now = Utc::now();

    db.subagent_start(task.id, "a1", "s1", now).await.unwrap();
    set_running_with_pending_stop(&db, &task).await;
    arm_sync_count_abort(&db).await;

    assert!(db
        .subagent_clear_and_void_pending_stop(task.id)
        .await
        .is_err());
    assert_eq!(subagent_rows(&db, task.id.0).await, vec!["a1".to_string()]);
    assert!(
        db.get_task(task.id).await.unwrap().unwrap().stop_pending,
        "delete, stop_pending write and count write are one transaction"
    );
}

#[tokio::test]
async fn live_subagents_matches_the_table_after_interleaved_operations() {
    let db = in_memory_db().await;
    let a = make_task(&db, "a").await;
    let b = make_task(&db, "b").await;
    let now = Utc::now();

    // Interleave the two tasks, both sessions, starts and stops, including a
    // session change and an unknown stop, then assert the denormalised count
    // still equals the table for both rows.
    db.subagent_start(a.id, "a1", "s1", now).await.unwrap();
    db.subagent_start(b.id, "b1", "s1", now).await.unwrap();
    db.subagent_start(a.id, "a2", "s1", now).await.unwrap();
    db.subagent_stop(b.id, "unknown", "s1").await.unwrap();
    db.subagent_stop(a.id, "a1", "s1").await.unwrap();
    db.subagent_start(b.id, "b2", "s2", now).await.unwrap();
    db.subagent_start(a.id, "a3", "s1", now).await.unwrap();
    db.subagent_stop(b.id, "b2", "s2").await.unwrap();
    db.subagent_clear(a.id).await.unwrap();
    db.subagent_start(a.id, "a4", "s1", now).await.unwrap();

    for id in [a.id, b.id] {
        let task = db.get_task(id).await.unwrap().unwrap();
        let rows = subagent_rows(&db, id.0).await.len() as i64;
        assert_eq!(
            task.live_subagents, rows,
            "live_subagents must equal COUNT(*) for task {}",
            id.0
        );
    }
}

#[tokio::test]
async fn entries_are_scoped_per_task() {
    let db = in_memory_db().await;
    let a = make_task(&db, "a").await;
    let b = make_task(&db, "b").await;
    let now = Utc::now();

    db.subagent_start(a.id, "shared-id", "s1", now)
        .await
        .unwrap();
    assert_eq!(
        db.subagent_start(b.id, "shared-id", "s1", now)
            .await
            .unwrap(),
        1,
        "two tasks must not share a subagent row"
    );
    assert_eq!(db.get_task(a.id).await.unwrap().unwrap().live_subagents, 1);
}

// ---------------------------------------------------------------------------
// The deferred-Stop drain, applied inside the subagent transaction
// ---------------------------------------------------------------------------

/// Plants the bit through a patch, which leaves `stop_pending_at` null — the
/// shape of a row written before that column existed, since the only writer that
/// stamps it is `try_record_stop`'s defer branch. Use this only where the legacy
/// shape is the point; for a Stop deferred the way production defers one, use
/// [`task_with_a_stop_deferred_at`].
async fn set_running_with_pending_stop(db: &Database, task: &Task) {
    db.patch_task(
        task.id,
        &crate::db::TaskPatch::new()
            .status(TaskStatus::Running)
            .stop_pending(true),
    )
    .await
    .unwrap();
}

/// A Running task with one live subagent and a Stop withheld waiting on it —
/// the arrangement every drain test starts from.
async fn task_with_a_live_subagent_and_a_deferred_stop(db: &Database) -> Task {
    task_with_a_stop_deferred_at(db, Utc::now()).await
}

async fn assert_flipped_to_review(db: &Database, task: &Task) {
    let reread = db.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(reread.status, TaskStatus::Review);
    assert_eq!(
        reread.sub_status,
        SubStatus::default_for(TaskStatus::Review)
    );
    assert!(!reread.stop_pending);
    assert!(reread.last_pre_tool_use_at.is_none());
    assert!(reread.last_notification_at.is_none());
}

#[tokio::test]
async fn the_last_subagent_stop_applies_a_deferred_stop() {
    let db = in_memory_db().await;
    let task = task_with_a_live_subagent_and_a_deferred_stop(&db).await;

    let drain = db.subagent_stop(task.id, "a1", "s1").await.unwrap();
    assert_eq!(drain.live, 0);
    assert!(
        drain.applied_pending_stop,
        "draining the last subagent must apply the withheld Stop in the same write"
    );
    assert_flipped_to_review(&db, &task).await;
}

#[tokio::test]
async fn a_subagent_stop_that_does_not_drain_leaves_the_pending_stop_alone() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    let now = Utc::now();
    db.subagent_start(task.id, "a1", "s1", now).await.unwrap();
    db.subagent_start(task.id, "a2", "s1", now).await.unwrap();
    set_running_with_pending_stop(&db, &task).await;

    let drain = db.subagent_stop(task.id, "a1", "s1").await.unwrap();
    assert_eq!(drain.live, 1);
    assert!(
        !drain.applied_pending_stop,
        "one subagent is still live, so the Stop stays deferred"
    );
    let reread = db.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(reread.status, TaskStatus::Running);
    assert!(reread.stop_pending);
}

#[tokio::test]
async fn draining_without_a_pending_stop_does_not_flip() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    db.subagent_start(task.id, "a1", "s1", Utc::now())
        .await
        .unwrap();
    db.patch_task(
        task.id,
        &crate::db::TaskPatch::new().status(TaskStatus::Running),
    )
    .await
    .unwrap();

    let drain = db.subagent_stop(task.id, "a1", "s1").await.unwrap();
    assert_eq!(drain.live, 0);
    assert!(!drain.applied_pending_stop);
    assert_eq!(
        db.get_task(task.id).await.unwrap().unwrap().status,
        TaskStatus::Running,
        "no Stop was ever withheld, so there is nothing to apply"
    );
}

#[tokio::test]
async fn draining_a_task_that_left_running_does_not_flip() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    db.subagent_start(task.id, "a1", "s1", Utc::now())
        .await
        .unwrap();
    db.patch_task(
        task.id,
        &crate::db::TaskPatch::new()
            .status(TaskStatus::Done)
            .stop_pending(true),
    )
    .await
    .unwrap();

    let drain = db.subagent_stop(task.id, "a1", "s1").await.unwrap();
    assert!(!drain.applied_pending_stop);
    assert_eq!(
        db.get_task(task.id).await.unwrap().unwrap().status,
        TaskStatus::Done,
        "status is part of the WHERE — a task that left Running is not dragged back"
    );
}

#[tokio::test]
async fn a_second_drain_is_idempotent() {
    let db = in_memory_db().await;
    let task = task_with_a_live_subagent_and_a_deferred_stop(&db).await;

    assert!(
        db.subagent_stop(task.id, "a1", "s1")
            .await
            .unwrap()
            .applied_pending_stop
    );
    assert!(
        !db.subagent_stop(task.id, "a1", "s1")
            .await
            .unwrap()
            .applied_pending_stop,
        "the pending bit is consumed; a replayed SubagentStop must write nothing"
    );
}

#[tokio::test]
async fn subagent_clear_also_applies_a_deferred_stop() {
    let db = in_memory_db().await;
    let task = task_with_a_live_subagent_and_a_deferred_stop(&db).await;

    // Detach reaches the draining variant — see DetachTmux in split-pane.allium.
    assert!(
        db.subagent_clear(task.id)
            .await
            .unwrap()
            .applied_pending_stop
    );
    assert_flipped_to_review(&db, &task).await;
}

/// The property the retired tick reconciler existed to repair: the count
/// reaching zero and the withheld Stop being applied must commit together, so
/// a hook process killed partway cannot leave the task stranded in
/// `Running + stop_pending + live_subagents = 0` with no hook left to fix it.
#[tokio::test]
async fn a_failed_drain_rolls_back_the_count_and_the_flip_together() {
    let db = in_memory_db().await;
    let task = task_with_a_live_subagent_and_a_deferred_stop(&db).await;

    arm_sync_count_abort(&db).await;
    assert!(db.subagent_stop(task.id, "a1", "s1").await.is_err());

    assert_eq!(
        subagent_rows(&db, task.id.0).await,
        vec!["a1".to_string()],
        "the entry must survive, so the count still reflects a live subagent"
    );
    let reread = db.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(reread.live_subagents, 1);
    assert_eq!(reread.status, TaskStatus::Running);
    assert!(
        reread.stop_pending,
        "the deferred Stop must still be pending — the next SubagentStop drains it"
    );
}

// ---------------------------------------------------------------------------
// try_record_stop — the Stop hook's conditional write
// ---------------------------------------------------------------------------

async fn set_running(db: &Database, task: &Task) {
    db.patch_task(
        task.id,
        &crate::db::TaskPatch::new()
            .status(TaskStatus::Running)
            .last_pre_tool_use_at(Some(Utc::now()))
            .last_notification_at(Some(Utc::now())),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn record_stop_flips_immediately_when_no_subagent_is_live() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    set_running(&db, &task).await;

    assert_eq!(
        db.try_record_stop(task.id, Utc::now()).await.unwrap(),
        StopOutcome::Flipped
    );
    assert_flipped_to_review(&db, &task).await;
}

#[tokio::test]
async fn record_stop_defers_while_a_subagent_is_live() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    set_running(&db, &task).await;
    db.subagent_start(task.id, "a1", "s1", Utc::now())
        .await
        .unwrap();

    assert_eq!(
        db.try_record_stop(task.id, Utc::now()).await.unwrap(),
        StopOutcome::Deferred
    );
    let reread = db.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(
        reread.status,
        TaskStatus::Running,
        "Stop does not fire inside subagents, so a Stop with live subagents \
         means the main agent finished while they keep working"
    );
    assert!(reread.stop_pending);
    assert!(
        reread.last_pre_tool_use_at.is_some(),
        "a deferred Stop is not an activity reset — the timestamps stay put"
    );
}

#[tokio::test]
async fn try_record_stop_defers_when_a_shell_is_live_and_no_subagents_are() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
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
    assert_eq!(
        reread.status,
        TaskStatus::Running,
        "must stay Running, not flip to review"
    );
}

#[tokio::test]
async fn record_stop_is_a_noop_for_a_task_that_is_not_running() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;

    assert_eq!(
        db.try_record_stop(task.id, Utc::now()).await.unwrap(),
        StopOutcome::NoOp
    );
    let reread = db.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(reread.status, TaskStatus::Backlog);
    assert!(!reread.stop_pending);
}

/// There is deliberately no `stop_pending = 0` precondition on the flip. A Stop
/// arriving at a row that already carries the bit with nothing live must still
/// flip it — adding the precondition would make both statements miss and strand
/// the row. This also matches the pre-existing behaviour, which branched on
/// `live_subagents` alone and never consulted `stop_pending`.
#[tokio::test]
async fn record_stop_flips_a_task_that_already_carries_a_pending_stop() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    set_running_with_pending_stop(&db, &task).await;

    assert_eq!(
        db.try_record_stop(task.id, Utc::now()).await.unwrap(),
        StopOutcome::Flipped
    );
    assert_flipped_to_review(&db, &task).await;
}

#[tokio::test]
async fn record_stop_is_a_noop_for_an_unknown_task() {
    let db = in_memory_db().await;

    assert_eq!(
        db.try_record_stop(TaskId(9999), Utc::now()).await.unwrap(),
        StopOutcome::NoOp
    );
}

// ---------------------------------------------------------------------------
// record_user_prompt_submit — the UserPromptSubmit hook's conditional write
// ---------------------------------------------------------------------------

/// `stop_pending_at` is a DB-only column: nothing outside the two statements
/// that need it reads it, so it is deliberately absent from `TASK_COLUMNS` and
/// the `Task` model. Tests reach it through the writer connection.
async fn stop_pending_at(db: &Database, id: TaskId) -> Option<String> {
    db.db_call(move |conn| {
        Ok(conn.query_row(
            "SELECT stop_pending_at FROM tasks WHERE id = ?1",
            [id.0],
            |row| row.get::<_, Option<String>>(0),
        )?)
    })
    .await
    .unwrap()
}

/// A Running task with one live subagent whose Stop was deferred at `at` — the
/// arrangement the conditional clear has to reason about. Goes through the real
/// defer path so the recorded time is the one production would record.
async fn task_with_a_stop_deferred_at(db: &Database, at: chrono::DateTime<Utc>) -> Task {
    let task = make_task(db, "t").await;
    set_running(db, &task).await;
    db.subagent_start(task.id, "a1", "s1", at).await.unwrap();
    assert_eq!(
        db.try_record_stop(task.id, at).await.unwrap(),
        StopOutcome::Deferred
    );
    task
}

/// The defer branch records when the Stop *fired*, not when its write landed —
/// that is what makes `record_user_prompt_submit`'s comparison independent of
/// write order.
#[tokio::test]
async fn record_stop_stamps_the_time_the_deferred_stop_fired() {
    let db = in_memory_db().await;
    let at = Utc::now();
    let task = task_with_a_stop_deferred_at(&db, at).await;

    assert_eq!(
        stop_pending_at(&db, task.id).await.as_deref(),
        Some(crate::db::queries::format_datetime_millis(at).as_str()),
        "the deferred Stop must carry its own event time"
    );
}

/// The immediate flip has no deferred Stop to time, so it records nothing.
#[tokio::test]
async fn record_stop_records_no_defer_time_when_it_flips() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    set_running(&db, &task).await;

    assert_eq!(
        db.try_record_stop(task.id, Utc::now()).await.unwrap(),
        StopOutcome::Flipped
    );
    assert_eq!(stop_pending_at(&db, task.id).await, None);
}

/// The turn that deferred the Stop is over: the human is starting a new one, so
/// this rule owns the resulting status and the stale bit must go.
#[tokio::test]
async fn user_prompt_submit_voids_a_stop_deferred_before_the_prompt() {
    let db = in_memory_db().await;
    let deferred_at = Utc::now();
    let task = task_with_a_stop_deferred_at(&db, deferred_at).await;

    assert_eq!(
        db.record_user_prompt_submit(task.id, deferred_at + chrono::Duration::seconds(1))
            .await
            .unwrap(),
        UserPromptOutcome::Refreshed
    );
    let reread = db.get_task(task.id).await.unwrap().unwrap();
    assert!(
        !reread.stop_pending,
        "a Stop deferred by the previous turn is voided by the human resuming"
    );
    assert_eq!(reread.status, TaskStatus::Running);
}

/// The late-write race this whole conditional exists for: the prompt fired at
/// t0, the turn it started ended at t1 with subagents live, the Stop deferred at
/// t2, and only then did this write land. Voiding here would delete a Stop that
/// no drain can re-create, leaving the task Running with nothing left to move
/// it.
#[tokio::test]
async fn user_prompt_submit_preserves_a_stop_deferred_after_the_prompt() {
    let db = in_memory_db().await;
    let deferred_at = Utc::now();
    let task = task_with_a_stop_deferred_at(&db, deferred_at).await;

    assert_eq!(
        db.record_user_prompt_submit(task.id, deferred_at - chrono::Duration::seconds(1))
            .await
            .unwrap(),
        UserPromptOutcome::Refreshed
    );
    let reread = db.get_task(task.id).await.unwrap().unwrap();
    assert!(
        reread.stop_pending,
        "a Stop deferred after this prompt fired belongs to the turn the prompt \
         started — the drain still owes it"
    );
    assert_eq!(reread.status, TaskStatus::Running);

    let drain = db.subagent_stop(task.id, "a1", "s1").await.unwrap();
    assert!(
        drain.applied_pending_stop,
        "the preserved Stop must still be drainable"
    );
}

/// Ties preserve. A bit wrongly kept is resolved by the next drain or prompt; one
/// wrongly voided strands the task in Running for good.
#[tokio::test]
async fn user_prompt_submit_preserves_a_stop_deferred_at_the_same_instant() {
    let db = in_memory_db().await;
    let at = Utc::now();
    let task = task_with_a_stop_deferred_at(&db, at).await;

    db.record_user_prompt_submit(task.id, at).await.unwrap();

    assert!(db.get_task(task.id).await.unwrap().unwrap().stop_pending);
}

/// A row carrying the bit from before `stop_pending_at` existed reads as "the
/// Stop fired before any prompt", which is exactly what the unconditional clear
/// those rows were written under did.
#[tokio::test]
async fn user_prompt_submit_voids_a_pending_stop_with_no_recorded_defer_time() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    set_running_with_pending_stop(&db, &task).await;
    assert_eq!(stop_pending_at(&db, task.id).await, None);

    db.record_user_prompt_submit(task.id, Utc::now())
        .await
        .unwrap();

    assert!(!db.get_task(task.id).await.unwrap().unwrap().stop_pending);
}

#[tokio::test]
async fn user_prompt_submit_resumes_a_review_task_to_running() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    set_running(&db, &task).await;
    db.try_record_stop(task.id, Utc::now()).await.unwrap();
    assert_flipped_to_review(&db, &task).await;
    let now = Utc::now();

    assert_eq!(
        db.record_user_prompt_submit(task.id, now).await.unwrap(),
        UserPromptOutcome::Resumed,
        "Review -> Running is the transition the caller recalculates epics for"
    );
    let reread = db.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(reread.status, TaskStatus::Running);
    assert_eq!(
        reread.sub_status,
        SubStatus::default_for(TaskStatus::Running)
    );
    assert!(reread.last_pre_tool_use_at.is_some());
}

#[tokio::test]
async fn user_prompt_submit_refreshes_an_already_running_task() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    set_running(&db, &task).await;
    db.patch_task(
        task.id,
        &crate::db::TaskPatch::new().sub_status(SubStatus::NeedsInput),
    )
    .await
    .unwrap();

    assert_eq!(
        db.record_user_prompt_submit(task.id, Utc::now())
            .await
            .unwrap(),
        UserPromptOutcome::Refreshed,
        "an already-Running task is a plain activity refresh, not a resume"
    );
    let reread = db.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(reread.status, TaskStatus::Running);
    assert_eq!(
        reread.sub_status,
        SubStatus::default_for(TaskStatus::Running),
        "the resume also clears needs_input"
    );
    assert!(reread.last_pre_tool_use_at.is_some());
}

#[tokio::test]
async fn user_prompt_submit_is_a_noop_outside_running_and_review() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;

    assert_eq!(
        db.record_user_prompt_submit(task.id, Utc::now())
            .await
            .unwrap(),
        UserPromptOutcome::NoOp
    );
    let reread = db.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(reread.status, TaskStatus::Backlog);
    assert!(reread.last_pre_tool_use_at.is_none());
}

#[tokio::test]
async fn user_prompt_submit_is_a_noop_for_an_unknown_task() {
    let db = in_memory_db().await;

    assert_eq!(
        db.record_user_prompt_submit(TaskId(9999), Utc::now())
            .await
            .unwrap(),
        UserPromptOutcome::NoOp
    );
}
