//! Tests for the read-connection pool ([`Database::db_call_read`]) added to
//! remove reader/reader and reader/writer serialization on the single
//! `tokio_rusqlite` worker thread. See
//! `docs/superpowers/specs/2026-07-25-db-connection-pooling-design.md`.

use super::in_memory_db;
use crate::db::SettingsStore;
use std::sync::Arc;
use std::time::Duration;

/// `db_call_read` runs the closure and returns its result, just like `db_call`.
#[tokio::test]
async fn db_call_read_returns_closure_result() {
    let db = in_memory_db().await;
    let value = db.db_call_read(|_conn| Ok(42_i64)).await.unwrap();
    assert_eq!(value, 42);
}

/// A write committed via `db_call` (the writer connection) is visible to a
/// subsequent `db_call_read` (a pool connection) — the read-after-write
/// consistency property the design doc calls out as load-bearing.
#[tokio::test]
async fn read_pool_sees_writer_commits() {
    let db = in_memory_db().await;
    db.save_repo_path("/only-via-writer").await.unwrap();

    let count: i64 = db
        .db_call_read(|conn| {
            conn.query_row("SELECT COUNT(*) FROM repo_paths", [], |row| row.get(0))
                .map_err(anyhow::Error::from)
        })
        .await
        .unwrap();
    assert_eq!(count, 1);
}

/// The read pool's connections are minted against the same unique per-instance
/// shared-cache URI as the writer — a pool read on one `Database` must not see
/// writes made to a different `Database` instance.
#[tokio::test]
async fn read_pool_does_not_cross_instances() {
    let db_a = in_memory_db().await;
    let db_b = in_memory_db().await;
    db_a.save_repo_path("/only-in-a").await.unwrap();

    let count_b: i64 = db_b
        .db_call_read(|conn| {
            conn.query_row("SELECT COUNT(*) FROM repo_paths", [], |row| row.get(0))
                .map_err(anyhow::Error::from)
        })
        .await
        .unwrap();
    assert_eq!(
        count_b, 0,
        "db_b's read pool must not see writes made to db_a"
    );
}

/// A `db_call_read` closure that attempts a write fails loudly (SQLite
/// `SQLITE_READONLY`) rather than silently succeeding — the safety net for the
/// manual per-method read/write classification in the design doc's routing
/// table.
#[tokio::test]
async fn db_call_read_rejects_writes() {
    let db = in_memory_db().await;
    let err = db
        .db_call_read(|conn| {
            conn.execute("INSERT INTO repo_paths (path) VALUES ('nope')", [])?;
            Ok(())
        })
        .await
        .expect_err("a write through db_call_read must fail");
    let msg = format!("{err:#}");
    assert!(
        msg.to_lowercase().contains("readonly") || msg.to_lowercase().contains("read-only"),
        "expected a read-only error, got: {msg}"
    );
}

/// Concurrency regression test for the actual bug being fixed: a slow read
/// held open on one pool connection must not block a concurrent read on
/// another pool connection. Uses a file-backed `Database::open` (not
/// `open_in_memory`) because WAL's snapshot isolation — the mechanism this
/// fix relies on — only applies to file-backed databases; an in-memory
/// shared-cache database uses coarser locking and would not exercise the
/// real code path.
///
/// Synchronization is via a blocking `std::sync::mpsc` channel (safe here:
/// the closure runs on its own dedicated OS thread owned by
/// `tokio_rusqlite`, not on a tokio worker) plus a bounded `tokio::time::timeout`
/// as a deadlock guard — not a sleep-based wait. If the two reads were
/// serialized onto the same connection, the timeout fires and the test fails
/// instead of hanging.
#[tokio::test]
async fn concurrent_reads_do_not_serialize_on_file_backed_wal_db() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("pool-concurrency.db");
    let db = Arc::new(crate::db::Database::open(&path).await.unwrap());

    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();

    let blocked_db = db.clone();
    let blocked = tokio::spawn(async move {
        blocked_db
            .db_call_read(move |conn| {
                started_tx.send(()).ok();
                release_rx
                    .recv()
                    .expect("release signal must arrive before the test times out");
                conn.query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
                    .map_err(anyhow::Error::from)
            })
            .await
    });

    started_rx
        .await
        .expect("blocked read must report it started");

    let second = db.db_call_read(|conn| {
        conn.query_row("SELECT 2", [], |row| row.get::<_, i64>(0))
            .map_err(anyhow::Error::from)
    });
    let second_value: i64 = tokio::time::timeout(Duration::from_secs(5), second)
        .await
        .expect("second read must complete promptly, not queue behind the blocked read")
        .unwrap();
    assert_eq!(second_value, 2);

    release_tx.send(()).unwrap();
    let first_value = blocked.await.unwrap().unwrap();
    assert_eq!(first_value, 1);
}
