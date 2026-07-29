//! Database migrations.
//!
//! ## Convention
//!
//! [`MIGRATIONS`] is the single ordered registry: a list of
//! `(version, fn)` pairs that the migration runner applies in array order.
//! That order is the **source of truth** — the migration functions below it
//! are defined in arbitrary order and grouped only by where they happened to
//! land over time.
//!
//! Rules for editing:
//! - **Append only.** Add new migrations at the end with the next sequential
//!   version number. Never reorder, renumber, or delete an existing entry —
//!   each has run against production databases and the version number is
//!   recorded there.
//! - The `// ── era ──` comments inside `MIGRATIONS` group entries by rough
//!   era/concern for **readability only**. They carry no runtime meaning;
//!   moving an entry across a group boundary would change schema history.
//!
//! ## Squashing policy
//!
//! We do **not** squash migrations. Every entry in `MIGRATIONS` has run in
//! production at some point; squashing would diverge the schema history from
//! the audit trail. Running all N migrations on a fresh install is negligible
//! (sub-millisecond for SQLite). If the number grows beyond ~100, revisit.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

pub(super) type Migration = (i64, fn(&Connection) -> Result<()>);

fn table_exists(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        params![table],
        |r| r.get::<_, i64>(0),
    )
    .unwrap_or(0)
        > 0
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
        params![table, column],
        |r| r.get::<_, i64>(0),
    )
    .unwrap_or(0)
        > 0
}

pub(super) const MIGRATIONS: &[Migration] = &[
    // ── Foundations (v1–v9): initial task/epic/settings schema ──
    (1, migrate_v1_add_plan_column),
    (2, migrate_v2_drop_notes_table),
    (3, migrate_v3_create_epics_table),
    (4, migrate_v4_add_needs_input_drop_epic_plan),
    (5, migrate_v5_create_settings_table),
    (6, migrate_v6_rename_ready_to_backlog),
    (7, migrate_v7_add_pr_columns),
    (8, migrate_v8_add_epic_plan),
    (9, migrate_v9_add_sort_order),
    // ── Usage, filter presets, tags & sub-statuses (v10–v17) ──
    (10, migrate_v10_create_task_usage_table),
    (11, migrate_v11_create_filter_presets_table),
    (12, migrate_v12_drop_pr_number),
    (13, migrate_v13_add_tag),
    (14, migrate_v14_create_review_prs_table),
    (15, migrate_v15_add_sub_status),
    (16, migrate_v16_add_status_check_constraint),
    (17, migrate_v17_add_conflict_sub_status),
    // ── Path normalization, PR/agent tracking & epic status (v18–v31) ──
    (18, migrate_v18_expand_tilde_paths),
    (19, migrate_v19_add_review_pr_columns),
    (20, migrate_v20_epic_status_enum),
    (21, migrate_v21_create_my_prs_table),
    (22, migrate_v22_add_filter_preset_mode),
    (23, migrate_v23_create_bot_prs_table),
    (24, migrate_v24_create_security_alerts_table),
    (25, migrate_v25_rename_plan_to_plan_path),
    (26, migrate_v26_add_agent_columns),
    (27, migrate_v27_add_agent_status),
    (28, migrate_v28_add_my_prs_agent_status),
    (29, migrate_v29_json_filter_presets),
    (30, migrate_v30_allow_conflict_for_review),
    (31, migrate_v31_re_expand_tilde_paths),
    // ── Branches, dispatch, epic nesting, feeds & projects (v32–v39) ──
    (32, migrate_v32_add_base_branch),
    (33, migrate_v33_add_auto_dispatch),
    (34, migrate_v34_add_parent_epic_id),
    (35, migrate_v35_add_self_ref_check),
    (36, migrate_v36_tips_state),
    (37, migrate_v37_pr_workflow_states), // pr_workflow_states created here — superseded by v51 drop
    (38, migrate_v38_feed_epic_columns),
    (39, migrate_v39_add_projects),
    // ── Knowledge base / learnings (v40–v51) ──
    (40, migrate_v40_create_learnings),
    (41, migrate_v41_drop_cost_usd),
    (42, migrate_v42_drop_epic_tag),
    (43, migrate_v43_proposed_to_approved),
    (44, migrate_v44_episodic_to_convention),
    (45, migrate_v45_add_task_labels),
    (46, migrate_v46_learning_source_task_set_null),
    (47, migrate_v47_task_usage_restore_cascade),
    (48, migrate_v48_learning_validation),
    (49, migrate_v49_rename_confirmed_to_upvote),
    (50, migrate_v50_add_hook_timestamps),
    (51, migrate_v51_drop_pr_workflow_states), // drops pr_workflow_states created in v37
    // ── Wrap-up, embeddings, schema cleanup & typed URLs (v52–v64) ──
    (52, migrate_v52_add_verify_command_to_repo_paths),
    (53, migrate_v53_add_wrap_up_mode),
    (54, migrate_v54_add_group_by_repo),
    (55, migrate_v55_add_learning_embedding),
    (56, migrate_v56_drop_task_usage),
    (57, migrate_v57_enforce_epic_project_consistency),
    (58, migrate_v58_reset_intermediate_epic_statuses),
    (59, migrate_v59_create_usage_events),
    (60, migrate_v60_drop_projects),
    (61, migrate_v61_drop_epic_repo_path),
    (62, migrate_v62_drop_unused_verdicts),
    (63, migrate_v63_add_task_indexes),
    (64, migrate_v64_typed_url),
    (65, migrate_v65_add_epic_feed_role),
    (66, migrate_v66_add_pr_learnings_gate),
    (67, migrate_v67_create_todos),
    (68, migrate_v68_add_todo_links),
    (69, migrate_v69_add_epic_origin),
    (70, migrate_v70_add_todo_parent_id),
    (71, migrate_v71_backup_and_dedup_role_subtree_tasks),
    (72, migrate_v72_add_feed_task_subtree_unique_triggers),
    (73, migrate_v73_needs_review_to_approved),
    (74, migrate_v74_drop_learning_verdicts),
    (75, migrate_v75_create_repo_base_branches),
    (
        76,
        migrate_v76_fix_feed_task_subtree_insert_trigger_self_conflict,
    ),
    (77, migrate_v77_add_auto_run_plan),
    (78, migrate_v78_create_task_watchers),
    (79, migrate_v79_backfill_done_sort_order),
];

/// The schema version a fresh database ends up at after all migrations run.
/// Derived from [`MIGRATIONS`] so adding a migration bumps this — and every
/// test that asserts against it — without touching a scattered literal.
#[cfg(test)]
pub(super) const LATEST_SCHEMA_VERSION: i64 = MIGRATIONS[MIGRATIONS.len() - 1].0;

/// Replace the single `pr_url` column with a typed URL: `url` + `url_type`.
/// Backfill classifies existing URLs the same way the old render-time
/// heuristic did. `security_alert` has NO legacy rows — it only arrives via
/// new code paths (and, later, feed-declared types per task #1808).
/// Uses DROP COLUMN (SQLite 3.35+, bundled) so all other columns, indexes
/// (tasks_epic_external_id, idx_tasks_status, idx_tasks_epic_id) and triggers
/// survive untouched — no table rebuild.
pub(super) fn migrate_v64_typed_url(conn: &Connection) -> Result<()> {
    // Guard for idempotency / partial runs.
    if !column_exists(conn, "tasks", "url") {
        conn.execute_batch("ALTER TABLE tasks ADD COLUMN url TEXT")
            .context("v64: add url column")?;
    }
    if !column_exists(conn, "tasks", "url_type") {
        conn.execute_batch("ALTER TABLE tasks ADD COLUMN url_type TEXT")
            .context("v64: add url_type column")?;
    }
    // Some historical pre-v64 schemas (and idempotent re-runs) may not have a
    // `pr_url` column at all; only backfill and drop when it is present.
    if column_exists(conn, "tasks", "pr_url") {
        // The WHERE clause covers exactly the two backfill cases: rows with a
        // legacy `pr_url`, and any partially-migrated row whose `url` is set but
        // lacks a `url_type`. Scoping it this way avoids leaving an inconsistent
        // (url set, url_type NULL) state behind after a mid-UPDATE partial run.
        conn.execute_batch(
            "UPDATE tasks SET
                 url = pr_url,
                 url_type = CASE
                     WHEN pr_url IS NULL           THEN NULL
                     -- LIKE '%/pull/%' / '%/issues/%' do not strip ?query/#fragment (unlike
                     -- UrlType::infer); GitHub URLs never place /pull/ or /issues/ after ? or #,
                     -- so the classification matches in practice.
                     WHEN pr_url LIKE '%/pull/%'   THEN 'pr'
                     WHEN pr_url LIKE '%/issues/%' THEN 'issue'
                     ELSE 'other'
                 END
             WHERE pr_url IS NOT NULL
                OR (url IS NOT NULL AND url_type IS NULL)",
        )
        .context("v64: backfill url/url_type from pr_url")?;
        conn.execute_batch("ALTER TABLE tasks DROP COLUMN pr_url")
            .context("v64: drop pr_url column")?;
    }
    Ok(())
}

pub(super) fn migrate_v62_drop_unused_verdicts(conn: &Connection) -> Result<()> {
    if !table_exists(conn, "learning_verdicts") {
        return Ok(());
    }
    conn.execute_batch("DELETE FROM learning_verdicts WHERE verdict = 'unused'")
        .context("Failed to delete legacy 'unused' learning verdicts")
}

fn migrate_v53_add_wrap_up_mode(conn: &Connection) -> Result<()> {
    conn.execute_batch("ALTER TABLE tasks ADD COLUMN wrap_up_mode TEXT")
        .context("Failed to add wrap_up_mode column")
}

fn migrate_v77_add_auto_run_plan(conn: &Connection) -> Result<()> {
    conn.execute_batch("ALTER TABLE tasks ADD COLUMN auto_run_plan BOOLEAN NOT NULL DEFAULT 0")
        .context("Failed to add auto_run_plan column to tasks")
}

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

fn migrate_v25_rename_plan_to_plan_path(conn: &Connection) -> Result<()> {
    conn.execute_batch("ALTER TABLE tasks RENAME COLUMN plan TO plan_path")
        .context("Failed to rename tasks.plan to plan_path")?;
    conn.execute_batch("ALTER TABLE epics RENAME COLUMN plan TO plan_path")
        .context("Failed to rename epics.plan to plan_path")?;
    Ok(())
}

fn migrate_v26_add_agent_columns(conn: &Connection) -> Result<()> {
    for table in &["review_prs", "my_prs", "bot_prs", "security_alerts"] {
        if let Err(e) =
            conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN tmux_window TEXT"))
        {
            tracing::debug!("ALTER {table} ADD tmux_window (may already exist): {e}");
        }
        if let Err(e) = conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN worktree TEXT"))
        {
            tracing::debug!("ALTER {table} ADD worktree (may already exist): {e}");
        }
    }
    Ok(())
}

fn migrate_v27_add_agent_status(conn: &Connection) -> Result<()> {
    for table in &["review_prs", "bot_prs", "security_alerts"] {
        if let Err(e) =
            conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN agent_status TEXT"))
        {
            tracing::debug!("ALTER {table} ADD agent_status (may already exist): {e}");
        }
    }
    Ok(())
}

fn migrate_v30_allow_conflict_for_review(conn: &Connection) -> Result<()> {
    // Allow 'conflict' sub_status for review tasks (rebase conflicts during wrap_up/finish).
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
             plan_path   TEXT,
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
                 (status = 'review'   AND sub_status IN ('awaiting_review','changes_requested','approved','conflict')) OR
                 (status = 'done'     AND sub_status = 'none') OR
                 (status = 'archived' AND sub_status = 'none')
             )
         );
         INSERT INTO tasks_new
             SELECT id, title, description, repo_path, status, worktree, tmux_window, plan_path,
                    epic_id, sub_status, pr_url, tag, sort_order, created_at, updated_at
             FROM tasks;
         DROP TABLE tasks;
         ALTER TABLE tasks_new RENAME TO tasks;
         COMMIT;
         PRAGMA foreign_keys = ON;",
    )
    .context("Failed to rebuild tasks table for migration 30 (allow conflict for review)")
}

fn migrate_v24_create_security_alerts_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS security_alerts (
            repo              TEXT    NOT NULL,
            number            INTEGER NOT NULL,
            kind              TEXT    NOT NULL,
            severity          TEXT    NOT NULL,
            title             TEXT    NOT NULL,
            package           TEXT,
            vulnerable_range  TEXT,
            fixed_version     TEXT,
            cvss_score        REAL,
            url               TEXT    NOT NULL,
            created_at        TEXT    NOT NULL,
            state             TEXT    NOT NULL,
            description       TEXT    NOT NULL DEFAULT '',
            PRIMARY KEY (repo, number, kind)
        )",
    )
    .context("Failed to create security_alerts table")
}

fn migrate_v28_add_my_prs_agent_status(conn: &Connection) -> Result<()> {
    // v27 missed my_prs when adding agent_status. Fix the gap.
    if let Err(e) = conn.execute_batch("ALTER TABLE my_prs ADD COLUMN agent_status TEXT") {
        tracing::debug!("ALTER my_prs ADD agent_status (may already exist): {e}");
    }
    Ok(())
}

fn migrate_v31_re_expand_tilde_paths(conn: &Connection) -> Result<()> {
    // Re-expand ~/... to $HOME/... in all path columns.
    // Migration v18 did this once, but paths saved between v18 and the
    // expand_tilde-on-write fix (commit fd26d80) may still contain tildes.
    if let Some(home) = std::env::var_os("HOME") {
        let home = home.to_string_lossy();
        let prefix = format!("{home}/");

        // Simple text columns: tasks.repo_path, epics.repo_path, repo_paths.path
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

        // JSON array columns (post v29): filter_presets.repo_paths, settings.repo_filter
        let presets: Vec<(String, String)> = conn
            .prepare("SELECT name, repo_paths FROM filter_presets WHERE repo_paths LIKE '%~/%'")?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        for (name, raw) in presets {
            if let Ok(paths) = serde_json::from_str::<Vec<String>>(&raw) {
                let expanded: Vec<String> = paths
                    .into_iter()
                    .map(|p| {
                        if let Some(rest) = p.strip_prefix("~/") {
                            format!("{prefix}{rest}")
                        } else {
                            p
                        }
                    })
                    .collect();
                let json = serde_json::to_string(&expanded).unwrap_or(raw);
                conn.execute(
                    "UPDATE filter_presets SET repo_paths = ?1 WHERE name = ?2",
                    params![json, name],
                )?;
            }
        }

        let filter: Option<String> = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'repo_filter' AND value LIKE '%~/%'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(raw) = filter {
            if let Ok(paths) = serde_json::from_str::<Vec<String>>(&raw) {
                let expanded: Vec<String> = paths
                    .into_iter()
                    .map(|p| {
                        if let Some(rest) = p.strip_prefix("~/") {
                            format!("{prefix}{rest}")
                        } else {
                            p
                        }
                    })
                    .collect();
                let json = serde_json::to_string(&expanded).unwrap_or(raw);
                conn.execute(
                    "UPDATE settings SET value = ?1 WHERE key = 'repo_filter'",
                    params![json],
                )?;
            }
        }
    }
    Ok(())
}

fn migrate_v32_add_base_branch(conn: &Connection) -> Result<()> {
    conn.execute_batch("ALTER TABLE tasks ADD COLUMN base_branch TEXT NOT NULL DEFAULT 'main';")
        .context("Failed to add base_branch column to tasks")
}

fn migrate_v29_json_filter_presets(conn: &Connection) -> Result<()> {
    // Convert filter_presets.repo_paths from newline-delimited to JSON arrays.
    let rows: Vec<(String, String)> = conn
        .prepare("SELECT name, repo_paths FROM filter_presets")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    for (name, raw) in rows {
        // Skip values that are already valid JSON arrays
        if raw.starts_with('[') {
            continue;
        }
        let paths: Vec<&str> = raw.split('\n').filter(|s| !s.is_empty()).collect();
        let json = serde_json::to_string(&paths).unwrap_or_else(|_| "[]".to_string());
        conn.execute(
            "UPDATE filter_presets SET repo_paths = ?1 WHERE name = ?2",
            params![json, name],
        )?;
    }

    // Convert settings.repo_filter from newline-delimited to JSON array.
    let filter: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'repo_filter'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(raw) = filter {
        if !raw.starts_with('[') {
            let paths: Vec<&str> = raw.split('\n').filter(|s| !s.is_empty()).collect();
            let json = serde_json::to_string(&paths).unwrap_or_else(|_| "[]".to_string());
            conn.execute(
                "UPDATE settings SET value = ?1 WHERE key = 'repo_filter'",
                params![json],
            )?;
        }
    }

    Ok(())
}

pub(super) fn migrate_v33_add_auto_dispatch(conn: &Connection) -> Result<()> {
    conn.execute_batch("ALTER TABLE epics ADD COLUMN auto_dispatch BOOLEAN NOT NULL DEFAULT 1;")
        .context("Failed to add auto_dispatch column to epics")
}

fn migrate_v34_add_parent_epic_id(conn: &Connection) -> Result<()> {
    conn.execute_batch("ALTER TABLE epics ADD COLUMN parent_epic_id INTEGER REFERENCES epics(id);")
        .context("Failed to add parent_epic_id column to epics")
}

fn migrate_v35_add_self_ref_check(conn: &Connection) -> Result<()> {
    // SQLite does not support ADD CONSTRAINT, so we rebuild the epics table to
    // add CHECK (parent_epic_id != id), which prevents a row from being its
    // own parent. This is defense-in-depth alongside the visited-set guard in
    // recalculate_epic_status_inner.
    //
    // Column order matches the post-v34 layout produced by ALTER TABLE additions:
    //   id, title, description, repo_path, status, plan_path, sort_order,
    //   created_at, updated_at, auto_dispatch, parent_epic_id
    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         BEGIN;
         CREATE TABLE epics_new (
             id             INTEGER PRIMARY KEY,
             title          TEXT NOT NULL,
             description    TEXT NOT NULL,
             repo_path      TEXT NOT NULL,
             status         TEXT NOT NULL DEFAULT 'backlog',
             plan_path      TEXT,
             sort_order     INTEGER,
             created_at     TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at     TEXT NOT NULL DEFAULT (datetime('now')),
             auto_dispatch  BOOLEAN NOT NULL DEFAULT 1,
             parent_epic_id INTEGER REFERENCES epics_new(id),
             CHECK (parent_epic_id != id)
         );
         INSERT INTO epics_new (
             id, title, description, repo_path, status, plan_path, sort_order,
             created_at, updated_at, auto_dispatch, parent_epic_id
         )
         SELECT
             id, title, description, repo_path, status, plan_path, sort_order,
             created_at, updated_at, auto_dispatch, parent_epic_id
         FROM epics;
         DROP TABLE epics;
         ALTER TABLE epics_new RENAME TO epics;
         COMMIT;
         PRAGMA foreign_keys = ON;",
    )
    .context("Failed to rebuild epics table for migration v35 (self-ref CHECK)")
}

fn migrate_v37_pr_workflow_states(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE pr_workflow_states (
            repo       TEXT    NOT NULL,
            number     INTEGER NOT NULL,
            kind       TEXT    NOT NULL,
            state      TEXT    NOT NULL DEFAULT 'backlog',
            sub_state  TEXT,
            updated_at TEXT    NOT NULL,
            PRIMARY KEY (repo, number, kind)
        );",
    )
    .context("Failed to create pr_workflow_states table (migration v37)")
}

fn migrate_v38_feed_epic_columns(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "ALTER TABLE epics ADD COLUMN feed_command TEXT;
         ALTER TABLE epics ADD COLUMN feed_interval_secs INTEGER;
         ALTER TABLE tasks ADD COLUMN external_id TEXT;
         CREATE UNIQUE INDEX IF NOT EXISTS tasks_epic_external_id
             ON tasks (epic_id, external_id)
             WHERE external_id IS NOT NULL;",
    )
    .context("Failed to add feed columns (migration v38)")
}

fn migrate_v39_add_projects(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "BEGIN;
        CREATE TABLE IF NOT EXISTS projects (
            id         INTEGER PRIMARY KEY,
            name       TEXT NOT NULL,
            sort_order INTEGER NOT NULL DEFAULT 0,
            is_default INTEGER NOT NULL DEFAULT 0
        );
        INSERT INTO projects (name, sort_order, is_default) VALUES ('Default', 0, 1);
        ALTER TABLE tasks ADD COLUMN project_id INTEGER NOT NULL DEFAULT 1;
        ALTER TABLE epics ADD COLUMN project_id INTEGER NOT NULL DEFAULT 1;
        CREATE INDEX idx_tasks_project_id ON tasks(project_id);
        CREATE INDEX idx_epics_project_id ON epics(project_id);
        COMMIT;",
    )
    .context("Failed to add projects table (migration v39)")
}

fn migrate_v36_tips_state(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tips_state (
            id         INTEGER PRIMARY KEY DEFAULT 1,
            seen_up_to INTEGER NOT NULL DEFAULT 0,
            show_mode  TEXT    NOT NULL DEFAULT 'always',
            CHECK (id = 1)
        );
        INSERT OR IGNORE INTO tips_state (id, seen_up_to, show_mode)
        VALUES (1, 0, 'always');",
    )
    .context("Failed to create tips_state table (migration v36)")
}

fn migrate_v40_create_learnings(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS learnings (
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
        CREATE INDEX IF NOT EXISTS idx_learnings_status ON learnings(status);",
    )
    .context("Failed to create learnings table (migration v40)")
}

fn migrate_v41_drop_cost_usd(conn: &Connection) -> Result<()> {
    if !column_exists(conn, "task_usage", "cost_usd") {
        return Ok(());
    }
    conn.execute_batch(
        "CREATE TABLE task_usage_new (
            task_id            INTEGER NOT NULL PRIMARY KEY REFERENCES tasks(id),
            input_tokens       INTEGER NOT NULL DEFAULT 0,
            output_tokens      INTEGER NOT NULL DEFAULT 0,
            cache_read_tokens  INTEGER NOT NULL DEFAULT 0,
            cache_write_tokens INTEGER NOT NULL DEFAULT 0,
            updated_at         TEXT NOT NULL DEFAULT ''
        );
        INSERT INTO task_usage_new
            SELECT task_id, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, updated_at
            FROM task_usage;
        DROP TABLE task_usage;
        ALTER TABLE task_usage_new RENAME TO task_usage;",
    )
    .context("Failed to drop cost_usd from task_usage (migration v41)")
}

pub(super) fn migrate_v73_needs_review_to_approved(conn: &Connection) -> Result<()> {
    // The needs_review learning status was removed (human-approval gate dropped).
    // Convert any surviving rows back to 'approved' so LearningStatus::parse,
    // which now hard-errors on the unknown 'needs_review' string (storage-boundary
    // policy in core.allium), never encounters one.
    //
    // Skip gracefully if the learnings table doesn't exist (minimal-schema
    // migration tests that never ran v40).
    if !table_exists(conn, "learnings") {
        return Ok(());
    }
    let promoted = conn
        .execute(
            "UPDATE learnings SET status = 'approved', updated_at = datetime('now') WHERE status = 'needs_review'",
            [],
        )
        .context("Failed to convert needs_review learnings to approved (migration v73)")?;
    if promoted > 0 {
        tracing::info!("Migration v73: promoted {promoted} needs_review learning(s) to approved");
    }
    Ok(())
}

pub(super) fn migrate_v74_drop_learning_verdicts(conn: &Connection) -> Result<()> {
    // Learning verdicts are no longer persisted: rate_learning applies only the
    // in-flight score effect (helped → +1, wrong → -1). Drop the vestigial
    // learning_verdicts table (and its index) that apply_verdicts_tx used to
    // write to. Guarded so minimal-schema migration tests that never created it
    // still pass. See docs/specs/learnings.allium (Retrievals & Verdicts).
    conn.execute_batch(
        "DROP INDEX IF EXISTS idx_lv_learning;
         DROP TABLE IF EXISTS learning_verdicts;",
    )
    .context("Failed to drop learning_verdicts table (migration v74)")?;
    Ok(())
}

/// Create the `repo_base_branches` table: per-repo base_branch history (see
/// docs/specs/dispatch.allium: rule RecordBaseBranch, surface BaseBranchPicker;
/// docs/specs/core.allium: entity SavedRepoBranch). Identity is the
/// `(repo_path, branch)` pair, enforced by a UNIQUE constraint so the
/// production upsert can `ON CONFLICT(repo_path, branch)`.
pub(super) fn migrate_v75_create_repo_base_branches(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS repo_base_branches (
            id        INTEGER PRIMARY KEY,
            repo_path TEXT NOT NULL,
            branch    TEXT NOT NULL,
            last_used TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(repo_path, branch)
        );",
    )
    .context("Failed to create repo_base_branches table (migration v75)")?;
    Ok(())
}

/// Creates `task_watchers`: one-shot subscriptions where `watcher_task_id`
/// wants to be notified when `target_task_id` reaches `Done`/`Archived`, or
/// is deleted before finishing. See `docs/specs/task-watchers.allium`.
pub(super) fn migrate_v78_create_task_watchers(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS task_watchers (
            id              INTEGER PRIMARY KEY,
            watcher_task_id INTEGER NOT NULL,
            target_task_id  INTEGER NOT NULL,
            created_at      TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(watcher_task_id, target_task_id)
        );",
    )
    .context("v78: failed to create task_watchers table")?;
    Ok(())
}

/// Backfills `sort_order` for tasks and epics already sitting in `Done`
/// status, using their existing `updated_at` as an approximation of
/// completion time (no real completion timestamp exists for historical
/// data — see the Done-column completion-order design doc). Only fills in
/// `NULL` values; never overwrites an already-set `sort_order` (e.g. one
/// set by a prior manual reorder). Going forward, live transitions use
/// millisecond precision (`sort_order_for_status_transition`); this
/// backfill is deliberately seconds-scale, matching `updated_at`'s storage
/// precision — see the design doc for why mixing scales here is correct,
/// not a bug.
pub(super) fn migrate_v79_backfill_done_sort_order(conn: &Connection) -> Result<()> {
    let tasks_updated = conn
        .execute(
            "UPDATE tasks SET sort_order = -CAST(strftime('%s', updated_at) AS INTEGER)
             WHERE status = 'done' AND sort_order IS NULL",
            [],
        )
        .context("Failed to backfill sort_order for Done tasks (migration v79)")?;
    if tasks_updated > 0 {
        tracing::info!("Migration v79: backfilled sort_order for {tasks_updated} Done task(s)");
    }

    let epics_updated = conn
        .execute(
            "UPDATE epics SET sort_order = -CAST(strftime('%s', updated_at) AS INTEGER)
             WHERE status = 'done' AND sort_order IS NULL",
            [],
        )
        .context("Failed to backfill sort_order for Done epics (migration v79)")?;
    if epics_updated > 0 {
        tracing::info!("Migration v79: backfilled sort_order for {epics_updated} Done epic(s)");
    }

    Ok(())
}

pub(super) fn migrate_v42_drop_epic_tag(conn: &Connection) -> Result<()> {
    // Some migration tests build a pre-v13 schema with no `tag` column.
    if !column_exists(conn, "tasks", "tag") {
        return Ok(());
    }
    let cleared = conn
        .execute("UPDATE tasks SET tag = NULL WHERE tag = 'epic'", [])
        .context("Failed to drop epic tag from tasks (migration v42)")?;
    if cleared > 0 {
        tracing::info!("Migration v42: cleared `epic` tag on {cleared} task(s)");
    }
    Ok(())
}

pub(super) fn migrate_v43_proposed_to_approved(conn: &Connection) -> Result<()> {
    // Change the default status for new learnings from 'proposed' to 'approved'
    // and promote all existing 'proposed' learnings to 'approved'.
    //
    // If the learnings table doesn't exist (e.g. in tests that build minimal
    // schemas without running v40), skip gracefully — there's nothing to migrate.
    if !table_exists(conn, "learnings") {
        return Ok(());
    }

    conn.execute_batch(
        "CREATE TABLE learnings_new (
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
        INSERT INTO learnings_new
            SELECT id, kind, summary, detail, scope, scope_ref, tags,
                CASE WHEN status = 'proposed' THEN 'approved' ELSE status END,
                source_task_id, confirmed_count, last_confirmed_at, created_at, updated_at
            FROM learnings;
        DROP TABLE learnings;
        ALTER TABLE learnings_new RENAME TO learnings;
        CREATE INDEX IF NOT EXISTS idx_learnings_scope ON learnings(scope, scope_ref);
        CREATE INDEX IF NOT EXISTS idx_learnings_status ON learnings(status);",
    )
    .context("Failed to migrate learnings to default 'approved' status (migration v43)")
}

pub(super) fn migrate_v44_episodic_to_convention(conn: &Connection) -> Result<()> {
    conn.execute_batch("UPDATE learnings SET kind = 'convention' WHERE kind = 'episodic'")
        .context("Failed to migrate episodic learnings to convention (migration v44)")
}

pub(super) fn migrate_v45_add_task_labels(conn: &Connection) -> Result<()> {
    if !column_exists(conn, "tasks", "labels") {
        conn.execute_batch("ALTER TABLE tasks ADD COLUMN labels TEXT NOT NULL DEFAULT '[]'")
            .context("Failed to add labels column to tasks (migration v45)")?;
    }
    Ok(())
}

pub(super) fn migrate_v46_learning_source_task_set_null(conn: &Connection) -> Result<()> {
    // learnings.source_task_id originally referenced tasks(id) without an
    // ON DELETE action. That blocked feed-task purges (UpsertFeedTasks)
    // whenever a feed-derived task had been the source of any learning.
    // Rebuild the table with ON DELETE SET NULL so a learning outlives its
    // source task as orphaned provenance rather than blocking the delete.

    if !table_exists(conn, "learnings") {
        return Ok(());
    }

    conn.execute_batch(
        "CREATE TABLE learnings_new (
            id                INTEGER PRIMARY KEY,
            kind              TEXT    NOT NULL,
            summary           TEXT    NOT NULL,
            detail            TEXT,
            scope             TEXT    NOT NULL,
            scope_ref         TEXT,
            tags              TEXT    NOT NULL DEFAULT '[]',
            status            TEXT    NOT NULL DEFAULT 'approved',
            source_task_id    INTEGER REFERENCES tasks(id) ON DELETE SET NULL,
            confirmed_count   INTEGER NOT NULL DEFAULT 0,
            last_confirmed_at TEXT,
            created_at        TEXT    NOT NULL DEFAULT (datetime('now')),
            updated_at        TEXT    NOT NULL DEFAULT (datetime('now')),
            CHECK (
                (scope = 'user' AND scope_ref IS NULL)
                OR (scope != 'user' AND scope_ref IS NOT NULL)
            )
        );
        INSERT INTO learnings_new
            SELECT id, kind, summary, detail, scope, scope_ref, tags, status,
                source_task_id, confirmed_count, last_confirmed_at, created_at, updated_at
            FROM learnings;
        DROP TABLE learnings;
        ALTER TABLE learnings_new RENAME TO learnings;
        CREATE INDEX IF NOT EXISTS idx_learnings_scope ON learnings(scope, scope_ref);
        CREATE INDEX IF NOT EXISTS idx_learnings_status ON learnings(status);",
    )
    .context("Failed to rebuild learnings with ON DELETE SET NULL (migration v46)")
}

pub(super) fn migrate_v47_task_usage_restore_cascade(conn: &Connection) -> Result<()> {
    // Migration v41 recreated task_usage to drop cost_usd but accidentally
    // dropped the ON DELETE CASCADE clause that was on the original FK in
    // v10. That broke feed-task purges (UpsertFeedTasks) for any task that
    // had usage reported. Recreate the table with the cascade restored.

    if !table_exists(conn, "task_usage") {
        return Ok(());
    }

    conn.execute_batch(
        "CREATE TABLE task_usage_new (
            task_id            INTEGER NOT NULL PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
            input_tokens       INTEGER NOT NULL DEFAULT 0,
            output_tokens      INTEGER NOT NULL DEFAULT 0,
            cache_read_tokens  INTEGER NOT NULL DEFAULT 0,
            cache_write_tokens INTEGER NOT NULL DEFAULT 0,
            updated_at         TEXT NOT NULL DEFAULT ''
        );
        INSERT INTO task_usage_new
            SELECT task_id, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, updated_at
            FROM task_usage;
        DROP TABLE task_usage;
        ALTER TABLE task_usage_new RENAME TO task_usage;",
    )
    .context("Failed to restore ON DELETE CASCADE on task_usage (migration v47)")
}

fn migrate_v48_learning_validation(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE learning_retrievals (
            id           INTEGER PRIMARY KEY,
            task_id      INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
            learning_id  INTEGER NOT NULL REFERENCES learnings(id) ON DELETE CASCADE,
            source       TEXT    NOT NULL,
            retrieved_at TEXT    NOT NULL DEFAULT (datetime('now'))
         );
         CREATE INDEX idx_lr_task ON learning_retrievals(task_id);
         CREATE INDEX idx_lr_learning ON learning_retrievals(learning_id);

         CREATE TABLE learning_verdicts (
            id           INTEGER PRIMARY KEY,
            task_id      INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
            learning_id  INTEGER NOT NULL REFERENCES learnings(id) ON DELETE CASCADE,
            verdict      TEXT    NOT NULL,
            recorded_at  TEXT    NOT NULL DEFAULT (datetime('now'))
         );
         CREATE INDEX idx_lv_learning ON learning_verdicts(learning_id);",
    )
    .context("Failed to create learning validation tables (migration v48)")
}

pub(super) fn migrate_v49_rename_confirmed_to_upvote(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "ALTER TABLE learnings RENAME COLUMN confirmed_count TO upvote_count;
         ALTER TABLE learnings RENAME COLUMN last_confirmed_at TO last_upvoted_at;",
    )
    .context("Failed to rename confirmed_* columns to upvote_* on learnings (migration v49)")
}

fn migrate_v50_add_hook_timestamps(conn: &Connection) -> Result<()> {
    if !column_exists(conn, "tasks", "last_pre_tool_use_at") {
        conn.execute("ALTER TABLE tasks ADD COLUMN last_pre_tool_use_at TEXT", [])
            .context("migration v50: add last_pre_tool_use_at")?;
    }
    if !column_exists(conn, "tasks", "last_notification_at") {
        conn.execute("ALTER TABLE tasks ADD COLUMN last_notification_at TEXT", [])
            .context("migration v50: add last_notification_at")?;
    }
    Ok(())
}

fn migrate_v51_drop_pr_workflow_states(conn: &Connection) -> Result<()> {
    conn.execute_batch("DROP TABLE IF EXISTS pr_workflow_states;")
        .context("Failed to drop pr_workflow_states table (migration v51)")
}

pub(super) fn migrate_v52_add_verify_command_to_repo_paths(conn: &Connection) -> Result<()> {
    if !column_exists(conn, "repo_paths", "verify_command") {
        conn.execute_batch("ALTER TABLE repo_paths ADD COLUMN verify_command TEXT")
            .context("v52: add verify_command column")?;
    }
    Ok(())
}

/// Add the `feed_role` column to `epics` plus a partial unique index so a
/// feed parent can hold at most one sub-epic per non-`none` role. The column
/// stores the kebab-case `FeedRole` string (defaults to `'none'`). The index
/// is partial (`WHERE feed_role <> 'none'`) so ordinary epics — all sharing
/// `'none'` — are not constrained.
fn migrate_v65_add_epic_feed_role(conn: &Connection) -> Result<()> {
    // Some migration tests build minimal schemas without an epics table.
    if !table_exists(conn, "epics") {
        return Ok(());
    }
    if !column_exists(conn, "epics", "feed_role") {
        conn.execute_batch("ALTER TABLE epics ADD COLUMN feed_role TEXT NOT NULL DEFAULT 'none';")
            .context("Failed to add feed_role column to epics (migration v65)")?;
    }
    // The index references parent_epic_id; some migration tests build minimal
    // epics schemas without it, so guard on the column existing (learning #110).
    if column_exists(conn, "epics", "parent_epic_id") {
        conn.execute_batch(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_epics_parent_feed_role
                 ON epics(parent_epic_id, feed_role)
                 WHERE feed_role <> 'none';",
        )
        .context("Failed to add feed_role unique index (migration v65)")?;
    }
    Ok(())
}

fn migrate_v54_add_group_by_repo(conn: &Connection) -> Result<()> {
    if !column_exists(conn, "epics", "group_by_repo") {
        conn.execute_batch(
            "ALTER TABLE epics ADD COLUMN group_by_repo BOOLEAN NOT NULL DEFAULT 0;",
        )
        .context("Failed to add group_by_repo column to epics")?;
    }
    Ok(())
}

pub(super) fn migrate_v55_add_learning_embedding(conn: &Connection) -> Result<()> {
    // Skip if learnings table doesn't exist (e.g. in tests with minimal schemas)
    if !table_exists(conn, "learnings") {
        return Ok(());
    }

    conn.execute_batch("ALTER TABLE learnings ADD COLUMN embedding BLOB;")
        .context("Failed to add embedding column to learnings")
}

fn migrate_v56_drop_task_usage(conn: &Connection) -> Result<()> {
    conn.execute_batch("DROP TABLE IF EXISTS task_usage;")
        .context("Failed to drop task_usage table (migration v56)")
}

pub(super) fn migrate_v57_enforce_epic_project_consistency(conn: &Connection) -> Result<()> {
    let epics_has_parent = column_exists(conn, "epics", "parent_epic_id");
    let epics_has_project = column_exists(conn, "epics", "project_id");
    let tasks_has_epic = column_exists(conn, "tasks", "epic_id");
    let tasks_has_project = column_exists(conn, "tasks", "project_id");

    if epics_has_parent && epics_has_project {
        loop {
            let fixed = conn
                .execute(
                    "UPDATE epics
                     SET project_id = (SELECT p.project_id FROM epics p WHERE p.id = epics.parent_epic_id)
                     WHERE parent_epic_id IS NOT NULL
                       AND project_id != (SELECT p.project_id FROM epics p WHERE p.id = epics.parent_epic_id)",
                    [],
                )
                .context("Failed to fix sub-epic project_id violations")?;
            if fixed == 0 {
                break;
            }
        }
        conn.execute_batch(
            "CREATE TRIGGER IF NOT EXISTS enforce_sub_epic_project_insert
             BEFORE INSERT ON epics
             WHEN NEW.parent_epic_id IS NOT NULL
             BEGIN
               SELECT RAISE(ABORT, 'sub-epic project_id must match parent epic project_id')
               WHERE (SELECT project_id FROM epics WHERE id = NEW.parent_epic_id) != NEW.project_id;
             END;

             CREATE TRIGGER IF NOT EXISTS enforce_sub_epic_project_update
             BEFORE UPDATE ON epics
             WHEN NEW.parent_epic_id IS NOT NULL
             BEGIN
               SELECT RAISE(ABORT, 'sub-epic project_id must match parent epic project_id')
               WHERE (SELECT project_id FROM epics WHERE id = NEW.parent_epic_id) != NEW.project_id;
             END;",
        )
        .context("Failed to create sub-epic project consistency triggers")?;
    }

    if tasks_has_epic && tasks_has_project && epics_has_project {
        conn.execute(
            "UPDATE tasks
             SET project_id = (SELECT e.project_id FROM epics e WHERE e.id = tasks.epic_id)
             WHERE epic_id IS NOT NULL
               AND project_id != (SELECT e.project_id FROM epics e WHERE e.id = tasks.epic_id)",
            [],
        )
        .context("Failed to fix task project_id violations")?;
        conn.execute_batch(
            "CREATE TRIGGER IF NOT EXISTS enforce_task_epic_project_insert
             BEFORE INSERT ON tasks
             WHEN NEW.epic_id IS NOT NULL
             BEGIN
               SELECT RAISE(ABORT, 'task project_id must match epic project_id')
               WHERE (SELECT project_id FROM epics WHERE id = NEW.epic_id) != NEW.project_id;
             END;

             CREATE TRIGGER IF NOT EXISTS enforce_task_epic_project_update
             BEFORE UPDATE ON tasks
             WHEN NEW.epic_id IS NOT NULL
             BEGIN
               SELECT RAISE(ABORT, 'task project_id must match epic project_id')
               WHERE (SELECT project_id FROM epics WHERE id = NEW.epic_id) != NEW.project_id;
             END;",
        )
        .context("Failed to create task epic project consistency triggers")?;
    }

    Ok(())
}

pub(super) fn migrate_v58_reset_intermediate_epic_statuses(conn: &Connection) -> Result<()> {
    // Guard for migration tests that run against minimal schemas without `epics.status`.
    if column_exists(conn, "epics", "status") {
        conn.execute(
            "UPDATE epics SET status = 'backlog', updated_at = datetime('now') WHERE status IN ('running', 'review')",
            [],
        )
        .context("Failed to reset intermediate epic statuses to backlog (migration v58)")?;
    }
    Ok(())
}

fn migrate_v59_create_usage_events(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS usage_events (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            recorded_at TEXT    NOT NULL DEFAULT (datetime('now')),
            category    TEXT    NOT NULL,
            action      TEXT    NOT NULL,
            detail      TEXT,
            actor       TEXT    NOT NULL
        )",
    )
    .context("Failed to create usage_events table")
}

pub(super) fn migrate_v60_drop_projects(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "DROP TRIGGER IF EXISTS enforce_sub_epic_project_insert;
         DROP TRIGGER IF EXISTS enforce_sub_epic_project_update;
         DROP TRIGGER IF EXISTS enforce_task_epic_project_insert;
         DROP TRIGGER IF EXISTS enforce_task_epic_project_update;
         DROP INDEX IF EXISTS idx_tasks_project_id;
         DROP INDEX IF EXISTS idx_epics_project_id;",
    )
    .context("Failed to drop project triggers and indexes (migration v60)")?;
    if column_exists(conn, "tasks", "project_id") {
        conn.execute_batch("ALTER TABLE tasks DROP COLUMN project_id")
            .context("Failed to drop project_id from tasks (migration v60)")?;
    }
    if column_exists(conn, "epics", "project_id") {
        conn.execute_batch("ALTER TABLE epics DROP COLUMN project_id")
            .context("Failed to drop project_id from epics (migration v60)")?;
    }
    conn.execute_batch("DROP TABLE IF EXISTS projects")
        .context("Failed to drop projects table (migration v60)")?;
    // Promote the first project-namespaced repo_filter settings key (e.g.
    // "repo_filter:1") to the new unnamespaced key. INSERT OR IGNORE ensures we
    // don't overwrite a key that was already written unnamespaced.
    if table_exists(conn, "settings") {
        conn.execute_batch(
            "INSERT OR IGNORE INTO settings (key, value)
                 SELECT 'repo_filter', value FROM settings
                 WHERE key LIKE 'repo_filter:%' ORDER BY key LIMIT 1;
             DELETE FROM settings WHERE key LIKE 'repo_filter:%';
             INSERT OR IGNORE INTO settings (key, value)
                 SELECT 'repo_filter_mode', value FROM settings
                 WHERE key LIKE 'repo_filter_mode:%' ORDER BY key LIMIT 1;
             DELETE FROM settings WHERE key LIKE 'repo_filter_mode:%';",
        )
        .context("Failed to migrate repo_filter settings keys (migration v60)")?;
    }
    // Drop project-scoped learnings — LearningScope::Project no longer exists
    // and any rows with scope='project' would cause deserialization errors.
    if table_exists(conn, "learnings") {
        conn.execute_batch("DELETE FROM learnings WHERE scope = 'project'")
            .context("Failed to delete project-scoped learnings (migration v60)")?;
    }
    Ok(())
}

fn migrate_v63_add_task_indexes(conn: &Connection) -> Result<()> {
    if !table_exists(conn, "tasks") {
        return Ok(());
    }
    if column_exists(conn, "tasks", "status") {
        conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status);")
            .context("Failed to add idx_tasks_status (migration v63)")?;
    }
    if column_exists(conn, "tasks", "epic_id") {
        conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_tasks_epic_id ON tasks(epic_id);")
            .context("Failed to add idx_tasks_epic_id (migration v63)")?;
    }
    Ok(())
}

fn migrate_v61_drop_epic_repo_path(conn: &Connection) -> Result<()> {
    if column_exists(conn, "epics", "repo_path") {
        conn.execute_batch("ALTER TABLE epics DROP COLUMN repo_path")
            .context("Failed to drop repo_path from epics (migration v61)")?;
    }
    Ok(())
}

fn migrate_v66_add_pr_learnings_gate(conn: &Connection) -> Result<()> {
    if !column_exists(conn, "tasks", "pr_learnings_gate_shown_at") {
        conn.execute(
            "ALTER TABLE tasks ADD COLUMN pr_learnings_gate_shown_at TEXT",
            [],
        )
        .context("migration v66: add pr_learnings_gate_shown_at")?;
    }
    Ok(())
}

fn migrate_v67_create_todos(conn: &Connection) -> Result<()> {
    if !table_exists(conn, "todos") {
        conn.execute_batch(
            "CREATE TABLE todos (
                id          INTEGER PRIMARY KEY,
                title       TEXT NOT NULL,
                done        INTEGER NOT NULL DEFAULT 0,
                sort_order  INTEGER NOT NULL DEFAULT 0,
                created_at  TEXT NOT NULL DEFAULT (datetime('now'))
            )",
        )
        .context("Failed to create todos table (migration v67)")?;
    }
    Ok(())
}

fn migrate_v70_add_todo_parent_id(conn: &Connection) -> Result<()> {
    if !column_exists(conn, "todos", "parent_id") {
        conn.execute_batch(
            "ALTER TABLE todos ADD COLUMN parent_id INTEGER REFERENCES todos(id) ON DELETE SET NULL;",
        )
        .context("v69: add parent_id to todos")?;
    }
    Ok(())
}

fn migrate_v68_add_todo_links(conn: &Connection) -> Result<()> {
    // The "at most one of task_id/epic_id is non-null" invariant is enforced
    // at the service layer (via TodoLink enum exhaustive match) rather than at
    // the schema level. SQLite ALTER TABLE ADD COLUMN does not support table-level
    // CHECK constraints; adding one would require a full table rebuild in a future
    // migration if deemed necessary.
    if !column_exists(conn, "todos", "task_id") {
        conn.execute_batch(
            "ALTER TABLE todos ADD COLUMN task_id INTEGER REFERENCES tasks(id) ON DELETE SET NULL;
             ALTER TABLE todos ADD COLUMN epic_id INTEGER REFERENCES epics(id) ON DELETE SET NULL;",
        )
        .context("v68: add task_id and epic_id to todos")?;
    }
    Ok(())
}

fn migrate_v69_add_epic_origin(conn: &Connection) -> Result<()> {
    // Minimal-schema migration tests may lack an epics table.
    if !table_exists(conn, "epics") {
        return Ok(());
    }
    if !column_exists(conn, "epics", "origin") {
        conn.execute_batch("ALTER TABLE epics ADD COLUMN origin TEXT NOT NULL DEFAULT 'manual';")
            .context("Failed to add origin column to epics (migration v69)")?;
    }
    // Guard on title + parent_epic_id (minimal schemas may omit parent_epic_id).
    if column_exists(conn, "epics", "parent_epic_id") {
        conn.execute_batch(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_epics_parent_origin_repo_group
                 ON epics(parent_epic_id, title)
                 WHERE origin = 'repo-group';",
        )
        .context("Failed to add origin unique index (migration v69)")?;
    }
    Ok(())
}

/// Back up the database, then remove duplicate feed tasks from role sub-epic
/// subtrees. A "duplicate" is a task in a repo-group sub-epic whose
/// `external_id` also appears directly on the parent role sub-epic. The copy
/// on the role sub-epic is kept; the repo-group copy is deleted.
///
/// The backup uses `VACUUM INTO` so it is atomic and consistent. Skipped for
/// in-memory databases (empty path string from `PRAGMA database_list`).
pub(super) fn migrate_v71_backup_and_dedup_role_subtree_tasks(conn: &Connection) -> Result<()> {
    // Backup (skip for in-memory / :memory: databases).
    let db_path: String = conn
        .query_row(
            "SELECT file FROM pragma_database_list WHERE name = 'main'",
            [],
            |row| row.get(0),
        )
        .context("v71: failed to read database path")?;

    if !db_path.is_empty() {
        let backup_path = format!("{}.bak.v71", db_path);
        if !std::path::Path::new(&backup_path).exists() {
            let escaped = backup_path.replace('\'', "''");
            conn.execute_batch(&format!("VACUUM INTO '{}'", escaped))
                .context("v71: failed to create database backup")?;
        }
    }

    // Remove repo-group-sub-epic task copies that are shadowed by a copy
    // on the parent role sub-epic (matched by external_id).
    // Guard: skip if required columns are absent (minimal-schema migration tests).
    let has_origin = column_exists(conn, "epics", "origin");
    let has_feed_role = column_exists(conn, "epics", "feed_role");
    let has_parent_epic_id = column_exists(conn, "epics", "parent_epic_id");
    let has_external_id = column_exists(conn, "tasks", "external_id");
    if has_origin && has_feed_role && has_parent_epic_id && has_external_id {
        conn.execute_batch(
            "DELETE FROM tasks WHERE id IN (
                SELECT t.id
                FROM tasks t
                JOIN epics e      ON e.id = t.epic_id
                JOIN epics parent ON parent.id = e.parent_epic_id
                WHERE e.origin = 'repo-group'
                  AND parent.feed_role NOT IN ('none', 'reviews-parent')
                  AND EXISTS (
                    SELECT 1 FROM tasks t2
                    WHERE t2.epic_id = parent.id
                      AND t2.external_id = t.external_id
                  )
            )",
        )
        .context("v71: failed to remove duplicate role sub-epic tasks")?;
    }

    Ok(())
}

/// Add BEFORE INSERT and BEFORE UPDATE OF epic_id triggers on `tasks` that
/// enforce: within any role sub-epic's subtree (role sub-epic + its direct
/// repo-group children), no two tasks share the same `external_id`.
///
/// "Role sub-epic" is identified by `feed_role NOT IN ('none', 'reviews-parent')`.
/// "Repo-group sub-epic" is identified by `origin = 'repo-group'` with a
/// parent whose `feed_role NOT IN ('none', 'reviews-parent')`.
///
/// NULL external_ids (manually-created tasks) are excluded from the check.
pub(super) fn migrate_v72_add_feed_task_subtree_unique_triggers(conn: &Connection) -> Result<()> {
    // BEFORE INSERT trigger.
    conn.execute_batch(
        "CREATE TRIGGER IF NOT EXISTS enforce_feed_task_subtree_unique_insert
         BEFORE INSERT ON tasks
         FOR EACH ROW
         WHEN NEW.external_id IS NOT NULL
         BEGIN
             SELECT RAISE(ABORT, 'duplicate external_id in role sub-epic subtree')
             WHERE EXISTS (
                 SELECT 1 FROM tasks t
                 JOIN epics target ON target.id = NEW.epic_id
                 LEFT JOIN epics tpar ON tpar.id = target.parent_epic_id
                 JOIN epics te ON te.id = t.epic_id
                 WHERE t.external_id = NEW.external_id
                   AND (
                     -- Target is a role sub-epic: check itself and all repo-group children.
                     ( target.feed_role NOT IN ('none', 'reviews-parent')
                       AND (te.id = target.id
                            OR te.parent_epic_id = target.id) )
                     OR
                     -- Target is a repo-group sub-epic: check the parent role sub-epic and all its children.
                     ( target.origin = 'repo-group'
                       AND tpar.feed_role NOT IN ('none', 'reviews-parent')
                       AND (te.id = tpar.id
                            OR te.parent_epic_id = tpar.id) )
                   )
             );
         END;",
    )
    .context("v72: failed to create insert trigger")?;

    // BEFORE UPDATE OF epic_id trigger (excludes the row being moved via OLD.id).
    conn.execute_batch(
        "CREATE TRIGGER IF NOT EXISTS enforce_feed_task_subtree_unique_update
         BEFORE UPDATE OF epic_id ON tasks
         FOR EACH ROW
         WHEN NEW.external_id IS NOT NULL AND NEW.epic_id != OLD.epic_id
         BEGIN
             SELECT RAISE(ABORT, 'duplicate external_id in role sub-epic subtree')
             WHERE EXISTS (
                 SELECT 1 FROM tasks t
                 JOIN epics target ON target.id = NEW.epic_id
                 LEFT JOIN epics tpar ON tpar.id = target.parent_epic_id
                 JOIN epics te ON te.id = t.epic_id
                 WHERE t.external_id = NEW.external_id
                   AND t.id != OLD.id
                   AND (
                     ( target.feed_role NOT IN ('none', 'reviews-parent')
                       AND (te.id = target.id
                            OR te.parent_epic_id = target.id) )
                     OR
                     ( target.origin = 'repo-group'
                       AND tpar.feed_role NOT IN ('none', 'reviews-parent')
                       AND (te.id = tpar.id
                            OR te.parent_epic_id = tpar.id) )
                   )
             );
         END;",
    )
    .context("v72: failed to create update trigger")?;

    Ok(())
}

/// Fix `enforce_feed_task_subtree_unique_insert` (v72): SQLite fires BEFORE
/// INSERT triggers even for `INSERT ... ON CONFLICT(epic_id, external_id) DO
/// UPDATE`, before the conflict is resolved. The v72 trigger's EXISTS check
/// had no exclusion for the row that ON CONFLICT is about to reconcile with
/// (unlike the sibling update trigger's `t.id != OLD.id`), so re-upserting
/// ANY already-tracked task — the normal path on every feed poll — matched
/// its own prior row and aborted the whole `upsert_feed_tasks` transaction,
/// silently dropping any other new task batched in the same call. Adding
/// `t.epic_id != NEW.epic_id` excludes exactly that self-row: the partial
/// unique index `tasks_epic_external_id` (v38) already guarantees at most
/// one row exists at (epic_id, external_id), so this can only ever exclude
/// the row ON CONFLICT would update — never a second, distinct same-epic
/// row, and never a genuine cross-epic duplicate elsewhere in the subtree.
pub(super) fn migrate_v76_fix_feed_task_subtree_insert_trigger_self_conflict(
    conn: &Connection,
) -> Result<()> {
    conn.execute_batch(
        "DROP TRIGGER IF EXISTS enforce_feed_task_subtree_unique_insert;
         CREATE TRIGGER enforce_feed_task_subtree_unique_insert
         BEFORE INSERT ON tasks
         FOR EACH ROW
         WHEN NEW.external_id IS NOT NULL
         BEGIN
             SELECT RAISE(ABORT, 'duplicate external_id in role sub-epic subtree')
             WHERE EXISTS (
                 SELECT 1 FROM tasks t
                 JOIN epics target ON target.id = NEW.epic_id
                 LEFT JOIN epics tpar ON tpar.id = target.parent_epic_id
                 JOIN epics te ON te.id = t.epic_id
                 WHERE t.external_id = NEW.external_id
                   AND t.epic_id != NEW.epic_id
                   AND (
                     -- Target is a role sub-epic: check itself and all repo-group children.
                     ( target.feed_role NOT IN ('none', 'reviews-parent')
                       AND (te.id = target.id
                            OR te.parent_epic_id = target.id) )
                     OR
                     -- Target is a repo-group sub-epic: check the parent role sub-epic and all its children.
                     ( target.origin = 'repo-group'
                       AND tpar.feed_role NOT IN ('none', 'reviews-parent')
                       AND (te.id = tpar.id
                            OR te.parent_epic_id = tpar.id) )
                   )
             );
         END;",
    )
    .context("v76: failed to recreate insert trigger without self-conflict")?;

    Ok(())
}
