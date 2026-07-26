mod migrations;
mod queries;
#[cfg(test)]
mod tests;

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use std::path::Path;

use crate::models::{
    Epic, EpicId, FeedItem, Learning, LearningId, LearningKind, LearningRetrieval, LearningScope,
    LearningStatus, LearningVerdict, RetrievalSource, SubStatus, Task, TaskId, TaskStatus, TaskTag,
    Todo, TodoId, WrapUpMode,
};

// ---------------------------------------------------------------------------
// patch_struct! — declarative macro for selective-update builder structs
// ---------------------------------------------------------------------------

/// Generates a lifetime-parameterised builder struct for partial DB updates.
///
/// Each field is wrapped in `Option<…>` (default `None` = don't touch).
/// Two field kinds:
/// - `plain    field: Type` — `Option<Type>` storage; setter takes `Type`.
/// - `nullable field: Type` — `Option<Option<Type>>` storage (double-Option);
///   setter takes `Option<Type>` (allows NULL vs value distinction).
///
/// Also generates `new()` (alias for `Default::default()`) and
/// `has_changes()` (true if any field is `Some`).
macro_rules! patch_struct {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident < $lt:lifetime > {
            $( $kind:ident $field:ident : $ty:ty ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Default)]
        $vis struct $name<$lt> {
            $( pub $field: patch_struct!(@field_type $kind $ty), )*
        }

        impl<$lt> $name<$lt> {
            pub fn new() -> Self { Self::default() }

            $( patch_struct!(@setter $kind $field $ty); )*

            pub fn has_changes(&self) -> bool {
                false $(|| self.$field.is_some())*
            }
        }
    };

    (@field_type plain    $ty:ty) => { Option<$ty> };
    (@field_type nullable $ty:ty) => { Option<Option<$ty>> };

    (@setter plain    $field:ident $ty:ty) => {
        pub fn $field(mut self, v: $ty) -> Self { self.$field = Some(v); self }
    };
    (@setter nullable $field:ident $ty:ty) => {
        pub fn $field(mut self, v: Option<$ty>) -> Self { self.$field = Some(v); self }
    };
}

// ---------------------------------------------------------------------------
// TaskPatch — builder for selective field updates
// ---------------------------------------------------------------------------

patch_struct! {
    /// Builder for selective task field updates.
    pub struct TaskPatch<'a> {
        plain    status:       TaskStatus,
        nullable plan_path:    &'a str,
        plain    title:        &'a str,
        plain    description:  &'a str,
        plain    repo_path:    &'a str,
        nullable worktree:     &'a str,
        nullable tmux_window:  &'a str,
        plain    sub_status:   SubStatus,
        nullable url:          &'a crate::models::TaskUrl,
        nullable tag:          TaskTag,
        nullable sort_order:   i64,
        plain    base_branch:  &'a str,
        nullable external_id:  &'a str,
        plain    labels:       &'a [String],
        nullable last_pre_tool_use_at: chrono::DateTime<chrono::Utc>,
        nullable last_notification_at: chrono::DateTime<chrono::Utc>,
        nullable wrap_up_mode: WrapUpMode,
        plain    auto_run_plan: bool,
    }
}

// ---------------------------------------------------------------------------
// CreateTaskRequest — input struct for the create_task DB operation
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct CreateTaskRequest<'a> {
    pub title: &'a str,
    pub description: &'a str,
    pub repo_path: &'a str,
    pub plan: Option<&'a str>,
    pub status: TaskStatus,
    pub base_branch: &'a str,
    pub epic_id: Option<EpicId>,
    pub sort_order: Option<i64>,
    pub tag: Option<TaskTag>,
    pub wrap_up_mode: Option<WrapUpMode>,
    pub auto_run_plan: bool,
}

// ---------------------------------------------------------------------------
// EpicPatch — builder for selective epic field updates
// ---------------------------------------------------------------------------

patch_struct! {
    /// Builder for selective epic field updates.
    pub struct EpicPatch<'a> {
        plain    title:              &'a str,
        plain    description:        &'a str,
        plain    status:             TaskStatus,
        nullable plan_path:          &'a str,
        nullable sort_order:         i64,
        plain    auto_dispatch:      bool,
        plain    group_by_repo:      bool,
        plain    feed_role:          crate::models::FeedRole,
        plain    origin:             crate::models::EpicOrigin,
        nullable feed_command:       &'a str,
        nullable feed_interval_secs: i64,
        nullable parent_epic_id:     EpicId,
    }
}

// ---------------------------------------------------------------------------
// Sub-traits — focused slices of the database API
// ---------------------------------------------------------------------------

/// Read-only task queries. Held (via [`TaskReadStore`]) by non-service consumers so
/// a direct task *mutation* from a handler is a compile error — see the
/// "Service layer is the mutation boundary" section of `docs/conventions.md`.
#[async_trait::async_trait]
pub trait TaskRead: Send + Sync {
    async fn get_task(&self, id: TaskId) -> Result<Option<Task>>;
    async fn list_all(&self) -> Result<Vec<Task>>;
    async fn list_by_status(&self, status: TaskStatus) -> Result<Vec<Task>>;
    async fn find_task_by_plan(&self, plan: &str) -> Result<Option<Task>>;
    async fn has_other_tasks_with_worktree(
        &self,
        worktree: &str,
        exclude_id: TaskId,
    ) -> Result<bool>;
    /// Return the cumulative INSERT/UPDATE/DELETE count for this connection since
    /// it was opened. Cheap watermark: if the value is the same as the last
    /// snapshot, no writes have occurred and a tick-driven full refresh can be skipped.
    async fn get_total_changes(&self) -> Result<i64>;
}

/// Task mutations, layered on top of the read surface. Reachable only by the
/// service layer (which owns invariants like epic-status recalculation) and by
/// the sanctioned feed subsystem — non-service consumers hold [`TaskReadStore`].
#[async_trait::async_trait]
pub trait TaskCrud: TaskRead {
    async fn create_task(&self, req: CreateTaskRequest<'_>) -> Result<TaskId>;
    /// Update status only if current status matches `expected`. Returns true if updated.
    async fn update_status_if(
        &self,
        id: TaskId,
        new_status: TaskStatus,
        expected: TaskStatus,
    ) -> Result<bool>;
    async fn delete_task(&self, id: TaskId) -> Result<()>;
    async fn patch_task(&self, id: TaskId, patch: &TaskPatch<'_>) -> Result<()>;
    /// Atomically set `pr_learnings_gate_shown_at` to now if it is currently null.
    /// Returns `true` if this call set it (first `gh pr create` for the task →
    /// caller should block), `false` if it was already set or the task does not
    /// exist (caller should allow the PR).
    async fn mark_pr_learnings_gate_shown(&self, id: TaskId) -> Result<bool>;
    /// Atomically claim a backlog task for dispatch: move it to `Running` with
    /// the default running sub-status and `last_pre_tool_use_at = now`, but only
    /// while it is still in `Backlog`. Returns `true` when this call won the
    /// row, `false` when a concurrent caller already claimed it (or the task
    /// does not exist).
    ///
    /// The `WHERE status = 'backlog'` clause is what makes the claim exclusive —
    /// not a lock. Every mutation serialises through the single writer
    /// connection, so the conditional `UPDATE` is atomic against all other
    /// writes. Backs the epic-chaining claim described by
    /// `AutoDispatchNextSubtask` in `docs/specs/epics.allium`.
    async fn try_claim_backlog_task(
        &self,
        id: TaskId,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool>;
    /// Upsert tasks from a feed. Inserts new tasks; on conflict (epic_id, external_id)
    /// updates title and description only — status and other user-managed fields are preserved.
    ///
    /// `repo_paths` is a parallel slice: `repo_paths[i]` is the resolved local path for
    /// `items[i]`. Pass `""` when the path could not be resolved — dispatch will be blocked
    /// until the user sets it via the task editor.
    ///
    /// `base_branches` is a parallel slice: `base_branches[i]` is the base branch the
    /// inserted task should use for its worktree. The caller is expected to resolve the
    /// repo's default branch (typically via [`crate::git::detect_default_branch`]),
    /// falling back to `"main"` when no path is known.
    async fn upsert_feed_tasks(
        &self,
        epic_id: EpicId,
        items: &[FeedItem],
        repo_paths: &[String],
        base_branches: &[String],
    ) -> Result<()>;
    /// Delete stale feed tasks across the WHOLE subtree of `parent_id` (all its
    /// direct child epics), keeping only those whose `external_id` is in
    /// `keep_external_ids`. Unlike [`TaskCrud::upsert_feed_tasks`]'s per-epic
    /// delete pass, this is subtree-scoped with a global keep-set, so a task
    /// that was just *moved* between role sub-epics (absent from its losing
    /// epic's group) is not deleted. Manual tasks (`external_id IS NULL`) are
    /// always preserved. Used by the role-routed reviews reconciler.
    async fn delete_stale_subtree_feed_tasks(
        &self,
        parent_id: EpicId,
        keep_external_ids: &[String],
    ) -> Result<()>;
    /// Atomically update `sub_status` for multiple tasks in a single transaction.
    /// Used by the tick to batch all per-task reclassifications into one DB round-trip.
    async fn batch_patch_sub_status(&self, updates: &[(TaskId, SubStatus)]) -> Result<()>;
    /// Insert a watch: `watcher_task_id` wants to be notified when
    /// `target_task_id` finishes (`Done`/`Archived`) or is deleted first.
    /// Idempotent — inserting an existing (watcher, target) pair is a no-op.
    async fn create_task_watcher(
        &self,
        watcher_task_id: TaskId,
        target_task_id: TaskId,
    ) -> Result<()>;
    /// Remove a specific watch. Idempotent — no-op if it doesn't exist.
    async fn delete_task_watcher(
        &self,
        watcher_task_id: TaskId,
        target_task_id: TaskId,
    ) -> Result<()>;
    /// List every task currently watching `target_task_id`.
    async fn list_watchers_of(&self, target_task_id: TaskId) -> Result<Vec<TaskId>>;
    /// Remove every watch row where `target_task_id` is the target. Called
    /// after firing finish/delete notifications for that target.
    async fn delete_watches_of_target(&self, target_task_id: TaskId) -> Result<()>;
    /// Remove every watch row where `watcher_task_id` is the watcher. Called
    /// when the watcher itself is deleted.
    async fn delete_watches_by_watcher(&self, watcher_task_id: TaskId) -> Result<()>;
}

/// Read-only epic queries. Held (via [`TaskReadStore`]) by non-service consumers.
#[async_trait::async_trait]
pub trait EpicRead: Send + Sync {
    async fn get_epic(&self, id: EpicId) -> Result<Option<Epic>>;
    async fn list_epics(&self) -> Result<Vec<Epic>>;
    /// List only root epics (no parent). Used for the main board view.
    async fn list_root_epics(&self) -> Result<Vec<Epic>>;
    /// List direct children of the given epic.
    async fn list_sub_epics(&self, parent_id: EpicId) -> Result<Vec<Epic>>;
    async fn list_tasks_for_epic(&self, epic_id: EpicId) -> Result<Vec<Task>>;
    /// Fetch all tasks that have a non-null epic_id in a single query.
    /// Use instead of looping over epics and calling list_tasks_for_epic() per epic.
    async fn list_all_tasks_with_epic_id(&self) -> Result<Vec<Task>>;
}

/// Epic mutations, layered on top of the read surface. The
/// `recalculate_epic_status` invariant lives here; reachable only by the service
/// layer and the sanctioned feed subsystem — non-service consumers hold
/// [`TaskReadStore`].
#[async_trait::async_trait]
pub trait EpicCrud: EpicRead {
    /// Create a new epic. `parent_epic_id` can be changed later via
    /// [`EpicCrud::patch_epic`]. The DB enforces `CHECK (parent_epic_id != id)`
    /// (migration v35) to prevent self-loops; cycle detection is handled in
    /// the service layer before any DB write.
    async fn create_epic(
        &self,
        title: &str,
        description: &str,
        parent_epic_id: Option<EpicId>,
    ) -> Result<Epic>;
    /// Find-or-create the `RepoGroup` sub-epic of `parent_id` titled `title`.
    /// Race-safe via the partial unique index; reuses (and unarchives) an
    /// existing match rather than creating a duplicate.
    async fn create_repo_group_sub_epic(&self, parent_id: EpicId, title: &str) -> Result<EpicId>;
    async fn patch_epic(&self, id: EpicId, patch: &EpicPatch<'_>) -> Result<()>;
    async fn delete_epic(&self, id: EpicId) -> Result<()>;
    async fn set_task_epic_id(&self, task_id: TaskId, epic_id: Option<EpicId>) -> Result<()>;
    /// Recalculate an epic's status from its active children (tasks + sub-epics).
    /// Propagates upward to the parent epic if one exists.
    async fn recalculate_epic_status(&self, epic_id: EpicId) -> Result<()>;
    /// Re-scope all epic-scoped learnings whose scope_ref = `from` to `to`.
    /// Used when a repo-group sub-epic is deleted, so its learnings are not
    /// left pointing at a deleted epic id. scope_ref is not an embedding
    /// input, so no re-embed obligation.
    async fn rescope_epic_learnings(&self, from: EpicId, to: EpicId) -> Result<()>;
}

/// Settings, filter presets, repo paths, and usage tracking.
#[async_trait::async_trait]
pub trait SettingsStore: Send + Sync {
    async fn list_repo_paths(&self) -> Result<Vec<String>>;
    async fn save_repo_path(&self, path: &str) -> Result<()>;
    async fn delete_repo_path(&self, path: &str) -> Result<()>;
    async fn get_setting_bool(&self, key: &str) -> Result<Option<bool>>;
    async fn set_setting_bool(&self, key: &str, value: bool) -> Result<()>;
    async fn get_setting_string(&self, key: &str) -> Result<Option<String>>;
    async fn set_setting_string(&self, key: &str, value: &str) -> Result<()>;
    async fn save_filter_preset(&self, name: &str, repo_paths: &[String], mode: &str)
        -> Result<()>;
    async fn delete_filter_preset(&self, name: &str) -> Result<()>;
    async fn list_filter_presets(&self) -> Result<Vec<(String, Vec<String>, String)>>;
    async fn get_tips_state(&self) -> Result<(u32, crate::models::TipsShowMode)>;
    async fn save_tips_state(
        &self,
        seen_up_to: u32,
        show_mode: crate::models::TipsShowMode,
    ) -> Result<()>;
    async fn get_verify_command(&self, path: &str) -> Result<Option<String>>;
    /// Set the verify command for a known repo path.
    ///
    /// If `command` is `Some(cmd)` and the path does not exist in `repo_paths`, a new
    /// row is inserted (with `last_used = now()`), equivalent to calling `save_repo_path`
    /// first. If `command` is `None`, the column is cleared to NULL; unknown paths are
    /// silently ignored (no row created).
    ///
    /// `command` must not contain a newline (`\n`) or carriage return (`\r`); returns
    /// an error if it does. Empty or whitespace-only commands are treated as `None`.
    async fn set_verify_command(&self, path: &str, command: Option<&str>) -> Result<()>;

    // -- Managed-feed config (WP5) --
    // Typed accessors over the `settings` table for the two managed feed
    // scripts and their poll intervals. `Some(..)` upserts; `None` clears the
    // key (so a subsequent get returns `None`). Intervals are stored as their
    // decimal string. See `epics.allium` ProvisionManagedEpics / the `config`
    // block for the authoritative semantics.
    async fn get_reviews_feed_command(&self) -> Result<Option<String>>;
    async fn set_reviews_feed_command(&self, value: Option<&str>) -> Result<()>;
    async fn get_reviews_feed_interval_secs(&self) -> Result<Option<i64>>;
    async fn set_reviews_feed_interval_secs(&self, value: Option<i64>) -> Result<()>;
    async fn get_cve_feed_command(&self) -> Result<Option<String>>;
    async fn set_cve_feed_command(&self, value: Option<&str>) -> Result<()>;
    async fn get_cve_feed_interval_secs(&self) -> Result<Option<i64>>;
    async fn set_cve_feed_interval_secs(&self, value: Option<i64>) -> Result<()>;

    // -- Base branch history (task #3422) --
    // Per-repo most-recently-used base_branch history. See
    // docs/specs/dispatch.allium (rule RecordBaseBranch, surface
    // BaseBranchPicker, config.max_base_branches_per_repo) and
    // docs/specs/core.allium (entity SavedRepoBranch).

    /// Upsert `(repo_path, branch)`, bumping `last_used` to now, then prune
    /// `repo_path`'s history down to the `max_base_branches_per_repo` (10)
    /// most-recently-used rows.
    async fn record_base_branch(&self, repo_path: &str, branch: &str) -> Result<()>;

    /// All `(repo_path, branch)` pairs across every repo, ordered by
    /// `last_used DESC`.
    async fn list_all_base_branches(&self) -> Result<Vec<(String, String)>>;
}

// ---------------------------------------------------------------------------
// TaskAndEpicStore — composite for consumers that need tasks + epics only
// ---------------------------------------------------------------------------

pub trait TaskAndEpicStore: TaskCrud + EpicCrud {}

impl<T: TaskCrud + EpicCrud> TaskAndEpicStore for T {}

// ---------------------------------------------------------------------------
// LearningPatch — builder for partial learning updates
// ---------------------------------------------------------------------------

patch_struct! {
    /// Builder for selective learning field updates.
    pub struct LearningPatch<'a> {
        plain    status:    LearningStatus,
        plain    summary:   &'a str,
        nullable detail:    &'a str,
        plain    kind:      LearningKind,
        plain    tags:      &'a [String],
        plain    embedding: &'a [u8],
    }
}

// ---------------------------------------------------------------------------
// TodoPatch — builder for partial todo updates
// ---------------------------------------------------------------------------

patch_struct! {
    /// Builder for selective todo field updates.
    pub struct TodoPatch<'a> {
        plain    title:      &'a str,
        plain    done:       bool,
        plain    sort_order: i64,
        nullable task_id:    i64,
        nullable epic_id:    i64,
        nullable parent_id:  i64,
    }
}

// ---------------------------------------------------------------------------
// CreateTodoRow — DB-layer params for inserting a todo row
// ---------------------------------------------------------------------------

pub struct CreateTodoRow<'a> {
    pub title: &'a str,
    /// Raw FK to tasks.id — None means no link.
    pub task_id: Option<i64>,
    /// Raw FK to epics.id — None means no link.
    pub epic_id: Option<i64>,
}

// ---------------------------------------------------------------------------
// LearningFilter — optional filter for list_learnings
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct LearningFilter {
    pub status: Option<LearningStatus>,
    pub scope: Option<LearningScope>,
    pub scope_ref: Option<String>,
    /// Return only learnings whose tags intersect this set (OR match).
    pub tags: Vec<String>,
    pub limit: Option<usize>,
}

// ---------------------------------------------------------------------------
// CreateLearningRow — DB-layer params for inserting a learning row
// ---------------------------------------------------------------------------

pub struct CreateLearningRow<'a> {
    pub kind: LearningKind,
    pub summary: &'a str,
    pub detail: Option<&'a str>,
    pub scope: LearningScope,
    pub scope_ref: Option<&'a str>,
    pub tags: &'a [String],
    pub source_task_id: Option<TaskId>,
    pub embedding: Option<&'a [u8]>,
}

// ---------------------------------------------------------------------------
// LearningStore — narrow sub-trait for the learnings table
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
pub trait LearningStore: Send + Sync {
    async fn create_learning(&self, row: CreateLearningRow<'_>) -> Result<LearningId>;

    async fn get_learning(&self, id: LearningId) -> Result<Option<Learning>>;

    async fn list_learnings(&self, filter: LearningFilter) -> Result<Vec<Learning>>;

    async fn patch_learning(&self, id: LearningId, patch: &LearningPatch<'_>) -> Result<()>;

    async fn delete_learning(&self, id: LearningId) -> Result<bool>;

    /// Returns approved learnings for the given task context, unioning user + repo + epic
    /// scopes. Task-scoped learnings are excluded (they surface via explicit query only).
    /// Ordered by scope priority (procedural > epic > repo > user), then upvote_count DESC.
    async fn list_learnings_for_dispatch(
        &self,
        repo_path: &str,
        epic_id: Option<EpicId>,
    ) -> Result<Vec<Learning>>;

    /// Returns all approved, non-task-scoped learnings that have embeddings stored,
    /// with their raw embedding bytes. Used by the RAG pipeline.
    async fn list_all_approved_non_task_learnings(&self) -> Result<Vec<(Learning, Vec<u8>)>>;

    /// Returns approved, non-task-scoped learnings that have no embedding stored yet.
    /// Used by the backfill job to determine which learnings need to be embedded.
    async fn list_learnings_missing_embedding(&self) -> Result<Vec<Learning>>;

    /// Archive approved learnings that have proven unvalued and gone stale:
    /// status = approved AND upvote_count <= 0 AND updated_at <= cutoff.
    /// Sets status = archived and updated_at = now. Returns the number of rows
    /// affected. See docs/specs/learnings.allium: ArchiveStaleLearning.
    async fn archive_stale_learnings(&self, cutoff: chrono::DateTime<chrono::Utc>) -> Result<u64>;
}

// ---------------------------------------------------------------------------
// LearningRetrievalStore — narrow sub-trait for retrievals + verdicts
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
pub trait LearningRetrievalStore: Send + Sync {
    /// Insert a row into `learning_retrievals` recording that `learning_id` was
    /// surfaced to `task_id` via the given source.
    async fn record_retrieval(
        &self,
        task_id: TaskId,
        learning_id: LearningId,
        source: RetrievalSource,
    ) -> Result<()>;

    /// Return all retrievals recorded for `task_id`, ordered by id ascending.
    async fn list_retrievals_for_task(&self, task_id: TaskId) -> Result<Vec<LearningRetrieval>>;

    /// Apply a batch of verdicts atomically. Verdicts are not persisted; each
    /// only adjusts the learning's score: `Helped` bumps `upvote_count` (and
    /// sets `last_upvoted_at`), `Wrong` decrements it (a downvote; may go
    /// negative). Neither verdict changes status.
    async fn apply_verdicts_tx(&self, verdicts: &[(LearningId, LearningVerdict)]) -> Result<()>;
}

// ---------------------------------------------------------------------------
// TodoStore — narrow sub-trait for the todos table
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
pub trait TodoStore: Send + Sync {
    /// Return all todos ordered by sort_order ASC.
    async fn list_todos(&self) -> Result<Vec<Todo>>;

    /// Insert a new todo. `sort_order` is set to
    /// `COALESCE((SELECT MAX(sort_order) FROM todos), -1) + 1` so new items
    /// always append. Returns the id of the inserted row.
    async fn insert_todo(&self, row: CreateTodoRow<'_>) -> Result<TodoId>;

    /// Apply a partial update to an existing todo. No-op when `patch.has_changes()` is false.
    async fn patch_todo(&self, id: TodoId, patch: &TodoPatch<'_>) -> Result<()>;

    /// Delete a single todo by id.
    async fn delete_todo(&self, id: TodoId) -> Result<()>;

    /// Delete all todos where `done = 1`.
    async fn delete_done_todos(&self) -> Result<()>;
}

// ---------------------------------------------------------------------------
// UsageStore — narrow sub-trait for the usage_events table
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct UsageQuery {
    pub category: Option<String>,
    pub actor: Option<String>,
    pub since: Option<chrono::DateTime<chrono::Utc>>,
    pub limit: Option<usize>,
}

/// Maximum number of events to keep in `usage_events`. Older rows are deleted
/// when the table exceeds this cap.
#[derive(Debug, Clone, Copy)]
pub struct UsageCap(u64);

impl UsageCap {
    pub fn new(n: u64) -> Self {
        assert!(n > 0, "UsageCap must be > 0");
        Self(n)
    }

    pub fn value(self) -> u64 {
        self.0
    }
}

impl Default for UsageCap {
    fn default() -> Self {
        Self(100_000)
    }
}

#[async_trait::async_trait]
pub trait UsageStore: Send + Sync {
    async fn record_usage_event(&self, event: &crate::models::UsageEvent) -> Result<()> {
        self.record_usage_event_with_cap(event, UsageCap::default())
            .await
    }
    async fn record_usage_event_with_cap(
        &self,
        event: &crate::models::UsageEvent,
        cap: UsageCap,
    ) -> Result<()>;
    /// Aggregated rows ordered by count ASC so the rarest features surface
    /// first — the primary use case is identifying candidates for pruning.
    async fn query_usage(&self, query: &UsageQuery) -> Result<Vec<crate::models::UsageSummary>>;
}

// ---------------------------------------------------------------------------
// TaskStore — supertrait combining all sub-traits
// ---------------------------------------------------------------------------

pub trait TaskStore:
    TaskAndEpicStore
    + TaskReadStore
    + SettingsStore
    + LearningStore
    + LearningRetrievalStore
    + UsageStore
{
}

impl<
        T: TaskAndEpicStore
            + TaskReadStore
            + SettingsStore
            + LearningStore
            + LearningRetrievalStore
            + UsageStore,
    > TaskStore for T
{
}

// ---------------------------------------------------------------------------
// TaskReadStore — task/epic-read-only handle held by non-service consumers
// ---------------------------------------------------------------------------

/// The handle held by non-service consumers (`McpState`, `TuiRuntime`). It
/// exposes the task/epic **read** surface ([`TaskRead`] + [`EpicRead`]) but not
/// [`TaskCrud`]/[`EpicCrud`] mutations, so a direct `state.db.patch_task(...)`
/// from a handler is a **compile error**. Task and epic writes must go through
/// `TaskServiceApi`/`EpicServiceApi`, which own the `recalculate_epic_status`
/// invariant — see the mutation-boundary section of `docs/conventions.md`.
///
/// The name is deliberately scoped: it seals **task/epic** writes, not every
/// write. Settings/learning/usage writes remain reachable here — they carry no
/// cross-entity invariant, so sealing them would add churn without protecting
/// anything. `TaskStore: TaskReadStore` holds transitively, so a write-capable
/// `Arc<dyn TaskStore>` upcasts to `Arc<dyn TaskReadStore>` for free.
///
/// Reads are reachable through the handle:
///
/// ```
/// use dispatch_tui::db::{TaskReadStore, TaskRead};
/// use dispatch_tui::models::TaskId;
/// async fn reads_ok(db: &dyn TaskReadStore) {
///     let _ = db.get_task(TaskId(1)).await; // a query — fine
/// }
/// ```
///
/// Task/epic mutations are a **compile error** — there is no way to reach
/// `patch_task` (or any `TaskCrud`/`EpicCrud` method) through a `TaskReadStore`:
///
/// ```compile_fail
/// use dispatch_tui::db::{TaskReadStore, TaskPatch};
/// use dispatch_tui::models::TaskId;
/// async fn mutation_rejected(db: &dyn TaskReadStore) {
///     // `patch_task` lives on `TaskCrud`, which `TaskReadStore` does not expose.
///     let _ = db.patch_task(TaskId(1), &TaskPatch::new()).await;
/// }
/// ```
pub trait TaskReadStore:
    TaskRead + EpicRead + SettingsStore + LearningStore + LearningRetrievalStore + UsageStore
{
}

impl<
        T: TaskRead + EpicRead + SettingsStore + LearningStore + LearningRetrievalStore + UsageStore,
    > TaskReadStore for T
{
}

// ---------------------------------------------------------------------------
// Database
// ---------------------------------------------------------------------------

/// A `db_call` whose measured wall-clock elapsed time (including any time
/// spent queued/retrying behind another connection's SQLite lock) exceeds
/// this threshold is considered abnormally slow and triggers a warn log.
/// See `config.slow_db_call_threshold_ms` and the `DbCallSlowWarning` rule in
/// `docs/specs/observability.allium`.
const SLOW_DB_CALL_THRESHOLD: std::time::Duration = std::time::Duration::from_millis(200);

/// Newtype that adapts `anyhow::Error` (which itself does not implement
/// [`std::error::Error`]) into the boxed-error slot that
/// [`tokio_rusqlite::Error::Other`] expects. Round-trips through `Box<dyn
/// StdError>` so [`Database::db_call`] can recover the original error.
#[derive(Debug)]
struct AnyhowErr(anyhow::Error);

impl std::fmt::Display for AnyhowErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

impl std::error::Error for AnyhowErr {}

/// Number of lazily-opened read-only connections available to
/// [`Database::db_call_read`]. Not runtime-configurable — see the "Pool size
/// rationale" section of
/// `docs/superpowers/specs/2026-07-25-db-connection-pooling-design.md`.
const READ_POOL_SIZE: usize = 4;

/// Monotonic counter used to mint a unique shared-cache in-memory database
/// name per [`Database::open_in_memory`] call, so independent in-memory
/// instances (e.g. parallel tests) never collide on the same named cache.
static NEXT_MEMDB_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Where read-pool connections should (re)open against. Resolved once, at
/// [`Database::open`]/[`Database::open_in_memory`] time, so a pool slot can
/// lazily open the right target the first time it's needed.
enum ReadTarget {
    File(std::path::PathBuf),
    MemoryUri(String),
}

/// Async-only storage backing for the [`Database`].
///
/// Wraps a single writer [`tokio_rusqlite::Connection`] — a dedicated worker
/// thread owning a `rusqlite::Connection` that all *mutating* async store
/// impls dispatch to via [`Database::db_call`] — plus a small pool of
/// read-only WAL connections that pure-read impls dispatch to via
/// [`Database::db_call_read`], removing reader/reader and reader/writer
/// serialization on the single writer thread. See
/// `docs/superpowers/specs/2026-07-25-db-connection-pooling-design.md`.
/// There is no sync connection or `Mutex`; schema init and migrations run on
/// the writer thread via the same closure mechanism, before any reader opens.
pub struct Database {
    conn: tokio_rusqlite::Connection,
    read_pool: Vec<tokio::sync::OnceCell<tokio_rusqlite::Connection>>,
    next_reader: std::sync::atomic::AtomicUsize,
    read_target: ReadTarget,
}

impl Database {
    pub async fn open(path: &Path) -> Result<Self> {
        // Ensure the parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create db directory: {}", parent.display()))?;
        }

        let conn = tokio_rusqlite::Connection::open(path)
            .await
            .with_context(|| format!("Failed to open database at {}", path.display()))?;

        Self::init_schema(&conn).await?;

        Ok(Database {
            conn,
            read_pool: Self::empty_read_pool(),
            next_reader: std::sync::atomic::AtomicUsize::new(0),
            read_target: ReadTarget::File(path.to_path_buf()),
        })
    }

    pub async fn open_in_memory() -> Result<Self> {
        let id = NEXT_MEMDB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // A plain `:memory:` connection is private to itself; the read pool
        // needs the writer and every reader to share the same in-memory
        // database, so this uses SQLite's `memdb` VFS (3.36+) via a named
        // URI — unique per `Database` instance so concurrently-running tests
        // never collide. Note: the older `file:name?mode=memory&cache=shared`
        // idiom does NOT work here — SQLite's docs say mixing the `mode=`
        // query parameter with explicit `sqlite3_open_v2` flags is undefined,
        // and empirically it silently ignores `SQLITE_OPEN_READ_ONLY` (every
        // connection ends up read-write regardless of the flags passed). The
        // `memdb` VFS is the purpose-built mechanism for a named, shared,
        // multi-connection in-memory database that correctly enforces
        // read-only opens — required for `open_reader` below. `memdb`
        // additionally requires the name to start with `/` to be treated as
        // a *shared* store (see `memdbOpen` in SQLite's `os_mem.c` — a name
        // with no leading slash gets a private, unshared store instead).
        let uri = format!("file:/dispatch-mem-{id}?vfs=memdb");
        let conn = tokio_rusqlite::Connection::open(&uri)
            .await
            .context("Failed to open in-memory database")?;
        Self::init_schema(&conn).await?;
        Ok(Database {
            conn,
            read_pool: Self::empty_read_pool(),
            next_reader: std::sync::atomic::AtomicUsize::new(0),
            read_target: ReadTarget::MemoryUri(uri),
        })
    }

    fn empty_read_pool() -> Vec<tokio::sync::OnceCell<tokio_rusqlite::Connection>> {
        std::iter::repeat_with(tokio::sync::OnceCell::new)
            .take(READ_POOL_SIZE)
            .collect()
    }

    /// Opens a new read-only connection against `target` and applies the
    /// reader-relevant PRAGMAs `init_schema_sync` sets on the writer
    /// (`busy_timeout`, `cache_size`, `temp_store`). `journal_mode` and
    /// `foreign_keys` are database-wide/write-relevant and don't need
    /// resetting per reader connection.
    async fn open_reader(target: &ReadTarget) -> Result<tokio_rusqlite::Connection> {
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = match target {
            ReadTarget::File(path) => tokio_rusqlite::Connection::open_with_flags(path, flags)
                .await
                .with_context(|| format!("Failed to open read connection to {}", path.display()))?,
            ReadTarget::MemoryUri(uri) => tokio_rusqlite::Connection::open_with_flags(uri, flags)
                .await
                .context("Failed to open in-memory read connection")?,
        };
        Self::dispatch(&conn, std::panic::Location::caller(), |c| {
            c.execute_batch(CONNECTION_PRAGMAS)
                .context("Failed to set reader PRAGMAs")
        })
        .await?;
        Ok(conn)
    }

    /// Shared dispatch body for [`Database::db_call`] and
    /// [`Database::db_call_read`]: run `f` on `conn`, time it, warn on slow
    /// calls, and translate `tokio_rusqlite`'s boxed error back to
    /// `anyhow::Error`.
    async fn dispatch<R, F>(
        conn: &tokio_rusqlite::Connection,
        caller: &'static std::panic::Location<'static>,
        f: F,
    ) -> Result<R>
    where
        F: FnOnce(&mut Connection) -> Result<R> + Send + 'static,
        R: Send + 'static,
    {
        let start = std::time::Instant::now();
        let result = conn
            .call(move |c| f(c).map_err(|e| tokio_rusqlite::Error::Other(Box::new(AnyhowErr(e)))))
            .await;
        let elapsed = start.elapsed();
        if elapsed > SLOW_DB_CALL_THRESHOLD {
            tracing::warn!(
                duration_ms = elapsed.as_millis() as u64,
                location = %caller,
                "slow db_call"
            );
        }
        match result {
            Ok(value) => Ok(value),
            Err(tokio_rusqlite::Error::Other(other)) => match other.downcast::<AnyhowErr>() {
                Ok(boxed) => Err(boxed.0),
                Err(other) => Err(anyhow::anyhow!(other.to_string())),
            },
            Err(e) => Err(anyhow::Error::from(e)),
        }
    }

    /// Run a synchronous closure against the writer connection from an async
    /// context, returning its result without blocking the Tokio worker. Use
    /// for any closure that writes (`execute`/`execute_batch`/INSERT/UPDATE/
    /// DELETE) or that must observe its own prior write in the same call.
    ///
    /// The closure receives a `&mut rusqlite::Connection` (the dedicated
    /// thread owned by [`tokio_rusqlite::Connection`]). It must be
    /// `Send + 'static`, so any borrowed parameters need to be cloned to
    /// owned values before being moved in.
    ///
    /// Errors returned from the closure are wrapped in
    /// [`tokio_rusqlite::Error::Other`] and surfaced as `anyhow::Error`.
    ///
    /// Written as a plain fn returning `impl Future` (not `async fn`) so
    /// `#[track_caller]` actually captures the caller's location:
    /// `#[track_caller]` on `async fn` is a no-op on stable Rust
    /// (rust-lang/rust#110011), since the location would otherwise be
    /// captured when the returned future is first polled, not when it's
    /// created at the call site. Every existing call site is unaffected —
    /// `db.db_call(closure).await` still reads identically.
    #[track_caller]
    pub fn db_call<R, F>(&self, f: F) -> impl std::future::Future<Output = Result<R>> + '_
    where
        F: FnOnce(&mut Connection) -> Result<R> + Send + 'static,
        R: Send + 'static,
    {
        let caller = std::panic::Location::caller();
        async move { Self::dispatch(&self.conn, caller, f).await }
    }

    /// Run a synchronous **read-only** closure against a pooled read
    /// connection, round-robin. Use only for closures that issue no writes —
    /// pool connections are opened `SQLITE_OPEN_READ_ONLY`, so a write
    /// attempted here fails loudly (`SQLITE_READONLY`) rather than silently
    /// succeeding or corrupting state.
    ///
    /// Pool slots are opened lazily (on first use, via
    /// `tokio::sync::OnceCell`) rather than eagerly at `Database::open` time —
    /// most callers (one-shot CLI subcommands, most tests) never need more
    /// than one reader, and eagerly opening `READ_POOL_SIZE` connections (and
    /// OS threads — `tokio_rusqlite` dedicates one per connection) for every
    /// `Database` instance would be pure overhead for them. A reader that
    /// fails to open is retried on the next call that round-robins onto its
    /// slot, since `get_or_try_init` leaves a slot uninitialized on error.
    ///
    /// See [`Database::db_call`] for why this is a plain fn, not `async fn`.
    #[track_caller]
    pub fn db_call_read<R, F>(&self, f: F) -> impl std::future::Future<Output = Result<R>> + '_
    where
        F: FnOnce(&mut Connection) -> Result<R> + Send + 'static,
        R: Send + 'static,
    {
        let caller = std::panic::Location::caller();
        let idx = self
            .next_reader
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            % self.read_pool.len();
        async move {
            let conn = self.read_pool[idx]
                .get_or_try_init(|| Self::open_reader(&self.read_target))
                .await?;
            Self::dispatch(conn, caller, f).await
        }
    }

    /// Deliberately does NOT go through [`Database::dispatch`] (unlike
    /// `db_call`/`db_call_read`) even though the error-translation logic is
    /// identical: `dispatch` also emits the slow-db-call warning
    /// (`SLOW_DB_CALL_THRESHOLD`), and schema init/migrations are expected to
    /// occasionally run slow (e.g. under test-suite load) without tripping
    /// that instrumentation — it exists to catch abnormal *query* latency,
    /// not one-time startup cost. Tests that assert exact warning counts
    /// around `Database::open`/`open_in_memory` (see `db::tests::async_handle`)
    /// depend on `init_schema` never contributing to that count.
    async fn init_schema(conn: &tokio_rusqlite::Connection) -> Result<()> {
        conn.call(|c| {
            init_schema_sync(c).map_err(|e| tokio_rusqlite::Error::Other(Box::new(AnyhowErr(e))))
        })
        .await
        .map_err(|e| match e {
            tokio_rusqlite::Error::Other(other) => match other.downcast::<AnyhowErr>() {
                Ok(boxed) => boxed.0,
                Err(other) => anyhow::anyhow!(other.to_string()),
            },
            other => anyhow::Error::from(other),
        })
    }
}

/// PRAGMAs that apply the same way to any connection, reader or writer:
/// bounds how long a connection waits on SQLite's lock before giving up, caps
/// the page cache, and keeps temp b-trees/sorting in memory rather than a
/// temp file. Shared between `init_schema_sync` (writer) and `open_reader`
/// (pool connections) so retuning one doesn't silently leave the other stale.
const CONNECTION_PRAGMAS: &str = "PRAGMA busy_timeout=5000;
     PRAGMA cache_size=-8000;
     PRAGMA temp_store=MEMORY;";

fn init_schema_sync(conn: &Connection) -> Result<()> {
    conn.execute_batch(&format!(
        "PRAGMA journal_mode=WAL;
         PRAGMA foreign_keys=ON;
         PRAGMA synchronous=NORMAL;
         {CONNECTION_PRAGMAS}"
    ))
    .context("Failed to set PRAGMAs")?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tasks (
            id          INTEGER PRIMARY KEY,
            title       TEXT NOT NULL,
            description TEXT NOT NULL,
            repo_path   TEXT NOT NULL,
            status      TEXT NOT NULL DEFAULT 'backlog',
            worktree    TEXT,
            tmux_window TEXT,
            plan_path   TEXT,
            tag         TEXT,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE TABLE IF NOT EXISTS repo_paths (
            id             INTEGER PRIMARY KEY,
            path           TEXT NOT NULL UNIQUE,
            last_used      TEXT NOT NULL DEFAULT (datetime('now')),
            verify_command TEXT
        );",
    )
    .context("Failed to create schema")?;

    // v71's dedup migration is destructive (deletes task rows), so it takes
    // an on-disk backup first via `VACUUM INTO` — but `VACUUM` can't run
    // inside a transaction, so that has to happen here, before
    // `apply_pending_migrations` opens its transaction. The file-existence
    // check inside `migrate_v71_create_backup` keeps this a no-op on every
    // open after the first. Kept out of the generic `apply_pending_migrations`
    // (rather than being version-gated there) so that helper stays
    // reusable/testable without baking in a specific migration's version
    // number. (Re-reading `user_version` here and again in
    // `apply_pending_migrations` is deliberate, not an oversight — the two
    // functions are independently callable, e.g. from tests.)
    let current_version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if current_version < 71 {
        migrations::migrate_v71_create_backup(conn).context("Failed to create pre-v71 backup")?;
    }

    apply_pending_migrations(conn, migrations::MIGRATIONS)
}

/// Applies every migration in `migrations` whose version exceeds the current
/// `user_version`, atomically with the `user_version` bump: the whole
/// catch-up (every pending migration's DDL, plus the final version) runs
/// inside one `BEGIN IMMEDIATE` transaction, which re-reads the version once
/// it has the write lock. `BEGIN IMMEDIATE` acquires that lock up front, so
/// if a second connection reaches this function concurrently it blocks (per
/// `busy_timeout`) until the first transaction commits, then finds the
/// version already caught up and applies nothing — closing the race
/// described in task #3724, where a bare `migrate_fn(conn)?` followed by a
/// separate `pragma_update` let two connections both observe the old version
/// and both apply the same migration.
///
/// Uses raw `BEGIN IMMEDIATE`/`COMMIT`/`ROLLBACK` via `execute_batch` (the
/// same idiom as `delete_epic` in `src/db/queries/epics.rs`) rather than
/// rusqlite's `Transaction` wrapper, so this — and `migrate_fn` — keep taking
/// a plain `&Connection` instead of forcing `&mut Connection` up through
/// every caller.
///
/// `PRAGMA foreign_keys` is a no-op once a transaction is open, and `BEGIN`
/// inside an already-open transaction is a SQLite error — so the toggle (used
/// by migrations that rebuild a table to work around SQLite's lack of `DROP
/// COLUMN`) has to bracket this transaction rather than live inside
/// `migrate_fn`, which is why it's unconditional here rather than owned by
/// individual migration bodies.
fn apply_pending_migrations(conn: &Connection, migrations: &[migrations::Migration]) -> Result<()> {
    let current_version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if migrations
        .iter()
        .all(|&(version, _)| current_version >= version)
    {
        return Ok(());
    }

    conn.execute_batch("PRAGMA foreign_keys = OFF")
        .context("Failed to disable foreign keys for migrations")?;
    conn.execute_batch("BEGIN IMMEDIATE")
        .context("Failed to start migration transaction")?;

    let result = (|| -> Result<()> {
        let current_version: i64 =
            conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        for &(version, migrate_fn) in migrations {
            if current_version < version {
                migrate_fn(conn)
                    .with_context(|| format!("Migration to version {version} failed"))?;
                conn.pragma_update(None, "user_version", version)
                    .with_context(|| format!("Failed to update schema version to {version}"))?;
            }
        }
        Ok(())
    })();

    match result {
        Ok(()) => conn
            .execute_batch("COMMIT")
            .context("Failed to commit migrations")?,
        Err(e) => {
            conn.execute_batch("ROLLBACK").ok(); // ignore rollback error; preserves and returns the original error
            return Err(e);
        }
    }

    conn.execute_batch("PRAGMA foreign_keys = ON")
        .context("Failed to re-enable foreign keys after migrations")?;

    Ok(())
}
