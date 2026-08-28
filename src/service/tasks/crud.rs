//! `TaskService` — CRUD and lifecycle operations for tasks.
//!
//! Pure parameter shapes live in `params.rs`; the patch builder lives in
//! `validators.rs`. Methods that read or mutate DB state are kept here
//! because they need `&self`.

use std::sync::Arc;

use crate::db::{self, CreateTaskRequest, TaskPatch};
use crate::models::{
    classify_agent_activity, clears_pending_stop, sort_order_for_status_transition, EpicId,
    HookEventKind, NotificationBehavior, ShellEvent, StopOutcome, SubStatus, SubagentEvent, Task,
    TaskId, TaskStatus, UserPromptOutcome, DEFAULT_BASE_BRANCH,
};
use crate::service::ServiceError;

use super::params::{CreateTaskParams, ListTasksFilter, UpdateTaskParams};
use super::validators::build_task_patch;
use crate::service::UrlUpdate;

/// Add every field a status transition derives from the task's PRIOR status to
/// `patch`, so the derived writes land in the same `UPDATE` as the status.
///
/// Two rules live here, and they are the complete set — a new status-writing
/// service method gets both by calling this instead of remembering each:
///
/// - `sort_order`: set on entering Done, cleared on leaving it
///   (`sort_order_for_status_transition`).
/// - `stop_pending`: cleared on leaving Running (`clears_pending_stop`), which
///   is what keeps `PendingStopOnlyWhileRunning` (`docs/specs/core.allium`)
///   true and stops the tick reconciler flipping a re-Running card back out.
///
/// Deliberately NOT pushed down into `patch_task`'s SQL, which would make the
/// clear unbypassable for every caller: the invariant is spec'd per rule
/// (tasks.allium, mcp-task-tools.allium, pr-workflow.allium) and the service
/// layer is where this codebase keeps rules that read prior state. A generic
/// patch primitive silently rewriting a column the caller did not name is also
/// the wrong shape for a rule that only some transitions carry.
fn with_status_transition(
    patch: TaskPatch,
    prior: TaskStatus,
    next: TaskStatus,
    now: chrono::DateTime<chrono::Utc>,
) -> TaskPatch {
    let mut patch = patch;
    if let Some(so) = sort_order_for_status_transition(prior, next, now) {
        patch = patch.sort_order(so);
    }
    if clears_pending_stop(prior, next) {
        patch = patch.stop_pending(false);
    }
    patch
}

/// Parse a native `SendMessage` `to` value back to the `TaskId` dispatch
/// assigned it at launch (`--name task-<id>`, `session_name_flag` in
/// `src/dispatch/agents.rs`). Strips a disambiguating `" [ref]"` suffix
/// first — `ListAgents`/`SendMessage` append one when more than one live
/// agent answers to a name, even though dispatch's own `task-<id>` names are
/// unique by construction and should rarely need it — then delegates to
/// [`TmuxWindow::task_id`](crate::models::TmuxWindow::task_id), the same parser
/// tmux window names go through, so the two can't drift apart into two
/// spellings of one convention. `None` for any value that doesn't match this shape — a
/// message addressed to a session dispatch didn't launch, or one the sender
/// renamed itself away from.
fn parse_peer_message_target_name(to: &str) -> Option<TaskId> {
    let name = to.split(' ').next().unwrap_or(to);
    crate::models::TmuxWindow::parse(name)?.task_id()
}

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
/// `ExitSession` (`docs/specs/pr-workflow.allium`).
#[derive(Debug, Clone)]
pub enum CloseSessionOutcome {
    /// `rebase` / `done`: the task is finished.
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
    ///
    /// No `sort_order_after_write` twin of [`UpdateTaskResult`]'s: the sole
    /// caller is the MCP `exit_session` handler, which holds no in-memory copy
    /// of the task to write back to. It notifies task-changed and the board
    /// re-reads the row.
    pub window: Option<crate::models::TmuxWindow>,
}

pub struct TaskService {
    /// The wide bundle, not the `TaskAndEpicStore` the CRUD methods alone would
    /// need: [`dispatch`](Self::dispatch) runs the dispatch prologue, which
    /// reads the whole [`TaskReadStore`](db::TaskReadStore) surface (epic
    /// banner, learning injections and their retrieval records). `TaskStore` is
    /// `TaskAndEpicStore` plus that read bundle, so it is still the narrowest
    /// handle covering what this service actually calls — and one handle is
    /// what keeps the prologue's reads and the service's writes on the same
    /// database by construction.
    pub db: Arc<dyn db::TaskStore>,
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
    pub fn new(db: Arc<dyn db::TaskStore>, runner: Arc<dyn crate::process::ProcessRunner>) -> Self {
        Self {
            db,
            clock: Arc::new(crate::service::SystemClock),
            runner,
        }
    }

    /// Construct a `TaskService` that shells out for real. Named so that the
    /// non-hermetic choice is visible at the call site; see [`new`](Self::new).
    pub fn new_with_real_runner(db: Arc<dyn db::TaskStore>) -> Self {
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
    /// This is the only status-writing path on the service: it can transition
    /// to any status including `Done` and `Archived`. The restriction against
    /// agents completing their own tasks lives at the MCP handler layer, not
    /// here — the TUI needs the unrestricted surface.
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
        // column it lands in next. Applied after `build_task_patch` for that
        // reason.
        // The (prior, next) pair every status-derived rule in this method reads:
        // `Some` exactly when this call moves the status and the prior row was
        // readable. Bound once because the rules that consume it sit on both
        // sides of the write — `with_status_transition` before it, the phoenix
        // respawn after — and re-deriving it at each would let the two drift on
        // what counts as a status change.
        let status_transition = params
            .status
            .zip(prior.as_ref().map(|p| p.status))
            .map(|(next, prior)| (prior, next));
        if let Some((prior_status, new_status)) = status_transition {
            patch = with_status_transition(patch, prior_status, new_status, self.clock.now());
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

        // PhoenixRespawn (tasks.allium). After the status write, never as part
        // of it — see `respawn_phoenix`.
        if let Some((prior_status, new_status)) = status_transition {
            self.respawn_phoenix(task_id, prior_status, new_status)
                .await;
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
        // The same prior-status rules `update_task` applies: the
        // completion-recency rank on entering Done, and the deferred-Stop clear
        // — both outcomes here leave Running. What this close does NOT clear is
        // the task's subagent rows or `live_subagents`; see the ExitSession
        // guidance in `docs/specs/pr-workflow.allium`.
        patch = with_status_transition(patch, prior.status, status, self.clock.now());

        self.db.patch_task(task_id, &patch).await?;

        // Everything past the write is infallible on purpose — see the doc
        // comment. The one-shot watcher notice fires on the Done transition
        // exactly as it does through `update_task`.
        self.notify_watchers_after_status_write(Some(&prior), Some(status))
            .await;
        self.recalculate_epic_for_task(task_id).await;
        // PhoenixRespawn (tasks.allium), on the same terms: the rebase/done
        // branch lands in Done and respawns, the pr branch lands in Review and
        // does not. `respawn_phoenix` swallows its own failures, which is what
        // keeps this method's `Err` meaning exactly "the terminal write did not
        // land" — the property the caller gates its tmux teardown on.
        self.respawn_phoenix(task_id, prior.status, status).await;

        Ok(ClosedSession {
            window: prior.tmux_window,
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

    /// Expand `~` in a repo path and canonicalise a plan path, the same
    /// operator-supplied-path pre-processing every task creation does — shared
    /// by `create_task_returning` and `respawn_phoenix` so a future change to
    /// path handling can't update one call site and silently skip the other.
    fn normalize_repo_and_plan(
        repo_path: &str,
        plan_path: Option<&str>,
    ) -> (String, Option<String>) {
        let repo_path = crate::models::expand_tilde(repo_path);
        let plan = plan_path.map(|p| {
            std::fs::canonicalize(p)
                .map(|abs| abs.to_string_lossy().into_owned())
                .unwrap_or_else(|_| p.to_string())
        });
        (repo_path, plan)
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
    /// into. Shared by `update_task` and `close_session` so both callers funnel
    /// through one call with one ordering relative to epic recalculation,
    /// instead of each re-deriving this check. `prior` is the
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
        let (repo_path, plan) =
            Self::normalize_repo_and_plan(&params.repo_path, params.plan_path.as_deref());

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
                phoenix: params.phoenix,
            })
            .await?;

        if let Some(eid) = effective_epic_id {
            self.recalculate_epic(eid).await;
        }

        self.get_task(task_id).await
    }

    /// `PhoenixRespawn` (`docs/specs/tasks.allium`): a phoenix task entering
    /// Done creates a fresh Backlog copy of itself, and the flag MOVES to that
    /// copy.
    ///
    /// Called after — never as part of — the status write, by both of this
    /// service's status-writing paths ([`update_task`](Self::update_task) and
    /// [`close_session`](Self::close_session)). That ordering is
    /// `DoneOutranksTheRespawn`: the completion is what the operator asked for
    /// and the respawn is a follow-on, so a failure here must not roll the
    /// transition back — and, for `close_session` specifically, must not turn
    /// into an `Err` its caller would read as "the terminal write did not
    /// land". Hence the return type: every failure is logged at ERROR and
    /// swallowed.
    ///
    /// It reads the task back rather than taking the pre-patch snapshot,
    /// because the same call that completes a task may also have changed what
    /// the successor should inherit — `phoenix` itself included, which is what
    /// lets a single editor save turn the flag on and complete the task.
    ///
    /// `TheFlagIsTheReceipt`: the predecessor's flag is cleared only once the
    /// successor row exists. So a surviving flag in Done means the copy did not
    /// land (`Task::respawn_failed`), re-entering Done retries, and a
    /// Done -> Review -> Done round-trip cannot duplicate.
    async fn respawn_phoenix(&self, task_id: TaskId, prior: TaskStatus, next: TaskStatus) {
        // `transitions_to done`: an actual change of value, so a no-op patch
        // rewriting Done over Done never re-fires.
        if next != TaskStatus::Done || prior == TaskStatus::Done {
            return;
        }

        let task = match self.db.get_task(task_id).await {
            Ok(Some(t)) => t,
            Ok(None) => return,
            Err(e) => {
                tracing::error!(task_id = task_id.0, error = %e, "phoenix respawn: re-read failed");
                return;
            }
        };
        // `FeedTasksAreExempt`: a feed epic reconciles its own rows, so a copy
        // would either duplicate the row the feed is about to re-ingest or be
        // deleted as stale by that same reconciliation.
        if !task.phoenix || task.external_id.is_some() {
            return;
        }

        // Mirrors `create_task_returning`'s own pre-processing (repo-group
        // routing, and path normalisation via the same shared helper), but
        // the actual writes go through `respawn_phoenix_successor` instead of
        // `create_task` so the successor's creation and the predecessor's
        // flag clear land in one transaction — see that method's doc comment
        // for why: `TheFlagIsTheReceipt` needs to be literally atomic, not
        // just usually true, or a failure between the two steps leaves a
        // landed successor with the flag still set, and re-entering Done
        // creates a second one.
        let (repo_path, plan) =
            Self::normalize_repo_and_plan(&task.repo_path, task.plan_path.as_deref());
        let effective_epic_id = match self.resolve_routed_epic(task.epic_id, &repo_path).await {
            Ok(id) => id,
            Err(e) => {
                tracing::error!(
                    task_id = task_id.0,
                    error = %e,
                    "phoenix respawn failed; the task stays Done and keeps its flag"
                );
                return;
            }
        };

        let successor_id = self
            .db
            .respawn_phoenix_successor(
                task_id,
                CreateTaskRequest {
                    title: &task.title,
                    description: &task.description,
                    repo_path: &repo_path,
                    plan: plan.as_deref(),
                    status: TaskStatus::Backlog,
                    base_branch: &task.base_branch,
                    epic_id: effective_epic_id,
                    // Left unset so the copy sorts by its own id, at the
                    // bottom of backlog, rather than inheriting a position in
                    // a column the predecessor has left.
                    sort_order: None,
                    tag: task.tag,
                    wrap_up_mode: task.wrap_up_mode,
                    auto_run_plan: task.auto_run_plan,
                    phoenix: true,
                },
                &task.labels,
            )
            .await;
        if let Err(e) = successor_id {
            tracing::error!(
                task_id = task_id.0,
                error = %e,
                "phoenix respawn failed; the task stays Done and keeps its flag"
            );
            return;
        }

        if let Some(eid) = effective_epic_id {
            self.recalculate_epic(eid).await;
        }
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

        if !task.is_wrappable() {
            return Err(ServiceError::Validation(format!(
                "Task {} cannot be wrapped up. Requires Running or Review status with a worktree.",
                task_id.0
            )));
        }

        Ok(task)
    }

    /// Record a Claude Code hook event for a task.
    ///
    /// `Stop` transitions Running → Review and clears both timestamps.
    /// `PreToolUse`/`Notification` stamp their timestamp and reclassify
    /// `sub_status`; both are no-ops on non-Running tasks. `UserPromptSubmit`
    /// additionally resumes a Review task straight to Running — it is the
    /// earliest signal that the human has continued the conversation — and
    /// voids the deferred `Stop` that prompt supersedes; it is a no-op outside
    /// {Running, Review}.
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

        // Stop and UserPromptSubmit are handled before the patch match, not
        // inside it: both decide a branch against the row's committed state, so
        // they produce no TaskPatch and must not read `task` — another hook
        // process can have invalidated that snapshot by the time we write. For
        // Stop that is the flip-or-defer choice; for UserPromptSubmit it is
        // whether the deferred Stop it would void is one this prompt actually
        // supersedes. See HookStop / HookUserPromptSubmit in
        // agent-health.allium.
        if kind == HookEventKind::Stop {
            if self.db.try_record_stop(id, self.clock.now()).await? == StopOutcome::Flipped {
                self.recalculate_epic_for_task(id).await;
            }
            return Ok(());
        }
        if kind == HookEventKind::UserPromptSubmit {
            // Recalculate only for the arm that moved status — Review ->
            // Running. A refresh of an already-Running task is a plain activity
            // signal and must not pay for a recalculation on this hot path.
            if self
                .db
                .record_user_prompt_submit(id, self.clock.now())
                .await?
                == UserPromptOutcome::Resumed
            {
                self.recalculate_epic_for_task(id).await;
            }
            return Ok(());
        }

        let now = self.clock.now();
        let patch = match kind {
            HookEventKind::PreToolUse if task.status == TaskStatus::Running => {
                let activity = classify_agent_activity(
                    Some(now),
                    task.last_notification_at,
                    task.live_subagents,
                    task.live_shells,
                    task.oldest_live_shell_started_at,
                    now,
                );
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
            _ => None,
        };
        // Neither remaining arm moves task.status, so no epic recalculation is
        // owed here.
        if let Some(patch) = patch {
            self.db.patch_task(id, &patch).await?;
        }
        Ok(())
    }

    /// Record an observed native Claude Code `SendMessage` call from
    /// `sender_id`'s agent, addressed to session `target_name` (task #4098).
    /// Called from the `dispatch hook <id> peer-message` CLI subcommand, which
    /// the Claude Code hook pipeline invokes on `PostToolUse` for the
    /// `SendMessage` tool — dispatch never performs the delivery itself, only
    /// observes it to drive the TUI flash and reconstruct an audit trail.
    ///
    /// Stamps `last_peer_message_sent_at` on the sender's own row
    /// unconditionally. `target_name` is resolved against dispatch's own
    /// `task-<id>` session-naming convention (`session_name_flag`,
    /// `src/dispatch/agents.rs`); when it resolves to a task that still
    /// exists, that task's `last_peer_message_received_at` is stamped too. A
    /// name that doesn't parse, or a task id that no longer exists, is not an
    /// error — the sender addressed a session outside dispatch's own fleet
    /// (or one that's since been deleted), which is not dispatch's concern.
    /// Only a missing *sender* is an error, matching `record_hook_event`'s
    /// contract.
    pub async fn record_peer_message_sent(
        &self,
        sender_id: TaskId,
        target_name: &str,
    ) -> Result<(), ServiceError> {
        if !self.db.task_exists(sender_id).await? {
            return Err(Self::task_not_found(sender_id));
        }

        let now = self.clock.now();
        self.db
            .patch_task(
                sender_id,
                &TaskPatch::new().last_peer_message_sent_at(Some(now)),
            )
            .await?;

        if let Some(target_id) = parse_peer_message_target_name(target_name) {
            // No existence pre-check: patch_task itself is the only
            // authoritative answer to "does this row still exist", and
            // checking first would leave a race (the target could be
            // deleted between the check and the write) that this direct
            // attempt does not have. A failure here — an unresolvable
            // target is already handled above; this is specifically "it
            // resolved but the row is gone by the time we got here" — is
            // not this call's concern either: the sender's own stamp above
            // already landed, and a missed *target* stamp is a missed TUI
            // flash, not a lost message (the message itself travels over
            // Claude Code's own native delivery, not through this call).
            if let Err(e) = self
                .db
                .patch_task(
                    target_id,
                    &TaskPatch::new().last_peer_message_received_at(Some(now)),
                )
                .await
            {
                tracing::debug!(
                    target_id = target_id.0,
                    "peer-message target vanished before its stamp landed: {e}"
                );
            }
        }

        Ok(())
    }

    /// Record a subagent lifecycle event and, when it drains the last
    /// subagent for a task carrying a deferred Stop, apply that Stop.
    ///
    /// A `Start` is handled on its own and returns before the drain path
    /// exists: it inserts its row *before* the recount, so the resulting count
    /// can never be zero and it can never drain. Keeping that as control flow
    /// rather than a condition means the drain predicate does not have to encode
    /// the invariant — nor silently depend on it.
    ///
    /// See `HookSubagentStart` / `HookSubagentStop` in
    /// `docs/specs/agent-health.allium`.
    pub async fn record_subagent_event(
        &self,
        id: TaskId,
        event: SubagentEvent,
    ) -> Result<(), ServiceError> {
        if !self.db.task_exists(id).await? {
            return Err(Self::task_not_found(id));
        }

        // Whether a deferred Stop applied is decided inside the same
        // transaction that recomputes the count, so there is nothing to check
        // out here — and no window in which the count has reached zero but the
        // flip has not landed.
        let applied_pending_stop = match event {
            SubagentEvent::Start {
                agent_id,
                session_id,
            } => {
                // A Start can never drain: it only ever raises the count.
                self.db
                    .subagent_start(id, &agent_id, &session_id, self.clock.now())
                    .await?;
                false
            }
            SubagentEvent::Stop {
                agent_id,
                session_id,
            } => {
                self.db
                    .subagent_stop(id, &agent_id, &session_id)
                    .await?
                    .applied_pending_stop
            }
            SubagentEvent::Clear => self.db.subagent_clear(id).await?.applied_pending_stop,
        };

        if applied_pending_stop {
            self.recalculate_epic_for_task(id).await;
        }
        Ok(())
    }

    /// The service's one spelling of "that task id doesn't exist". Callers that
    /// need the row use [`get_task`](Self::get_task), which produces the same
    /// error; this is for the existence-only checks, which have no row to fail on.
    fn task_not_found(id: TaskId) -> ServiceError {
        ServiceError::NotFound(format!("Task {} not found", id.0))
    }

    /// Clear a task's subagent entries and void any pending Stop **without**
    /// the drain path.
    ///
    /// Three callers: crash and dispatch-claim already own the resulting status
    /// (running the drain path there would leave a task Crashed-and-in-Review
    /// or freshly-dispatched-and-in-Review), and `SessionStart` — where a Stop
    /// deferred by the *previous* turn is stale by definition and must be
    /// voided, not applied. See `ClearSubagentsOnSessionStart` in
    /// `docs/specs/agent-health.allium`.
    ///
    /// `NotFound` for an unknown task, matching `record_subagent_event`: the
    /// hook CLI turns that into a silent skip rather than a failed tool call.
    /// Existence is all this needs, so it does not materialise the row.
    ///
    /// Entries and `stop_pending` go in one writer round trip. Not just for the
    /// saved trip: a partial clear-then-patch would briefly leave
    /// `live_subagents = 0` + `stop_pending` + Running, so a `SubagentStart`
    /// landing in that window would be counted against a task whose bit is
    /// about to be wiped. The single transaction rules that out.
    pub async fn clear_subagents_no_drain(&self, id: TaskId) -> Result<(), ServiceError> {
        if !self.db.task_exists(id).await? {
            return Err(Self::task_not_found(id));
        }
        self.db.subagent_clear_and_void_pending_stop(id).await?;
        Ok(())
    }

    /// Record a shell lifecycle event and, when it drains the last live
    /// shell for a task carrying a deferred Stop (with no subagent still
    /// live either), apply that Stop. Mirrors `record_subagent_event`.
    pub async fn record_shell_event(
        &self,
        id: TaskId,
        event: ShellEvent,
    ) -> Result<(), ServiceError> {
        if !self.db.task_exists(id).await? {
            return Err(Self::task_not_found(id));
        }
        let applied_pending_stop = match event {
            ShellEvent::Start {
                shell_id,
                session_id,
            } => {
                self.db
                    .shell_start(id, &shell_id, &session_id, self.clock.now())
                    .await?;
                false
            }
            ShellEvent::Stop {
                shell_id,
                session_id,
            } => {
                self.db
                    .shell_stop(id, &shell_id, &session_id)
                    .await?
                    .applied_pending_stop
            }
        };
        if applied_pending_stop {
            self.recalculate_epic_for_task(id).await;
        }
        Ok(())
    }

    /// Non-draining clear: deletes every `task_shells` row for the task and
    /// resyncs `live_shells`, without touching status. For
    /// `DetectCrashedAgent` and `DispatchTask`'s claim functions —
    /// deliberately NOT called from `SessionStart`. See
    /// docs/superpowers/specs/2026-08-15-shell-visibility-design.md.
    pub async fn clear_shells_no_drain(&self, id: TaskId) -> Result<(), ServiceError> {
        if !self.db.task_exists(id).await? {
            return Err(Self::task_not_found(id));
        }
        self.db.shell_clear_no_drain(id).await?;
        Ok(())
    }

    /// Both non-draining clears in one call. Every no-drain caller wants both
    /// halves except `SessionStart` (`cmd_hook_subagent`'s `clear` action),
    /// which deliberately calls [`Self::clear_subagents_no_drain`] alone —
    /// see the session-fencing section of
    /// docs/superpowers/specs/2026-08-15-shell-visibility-design.md for why
    /// shells don't get a SessionStart-driven clear. Using this combined
    /// method everywhere else means a future no-drain call site gets both
    /// clears by construction, rather than needing to remember to call two
    /// separate methods.
    pub async fn clear_structural_no_drain(&self, id: TaskId) -> Result<(), ServiceError> {
        self.clear_subagents_no_drain(id).await?;
        self.clear_shells_no_drain(id).await?;
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
        // No-drain: a claim moves the task *into* Running; draining a
        // leftover count here would race that with a Review flip. Both
        // halves: guards against subagent/shell entries left over from a
        // prior run of this task.
        self.clear_structural_no_drain(claimed_id).await?;
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
        // No-drain: a claim moves the task *into* Running; draining a
        // leftover count here would race that with a Review flip.
        self.clear_structural_no_drain(task_id).await?;
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
