//! The single dispatch orchestration seam.
//!
//! `claim → prepare inputs → pick the agent for the mode → provision on the
//! blocking pool → record where the agent landed → release the claim on
//! failure` used to be written out once per entry point (the MCP
//! `dispatch_task` handler, the epic auto-dispatch chain, and the TUI runtime),
//! with the `DispatchMode` match duplicated on top of that. `DispatchClaimExclusive`
//! (`docs/specs/dispatch.allium`) is the most safety-critical rule in the
//! system and it is exactly the part those copies each re-asserted separately,
//! so it lives here once.
//!
//! Callers keep only what is genuinely theirs: how they came by the task, and
//! how they report the outcome (a JSON-RPC response, a board message).

use std::sync::Arc;

use crate::dispatch;
use crate::models::{DispatchMode, DispatchResult, Task};
use crate::service::{FieldUpdate, ServiceError, UpdateTaskParams};

use super::crud::TaskService;

/// How the caller came by the task it is asking to dispatch.
///
/// Not a bool: the two arms carry different obligations. `Take` means this call
/// owns the claim and therefore owes the release if provisioning fails; `Held`
/// means the caller already won it — the epic chain's
/// [`claim_next_backlog_task`](TaskService::claim_next_backlog_task) both
/// *selects* and claims, so the claim cannot be taken here without dispatching
/// something the chain never chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchClaim {
    /// Take the pre-provisioning claim as the first step of this call.
    Take,
    /// The caller already holds the claim for this task.
    Held,
}

/// What a caller brings to [`TaskService::dispatch`]: the row, how to launch
/// it, who owns the claim, and the two collaborators the service does not hold.
///
/// No read handle: the prologue reads through `TaskService`'s own
/// [`db`](TaskService::db), so *within this seam* the reads cannot be aimed at
/// one database while the claim and the worktree write land in another. That
/// scoping is literal — the TUI's `exec_dispatch_agent`/`exec_quick_dispatch`
/// (`src/runtime/tasks.rs`) do not come through here and still run the prologue
/// against their own `TaskReadStore`.
///
/// `emb_svc` does stay a per-call parameter: embeddings live with the learning
/// service, and `EmbeddingService::new_noop` spawns an OS thread that a
/// constructor argument would start in every fixture that never dispatches.
pub struct DispatchRequest {
    /// The task to provision. Must be the claimed row (`Held`) or a `Backlog`
    /// row this call will claim (`Take`).
    pub task: Task,
    /// Which agent to launch. MCP callers derive it with
    /// [`DispatchMode::for_task`]; the TUI carries the operator's choice.
    pub mode: DispatchMode,
    /// Embedding service backing the learning injections.
    pub emb_svc: Arc<crate::service::embeddings::EmbeddingService>,
    /// Pre-resolved epic banner, for a caller that already holds the epic row
    /// (the chain reads it to check `auto_dispatch`). `None` means "read it
    /// from the service's own handle" — which is also what a task with no epic
    /// resolves to, so
    /// leaving it unset is always correct, just one read slower.
    pub epic_ctx: Option<dispatch::EpicContext>,
    /// Whether this call takes the claim or the caller already holds it.
    pub claim: DispatchClaim,
}

/// What a [`TaskService::dispatch`] attempt made of the task.
#[derive(Debug)]
pub enum DispatchOutcome {
    /// The agent is running. `worktree`/`tmux_window` are already persisted.
    Launched(DispatchResult),
    /// The claim went to another entry point. Nothing was provisioned and
    /// nothing is owed: the claim is a single conditional write, so a caller
    /// that lost it holds nothing to unwind. Only reachable with
    /// [`DispatchClaim::Take`].
    ClaimLost,
    /// The claim write itself errored, so it wrote nothing. As with
    /// [`Self::ClaimLost`], there is nothing to release.
    ClaimFailed(ServiceError),
    /// Provisioning failed after the claim was won, and the claim has been
    /// released (conditionally — see
    /// [`release_claim`](TaskService::release_claim)). The string is the
    /// failure, ready to be embedded in the caller's own error prose.
    Failed(String),
}

impl TaskService {
    /// Provision `req.task` and launch its agent.
    ///
    /// Owns the whole sequence: claim (per [`DispatchClaim`]), the dispatch
    /// prologue, the [`DispatchMode`] match, the blocking provision, the
    /// worktree/tmux write on success, and the claim release on failure.
    ///
    /// Never returns `Err`: every failure mode is a [`DispatchOutcome`] the
    /// caller must distinguish, and collapsing them into one error type is how
    /// a lost claim gets mistaken for a failed dispatch — the difference
    /// decides whether anyone owes a release.
    pub async fn dispatch(&self, req: DispatchRequest) -> DispatchOutcome {
        let DispatchRequest {
            task,
            mode,
            emb_svc,
            epic_ctx,
            claim,
        } = req;
        let task_id = task.id;

        if claim == DispatchClaim::Take {
            match self.claim_backlog_task(task_id).await {
                Ok(true) => {}
                Ok(false) => return DispatchOutcome::ClaimLost,
                Err(e) => return DispatchOutcome::ClaimFailed(e),
            }
        }

        // The prologue runs a local embedding inference and several writes, so
        // it happens before the task is handed to the blocking pool.
        let inputs = match epic_ctx {
            Some(ctx) => {
                dispatch::prepare_inputs_with_epic_ctx(&*self.db, &task, &emb_svc, Some(ctx)).await
            }
            None => dispatch::prepare_inputs(&*self.db, &task, &emb_svc).await,
        };

        let runner = Arc::clone(&self.runner);
        tracing::info!(task_id = task_id.0, label = mode.label(), "dispatching");
        let result = tokio::task::spawn_blocking(move || {
            dispatch::run_agent_for_mode(&task, mode, &*runner, inputs)
        })
        .await;

        match result {
            Ok(Ok(dr)) => {
                // The claim already applied Running and seeded
                // last_pre_tool_use_at, so this patch only records where the
                // agent actually landed.
                let params = UpdateTaskParams::for_task(task_id)
                    .worktree(FieldUpdate::Set(dr.worktree_path.clone()))
                    .tmux_window(FieldUpdate::Set(dr.tmux_window.clone()));
                if let Err(e) = self.update_task(params).await {
                    tracing::warn!(
                        task_id = task_id.0,
                        "dispatch: failed to record worktree/tmux_window: {e}"
                    );
                }
                DispatchOutcome::Launched(dr)
            }
            Ok(Err(e)) => {
                let reason = format!("{e:#}");
                tracing::warn!(task_id = task_id.0, "dispatch failed: {reason}");
                self.release_claim_logged(task_id).await;
                DispatchOutcome::Failed(reason)
            }
            Err(e) => {
                let reason = format!("dispatch worker died: {e}");
                tracing::warn!(task_id = task_id.0, "{reason}");
                self.release_claim_logged(task_id).await;
                DispatchOutcome::Failed(reason)
            }
        }
    }

    /// Return a claimed-but-unprovisioned task to `Backlog` so a failed
    /// dispatch leaves it dispatchable exactly as it was before the attempt.
    ///
    /// `Ok(false)` is an expected outcome, not an error: the release is
    /// conditional on the task still being claimed-and-unprovisioned, because
    /// provisioning can take a `git fetch`'s worth of wall time and an
    /// unconditional revert would stomp anything that touched the task
    /// meanwhile.
    async fn release_claim_logged(&self, task_id: crate::models::TaskId) {
        match self.release_claim(task_id).await {
            Ok(true) => {}
            Ok(false) => tracing::info!(
                task_id = task_id.0,
                "release_claim: claim already released or task moved on; left as-is"
            ),
            Err(e) => tracing::warn!(task_id = task_id.0, "release_claim: failed: {e}"),
        }
    }
}
