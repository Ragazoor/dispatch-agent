#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;

#[tokio::test]
async fn a_fresh_db_has_no_tips_state_table() {
    let db = in_memory_db().await;
    let tables: i64 = db
        .db_call(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='tips_state'",
                [],
                |r| r.get(0),
            )
            .map_err(anyhow::Error::from)
        })
        .await
        .unwrap();
    assert_eq!(
        tables, 0,
        "v84 must leave no tips_state table behind on a fresh database"
    );
}

/// v36 created `tips_state`; v84 drops it. The historical v36 entry stays in
/// `MIGRATIONS` untouched, so an existing database still creates the table on
/// its way forward and must then lose it — including when the row carries a
/// non-default watermark and show mode.
#[tokio::test]
async fn migration_84_drops_a_populated_v36_tips_state_table() {
    use rusqlite::Connection as RawConn;
    let conn = RawConn::open_in_memory().unwrap();
    // The v36 schema, built by migrate_v36_tips_state itself so the fixture
    // cannot drift from what shipped.
    crate::db::migrations::migrate_v36_tips_state(&conn).unwrap();
    conn.execute(
        "UPDATE tips_state SET seen_up_to = 9, show_mode = 'new_only' WHERE id = 1",
        [],
    )
    .unwrap();

    crate::db::migrations::migrate_v84_drop_tips_state(&conn).unwrap();

    let remaining: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='tips_state'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(remaining, 0, "v84 must drop the populated tips_state table");
}

/// The drop is unconditional DDL, so it must tolerate both a database that
/// never had the table and a second application against one that has already
/// lost it.
#[tokio::test]
async fn migration_84_is_idempotent_without_a_tips_state_table() {
    use rusqlite::Connection as RawConn;
    let conn = RawConn::open_in_memory().unwrap();
    crate::db::migrations::migrate_v84_drop_tips_state(&conn).unwrap();
    crate::db::migrations::migrate_v84_drop_tips_state(&conn).unwrap();
}

#[tokio::test]
async fn migration_81_creates_task_subagents_and_columns() {
    let db = in_memory_db().await;
    let has = db
        .db_call(|conn| {
            let table: i64 = conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='task_subagents'",
                [],
                |r| r.get(0),
            )?;
            let live: i64 = conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('tasks') WHERE name='live_subagents'",
                [],
                |r| r.get(0),
            )?;
            let pending: i64 = conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('tasks') WHERE name='stop_pending'",
                [],
                |r| r.get(0),
            )?;
            Ok((table, live, pending))
        })
        .await
        .expect("query schema");
    assert_eq!(
        has,
        (1, 1, 1),
        "migration 81 must create the table and both columns"
    );
}

#[tokio::test]
async fn migration_85_creates_task_shells_and_columns() {
    let db = in_memory_db().await;
    let has = db
        .db_call(|conn| {
            let table: i64 = conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='task_shells'",
                [],
                |r| r.get(0),
            )?;
            let live: i64 = conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('tasks') WHERE name='live_shells'",
                [],
                |r| r.get(0),
            )?;
            let oldest: i64 = conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('tasks') WHERE name='oldest_live_shell_started_at'",
                [],
                |r| r.get(0),
            )?;
            Ok((table, live, oldest))
        })
        .await
        .expect("query schema");
    assert_eq!(
        has,
        (1, 1, 1),
        "migration 85 must create task_shells and both tasks columns"
    );
}

/// v82 is the one-shot replacement for the retired tick reconciler: a database
/// written by the older read-then-write code can still hold a task stranded in
/// `Running` + `stop_pending` + `live_subagents = 0`, and with the reconciler
/// gone nothing else would ever resolve it. The migration is idempotent and
/// conditional, so re-running it here against seeded rows is a faithful test.
#[tokio::test]
async fn migration_82_resolves_a_stranded_pending_stop() {
    let db = in_memory_db().await;
    let task = create_task_returning(&db, "t", "d", "/r", None, TaskStatus::Backlog)
        .await
        .unwrap();
    db.db_call(move |conn| {
        conn.execute(
            "UPDATE tasks SET status = 'running', sub_status = 'active', \
             stop_pending = 1, live_subagents = 0, \
             last_pre_tool_use_at = '2026-01-01T00:00:00Z', \
             last_notification_at = '2026-01-01T00:00:00Z' WHERE id = ?1",
            [task.id.0],
        )?;
        crate::db::migrations::migrate_v82_resolve_stranded_pending_stops(conn)
    })
    .await
    .unwrap();

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
async fn migration_82_leaves_tasks_that_are_not_stranded_alone() {
    let db = in_memory_db().await;
    // A deferred Stop with a subagent still live is legitimately waiting.
    let waiting = create_task_returning(&db, "waiting", "d", "/r", None, TaskStatus::Backlog)
        .await
        .unwrap();
    // A plain running task with no Stop withheld.
    let running = create_task_returning(&db, "running", "d", "/r", None, TaskStatus::Backlog)
        .await
        .unwrap();

    db.db_call(move |conn| {
        conn.execute(
            "UPDATE tasks SET status = 'running', sub_status = 'active', \
             stop_pending = 1, live_subagents = 2 WHERE id = ?1",
            [waiting.id.0],
        )?;
        conn.execute(
            "UPDATE tasks SET status = 'running', sub_status = 'active', \
             stop_pending = 0, live_subagents = 0 WHERE id = ?1",
            [running.id.0],
        )?;
        crate::db::migrations::migrate_v82_resolve_stranded_pending_stops(conn)
    })
    .await
    .unwrap();

    let waiting = db.get_task(waiting.id).await.unwrap().unwrap();
    assert_eq!(
        waiting.status,
        TaskStatus::Running,
        "subagents are still live — the Stop is legitimately deferred"
    );
    assert!(waiting.stop_pending);

    let running = db.get_task(running.id).await.unwrap().unwrap();
    assert_eq!(running.status, TaskStatus::Running);
}

/// v83 adds the column `HookStop`'s defer branch stamps and
/// `record_user_prompt_submit` compares against. Additive and idempotent, with
/// no backfill: a null on a row that already carries `stop_pending` reads as
/// "the Stop fired before any prompt", which is the unconditional behaviour
/// those rows were written under.
#[tokio::test]
async fn migration_83_adds_stop_pending_at() {
    let db = in_memory_db().await;
    let column: i64 = db
        .db_call(|conn| {
            // Re-running it must be a no-op, not a duplicate-column error.
            crate::db::migrations::migrate_v83_add_stop_pending_at(conn)?;
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('tasks') WHERE name='stop_pending_at'",
                [],
                |r| r.get(0),
            )?)
        })
        .await
        .expect("query schema");
    assert_eq!(column, 1);
}

#[tokio::test]
async fn new_task_defaults_to_zero_subagents_and_no_pending_stop() {
    let db = in_memory_db().await;
    let task = create_task_returning(&db, "t", "d", "/r", None, TaskStatus::Backlog)
        .await
        .expect("create task");
    assert_eq!(task.live_subagents, 0);
    assert!(!task.stop_pending);
}

#[tokio::test]
async fn fresh_db_sets_performance_pragmas() {
    let db = in_memory_db().await;
    let (synchronous, cache_size, temp_store): (i64, i64, i64) = db
        .db_call(|conn| {
            let pq = |name| {
                conn.pragma_query_value(None, name, |r| r.get::<_, i64>(0))
                    .map_err(anyhow::Error::from)
            };
            Ok((pq("synchronous")?, pq("cache_size")?, pq("temp_store")?))
        })
        .await
        .unwrap();
    assert_eq!(synchronous, 1, "synchronous should be NORMAL (1)");
    assert_eq!(cache_size, -8000, "cache_size should be -8000 (8 MB)");
    assert_eq!(temp_store, 2, "temp_store should be MEMORY (2)");
}

#[tokio::test]
async fn fresh_db_has_latest_schema_version() {
    let db = in_memory_db().await;
    let version: i64 = db
        .db_call(|conn| {
            conn.pragma_query_value(None, "user_version", |row| row.get(0))
                .map_err(anyhow::Error::from)
        })
        .await
        .unwrap();
    assert_eq!(version, super::super::migrations::LATEST_SCHEMA_VERSION);
}

#[tokio::test]
async fn v64_backfills_url_type_from_pr_url() {
    use rusqlite::Connection as RawConn;
    let conn = RawConn::open_in_memory().unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE tasks (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             description TEXT NOT NULL DEFAULT '',
             repo_path TEXT NOT NULL,
             status TEXT NOT NULL,
             sub_status TEXT NOT NULL DEFAULT 'none',
             base_branch TEXT NOT NULL DEFAULT 'main',
             pr_url TEXT,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         INSERT INTO tasks (title, description, repo_path, status, sub_status, base_branch, pr_url)
         VALUES
           ('a','','/r','review','awaiting_review','main','https://github.com/o/r/pull/12'),
           ('b','','/r','review','awaiting_review','main','https://github.com/o/r/issues/7'),
           ('c','','/r','review','awaiting_review','main','https://example.com/x'),
           ('d','','/r','backlog','none','main',NULL);
         PRAGMA user_version = 63;",
    )
    .unwrap();

    crate::db::migrations::migrate_v64_typed_url(&conn).unwrap();

    let mut stmt = conn
        .prepare("SELECT title, url, url_type FROM tasks ORDER BY title")
        .unwrap();
    let rows: Vec<(String, Option<String>, Option<String>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();

    assert_eq!(
        rows[0],
        (
            "a".into(),
            Some("https://github.com/o/r/pull/12".into()),
            Some("pr".into())
        )
    );
    assert_eq!(
        rows[1],
        (
            "b".into(),
            Some("https://github.com/o/r/issues/7".into()),
            Some("issue".into())
        )
    );
    assert_eq!(
        rows[2],
        (
            "c".into(),
            Some("https://example.com/x".into()),
            Some("other".into())
        )
    );
    assert_eq!(rows[3], ("d".into(), None, None));

    let has_pr_url: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('tasks') WHERE name = 'pr_url'")
        .unwrap()
        .query_map([], |_| Ok(()))
        .unwrap()
        .next()
        .is_some();
    assert!(!has_pr_url, "pr_url column should be dropped");
}

#[tokio::test]
async fn v53_adds_wrap_up_mode_column() {
    let db = in_memory_db().await;
    let count: i64 = db
        .db_call(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('tasks') WHERE name = 'wrap_up_mode'",
                [],
                |r| r.get(0),
            )
            .map_err(anyhow::Error::from)
        })
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn v50_adds_hook_timestamp_columns() {
    let db = in_memory_db().await;
    let count: i64 = db
        .db_call(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('tasks') \
                 WHERE name IN ('last_pre_tool_use_at', 'last_notification_at')",
                [],
                |r| r.get(0),
            )
            .map_err(anyhow::Error::from)
        })
        .await
        .unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn v87_adds_peer_message_columns() {
    let db = in_memory_db().await;
    let count: i64 = db
        .db_call(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('tasks') \
                 WHERE name IN ('last_peer_message_sent_at', 'last_peer_message_received_at')",
                [],
                |r| r.get(0),
            )
            .map_err(anyhow::Error::from)
        })
        .await
        .unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn latest_schema_keeps_retrievals_drops_verdicts() {
    // learning_retrievals survives; learning_verdicts is dropped by migration v74
    // (verdicts are no longer persisted — rate_learning applies only the score).
    let db = in_memory_db().await;
    let (retrievals, verdicts): (i64, i64) = db
        .db_call(|conn| {
            let retrievals: i64 = conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name = 'learning_retrievals'",
                [],
                |r| r.get(0),
            )?;
            let verdicts: i64 = conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name = 'learning_verdicts'",
                [],
                |r| r.get(0),
            )?;
            Ok((retrievals, verdicts))
        })
        .await
        .unwrap();
    assert_eq!(retrievals, 1, "learning_retrievals must still exist");
    assert_eq!(verdicts, 0, "learning_verdicts must be dropped by v74");
}

#[test]
fn migrate_v74_drops_learning_verdicts_table() {
    use rusqlite::Connection as RawConn;
    let conn = RawConn::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE learning_verdicts (
             id          INTEGER PRIMARY KEY,
             task_id     INTEGER NOT NULL,
             learning_id INTEGER NOT NULL,
             verdict     TEXT NOT NULL
         );
         CREATE INDEX idx_lv_learning ON learning_verdicts(learning_id);",
    )
    .unwrap();

    crate::db::migrations::migrate_v74_drop_learning_verdicts(&conn).unwrap();

    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name = 'learning_verdicts'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(exists, 0, "v74 must drop the learning_verdicts table");

    // Idempotent: running again on a schema without the table must not error.
    crate::db::migrations::migrate_v74_drop_learning_verdicts(&conn).unwrap();
}

#[tokio::test]
async fn v48_schema_stores_arbitrary_status_text() {
    // The learnings schema has no CHECK constraint on `status`, so the historical
    // 'needs_review' string can still be written and read back at the raw TEXT
    // level. The LearningStatus enum no longer recognises it (migration v73
    // converts any surviving rows to 'approved'), but the storage layer is
    // schema-tolerant. Assert the raw column round-trips.
    let db = in_memory_db().await;
    db.db_call(|conn| {
        conn.execute(
            "INSERT INTO learnings (kind, summary, scope, status) VALUES ('pitfall','x','user','needs_review')",
            [],
        )
        .map(|_| ())
        .map_err(anyhow::Error::from)
    })
    .await
    .unwrap();
    let status: String = db
        .db_call(|conn| {
            conn.query_row(
                "SELECT status FROM learnings WHERE summary = 'x'",
                [],
                |r| r.get(0),
            )
            .map_err(anyhow::Error::from)
        })
        .await
        .unwrap();
    assert_eq!(status, "needs_review");
}

#[tokio::test]
async fn v49_renames_confirmed_columns_to_upvote() {
    let db = in_memory_db().await;
    let count: i64 = db
        .db_call(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('learnings')
                 WHERE name IN ('upvote_count','last_upvoted_at')",
                [],
                |r| r.get(0),
            )
            .map_err(anyhow::Error::from)
        })
        .await
        .unwrap();
    assert_eq!(
        count, 2,
        "expected upvote_count and last_upvoted_at columns"
    );

    let stale: i64 = db
        .db_call(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('learnings')
                 WHERE name IN ('confirmed_count','last_confirmed_at')",
                [],
                |r| r.get(0),
            )
            .map_err(anyhow::Error::from)
        })
        .await
        .unwrap();
    assert_eq!(stale, 0, "old confirmed_* columns must be removed");
}

#[test]
fn migrate_v49_preserves_existing_counts() {
    use rusqlite::Connection as RawConn;
    let conn = RawConn::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE learnings (
             id                INTEGER PRIMARY KEY,
             kind              TEXT NOT NULL,
             summary           TEXT NOT NULL,
             scope             TEXT NOT NULL,
             status            TEXT NOT NULL,
             confirmed_count   INTEGER NOT NULL DEFAULT 0,
             last_confirmed_at TEXT
         );
         INSERT INTO learnings (kind, summary, scope, status, confirmed_count, last_confirmed_at)
         VALUES ('pitfall','one','user','approved', 7, '2026-05-09T12:00:00Z');",
    )
    .unwrap();

    crate::db::migrations::migrate_v49_rename_confirmed_to_upvote(&conn).unwrap();

    let (count, ts): (i64, String) = conn
        .query_row(
            "SELECT upvote_count, last_upvoted_at FROM learnings WHERE summary='one'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(count, 7);
    assert_eq!(ts, "2026-05-09T12:00:00Z");
}

#[test]
fn migrate_v62_drops_only_unused_verdicts() {
    use rusqlite::Connection as RawConn;
    let conn = RawConn::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE learning_verdicts (
             id          INTEGER PRIMARY KEY,
             task_id     INTEGER NOT NULL,
             learning_id INTEGER NOT NULL,
             verdict     TEXT NOT NULL,
             recorded_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         INSERT INTO learning_verdicts (task_id, learning_id, verdict) VALUES
             (1, 1, 'unused'),
             (1, 2, 'helped'),
             (1, 3, 'wrong'),
             (2, 1, 'unused');",
    )
    .unwrap();

    crate::db::migrations::migrate_v62_drop_unused_verdicts(&conn).unwrap();

    let unused: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM learning_verdicts WHERE verdict = 'unused'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(unused, 0, "all 'unused' verdict rows must be deleted");

    let kept: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM learning_verdicts WHERE verdict IN ('helped','wrong')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        kept, 2,
        "'helped' and 'wrong' verdict rows must be preserved"
    );
}

#[tokio::test]
async fn migration_v42_nulls_out_epic_tag() {
    use rusqlite::Connection as RawConn;
    let conn = RawConn::open_in_memory().unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA foreign_keys=ON;
         CREATE TABLE tasks (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             description TEXT NOT NULL DEFAULT '',
             repo_path TEXT NOT NULL,
             status TEXT NOT NULL CHECK (status IN ('backlog','running','review','done','archived')),
             sub_status TEXT NOT NULL DEFAULT 'none',
             worktree TEXT,
             tmux_window TEXT,
             plan_path TEXT,
             epic_id INTEGER,
             pr_url TEXT,
             tag TEXT,
             sort_order INTEGER,
             base_branch TEXT NOT NULL DEFAULT 'main',
             created_at TEXT NOT NULL,
             updated_at TEXT NOT NULL,
             agent_pid INTEGER,
             agent_status TEXT,
             external_id TEXT,
             project_id INTEGER NOT NULL DEFAULT 1
         );
         INSERT INTO tasks (id, title, repo_path, status, sub_status, tag, base_branch, created_at, updated_at)
             VALUES (1, 'epic-tagged', '/r', 'backlog', 'none', 'epic', 'main', '2026-01-01', '2026-01-01');
         INSERT INTO tasks (id, title, repo_path, status, sub_status, tag, base_branch, created_at, updated_at)
             VALUES (2, 'feature-tagged', '/r', 'backlog', 'none', 'feature', 'main', '2026-01-01', '2026-01-01');
         INSERT INTO tasks (id, title, repo_path, status, sub_status, tag, base_branch, created_at, updated_at)
             VALUES (3, 'bug-tagged', '/r', 'backlog', 'none', 'bug', 'main', '2026-01-01', '2026-01-01');
         PRAGMA user_version = 41;",
    )
    .unwrap();

    crate::db::migrations::migrate_v42_drop_epic_tag(&conn).unwrap();

    let mut stmt = conn
        .prepare("SELECT id, tag FROM tasks ORDER BY id")
        .unwrap();
    let rows: Vec<(i64, Option<String>)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(rows[0], (1, None), "epic-tagged task should have tag NULL");
    assert_eq!(
        rows[1],
        (2, Some("feature".to_string())),
        "feature tag must be untouched"
    );
    assert_eq!(
        rows[2],
        (3, Some("bug".to_string())),
        "bug tag must be untouched"
    );
}

#[tokio::test]
async fn migration_v39_and_v60_round_trip_project_columns() {
    use rusqlite::Connection as RawConn;
    // Build a pre-v39 database manually (v38 schema)
    let conn = RawConn::open_in_memory().unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA foreign_keys=ON;
         CREATE TABLE tasks (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             description TEXT NOT NULL DEFAULT '',
             repo_path TEXT NOT NULL DEFAULT '',
             status TEXT NOT NULL DEFAULT 'backlog',
             worktree TEXT,
             tmux_window TEXT,
             plan_path TEXT,
             tag TEXT,
             epic_id INTEGER,
             sub_status TEXT NOT NULL DEFAULT 'none',
             pr_url TEXT,
             sort_order INTEGER,
             base_branch TEXT NOT NULL DEFAULT 'main',
             external_id TEXT,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE epics (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             description TEXT NOT NULL DEFAULT '',
             repo_path TEXT NOT NULL DEFAULT '',
             status TEXT NOT NULL DEFAULT 'backlog',
             plan_path TEXT,
             sort_order INTEGER,
             auto_dispatch INTEGER NOT NULL DEFAULT 0,
             parent_epic_id INTEGER,
             feed_command TEXT,
             feed_interval_secs INTEGER,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         INSERT INTO tasks (title, repo_path) VALUES ('Old task', '/repo');
         INSERT INTO epics (title, repo_path) VALUES ('Old epic', '/repo');
         PRAGMA user_version = 38;",
    )
    .unwrap();
    // Apply all migrations including v39 (adds project_id) then v60 (drops it)
    super::super::init_schema_sync(&conn).unwrap();
    // After v60, project_id should no longer exist on tasks or epics
    let tasks_has_project_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('tasks') WHERE name='project_id'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        tasks_has_project_id, 0,
        "v60 should drop project_id from tasks"
    );
    let epics_has_project_id: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('epics') WHERE name='project_id'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        epics_has_project_id, 0,
        "v60 should drop project_id from epics"
    );
    // Data is preserved
    let title: String = conn
        .query_row("SELECT title FROM tasks WHERE id=1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(title, "Old task");
}

#[tokio::test]
async fn legacy_db_migrates_to_latest_version() {
    // Simulate a pre-versioning DB: create tables manually including notes
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys=ON;
         CREATE TABLE tasks (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path TEXT NOT NULL,
             status TEXT NOT NULL DEFAULT 'backlog',
             worktree TEXT,
             tmux_window TEXT,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE notes (
             id INTEGER PRIMARY KEY,
             task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
             content TEXT NOT NULL,
             source TEXT NOT NULL DEFAULT 'user',
             created_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE repo_paths (
             id INTEGER PRIMARY KEY,
             path TEXT NOT NULL UNIQUE,
             last_used TEXT NOT NULL DEFAULT (datetime('now'))
         );",
    )
    .unwrap();

    // Insert a note so we can verify the table gets dropped
    conn.execute(
        "INSERT INTO tasks (title, description, repo_path) VALUES ('T', 'D', '/r')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO notes (task_id, content) VALUES (1, 'hello')",
        [],
    )
    .unwrap();

    // Run init_schema which should migrate
    super::super::init_schema_sync(&conn).unwrap();

    // Notes table should be gone
    let table_exists: bool = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name='notes'")
        .unwrap()
        .exists([])
        .unwrap();
    assert!(
        !table_exists,
        "notes table should be dropped after migration"
    );

    // Verify Migration 25 renamed the plan column to plan_path
    let has_plan_path: bool = conn.prepare("SELECT plan_path FROM tasks LIMIT 1").is_ok();
    assert!(
        has_plan_path,
        "Migration 25 should have renamed plan to plan_path"
    );

    // Version should be latest
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, super::super::migrations::LATEST_SCHEMA_VERSION);
}

#[tokio::test]
async fn migration_25_renames_plan_to_plan_path() {
    // Simulate a v24 DB (plan column exists, plan_path does not)
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys=OFF;
         PRAGMA user_version=24;
         CREATE TABLE tasks (
             id          INTEGER PRIMARY KEY,
             title       TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path   TEXT NOT NULL,
             status      TEXT NOT NULL DEFAULT 'backlog',
             worktree    TEXT,
             tmux_window TEXT,
             plan        TEXT,
             epic_id     INTEGER,
             sub_status  TEXT NOT NULL DEFAULT 'none',
             pr_url      TEXT,
             tag         TEXT,
             sort_order  INTEGER,
             created_at  TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE epics (
             id          INTEGER PRIMARY KEY,
             title       TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path   TEXT NOT NULL,
             status      TEXT NOT NULL DEFAULT 'backlog',
             plan        TEXT,
             sort_order  INTEGER,
             created_at  TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE repo_paths (
             id        INTEGER PRIMARY KEY,
             path      TEXT NOT NULL UNIQUE,
             last_used TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE settings (
             key   TEXT PRIMARY KEY,
             value TEXT NOT NULL
         );
         CREATE TABLE filter_presets (
             name       TEXT PRIMARY KEY,
             repo_paths TEXT NOT NULL,
             mode       TEXT NOT NULL DEFAULT 'include'
         );
         INSERT INTO tasks (title, description, repo_path, plan)
             VALUES ('T1', 'D1', '/r', 'docs/plans/task.md');
         INSERT INTO epics (title, description, repo_path, plan)
             VALUES ('E1', 'D1', '/r', 'docs/plans/epic.md');",
    )
    .unwrap();

    // Apply migration 25
    super::super::init_schema_sync(&conn).unwrap();

    // plan_path column exists with data preserved
    let task_plan_path: Option<String> = conn
        .query_row("SELECT plan_path FROM tasks WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(
        task_plan_path.as_deref(),
        Some("docs/plans/task.md"),
        "task plan_path should be preserved after migration"
    );

    let epic_plan_path: Option<String> = conn
        .query_row("SELECT plan_path FROM epics WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(
        epic_plan_path.as_deref(),
        Some("docs/plans/epic.md"),
        "epic plan_path should be preserved after migration"
    );

    // Version bumped to 25
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, super::super::migrations::LATEST_SCHEMA_VERSION);
}

#[tokio::test]
async fn migrate_v26_adds_agent_columns() {
    let db = in_memory_db().await;

    let (tw1, wt1, tw2, wt2): (Option<String>, Option<String>, Option<String>, Option<String>) = db
        .db_call(|conn| {
            conn.execute(
                "INSERT INTO review_prs (repo, number, title, author, url, is_draft,
                 created_at, updated_at, additions, deletions, review_decision,
                 labels, body, head_ref, ci_status, reviewers, tmux_window, worktree)
                 VALUES ('acme/app', 1, 'Test', 'alice', 'https://example.com', 0,
                 '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 0, 0, 'ReviewRequired',
                 '[]', '', '', 'None', '[]', 'dispatch:review-1', '/tmp/wt')",
                [],
            )?;
            let (tw1, wt1): (Option<String>, Option<String>) = conn.query_row(
                "SELECT tmux_window, worktree FROM review_prs WHERE repo = 'acme/app' AND number = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            conn.execute(
                "INSERT INTO security_alerts (repo, number, kind, severity, title,
                 url, created_at, state, description, tmux_window, worktree)
                 VALUES ('acme/app', 1, 'dependabot', 'high', 'Alert',
                 'https://example.com', '2024-01-01T00:00:00Z', 'open', 'desc',
                 'dispatch:fix-1', '/tmp/wt4')",
                [],
            )?;
            let (tw2, wt2): (Option<String>, Option<String>) = conn.query_row(
                "SELECT tmux_window, worktree FROM security_alerts WHERE repo = 'acme/app'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            Ok((tw1, wt1, tw2, wt2))
        })
        .await
        .unwrap();
    assert_eq!(tw1.as_deref(), Some("dispatch:review-1"));
    assert_eq!(wt1.as_deref(), Some("/tmp/wt"));
    assert_eq!(tw2.as_deref(), Some("dispatch:fix-1"));
    assert_eq!(wt2.as_deref(), Some("/tmp/wt4"));
}

#[tokio::test]
async fn migration_6_converts_ready_to_backlog() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys=ON;
         CREATE TABLE tasks (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path TEXT NOT NULL,
             status TEXT NOT NULL DEFAULT 'backlog',
             worktree TEXT,
             tmux_window TEXT,
             plan TEXT,
             epic_id INTEGER,
             needs_input INTEGER NOT NULL DEFAULT 0,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE repo_paths (
             id INTEGER PRIMARY KEY,
             path TEXT NOT NULL UNIQUE,
             last_used TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE epics (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path TEXT NOT NULL,
             done INTEGER NOT NULL DEFAULT 0,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE settings (
             key TEXT PRIMARY KEY,
             value TEXT NOT NULL
         );
         PRAGMA user_version = 5;",
    )
    .unwrap();

    // Insert a ready task
    conn.execute(
        "INSERT INTO tasks (title, description, repo_path, status) VALUES ('T', 'D', '/r', 'ready')",
        [],
    ).unwrap();

    super::super::init_schema_sync(&conn).unwrap();

    let status: String = conn
        .query_row("SELECT status FROM tasks WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(status, "backlog");

    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, super::super::migrations::LATEST_SCHEMA_VERSION);
}

#[tokio::test]
async fn migration_13_converts_needs_input() {
    // Simulate a database at version 12 with needs_input column
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys=ON;
         CREATE TABLE tasks (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path TEXT NOT NULL,
             status TEXT NOT NULL DEFAULT 'backlog',
             worktree TEXT,
             tmux_window TEXT,
             plan TEXT,
             epic_id INTEGER,
             needs_input INTEGER NOT NULL DEFAULT 0,
             pr_url TEXT,
             sort_order INTEGER,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE repo_paths (
             id INTEGER PRIMARY KEY,
             path TEXT NOT NULL UNIQUE,
             last_used TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE epics (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path TEXT NOT NULL,
             done INTEGER NOT NULL DEFAULT 0,
             plan TEXT,
             sort_order INTEGER,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE settings (
             key TEXT PRIMARY KEY,
             value TEXT NOT NULL
         );
         CREATE TABLE task_usage (
             task_id            INTEGER PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
             input_tokens       INTEGER NOT NULL DEFAULT 0,
             output_tokens      INTEGER NOT NULL DEFAULT 0,
             cache_read_tokens  INTEGER NOT NULL DEFAULT 0,
             cache_write_tokens INTEGER NOT NULL DEFAULT 0,
             updated_at         TEXT    NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE filter_presets (
             name       TEXT PRIMARY KEY,
             repo_paths TEXT NOT NULL
         );
         PRAGMA user_version = 12;",
    )
    .unwrap();

    // Insert tasks with various states
    conn.execute(
        "INSERT INTO tasks (title, description, repo_path, status, needs_input) VALUES ('Blocked', 'desc', '/r', 'running', 1)",
        [],
    ).unwrap();
    conn.execute(
        "INSERT INTO tasks (title, description, repo_path, status, needs_input) VALUES ('Active', 'desc', '/r', 'running', 0)",
        [],
    ).unwrap();
    conn.execute(
        "INSERT INTO tasks (title, description, repo_path, status, needs_input) VALUES ('InReview', 'desc', '/r', 'review', 0)",
        [],
    ).unwrap();

    // Run migration
    super::super::init_schema_sync(&conn).unwrap();

    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, super::super::migrations::LATEST_SCHEMA_VERSION);

    // Verify needs_input=1 became sub_status='needs_input'
    let ss: String = conn
        .query_row("SELECT sub_status FROM tasks WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(ss, "needs_input");

    // Verify running task with needs_input=0 became 'active'
    let ss: String = conn
        .query_row("SELECT sub_status FROM tasks WHERE id = 2", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(ss, "active");

    // Verify review task became 'awaiting_review'
    let ss: String = conn
        .query_row("SELECT sub_status FROM tasks WHERE id = 3", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(ss, "awaiting_review");

    // Verify needs_input column no longer exists
    let has_needs_input = conn
        .prepare("SELECT needs_input FROM tasks LIMIT 1")
        .is_ok();
    assert!(
        !has_needs_input,
        "needs_input column should be removed after migration"
    );
}

#[tokio::test]
async fn migration_16_cleans_invalid_review_needs_input() {
    // Simulate a v15 DB that has (review, needs_input) rows from old hook behavior
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys=ON;
         CREATE TABLE tasks (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path TEXT NOT NULL,
             status TEXT NOT NULL DEFAULT 'backlog',
             worktree TEXT,
             tmux_window TEXT,
             plan TEXT,
             epic_id INTEGER,
             sub_status TEXT NOT NULL DEFAULT 'none',
             pr_url TEXT,
             tag TEXT,
             sort_order INTEGER,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE repo_paths (
             id INTEGER PRIMARY KEY,
             path TEXT NOT NULL UNIQUE,
             last_used TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE epics (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path TEXT NOT NULL,
             done INTEGER NOT NULL DEFAULT 0,
             plan TEXT,
             sort_order INTEGER,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE task_usage (
             task_id INTEGER PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
             input_tokens INTEGER NOT NULL DEFAULT 0,
             output_tokens INTEGER NOT NULL DEFAULT 0,
             cache_read_tokens INTEGER NOT NULL DEFAULT 0,
             cache_write_tokens INTEGER NOT NULL DEFAULT 0,
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE filter_presets (name TEXT PRIMARY KEY, repo_paths TEXT NOT NULL);
         CREATE TABLE review_prs (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             number INTEGER NOT NULL,
             title TEXT NOT NULL,
             url TEXT NOT NULL,
             repo TEXT NOT NULL,
             author TEXT NOT NULL,
             state TEXT NOT NULL DEFAULT 'open',
             review_decision TEXT,
             created_at TEXT NOT NULL,
             updated_at TEXT NOT NULL
         );
         PRAGMA user_version = 15;",
    )
    .unwrap();

    // Insert invalid rows that migration 16 must clean up
    conn.execute(
        "INSERT INTO tasks (title, description, repo_path, status, sub_status) \
         VALUES ('ReviewBlocked', 'desc', '/r', 'review', 'needs_input')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO tasks (title, description, repo_path, status, sub_status) \
         VALUES ('ValidReview', 'desc', '/r', 'review', 'awaiting_review')",
        [],
    )
    .unwrap();

    // Run migrations
    super::super::init_schema_sync(&conn).unwrap();

    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, super::super::migrations::LATEST_SCHEMA_VERSION);

    // (review, needs_input) must be converted to (review, awaiting_review)
    let ss: String = conn
        .query_row(
            "SELECT sub_status FROM tasks WHERE title = 'ReviewBlocked'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        ss, "awaiting_review",
        "legacy (review, needs_input) must be cleaned up"
    );

    // Valid row must be unchanged
    let ss2: String = conn
        .query_row(
            "SELECT sub_status FROM tasks WHERE title = 'ValidReview'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(ss2, "awaiting_review");
}

// ---------------------------------------------------------------------------
// Migration-specific tests — verify data preservation through table rebuilds
// ---------------------------------------------------------------------------

#[tokio::test]
async fn migration_v4_preserves_epic_data_after_table_rebuild() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys=ON;
         PRAGMA user_version=3;
         CREATE TABLE tasks (
             id          INTEGER PRIMARY KEY,
             title       TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path   TEXT NOT NULL,
             status      TEXT NOT NULL DEFAULT 'backlog',
             worktree    TEXT,
             tmux_window TEXT,
             plan        TEXT,
             epic_id     INTEGER REFERENCES epics(id),
             created_at  TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE epics (
             id          INTEGER PRIMARY KEY,
             title       TEXT NOT NULL,
             description TEXT NOT NULL,
             plan        TEXT NOT NULL DEFAULT '',
             repo_path   TEXT NOT NULL,
             done        INTEGER NOT NULL DEFAULT 0,
             created_at  TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE repo_paths (
             id        INTEGER PRIMARY KEY,
             path      TEXT NOT NULL UNIQUE,
             last_used TEXT NOT NULL DEFAULT (datetime('now'))
         );
         INSERT INTO epics (title, description, plan, repo_path, done)
             VALUES ('Active Epic', 'Active desc', 'Original plan', '/repo/a', 0);
         INSERT INTO epics (title, description, plan, repo_path, done)
             VALUES ('Done Epic', 'Done desc', 'Done plan', '/repo/b', 1);
         INSERT INTO tasks (title, description, repo_path, epic_id)
             VALUES ('Task 1', 'Task desc', '/repo/a', 1);",
    )
    .unwrap();

    super::super::init_schema_sync(&conn).unwrap();

    // Epic core data preserved through v4 table rebuild
    // Note: epics.repo_path was dropped in v61, so we only check title and description
    let (title, desc): (String, String) = conn
        .query_row(
            "SELECT title, description FROM epics WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(title, "Active Epic");
    assert_eq!(desc, "Active desc");

    // v4 dropped plan; v8 re-added it (NULL); v25 renamed to plan_path
    let plan_path: Option<String> = conn
        .query_row("SELECT plan_path FROM epics WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert!(
        plan_path.is_none(),
        "plan should be NULL after v4 dropped and v8 re-added it"
    );

    // Task-epic FK preserved through rebuild
    let epic_id: Option<i64> = conn
        .query_row("SELECT epic_id FROM tasks WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(epic_id, Some(1));
}

#[tokio::test]
async fn migration_v15_converts_needs_input_to_sub_status() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys=ON;
         PRAGMA user_version=14;
         CREATE TABLE tasks (
             id          INTEGER PRIMARY KEY,
             title       TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path   TEXT NOT NULL,
             status      TEXT NOT NULL DEFAULT 'backlog',
             worktree    TEXT,
             tmux_window TEXT,
             plan        TEXT,
             epic_id     INTEGER,
             needs_input INTEGER NOT NULL DEFAULT 0,
             pr_url      TEXT,
             tag         TEXT,
             sort_order  INTEGER,
             created_at  TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE epics (
             id          INTEGER PRIMARY KEY,
             title       TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path   TEXT NOT NULL,
             done        INTEGER NOT NULL DEFAULT 0,
             plan        TEXT,
             sort_order  INTEGER,
             created_at  TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE repo_paths (
             id        INTEGER PRIMARY KEY,
             path      TEXT NOT NULL UNIQUE,
             last_used TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE settings (
             key   TEXT PRIMARY KEY,
             value TEXT NOT NULL
         );
         CREATE TABLE filter_presets (
             name       TEXT PRIMARY KEY,
             repo_paths TEXT NOT NULL
         );
         INSERT INTO tasks (title, description, repo_path, status, needs_input)
             VALUES ('Needs Input', 'desc', '/r', 'running', 1);
         INSERT INTO tasks (title, description, repo_path, status, needs_input)
             VALUES ('Running Active', 'desc', '/r', 'running', 0);
         INSERT INTO tasks (title, description, repo_path, status, needs_input)
             VALUES ('In Review', 'desc', '/r', 'review', 0);
         INSERT INTO tasks (title, description, repo_path, status, needs_input)
             VALUES ('In Backlog', 'desc', '/r', 'backlog', 0);",
    )
    .unwrap();

    super::super::init_schema_sync(&conn).unwrap();

    let rows: Vec<(String, String)> = conn
        .prepare("SELECT title, sub_status FROM tasks ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();

    assert_eq!(rows[0], ("Needs Input".into(), "needs_input".into()));
    assert_eq!(rows[1], ("Running Active".into(), "active".into()));
    assert_eq!(rows[2], ("In Review".into(), "awaiting_review".into()));
    assert_eq!(rows[3], ("In Backlog".into(), "none".into()));

    // needs_input column should be removed by v15 table rebuild
    assert!(
        conn.prepare("SELECT needs_input FROM tasks").is_err(),
        "needs_input column should be removed after migration"
    );
}

#[tokio::test]
async fn migration_v16_cleans_invalid_status_pairs() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys=ON;
         PRAGMA user_version=15;
         CREATE TABLE tasks (
             id          INTEGER PRIMARY KEY,
             title       TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path   TEXT NOT NULL,
             status      TEXT NOT NULL DEFAULT 'backlog',
             worktree    TEXT,
             tmux_window TEXT,
             plan        TEXT,
             epic_id     INTEGER,
             sub_status  TEXT NOT NULL DEFAULT 'none',
             pr_url      TEXT,
             tag         TEXT,
             sort_order  INTEGER,
             created_at  TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE epics (
             id          INTEGER PRIMARY KEY,
             title       TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path   TEXT NOT NULL,
             done        INTEGER NOT NULL DEFAULT 0,
             plan        TEXT,
             sort_order  INTEGER,
             created_at  TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE repo_paths (
             id        INTEGER PRIMARY KEY,
             path      TEXT NOT NULL UNIQUE,
             last_used TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE settings (
             key   TEXT PRIMARY KEY,
             value TEXT NOT NULL
         );
         CREATE TABLE filter_presets (
             name       TEXT PRIMARY KEY,
             repo_paths TEXT NOT NULL
         );
         -- Invalid: (review, needs_input) → should become (review, awaiting_review)
         INSERT INTO tasks (title, description, repo_path, status, sub_status)
             VALUES ('Review NI', 'desc', '/r', 'review', 'needs_input');
         -- Invalid: (running, none) → should become (running, active)
         INSERT INTO tasks (title, description, repo_path, status, sub_status)
             VALUES ('Running None', 'desc', '/r', 'running', 'none');
         -- Invalid: (backlog, active) → should become (backlog, none)
         INSERT INTO tasks (title, description, repo_path, status, sub_status)
             VALUES ('Backlog Active', 'desc', '/r', 'backlog', 'active');
         -- Valid: (running, active) → unchanged
         INSERT INTO tasks (title, description, repo_path, status, sub_status)
             VALUES ('Running OK', 'desc', '/r', 'running', 'active');",
    )
    .unwrap();

    super::super::init_schema_sync(&conn).unwrap();

    let rows: Vec<(String, String, String)> = conn
        .prepare("SELECT title, status, sub_status FROM tasks ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();

    assert_eq!(
        rows[0],
        (
            "Review NI".into(),
            "review".into(),
            "awaiting_review".into()
        )
    );
    assert_eq!(
        rows[1],
        ("Running None".into(), "running".into(), "active".into())
    );
    assert_eq!(
        rows[2],
        ("Backlog Active".into(), "backlog".into(), "none".into())
    );
    assert_eq!(
        rows[3],
        ("Running OK".into(), "running".into(), "active".into())
    );

    // CHECK constraint should reject invalid pairs after migration
    let result = conn.execute(
        "INSERT INTO tasks (title, description, repo_path, status, sub_status)
         VALUES ('x', 'x', '/x', 'backlog', 'active')",
        [],
    );
    assert!(
        result.is_err(),
        "CHECK constraint should reject (backlog, active)"
    );
}

#[tokio::test]
async fn migration_v18_expands_tilde_paths() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys=ON;
         PRAGMA user_version=17;
         CREATE TABLE tasks (
             id          INTEGER PRIMARY KEY,
             title       TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path   TEXT NOT NULL,
             status      TEXT NOT NULL DEFAULT 'backlog',
             worktree    TEXT,
             tmux_window TEXT,
             plan        TEXT,
             epic_id     INTEGER,
             sub_status  TEXT NOT NULL DEFAULT 'none',
             pr_url      TEXT,
             tag         TEXT,
             sort_order  INTEGER,
             created_at  TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
             CHECK (
                 (status = 'backlog'  AND sub_status = 'none') OR
                 (status = 'running'  AND sub_status IN ('active','needs_input','stale','crashed','conflict')) OR
                 (status = 'review'   AND sub_status IN ('awaiting_review','changes_requested','approved')) OR
                 (status = 'done'     AND sub_status = 'none') OR
                 (status = 'archived' AND sub_status = 'none')
             )
         );
         CREATE TABLE epics (
             id          INTEGER PRIMARY KEY,
             title       TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path   TEXT NOT NULL,
             done        INTEGER NOT NULL DEFAULT 0,
             plan        TEXT,
             sort_order  INTEGER,
             created_at  TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE repo_paths (
             id        INTEGER PRIMARY KEY,
             path      TEXT NOT NULL UNIQUE,
             last_used TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE settings (
             key   TEXT PRIMARY KEY,
             value TEXT NOT NULL
         );
         CREATE TABLE filter_presets (
             name       TEXT PRIMARY KEY,
             repo_paths TEXT NOT NULL
         );
         INSERT INTO tasks (title, description, repo_path)
             VALUES ('Tilde', 'desc', '~/project/a');
         INSERT INTO tasks (title, description, repo_path)
             VALUES ('Absolute', 'desc', '/absolute/path');
         INSERT INTO epics (title, description, repo_path)
             VALUES ('Epic', 'desc', '~/project/b');
         INSERT INTO repo_paths (path) VALUES ('~/project/c');
         INSERT INTO settings (key, value) VALUES ('repo_filter', '~/project/d');
         INSERT INTO filter_presets (name, repo_paths)
             VALUES ('preset', '~/project/e');",
    )
    .unwrap();

    super::super::init_schema_sync(&conn).unwrap();

    let home = std::env::var("HOME").expect("HOME must be set for this test");

    let task_path: String = conn
        .query_row("SELECT repo_path FROM tasks WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(task_path, format!("{home}/project/a"));

    // Absolute paths unchanged
    let abs_path: String = conn
        .query_row("SELECT repo_path FROM tasks WHERE id = 2", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(abs_path, "/absolute/path");

    // Note: epics.repo_path was dropped in v61, so we can't verify it here
    // The v18 migration itself (tilde expansion) ran successfully before v61 dropped the column

    let rp: String = conn
        .query_row("SELECT path FROM repo_paths", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rp, format!("{home}/project/c"));

    // After v29, repo_filter is stored as JSON array
    let setting: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'repo_filter'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let filter_paths: Vec<String> = serde_json::from_str(&setting).unwrap();
    assert_eq!(filter_paths, vec![format!("{home}/project/d")]);

    // After v29, filter_presets.repo_paths is stored as JSON array
    let preset: String = conn
        .query_row(
            "SELECT repo_paths FROM filter_presets WHERE name = 'preset'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let preset_paths: Vec<String> = serde_json::from_str(&preset).unwrap();
    assert_eq!(preset_paths, vec![format!("{home}/project/e")]);
}

#[tokio::test]
async fn migration_v20_converts_done_boolean_to_status_enum() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys=ON;
         PRAGMA user_version=19;
         CREATE TABLE tasks (
             id          INTEGER PRIMARY KEY,
             title       TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path   TEXT NOT NULL,
             status      TEXT NOT NULL DEFAULT 'backlog',
             worktree    TEXT,
             tmux_window TEXT,
             plan        TEXT,
             epic_id     INTEGER,
             sub_status  TEXT NOT NULL DEFAULT 'none',
             pr_url      TEXT,
             tag         TEXT,
             sort_order  INTEGER,
             created_at  TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
             CHECK (
                 (status = 'backlog'  AND sub_status = 'none') OR
                 (status = 'running'  AND sub_status IN ('active','needs_input','stale','crashed','conflict')) OR
                 (status = 'review'   AND sub_status IN ('awaiting_review','changes_requested','approved')) OR
                 (status = 'done'     AND sub_status = 'none') OR
                 (status = 'archived' AND sub_status = 'none')
             )
         );
         CREATE TABLE epics (
             id          INTEGER PRIMARY KEY,
             title       TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path   TEXT NOT NULL,
             done        INTEGER NOT NULL DEFAULT 0,
             plan        TEXT,
             sort_order  INTEGER,
             created_at  TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE repo_paths (
             id        INTEGER PRIMARY KEY,
             path      TEXT NOT NULL UNIQUE,
             last_used TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE settings (
             key   TEXT PRIMARY KEY,
             value TEXT NOT NULL
         );
         CREATE TABLE filter_presets (
             name       TEXT PRIMARY KEY,
             repo_paths TEXT NOT NULL
         );
         -- Epic 1: done=1 → status 'done'
         INSERT INTO epics (title, description, repo_path, done)
             VALUES ('Done Epic', 'desc', '/r', 1);
         -- Epic 2: done=0, no subtasks → status 'backlog'
         INSERT INTO epics (title, description, repo_path, done)
             VALUES ('Empty Epic', 'desc', '/r', 0);
         -- Epic 3: done=0, all subtasks done → status 'done'
         INSERT INTO epics (title, description, repo_path, done)
             VALUES ('All Done', 'desc', '/r', 0);
         INSERT INTO tasks (title, description, repo_path, status, sub_status, epic_id)
             VALUES ('T1', 'd', '/r', 'done', 'none', 3);
         INSERT INTO tasks (title, description, repo_path, status, sub_status, epic_id)
             VALUES ('T2', 'd', '/r', 'done', 'none', 3);
         -- Epic 4: done=0, has running subtask → status 'running'
         INSERT INTO epics (title, description, repo_path, done)
             VALUES ('Running Epic', 'desc', '/r', 0);
         INSERT INTO tasks (title, description, repo_path, status, sub_status, epic_id)
             VALUES ('T3', 'd', '/r', 'running', 'active', 4);
         INSERT INTO tasks (title, description, repo_path, status, sub_status, epic_id)
             VALUES ('T4', 'd', '/r', 'done', 'none', 4);
         -- Epic 5: done=0, review+done subtasks → status 'review'
         INSERT INTO epics (title, description, repo_path, done)
             VALUES ('Review Epic', 'desc', '/r', 0);
         INSERT INTO tasks (title, description, repo_path, status, sub_status, epic_id)
             VALUES ('T5', 'd', '/r', 'review', 'awaiting_review', 5);
         INSERT INTO tasks (title, description, repo_path, status, sub_status, epic_id)
             VALUES ('T6', 'd', '/r', 'done', 'none', 5);",
    )
    .unwrap();

    super::super::init_schema_sync(&conn).unwrap();

    let statuses: Vec<(String, String)> = conn
        .prepare("SELECT title, status FROM epics ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();

    assert_eq!(statuses[0], ("Done Epic".into(), "done".into()));
    assert_eq!(statuses[1], ("Empty Epic".into(), "backlog".into()));
    assert_eq!(statuses[2], ("All Done".into(), "done".into()));
    // After v20 derives 'running' and 'review', v58 resets them to 'backlog'
    assert_eq!(statuses[3], ("Running Epic".into(), "backlog".into()));
    assert_eq!(statuses[4], ("Review Epic".into(), "backlog".into()));

    // done column should be removed (replaced by status enum)
    assert!(
        conn.prepare("SELECT done FROM epics").is_err(),
        "done column should be removed after migration"
    );
}

#[tokio::test]
async fn migration_v17_adds_conflict_sub_status() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys=ON;
         PRAGMA user_version=16;
         CREATE TABLE tasks (
             id          INTEGER PRIMARY KEY,
             title       TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path   TEXT NOT NULL,
             status      TEXT NOT NULL DEFAULT 'backlog',
             worktree    TEXT,
             tmux_window TEXT,
             plan        TEXT,
             epic_id     INTEGER,
             sub_status  TEXT NOT NULL DEFAULT 'none',
             pr_url      TEXT,
             tag         TEXT,
             sort_order  INTEGER,
             created_at  TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
             CHECK (
                 (status = 'backlog'  AND sub_status = 'none') OR
                 (status = 'running'  AND sub_status IN ('active','needs_input','stale','crashed')) OR
                 (status = 'review'   AND sub_status IN ('awaiting_review','changes_requested','approved')) OR
                 (status = 'done'     AND sub_status = 'none') OR
                 (status = 'archived' AND sub_status = 'none')
             )
         );
         CREATE TABLE epics (
             id          INTEGER PRIMARY KEY,
             title       TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path   TEXT NOT NULL,
             done        INTEGER NOT NULL DEFAULT 0,
             plan        TEXT,
             sort_order  INTEGER,
             created_at  TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE repo_paths (
             id        INTEGER PRIMARY KEY,
             path      TEXT NOT NULL UNIQUE,
             last_used TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE settings (
             key   TEXT PRIMARY KEY,
             value TEXT NOT NULL
         );
         CREATE TABLE filter_presets (
             name       TEXT PRIMARY KEY,
             repo_paths TEXT NOT NULL
         );
         -- Insert tasks with valid sub_status values
         INSERT INTO tasks (title, description, repo_path, status, sub_status)
             VALUES ('Active', 'desc', '/r', 'running', 'active');
         INSERT INTO tasks (title, description, repo_path, status, sub_status)
             VALUES ('Stale', 'desc', '/r', 'running', 'stale');
         INSERT INTO tasks (title, description, repo_path, status, sub_status)
             VALUES ('In Review', 'desc', '/r', 'review', 'awaiting_review');",
    )
    .unwrap();

    // Before migration, 'conflict' should be rejected by CHECK constraint
    let result = conn.execute(
        "INSERT INTO tasks (title, description, repo_path, status, sub_status)
         VALUES ('x', 'x', '/x', 'running', 'conflict')",
        [],
    );
    assert!(
        result.is_err(),
        "pre-migration CHECK should reject 'conflict'"
    );

    super::super::init_schema_sync(&conn).unwrap();

    // Existing data preserved
    let rows: Vec<(String, String, String)> = conn
        .prepare("SELECT title, status, sub_status FROM tasks ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();

    assert_eq!(
        rows[0],
        ("Active".into(), "running".into(), "active".into())
    );
    assert_eq!(rows[1], ("Stale".into(), "running".into(), "stale".into()));
    assert_eq!(
        rows[2],
        (
            "In Review".into(),
            "review".into(),
            "awaiting_review".into()
        )
    );

    // 'conflict' now accepted after migration
    let result = conn.execute(
        "INSERT INTO tasks (title, description, repo_path, status, sub_status)
         VALUES ('Conflict', 'desc', '/r', 'running', 'conflict')",
        [],
    );
    assert!(
        result.is_ok(),
        "post-migration CHECK should accept 'conflict'"
    );
}

#[tokio::test]
async fn migration_v29_converts_newline_presets_to_json() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys=ON;
         PRAGMA user_version=28;
         CREATE TABLE tasks (
             id          INTEGER PRIMARY KEY,
             title       TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path   TEXT NOT NULL,
             status      TEXT NOT NULL DEFAULT 'backlog',
             worktree    TEXT,
             tmux_window TEXT,
             plan_path   TEXT,
             epic_id     INTEGER,
             sub_status  TEXT NOT NULL DEFAULT 'none',
             pr_url      TEXT,
             tag         TEXT,
             sort_order  INTEGER,
             created_at  TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
             CHECK (
                 (status = 'backlog'  AND sub_status = 'none') OR
                 (status = 'running'  AND sub_status IN ('active','needs_input','stale','crashed','conflict')) OR
                 (status = 'review'   AND sub_status IN ('awaiting_review','changes_requested','approved')) OR
                 (status = 'done'     AND sub_status = 'none') OR
                 (status = 'archived' AND sub_status = 'none')
             )
         );
         CREATE TABLE epics (
             id          INTEGER PRIMARY KEY,
             title       TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path   TEXT NOT NULL,
             status      TEXT NOT NULL DEFAULT 'backlog',
             plan_path   TEXT,
             sort_order  INTEGER,
             created_at  TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE repo_paths (
             id        INTEGER PRIMARY KEY,
             path      TEXT NOT NULL UNIQUE,
             last_used TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE settings (
             key   TEXT PRIMARY KEY,
             value TEXT NOT NULL
         );
         CREATE TABLE filter_presets (
             name       TEXT PRIMARY KEY,
             repo_paths TEXT NOT NULL,
             mode       TEXT NOT NULL DEFAULT 'include'
         );
         -- Newline-delimited preset
         INSERT INTO filter_presets (name, repo_paths, mode)
             VALUES ('multi', '/repo/a\n/repo/b\n/repo/c', 'include');
         -- Single-path preset (no newlines)
         INSERT INTO filter_presets (name, repo_paths, mode)
             VALUES ('single', '/repo/only', 'exclude');
         -- Newline-delimited repo_filter setting
         INSERT INTO settings (key, value) VALUES ('repo_filter', '/repo/x\n/repo/y');
         -- Non-filter setting should be unaffected
         INSERT INTO settings (key, value) VALUES ('other_key', 'some\nvalue');",
    )
    .unwrap();

    super::super::init_schema_sync(&conn).unwrap();

    // Filter presets converted to JSON
    let multi: String = conn
        .query_row(
            "SELECT repo_paths FROM filter_presets WHERE name = 'multi'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let multi_paths: Vec<String> = serde_json::from_str(&multi).unwrap();
    assert_eq!(
        multi_paths,
        vec![
            "/repo/a".to_string(),
            "/repo/b".to_string(),
            "/repo/c".to_string()
        ]
    );

    let single: String = conn
        .query_row(
            "SELECT repo_paths FROM filter_presets WHERE name = 'single'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let single_paths: Vec<String> = serde_json::from_str(&single).unwrap();
    assert_eq!(single_paths, vec!["/repo/only".to_string()]);

    // repo_filter setting converted to JSON
    let filter: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'repo_filter'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let filter_paths: Vec<String> = serde_json::from_str(&filter).unwrap();
    assert_eq!(
        filter_paths,
        vec!["/repo/x".to_string(), "/repo/y".to_string()]
    );

    // Non-filter settings unchanged
    let other: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'other_key'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(other, "some\nvalue");
}

#[tokio::test]
async fn migration_v29_skips_already_json_presets() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys=ON;
         PRAGMA user_version=28;
         CREATE TABLE tasks (
             id          INTEGER PRIMARY KEY,
             title       TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path   TEXT NOT NULL,
             status      TEXT NOT NULL DEFAULT 'backlog',
             worktree    TEXT,
             tmux_window TEXT,
             plan_path   TEXT,
             epic_id     INTEGER,
             sub_status  TEXT NOT NULL DEFAULT 'none',
             pr_url      TEXT,
             tag         TEXT,
             sort_order  INTEGER,
             created_at  TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
             CHECK (
                 (status = 'backlog'  AND sub_status = 'none') OR
                 (status = 'running'  AND sub_status IN ('active','needs_input','stale','crashed','conflict')) OR
                 (status = 'review'   AND sub_status IN ('awaiting_review','changes_requested','approved')) OR
                 (status = 'done'     AND sub_status = 'none') OR
                 (status = 'archived' AND sub_status = 'none')
             )
         );
         CREATE TABLE epics (
             id          INTEGER PRIMARY KEY,
             title       TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path   TEXT NOT NULL,
             status      TEXT NOT NULL DEFAULT 'backlog',
             plan_path   TEXT,
             sort_order  INTEGER,
             created_at  TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE repo_paths (
             id        INTEGER PRIMARY KEY,
             path      TEXT NOT NULL UNIQUE,
             last_used TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE settings (
             key   TEXT PRIMARY KEY,
             value TEXT NOT NULL
         );
         CREATE TABLE filter_presets (
             name       TEXT PRIMARY KEY,
             repo_paths TEXT NOT NULL,
             mode       TEXT NOT NULL DEFAULT 'include'
         );
         -- Already JSON — should not be double-converted
         INSERT INTO filter_presets (name, repo_paths, mode)
             VALUES ('already_json', '[\"/repo/a\",\"/repo/b\"]', 'include');
         INSERT INTO settings (key, value)
             VALUES ('repo_filter', '[\"/repo/x\"]');",
    )
    .unwrap();

    super::super::init_schema_sync(&conn).unwrap();

    let preset: String = conn
        .query_row(
            "SELECT repo_paths FROM filter_presets WHERE name = 'already_json'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let paths: Vec<String> = serde_json::from_str(&preset).unwrap();
    assert_eq!(paths, vec!["/repo/a".to_string(), "/repo/b".to_string()]);

    let filter: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'repo_filter'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let filter_paths: Vec<String> = serde_json::from_str(&filter).unwrap();
    assert_eq!(filter_paths, vec!["/repo/x".to_string()]);
}

#[tokio::test]
async fn migration_31_re_expands_tilde_paths() {
    // Simulate a v30 DB where tilde paths snuck in after the v18 migration
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys=OFF;
         PRAGMA user_version=30;
         CREATE TABLE tasks (
             id          INTEGER PRIMARY KEY,
             title       TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path   TEXT NOT NULL,
             status      TEXT NOT NULL DEFAULT 'backlog',
             worktree    TEXT,
             tmux_window TEXT,
             plan_path   TEXT,
             epic_id     INTEGER,
             sub_status  TEXT NOT NULL DEFAULT 'none',
             pr_url      TEXT,
             tag         TEXT,
             sort_order  INTEGER,
             created_at  TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
             CHECK (
                 (status = 'backlog'  AND sub_status = 'none') OR
                 (status = 'running'  AND sub_status IN ('active','needs_input','stale','crashed','conflict')) OR
                 (status = 'review'   AND sub_status IN ('awaiting_review','changes_requested','approved','conflict')) OR
                 (status = 'done'     AND sub_status = 'none') OR
                 (status = 'archived' AND sub_status = 'none')
             )
         );
         CREATE TABLE epics (
             id          INTEGER PRIMARY KEY,
             title       TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path   TEXT NOT NULL,
             status      TEXT NOT NULL DEFAULT 'backlog',
             plan_path   TEXT,
             sort_order  INTEGER,
             created_at  TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE repo_paths (
             id        INTEGER PRIMARY KEY,
             path      TEXT NOT NULL UNIQUE,
             last_used TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE settings (
             key   TEXT PRIMARY KEY,
             value TEXT NOT NULL
         );
         CREATE TABLE filter_presets (
             name       TEXT PRIMARY KEY,
             repo_paths TEXT NOT NULL,
             mode       TEXT NOT NULL DEFAULT 'include'
         );",
    )
    .unwrap();

    let home = std::env::var("HOME").unwrap();

    // Insert rows with tilde paths
    conn.execute(
        "INSERT INTO tasks (title, description, repo_path) VALUES ('T1', 'D', '~/code/project')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO tasks (title, description, repo_path) VALUES ('T2', 'D', '/absolute/path')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO epics (title, description, repo_path) VALUES ('E1', 'D', '~/code/epic')",
        [],
    )
    .unwrap();
    conn.execute("INSERT INTO repo_paths (path) VALUES ('~/code/saved')", [])
        .unwrap();
    // filter_presets are now JSON arrays (post v29)
    conn.execute(
        r#"INSERT INTO filter_presets (name, repo_paths) VALUES ('my_preset', '["~/code/a","~/code/b","/abs/c"]')"#,
        [],
    )
    .unwrap();
    conn.execute(
        r#"INSERT INTO settings (key, value) VALUES ('repo_filter', '["~/code/x"]')"#,
        [],
    )
    .unwrap();

    super::super::init_schema_sync(&conn).unwrap();

    // tasks.repo_path expanded
    let repo: String = conn
        .query_row("SELECT repo_path FROM tasks WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(repo, format!("{home}/code/project"));

    // Absolute path unchanged
    let repo2: String = conn
        .query_row("SELECT repo_path FROM tasks WHERE id = 2", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(repo2, "/absolute/path");

    // Note: epics.repo_path was dropped in v61, so we can't verify it here
    // The v31 migration itself (re-expand tilde) ran successfully before v61 dropped the column

    // repo_paths.path expanded
    let rp: String = conn
        .query_row("SELECT path FROM repo_paths WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(rp, format!("{home}/code/saved"));

    // filter_presets.repo_paths (JSON) expanded
    let preset: String = conn
        .query_row(
            "SELECT repo_paths FROM filter_presets WHERE name = 'my_preset'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let paths: Vec<String> = serde_json::from_str(&preset).unwrap();
    assert_eq!(
        paths,
        vec![
            format!("{home}/code/a"),
            format!("{home}/code/b"),
            "/abs/c".to_string(),
        ]
    );

    // settings.repo_filter (JSON) expanded
    let filter: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'repo_filter'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let filter_paths: Vec<String> = serde_json::from_str(&filter).unwrap();
    assert_eq!(filter_paths, vec![format!("{home}/code/x")]);

    // Version bumped
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, super::super::migrations::LATEST_SCHEMA_VERSION);
}

#[tokio::test]
async fn migrate_v32_adds_base_branch_column() {
    let conn = Connection::open_in_memory().unwrap();
    // Build a v31 schema (tasks table with CHECK constraint from v30, plus repo_paths).
    // Setting user_version = 31 ensures only v32 runs when init_schema is called.
    conn.execute_batch(
        "PRAGMA foreign_keys=ON;
         CREATE TABLE tasks (
             id          INTEGER PRIMARY KEY,
             title       TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path   TEXT NOT NULL,
             status      TEXT NOT NULL DEFAULT 'backlog',
             worktree    TEXT,
             tmux_window TEXT,
             plan_path   TEXT,
             epic_id     INTEGER,
             sub_status  TEXT NOT NULL DEFAULT 'none',
             pr_url      TEXT,
             tag         TEXT,
             sort_order  INTEGER,
             created_at  TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
             CHECK (
                 (status = 'backlog'  AND sub_status = 'none') OR
                 (status = 'running'  AND sub_status IN ('active','needs_input','stale','crashed','conflict')) OR
                 (status = 'review'   AND sub_status IN ('awaiting_review','changes_requested','approved','conflict')) OR
                 (status = 'done'     AND sub_status = 'none') OR
                 (status = 'archived' AND sub_status = 'none')
             )
         );
         CREATE TABLE epics (
             id          INTEGER PRIMARY KEY,
             title       TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path   TEXT NOT NULL,
             status      TEXT NOT NULL DEFAULT 'backlog',
             plan_path   TEXT,
             sort_order  INTEGER,
             created_at  TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE repo_paths (
             id        INTEGER PRIMARY KEY,
             path      TEXT NOT NULL UNIQUE,
             last_used TEXT NOT NULL DEFAULT (datetime('now'))
         );
         PRAGMA user_version = 31;",
    )
    .unwrap();

    // Insert a task using the v31 schema (no base_branch column yet)
    conn.execute(
        "INSERT INTO tasks (title, description, repo_path) VALUES ('Old Task', 'pre-migration desc', '/repo')",
        [],
    )
    .unwrap();

    // Run init_schema: only v32 should run (user_version = 31)
    super::super::init_schema_sync(&conn).unwrap();

    // Existing task should have base_branch defaulted to 'main'
    let base_branch: String = conn
        .query_row("SELECT base_branch FROM tasks WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(base_branch, "main");

    // init_schema runs all pending migrations (v32 and v33), so final version is 39
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, super::super::migrations::LATEST_SCHEMA_VERSION);
}

#[tokio::test]
async fn migration_v33_adds_auto_dispatch_to_epics() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys=ON;
         PRAGMA user_version=32;
         CREATE TABLE epics (
             id          INTEGER PRIMARY KEY,
             title       TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path   TEXT NOT NULL,
             status      TEXT NOT NULL DEFAULT 'backlog',
             plan_path   TEXT,
             sort_order  INTEGER,
             created_at  TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
         );
         INSERT INTO epics (title, description, repo_path) VALUES ('Test', 'desc', '/r');",
    )
    .unwrap();

    crate::db::migrations::migrate_v33_add_auto_dispatch(&conn).unwrap();

    let auto_dispatch: i64 = conn
        .query_row(
            "SELECT auto_dispatch FROM epics WHERE title = 'Test'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(auto_dispatch, 1);
}

#[tokio::test]
async fn v65_adds_feed_role_column_and_unique_index() {
    let db = in_memory_db().await;

    // Column present, defaults to 'none'.
    let epic = db.create_epic("E", "", None).await.unwrap();
    assert_eq!(epic.feed_role, crate::models::FeedRole::None);

    // The partial unique index exists.
    let index_count: i64 = db
        .db_call(|conn| {
            conn.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='index' AND name='idx_epics_parent_feed_role'",
                [],
                |r| r.get(0),
            )
            .map_err(anyhow::Error::from)
        })
        .await
        .unwrap();
    assert_eq!(index_count, 1, "idx_epics_parent_feed_role should exist");

    let parent = db.create_epic("P", "", None).await.unwrap();

    // Two ordinary ('none') sibling epics are allowed — the partial index
    // (WHERE feed_role <> 'none') does not constrain them.
    let a = db.create_epic("a", "", Some(parent.id)).await.unwrap();
    let b = db.create_epic("b", "", Some(parent.id)).await.unwrap();

    // Two siblings sharing the same non-'none' role are rejected.
    set_feed_role_raw(&db, a.id.0, "my-reviews").await.unwrap();
    let dup = set_feed_role_raw(&db, b.id.0, "my-reviews").await;
    assert!(dup.is_err(), "duplicate (parent, role) must be rejected");
}

/// Set `epics.feed_role` directly via SQL (bypasses the patch builder, which is
/// added in a later WP step) so the migration's unique index can be exercised.
async fn set_feed_role_raw(db: &Database, epic_id: i64, role: &str) -> anyhow::Result<()> {
    let role = role.to_string();
    db.db_call(move |conn| {
        conn.execute(
            "UPDATE epics SET feed_role = ?1 WHERE id = ?2",
            rusqlite::params![role, epic_id],
        )
        .map(|_| ())
        .map_err(anyhow::Error::from)
    })
    .await
}

#[tokio::test]
async fn migrate_v51_drops_pr_workflow_states_table() {
    let db = in_memory_db().await;

    let count: i64 = db
        .db_call(|conn| {
            conn.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='pr_workflow_states'",
                [],
                |r| r.get(0),
            )
            .map_err(anyhow::Error::from)
        })
        .await
        .unwrap();
    assert_eq!(count, 0, "pr_workflow_states should be dropped by v51");
}

#[tokio::test]
async fn migration_v38_feed_epic_columns() {
    let conn = Connection::open_in_memory().unwrap();
    // Minimal v37 schema: just the tables that v38 ALTER TABLEs
    conn.execute_batch(
        "PRAGMA foreign_keys=OFF;
         PRAGMA user_version=37;
         CREATE TABLE tasks (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             description TEXT NOT NULL DEFAULT '',
             repo_path TEXT NOT NULL,
             status TEXT NOT NULL DEFAULT 'backlog',
             sub_status TEXT NOT NULL DEFAULT 'none',
             base_branch TEXT NOT NULL DEFAULT 'main',
             sort_order INTEGER,
             updated_at TEXT NOT NULL DEFAULT (datetime('now')),
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             epic_id INTEGER
         );
         CREATE TABLE epics (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             description TEXT NOT NULL DEFAULT '',
             status TEXT NOT NULL DEFAULT 'backlog',
             sort_order INTEGER,
             updated_at TEXT NOT NULL DEFAULT (datetime('now')),
             created_at TEXT NOT NULL DEFAULT (datetime('now'))
         );",
    )
    .unwrap();

    super::super::init_schema_sync(&conn).unwrap();

    assert!(
        conn.prepare("SELECT feed_command FROM epics LIMIT 1")
            .is_ok(),
        "feed_command column should exist on epics"
    );
    assert!(
        conn.prepare("SELECT feed_interval_secs FROM epics LIMIT 1")
            .is_ok(),
        "feed_interval_secs column should exist on epics"
    );
    assert!(
        conn.prepare("SELECT external_id FROM tasks LIMIT 1")
            .is_ok(),
        "external_id column should exist on tasks"
    );
    let index_exists: bool = conn
        .prepare(
            "SELECT name FROM sqlite_master WHERE type='index' AND name='tasks_epic_external_id'",
        )
        .unwrap()
        .exists([])
        .unwrap();
    assert!(index_exists, "tasks_epic_external_id index should exist");

    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, super::super::migrations::LATEST_SCHEMA_VERSION);
}

#[tokio::test]
async fn migration_v40_creates_learnings_table() {
    use rusqlite::Connection as RawConn;
    let conn = RawConn::open_in_memory().unwrap();
    // Simulate a v39 database
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA foreign_keys=ON;
         CREATE TABLE tasks (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             description TEXT NOT NULL DEFAULT '',
             repo_path TEXT NOT NULL DEFAULT '',
             status TEXT NOT NULL DEFAULT 'backlog',
             worktree TEXT,
             tmux_window TEXT,
             plan_path TEXT,
             tag TEXT,
             epic_id INTEGER,
             sub_status TEXT NOT NULL DEFAULT 'none',
             pr_url TEXT,
             sort_order INTEGER,
             base_branch TEXT NOT NULL DEFAULT 'main',
             external_id TEXT,
             project_id INTEGER NOT NULL DEFAULT 1,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE epics (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             description TEXT NOT NULL DEFAULT '',
             repo_path TEXT NOT NULL DEFAULT '',
             status TEXT NOT NULL DEFAULT 'backlog',
             plan_path TEXT,
             sort_order INTEGER,
             auto_dispatch INTEGER NOT NULL DEFAULT 0,
             parent_epic_id INTEGER,
             feed_command TEXT,
             feed_interval_secs INTEGER,
             project_id INTEGER NOT NULL DEFAULT 1,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE projects (
             id INTEGER PRIMARY KEY,
             name TEXT NOT NULL,
             sort_order INTEGER NOT NULL DEFAULT 0,
             is_default INTEGER NOT NULL DEFAULT 0
         );
         INSERT INTO projects (name, sort_order, is_default) VALUES ('Default', 0, 1);
         PRAGMA user_version = 39;",
    )
    .unwrap();
    super::super::init_schema_sync(&conn).unwrap();
    // learnings table must exist
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='learnings'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "learnings table should exist after migration v40");
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, super::super::migrations::LATEST_SCHEMA_VERSION);
}

#[tokio::test]
async fn migration_v41_drops_cost_usd_column() {
    use rusqlite::Connection as RawConn;
    let conn = RawConn::open_in_memory().unwrap();
    // Simulate a v40 database with task_usage including cost_usd
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA foreign_keys=OFF;
         CREATE TABLE tasks (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             description TEXT NOT NULL DEFAULT '',
             repo_path TEXT NOT NULL DEFAULT '',
             status TEXT NOT NULL DEFAULT 'backlog',
             worktree TEXT,
             tmux_window TEXT,
             plan_path TEXT,
             tag TEXT,
             epic_id INTEGER,
             sub_status TEXT NOT NULL DEFAULT 'none',
             pr_url TEXT,
             sort_order INTEGER,
             base_branch TEXT NOT NULL DEFAULT 'main',
             external_id TEXT,
             project_id INTEGER NOT NULL DEFAULT 1,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE epics (
             id             INTEGER PRIMARY KEY,
             title          TEXT NOT NULL,
             description    TEXT NOT NULL DEFAULT '',
             repo_path      TEXT NOT NULL DEFAULT '',
             status         TEXT NOT NULL DEFAULT 'backlog',
             plan_path      TEXT,
             sort_order     INTEGER,
             auto_dispatch  BOOLEAN NOT NULL DEFAULT 1,
             parent_epic_id INTEGER,
             feed_command   TEXT,
             feed_interval_secs INTEGER,
             project_id     INTEGER NOT NULL DEFAULT 1,
             created_at     TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at     TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE task_usage (
             task_id INTEGER NOT NULL PRIMARY KEY REFERENCES tasks(id),
             cost_usd REAL NOT NULL DEFAULT 0.0,
             input_tokens INTEGER NOT NULL DEFAULT 0,
             output_tokens INTEGER NOT NULL DEFAULT 0,
             cache_read_tokens INTEGER NOT NULL DEFAULT 0,
             cache_write_tokens INTEGER NOT NULL DEFAULT 0,
             updated_at TEXT NOT NULL DEFAULT ''
         );
         CREATE TABLE learnings (
             id                INTEGER PRIMARY KEY,
             kind              TEXT    NOT NULL,
             summary           TEXT    NOT NULL,
             detail            TEXT,
             scope             TEXT    NOT NULL,
             scope_ref         TEXT,
             tags              TEXT    NOT NULL DEFAULT '[]',
             status            TEXT    NOT NULL DEFAULT 'approved',
             source_task_id    INTEGER REFERENCES tasks(id),
             confirmed_count   INTEGER NOT NULL DEFAULT 0,
             last_confirmed_at TEXT,
             created_at        TEXT    NOT NULL DEFAULT (datetime('now')),
             updated_at        TEXT    NOT NULL DEFAULT (datetime('now')),
             CHECK (
                 (scope = 'user' AND scope_ref IS NULL)
                 OR (scope != 'user' AND scope_ref IS NOT NULL)
             )
         );
         CREATE INDEX IF NOT EXISTS idx_learnings_scope ON learnings(scope, scope_ref);
         CREATE INDEX IF NOT EXISTS idx_learnings_status ON learnings(status);
         INSERT INTO tasks (id, title, status) VALUES (999, 'test', 'backlog');
         INSERT INTO task_usage (task_id, cost_usd, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, updated_at)
             VALUES (999, 0.42, 100, 50, 10, 5, '');
         PRAGMA user_version = 40;",
    )
    .unwrap();
    super::super::init_schema_sync(&conn).unwrap();
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(version, super::super::migrations::LATEST_SCHEMA_VERSION);
    // task_usage dropped entirely by v56
    let table_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='task_usage'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(table_count, 0, "task_usage should be dropped by v56");
}

#[tokio::test]
async fn test_migrate_v43_proposed_to_approved() {
    use rusqlite::Connection as RawConn;
    let conn = RawConn::open_in_memory().unwrap();
    // Build a v42 database with the learnings table using DEFAULT 'proposed'
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA foreign_keys=ON;
         CREATE TABLE tasks (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             description TEXT NOT NULL DEFAULT '',
             repo_path TEXT NOT NULL DEFAULT '',
             status TEXT NOT NULL DEFAULT 'backlog',
             worktree TEXT,
             tmux_window TEXT,
             plan_path TEXT,
             tag TEXT,
             epic_id INTEGER,
             sub_status TEXT NOT NULL DEFAULT 'none',
             pr_url TEXT,
             sort_order INTEGER,
             base_branch TEXT NOT NULL DEFAULT 'main',
             external_id TEXT,
             project_id INTEGER NOT NULL DEFAULT 1,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE epics (
             id             INTEGER PRIMARY KEY,
             title          TEXT NOT NULL,
             description    TEXT NOT NULL DEFAULT '',
             repo_path      TEXT NOT NULL DEFAULT '',
             status         TEXT NOT NULL DEFAULT 'backlog',
             plan_path      TEXT,
             sort_order     INTEGER,
             auto_dispatch  BOOLEAN NOT NULL DEFAULT 1,
             parent_epic_id INTEGER,
             feed_command   TEXT,
             feed_interval_secs INTEGER,
             project_id     INTEGER NOT NULL DEFAULT 1,
             created_at     TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at     TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE learnings (
             id                INTEGER PRIMARY KEY,
             kind              TEXT    NOT NULL,
             summary           TEXT    NOT NULL,
             detail            TEXT,
             scope             TEXT    NOT NULL,
             scope_ref         TEXT,
             tags              TEXT    NOT NULL DEFAULT '[]',
             status            TEXT    NOT NULL DEFAULT 'proposed',
             source_task_id    INTEGER REFERENCES tasks(id),
             confirmed_count   INTEGER NOT NULL DEFAULT 0,
             last_confirmed_at TEXT,
             created_at        TEXT    NOT NULL DEFAULT (datetime('now')),
             updated_at        TEXT    NOT NULL DEFAULT (datetime('now')),
             CHECK (
                 (scope = 'user' AND scope_ref IS NULL)
                 OR (scope != 'user' AND scope_ref IS NOT NULL)
             )
         );
         CREATE INDEX IF NOT EXISTS idx_learnings_scope ON learnings(scope, scope_ref);
         CREATE INDEX IF NOT EXISTS idx_learnings_status ON learnings(status);
         INSERT INTO learnings (kind, summary, scope, status) VALUES ('pitfall', 'test', 'user', 'proposed');
         PRAGMA user_version = 42;",
    )
    .unwrap();

    // Apply v43 via init_schema
    super::super::init_schema_sync(&conn).unwrap();

    // Assert: the previously proposed row is now approved
    let status: String = conn
        .query_row(
            "SELECT status FROM learnings WHERE summary = 'test'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "approved");

    // Assert: new inserts default to approved (not proposed)
    conn.execute(
        "INSERT INTO learnings (kind, summary, scope) VALUES ('pitfall', 'new', 'user')",
        [],
    )
    .unwrap();
    let new_status: String = conn
        .query_row(
            "SELECT status FROM learnings WHERE summary = 'new'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(new_status, "approved");

    // Assert: schema version bumped to 44
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(version, super::super::migrations::LATEST_SCHEMA_VERSION);
}

#[tokio::test]
async fn migration_v44_converts_episodic_to_convention() {
    use rusqlite::Connection as RawConn;
    let conn = RawConn::open_in_memory().unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys=ON;
         CREATE TABLE learnings (
             id                INTEGER PRIMARY KEY,
             kind              TEXT    NOT NULL,
             summary           TEXT    NOT NULL,
             detail            TEXT,
             scope             TEXT    NOT NULL,
             scope_ref         TEXT,
             tags              TEXT    NOT NULL DEFAULT '[]',
             status            TEXT    NOT NULL DEFAULT 'approved',
             source_task_id    INTEGER,
             confirmed_count   INTEGER NOT NULL DEFAULT 0,
             last_confirmed_at TEXT,
             created_at        TEXT    NOT NULL DEFAULT (datetime('now')),
             updated_at        TEXT    NOT NULL DEFAULT (datetime('now'))
         );
         INSERT INTO learnings (kind, summary, scope, status)
             VALUES ('episodic', 'how I solved task 42', 'user', 'approved');
         INSERT INTO learnings (kind, summary, scope, status)
             VALUES ('convention', 'use Arc for shared state', 'user', 'approved');
         PRAGMA user_version = 43;",
    )
    .unwrap();

    crate::db::migrations::migrate_v44_episodic_to_convention(&conn).unwrap();

    let mut stmt = conn
        .prepare("SELECT kind FROM learnings ORDER BY id")
        .unwrap();
    let kinds: Vec<String> = stmt
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(
        kinds[0], "convention",
        "episodic entry should be converted to convention"
    );
    assert_eq!(
        kinds[1], "convention",
        "non-episodic entry should be unchanged"
    );
}

#[tokio::test]
async fn migration_v45_adds_labels_column_with_default() {
    use rusqlite::Connection as RawConn;
    let conn = RawConn::open_in_memory().unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys=OFF;
         CREATE TABLE tasks (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             description TEXT NOT NULL DEFAULT '',
             repo_path TEXT NOT NULL,
             status TEXT NOT NULL DEFAULT 'backlog'
         );
         INSERT INTO tasks (title, repo_path, status)
             VALUES ('legacy task', '/repo', 'backlog');
         PRAGMA user_version = 44;",
    )
    .unwrap();

    // Pre-migration: no labels column
    assert!(
        conn.prepare("SELECT labels FROM tasks LIMIT 1").is_err(),
        "labels column should not exist before migration v45"
    );

    crate::db::migrations::migrate_v45_add_task_labels(&conn).unwrap();

    let labels: String = conn
        .query_row("SELECT labels FROM tasks WHERE id = 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(labels, "[]");

    conn.execute(
        "INSERT INTO tasks (title, repo_path, status) VALUES ('new task', '/repo', 'backlog')",
        [],
    )
    .unwrap();
    let new_labels: String = conn
        .query_row(
            "SELECT labels FROM tasks WHERE title = 'new task'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(new_labels, "[]");
}

#[tokio::test]
async fn migration_v45_is_idempotent() {
    use rusqlite::Connection as RawConn;
    let conn = RawConn::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE tasks (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             description TEXT NOT NULL DEFAULT '',
             repo_path TEXT NOT NULL,
             status TEXT NOT NULL DEFAULT 'backlog',
             labels TEXT NOT NULL DEFAULT '[]'
         );",
    )
    .unwrap();

    // Running the migration on a DB that already has the column must be a no-op.
    crate::db::migrations::migrate_v45_add_task_labels(&conn).unwrap();
}

#[tokio::test]
async fn migration_v52_is_idempotent() {
    use rusqlite::Connection as RawConn;
    let conn = RawConn::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE repo_paths (
             id        INTEGER PRIMARY KEY,
             path      TEXT NOT NULL UNIQUE,
             last_used TEXT NOT NULL DEFAULT (datetime('now')),
             verify_command TEXT
         );",
    )
    .unwrap();

    // Running the migration on a DB that already has the column must be a no-op.
    crate::db::migrations::migrate_v52_add_verify_command_to_repo_paths(&conn).unwrap();
}

#[tokio::test]
async fn migration_v52_adds_verify_command_to_repo_paths() {
    let temp = tempfile::NamedTempFile::new().unwrap();
    {
        let conn = rusqlite::Connection::open(temp.path()).unwrap();
        conn.execute_batch(
            "CREATE TABLE tasks (
                id INTEGER PRIMARY KEY,
                title TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                repo_path TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'backlog',
                sub_status TEXT NOT NULL DEFAULT 'none',
                base_branch TEXT NOT NULL DEFAULT 'main',
                sort_order INTEGER,
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE epics (
                id INTEGER PRIMARY KEY,
                title TEXT NOT NULL DEFAULT '',
                status TEXT NOT NULL DEFAULT 'backlog',
                sort_order INTEGER,
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE repo_paths (
                id        INTEGER PRIMARY KEY,
                path      TEXT NOT NULL UNIQUE,
                last_used TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 51).unwrap();

        // assert column absent before migration
        let cols_before: Vec<String> = conn
            .prepare("SELECT name FROM pragma_table_info('repo_paths')")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(
            !cols_before.contains(&"verify_command".to_string()),
            "column must not exist before migration"
        );
    }

    let db = crate::db::Database::open(temp.path()).await.unwrap();
    let columns: Vec<(String, String, i64)> = db
        .db_call(|conn| {
            conn.prepare("SELECT name, type, \"notnull\" FROM pragma_table_info('repo_paths')")
                .map_err(anyhow::Error::from)?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .map_err(anyhow::Error::from)?
                .collect::<Result<_, _>>()
                .map_err(anyhow::Error::from)
        })
        .await
        .unwrap();

    let verify = columns
        .iter()
        .find(|(n, _, _)| n == "verify_command")
        .expect("verify_command column must be added by migration v52");
    assert_eq!(verify.1, "TEXT");
    assert_eq!(verify.2, 0, "verify_command must be nullable");
}

#[tokio::test]
async fn fresh_db_has_verify_command_column() {
    let temp = tempfile::NamedTempFile::new().unwrap();
    let db = crate::db::Database::open(temp.path()).await.unwrap();
    db.db_call(|conn| {
        conn.execute(
            "INSERT INTO repo_paths(path, verify_command) VALUES('/x', 'cargo test')",
            [],
        )
        .map(|_| ())
        .map_err(anyhow::Error::from)
    })
    .await
    .expect("verify_command must exist on fresh schema");
}

#[test]
fn test_v55_learning_embedding_column() {
    use rusqlite::Connection as RawConn;
    let conn = RawConn::open_in_memory().unwrap();
    super::super::init_schema_sync(&conn).unwrap();
    // embedding column is nullable BLOB
    conn.execute(
        "INSERT INTO learnings (kind, summary, scope, status, tags, upvote_count, created_at, updated_at) VALUES ('convention', 's', 'user', 'approved', '[]', 0, '2026-05-18T00:00:00Z', '2026-05-18T00:00:00Z')",
        [],
    ).unwrap();
    conn.execute("UPDATE learnings SET embedding = X'01020304'", [])
        .unwrap();
    let val: Option<Vec<u8>> = conn
        .query_row("SELECT embedding FROM learnings LIMIT 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(val, Some(vec![1, 2, 3, 4]));
}

// ---------------------------------------------------------------------------
// v80 — drop the orphaned agent_status column
// ---------------------------------------------------------------------------

#[test]
fn migrate_v80_drops_agent_status_and_keeps_sibling_columns() {
    use rusqlite::Connection as RawConn;
    let conn = RawConn::open_in_memory().unwrap();
    // Pre-v80 shape of the four tables v27/v28 added agent_status to.
    conn.execute_batch(
        "CREATE TABLE review_prs (repo TEXT, number INTEGER, tmux_window TEXT,
                                  worktree TEXT, agent_status TEXT);
         CREATE TABLE bot_prs (repo TEXT, number INTEGER, agent_status TEXT);
         CREATE TABLE security_alerts (repo TEXT, number INTEGER, agent_status TEXT);
         CREATE TABLE my_prs (repo TEXT, number INTEGER, agent_status TEXT);
         INSERT INTO review_prs (repo, number, tmux_window, worktree, agent_status)
             VALUES ('kognic/x', 7, 'win', '/wt', 'reviewing');
         PRAGMA user_version = 79;",
    )
    .unwrap();

    crate::db::migrations::migrate_v80_drop_agent_status(&conn).unwrap();

    for table in ["review_prs", "bot_prs", "security_alerts", "my_prs"] {
        let cols: Vec<String> = conn
            .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(
            !cols.contains(&"agent_status".to_string()),
            "agent_status must be gone from {table}, got {cols:?}"
        );
    }

    // Sibling columns and their data survive the drop.
    let (window, worktree): (String, String) = conn
        .query_row("SELECT tmux_window, worktree FROM review_prs", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!((window.as_str(), worktree.as_str()), ("win", "/wt"));
}

#[test]
fn migrate_v80_is_idempotent_and_tolerates_missing_tables() {
    use rusqlite::Connection as RawConn;
    let conn = RawConn::open_in_memory().unwrap();
    // No agent_status column, and three of the four tables absent entirely.
    conn.execute_batch("CREATE TABLE my_prs (repo TEXT, number INTEGER);")
        .unwrap();
    crate::db::migrations::migrate_v80_drop_agent_status(&conn).unwrap();
    crate::db::migrations::migrate_v80_drop_agent_status(&conn).unwrap();
}

#[tokio::test]
async fn fresh_db_has_no_agent_status_column_anywhere() {
    let db = in_memory_db().await;
    let hits: i64 = db
        .db_call(|conn| {
            conn.query_row(
                "SELECT count(*) FROM sqlite_master m
                 JOIN pragma_table_info(m.name) c
                 WHERE m.type = 'table' AND c.name = 'agent_status'",
                [],
                |r| r.get(0),
            )
            .map_err(anyhow::Error::from)
        })
        .await
        .unwrap();
    assert_eq!(hits, 0, "no table may carry agent_status after v80");
}

#[tokio::test]
async fn migrate_v56_drops_task_usage_table() {
    let db = in_memory_db().await;

    let count: i64 = db
        .db_call(|conn| {
            conn.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='task_usage'",
                [],
                |r| r.get(0),
            )
            .map_err(anyhow::Error::from)
        })
        .await
        .unwrap();
    assert_eq!(count, 0, "task_usage should be dropped by v56");
}

// ---------------------------------------------------------------------------
// v57 — epic project consistency triggers + data fix
// ---------------------------------------------------------------------------

#[test]
fn migrate_v57_fixes_sub_epic_project_id_violations() {
    use rusqlite::Connection as RawConn;
    let conn = RawConn::open_in_memory().unwrap();
    // Minimal pre-v57 schema: two projects, parent epic in project 2, sub-epic mistakenly in project 1.
    conn.execute_batch(
        "PRAGMA foreign_keys=OFF;
         CREATE TABLE projects (id INTEGER PRIMARY KEY, name TEXT NOT NULL, sort_order INTEGER NOT NULL DEFAULT 0, is_default INTEGER NOT NULL DEFAULT 0);
         INSERT INTO projects VALUES (1, 'Default', 0, 1);
         INSERT INTO projects VALUES (2, 'Work', 1, 0);
         CREATE TABLE epics (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             description TEXT NOT NULL DEFAULT '',
             repo_path TEXT NOT NULL DEFAULT '',
             status TEXT NOT NULL DEFAULT 'backlog',
             plan_path TEXT,
             sort_order INTEGER,
             auto_dispatch BOOLEAN NOT NULL DEFAULT 1,
             parent_epic_id INTEGER,
             feed_command TEXT,
             feed_interval_secs INTEGER,
             project_id INTEGER NOT NULL DEFAULT 1,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE tasks (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             description TEXT NOT NULL DEFAULT '',
             repo_path TEXT NOT NULL DEFAULT '',
             status TEXT NOT NULL DEFAULT 'backlog',
             sub_status TEXT NOT NULL DEFAULT 'none',
             base_branch TEXT NOT NULL DEFAULT 'main',
             epic_id INTEGER,
             project_id INTEGER NOT NULL DEFAULT 1,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         -- parent epic in project 2
         INSERT INTO epics (id, title, project_id) VALUES (1, 'Parent', 2);
         -- sub-epic in wrong project (1 instead of 2)
         INSERT INTO epics (id, title, project_id, parent_epic_id) VALUES (2, 'Sub', 1, 1);
         -- grandchild also wrong
         INSERT INTO epics (id, title, project_id, parent_epic_id) VALUES (3, 'Grandchild', 1, 2);
         -- task under sub-epic in wrong project
         INSERT INTO tasks (id, title, epic_id, project_id) VALUES (1, 'Task', 2, 1);
         PRAGMA user_version = 56;",
    )
    .unwrap();

    crate::db::migrations::migrate_v57_enforce_epic_project_consistency(&conn).unwrap();

    let sub_pid: i64 = conn
        .query_row("SELECT project_id FROM epics WHERE id = 2", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        sub_pid, 2,
        "sub-epic project_id must be fixed to match parent"
    );

    let grandchild_pid: i64 = conn
        .query_row("SELECT project_id FROM epics WHERE id = 3", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        grandchild_pid, 2,
        "grandchild project_id must be fixed (multi-level)"
    );

    let task_pid: i64 = conn
        .query_row("SELECT project_id FROM tasks WHERE id = 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(task_pid, 2, "task project_id must be fixed to match epic");
}

// ---------------------------------------------------------------------------
// v58 — reset intermediate epic statuses to backlog
// ---------------------------------------------------------------------------

#[test]
fn migrate_v58_resets_running_and_review_epics_to_backlog() {
    use rusqlite::Connection as RawConn;
    let conn = RawConn::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE epics (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL DEFAULT '',
             description TEXT NOT NULL DEFAULT '',
             repo_path TEXT NOT NULL DEFAULT '',
             status TEXT NOT NULL DEFAULT 'backlog',
             plan_path TEXT,
             sort_order INTEGER,
             auto_dispatch BOOLEAN NOT NULL DEFAULT 1,
             parent_epic_id INTEGER,
             feed_command TEXT,
             feed_interval_secs INTEGER,
             project_id INTEGER NOT NULL DEFAULT 1,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now')),
             group_by_repo BOOLEAN NOT NULL DEFAULT 0
         );
         INSERT INTO epics (id, title, status) VALUES (1, 'A', 'running');
         INSERT INTO epics (id, title, status) VALUES (2, 'B', 'review');
         INSERT INTO epics (id, title, status) VALUES (3, 'C', 'backlog');
         INSERT INTO epics (id, title, status) VALUES (4, 'D', 'done');",
    )
    .unwrap();

    crate::db::migrations::migrate_v58_reset_intermediate_epic_statuses(&conn).unwrap();

    let status = |id: i64| -> String {
        conn.query_row("SELECT status FROM epics WHERE id = ?", [id], |row| {
            row.get(0)
        })
        .unwrap()
    };

    assert_eq!(status(1), "backlog"); // running → backlog
    assert_eq!(status(2), "backlog"); // review → backlog
    assert_eq!(status(3), "backlog"); // backlog unchanged
    assert_eq!(status(4), "done"); // done unchanged
}

#[tokio::test]
async fn migration_v60_drops_projects_table_and_columns() {
    let db = in_memory_db().await;
    let (projects_gone, tasks_no_project_id, epics_no_project_id): (i64, i64, i64) = db
        .db_call(|conn| {
            let projects_gone: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='projects'",
                    [],
                    |r| r.get(0),
                )
                .map_err(anyhow::Error::from)?;
            let tasks_no_project_id: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('tasks') WHERE name='project_id'",
                    [],
                    |r| r.get(0),
                )
                .map_err(anyhow::Error::from)?;
            let epics_no_project_id: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('epics') WHERE name='project_id'",
                    [],
                    |r| r.get(0),
                )
                .map_err(anyhow::Error::from)?;
            Ok((projects_gone, tasks_no_project_id, epics_no_project_id))
        })
        .await
        .unwrap();
    assert_eq!(projects_gone, 0, "projects table should be dropped by v60");
    assert_eq!(
        tasks_no_project_id, 0,
        "tasks.project_id should be dropped by v60"
    );
    assert_eq!(
        epics_no_project_id, 0,
        "epics.project_id should be dropped by v60"
    );
}

#[test]
fn migration_v60_migrates_project_namespaced_repo_filter_settings() {
    use crate::db::migrations::migrate_v60_drop_projects;
    use rusqlite::{Connection as RawConn, OptionalExtension};
    let conn = RawConn::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE tasks (id INTEGER PRIMARY KEY, title TEXT NOT NULL, description TEXT NOT NULL DEFAULT '', repo_path TEXT NOT NULL DEFAULT '', status TEXT NOT NULL DEFAULT 'backlog', sub_status TEXT NOT NULL DEFAULT 'none', base_branch TEXT NOT NULL DEFAULT 'main', project_id INTEGER NOT NULL DEFAULT 1);
         CREATE TABLE epics (id INTEGER PRIMARY KEY, title TEXT NOT NULL, description TEXT NOT NULL DEFAULT '', repo_path TEXT NOT NULL DEFAULT '', status TEXT NOT NULL DEFAULT 'backlog', project_id INTEGER NOT NULL DEFAULT 1);
         CREATE TABLE learnings (id INTEGER PRIMARY KEY, scope TEXT NOT NULL, content TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'pending');
         CREATE TABLE projects (id INTEGER PRIMARY KEY, name TEXT NOT NULL, sort_order INTEGER NOT NULL DEFAULT 0);
         INSERT INTO settings VALUES ('repo_filter:1', 'myrepo');
         INSERT INTO settings VALUES ('repo_filter_mode:1', 'include');
         INSERT INTO settings VALUES ('other_key', 'untouched');",
    )
    .unwrap();

    migrate_v60_drop_projects(&conn).unwrap();

    let filter: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'repo_filter'",
            [],
            |r| r.get(0),
        )
        .optional()
        .unwrap();
    assert_eq!(
        filter.as_deref(),
        Some("myrepo"),
        "repo_filter:1 should be promoted to repo_filter"
    );

    let mode: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'repo_filter_mode'",
            [],
            |r| r.get(0),
        )
        .optional()
        .unwrap();
    assert_eq!(
        mode.as_deref(),
        Some("include"),
        "repo_filter_mode:1 should be promoted to repo_filter_mode"
    );

    let old_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM settings WHERE key LIKE 'repo_filter:%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        old_count, 0,
        "namespaced repo_filter keys should be deleted"
    );

    let other: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'other_key'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        other, "untouched",
        "unrelated settings should not be touched"
    );
}

#[test]
fn migration_v60_repo_filter_does_not_overwrite_existing_unnamespaced_key() {
    use crate::db::migrations::migrate_v60_drop_projects;
    use rusqlite::Connection as RawConn;
    let conn = RawConn::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE tasks (id INTEGER PRIMARY KEY, title TEXT NOT NULL, description TEXT NOT NULL DEFAULT '', repo_path TEXT NOT NULL DEFAULT '', status TEXT NOT NULL DEFAULT 'backlog', sub_status TEXT NOT NULL DEFAULT 'none', base_branch TEXT NOT NULL DEFAULT 'main', project_id INTEGER NOT NULL DEFAULT 1);
         CREATE TABLE epics (id INTEGER PRIMARY KEY, title TEXT NOT NULL, description TEXT NOT NULL DEFAULT '', repo_path TEXT NOT NULL DEFAULT '', status TEXT NOT NULL DEFAULT 'backlog', project_id INTEGER NOT NULL DEFAULT 1);
         CREATE TABLE learnings (id INTEGER PRIMARY KEY, scope TEXT NOT NULL, content TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'pending');
         CREATE TABLE projects (id INTEGER PRIMARY KEY, name TEXT NOT NULL, sort_order INTEGER NOT NULL DEFAULT 0);
         INSERT INTO settings VALUES ('repo_filter', 'already-set');
         INSERT INTO settings VALUES ('repo_filter:1', 'old-value');",
    )
    .unwrap();

    migrate_v60_drop_projects(&conn).unwrap();

    let filter: String = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'repo_filter'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        filter, "already-set",
        "existing unnamespaced key must not be overwritten"
    );
}

// ---------------------------------------------------------------------------
// Table-existence guard: skip gracefully when target table is absent
// ---------------------------------------------------------------------------

#[test]
fn migrate_v43_skips_when_learnings_absent() {
    use crate::db::migrations::migrate_v43_proposed_to_approved;
    use rusqlite::Connection as RawConn;
    let conn = RawConn::open_in_memory().unwrap();
    migrate_v43_proposed_to_approved(&conn).unwrap();
}

#[test]
fn migrate_v46_skips_when_learnings_absent() {
    use crate::db::migrations::migrate_v46_learning_source_task_set_null;
    use rusqlite::Connection as RawConn;
    let conn = RawConn::open_in_memory().unwrap();
    migrate_v46_learning_source_task_set_null(&conn).unwrap();
}

#[test]
fn migrate_v47_skips_when_task_usage_absent() {
    use crate::db::migrations::migrate_v47_task_usage_restore_cascade;
    use rusqlite::Connection as RawConn;
    let conn = RawConn::open_in_memory().unwrap();
    migrate_v47_task_usage_restore_cascade(&conn).unwrap();
}

#[test]
fn migrate_v55_skips_when_learnings_absent() {
    use crate::db::migrations::migrate_v55_add_learning_embedding;
    use rusqlite::Connection as RawConn;
    let conn = RawConn::open_in_memory().unwrap();
    migrate_v55_add_learning_embedding(&conn).unwrap();
}

#[test]
fn migration_v60_deletes_project_scoped_learnings() {
    use crate::db::migrations::migrate_v60_drop_projects;
    use rusqlite::Connection as RawConn;
    let conn = RawConn::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE tasks (id INTEGER PRIMARY KEY, title TEXT NOT NULL, description TEXT NOT NULL DEFAULT '', repo_path TEXT NOT NULL DEFAULT '', status TEXT NOT NULL DEFAULT 'backlog', sub_status TEXT NOT NULL DEFAULT 'none', base_branch TEXT NOT NULL DEFAULT 'main', project_id INTEGER NOT NULL DEFAULT 1);
         CREATE TABLE epics (id INTEGER PRIMARY KEY, title TEXT NOT NULL, description TEXT NOT NULL DEFAULT '', repo_path TEXT NOT NULL DEFAULT '', status TEXT NOT NULL DEFAULT 'backlog', project_id INTEGER NOT NULL DEFAULT 1);
         CREATE TABLE learnings (id INTEGER PRIMARY KEY, scope TEXT NOT NULL, content TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'pending');
         CREATE TABLE projects (id INTEGER PRIMARY KEY, name TEXT NOT NULL, sort_order INTEGER NOT NULL DEFAULT 0);
         INSERT INTO learnings (scope, content, status) VALUES ('project', 'old learning', 'approved');
         INSERT INTO learnings (scope, content, status) VALUES ('user', 'keep me', 'approved');
         INSERT INTO learnings (scope, content, status) VALUES ('repo', 'keep me too', 'pending');",
    )
    .unwrap();

    migrate_v60_drop_projects(&conn).unwrap();

    let project_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM learnings WHERE scope = 'project'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        project_count, 0,
        "project-scoped learnings should be deleted by v60"
    );

    let remaining: i64 = conn
        .query_row("SELECT COUNT(*) FROM learnings", [], |r| r.get(0))
        .unwrap();
    assert_eq!(remaining, 2, "non-project learnings should be preserved");
}

#[tokio::test]
async fn migration_v63_adds_idx_tasks_status_and_epic_id() {
    let db = in_memory_db().await;
    let (status_idx, epic_id_idx): (i64, i64) = db
        .db_call(|conn| {
            let status_idx: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_tasks_status'",
                    [],
                    |r| r.get(0),
                )
                .map_err(anyhow::Error::from)?;
            let epic_id_idx: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_tasks_epic_id'",
                    [],
                    |r| r.get(0),
                )
                .map_err(anyhow::Error::from)?;
            Ok((status_idx, epic_id_idx))
        })
        .await
        .unwrap();
    assert_eq!(
        status_idx, 1,
        "idx_tasks_status must exist after migration v63"
    );
    assert_eq!(
        epic_id_idx, 1,
        "idx_tasks_epic_id must exist after migration v63"
    );
}

#[tokio::test]
async fn v69_adds_origin_column_and_repo_group_index() {
    let db = in_memory_db().await;

    // Verify the origin column exists.
    let has_origin: bool = db
        .db_call(|conn| {
            conn.prepare("SELECT 1 FROM pragma_table_info('epics') WHERE name = 'origin'")
                .and_then(|mut stmt| {
                    stmt.query_map([], |_| Ok(()))
                        .map(|mut rows| rows.next().is_some())
                })
                .map_err(anyhow::Error::from)
        })
        .await
        .unwrap();
    assert!(has_origin, "origin column should exist on epics table");

    // Two RepoGroup sub-epics with DIFFERENT titles under one parent: allowed.
    let root = db.create_epic("root", "", None).await.unwrap();
    let result = set_origin_raw(&db, root.id.0, "manual").await;
    assert!(result.is_ok(), "setting manual origin should succeed");

    let a = db.create_epic("repo-a", "", Some(root.id)).await.unwrap();
    let result = set_origin_raw(&db, a.id.0, "repo-group").await;
    assert!(result.is_ok(), "setting repo-group origin should succeed");

    let b = db.create_epic("repo-b", "", Some(root.id)).await.unwrap();
    let result = set_origin_raw(&db, b.id.0, "repo-group").await;
    assert!(
        result.is_ok(),
        "different repo-group titles under same parent should be allowed"
    );

    // A SECOND RepoGroup sub-epic with the SAME title under the same parent: rejected.
    let c = db.create_epic("repo-a", "", Some(root.id)).await.unwrap();
    let dup = set_origin_raw(&db, c.id.0, "repo-group").await;
    assert!(
        dup.is_err(),
        "duplicate RepoGroup (parent,title) must violate the unique index"
    );

    // A Manual sub-epic with the same title coexists (index only constrains repo-group).
    let d = db.create_epic("repo-a", "", Some(root.id)).await.unwrap();
    let result = set_origin_raw(&db, d.id.0, "manual").await;
    assert!(
        result.is_ok(),
        "manual origin with duplicate title should be allowed (index is partial)"
    );
}

/// Set `epics.origin` directly via SQL so the migration's unique index can be exercised.
async fn set_origin_raw(db: &Database, epic_id: i64, origin: &str) -> anyhow::Result<()> {
    let origin = origin.to_string();
    db.db_call(move |conn| {
        conn.execute(
            "UPDATE epics SET origin = ?1 WHERE id = ?2",
            rusqlite::params![origin, epic_id],
        )
        .map(|_| ())
        .map_err(anyhow::Error::from)
    })
    .await
}

/// v71 removes repo-sub-epic task copies when the same external_id
/// already exists directly on the parent role sub-epic.
#[tokio::test]
async fn v71_dedup_removes_repo_sub_epic_shadow_tasks() {
    // in_memory_db() runs ALL migrations including v72, which installs the
    // BEFORE INSERT trigger.  Two tasks with the same external_id can't be
    // inserted directly, so we insert both with external_id=NULL and then
    // UPDATE to set it — the BEFORE UPDATE OF epic_id trigger doesn't fire
    // for external_id-only updates, so this path is safe.
    let db = in_memory_db().await;
    db.db_call(|conn| {
        conn.execute_batch(
            "INSERT INTO epics (id, title, description, status, feed_role, origin)
             VALUES (1, 'PR Reviews', '', 'backlog', 'reviews-parent', 'manual');
             INSERT INTO epics (id, title, description, status, feed_role, origin, parent_epic_id)
             VALUES (2, 'My Reviews', '', 'backlog', 'my-reviews', 'manual', 1);
             INSERT INTO epics (id, title, description, status, feed_role, origin, parent_epic_id)
             VALUES (3, 'myrepo', '', 'backlog', 'none', 'repo-group', 2);
             INSERT INTO tasks (id, title, description, repo_path, status, base_branch, epic_id)
             VALUES (10, 'PR #1', '', '/r', 'backlog', 'main', 2);
             INSERT INTO tasks (id, title, description, repo_path, status, base_branch, epic_id)
             VALUES (11, 'PR #1 dup', '', '/r', 'backlog', 'main', 3);
             UPDATE tasks SET external_id = 'pr-1' WHERE id IN (10, 11);",
        )
        .map_err(anyhow::Error::from)
    })
    .await
    .unwrap();

    db.db_call(|conn| crate::db::migrations::migrate_v71_dedup_role_subtree_tasks(conn))
        .await
        .unwrap();

    let count: i64 = db
        .db_call(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM tasks WHERE external_id = 'pr-1'",
                [],
                |r| r.get(0),
            )
            .map_err(anyhow::Error::from)
        })
        .await
        .unwrap();
    assert_eq!(count, 1, "duplicate repo-sub-epic copy must be deleted");

    let surviving_epic: i64 = db
        .db_call(|conn| {
            conn.query_row(
                "SELECT epic_id FROM tasks WHERE external_id = 'pr-1'",
                [],
                |r| r.get(0),
            )
            .map_err(anyhow::Error::from)
        })
        .await
        .unwrap();
    assert_eq!(surviving_epic, 2, "surviving task must be on role sub-epic");
}

/// v71 leaves tasks untouched when there is no matching copy on the parent.
#[tokio::test]
async fn v71_dedup_preserves_solo_repo_sub_epic_tasks() {
    let db = in_memory_db().await;
    db.db_call(|conn| {
        conn.execute_batch(
            "INSERT INTO epics (id, title, description, status, feed_role, origin)
             VALUES (1, 'PR Reviews', '', 'backlog', 'reviews-parent', 'manual');
             INSERT INTO epics (id, title, description, status, feed_role, origin, parent_epic_id)
             VALUES (2, 'My Reviews', '', 'backlog', 'my-reviews', 'manual', 1);
             INSERT INTO epics (id, title, description, status, feed_role, origin, parent_epic_id)
             VALUES (3, 'myrepo', '', 'backlog', 'none', 'repo-group', 2);
             INSERT INTO tasks (id, title, description, repo_path, status, base_branch, epic_id, external_id)
             VALUES (10, 'PR #2', '', '/r', 'backlog', 'main', 3, 'pr-2');",
        )
        .map_err(anyhow::Error::from)
    })
    .await
    .unwrap();

    db.db_call(|conn| crate::db::migrations::migrate_v71_dedup_role_subtree_tasks(conn))
        .await
        .unwrap();

    let count: i64 = db
        .db_call(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM tasks WHERE external_id = 'pr-2'",
                [],
                |r| r.get(0),
            )
            .map_err(anyhow::Error::from)
        })
        .await
        .unwrap();
    assert_eq!(count, 1, "solo repo-sub-epic task must survive");
}

/// Inserting a task with a duplicate external_id into the same role sub-epic
/// must be rejected. Since v76, a plain second INSERT (no ON CONFLICT
/// clause) at the same (epic_id, external_id) is rejected by the
/// `tasks_epic_external_id` unique index (v38) rather than by the subtree
/// trigger itself — the trigger now excludes same-epic matches so it no
/// longer false-positives on ON CONFLICT DO UPDATE upserts — but the net
/// effect asserted here (insert is still rejected) is unchanged.
#[tokio::test]
async fn v72_trigger_blocks_duplicate_insert_into_role_sub_epic() {
    let db = in_memory_db().await;
    db.db_call(|conn| {
        conn.execute_batch(
            "INSERT INTO epics (id, title, description, status, feed_role, origin)
             VALUES (1, 'PR Reviews', '', 'backlog', 'reviews-parent', 'manual');
             INSERT INTO epics (id, title, description, status, feed_role, origin, parent_epic_id)
             VALUES (2, 'My Reviews', '', 'backlog', 'my-reviews', 'manual', 1);
             INSERT INTO tasks (id, title, description, repo_path, status, base_branch, epic_id, external_id)
             VALUES (10, 'PR #1', '', '/r', 'backlog', 'main', 2, 'pr-1');",
        )
        .map_err(anyhow::Error::from)
    })
    .await
    .unwrap();

    let result = db
        .db_call(|conn| {
            conn.execute(
                "INSERT INTO tasks (title, description, repo_path, status, base_branch, epic_id, external_id)
                 VALUES ('PR #1 dup', '', '/r', 'backlog', 'main', 2, 'pr-1')",
                [],
            )
            .map_err(anyhow::Error::from)
        })
        .await;

    assert!(
        result.is_err(),
        "trigger must reject duplicate external_id in same role sub-epic"
    );
}

/// Inserting a task with a duplicate external_id into a repo-group sub-epic
/// whose parent role sub-epic already has that external_id must be rejected.
#[tokio::test]
async fn v72_trigger_blocks_duplicate_insert_into_repo_group_sub_epic() {
    let db = in_memory_db().await;
    db.db_call(|conn| {
        conn.execute_batch(
            "INSERT INTO epics (id, title, description, status, feed_role, origin)
             VALUES (1, 'PR Reviews', '', 'backlog', 'reviews-parent', 'manual');
             INSERT INTO epics (id, title, description, status, feed_role, origin, parent_epic_id)
             VALUES (2, 'My Reviews', '', 'backlog', 'my-reviews', 'manual', 1);
             INSERT INTO epics (id, title, description, status, feed_role, origin, parent_epic_id)
             VALUES (3, 'myrepo', '', 'backlog', 'none', 'repo-group', 2);
             INSERT INTO tasks (id, title, description, repo_path, status, base_branch, epic_id, external_id)
             VALUES (10, 'PR #1', '', '/r', 'backlog', 'main', 2, 'pr-1');",
        )
        .map_err(anyhow::Error::from)
    })
    .await
    .unwrap();

    let result = db
        .db_call(|conn| {
            conn.execute(
                "INSERT INTO tasks (title, description, repo_path, status, base_branch, epic_id, external_id)
                 VALUES ('PR #1 dup', '', '/r', 'backlog', 'main', 3, 'pr-1')",
                [],
            )
            .map_err(anyhow::Error::from)
        })
        .await;

    assert!(
        result.is_err(),
        "trigger must reject insert into repo-group sub-epic when role sub-epic already has same external_id"
    );
}

/// Moving a task into a role sub-epic subtree that already holds the same
/// external_id must be rejected.
#[tokio::test]
async fn v72_trigger_blocks_move_that_creates_duplicate() {
    let db = in_memory_db().await;
    db.db_call(|conn| {
        conn.execute_batch(
            "INSERT INTO epics (id, title, description, status, feed_role, origin)
             VALUES (1, 'PR Reviews', '', 'backlog', 'reviews-parent', 'manual');
             INSERT INTO epics (id, title, description, status, feed_role, origin, parent_epic_id)
             VALUES (2, 'My Reviews', '', 'backlog', 'my-reviews', 'manual', 1);
             INSERT INTO epics (id, title, description, status, feed_role, origin, parent_epic_id)
             VALUES (3, 'Team Reviews', '', 'backlog', 'team-reviews', 'manual', 1);
             -- pr-1 exists in My Reviews
             INSERT INTO tasks (id, title, description, repo_path, status, base_branch, epic_id, external_id)
             VALUES (10, 'PR #1', '', '/r', 'backlog', 'main', 2, 'pr-1');
             -- pr-1 also in Team Reviews (this is the pre-existing duplicate we are moving)
             INSERT INTO tasks (id, title, description, repo_path, status, base_branch, epic_id, external_id)
             VALUES (11, 'PR #1 team', '', '/r', 'backlog', 'main', 3, 'pr-1');",
        )
        .map_err(anyhow::Error::from)
    })
    .await
    .unwrap();

    // Try to move task 11 from Team Reviews to My Reviews — should fail
    // because My Reviews already has pr-1.
    let result = db
        .db_call(|conn| {
            conn.execute("UPDATE tasks SET epic_id = 2 WHERE id = 11", [])
                .map_err(anyhow::Error::from)
        })
        .await;

    assert!(
        result.is_err(),
        "trigger must reject move that would create duplicate external_id in target subtree"
    );
}

/// Tasks with NULL external_id are never subject to the trigger.
#[tokio::test]
async fn v72_trigger_allows_manual_tasks_with_null_external_id() {
    let db = in_memory_db().await;
    db.db_call(|conn| {
        conn.execute_batch(
            "INSERT INTO epics (id, title, description, status, feed_role, origin)
             VALUES (1, 'PR Reviews', '', 'backlog', 'reviews-parent', 'manual');
             INSERT INTO epics (id, title, description, status, feed_role, origin, parent_epic_id)
             VALUES (2, 'My Reviews', '', 'backlog', 'my-reviews', 'manual', 1);",
        )
        .map_err(anyhow::Error::from)
    })
    .await
    .unwrap();

    // Two manual tasks with NULL external_id should coexist fine.
    let result = db
        .db_call(|conn| {
            conn.execute_batch(
                "INSERT INTO tasks (title, description, repo_path, status, base_branch, epic_id)
                 VALUES ('Manual 1', '', '/r', 'backlog', 'main', 2);
                 INSERT INTO tasks (title, description, repo_path, status, base_branch, epic_id)
                 VALUES ('Manual 2', '', '/r', 'backlog', 'main', 2);",
            )
            .map_err(anyhow::Error::from)
        })
        .await;

    assert!(
        result.is_ok(),
        "manual tasks with NULL external_id must not trigger the constraint"
    );
}

// ---------------------------------------------------------------------------
// v75 — create repo_base_branches table (see docs/specs/dispatch.allium:
// RecordBaseBranch/BaseBranchPicker and docs/specs/core.allium: SavedRepoBranch)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fresh_db_creates_repo_base_branches_table() {
    let db = in_memory_db().await;
    let exists: i64 = db
        .db_call(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name = 'repo_base_branches'",
                [],
                |r| r.get(0),
            )
            .map_err(anyhow::Error::from)
        })
        .await
        .unwrap();
    assert_eq!(
        exists, 1,
        "expected migration v75 to create the repo_base_branches table"
    );
}

#[test]
fn migrate_v75_creates_repo_base_branches_table_on_legacy_db() {
    use rusqlite::Connection as RawConn;
    // Simulate a v74 DB: no repo_base_branches table yet.
    let conn = RawConn::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA user_version = 74;").unwrap();

    crate::db::migrations::migrate_v75_create_repo_base_branches(&conn).unwrap();

    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name = 'repo_base_branches'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        exists, 1,
        "migrate_v75 should create the repo_base_branches table on a legacy DB"
    );

    // Idempotent: running again on a schema that already has the table must not error.
    crate::db::migrations::migrate_v75_create_repo_base_branches(&conn).unwrap();
}

#[test]
fn migrate_v75_repo_base_branches_schema_supports_upsert_and_recency_query() {
    use rusqlite::Connection as RawConn;
    let conn = RawConn::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA user_version = 74;").unwrap();
    crate::db::migrations::migrate_v75_create_repo_base_branches(&conn).unwrap();

    // Columns required by RecordBaseBranch / BaseBranchPicker: repo_path,
    // branch, last_used. Identity is the (repo_path, branch) pair (see
    // core.allium: SavedRepoBranch / UniqueBranchPerRepo), so a second insert
    // of the same pair must be rejected by a uniqueness constraint (the
    // production upsert uses ON CONFLICT against exactly this constraint).
    conn.execute(
        "INSERT INTO repo_base_branches (repo_path, branch, last_used) VALUES ('/r', 'main', datetime('now'))",
        [],
    )
    .unwrap();
    let dup = conn.execute(
        "INSERT INTO repo_base_branches (repo_path, branch, last_used) VALUES ('/r', 'main', datetime('now'))",
        [],
    );
    assert!(
        dup.is_err(),
        "expected a uniqueness violation on (repo_path, branch), got: {dup:?}"
    );
}

#[tokio::test]
async fn auto_run_plan_column_defaults_to_false() {
    let db = in_memory_db().await;
    let id = db
        .create_task(CreateTaskRequest {
            title: "T",
            description: "d",
            repo_path: "/r",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
            schedule_interval_secs: None,
            pinned_branch: None,
        })
        .await
        .unwrap();
    let task = db.get_task(id).await.unwrap().expect("task should exist");
    assert!(!task.auto_run_plan);
}

#[tokio::test]
async fn fresh_db_creates_task_watchers_table() {
    let db = in_memory_db().await;
    let exists: i64 = db
        .db_call(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name = 'task_watchers'",
                [],
                |r| r.get(0),
            )
            .map_err(anyhow::Error::from)
        })
        .await
        .unwrap();
    assert_eq!(
        exists, 1,
        "expected migration v78 to create the task_watchers table"
    );
}

#[test]
fn migrate_v78_creates_task_watchers_table_on_legacy_db() {
    use rusqlite::Connection as RawConn;
    let conn = RawConn::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA user_version = 77;").unwrap();

    crate::db::migrations::migrate_v78_create_task_watchers(&conn).unwrap();

    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name = 'task_watchers'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        exists, 1,
        "migrate_v78 should create the task_watchers table on a legacy DB"
    );

    // Idempotent: running again must not error.
    crate::db::migrations::migrate_v78_create_task_watchers(&conn).unwrap();
}

#[test]
fn migrate_v78_task_watchers_unique_constraint_rejects_duplicate_pair() {
    use rusqlite::Connection as RawConn;
    let conn = RawConn::open_in_memory().unwrap();
    crate::db::migrations::migrate_v78_create_task_watchers(&conn).unwrap();

    conn.execute(
        "INSERT INTO task_watchers (watcher_task_id, target_task_id) VALUES (1, 2)",
        [],
    )
    .unwrap();

    let err = conn
        .execute(
            "INSERT INTO task_watchers (watcher_task_id, target_task_id) VALUES (1, 2)",
            [],
        )
        .unwrap_err();
    assert!(format!("{err}").to_lowercase().contains("unique"));
}

/// Regression test for the race described in task #3724: `init_schema_sync`
/// used to run a migration's DDL and its `user_version` bump as two separate
/// statements, with no shared transaction and no cross-process lock around
/// the pair. Two processes opening the same DB file concurrently could both
/// observe the old `user_version` and both apply the same migration.
///
/// This exercises the extracted `apply_pending_migrations` helper directly
/// with a synthetic, deliberately non-idempotent migration (an unconditional
/// INSERT, mirroring the real `migrate_v39_add_projects`'s unguarded
/// `INSERT INTO projects ... VALUES ('Default', ...)`), racing two real
/// file-backed connections on two OS threads. This is not timing-flaky: the
/// pre-fix bug is a structural check-then-act race (both threads decide
/// "pending" from a version read taken before either one starts writing),
/// not a narrow window — so it reproduces deterministically without the fix,
/// and is deterministically fixed once each migration is applied inside a
/// single `BEGIN IMMEDIATE` transaction that re-checks the version before
/// running the migration body.
#[test]
fn concurrent_open_applies_pending_migration_exactly_once() {
    fn insert_marker(conn: &rusqlite::Connection) -> anyhow::Result<()> {
        conn.execute("INSERT INTO marker DEFAULT VALUES", [])?;
        Ok(())
    }
    const TEST_MIGRATIONS: &[crate::db::migrations::Migration] = &[(1, insert_marker)];

    let temp = tempfile::NamedTempFile::new().unwrap();
    {
        let setup = rusqlite::Connection::open(temp.path()).unwrap();
        setup
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 CREATE TABLE marker (id INTEGER PRIMARY KEY);",
            )
            .unwrap();
    }

    let run = |path: std::path::PathBuf| {
        std::thread::spawn(move || {
            let conn = rusqlite::Connection::open(path).unwrap();
            conn.busy_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            crate::db::apply_pending_migrations(&conn, TEST_MIGRATIONS).unwrap();
        })
    };
    let a = run(temp.path().to_path_buf());
    let b = run(temp.path().to_path_buf());
    a.join().unwrap();
    b.join().unwrap();

    let conn = rusqlite::Connection::open(temp.path()).unwrap();
    let marker_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM marker", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        marker_count, 1,
        "migration must apply exactly once even when two connections race to apply it"
    );
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(version, 1);
}

#[tokio::test]
async fn v79_backfills_sort_order_for_done_tasks_and_epics() {
    use rusqlite::Connection as RawConn;
    let conn = RawConn::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE tasks (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             status TEXT NOT NULL,
             sort_order INTEGER,
             updated_at TEXT NOT NULL
         );
         CREATE TABLE epics (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             status TEXT NOT NULL,
             sort_order INTEGER,
             updated_at TEXT NOT NULL
         );
         INSERT INTO tasks (title, status, sort_order, updated_at) VALUES
           ('done-no-sort-order', 'done', NULL, '2026-01-15 12:00:00'),
           ('done-already-sorted', 'done', -999, '2026-01-15 12:00:00'),
           ('not-done', 'backlog', NULL, '2026-01-15 12:00:00');
         INSERT INTO epics (title, status, sort_order, updated_at) VALUES
           ('epic-done-no-sort-order', 'done', NULL, '2026-02-01 08:30:00'),
           ('epic-not-done', 'running', NULL, '2026-02-01 08:30:00');",
    )
    .unwrap();

    crate::db::migrations::migrate_v79_backfill_done_sort_order(&conn).unwrap();

    let mut stmt = conn
        .prepare("SELECT title, sort_order FROM tasks ORDER BY title")
        .unwrap();
    let task_rows: Vec<(String, Option<i64>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(task_rows[0].0, "done-already-sorted");
    assert_eq!(
        task_rows[0].1,
        Some(-999),
        "an already-set sort_order must not be overwritten"
    );
    assert_eq!(task_rows[1].0, "done-no-sort-order");
    assert!(
        task_rows[1].1.is_some_and(|so| so < 0),
        "a null sort_order on a Done task must be backfilled to a negative value, got {:?}",
        task_rows[1].1
    );
    assert_eq!(task_rows[2].0, "not-done");
    assert_eq!(
        task_rows[2].1, None,
        "a non-Done task's null sort_order must be left alone"
    );

    let mut estmt = conn
        .prepare("SELECT title, sort_order FROM epics ORDER BY title")
        .unwrap();
    let epic_rows: Vec<(String, Option<i64>)> = estmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(epic_rows[0].0, "epic-done-no-sort-order");
    assert!(epic_rows[0].1.is_some_and(|so| so < 0));
    assert_eq!(epic_rows[1].0, "epic-not-done");
    assert_eq!(epic_rows[1].1, None);
}

/// The four scheduling columns land as nullable, so every pre-existing row
/// reads back as "not scheduled, not pinned, never processed, never checked".
#[test]
fn migration_v88_adds_scheduling_fields_to_tasks() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE tasks (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             status TEXT NOT NULL DEFAULT 'backlog'
         );
         INSERT INTO tasks(title) VALUES('pre-existing');",
    )
    .unwrap();

    crate::db::migrations::migrate_v88_add_scheduling_fields(&conn).unwrap();

    let columns: Vec<(String, String, i64)> = conn
        .prepare("SELECT name, type, \"notnull\" FROM pragma_table_info('tasks')")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();

    for (name, ty) in [
        ("schedule_interval_secs", "INTEGER"),
        ("pinned_branch", "TEXT"),
        ("last_processed_sha", "TEXT"),
        ("last_scheduled_check_at", "TEXT"),
    ] {
        let col = columns
            .iter()
            .find(|(n, _, _)| n == name)
            .unwrap_or_else(|| panic!("{name} must be added by migration v88"));
        assert_eq!(col.1, ty, "{name} type");
        assert_eq!(col.2, 0, "{name} must be nullable");
    }

    let row: (Option<i64>, Option<String>, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT schedule_interval_secs, pinned_branch, last_processed_sha, \
             last_scheduled_check_at FROM tasks",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        row,
        (None, None, None, None),
        "existing rows stay unscheduled"
    );
}

#[test]
fn migration_v88_is_idempotent() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE tasks (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             schedule_interval_secs INTEGER,
             pinned_branch TEXT,
             last_processed_sha TEXT,
             last_scheduled_check_at TEXT
         );",
    )
    .unwrap();

    crate::db::migrations::migrate_v88_add_scheduling_fields(&conn).unwrap();
}
