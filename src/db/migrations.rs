use anyhow::{Context, Result};
use rusqlite::{params, Connection};

pub(super) type Migration = (i64, fn(&Connection) -> Result<()>);

pub(super) const MIGRATIONS: &[Migration] = &[
    (1, migrate_v1_add_plan_column),
    (2, migrate_v2_drop_notes_table),
    (3, migrate_v3_create_epics_table),
    (4, migrate_v4_add_needs_input_drop_epic_plan),
    (5, migrate_v5_create_settings_table),
    (6, migrate_v6_rename_ready_to_backlog),
    (7, migrate_v7_add_pr_columns),
    (8, migrate_v8_add_epic_plan),
    (9, migrate_v9_add_sort_order),
    (10, migrate_v10_create_task_usage_table),
    (11, migrate_v11_create_filter_presets_table),
    (12, migrate_v12_drop_pr_number),
    (13, migrate_v13_add_tag),
    (14, migrate_v14_create_review_prs_table),
    (15, migrate_v15_add_sub_status),
    (16, migrate_v16_add_status_check_constraint),
    (17, migrate_v17_add_conflict_sub_status),
    (18, migrate_v18_expand_tilde_paths),
    (19, migrate_v19_add_review_pr_columns),
    (20, migrate_v20_epic_status_enum),
    (21, migrate_v21_create_my_prs_table),
    (22, migrate_v22_add_filter_preset_mode),
    (23, migrate_v23_create_bot_prs_table),
];

fn migrate_v1_add_plan_column(conn: &Connection) -> Result<()> {
    let _ = conn.execute_batch("ALTER TABLE tasks ADD COLUMN plan TEXT");
    Ok(())
}

fn migrate_v2_drop_notes_table(conn: &Connection) -> Result<()> {
    conn.execute_batch("DROP TABLE IF EXISTS notes")
        .context("Failed to drop notes table")
}

fn migrate_v3_create_epics_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS epics (
            id          INTEGER PRIMARY KEY,
            title       TEXT NOT NULL,
            description TEXT NOT NULL,
            plan        TEXT NOT NULL DEFAULT '',
            repo_path   TEXT NOT NULL,
            done        INTEGER NOT NULL DEFAULT 0,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
        )",
    )
    .context("Failed to create epics table")?;

    let _ = conn.execute_batch("ALTER TABLE tasks ADD COLUMN epic_id INTEGER REFERENCES epics(id)");

    Ok(())
}

fn migrate_v4_add_needs_input_drop_epic_plan(conn: &Connection) -> Result<()> {
    let _ =
        conn.execute_batch("ALTER TABLE tasks ADD COLUMN needs_input INTEGER NOT NULL DEFAULT 0");

    // SQLite doesn't support DROP COLUMN before 3.35.0; recreate the table.
    // Disable FK checks so DROP TABLE succeeds when tasks reference epics,
    // and wrap in a transaction for atomicity.
    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
        BEGIN;
        CREATE TABLE epics_new (
            id          INTEGER PRIMARY KEY,
            title       TEXT NOT NULL,
            description TEXT NOT NULL,
            repo_path   TEXT NOT NULL,
            done        INTEGER NOT NULL DEFAULT 0,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );
        INSERT INTO epics_new (id, title, description, repo_path, done, created_at, updated_at)
            SELECT id, title, description, repo_path, done, created_at, updated_at FROM epics;
        DROP TABLE epics;
        ALTER TABLE epics_new RENAME TO epics;
        COMMIT;
        PRAGMA foreign_keys = ON;",
    )
    .context("Failed to migrate epics (drop plan column)")
}

fn migrate_v5_create_settings_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS settings (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )",
    )
    .context("Failed to create settings table")
}

fn migrate_v6_rename_ready_to_backlog(conn: &Connection) -> Result<()> {
    conn.execute_batch("UPDATE tasks SET status = 'backlog' WHERE status = 'ready'")
        .context("Failed to migrate ready tasks to backlog")
}

fn migrate_v7_add_pr_columns(conn: &Connection) -> Result<()> {
    let _ = conn.execute_batch("ALTER TABLE tasks ADD COLUMN pr_url TEXT");
    let _ = conn.execute_batch("ALTER TABLE tasks ADD COLUMN pr_number INTEGER");
    Ok(())
}

fn migrate_v8_add_epic_plan(conn: &Connection) -> Result<()> {
    let _ = conn.execute_batch("ALTER TABLE epics ADD COLUMN plan TEXT");
    Ok(())
}

fn migrate_v9_add_sort_order(conn: &Connection) -> Result<()> {
    let _ = conn.execute_batch("ALTER TABLE tasks ADD COLUMN sort_order INTEGER");
    let _ = conn.execute_batch("ALTER TABLE epics ADD COLUMN sort_order INTEGER");
    Ok(())
}

fn migrate_v10_create_task_usage_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS task_usage (
            task_id            INTEGER PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
            cost_usd           REAL    NOT NULL DEFAULT 0.0,
            input_tokens       INTEGER NOT NULL DEFAULT 0,
            output_tokens      INTEGER NOT NULL DEFAULT 0,
            cache_read_tokens  INTEGER NOT NULL DEFAULT 0,
            cache_write_tokens INTEGER NOT NULL DEFAULT 0,
            updated_at         TEXT    NOT NULL DEFAULT (datetime('now'))
        )",
    )
    .context("Failed to create task_usage table")
}

fn migrate_v11_create_filter_presets_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS filter_presets (
            name       TEXT PRIMARY KEY,
            repo_paths TEXT NOT NULL
        )",
    )
    .context("Failed to create filter_presets table")
}

fn migrate_v12_drop_pr_number(conn: &Connection) -> Result<()> {
    // DROP COLUMN requires SQLite 3.35.0+; bundled libsqlite3-sys satisfies this.
    // Ignore error for fresh DBs where the column was never added.
    let _ = conn.execute_batch("ALTER TABLE tasks DROP COLUMN pr_number");
    Ok(())
}

fn migrate_v13_add_tag(conn: &Connection) -> Result<()> {
    let _ = conn.execute_batch("ALTER TABLE tasks ADD COLUMN tag TEXT");
    Ok(())
}

fn migrate_v14_create_review_prs_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS review_prs (
            repo            TEXT    NOT NULL,
            number          INTEGER NOT NULL,
            title           TEXT    NOT NULL,
            author          TEXT    NOT NULL,
            url             TEXT    NOT NULL,
            is_draft        INTEGER NOT NULL,
            created_at      TEXT    NOT NULL,
            updated_at      TEXT    NOT NULL,
            additions       INTEGER NOT NULL,
            deletions       INTEGER NOT NULL,
            review_decision TEXT    NOT NULL,
            labels          TEXT    NOT NULL,
            body            TEXT    NOT NULL DEFAULT '',
            head_ref        TEXT    NOT NULL DEFAULT '',
            ci_status       TEXT    NOT NULL DEFAULT 'none',
            reviewers       TEXT    NOT NULL DEFAULT '[]',
            PRIMARY KEY (repo, number)
        )",
    )
    .context("Failed to create review_prs table")
}

fn migrate_v15_add_sub_status(conn: &Connection) -> Result<()> {
    let _ =
        conn.execute_batch("ALTER TABLE tasks ADD COLUMN sub_status TEXT NOT NULL DEFAULT 'none'");
    let _ = conn.execute_batch("UPDATE tasks SET sub_status = 'needs_input' WHERE needs_input = 1");
    let _ = conn.execute_batch(
        "UPDATE tasks SET sub_status = 'active' WHERE status = 'running' AND sub_status = 'none'",
    );
    let _ = conn.execute_batch(
        "UPDATE tasks SET sub_status = 'awaiting_review' WHERE status = 'review' AND sub_status = 'none'",
    );
    conn.execute_batch(
        "CREATE TABLE tasks_new (
            id          INTEGER PRIMARY KEY,
            title       TEXT NOT NULL,
            description TEXT NOT NULL,
            repo_path   TEXT NOT NULL,
            status      TEXT NOT NULL DEFAULT 'backlog',
            worktree    TEXT,
            tmux_window TEXT,
            plan        TEXT,
            epic_id     INTEGER REFERENCES epics(id),
            sub_status  TEXT NOT NULL DEFAULT 'none',
            pr_url      TEXT,
            tag         TEXT,
            sort_order  INTEGER,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );
        INSERT INTO tasks_new SELECT id, title, description, repo_path, status, worktree, tmux_window, plan, epic_id, sub_status, pr_url, tag, sort_order, created_at, updated_at FROM tasks;
        DROP TABLE tasks;
        ALTER TABLE tasks_new RENAME TO tasks;",
    )
    .context("Failed to rebuild tasks table for sub_status migration")
}

fn migrate_v16_add_status_check_constraint(conn: &Connection) -> Result<()> {
    // Clean up invalid (status, sub_status) pairs so the CHECK constraint can be added.
    // Before this migration, (review, needs_input) rows could exist from old hook behavior.
    let _ = conn.execute_batch(
        "-- Legacy (review, needs_input) from old HookNotification hook → awaiting_review
         UPDATE tasks SET sub_status = 'awaiting_review'
         WHERE status = 'review' AND sub_status = 'needs_input';

         -- Any other invalid running pairs → active
         UPDATE tasks SET sub_status = 'active'
         WHERE status = 'running'
           AND sub_status NOT IN ('active', 'needs_input', 'stale', 'crashed');

         -- Any other invalid review pairs → awaiting_review
         UPDATE tasks SET sub_status = 'awaiting_review'
         WHERE status = 'review'
           AND sub_status NOT IN ('awaiting_review', 'changes_requested', 'approved');

         -- Any other invalid terminal-status pairs → none
         UPDATE tasks SET sub_status = 'none'
         WHERE status IN ('backlog', 'done', 'archived') AND sub_status != 'none';",
    );

    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         BEGIN;
         CREATE TABLE tasks_new (
             id          INTEGER PRIMARY KEY,
             title       TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path   TEXT NOT NULL,
             status      TEXT NOT NULL DEFAULT 'backlog',
             worktree    TEXT,
             tmux_window TEXT,
             plan        TEXT,
             epic_id     INTEGER REFERENCES epics(id),
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
         INSERT INTO tasks_new
             SELECT id, title, description, repo_path, status, worktree, tmux_window, plan,
                    epic_id, sub_status, pr_url, tag, sort_order, created_at, updated_at
             FROM tasks;
         DROP TABLE tasks;
         ALTER TABLE tasks_new RENAME TO tasks;
         COMMIT;
         PRAGMA foreign_keys = ON;",
    )
    .context("Failed to rebuild tasks table with CHECK constraint")
}

fn migrate_v17_add_conflict_sub_status(conn: &Connection) -> Result<()> {
    // Add 'conflict' as a valid running sub_status. Rebuild table to update the CHECK constraint.
    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         BEGIN;
         CREATE TABLE tasks_new (
             id          INTEGER PRIMARY KEY,
             title       TEXT NOT NULL,
             description TEXT NOT NULL,
             repo_path   TEXT NOT NULL,
             status      TEXT NOT NULL DEFAULT 'backlog',
             worktree    TEXT,
             tmux_window TEXT,
             plan        TEXT,
             epic_id     INTEGER REFERENCES epics(id),
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
         INSERT INTO tasks_new
             SELECT id, title, description, repo_path, status, worktree, tmux_window, plan,
                    epic_id, sub_status, pr_url, tag, sort_order, created_at, updated_at
             FROM tasks;
         DROP TABLE tasks;
         ALTER TABLE tasks_new RENAME TO tasks;
         COMMIT;
         PRAGMA foreign_keys = ON;",
    )
    .context("Failed to rebuild tasks table for migration 17 (add conflict sub_status)")
}

fn migrate_v18_expand_tilde_paths(conn: &Connection) -> Result<()> {
    // Expand ~/... to $HOME/... in all repo_path columns.
    // This prevents filter mismatches between tilde and absolute forms.
    if let Some(home) = std::env::var_os("HOME") {
        let home = home.to_string_lossy();
        let prefix = format!("{home}/");
        conn.execute(
            "UPDATE tasks SET repo_path = ?1 || substr(repo_path, 3) WHERE repo_path LIKE '~/%'",
            params![prefix],
        )
        .context("Failed to expand ~ in tasks.repo_path")?;
        conn.execute(
            "UPDATE epics SET repo_path = ?1 || substr(repo_path, 3) WHERE repo_path LIKE '~/%'",
            params![prefix],
        )
        .context("Failed to expand ~ in epics.repo_path")?;
        conn.execute(
            "UPDATE repo_paths SET path = ?1 || substr(path, 3) WHERE path LIKE '~/%'",
            params![prefix],
        )
        .context("Failed to expand ~ in repo_paths.path")?;
        conn.execute(
            "UPDATE filter_presets SET repo_paths = replace(repo_paths, '~/', ?1) WHERE repo_paths LIKE '%~/%'",
            params![prefix],
        )
        .context("Failed to expand ~ in filter_presets.repo_paths")?;
        conn.execute(
            "UPDATE settings SET value = replace(value, '~/', ?1) WHERE key = 'repo_filter' AND value LIKE '%~/%'",
            params![prefix],
        )
        .context("Failed to expand ~ in settings.repo_filter")?;
    }
    Ok(())
}

fn migrate_v19_add_review_pr_columns(conn: &Connection) -> Result<()> {
    // Fresh DBs already have these from the CREATE TABLE in migration 14,
    // so ignore "duplicate column" errors.
    let _ = conn.execute_batch("ALTER TABLE review_prs ADD COLUMN body TEXT NOT NULL DEFAULT ''");
    let _ =
        conn.execute_batch("ALTER TABLE review_prs ADD COLUMN head_ref TEXT NOT NULL DEFAULT ''");
    let _ = conn
        .execute_batch("ALTER TABLE review_prs ADD COLUMN ci_status TEXT NOT NULL DEFAULT 'none'");
    let _ = conn
        .execute_batch("ALTER TABLE review_prs ADD COLUMN reviewers TEXT NOT NULL DEFAULT '[]'");
    Ok(())
}

fn migrate_v20_epic_status_enum(conn: &Connection) -> Result<()> {
    // Replace epic `done` boolean with `status` enum.
    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         BEGIN;
         CREATE TABLE epics_new (
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
         INSERT INTO epics_new (id, title, description, repo_path, status, plan, sort_order, created_at, updated_at)
             SELECT id, title, description, repo_path,
                    CASE WHEN done = 1 THEN 'done' ELSE 'backlog' END,
                    plan, sort_order, created_at, updated_at
             FROM epics;
         DROP TABLE epics;
         ALTER TABLE epics_new RENAME TO epics;
         COMMIT;
         PRAGMA foreign_keys = ON;",
    )
    .context("Failed to rebuild epics table for migration 20 (status enum)")?;

    // Derive status for non-done epics from their subtasks
    let epics: Vec<(i64, String)> = conn
        .prepare("SELECT id, status FROM epics WHERE status != 'done'")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    for (epic_id, _) in epics {
        let statuses: Vec<String> = conn
            .prepare("SELECT status FROM tasks WHERE epic_id = ?1 AND status != 'archived'")?
            .query_map(params![epic_id], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let new_status = if statuses.is_empty() {
            "backlog"
        } else if statuses.iter().all(|s| s == "done") {
            "done"
        } else if statuses.iter().all(|s| s == "done" || s == "review") {
            "review"
        } else if statuses.iter().any(|s| s == "running") {
            "running"
        } else {
            "backlog"
        };
        conn.execute(
            "UPDATE epics SET status = ?1 WHERE id = ?2",
            params![new_status, epic_id],
        )?;
    }

    Ok(())
}

fn migrate_v21_create_my_prs_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS my_prs (
            repo            TEXT    NOT NULL,
            number          INTEGER NOT NULL,
            title           TEXT    NOT NULL,
            author          TEXT    NOT NULL,
            url             TEXT    NOT NULL,
            is_draft        INTEGER NOT NULL DEFAULT 0,
            created_at      TEXT    NOT NULL,
            updated_at      TEXT    NOT NULL,
            additions       INTEGER NOT NULL DEFAULT 0,
            deletions       INTEGER NOT NULL DEFAULT 0,
            review_decision TEXT    NOT NULL DEFAULT 'ReviewRequired',
            labels          TEXT    NOT NULL DEFAULT '[]',
            body            TEXT    NOT NULL DEFAULT '',
            head_ref        TEXT    NOT NULL DEFAULT '',
            ci_status       TEXT    NOT NULL DEFAULT 'None',
            reviewers       TEXT    NOT NULL DEFAULT '[]',
            PRIMARY KEY (repo, number)
        )",
    )
    .context("Failed to create my_prs table")
}

fn migrate_v22_add_filter_preset_mode(conn: &Connection) -> Result<()> {
    conn.execute_batch("ALTER TABLE filter_presets ADD COLUMN mode TEXT NOT NULL DEFAULT 'include'")
        .context("Failed to add mode column to filter_presets")
}

fn migrate_v23_create_bot_prs_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS bot_prs (
            repo            TEXT    NOT NULL,
            number          INTEGER NOT NULL,
            title           TEXT    NOT NULL,
            author          TEXT    NOT NULL,
            url             TEXT    NOT NULL,
            is_draft        INTEGER NOT NULL DEFAULT 0,
            created_at      TEXT    NOT NULL,
            updated_at      TEXT    NOT NULL,
            additions       INTEGER NOT NULL DEFAULT 0,
            deletions       INTEGER NOT NULL DEFAULT 0,
            review_decision TEXT    NOT NULL DEFAULT 'ReviewRequired',
            labels          TEXT    NOT NULL DEFAULT '[]',
            body            TEXT    NOT NULL DEFAULT '',
            head_ref        TEXT    NOT NULL DEFAULT '',
            ci_status       TEXT    NOT NULL DEFAULT 'None',
            reviewers       TEXT    NOT NULL DEFAULT '[]',
            PRIMARY KEY (repo, number)
        )",
    )
    .context("Failed to create bot_prs table")
}
