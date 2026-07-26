//! `TaskService` — CRUD and lifecycle operations for tasks.
//!
//! Pure parameter shapes live in `params.rs`; the patch builder lives in
//! `validators.rs`. Methods that read or mutate DB state are kept here
//! because they need `&self`.

use std::sync::Arc;

use crate::db::{self, CreateTaskRequest, TaskPatch};
use crate::models::{
    classify_agent_activity, EpicId, HookEventKind, NotificationBehavior, SubStatus, Task, TaskId,
    TaskStatus, DEFAULT_BASE_BRANCH,
};
use crate::service::ServiceError;

use super::params::{ClaimTaskParams, CreateTaskParams, ListTasksFilter, UpdateTaskParams};
use super::validators::build_task_patch;
use crate::service::UrlUpdate;

/// Result of [`TaskService::update_task`]. Carries the updated task id plus
/// presentation-relevant transition flags so MCP handlers can format their
/// response without re-reading the DB.
#[derive(Debug, Clone)]
pub struct UpdateTaskResult {
    pub task_id: TaskId,
    /// `true` when the same call set a PR-typed `url` on a task that
    /// previously had no url AND moved its status to Review.
    pub was_pr_finalisation: bool,
}

pub struct TaskService {
    pub db: Arc<dyn db::TaskAndEpicStore>,
    clock: Arc<dyn crate::service::Clock>,
    pub(super) runner: Arc<dyn crate::process::ProcessRunner>,
}

impl TaskService {
    /// Construct a `TaskService` with an explicitly chosen process runner.
    ///
    /// Required, not defaulted: `TaskService` really does shell out (see
    /// `watchers.rs`), so a default would let a test silently touch the host.
    /// Tests pass [`MockProcessRunner::unused`](crate::process::MockProcessRunner::unused);
    /// production says so by name via
    /// [`new_with_real_runner`](Self::new_with_real_runner).
    pub fn new(
        db: Arc<dyn db::TaskAndEpicStore>,
        runner: Arc<dyn crate::process::ProcessRunner>,
    ) -> Self {
        Self {
            db,
            clock: Arc::new(crate::service::SystemClock),
            runner,
        }
    }

    /// Construct a `TaskService` that shells out for real. Named so that the
    /// non-hermetic choice is visible at the call site; see [`new`](Self::new).
    pub fn new_with_real_runner(db: Arc<dyn db::TaskAndEpicStore>) -> Self {
        Self::new(db, Arc::new(crate::process::RealProcessRunner))
    }

    /// Override the clock used for timestamping. Tests inject a
    /// [`FixedClock`](crate::service::FixedClock) so timestamp-dependent flows
    /// (hook-event ordering) are deterministic without sleeping.
    ///
    /// Unlike the runner, this stays an optional builder on purpose:
    /// `SystemClock` only reads the wall clock, so an un-injected default
    /// costs determinism, never a real side effect.
    pub fn with_clock(mut self, clock: Arc<dyn crate::service::Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Updates a task. Used by MCP handlers and internal dispatch flows.
    ///
    /// Supports the full `UpdateTaskParams` builder (title, description,
    /// repo_path, pr_url, worktree, tmux_window, base_branch, sub_status,
    /// epic_id, sort_order, tag, status). Calls `recalculate_epic_for_task`
    /// whenever `params.status` is set.
    ///
    /// Use [`cli_update_task`](Self::cli_update_task) for CLI subcommands
    /// that need to archive tasks.
    pub async fn update_task(
        &self,
        params: UpdateTaskParams,
    ) -> Result<UpdateTaskResult, ServiceError> {
        if !params.has_any_field() {
            return Err(ServiceError::Validation(
                "At least one field must be provided".into(),
            ));
        }

        let task_id = params.task_id;
        let expanded_repo_path = params.repo_path.as_deref().map(crate::models::expand_tilde);
        let validated_sub_status = self.validate_sub_status(task_id, &params).await?;

        let patch = build_task_patch(&params, expanded_repo_path.as_deref(), validated_sub_status);

        // Snapshot the task before the patch so we can detect the
        // null-url → PR-set transition without an extra round-trip later.
        // Skip the read entirely unless this update both moves to Review
        // and sets a PR-typed url — the only shape that can be a finalisation —
        // or relinks the task to a different epic (also wants the prior).
        let is_pr_url_set = matches!(
            params.url.as_ref(),
            Some(UrlUpdate::Set(u)) if u.is_pr()
        );
        let is_finishing_status =
            matches!(params.status, Some(TaskStatus::Done | TaskStatus::Archived));
        let needs_prior = params.epic_id.is_some()
            || (params.status == Some(TaskStatus::Review) && is_pr_url_set)
            || is_finishing_status;
        let prior = if needs_prior {
            self.db.get_task(task_id).await?
        } else {
            None
        };
        let was_pr_finalisation = params.status == Some(TaskStatus::Review)
            && is_pr_url_set
            && prior.as_ref().is_some_and(|t| t.url.is_none());

        // Resolve grouping target for an explicit epic relink (before the write).
        let routed_epic_id = if params.epic_id.is_some() {
            let repo = expanded_repo_path
                .clone()
                .or_else(|| prior.as_ref().map(|t| t.repo_path.clone()))
                .unwrap_or_default();
            self.resolve_routed_epic(params.epic_id, &repo).await?
        } else {
            None
        };

        self.db.patch_task(task_id, &patch).await?;

        self.notify_watchers_after_status_write(prior.as_ref(), params.status)
            .await;

        if let Some(routed_id) = routed_epic_id {
            let old_epic_id = prior.as_ref().and_then(|t| t.epic_id);
            self.db.set_task_epic_id(task_id, Some(routed_id)).await?;
            if let Some(old) = old_epic_id {
                self.recalculate_epic(old).await;
            }
            self.recalculate_epic(routed_id).await;
        }

        // Repo changed without an explicit relink: re-route within a grouped subtree.
        if params.epic_id.is_none() {
            if let Some(ref new_repo) = expanded_repo_path {
                crate::service::reroute_on_repo_change(&*self.db, task_id, new_repo).await?;
            }
        }

        if params.status.is_some() {
            self.recalculate_epic_for_task(task_id).await;
        }

        Ok(UpdateTaskResult {
            task_id,
            was_pr_finalisation,
        })
    }

    /// Move a task to a different epic, or detach it to standalone when
    /// `new_epic` is `None`. Validates that a chosen target epic exists, then
    /// routes through `route_target` (so a task moved onto a `group_by_repo`
    /// non-feed root lands in the correct per-repo sub-epic), and recalculates
    /// the status of both the previous epic (if any) and the new epic (if any)
    /// per the epic-status-recalculation invariant.
    pub async fn move_task_to_epic(
        &self,
        task_id: TaskId,
        new_epic: Option<EpicId>,
    ) -> Result<(), ServiceError> {
        // A chosen target must exist; a null target detaches the task.
        // Validate against the ORIGINAL requested epic (route_target may
        // create/return a sub-epic, but the caller's intent is this epic).
        if let Some(epic_id) = new_epic {
            if self.db.get_epic(epic_id).await?.is_none() {
                return Err(ServiceError::NotFound(format!(
                    "Epic {} not found",
                    epic_id.0
                )));
            }
        }

        let task = self
            .db
            .get_task(task_id)
            .await?
            .ok_or_else(|| ServiceError::NotFound(format!("Task {} not found", task_id.0)))?;
        let old_epic_id = task.epic_id;

        // When moving to a target, route through grouping logic so a task
        // dropped onto a group_by_repo (non-feed) root lands in its per-repo
        // sub-epic instead of directly on the root. Detach (None) is unchanged.
        let routed_epic = self.resolve_routed_epic(new_epic, &task.repo_path).await?;

        self.db.set_task_epic_id(task_id, routed_epic).await?;

        if let Some(old) = old_epic_id {
            self.recalculate_epic(old).await;
        }
        if let Some(routed) = routed_epic {
            self.recalculate_epic(routed).await;
        }
        Ok(())
    }

    /// Validate `params.sub_status` against the task's effective (current or
    /// requested) status. Returns the sub_status to write, if any.
    ///
    /// Intentional TOCTOU: we read the current status here to validate the
    /// sub_status, then write via patch_task in `update_task` afterwards. A
    /// concurrent update between the two is theoretically possible but benign
    /// in practice — Dispatch is a single-process tokio runtime with
    /// cooperative scheduling, so no two MCP handlers run truly concurrently
    /// on the same task. SQLite serialises writes regardless.
    async fn validate_sub_status(
        &self,
        task_id: TaskId,
        params: &UpdateTaskParams,
    ) -> Result<Option<SubStatus>, ServiceError> {
        let Some(ss) = params.sub_status else {
            return Ok(None);
        };
        let effective_status = match params.status {
            Some(s) => Some(s),
            None => self
                .db
                .get_task(task_id)
                .await
                .ok()
                .flatten()
                .map(|t| t.status),
        };
        if let Some(eff) = effective_status {
            if !ss.is_valid_for(eff) {
                return Err(ServiceError::Validation(format!(
                    "sub_status '{}' is not valid for status '{}'",
                    ss.as_str(),
                    eff.as_str()
                )));
            }
        }
        Ok(Some(ss))
    }

    /// Resolve the actual epic a task should land in: if `epic_id` targets a
    /// group_by_repo (non-feed) epic, route into its per-repo sub-epic; else
    /// return `epic_id` unchanged. `None` stays `None`.
    async fn resolve_routed_epic(
        &self,
        epic_id: Option<EpicId>,
        repo_path: &str,
    ) -> Result<Option<EpicId>, ServiceError> {
        match epic_id {
            Some(id) => Ok(Some(
                crate::service::route_target(&*self.db, id, repo_path).await?,
            )),
            None => Ok(None),
        }
    }

    /// Recalculate the given epic, logging any database error.
    async fn recalculate_epic(&self, epic_id: EpicId) {
        if let Err(err) = self.db.recalculate_epic_status(epic_id).await {
            tracing::warn!(
                "failed to recalculate epic status for epic {}: {err}",
                epic_id.0
            );
        }
    }

    /// Recalculate the parent epic of the given task, if it has one.
    async fn recalculate_epic_for_task(&self, task_id: TaskId) {
        if let Ok(Some(task)) = self.db.get_task(task_id).await {
            if let Some(epic_id) = task.epic_id {
                self.recalculate_epic(epic_id).await;
            }
        }
    }

    /// After a status-affecting write, notify watchers if `new_status` is a
    /// finishing status (`Done`/`Archived`) the write actually transitioned
    /// into. Shared by `update_task` and `cli_update_task` so both callers
    /// funnel through one call with one ordering relative to epic
    /// recalculation, instead of each re-deriving this check. `prior` is the
    /// task as fetched before the write — pass `None` when the caller didn't
    /// need to fetch it (this no-ops immediately in that case, since a
    /// finishing transition always requires `prior` to have been fetched).
    async fn notify_watchers_after_status_write(
        &self,
        prior: Option<&Task>,
        new_status: Option<TaskStatus>,
    ) {
        let Some(new_status) = new_status else {
            return;
        };
        if !matches!(new_status, TaskStatus::Done | TaskStatus::Archived) {
            return;
        }
        let Some(prior) = prior else { return };
        self.notify_watchers_if_finished(prior, new_status).await;
    }

    /// Updates a task status from a CLI subcommand (human operator path).
    ///
    /// **Caller:** `src/main.rs` CLI subcommands (`dispatch update`, etc.).
    ///
    /// **Differences from [`update_task`](Self::update_task):**
    /// - Can transition to any status including `Done` and `Archived`.
    /// - Supports conditional update: `only_if` skips the write if the current
    ///   status doesn't match, returning `Ok(false)` instead of an error.
    /// - Accepts only status + sub_status — not the full field builder.
    ///
    /// Use `update_task` for agent/MCP call sites that must not complete tasks.
    pub async fn cli_update_task(
        &self,
        task_id: TaskId,
        new_status: TaskStatus,
        only_if: Option<TaskStatus>,
        sub_status: Option<SubStatus>,
    ) -> Result<bool, ServiceError> {
        let is_finishing_status = matches!(new_status, TaskStatus::Done | TaskStatus::Archived);
        let prior = if is_finishing_status {
            self.db.get_task(task_id).await?
        } else {
            None
        };

        let updated = if let Some(expected) = only_if {
            let changed = self
                .db
                .update_status_if(task_id, new_status, expected)
                .await?;
            if changed {
                if let Some(ss) = sub_status {
                    self.db
                        .patch_task(task_id, &crate::db::TaskPatch::new().sub_status(ss))
                        .await?;
                }
            }
            changed
        } else {
            let mut patch = crate::db::TaskPatch::new().status(new_status);
            if let Some(ss) = sub_status {
                patch = patch.sub_status(ss);
            }
            self.db.patch_task(task_id, &patch).await?;
            true
        };

        if updated {
            self.notify_watchers_after_status_write(prior.as_ref(), Some(new_status))
                .await;
            self.recalculate_epic_for_task(task_id).await;
        }

        Ok(updated)
    }

    /// Attach a plan file (by absolute path) to a task. Used by the `plan`
    /// CLI subcommand so plan attachment routes through the service like its
    /// siblings, rather than writing to the DB directly. Plan attachment
    /// carries no epic-status invariant, so no recalculation is needed.
    pub async fn attach_plan(&self, task_id: TaskId, plan_path: &str) -> Result<(), ServiceError> {
        // Verify the task exists so a bad id surfaces as NotFound rather than
        // a silent no-op.
        self.get_task(task_id).await?;
        self.db
            .patch_task(task_id, &TaskPatch::new().plan_path(Some(plan_path)))
            .await
            .map_err(ServiceError::from)
    }

    pub async fn create_task(&self, params: CreateTaskParams) -> Result<TaskId, ServiceError> {
        Ok(self.create_task_returning(params).await?.id)
    }

    /// Create a task and return the full Task object (used by TUI).
    pub async fn create_task_returning(
        &self,
        params: CreateTaskParams,
    ) -> Result<Task, ServiceError> {
        let repo_path = crate::models::expand_tilde(&params.repo_path);

        let plan = params.plan_path.as_deref().map(|p| {
            std::fs::canonicalize(p)
                .map(|abs| abs.to_string_lossy().into_owned())
                .unwrap_or_else(|_| p.to_string())
        });

        let base_branch = params.base_branch.as_deref().unwrap_or(DEFAULT_BASE_BRANCH);

        // Repo-grouping: a task assigned to a group_by_repo (non-feed) epic is
        // placed into its per-repo sub-epic instead of the parent.
        let effective_epic_id = self.resolve_routed_epic(params.epic_id, &repo_path).await?;

        let task_id = self
            .db
            .create_task(CreateTaskRequest {
                title: &params.title,
                description: &params.description,
                repo_path: &repo_path,
                plan: plan.as_deref(),
                status: TaskStatus::Backlog,
                base_branch,
                epic_id: effective_epic_id,
                sort_order: params.sort_order,
                tag: params.tag,
                wrap_up_mode: params.wrap_up_mode,
                auto_run_plan: params.auto_run_plan,
            })
            .await?;

        if let Some(eid) = effective_epic_id {
            self.recalculate_epic(eid).await;
        }

        self.get_task(task_id).await
    }

    pub async fn delete_task(&self, task_id: TaskId) -> Result<(), ServiceError> {
        let task = self.get_task(task_id).await?;
        self.notify_watchers_of_deletion(&task).await;

        self.db
            .delete_task(task_id)
            .await
            .map_err(ServiceError::from)
    }

    pub async fn get_task(&self, task_id: TaskId) -> Result<Task, ServiceError> {
        self.db
            .get_task(task_id)
            .await?
            .ok_or_else(|| ServiceError::NotFound(format!("Task {} not found", task_id.0)))
    }

    /// Batch-update `sub_status` for many tasks in a single transaction.
    ///
    /// `sub_status` is derived board state (activity classification), not a
    /// status/epic-linkage change, so it carries no `recalculate_epic_status`
    /// obligation — but it still routes through the service so consumers
    /// (the tick) never touch the DB write surface directly.
    pub async fn batch_patch_sub_status(
        &self,
        updates: &[(TaskId, crate::models::SubStatus)],
    ) -> Result<(), ServiceError> {
        self.db
            .batch_patch_sub_status(updates)
            .await
            .map_err(ServiceError::from)
    }

    pub async fn list_tasks(&self, filter: ListTasksFilter) -> Result<Vec<Task>, ServiceError> {
        let tasks = self.db.list_all().await?;

        let filtered: Vec<_> = tasks
            .into_iter()
            .filter(|t| match &filter.statuses {
                Some(statuses) => statuses.contains(&t.status),
                None => t.status != TaskStatus::Archived,
            })
            .filter(|t| match filter.epic_id {
                Some(eid) => t.epic_id == Some(eid),
                None => true,
            })
            .filter(|t| match &filter.repo_paths {
                Some(paths) => paths.iter().any(|p| p == &t.repo_path),
                None => true,
            })
            .filter(|t| match filter.exclude_task_id {
                Some(excluded) => t.id != excluded,
                None => true,
            })
            .collect();

        Ok(filtered)
    }

    pub async fn claim_task(&self, params: ClaimTaskParams) -> Result<Task, ServiceError> {
        let task = self.get_task(params.task_id).await?;

        if task.status != TaskStatus::Backlog {
            return Err(ServiceError::Validation(format!(
                "Task {} is already {}",
                params.task_id,
                task.status.as_str()
            )));
        }

        // Same-repo check: derive repo from worktree by stripping /.worktrees/<anything>
        let repo_from_worktree = params
            .worktree
            .find("/.worktrees/")
            .map(|idx| &params.worktree[..idx])
            .unwrap_or(&params.worktree);

        if repo_from_worktree != task.repo_path {
            return Err(ServiceError::Validation(format!(
                "Repo mismatch: task belongs to {}, your worktree is in {}",
                task.repo_path, repo_from_worktree
            )));
        }

        // Seed last_pre_tool_use_at so ClassifyAgentActivity treats the
        // freshly running task as Active until the agent's first PreToolUse
        // hook fires — otherwise it flickers into Stale on the next tick.
        self.db
            .patch_task(
                params.task_id,
                &TaskPatch::new()
                    .status(TaskStatus::Running)
                    .worktree(Some(&params.worktree))
                    .tmux_window(Some(&params.tmux_window))
                    .last_pre_tool_use_at(Some(self.clock.now())),
            )
            .await?;

        // Backlog → Running changes the parent epic's rolled-up status; see
        // the EpicStatusRecalculation invariant in docs/specs/epics.allium.
        // `task.epic_id` is still current: `TaskPatch` cannot carry epic_id
        // (linkage moves only via `set_task_epic_id`), so there is no need to
        // re-read the task the way `recalculate_epic_for_task` would.
        if let Some(epic_id) = task.epic_id {
            self.recalculate_epic(epic_id).await;
        }

        // Return the post-patch row: `task` above is the pre-patch snapshot
        // and would still report Backlog with no worktree.
        self.get_task(params.task_id).await
    }

    pub async fn validate_wrap_up(&self, task_id: TaskId) -> Result<Task, ServiceError> {
        let task = self.get_task(task_id).await?;

        if !crate::dispatch::is_wrappable(&task) {
            return Err(ServiceError::Validation(format!(
                "Task {} cannot be wrapped up. Requires Running or Review status with a worktree.",
                task_id.0
            )));
        }

        Ok(task)
    }

    pub async fn validate_send_message(
        &self,
        from_task_id: TaskId,
        to_task_id: TaskId,
    ) -> Result<(Task, Task), ServiceError> {
        let from_task = self.db.get_task(from_task_id).await?.ok_or_else(|| {
            ServiceError::NotFound(format!("sender task {} not found", from_task_id.0))
        })?;

        let to_task = self.db.get_task(to_task_id).await?.ok_or_else(|| {
            ServiceError::NotFound(format!("target task {} not found", to_task_id.0))
        })?;

        if to_task.worktree.is_none() {
            return Err(ServiceError::Validation(format!(
                "target task {} has no worktree (not running)",
                to_task_id.0
            )));
        }

        if to_task.tmux_window.is_none() {
            return Err(ServiceError::Validation(format!(
                "target task {} has no tmux window (not running)",
                to_task_id.0
            )));
        }

        Ok((from_task, to_task))
    }

    /// Record a Claude Code hook event for a task.
    ///
    /// `Stop` transitions Running → Review and clears both timestamps.
    /// `PreToolUse`/`Notification` stamp their timestamp and reclassify
    /// `sub_status`; both are no-ops on non-Running tasks. `UserPromptSubmit`
    /// additionally resumes a Review task straight to Running — it is the
    /// earliest signal that the human has continued the conversation — and
    /// is a no-op outside {Running, Review}.
    pub async fn record_hook_event(
        &self,
        id: TaskId,
        kind: HookEventKind,
    ) -> Result<(), ServiceError> {
        let task = self
            .db
            .get_task(id)
            .await?
            .ok_or_else(|| ServiceError::NotFound(format!("Task {} not found", id.0)))?;
        let now = self.clock.now();
        let was_review = task.status == TaskStatus::Review;
        let patch = match kind {
            HookEventKind::PreToolUse if task.status == TaskStatus::Running => {
                let activity = classify_agent_activity(Some(now), task.last_notification_at, now);
                Some(
                    TaskPatch::new()
                        .last_pre_tool_use_at(Some(now))
                        .sub_status(activity.to_sub_status()),
                )
            }
            HookEventKind::Notification(notification_kind)
                if task.status == TaskStatus::Running =>
            {
                Some(match NotificationBehavior::from_kind(notification_kind) {
                    // Clear: an elicitation just resolved, so the block is
                    // over. Mirror the PreToolUse resume path — back to
                    // active, drop the notification timestamp — so the card
                    // flips back the instant the user answers, without
                    // waiting for the next PreToolUse.
                    NotificationBehavior::Clear => TaskPatch::new()
                        .sub_status(SubStatus::default_for(TaskStatus::Running))
                        .last_notification_at(None),
                    // Ignore: an auth-success toast is informational only.
                    // Empty patch — sub_status and last_notification_at are
                    // left untouched, so it never raises a false needs_input.
                    NotificationBehavior::Ignore => TaskPatch::new(),
                    // Raise: the agent is genuinely blocked (or the kind is
                    // absent/unrecognised — compat default). See
                    // `NotificationBehavior::from_kind`.
                    NotificationBehavior::Raise => TaskPatch::new()
                        .last_notification_at(Some(now))
                        .sub_status(SubStatus::NeedsInput),
                })
            }
            HookEventKind::Stop if task.status == TaskStatus::Running => Some(
                TaskPatch::new()
                    .status(TaskStatus::Review)
                    .last_pre_tool_use_at(None)
                    .last_notification_at(None),
            ),
            HookEventKind::UserPromptSubmit if task.status == TaskStatus::Running || was_review => {
                Some(
                    TaskPatch::new()
                        .status(TaskStatus::Running)
                        .sub_status(SubStatus::default_for(TaskStatus::Running))
                        .last_pre_tool_use_at(Some(now)),
                )
            }
            _ => None,
        };
        let Some(patch) = patch else {
            return Ok(());
        };
        self.db.patch_task(id, &patch).await?;
        if matches!(kind, HookEventKind::Stop)
            || (matches!(kind, HookEventKind::UserPromptSubmit) && was_review)
        {
            self.recalculate_epic_for_task(id).await;
        }
        Ok(())
    }

    /// Mark that the PR-learnings reminder has been shown for this task.
    ///
    /// Returns `true` if this call set the flag (first `gh pr create` →
    /// caller should block), `false` if it was already set or the task does
    /// not exist (caller should allow the PR). One-time reminder; no epic
    /// recalculation is involved.
    pub async fn mark_pr_learnings_gate_shown(&self, id: TaskId) -> Result<bool, ServiceError> {
        Ok(self.db.mark_pr_learnings_gate_shown(id).await?)
    }

    /// Find the next backlog task for an epic, sorted by sort_order then id.
    /// Returns `Ok(None)` if no backlog tasks remain.
    pub async fn next_backlog_task(&self, epic_id: EpicId) -> Result<Option<Task>, ServiceError> {
        // Verify the epic exists
        self.db
            .get_epic(epic_id)
            .await?
            .ok_or_else(|| ServiceError::NotFound(format!("Epic {} not found", epic_id.0)))?;

        let tasks = self.db.list_tasks_for_epic(epic_id).await?;

        let mut backlog: Vec<Task> = tasks
            .into_iter()
            .filter(|t| t.status == TaskStatus::Backlog)
            .collect();
        backlog.sort_by_key(|t| (t.sort_order.unwrap_or(t.id.0), t.id.0));

        Ok(backlog.into_iter().next())
    }
}
