use serde_json::{json, Value};

use crate::dispatch;
use crate::mcp::identity::CallerIdentity;
use crate::mcp::McpState;
use crate::models::{DispatchMode, EpicId, TaskId, TaskStatus};

use super::{
    parse_args, service_err_to_response, ClaimTaskArgs, DispatchNextArgs, DispatchTaskArgs,
    JsonRpcResponse, SendMessageArgs,
};
use crate::service::{ClaimTaskParams, FieldUpdate, UpdateTaskParams};

fn do_dispatch(
    task: &crate::models::Task,
    runner: &dyn crate::process::ProcessRunner,
    epic_ctx: Option<dispatch::EpicContext>,
    injected: &[crate::models::Learning],
    verify_command: Option<String>,
) -> anyhow::Result<crate::models::DispatchResult> {
    let injections = dispatch::LearningInjections::from(injected);
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

pub(crate) async fn handle_dispatch_next(
    state: &McpState,
    id: Option<Value>,
    _identity: &CallerIdentity,
    args: Value,
) -> JsonRpcResponse {
    let parsed = match parse_args::<DispatchNextArgs>(&id, args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!(epic_id = parsed.epic_id, "MCP dispatch_next");

    // Check auto_dispatch flag before doing any work
    match state
        .db
        .get_epic(crate::models::EpicId(parsed.epic_id))
        .await
    {
        Ok(Some(epic)) if !epic.auto_dispatch => {
            return JsonRpcResponse::ok(
                id,
                json!({"content": [{"type": "text", "text": format!(
                    "auto dispatch is disabled for epic #{} — dispatch the next task manually",
                    parsed.epic_id
                )}]}),
            );
        }
        Ok(Some(_)) => {}
        Ok(None) => {
            return JsonRpcResponse::err(id, -32602, format!("epic #{} not found", parsed.epic_id));
        }
        Err(e) => {
            tracing::warn!(
                "dispatch_next: failed to fetch epic #{}: {e}",
                parsed.epic_id
            );
            // Don't block dispatch on a DB error reading the flag
        }
    }

    let next_task = match state
        .task_svc
        .next_backlog_task(EpicId(parsed.epic_id))
        .await
    {
        Ok(Some(task)) => task,
        Ok(None) => {
            return JsonRpcResponse::ok(
                id,
                json!({"content": [{"type": "text", "text": format!(
                    "no backlog tasks to dispatch for epic #{}",
                    parsed.epic_id
                )}]}),
            );
        }
        Err(e) => return service_err_to_response(id, e),
    };

    let next_id = next_task.id;
    let next_title = next_task.title.clone();
    let next_epic_id = next_task.epic_id;
    let epic_ctx = dispatch::EpicContext::from_db(&next_task, &*state.db).await;
    let db = state.db.clone();
    let task_svc = state.task_svc.clone();
    let runner = state.runner.clone();
    let notify_tx = state.notify_tx.clone();

    let injected =
        dispatch::build_and_record_injections(&*db, &next_task, &state.embedding_service).await;
    let verify_command = dispatch::fetch_verify_command(&*db, &next_task.repo_path).await;

    tokio::spawn(async move {
        let next_task_for_blocking = next_task.clone();
        let result = tokio::task::spawn_blocking(move || {
            do_dispatch(
                &next_task_for_blocking,
                &*runner,
                epic_ctx,
                &injected,
                verify_command,
            )
        })
        .await;

        match result {
            Ok(Ok(dispatch_result)) => {
                // Seed last_pre_tool_use_at so ClassifyAgentActivity treats
                // the freshly running task as Active until the agent's first
                // PreToolUse hook fires — otherwise the TUI tick flickers it
                // into Stale.
                let params = UpdateTaskParams::for_task(next_id)
                    .status(TaskStatus::Running)
                    .worktree(FieldUpdate::Set(dispatch_result.worktree_path))
                    .tmux_window(FieldUpdate::Set(dispatch_result.tmux_window))
                    .last_pre_tool_use_at(Some(chrono::Utc::now()));
                if let Err(e) = task_svc.update_task(params).await {
                    tracing::warn!(
                        task_id = next_id.0,
                        "dispatch_next: failed to update task: {e}"
                    );
                }
            }
            Ok(Err(e)) => {
                tracing::warn!(task_id = next_id.0, "dispatch_next: dispatch failed: {e:#}");
            }
            Err(e) => {
                tracing::warn!(
                    task_id = next_id.0,
                    "dispatch_next: blocking task panicked: {e}"
                );
            }
        }

        if let Some(tx) = notify_tx {
            let _ = tx.send(crate::mcp::McpEvent::TaskChanged(next_id));
            if let Some(epic_id) = next_epic_id {
                let _ = tx.send(crate::mcp::McpEvent::EpicChanged(epic_id));
            }
        }
    });

    JsonRpcResponse::ok(
        id,
        json!({"content": [{"type": "text", "text": format!(
            "dispatching task #{} '{}'",
            next_id.0, next_title
        )}]}),
    )
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

    let epic_ctx = dispatch::EpicContext::from_db(&task, &*state.db).await;
    let db = state.db.clone();
    let runner = state.runner.clone();
    let epic_id = task.epic_id;

    let injected =
        dispatch::build_and_record_injections(&*db, &task, &state.embedding_service).await;
    let verify_command = dispatch::fetch_verify_command(&*db, &task.repo_path).await;
    let result = tokio::task::spawn_blocking(move || {
        do_dispatch(&task, &*runner, epic_ctx, &injected, verify_command)
    })
    .await;

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
