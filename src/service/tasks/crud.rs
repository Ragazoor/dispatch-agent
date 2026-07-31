//! `TaskService` — CRUD and lifecycle operations for tasks.
//!
//! Pure parameter shapes live in `params.rs`; the patch builder lives in
//! `validators.rs`. Methods that read or mutate DB state are kept here
//! because they need `&self`.

use std::sync::Arc;

use crate::db::{self, CreateTaskRequest, TaskPatch};
use crate::models::{
    classify_agent_activity, sort_order_for_status_transition, EpicId, HookEventKind,
    NotificationBehavior, SubStatus, Task, TaskId, TaskStatus, DEFAULT_BASE_BRANCH,
};
use crate::service::ServiceError;

use super::params::{CreateTaskParams, ListTasksFilter, UpdateTaskParams};
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
    /// Whether this call wrote `sort_order`, and to what.
    ///
    /// `None` means this call's patch didn't touch `sort_order` at all (the
    /// in-memory value the caller already holds is still current). `Some(v)`
    /// means the patch wrote `sort_order`, where `v` is exactly what was
    /// written — `Some(None)` for a clear, `Some(Some(x))` for a set to `x`.
    /// Callers that hold their own in-memory copy of the task (the TUI's
    /// `App.board.tasks`) use this to learn a value they could not have
    /// computed themselves: `sort_order_for_status_transition` runs inside
    /// this method, not at the call site.
    pub sort_order_after_write: Option<Option<i64>>,
}

/// What a session close makes of the task — the terminal status half of
/// `ExitSession` / `FinishTaskSuccess` (`docs/specs/pr-workflow.allium`).
#[derive(Debug, Clone)]
pub enum CloseSessionOutcome {
    /// `rebase` / `done`, and the TUI finish path: the task is finished.
    Done,
    /// `pr`: the task moves to Review carrying the PR url.
    Review { pr_url: crate::models::TaskUrl },
}

/// Result of [`TaskService::close_session`].
#[derive(Debug, Clone)]
pub struct ClosedSession {
    /// The tmux window the close cleared, or `None` if the task had none. The
    /// caller tears this window down — and only ever reaches it by holding an
    /// `Ok`, which is the point of the call.
    pub window: Option<String>,
    /// Whether this close wrote `sort_order`, and to what — same contract as
    /// [`UpdateTaskResult::sort_order_after_write`]. The TUI holds its own copy
    /// of the task and cannot compute the completion-recency rank itself.
    pub sort_order_after_write: Option<Option<i64>>,
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

        let mut patch =
            build_task_patch(&params, expanded_repo_path.as_deref(), validated_sub_status);

        // Snapshot the task before the patch. Needed whenever `epic_id` is
        // relinked (existing reason), whenever `status` changes and sets a
        // PR-typed url (existing PR-finalisation check), and now whenever
        // `status` changes at all — to detect a transition into/out of Done
        // for the sort_order-on-completion rule below.
        let is_pr_url_set = matches!(
            params.url.as_ref(),
            Some(UrlUpdate::Set(u)) if u.is_pr()
        );
        let needs_prior = params.epic_id.is_some() || params.status.is_some();
        let prior = if needs_prior {
            self.db.get_task(task_id).await?
        } else {
            None
        };
        let was_pr_finalisation = params.status == Some(TaskStatus::Review)
            && is_pr_url_set
            && prior.as_ref().is_some_and(|t| t.url.is_none());

        // The Done-transition rule must win over anything the caller's
        // params already set for sort_order — exec_persist_task
        // (src/runtime/tasks.rs) unconditionally forwards whatever
        // sort_order is sitting on the in-memory Task struct alongside a
        // status change, so a defensive-only override would leave a task
        // that just left Done permanently pinned to the top of whatever
        // column it lands in next.
        if let (Some(new_status), Some(p)) = (params.status, prior.as_ref()) {
            if let Some(so) =
                sort_order_for_status_transition(p.status, new_status, self.clock.now())
            {
                patch = patch.sort_order(so);
            }
        }

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

        // Captured before the write so the caller can learn what this call
        // wrote to sort_order (including the Done-transition override just
        // above) without a second DB round-trip. See `UpdateTaskResult`.
        let sort_order_after_write = patch.sort_order;

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
            sort_order_after_write,
        })
    }

    /// Apply a session close: the terminal status, its default sub-status, the
    /// PR url when there is one, and the cleared `tmux_window`, as one patch.
    ///
    /// Purpose-built rather than expressed through [`Self::update_task`], and
    /// deliberately so. Callers gate the tmux teardown (and, for the MCP path,
    /// the epic chain) on this `Result`, which is only sound if `Err` means
    /// exactly "the terminal write did not land". `update_task` cannot promise
    /// that: its fallible follow-up steps (epic re-linking, repo rerouting) run
    /// *after* the patch, so an `Err` from it can mean "patch landed, follow-up
    /// failed" — and a caller reading that as a failed close would leave a live
    /// window on a task that is already done. Here the patch is the only
    /// fallible step, so the two can never disagree. See the `close_persisted`
    /// discussion in `ExitSession` (`docs/specs/pr-workflow.allium`).
    ///
    /// Returns the window the close cleared, for the caller to tear down.
    pub async fn close_session(
        &self,
        task_id: TaskId,
        outcome: CloseSessionOutcome,
    ) -> Result<ClosedSession, ServiceError> {
        let prior = self
            .db
            .get_task(task_id)
            .await?
            .ok_or_else(|| ServiceError::NotFound(format!("Task {} not found", task_id.0)))?;

        // `pr_url` is bound out here rather than inside the builder because
        // `TaskPatch` borrows it.
        let (status, pr_url) = match &outcome {
            CloseSessionOutcome::Done => (TaskStatus::Done, None),
            CloseSessionOutcome::Review { pr_url } => (TaskStatus::Review, Some(pr_url)),
        };

        let mut patch = db::TaskPatch::new()
            .status(status)
            .sub_status(SubStatus::default_for(status))
            .tmux_window(None);
        if let Some(url) = pr_url {
            patch = patch.url(Some(url));
        }
        // Same completion-recency rule `update_task` applies on a Done
        // transition, so a closed task sorts to the top of Done.
        if let Some(so) = sort_order_for_status_transition(prior.status, status, self.clock.now()) {
            patch = patch.sort_order(so);
        }

        let sort_order_after_write = patch.sort_order;

        self.db.patch_task(task_id, &patch).await?;

        // Everything past the write is infallible on purpose — see the doc
        // comment. The one-shot watcher notice fires on the Done transition
        // exactly as it does through `update_task`.
        self.notify_watchers_after_status_write(Some(&prior), Some(status))
            .await;
        self.recalculate_epic_for_task(task_id).await;

        Ok(ClosedSession {
            window: prior.tmux_window,
            sort_order_after_write,
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
        // Always fetched (not just for finishing statuses): needed to
        // detect a transition away from Done regardless of what the new
        // status is, per sort_order_for_status_transition.
        let prior = self.db.get_task(task_id).await?;
        let sort_order_override = prior
            .as_ref()
            .and_then(|p| sort_order_for_status_transition(p.status, new_status, self.clock.now()));

        let updated = if let Some(expected) = only_if {
            let changed = self
                .db
                .update_status_if(task_id, new_status, expected)
                .await?;
            if changed {
                let mut patch = crate::db::TaskPatch::new();
                if let Some(ss) = sub_status {
                    patch = patch.sub_status(ss);
                }
                if let Some(so) = sort_order_override {
                    patch = patch.sort_order(so);
                }
                if patch.has_changes() {
                    self.db.patch_task(task_id, &patch).await?;
                }
            }
            changed
        } else {
            let mut patch = crate::db::TaskPatch::new().status(new_status);
            if let Some(ss) = sub_status {
                patch = patch.sub_status(ss);
            }
            if let Some(so) = sort_order_override {
                patch = patch.sort_order(so);
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

    /// Select and atomically claim the epic's next backlog subtask.
    ///
    /// Returns the claimed task with its `Running` status applied, or `Ok(None)`
    /// when no backlog subtask remains. Selecting and claiming are a single
    /// conditional write ([`db::TaskStore::try_claim_next_backlog_task`]), so
    /// there is no window in which a concurrent caller can take the row this one
    /// picked: two concurrent callers claim two *different* subtasks, never the
    /// same one — the guarantee `AutoDispatchNextSubtask` in
    /// `docs/specs/epics.allium` depends on. `Ok(None)` therefore means exactly
    /// "no backlog subtask left", never "gave up under contention".
    ///
    /// Claiming moves the `Running` transition ahead of worktree provisioning,
    /// so the returned task has `worktree = None` until the dispatch completes.
    pub async fn claim_next_backlog_task(
        &self,
        epic_id: EpicId,
    ) -> Result<Option<Task>, ServiceError> {
        // Read only to honour the NotFound contract — the claim itself needs no
        // prior selection.
        self.db
            .get_epic(epic_id)
            .await?
            .ok_or_else(|| ServiceError::NotFound(format!("Epic {} not found", epic_id.0)))?;

        let now = self.clock.now();
        let Some(claimed_id) = self.db.try_claim_next_backlog_task(epic_id, now).await? else {
            return Ok(None);
        };
        self.recalculate_epic(epic_id).await;
        // Re-read rather than mirroring the claim's SET list here: the row is
        // the truth, and hand-copying it silently drifts (the DB also stamps
        // `updated_at`, which no in-memory copy would carry).
        Ok(self.db.get_task(claimed_id).await?)
    }

    /// Atomically claim one specific `Backlog` task for dispatch, moving it to
    /// `Running` before any provisioning happens. Returns whether the claim was
    /// won.
    ///
    /// The by-id twin of [`Self::claim_next_backlog_task`], and the guard every
    /// dispatch entry point takes ahead of provisioning — that is what makes
    /// `DispatchClaimExclusive` (`docs/specs/dispatch.allium`) hold across
    /// entry points rather than only between epic chains. `Ok(false)` means
    /// someone else got there first (or the task is gone); the caller must
    /// provision nothing and launch no agent.
    ///
    /// One conditional write ([`db::TaskStore::try_claim_backlog_task`]), sharing
    /// its SET list with the by-epic claim so "what a claim writes" has a single
    /// definition. Being one statement is what keeps the caller's side simple:
    /// the claim can never half-apply, so `Err` means nothing was written and
    /// there is no partial state for the caller to unwind.
    ///
    /// No `sort_order` recency rank is applied: this transition can neither reach
    /// nor leave `Done`, so `sort_order_for_status_transition` would return
    /// `None` regardless.
    pub async fn claim_backlog_task(&self, task_id: TaskId) -> Result<bool, ServiceError> {
        if !self
            .db
            .try_claim_backlog_task(task_id, self.clock.now())
            .await?
        {
            return Ok(false);
        }
        self.recalculate_epic_for_task(task_id).await;
        Ok(true)
    }

    /// Undo a claim on a subtask that was never provisioned, returning it to
    /// `Backlog` with the claim's activity stamp cleared.
    ///
    /// Conditional, mirroring [`Self::claim_next_backlog_task`]: it only fires
    /// while the task is still `Running` with no worktree. Provisioning can take
    /// a `git fetch`'s worth of wall time, so an unconditional revert would
    /// drag a task a human moved in that window back to `Backlog`. Returns
    /// whether the release applied.
    pub async fn release_claim(&self, task_id: TaskId) -> Result<bool, ServiceError> {
        let released = self.db.try_release_backlog_claim(task_id).await?;
        if released {
            self.recalculate_epic_for_task(task_id).await;
        }
        Ok(released)
    }
}
