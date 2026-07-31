use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};

use crate::set_field;

use crate::models::{EpicId, FeedItem, SubStatus, TaskId, TaskStatus, WrapUpMode};

use super::super::{CreateTaskRequest, Database, TaskPatch};
use super::{collect_decodable, row_to_task, write_json_string_vec, TASK_COLUMNS};

/// Owned mirror of [`CreateTaskRequest`] for moving into a `db_call` closure.
///
/// Parity with [`CreateTaskRequest`] is compiler-enforced: the [`From`] impl uses an
/// exhaustive destructuring pattern (no `..`), so adding a field to [`CreateTaskRequest`]
/// without also adding it here is a compile error.
#[derive(Debug)]
struct OwnedCreateTaskRequest {
    title: String,
    description: String,
    repo_path: String,
    plan: Option<String>,
    status: TaskStatus,
    base_branch: String,
    epic_id: Option<EpicId>,
    sort_order: Option<i64>,
    tag: Option<crate::models::TaskTag>,
    wrap_up_mode: Option<WrapUpMode>,
    auto_run_plan: bool,
}

impl<'a> From<CreateTaskRequest<'a>> for OwnedCreateTaskRequest {
    fn from(r: CreateTaskRequest<'a>) -> Self {
        let CreateTaskRequest {
            title,
            description,
            repo_path,
            plan,
            status,
            base_branch,
            epic_id,
            sort_order,
            tag,
            wrap_up_mode,
            auto_run_plan,
        } = r;
        Self {
            title: title.to_string(),
            description: description.to_string(),
            repo_path: repo_path.to_string(),
            plan: plan.map(str::to_string),
            status,
            base_branch: base_branch.to_string(),
            epic_id,
            sort_order,
            tag,
            wrap_up_mode,
            auto_run_plan,
        }
    }
}

/// Owned mirror of [`TaskPatch`] for moving into a `db_call` closure
/// (`Send + 'static` bound, so borrowed fields cannot cross the boundary).
///
/// Parity with [`TaskPatch`] is compiler-enforced: [`From<&TaskPatch<'_>>`] uses an
/// exhaustive destructuring pattern (no `..`), so adding a field to [`TaskPatch`]
/// without also adding it here is a compile error.
///
/// `labels` is deliberately omitted — it is pre-serialised to JSON before entering
/// `db_call` and handled directly via `labels_json` in [`patch_task`].
#[derive(Debug, Default)]
struct OwnedTaskPatch {
    status: Option<TaskStatus>,
    plan_path: Option<Option<String>>,
    title: Option<String>,
    description: Option<String>,
    repo_path: Option<String>,
    worktree: Option<Option<String>>,
    tmux_window: Option<Option<String>>,
    sub_status: Option<SubStatus>,
    url: Option<Option<crate::models::TaskUrl>>,
    tag: Option<Option<crate::models::TaskTag>>,
    sort_order: Option<Option<i64>>,
    base_branch: Option<String>,
    external_id: Option<Option<String>>,
    last_pre_tool_use_at: Option<Option<chrono::DateTime<chrono::Utc>>>,
    last_notification_at: Option<Option<chrono::DateTime<chrono::Utc>>>,
    wrap_up_mode: Option<Option<WrapUpMode>>,
    auto_run_plan: Option<bool>,
}

impl<'a> From<&TaskPatch<'a>> for OwnedTaskPatch {
    fn from(p: &TaskPatch<'a>) -> Self {
        let TaskPatch {
            status,
            plan_path,
            title,
            description,
            repo_path,
            worktree,
            tmux_window,
            sub_status,
            url,
            tag,
            sort_order,
            base_branch,
            external_id,
            labels: _, // pre-serialised to JSON before db_call; see patch_task
            last_pre_tool_use_at,
            last_notification_at,
            wrap_up_mode,
            auto_run_plan,
        } = *p;
        Self {
            status,
            plan_path: plan_path.map(|o| o.map(str::to_string)),
            title: title.map(str::to_string),
            description: description.map(str::to_string),
            repo_path: repo_path.map(str::to_string),
            worktree: worktree.map(|o| o.map(str::to_string)),
            tmux_window: tmux_window.map(|o| o.map(str::to_string)),
            sub_status,
            url: url.map(|o| o.cloned()),
            tag,
            sort_order,
            base_branch: base_branch.map(str::to_string),
            external_id: external_id.map(|o| o.map(str::to_string)),
            last_pre_tool_use_at,
            last_notification_at,
            wrap_up_mode,
            auto_run_plan,
        }
    }
}

#[async_trait::async_trait]
impl super::super::TaskRead for Database {
    async fn get_task(&self, id: TaskId) -> Result<Option<crate::models::Task>> {
        self.db_call_read(move |conn| {
            conn.query_row(
                &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?1"),
                params![id.0],
                row_to_task,
            )
            .optional()
            .context("Failed to get task")
        })
        .await
    }

    async fn list_all(&self) -> Result<Vec<crate::models::Task>> {
        self.db_call_read(move |conn| {
            let mut stmt = conn
                .prepare_cached(&format!(
                    "SELECT {TASK_COLUMNS} FROM tasks ORDER BY COALESCE(sort_order, id) ASC, id ASC"
                ))
                .context("Failed to prepare list_all")?;
            let rows = stmt
                .query_map([], row_to_task)
                .context("Failed to query tasks")?;
            let tasks = collect_decodable(rows, "tasks").context("Failed to collect tasks")?;
            Ok(tasks)
        })
        .await
    }

    async fn list_by_status(&self, status: TaskStatus) -> Result<Vec<crate::models::Task>> {
        self.db_call_read(move |conn| {
            let mut stmt = conn
                .prepare_cached(
                    &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE status = ?1 ORDER BY COALESCE(sort_order, id) ASC, id ASC"),
                )
                .context("Failed to prepare list_by_status")?;
            let rows = stmt
                .query_map(params![status.as_str()], row_to_task)
                .context("Failed to query tasks by status")?;
            let tasks =
                collect_decodable(rows, "tasks").context("Failed to collect tasks by status")?;
            Ok(tasks)
        })
        .await
    }

    async fn find_task_by_plan(&self, plan: &str) -> Result<Option<crate::models::Task>> {
        let plan = plan.to_string();
        self.db_call_read(move |conn| {
            conn.query_row(
                &format!("SELECT {TASK_COLUMNS} FROM tasks WHERE plan_path = ?1"),
                params![plan],
                row_to_task,
            )
            .optional()
            .context("Failed to find task by plan")
        })
        .await
    }

    async fn has_other_tasks_with_worktree(
        &self,
        worktree: &str,
        exclude_id: TaskId,
    ) -> Result<bool> {
        let worktree = worktree.to_string();
        self.db_call_read(move |conn| {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM tasks WHERE worktree = ?1 AND id != ?2 AND status != 'done'",
                    params![worktree, exclude_id.0],
                    |row| row.get(0),
                )
                .context("Failed to check shared worktree")?;
            Ok(count > 0)
        })
        .await
    }

    async fn get_total_changes(&self) -> Result<i64> {
        self.db_call(|conn| {
            conn.query_row("SELECT total_changes()", [], |row| row.get(0))
                .context("Failed to read total_changes()")
        })
        .await
    }
}

#[async_trait::async_trait]
impl super::super::TaskCrud for Database {
    async fn create_task(&self, req: CreateTaskRequest<'_>) -> Result<TaskId> {
        let req = OwnedCreateTaskRequest::from(req);
        self.db_call(move |conn| {
            let sub_status = SubStatus::default_for(req.status);
            conn.execute(
                "INSERT INTO tasks \
                 (title, description, repo_path, plan_path, status, sub_status, base_branch, \
                  epic_id, sort_order, tag, wrap_up_mode, auto_run_plan) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    req.title,
                    req.description,
                    req.repo_path,
                    req.plan,
                    req.status.as_str(),
                    sub_status.as_str(),
                    req.base_branch,
                    req.epic_id.map(|e| e.0),
                    req.sort_order,
                    req.tag.map(|t| t.as_str()),
                    req.wrap_up_mode.map(|m| m.as_str()),
                    req.auto_run_plan,
                ],
            )
            .context("Failed to insert task")?;
            Ok(TaskId(conn.last_insert_rowid()))
        })
        .await
    }

    async fn update_status_if(
        &self,
        id: TaskId,
        new_status: TaskStatus,
        expected: TaskStatus,
    ) -> Result<bool> {
        self.db_call(move |conn| {
            let default_sub = SubStatus::default_for(new_status);
            let rows = conn
                .execute(
                    "UPDATE tasks SET status = ?1, sub_status = ?2, updated_at = datetime('now') WHERE id = ?3 AND status = ?4",
                    params![new_status.as_str(), default_sub.as_str(), id.0, expected.as_str()],
                )
                .context("Failed to conditional-update status")?;
            Ok(rows > 0)
        })
        .await
    }

    async fn delete_task(&self, id: TaskId) -> Result<()> {
        self.db_call(move |conn| {
            let rows = conn
                .execute("DELETE FROM tasks WHERE id = ?1", params![id.0])
                .context("Failed to delete task")?;
            if rows == 0 {
                anyhow::bail!("Task {} not found", id);
            }
            Ok(())
        })
        .await
    }

    async fn patch_task(&self, id: TaskId, patch: &TaskPatch<'_>) -> Result<()> {
        if !patch.has_changes() {
            return Ok(());
        }
        if matches!((patch.status, patch.sub_status), (Some(s), Some(ss)) if !ss.is_valid_for(s)) {
            anyhow::bail!(
                "invalid (status, sub_status) pair in patch: {:?}/{:?}",
                patch.status,
                patch.sub_status
            );
        }
        let labels_json = match patch.labels {
            Some(labels) => Some(write_json_string_vec(labels)?),
            None => None,
        };
        let patch = OwnedTaskPatch::from(patch);
        self.db_call(move |conn| {
            let mut sets: Vec<&str> = Vec::new();
            let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

            let effective_sub_status = patch
                .sub_status
                .or_else(|| patch.status.map(SubStatus::default_for));

            set_field!(
                sets,
                values,
                patch.status.map(|s| s.as_str().to_string()),
                "status"
            );
            set_field!(sets, values, patch.title, "title");
            set_field!(sets, values, patch.description, "description");
            set_field!(sets, values, patch.repo_path, "repo_path");
            set_field!(sets, values, patch.plan_path, "plan_path");
            set_field!(sets, values, patch.worktree, "worktree");
            set_field!(sets, values, patch.tmux_window, "tmux_window");
            set_field!(
                sets,
                values,
                effective_sub_status.map(|ss| ss.as_str().to_string()),
                "sub_status"
            );
            // url + url_type are written together so the columns stay consistent.
            let url_col = patch
                .url
                .as_ref()
                .map(|o| o.as_ref().map(|u| u.url.clone()));
            let url_type_col = patch
                .url
                .as_ref()
                .map(|o| o.as_ref().map(|u| u.url_type.as_str().to_string()));
            set_field!(sets, values, url_col, "url");
            set_field!(sets, values, url_type_col, "url_type");
            set_field!(
                sets,
                values,
                patch.tag.map(|opt| opt.map(|t| t.as_str().to_string())),
                "tag"
            );
            set_field!(sets, values, patch.sort_order, "sort_order");
            set_field!(sets, values, patch.base_branch, "base_branch");
            set_field!(sets, values, patch.external_id, "external_id");
            set_field!(sets, values, labels_json, "labels");
            set_field!(
                sets,
                values,
                patch
                    .last_pre_tool_use_at
                    .map(|opt| opt.map(super::format_datetime)),
                "last_pre_tool_use_at"
            );
            set_field!(
                sets,
                values,
                patch
                    .last_notification_at
                    .map(|opt| opt.map(super::format_datetime)),
                "last_notification_at"
            );
            set_field!(
                sets,
                values,
                patch
                    .wrap_up_mode
                    .map(|opt| opt.map(|v| v.as_str().to_string())),
                "wrap_up_mode"
            );
            set_field!(sets, values, patch.auto_run_plan, "auto_run_plan");

            sets.push("updated_at = datetime('now')");
            values.push(Box::new(id.0));

            let sql = format!("UPDATE tasks SET {} WHERE id = ?", sets.join(", "));
            let refs: Vec<&dyn rusqlite::types::ToSql> =
                values.iter().map(|v| v.as_ref()).collect();
            let rows = conn
                .execute(&sql, refs.as_slice())
                .context("Failed to patch task")?;
            if rows == 0 {
                anyhow::bail!("Task {id} not found");
            }
            Ok(())
        })
        .await
    }

    async fn upsert_feed_tasks(
        &self,
        epic_id: EpicId,
        items: &[FeedItem],
        repo_paths: &[String],
        base_branches: &[String],
    ) -> Result<()> {
        // repo_paths and base_branches are parallel-to-items by contract. Verify
        // it up front: a mismatch would let the zip below silently truncate and
        // drop feed items, so reject it explicitly instead.
        if items.len() != repo_paths.len() || items.len() != base_branches.len() {
            anyhow::bail!(
                "upsert_feed_tasks slice length mismatch: items={}, repo_paths={}, base_branches={}",
                items.len(),
                repo_paths.len(),
                base_branches.len()
            );
        }
        let items = items.to_vec();
        let repo_paths = repo_paths.to_vec();
        let base_branches = base_branches.to_vec();
        // Pre-serialize labels (write_json_string_vec is sync but returns Result).
        let labels_jsons: Vec<String> = items
            .iter()
            .map(|i| write_json_string_vec(&i.labels))
            .collect::<Result<Vec<_>>>()?;
        let keep_ids =
            serde_json::to_string(&items.iter().map(|i| &i.external_id).collect::<Vec<_>>())
                .context("failed to serialize external_ids for feed task cleanup")?;
        self.db_call(move |conn| {
            // Verify epic exists before upserting tasks
            let epic_exists: bool = conn
                .query_row(
                    "SELECT 1 FROM epics WHERE id = ?1",
                    params![epic_id.0],
                    |_| Ok(true),
                )
                .optional()
                .with_context(|| format!("Failed to check epic {} for upsert_feed_tasks", epic_id))?
                .is_some();
            if !epic_exists {
                anyhow::bail!("Epic {} not found for upsert_feed_tasks", epic_id);
            }

            let tx = conn.unchecked_transaction()?;

            for (((item, repo_path), base_branch), labels_json) in items
                .iter()
                .zip(repo_paths.iter())
                .zip(base_branches.iter())
                .zip(labels_jsons.iter())
            {
                let sub_status = SubStatus::default_for(item.status).as_str().to_string();
                // item.url is copied into url so the card surfaces it
                // immediately. url_type precedence: an explicit item.url_type
                // wins; otherwise it is inferred from the URL string. On
                // conflict, an existing non-null url (and its type) wins —
                // both columns are backfilled together via paired CASE
                // expressions, never split.
                // See feeds.allium::UpsertFeedTasks.
                let (url, url_type) = if item.url.is_empty() {
                    (None, None)
                } else {
                    (
                        Some(item.url.as_str()),
                        Some(
                            item.url_type
                                .unwrap_or_else(|| crate::models::UrlType::infer(&item.url))
                                .as_str(),
                        ),
                    )
                };
                tx.execute(
                    // wrap_up_mode is INSERT-ONLY: deliberately absent from the
                    // ON CONFLICT DO UPDATE SET below, so a user's manual
                    // wrap-up choice survives feed refreshes (mirrors
                    // status/sub_status/repo_path). See feeds.allium:UpsertFeedTasks.
                    "INSERT INTO tasks
                         (title, description, repo_path, status, sub_status, base_branch,
                          epic_id, external_id, tag, labels, sort_order, url, url_type,
                          wrap_up_mode)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                     ON CONFLICT(epic_id, external_id) WHERE external_id IS NOT NULL
                     DO UPDATE SET
                         title       = excluded.title,
                         description = excluded.description,
                         tag         = excluded.tag,
                         labels      = excluded.labels,
                         sort_order  = CASE WHEN tasks.status != 'done' THEN excluded.sort_order ELSE tasks.sort_order END,
                         url      = CASE WHEN tasks.url IS NOT NULL THEN tasks.url      ELSE excluded.url      END,
                         url_type = CASE WHEN tasks.url IS NOT NULL THEN tasks.url_type ELSE excluded.url_type END,
                         updated_at  = datetime('now')",
                    params![
                        item.title,
                        item.description,
                        repo_path,
                        item.status.as_str(),
                        sub_status,
                        base_branch,
                        epic_id.0,
                        item.external_id,
                        item.tag.as_str(),
                        labels_json,
                        item.sort_order,
                        url,
                        url_type,
                        item.wrap_up_mode.map(|m| m.as_str()),
                    ],
                )
                .with_context(|| format!("Failed to upsert feed task '{}'", item.external_id))?;
            }

            tx.execute(
                "DELETE FROM tasks
                 WHERE epic_id = ?1
                   AND external_id IS NOT NULL
                   AND external_id NOT IN (SELECT value FROM json_each(?2))",
                params![epic_id.0, keep_ids],
            )
            .context("Failed to delete stale feed tasks")?;

            tx.commit()?;
            Ok(())
        })
        .await
    }

    async fn delete_stale_subtree_feed_tasks(
        &self,
        parent_id: EpicId,
        keep_external_ids: &[String],
    ) -> Result<()> {
        let keep = serde_json::to_string(keep_external_ids)
            .context("failed to serialize external_ids for subtree feed task cleanup")?;
        self.db_call(move |conn| {
            conn.execute(
                "DELETE FROM tasks
                 WHERE epic_id IN (SELECT id FROM epics WHERE parent_epic_id = ?1)
                   AND external_id IS NOT NULL
                   AND external_id NOT IN (SELECT value FROM json_each(?2))",
                params![parent_id.0, keep],
            )
            .context("Failed to delete stale subtree feed tasks")?;
            Ok(())
        })
        .await
    }

    async fn mark_pr_learnings_gate_shown(&self, id: TaskId) -> Result<bool> {
        self.db_call(move |conn| {
            let changed = conn
                .execute(
                    "UPDATE tasks SET pr_learnings_gate_shown_at = datetime('now') \
                     WHERE id = ?1 AND pr_learnings_gate_shown_at IS NULL",
                    params![id.0],
                )
                .context("Failed to mark pr_learnings_gate_shown_at")?;
            Ok(changed > 0)
        })
        .await
    }

    async fn try_claim_next_backlog_task(
        &self,
        epic_id: EpicId,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<TaskId>> {
        let now = super::format_datetime(now);
        self.db_call(move |conn| {
            let running = TaskStatus::Running;
            // The subquery carries the whole selection rule, so the row this
            // updates is chosen and claimed in the same statement.
            let claimed = conn
                .query_row(
                    "UPDATE tasks \
                     SET status = ?1, sub_status = ?2, last_pre_tool_use_at = ?3, \
                         updated_at = datetime('now') \
                     WHERE id = (SELECT id FROM tasks \
                                  WHERE epic_id = ?4 AND status = ?5 \
                                  ORDER BY COALESCE(sort_order, id), id \
                                  LIMIT 1) \
                     RETURNING id",
                    params![
                        running.as_str(),
                        SubStatus::default_for(running).as_str(),
                        now,
                        epic_id.0,
                        TaskStatus::Backlog.as_str(),
                    ],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .context("Failed to claim next backlog subtask")?;
            Ok(claimed.map(TaskId))
        })
        .await
    }

    async fn try_claim_backlog_task(
        &self,
        id: TaskId,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool> {
        let now = super::format_datetime(now);
        self.db_call(move |conn| {
            let running = TaskStatus::Running;
            // Same SET list as `try_claim_next_backlog_task` above — deliberately,
            // so "what a claim writes" has one definition — with the ordering
            // subquery replaced by the caller's id. One statement, so the claim
            // either applies whole or not at all.
            let rows = conn
                .execute(
                    "UPDATE tasks \
                     SET status = ?1, sub_status = ?2, last_pre_tool_use_at = ?3, \
                         updated_at = datetime('now') \
                     WHERE id = ?4 AND status = ?5",
                    params![
                        running.as_str(),
                        SubStatus::default_for(running).as_str(),
                        now,
                        id.0,
                        TaskStatus::Backlog.as_str(),
                    ],
                )
                .context("Failed to claim backlog task")?;
            Ok(rows == 1)
        })
        .await
    }

    async fn try_release_backlog_claim(&self, id: TaskId) -> Result<bool> {
        self.db_call(move |conn| {
            let backlog = TaskStatus::Backlog;
            let rows = conn
                .execute(
                    "UPDATE tasks \
                     SET status = ?1, sub_status = ?2, last_pre_tool_use_at = NULL, \
                         updated_at = datetime('now') \
                     WHERE id = ?3 AND status = ?4 AND worktree IS NULL",
                    params![
                        backlog.as_str(),
                        SubStatus::default_for(backlog).as_str(),
                        id.0,
                        TaskStatus::Running.as_str(),
                    ],
                )
                .context("Failed to release backlog claim")?;
            Ok(rows == 1)
        })
        .await
    }

    async fn batch_patch_sub_status(&self, updates: &[(TaskId, SubStatus)]) -> Result<()> {
        if updates.is_empty() {
            return Ok(());
        }
        let updates = updates.to_vec();
        self.db_call(move |conn| {
            let tx = conn.unchecked_transaction()?;
            for (id, sub_status) in &updates {
                tx.execute(
                    "UPDATE tasks SET sub_status = ?1, updated_at = datetime('now') WHERE id = ?2",
                    params![sub_status.as_str(), id.0],
                )
                .with_context(|| format!("Failed to patch sub_status for task {}", id.0))?;
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }

    async fn create_task_watcher(
        &self,
        watcher_task_id: TaskId,
        target_task_id: TaskId,
    ) -> Result<()> {
        self.db_call(move |conn| {
            conn.execute(
                "INSERT OR IGNORE INTO task_watchers (watcher_task_id, target_task_id) VALUES (?1, ?2)",
                params![watcher_task_id.0, target_task_id.0],
            )
            .context("Failed to insert task_watcher")?;
            Ok(())
        })
        .await
    }

    async fn delete_task_watcher(
        &self,
        watcher_task_id: TaskId,
        target_task_id: TaskId,
    ) -> Result<()> {
        self.db_call(move |conn| {
            conn.execute(
                "DELETE FROM task_watchers WHERE watcher_task_id = ?1 AND target_task_id = ?2",
                params![watcher_task_id.0, target_task_id.0],
            )
            .context("Failed to delete task_watcher")?;
            Ok(())
        })
        .await
    }

    async fn list_watchers_of(&self, target_task_id: TaskId) -> Result<Vec<TaskId>> {
        self.db_call(move |conn| {
            let mut stmt = conn
                .prepare("SELECT watcher_task_id FROM task_watchers WHERE target_task_id = ?1")
                .context("Failed to prepare list_watchers_of query")?;
            let rows = stmt
                .query_map(params![target_task_id.0], |r| r.get::<_, i64>(0))
                .context("Failed to query watchers")?;
            let mut out = Vec::new();
            for row in rows {
                out.push(TaskId(row.context("Failed to read watcher_task_id row")?));
            }
            Ok(out)
        })
        .await
    }

    async fn delete_watches_of_target(&self, target_task_id: TaskId) -> Result<()> {
        self.db_call(move |conn| {
            conn.execute(
                "DELETE FROM task_watchers WHERE target_task_id = ?1",
                params![target_task_id.0],
            )
            .context("Failed to delete watches of target")?;
            Ok(())
        })
        .await
    }

    async fn delete_watches_by_watcher(&self, watcher_task_id: TaskId) -> Result<()> {
        self.db_call(move |conn| {
            conn.execute(
                "DELETE FROM task_watchers WHERE watcher_task_id = ?1",
                params![watcher_task_id.0],
            )
            .context("Failed to delete watches by watcher")?;
            Ok(())
        })
        .await
    }
}
