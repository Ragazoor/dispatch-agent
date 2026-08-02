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
