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

    assert_eq!(
        db.subagent_start(task.id, "a1", "s1", now).await.unwrap(),
        1
    );
    assert_eq!(
        db.subagent_start(task.id, "a2", "s1", now).await.unwrap(),
        2
    );
    assert_eq!(db.subagent_stop(task.id, "a1", "s1").await.unwrap(), 1);
    assert_eq!(db.subagent_stop(task.id, "a2", "s1").await.unwrap(), 0);
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
            .unwrap(),
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
// try_apply_pending_stop — the stranded-state reconciler's conditional write
// ---------------------------------------------------------------------------

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

#[tokio::test]
async fn apply_pending_stop_flips_a_stranded_running_task_to_review() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    set_running_with_pending_stop(&db, &task).await;

    assert!(db.try_apply_pending_stop(task.id).await.unwrap());

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
async fn apply_pending_stop_is_idempotent() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    set_running_with_pending_stop(&db, &task).await;

    assert!(db.try_apply_pending_stop(task.id).await.unwrap());
    assert!(
        !db.try_apply_pending_stop(task.id).await.unwrap(),
        "the second call must write nothing — stop_pending is already consumed"
    );
}

#[tokio::test]
async fn apply_pending_stop_does_nothing_while_a_subagent_is_live() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    set_running_with_pending_stop(&db, &task).await;
    db.subagent_start(task.id, "a1", "s1", Utc::now())
        .await
        .unwrap();

    assert!(
        !db.try_apply_pending_stop(task.id).await.unwrap(),
        "live_subagents = 0 is part of the WHERE, so a subagent that started \
         between the caller's read and this write cannot be flipped out from under"
    );
    assert_eq!(
        db.get_task(task.id).await.unwrap().unwrap().status,
        TaskStatus::Running
    );
}

#[tokio::test]
async fn apply_pending_stop_does_nothing_without_a_pending_stop() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    db.patch_task(
        task.id,
        &crate::db::TaskPatch::new().status(TaskStatus::Running),
    )
    .await
    .unwrap();

    assert!(!db.try_apply_pending_stop(task.id).await.unwrap());
}

#[tokio::test]
async fn apply_pending_stop_does_nothing_for_a_task_that_left_running() {
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

    assert!(!db.try_apply_pending_stop(task.id).await.unwrap());
    assert_eq!(
        db.get_task(task.id).await.unwrap().unwrap().status,
        TaskStatus::Done
    );
}
