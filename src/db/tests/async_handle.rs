//! Tests for the WP-1 async-DB foundation: the lazily-opened
//! `tokio_rusqlite::Connection` and the [`Database::db_call`] helper.
//!
//! Subsequent work packages (WP-2..WP-6) migrate individual `*Store` traits
//! onto this handle. The tests here exercise it directly so the plumbing has
//! coverage before any real impl moves.

use super::in_memory_db;
use std::sync::{Arc, Mutex};

/// `db_call` runs the closure and returns its result.
#[tokio::test]
async fn db_call_returns_closure_result() {
    let db = in_memory_db().await;
    let value = db.db_call(|_conn| Ok(42_i64)).await.unwrap();
    assert_eq!(value, 42);
}

/// The async connection shares state with the sync connection — a row
/// inserted through the sync path is visible from `db_call`. This validates
/// the shared-cache memory URI setup in [`Database::open_in_memory`].
#[tokio::test]
async fn async_connection_sees_sync_writes() {
    use crate::db::SettingsStore;

    let db = in_memory_db().await;
    db.save_repo_path("/tmp/example-repo").await.unwrap();

    let count: i64 = db
        .db_call(|conn| {
            conn.query_row("SELECT COUNT(*) FROM repo_paths", [], |row| row.get(0))
                .map_err(anyhow::Error::from)
        })
        .await
        .unwrap();
    assert_eq!(count, 1);
}

/// Errors from the closure surface as `anyhow::Error` (round-tripped through
/// `tokio_rusqlite::Error::Other`).
#[tokio::test]
async fn db_call_propagates_closure_errors() {
    let db = in_memory_db().await;
    let err = db
        .db_call(|_conn| -> anyhow::Result<()> { Err(anyhow::anyhow!("boom: 12345")) })
        .await
        .expect_err("closure error should propagate");
    assert!(
        err.to_string().contains("boom: 12345"),
        "expected boom message, got: {err}"
    );
}

/// rusqlite errors inside the closure also surface as `anyhow::Error` with the
/// SQL diagnostic preserved.
#[tokio::test]
async fn db_call_propagates_rusqlite_errors() {
    let db = in_memory_db().await;
    let err = db
        .db_call(|conn| {
            conn.execute("SELECT * FROM does_not_exist", [])?;
            Ok(())
        })
        .await
        .expect_err("rusqlite error should propagate");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("does_not_exist"),
        "expected SQL diagnostic in error, got: {msg}"
    );
}

/// Independent in-memory databases must not see each other's data — the
/// shared-cache URIs minted by [`Database::open_in_memory`] are unique per
/// instance.
#[tokio::test]
async fn distinct_in_memory_dbs_are_isolated() {
    use crate::db::SettingsStore;

    let db_a = in_memory_db().await;
    let db_b = in_memory_db().await;
    db_a.save_repo_path("/only-in-a").await.unwrap();

    let count_b: i64 = db_b
        .db_call(|conn| {
            conn.query_row("SELECT COUNT(*) FROM repo_paths", [], |row| row.get(0))
                .map_err(anyhow::Error::from)
        })
        .await
        .unwrap();
    assert_eq!(count_b, 0, "db_b must not see writes made to db_a");
}

// ---------------------------------------------------------------------------
// DbCallSlowWarning (docs/specs/observability.allium)
// ---------------------------------------------------------------------------

/// An in-memory sink for a `tracing_subscriber::fmt` writer, so tests can
/// assert on the rendered log text without a real SQLite lock-contention
/// scenario (impractical to reproduce deterministically — see the design
/// doc's Testing section).
#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Run `body` with a `fmt` subscriber installed as the default for the
/// current task, and return everything it logged as text. Relies on
/// `#[tokio::test]`'s default `current_thread` runtime so the thread-local
/// subscriber guard survives every `.await` in `body` (the task never
/// migrates to another OS thread).
async fn logged_during<F, Fut>(body: F) -> String
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let buffer = LogBuffer::default();
    let make_writer = {
        let buffer = buffer.clone();
        move || buffer.clone()
    };
    let subscriber = tracing_subscriber::fmt()
        .with_writer(make_writer)
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    body().await;
    let bytes = buffer.0.lock().unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Run a `db_call` that is guaranteed to count as slow, and return everything
/// logged during it.
///
/// The threshold (`config.slow_db_call_threshold_ms` in
/// `docs/specs/observability.allium`, 200ms in production) is pinned to zero
/// for this instance rather than sleeping past the real value: a wall-clock
/// sleep is both slow and banned in tests — see
/// `Database::set_slow_call_threshold` and the "No `tokio::time::sleep` in
/// tests" section of `docs/conventions.md`.
async fn logged_during_slow_db_call() -> String {
    logged_during(|| async {
        let mut db = in_memory_db().await;
        db.set_slow_call_threshold(std::time::Duration::ZERO);
        db.db_call(|_conn| Ok(())).await.unwrap();
    })
    .await
}

fn extract_field(log: &str, field: &str) -> Option<u64> {
    let needle = format!("{field}=");
    let start = log.find(&needle)? + needle.len();
    let rest = &log[start..];
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// A `db_call` whose closure runs past the threshold emits a single "slow
/// db_call" warning carrying the measured duration.
#[tokio::test]
async fn slow_db_call_emits_warning_above_threshold() {
    let log = logged_during_slow_db_call().await;
    assert_eq!(
        log.matches("slow db_call").count(),
        1,
        "expected exactly one slow db_call warning, got log: {log}"
    );
    // The value is whatever the closure actually took; only its presence and
    // parseability are deterministic once the threshold is pinned.
    extract_field(&log, "duration_ms").expect("duration_ms field must be present");
}

/// A `db_call` that completes under the threshold emits no warning. The
/// threshold is pinned absurdly high rather than relying on a trivial closure
/// beating the real 200ms — a loaded CI box can lose that race.
#[tokio::test]
async fn fast_db_call_emits_no_warning() {
    let log = logged_during(|| async {
        let mut db = in_memory_db().await;
        db.set_slow_call_threshold(std::time::Duration::from_secs(3600));
        db.db_call(|_conn| Ok(())).await.unwrap();
    })
    .await;

    assert!(
        !log.contains("slow db_call"),
        "fast db_call must not emit a slow db_call warning, got log: {log}"
    );
}

/// The warning's `location` field identifies the call site that invoked
/// `db_call` (via `#[track_caller]`), formatted as `file.rs:line:column`.
#[tokio::test]
async fn slow_db_call_warning_captures_call_site_location() {
    let log = logged_during_slow_db_call().await;
    assert!(
        log.contains("async_handle.rs:"),
        "expected location to identify this test's call site, got log: {log}"
    );
}
