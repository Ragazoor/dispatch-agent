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
        db.shell_start(task.id, "bash_1", "s1", now).await.unwrap(),
        1
    );
    assert_eq!(
        db.shell_start(task.id, "bash_2", "s1", now).await.unwrap(),
        2
    );
    assert_eq!(
        db.shell_stop(task.id, "bash_1", "s1").await.unwrap().live,
        1
    );
    assert_eq!(
        db.shell_stop(task.id, "bash_2", "s1").await.unwrap().live,
        0
    );
}

#[tokio::test]
async fn duplicate_start_is_idempotent() {
    let db = in_memory_db().await;
    let task = make_task(&db, "t").await;
    let now = Utc::now();

    assert_eq!(
        db.shell_start(task.id, "bash_1", "s1", now).await.unwrap(),
        1
    );
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
        db.shell_stop(task.id, "never-started", "s1")
            .await
            .unwrap()
            .live,
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

    db.shell_start(task.id, "bash_1", "old-session", now)
        .await
        .unwrap();
    let count = db
        .shell_start(task.id, "bash_2", "new-session", now)
        .await
        .unwrap();
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
            "UPDATE tasks SET status = 'running', sub_status = 'active', stop_pending = 1 WHERE id = ?1",
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
            "UPDATE tasks SET status = 'running', sub_status = 'active', stop_pending = 1, live_subagents = 1 WHERE id = ?1",
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
