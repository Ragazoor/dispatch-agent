use serde_json::{json, Value};

use crate::dispatch;
use crate::mcp::identity::CallerIdentity;
use crate::mcp::McpState;
use crate::models::{DispatchMode, EpicId, TaskId};

use super::{
    parse_args, service_err_to_response, DispatchTaskArgs, JsonRpcResponse, INTERNAL_ERROR,
    INVALID_PARAMS,
};
use crate::service::{DispatchClaim, DispatchOutcome, DispatchRequest};

/// Dispatches the next backlog subtask of `epic_id`, if any. Returns
/// `Some((id, title))` when a dispatch was started, `None` when the chain
/// stops here (auto_dispatch off, no backlog subtask, or a lookup failure).
/// Never returns an error: a chain problem must not fail the caller.
///
/// Implements `AutoDispatchNextSubtask` in `docs/specs/epics.allium`, fired by
/// `handle_exit_session` as the last thing a session close does.
pub(in crate::mcp::handlers) async fn auto_dispatch_next(
    state: &McpState,
    epic_id: EpicId,
) -> Option<(TaskId, String)> {
    // Fail closed: nothing *requested* this chain, so a DB hiccup must not
    // launch an agent on an epic whose operator turned chaining off. Every
    // non-dispatch outcome here is indistinguishable from the documented
    // normal stops.
    let epic = match state.db.get_epic(epic_id).await {
        Ok(Some(epic)) => epic,
        Ok(None) => {
            tracing::warn!("auto_dispatch_next: epic #{} not found", epic_id.0);
            return None;
        }
        Err(e) => {
            tracing::warn!(
                "auto_dispatch_next: failed to fetch epic #{}: {e}",
                epic_id.0
            );
            return None;
        }
    };
    if !epic.auto_dispatch {
        return None;
    }

    // Claiming is what makes the selection exclusive: a concurrent close on the
    // same epic can never win the same subtask.
    let next_task = match state.task_svc.claim_next_backlog_task(epic_id).await {
        Ok(Some(task)) => task,
        Ok(None) => return None,
        Err(e) => {
            tracing::warn!(
                "auto_dispatch_next: failed to claim next subtask of epic #{}: {e}",
                epic_id.0
            );
            return None;
        }
    };

    let next_id = next_task.id;
    let next_title = next_task.title.clone();
    // Read before the task is moved into the dispatch request.
    let repo_path = next_task.repo_path.clone();
    // The epic row is already in hand, so skip `EpicContext::from_db`'s
    // re-read. The claim selects only from this epic's subtasks, so the
    // context is always this epic.
    let epic_ctx = Some(dispatch::EpicContext {
        epic_id,
        epic_title: epic.title,
    });
    let request = DispatchRequest {
        mode: DispatchMode::for_task(&next_task),
        task: next_task,
        emb_svc: state.embedding_service.clone(),
        epic_ctx,
        // `claim_next_backlog_task` above both selected and claimed this row.
        claim: DispatchClaim::Held,
    };
    let task_svc = state.task_svc.clone();
    let notify_tx = state.notify_tx.clone();

    tokio::spawn(async move {
        // The seam's prologue runs a local embedding inference and several
        // writes; keep it off the caller's request path — `exit_session` only
        // needs the id and title, both already known.
        let outcome = task_svc.dispatch(request).await;

        let launched = matches!(outcome, DispatchOutcome::Launched(_));
        // Why the chain stopped, when it stopped after claiming. Reported to the
        // board below (SurfaceAutoDispatchFailure in docs/specs/epics.allium) —
        // logging alone leaves a stalled epic indistinguishable from a finished
        // one.
        let failure = match outcome {
            DispatchOutcome::Launched(_) => None,
            DispatchOutcome::Failed(reason) => Some(reason),
            // Unreachable — the chain passes `DispatchClaim::Held`, so the
            // seam's claim block never runs — and reported as nothing even if
            // that changed: `SurfaceAutoDispatchFailure` scopes the claim stops
            // out of itself because they fail before a subtask is selected, so
            // neither can supply the task the rule marks.
            DispatchOutcome::ClaimLost | DispatchOutcome::ClaimFailed(_) => None,
        };

        if let Some(tx) = notify_tx {
            // Sent ahead of the row reloads: the stall is the fact this attempt
            // established, and a consumer that reloaded first would paint the
            // released card before knowing it is stalled.
            if let Some(reason) = failure {
                let _ = tx.send(crate::mcp::McpEvent::AutoDispatchFailed {
                    task_id: next_id,
                    epic_id,
                    reason,
                });
            }
            // RefreshRepoSyncStateAfterDispatch: the chain provisioned a worktree
            // and fetched origin/<base>, so the board's drift measurement is
            // stale. Sent ahead of the row reloads because it is the fact the
            // dispatch established; a dispatch that failed established nothing.
            if launched {
                let _ = tx.send(crate::mcp::McpEvent::AgentLaunched { repo_path });
            }
            let _ = tx.send(crate::mcp::McpEvent::TaskChanged(next_id));
            let _ = tx.send(crate::mcp::McpEvent::EpicChanged(epic_id));
        }
    });

    Some((next_id, next_title))
}

pub(crate) async fn handle_dispatch_task(
    state: &McpState,
    id: Option<Value>,
    _identity: &CallerIdentity,
    args: Value,
) -> JsonRpcResponse {
    let parsed = match parse_args::<DispatchTaskArgs>(&id, args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    let task_id = parsed.task_id;

    let task = match state.db.get_task(task_id).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return service_err_to_response(
                id,
                crate::service::ServiceError::NotFound(format!("task #{} not found", task_id.0)),
            )
        }
        Err(e) => return JsonRpcResponse::err(id, INTERNAL_ERROR, format!("db error: {e:#}")),
    };

    let epic_id = task.epic_id;
    // Read before the task is moved into the dispatch request.
    let repo_path = task.repo_path.clone();

    // The claim, the provisioning and the release-on-failure unwind all live in
    // the seam. The backlog guard is the claim, not the status read above: a
    // read-then-provision guard leaves a window in which a concurrent chain or
    // TUI dispatch takes the same task and both provision it
    // (DispatchClaimExclusive in `docs/specs/dispatch.allium`).
    let outcome = state
        .task_svc
        .dispatch(DispatchRequest {
            mode: DispatchMode::for_task(&task),
            task,
            emb_svc: state.embedding_service.clone(),
            epic_ctx: None,
            claim: DispatchClaim::Take,
        })
        .await;

    match outcome {
        DispatchOutcome::Launched(dr) => {
            // RefreshRepoSyncStateAfterDispatch: this call provisioned a worktree
            // and fetched origin/<base>, so the board's drift measurement for the
            // repository is stale. Only the success arm notifies — a failed
            // dispatch moved nothing.
            state.notify_agent_launched(&repo_path);
            state.notify_task_changed(task_id);
            if let Some(eid) = epic_id {
                state.notify_epic_changed(eid);
            }
            JsonRpcResponse::ok(
                id,
                json!({"content": [{"type": "text", "text": format!(
                    "dispatched task #{} — worktree: {}, tmux: {}",
                    task_id.0, dr.worktree_path, dr.tmux_window
                )}]}),
            )
        }
        DispatchOutcome::ClaimLost => not_in_backlog_response(state, id, task_id).await,
        DispatchOutcome::ClaimFailed(e) => service_err_to_response(id, e),
        DispatchOutcome::Failed(reason) => {
            JsonRpcResponse::err(id, INTERNAL_ERROR, format!("dispatch failed: {reason}"))
        }
    }
}

/// The error for a `dispatch_task` whose claim was lost.
///
/// Re-reads the row so the message names the status the task actually holds now
/// rather than the pre-claim one this call read — the status is precisely what
/// changed under us. A row that vanished between the two reads is reported as
/// `NotFound`, matching what the initial read would have said.
async fn not_in_backlog_response(
    state: &McpState,
    id: Option<Value>,
    task_id: TaskId,
) -> JsonRpcResponse {
    let current = match state.db.get_task(task_id).await {
        Ok(Some(t)) => t.status.to_string(),
        Ok(None) => {
            return service_err_to_response(
                id,
                crate::service::ServiceError::NotFound(format!("task #{} not found", task_id.0)),
            )
        }
        Err(e) => return JsonRpcResponse::err(id, INTERNAL_ERROR, format!("db error: {e:#}")),
    };
    JsonRpcResponse::err(
        id,
        INVALID_PARAMS,
        format!("task #{} is not in backlog (current: {current})", task_id.0),
    )
}
