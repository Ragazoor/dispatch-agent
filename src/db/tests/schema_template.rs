//! Guards for the schema-template fast path used by
//! [`Database::open_in_memory`].
//!
//! A fresh in-memory database used to replay all ~88 migrations on every
//! construction (~87 ms), which dominated the test suite. It now clones a
//! process-wide, already-migrated template via SQLite's backup API (~0.05 ms).
//! That is only sound while the cloned database is *indistinguishable* from a
//! migrated one, so each test here pins one way the two could silently drift
//! apart. See "DB access" in `docs/conventions.md`.

use super::in_memory_db;
use crate::db::{init_schema_from_template_sync, init_schema_sync};
use rusqlite::Connection;

/// Every object SQLite records for a database, in a stable order.
fn schema_objects(conn: &Connection) -> Vec<(String, String, String)> {
    let mut stmt = conn
        .prepare(
            "SELECT type, name, COALESCE(sql, '') FROM sqlite_master
             ORDER BY type, name",
        )
        .unwrap();
    let rows = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    rows
}

fn user_version(conn: &Connection) -> i64 {
    conn.pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap()
}

/// A migrated connection, built the slow/real way.
fn migrated() -> Connection {
    let c = Connection::open_in_memory().unwrap();
    init_schema_sync(&c).unwrap();
    c
}

/// A connection built from the template, the fast way.
fn from_template() -> Connection {
    let mut c = Connection::open_in_memory().unwrap();
    init_schema_from_template_sync(&mut c).unwrap();
    c
}

/// The anti-drift guard. Adding migration 89 without the template picking it
/// up fails here rather than in whichever unrelated test happens to touch the
/// new column first.
#[test]
fn template_schema_is_identical_to_a_migrated_schema() {
    let expected = schema_objects(&migrated());
    let actual = schema_objects(&from_template());

    assert_eq!(
        actual, expected,
        "template-built schema diverged from the migrated schema"
    );
    assert!(
        !expected.is_empty(),
        "sanity: the migrated schema must not be empty"
    );
}

/// `user_version` is how the migration runner decides what is still pending.
/// A clone that reported 0 would look like a brand-new database and re-run the
/// entire chain over a fully-migrated schema.
#[test]
fn template_carries_the_migrated_user_version() {
    assert_eq!(user_version(&from_template()), user_version(&migrated()));
}

/// Row-count parity across every table.
///
/// No migration currently leaves a row behind — v39 seeds a default `projects`
/// row, but v60 drops that table again — so today both sides are empty
/// everywhere. The guard is for the migration that *does* seed something: DDL
/// captured from `sqlite_master` carries no rows, so a future switch to that
/// cheaper-looking approach, or a seed added to the chain after the template is
/// built, fails here instead of surfacing as a mystery empty table.
#[test]
fn template_carries_the_same_rows_as_a_migrated_database() {
    let migrated = migrated();
    let cloned = from_template();

    let tables: Vec<String> = schema_objects(&migrated)
        .into_iter()
        .filter(|(kind, name, _)| kind == "table" && !name.starts_with("sqlite_"))
        .map(|(_, name, _)| name)
        .collect();
    assert!(!tables.is_empty(), "sanity: the schema must have tables");

    for table in tables {
        let count = |c: &Connection| -> i64 {
            c.query_row(&format!("SELECT COUNT(*) FROM \"{table}\""), [], |r| {
                r.get(0)
            })
            .unwrap()
        };
        assert_eq!(
            count(&cloned),
            count(&migrated),
            "row count for `{table}` differs between a cloned and a migrated database"
        );
    }
}

/// Connection-level PRAGMAs are settings of the *connection*, not of the
/// database pages, so the backup API does not carry them — the template path
/// has to set them itself, and its list has to stay in step with
/// `init_schema_sync`'s. Dropping one there produces no error and no failing
/// assertion anywhere else; it just quietly changes how every test database
/// behaves.
///
/// Note `foreign_keys` is *not* the sharp edge it looks like: `libsqlite3-sys`
/// builds bundled SQLite with `-DSQLITE_DEFAULT_FOREIGN_KEYS=1`, so it is on
/// either way. `synchronous`, `cache_size` and `temp_store` are the ones that
/// genuinely differ from their defaults, and this fails if any is dropped.
#[test]
fn template_connection_pragmas_match_a_migrated_connection() {
    let cloned = from_template();
    let migrated = migrated();

    for pragma in [
        "foreign_keys",
        "synchronous",
        "busy_timeout",
        "cache_size",
        "temp_store",
    ] {
        let read =
            |c: &Connection| -> i64 { c.pragma_query_value(None, pragma, |r| r.get(0)).unwrap() };
        assert_eq!(
            read(&cloned),
            read(&migrated),
            "PRAGMA {pragma} differs between a cloned and a migrated connection"
        );
    }
}

/// The read pool opens its own connections against the same memdb URI. They
/// must see the cloned schema, not an empty database — the clone has to land
/// before any reader is minted.
#[tokio::test]
async fn the_read_pool_sees_the_template_built_schema() {
    let db = in_memory_db().await;
    let tables: i64 = db
        .db_call_read(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
                [],
                |r| r.get(0),
            )
            .map_err(anyhow::Error::from)
        })
        .await
        .unwrap();
    assert!(
        tables > 5,
        "a pooled reader saw {tables} tables — the clone must land before readers are minted"
    );
}
