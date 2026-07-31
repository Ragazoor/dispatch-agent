use serde_json::{json, Value};

use crate::dispatch;
use crate::mcp::identity::CallerIdentity;
use crate::mcp::McpState;
use crate::models::{DispatchMode, EpicId, TaskId, TaskStatus};

use super::{
    parse_args, service_err_to_response, ClaimTaskArgs, DispatchTaskArgs, JsonRpcResponse,
    SendMessageArgs,
};
use crate::service::{ClaimTaskParams, FieldUpdate, UpdateTaskParams};

fn do_dispatch(
    task: &crate::models::Task,
    runner: &dyn crate::process::ProcessRunner,
    inputs: dispatch::DispatchInputs,
) -> anyhow::Result<crate::models::DispatchResult> {
    let dispatch::DispatchInputs {
        epic_ctx,
        injected,
        verify_command,
    } = inputs;
    let injections = dispatch::LearningInjections::from(injected.as_slice());
    match DispatchMode::for_task(task) {
        DispatchMode::Dispatch => dispatch::dispatch_agent(
            task,
            runner,
            epic_ctx.as_ref(),
            &injections,
            verify_command.as_deref(),
        ),
        DispatchMode::Research => {
            dispatch::research_agent(task, runner, epic_ctx.as_ref(), verify_command.as_deref())
        }
    }
}

pub(crate) async fn handle_claim_task(
    state: &McpState,
    id: Option<Value>,
    _identity: &CallerIdentity,
    args: Value,
) -> JsonRpcResponse {
    let parsed = match parse_args::<ClaimTaskArgs>(&id, args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!(task_id = parsed.task_id, worktree = %parsed.worktree, "MCP claim_task");

    match state
        .task_svc
        .claim_task(ClaimTaskParams {
            task_id: TaskId(parsed.task_id),
            worktree: parsed.worktree,
            tmux_window: parsed.tmux_window,
        })
        .await
    {
        Ok(task) => {
            state.notify_task_changed(task.id);
            JsonRpcResponse::ok(
                id,
                json!({"content": [{"type": "text", "text": format!("Task {} claimed: {}", parsed.task_id, task.title)}]}),
            )
        }
        Err(e) => service_err_to_response(id, e),
    }
}

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
    // The epic row is already in hand, so skip `EpicContext::from_db`'s
    // re-read. The claim selects only from this epic's subtasks, so the
    // context is always this epic.
    let epic_ctx = Some(dispatch::EpicContext {
        epic_id,
        epic_title: epic.title,
    });
    let db = state.db.clone();
    let task_svc = state.task_svc.clone();
    let runner = state.runner.clone();
    let notify_tx = state.notify_tx.clone();
    let embedding_service = state.embedding_service.clone();

    tokio::spawn(async move {
        // The prologue runs a local embedding inference and several writes; keep
        // it off the caller's request path — `exit_session` only needs the id and
        // title, both already known.
        let inputs =
            dispatch::prepare_inputs_with_epic_ctx(&*db, &next_task, &embedding_service, epic_ctx)
                .await;

        let result =
            tokio::task::spawn_blocking(move || do_dispatch(&next_task, &*runner, inputs)).await;

        match result {
            Ok(Ok(dispatch_result)) => {
                // The claim already applied Running and seeded
                // last_pre_tool_use_at, so this patch only records where the
                // agent actually landed.
                let params = UpdateTaskParams::for_task(next_id)
                    .worktree(FieldUpdate::Set(dispatch_result.worktree_path))
                    .tmux_window(FieldUpdate::Set(dispatch_result.tmux_window));
                if let Err(e) = task_svc.update_task(params).await {
                    tracing::warn!(
                        task_id = next_id.0,
                        "auto_dispatch_next: failed to update task: {e}"
                    );
                }
            }
            Ok(Err(e)) => {
                tracing::warn!(
                    task_id = next_id.0,
                    "auto_dispatch_next: dispatch failed: {e:#}"
                );
                release_claim(&*task_svc, next_id).await;
            }
            Err(e) => {
                tracing::warn!(
                    task_id = next_id.0,
                    "auto_dispatch_next: blocking task panicked: {e}"
                );
                release_claim(&*task_svc, next_id).await;
            }
        }

        if let Some(tx) = notify_tx {
            let _ = tx.send(crate::mcp::McpEvent::TaskChanged(next_id));
            let _ = tx.send(crate::mcp::McpEvent::EpicChanged(epic_id));
        }
    });

    Some((next_id, next_title))
}

/// Return a claimed-but-unprovisioned subtask to `Backlog` so a failed chain
/// leaves it dispatchable exactly as it was before the chain fired.
///
/// Delegates to [`crate::service::TaskServiceApi::release_claim`], which is
/// conditional on the task still being claimed-and-unprovisioned — provisioning
/// can take a `git fetch`'s worth of wall time, and an unconditional revert
/// would stomp anything that touched the task meanwhile.
async fn release_claim(task_svc: &dyn crate::service::TaskServiceApi, task_id: TaskId) {
    match task_svc.release_claim(task_id).await {
        Ok(true) => {}
        Ok(false) => tracing::info!(
            task_id = task_id.0,
            "auto_dispatch_next: claim already released or task moved on; left as-is"
        ),
        Err(e) => tracing::warn!(
            task_id = task_id.0,
            "auto_dispatch_next: failed to release claim: {e}"
        ),
    }
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
    let task_id = crate::models::TaskId(parsed.task_id);

    let task = match state.db.get_task(task_id).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return service_err_to_response(
                id,
                crate::service::ServiceError::NotFound(format!("task #{} not found", task_id.0)),
            )
        }
        Err(e) => return JsonRpcResponse::err(id, -32603, format!("db error: {e:#}")),
    };

    if task.status != TaskStatus::Backlog {
        return JsonRpcResponse::err(
            id,
            -32602,
            format!(
                "task #{} is not in backlog (current: {})",
                task_id.0, task.status
            ),
        );
    }

    let db = state.db.clone();
    let runner = state.runner.clone();
    let epic_id = task.epic_id;

    let inputs = dispatch::prepare_inputs(&*db, &task, &state.embedding_service).await;
    let result = tokio::task::spawn_blocking(move || do_dispatch(&task, &*runner, inputs)).await;

    match result {
        Ok(Ok(dr)) => {
            // Seed last_pre_tool_use_at so ClassifyAgentActivity treats the
            // freshly running task as Active until the agent's first
            // PreToolUse hook fires.
            let response_text = format!(
                "dispatched task #{} — worktree: {}, tmux: {}",
                task_id.0, dr.worktree_path, dr.tmux_window
            );
            let params = UpdateTaskParams::for_task(task_id)
                .status(TaskStatus::Running)
                .worktree(FieldUpdate::Set(dr.worktree_path))
                .tmux_window(FieldUpdate::Set(dr.tmux_window))
                .last_pre_tool_use_at(Some(chrono::Utc::now()));
            if let Err(e) = state.task_svc.update_task(params).await {
                tracing::warn!(
                    task_id = task_id.0,
                    "dispatch_task: failed to update task: {e}"
                );
            }
            state.notify_task_changed(task_id);
            if let Some(eid) = epic_id {
                state.notify_epic_changed(eid);
            }
            JsonRpcResponse::ok(
                id,
                json!({"content": [{"type": "text", "text": response_text}]}),
            )
        }
        Ok(Err(e)) => JsonRpcResponse::err(id, -32603, format!("dispatch failed: {e:#}")),
        Err(e) => JsonRpcResponse::err(id, -32603, format!("dispatch join error: {e}")),
    }
}

pub(crate) async fn handle_send_message(
    state: &McpState,
    id: Option<Value>,
    _identity: &CallerIdentity,
    args: Value,
) -> JsonRpcResponse {
    let parsed: SendMessageArgs = match parse_args(&id, args) {
        Ok(v) => v,
        Err(e) => return e,
    };

    let (from_task, to_task) = match state
        .task_svc
        .validate_send_message(TaskId(parsed.from_task_id), TaskId(parsed.to_task_id))
        .await
    {
        Ok(pair) => pair,
        Err(e) => return service_err_to_response(id, e),
    };

    let Some(worktree) = to_task.worktree.as_ref() else {
        return JsonRpcResponse::err(id, -32603, "target task has no worktree (internal error)");
    };
    let Some(tmux_window) = to_task.tmux_window.as_ref() else {
        return JsonRpcResponse::err(
            id,
            -32603,
            "target task has no tmux window (internal error)",
        );
    };

    let sender_id = from_task.id.0;
    let message_content = format!(
        "[Message from task {}: \"{}\"]\n{}",
        from_task.id.0, from_task.title, parsed.body
    );
    let file_prefix = sender_id.to_string();
    if let Err(e) = crate::notify::deliver(
        state.runner.clone(),
        worktree.clone(),
        tmux_window.clone(),
        file_prefix,
        message_content,
        move |filename| {
            format!(
                "You received a message from task {sender_id}. Read .claude-messages/{filename} for the full content, then delete the file."
            )
        },
    )
    .await
    {
        return JsonRpcResponse::err(id, -32603, e);
    }

    state.notify_message_sent(to_task.id);

    tracing::info!(
        from_task_id = parsed.from_task_id,
        to_task_id = parsed.to_task_id,
        "message sent between agents"
    );

    JsonRpcResponse::ok(
        id,
        json!({"content": [{"type": "text", "text": format!(
            "Message sent to task {} ({})",
            to_task.id.0, to_task.title
        )}]}),
    )
}
