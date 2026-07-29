use std::collections::HashSet;

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, OptionalExtension};

use crate::set_field;

use crate::models::{sort_order_for_status_transition, EpicId, TaskId, TaskStatus};

use super::super::{Database, EpicPatch};
use super::{row_to_epic, row_to_task, EPIC_COLUMNS, TASK_COLUMNS};

#[async_trait::async_trait]
impl super::super::EpicRead for Database {
    async fn get_epic(&self, id: EpicId) -> Result<Option<crate::models::Epic>> {
        self.db_call_read(move |conn| get_epic_row(conn, id)).await
    }

    async fn list_epics(&self) -> Result<Vec<crate::models::Epic>> {
        self.db_call_read(move |conn| {
            let mut stmt = conn
                .prepare_cached(&format!(
                    "SELECT {EPIC_COLUMNS} FROM epics ORDER BY COALESCE(sort_order, id) ASC, id ASC"
                ))
                .context("Failed to prepare list_epics")?;
            let epics = stmt
                .query_map([], row_to_epic)
                .context("Failed to query epics")?
                .collect::<rusqlite::Result<Vec<_>>>()
                .context("Failed to collect epics")?;
            Ok(epics)
        })
        .await
    }

    async fn list_root_epics(&self) -> Result<Vec<crate::models::Epic>> {
        self.db_call_read(move |conn| {
            let mut stmt = conn
                .prepare_cached(&format!(
                    "SELECT {EPIC_COLUMNS} FROM epics WHERE parent_epic_id IS NULL \
                     ORDER BY COALESCE(sort_order, id) ASC, id ASC"
                ))
                .context("Failed to prepare list_root_epics")?;
            let epics = stmt
                .query_map([], row_to_epic)
                .context("Failed to query root epics")?
                .collect::<rusqlite::Result<Vec<_>>>()
                .context("Failed to collect root epics")?;
            Ok(epics)
        })
        .await
    }

    async fn list_sub_epics(&self, parent_id: EpicId) -> Result<Vec<crate::models::Epic>> {
        self.db_call_read(move |conn| {
            let mut stmt = conn
                .prepare_cached(&format!(
                    "SELECT {EPIC_COLUMNS} FROM epics WHERE parent_epic_id = ?1 \
                     ORDER BY COALESCE(sort_order, id) ASC, id ASC"
                ))
                .context("Failed to prepare list_sub_epics")?;
            let epics = stmt
                .query_map(params![parent_id.0], row_to_epic)
                .context("Failed to query sub-epics")?
                .collect::<rusqlite::Result<Vec<_>>>()
                .context("Failed to collect sub-epics")?;
            Ok(epics)
        })
        .await
    }

    async fn list_tasks_for_epic(&self, epic_id: EpicId) -> Result<Vec<crate::models::Task>> {
        self.db_call_read(move |conn| {
            let mut stmt = conn
                .prepare_cached(
                    &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE epic_id = ?1 ORDER BY COALESCE(sort_order, id) ASC, id ASC"),
                )
                .context("Failed to prepare list_tasks_for_epic")?;
            let tasks = stmt
                .query_map(params![epic_id.0], row_to_task)
                .context("Failed to query tasks for epic")?
                .collect::<rusqlite::Result<Vec<_>>>()
                .context("Failed to collect tasks for epic")?;
            Ok(tasks)
        })
        .await
    }

    async fn list_all_tasks_with_epic_id(&self) -> Result<Vec<crate::models::Task>> {
        self.db_call_read(move |conn| {
            let mut stmt = conn
                .prepare_cached(&format!(
                    "SELECT {TASK_COLUMNS} FROM tasks WHERE epic_id IS NOT NULL ORDER BY epic_id ASC, COALESCE(sort_order, id) ASC, id ASC"
                ))
                .context("Failed to prepare list_all_tasks_with_epic_id")?;
            let tasks = stmt
                .query_map([], row_to_task)
                .context("Failed to query tasks with epic_id")?
                .collect::<rusqlite::Result<Vec<_>>>()
                .context("Failed to collect tasks with epic_id")?;
            Ok(tasks)
        })
        .await
    }
}

#[async_trait::async_trait]
impl super::super::EpicCrud for Database {
    async fn create_epic(
        &self,
        title: &str,
        description: &str,
        parent_epic_id: Option<EpicId>,
    ) -> Result<crate::models::Epic> {
        let title = title.to_string();
        let description = description.to_string();
        self.db_call(move |conn| {
            // auto_dispatch is set explicitly to 0 (false): new epics do not
            // auto-dispatch by default — it is an opt-in toggled with `U` in
            // the epic view. Specifying it here makes the default independent
            // of the column's historical `DEFAULT 1`.
            conn.execute(
                "INSERT INTO epics (title, description, parent_epic_id, auto_dispatch) \
                 VALUES (?1, ?2, ?3, 0)",
                params![title, description, parent_epic_id.map(|e| e.0),],
            )
            .context("Failed to insert epic")?;
            let id = EpicId(conn.last_insert_rowid());
            get_epic_row(conn, id)?
                .ok_or_else(|| anyhow::anyhow!("Epic {id} vanished after insert"))
        })
        .await
    }

    async fn create_repo_group_sub_epic(&self, parent_id: EpicId, title: &str) -> Result<EpicId> {
        let title = title.to_string();
        self.db_call(move |conn| {
            // Reuse an existing RepoGroup sub-epic of this (parent, title),
            // regardless of status; unarchive it if needed.
            if let Some(id) = conn
                .query_row(
                    "SELECT id FROM epics \
                     WHERE parent_epic_id = ?1 AND title = ?2 AND origin = 'repo-group'",
                    params![parent_id.0, title],
                    |r| r.get::<_, i64>(0),
                )
                .optional()
                .context("lookup repo-group sub-epic")?
            {
                conn.execute(
                    "UPDATE epics SET status = 'backlog', updated_at = datetime('now') \
                     WHERE id = ?1 AND status = 'archived'",
                    params![id],
                )
                .context("unarchive repo-group sub-epic")?;
                return Ok(EpicId(id));
            }

            // Create it. auto_dispatch=0, group_by_repo=0, origin='repo-group'.
            match conn.execute(
                "INSERT INTO epics (title, description, parent_epic_id, auto_dispatch, group_by_repo, origin) \
                 VALUES (?1, '', ?2, 0, 0, 'repo-group')",
                params![title, parent_id.0],
            ) {
                Ok(_) => Ok(EpicId(conn.last_insert_rowid())),
                // Lost a race: another writer inserted the same (parent,title).
                // The partial unique index rejected us — re-select and return it.
                Err(rusqlite::Error::SqliteFailure(e, _))
                    if e.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    let id = conn
                        .query_row(
                            "SELECT id FROM epics \
                             WHERE parent_epic_id = ?1 AND title = ?2 AND origin = 'repo-group'",
                            params![parent_id.0, title],
                            |r| r.get::<_, i64>(0),
                        )
                        .context("re-select after unique violation")?;
                    Ok(EpicId(id))
                }
                Err(e) => Err(anyhow::Error::from(e).context("insert repo-group sub-epic")),
            }
        })
        .await
    }

    async fn patch_epic(&self, id: EpicId, patch: &EpicPatch<'_>) -> Result<()> {
        if !patch.has_changes() {
            return Ok(());
        }
        // Materialise the patch into owned (sets, values) before crossing the
        // db_call boundary — `&EpicPatch<'_>` cannot be moved into a 'static
        // closure.
        let mut sets: Vec<&'static str> = Vec::new();
        let mut values: Vec<Box<dyn rusqlite::types::ToSql + Send>> = Vec::new();

        set_field!(sets, values, patch.title.map(str::to_string), "title");
        set_field!(
            sets,
            values,
            patch.description.map(str::to_string),
            "description"
        );
        set_field!(
            sets,
            values,
            patch.status.map(|s| s.as_str().to_string()),
            "status"
        );
        set_field!(
            sets,
            values,
            patch.plan_path.map(|opt| opt.map(str::to_string)),
            "plan_path"
        );
        set_field!(sets, values, patch.sort_order, "sort_order");
        set_field!(sets, values, patch.auto_dispatch, "auto_dispatch");
        set_field!(sets, values, patch.group_by_repo, "group_by_repo");
        set_field!(
            sets,
            values,
            patch.feed_role.map(|r| r.as_str().to_string()),
            "feed_role"
        );
        set_field!(
            sets,
            values,
            patch.origin.map(|o| o.as_str().to_string()),
            "origin"
        );
        set_field!(
            sets,
            values,
            patch.feed_command.map(|opt| opt.map(str::to_string)),
            "feed_command"
        );
        set_field!(sets, values, patch.feed_interval_secs, "feed_interval_secs");
        set_field!(
            sets,
            values,
            patch.parent_epic_id.map(|opt| opt.map(|e| e.0)),
            "parent_epic_id"
        );

        sets.push("updated_at = datetime('now')");
        values.push(Box::new(id.0));

        let sql = format!("UPDATE epics SET {} WHERE id = ?", sets.join(", "));

        self.db_call(move |conn| {
            let refs: Vec<&dyn rusqlite::types::ToSql> = values
                .iter()
                .map(|v| v.as_ref() as &dyn rusqlite::types::ToSql)
                .collect();
            let rows = conn
                .execute(&sql, refs.as_slice())
                .context("Failed to patch epic")?;
            if rows == 0 {
                anyhow::bail!("Epic {id} not found");
            }
            Ok(())
        })
        .await
    }

    async fn delete_epic(&self, id: EpicId) -> Result<()> {
        self.db_call(move |conn| {
            conn.execute_batch("BEGIN IMMEDIATE")
                .context("Failed to begin transaction")?;
            let result = delete_epic_recursive(conn, id);
            match result {
                Ok(rows) => {
                    conn.execute_batch("COMMIT")
                        .context("Failed to commit delete_epic transaction")?;
                    if rows == 0 {
                        anyhow::bail!("Epic {} not found", id);
                    }
                    Ok(())
                }
                Err(e) => {
                    conn.execute_batch("ROLLBACK").ok(); // ignore rollback error; preserves and returns the original error
                    Err(e)
                }
            }
        })
        .await
    }

    async fn set_task_epic_id(&self, task_id: TaskId, epic_id: Option<EpicId>) -> Result<()> {
        self.db_call(move |conn| {
            let rows = conn
                .execute(
                    "UPDATE tasks SET epic_id = ?1, updated_at = datetime('now') WHERE id = ?2",
                    params![epic_id.map(|e| e.0), task_id.0],
                )
                .context("Failed to set task epic_id")?;
            if rows == 0 {
                anyhow::bail!("Task {} not found", task_id);
            }
            Ok(())
        })
        .await
    }

    async fn recalculate_epic_status(&self, epic_id: EpicId) -> Result<()> {
        // Run the entire recursive walk inside a single db_call closure so the
        // recursion stays sync on the dedicated tokio_rusqlite thread. This
        // avoids the need for Box<dyn Future> to recurse through async fn.
        self.db_call(move |conn| {
            let mut visited = HashSet::new();
            recalculate_epic_status_inner(conn, epic_id, &mut visited)
        })
        .await
    }

    async fn rescope_epic_learnings(&self, from: EpicId, to: EpicId) -> Result<()> {
        let from_str = from.0.to_string();
        let to_str = to.0.to_string();
        self.db_call(move |conn| {
            conn.execute(
                "UPDATE learnings SET scope_ref = ?1 \
                 WHERE scope = 'epic' AND scope_ref = ?2",
                params![to_str, from_str],
            )
            .context("Failed to rescope epic learnings")?;
            Ok(())
        })
        .await
    }
}

// ---------------------------------------------------------------------------
// shared sync helpers (run inside db_call closures or hold the sync mutex)
// ---------------------------------------------------------------------------

fn get_epic_row(conn: &rusqlite::Connection, id: EpicId) -> Result<Option<crate::models::Epic>> {
    conn.query_row(
        &format!("SELECT {EPIC_COLUMNS} FROM epics WHERE id = ?1"),
        params![id.0],
        row_to_epic,
    )
    .optional()
    .context("Failed to get epic")
}

/// Recursively deletes sub-epics and their tasks, then deletes the epic row.
/// Returns the number of rows deleted for the root epic (0 = not found).
/// Caller must hold the connection lock and manage the transaction.
fn delete_epic_recursive(conn: &rusqlite::Connection, id: EpicId) -> Result<usize> {
    // Find direct children — collect fully before dropping the statement
    let mut stmt = conn
        .prepare_cached("SELECT id FROM epics WHERE parent_epic_id = ?1")
        .context("Failed to prepare child epic query")?;
    let child_ids: Vec<EpicId> = stmt
        .query_map(params![id.0], |row| row.get::<_, i64>(0))
        .context("Failed to query child epics")?
        .map(|r| r.map(EpicId))
        .collect::<Result<Vec<_>, _>>()
        .context("Failed to collect child epic ids")?;
    drop(stmt);
    for child_id in child_ids {
        delete_epic_recursive(conn, child_id)?;
    }
    conn.execute("DELETE FROM tasks WHERE epic_id = ?1", params![id.0])
        .context("Failed to delete epic subtasks")?;
    conn.execute("DELETE FROM epics WHERE id = ?1", params![id.0])
        .context("Failed to delete epic")
}

/// Inner recursive helper for `EpicCrud::recalculate_epic_status`. Runs sync
/// against a single `&rusqlite::Connection` (the dedicated tokio_rusqlite
/// thread) so we can use plain recursion instead of async recursion.
///
/// Threads a visited set to detect and break parent cycles, preventing
/// infinite recursion when `parent_epic_id` forms a cycle in the DB.
fn recalculate_epic_status_inner(
    conn: &rusqlite::Connection,
    epic_id: EpicId,
    visited: &mut HashSet<EpicId>,
) -> Result<()> {
    if !visited.insert(epic_id) {
        return Ok(());
    }

    let epic = match get_epic_row(conn, epic_id)? {
        Some(e) => e,
        None => return Ok(()),
    };

    // Active task statuses for this epic — project only the status column
    let mut stmt = conn
        .prepare_cached("SELECT status FROM tasks WHERE epic_id = ?1 AND status != 'archived'")
        .context("Failed to prepare task status query (recalc)")?;
    let task_statuses: Vec<TaskStatus> = stmt
        .query_map(params![epic_id.0], |row| row.get::<_, String>(0))
        .context("Failed to query task statuses (recalc)")?
        .map(|r| {
            r.map_err(anyhow::Error::from).and_then(|s| {
                TaskStatus::parse(&s)
                    .ok_or_else(|| anyhow::anyhow!("unknown task status {s:?} in recalc"))
            })
        })
        .collect::<Result<Vec<_>>>()
        .context("Failed to collect task statuses (recalc)")?;
    drop(stmt);

    // Active sub-epic statuses — project only the status column
    let mut stmt = conn
        .prepare_cached(
            "SELECT status FROM epics WHERE parent_epic_id = ?1 AND status != 'archived'",
        )
        .context("Failed to prepare sub-epic status query (recalc)")?;
    let sub_epic_statuses: Vec<TaskStatus> = stmt
        .query_map(params![epic_id.0], |row| row.get::<_, String>(0))
        .context("Failed to query sub-epic statuses (recalc)")?
        .map(|r| {
            r.map_err(anyhow::Error::from).and_then(|s| {
                TaskStatus::parse(&s)
                    .ok_or_else(|| anyhow::anyhow!("unknown epic status {s:?} in recalc"))
            })
        })
        .collect::<Result<Vec<_>>>()
        .context("Failed to collect sub-epic statuses (recalc)")?;
    drop(stmt);

    let all_statuses: Vec<TaskStatus> =
        task_statuses.into_iter().chain(sub_epic_statuses).collect();

    let target = if epic.status == TaskStatus::Archived {
        // Archived is terminal for this recalculation: an archived epic must
        // never be flipped back to `Done` just because a newly-attached
        // child happens to be all-done.
        epic.status
    } else if all_statuses.is_empty() {
        epic.status
    } else if all_statuses.iter().all(|s| *s == TaskStatus::Done) {
        TaskStatus::Done
    } else if epic.status == TaskStatus::Done {
        // regression: done epic has active non-done children
        TaskStatus::Backlog
    } else {
        epic.status
    };

    if target != epic.status {
        let now = Utc::now();
        let rows = match sort_order_for_status_transition(epic.status, target, now) {
            Some(sort_order) => conn
                .execute(
                    "UPDATE epics SET status = ?1, sort_order = ?2, updated_at = datetime('now') WHERE id = ?3",
                    params![target.as_str(), sort_order, epic_id.0],
                )
                .context("Failed to update epic status (recalc)")?,
            None => conn
                .execute(
                    "UPDATE epics SET status = ?1, updated_at = datetime('now') WHERE id = ?2",
                    params![target.as_str(), epic_id.0],
                )
                .context("Failed to update epic status (recalc)")?,
        };
        if rows == 0 {
            anyhow::bail!("Epic {epic_id} not found");
        }
    }

    if let Some(parent_id) = epic.parent_epic_id {
        recalculate_epic_status_inner(conn, parent_id, visited)?;
    }

    Ok(())
}
