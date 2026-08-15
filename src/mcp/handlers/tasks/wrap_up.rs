use serde_json::{json, Value};

use crate::dispatch;
use crate::mcp::identity::CallerIdentity;
use crate::mcp::McpState;
use crate::models::{SubStatus, Task, TaskId};
use crate::service::UpdateTaskParams;

use super::{
    fetch_caller_task, parse_args, service_err_to_response, ExitSessionArgs, JsonRpcResponse,
    WrapUpAction, WrapUpArgs,
};

const ERR_NO_TOKEN: &str = "no exit token — call wrap_up first";

fn exit_instruction(action: WrapUpAction) -> String {
    let extra_arg = match action {
        WrapUpAction::Pr => ", and pr_url (the URL returned by `gh pr create`)",
        WrapUpAction::Rebase | WrapUpAction::Done => "",
    };
    format!(
        "call `exit_session` with action=\"{}\" and this token{extra_arg}",
        action.as_str()
    )
}

/// Worded per action rather than uniformly, because what changed since the
/// `/wrap-up` skill's own pre-`wrap_up` verification step (see the get_task
/// verify-command note in `docs/specs/mcp-task-tools.allium`) differs by
/// action: `rebase` just ran a real git rebase server-side, which can pull in
/// sibling-epic changes the skill's check could not have seen, so it names
/// that as a reason to re-verify. `pr`/`done` perform no git operation at all,
/// so nothing justifies telling the agent to redo work — the wording defaults
/// to verifying only when it is not already known to have happened, rather
/// than granting a blanket permission to skip (an agent that reached
/// `wrap_up` without running the skill's check must not read this as licence
/// to skip verifying altogether).
async fn wrap_up_verify_line(
    db: &dyn crate::db::TaskReadStore,
    repo_path: &str,
    action: WrapUpAction,
) -> String {
    match dispatch::fetch_verify_command(db, repo_path).await {
        Some(cmd) => match action {
            WrapUpAction::Rebase => format!(
                " **Verify before exiting**: this rebase may have pulled in changes since \
                you last checked — run `{cmd}` and confirm it passes."
            ),
            WrapUpAction::Pr | WrapUpAction::Done => format!(
                " **Verify before exiting**: if you haven't already run `{cmd}` and confirmed \
                it passes earlier in this wrap-up, do so now."
            ),
        },
        None => String::new(),
    }
}

/// Common wrap_up finishing sequence shared by all three actions: fetch the
/// verify-command line, issue the exit token recording `action`, and build
/// the exit_session instruction. Only the surrounding response prose differs
/// per action.
async fn issue_wrap_up_token(
    state: &McpState,
    task_id: TaskId,
    repo_path: &str,
    action: WrapUpAction,
) -> (String, String, String) {
    let verify_line = wrap_up_verify_line(&*state.db, repo_path, action).await;
    let token = state.issue_exit_token(task_id, action);
    let exit_line = exit_instruction(action);
    (verify_line, token, exit_line)
}

/// Checks the task is wrappable, returning the JSON-RPC error response to
/// return immediately if not. The worktree/branch pair is only needed by the
/// rebase path, so it is resolved separately in `finish_wrap_up_rebase`.
async fn validate_wrap_up_request(
    state: &McpState,
    id: &Option<Value>,
    task_id: i64,
) -> Result<Task, JsonRpcResponse> {
    state
        .task_svc
        .validate_wrap_up(TaskId(task_id))
        .await
        .map_err(|e| service_err_to_response(id.clone(), e))
}

/// Resolves the worktree/branch pair the rebase path needs from an
/// already-validated task.
fn resolve_rebase_target(
    id: &Option<Value>,
    task: &Task,
) -> Result<(String, String), JsonRpcResponse> {
    // Defence in depth: `validate_wrap_up` (via `is_wrappable`) guarantees the
    // worktree is `Some` today, but a future change to the validator could
    // silently break that contract. Returning an internal JSON-RPC error keeps
    // a violation from panicking the runtime.
    let worktree = task.worktree.clone().ok_or_else(|| {
        JsonRpcResponse::err(
            id.clone(),
            -32603,
            "internal: validate_wrap_up returned task without worktree".to_string(),
        )
    })?;

    let branch = dispatch::branch_from_worktree(&worktree).ok_or_else(|| {
        JsonRpcResponse::err(
            id.clone(),
            -32602,
            format!("Cannot derive branch from worktree: {worktree}"),
        )
    })?;

    Ok((worktree, branch))
}

/// Finishes the two no-rebase actions (`done`, `pr`), which only differ in a
/// short note on git operations and a trailing `rate_learning` nudge for `pr`.
async fn finish_wrap_up_simple(
    state: &McpState,
    id: Option<Value>,
    task_id: TaskId,
    repo_path: &str,
    action: WrapUpAction,
) -> JsonRpcResponse {
    let (verify_line, token, exit_line) =
        issue_wrap_up_token(state, task_id, repo_path, action).await;
    let no_git_note = match action {
        WrapUpAction::Done => " No git operations performed.",
        WrapUpAction::Pr | WrapUpAction::Rebase => "",
    };
    let rate_learning_nudge = match action {
        WrapUpAction::Pr => {
            "\n\n\
            Before you finish: if any knowledge base entry was surfaced to you this task \
            and you haven't rated it yet, call `rate_learning` now (helped or wrong). \
            You can only rate learnings that were surfaced to you during this session."
        }
        WrapUpAction::Done | WrapUpAction::Rebase => "",
    };
    JsonRpcResponse::ok(
        id,
        json!({"content": [{"type": "text", "text": format!(
            "wrap_up complete (task {tid}, action: {action_str}).{no_git_note} \
        The session is NOT yet closed.{verify_line} \
        Exit token: {token} — {exit_line}. \
        You MUST call `exit_session` next as your final action.{rate_learning_nudge}",
            tid = task_id.0,
            action_str = action.as_str(),
        )}]}),
    )
}

/// Optimistically clears a `Conflict` sub_status before rebasing, so the task
/// is no longer visually flagged while the rebase runs (`WrapUpRebase` in
/// `docs/specs/pr-workflow.allium`).
async fn clear_conflict_sub_status_if_set(state: &McpState, task: &Task) {
    if task.sub_status == SubStatus::Conflict {
        let clear =
            UpdateTaskParams::for_task(task.id).sub_status(SubStatus::default_for(task.status));
        if let Err(e) = state.task_svc.update_task(clear).await {
            tracing::warn!(
                task_id = task.id.0,
                "wrap_up: failed to clear conflict sub_status: {e}"
            );
        }
    }
}

/// Fire-and-forget refresh of the repo's RAG index after a successful rebase
/// fast-forwards the base branch. Never blocks the exit-token response, and
/// never surfaces a failure to the agent.
fn reindex_repo_in_background(state: &McpState, repo_path: String) {
    let reindex_svc =
        crate::service::repo_index::RepoIndexService::new(state.embedding_service.clone());
    tokio::spawn(async move {
        match reindex_svc
            .reindex_if_indexed(std::path::Path::new(&repo_path))
            .await
        {
            Ok(Some(r)) => tracing::info!(
                repo = %repo_path,
                chunks = r.chunks_total,
                "wrap_up re-indexed repo"
            ),
            Ok(None) => tracing::debug!(
                repo = %repo_path,
                "wrap_up: no RAG index, skipping re-index"
            ),
            Err(e) => tracing::warn!(
                repo = %repo_path,
                "wrap_up re-index failed: {e}"
            ),
        }
    });
}

async fn finish_wrap_up_rebase(state: &McpState, id: Option<Value>, task: Task) -> JsonRpcResponse {
    let (worktree, branch) = match resolve_rebase_target(&id, &task) {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    let task_id = task.id;
    let repo_path = task.repo_path.clone();
    let base_branch = task.base_branch.clone();
    let runner = state.runner.clone();

    clear_conflict_sub_status_if_set(state, &task).await;

    let rebase_result = match tokio::task::spawn_blocking(move || {
        tracing::info!(task_id = task_id.0, %branch, "MCP wrap_up rebase starting");
        dispatch::finish_task(
            &dispatch::FinishContext {
                repo_path: &repo_path,
                worktree: &worktree,
                branch: &branch,
                base_branch: &base_branch,
                timeout: crate::process::SUBPROCESS_TIMEOUT,
            },
            &*runner,
        )
    })
    .await
    {
        Ok(r) => r,
        Err(e) => return JsonRpcResponse::err(id, -32603, format!("internal error: {e}")),
    };

    match rebase_result {
        Ok(()) => {
            // The base branch was just fast-forwarded, so repo_path now
            // reflects the merged code.
            reindex_repo_in_background(state, task.repo_path.clone());
            state.notify_task_changed(task_id);
            // Local base provably just moved ahead of origin, and the refs are
            // already current — RefreshRepoSyncStateAfterRebase.
            state.notify_branch_rebased(&task.repo_path);
            let (verify_line, token, exit_line) =
                issue_wrap_up_token(state, task_id, &task.repo_path, WrapUpAction::Rebase).await;
            JsonRpcResponse::ok(
                id,
                json!({"content": [{"type": "text", "text": format!(
                    "wrap_up complete (task {}, action: rebase). The session is NOT yet closed.{verify_line} \
                Exit token: {token} — {exit_line}. \
                You MUST call `exit_session` next as your final action — without it, the tmux window stays alive \
                and the task remains in its current status. Do not stop, and do not call any other tool first.",
                    task_id.0
                )}]}),
            )
        }
        Err(e) => {
            if matches!(e, dispatch::FinishError::RebaseConflict { .. }) {
                let patch = UpdateTaskParams::for_task(task_id).sub_status(SubStatus::Conflict);
                if let Err(e) = state.task_svc.update_task(patch).await {
                    tracing::warn!(
                        task_id = task_id.0,
                        "wrap_up: failed to set conflict sub_status: {e}"
                    );
                }
            }
            state.notify_task_changed(task_id);
            JsonRpcResponse::err(id, -32603, format!("wrap_up failed: {e}"))
        }
    }
}

pub(crate) async fn handle_wrap_up(
    state: &McpState,
    id: Option<Value>,
    _identity: &CallerIdentity,
    args: Value,
) -> JsonRpcResponse {
    let parsed = match parse_args::<WrapUpArgs>(&id, args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    tracing::info!(task_id = parsed.task_id, action = ?parsed.action, "MCP wrap_up");

    let task = match validate_wrap_up_request(state, &id, parsed.task_id).await {
        Ok(t) => t,
        Err(resp) => return resp,
    };

    match parsed.action {
        WrapUpAction::Done | WrapUpAction::Pr => {
            finish_wrap_up_simple(state, id, task.id, &task.repo_path, parsed.action).await
        }
        WrapUpAction::Rebase => finish_wrap_up_rebase(state, id, task).await,
    }
}

pub(crate) async fn handle_exit_session(
    state: &McpState,
    id: Option<Value>,
    _identity: &CallerIdentity,
    args: Value,
) -> JsonRpcResponse {
    let parsed = match parse_args::<ExitSessionArgs>(&id, args) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    let task_id = TaskId(parsed.task_id);

    let task = match fetch_caller_task(&*state.db, &id, task_id).await {
        Ok(t) => t,
        Err(resp) => return resp,
    };

    let token = match parsed.token {
        Some(t) => t,
        None => return JsonRpcResponse::err(id, -32602, ERR_NO_TOKEN),
    };

    // Single call: no more reflect-then-close two-phase dance — the mandatory
    // reflection is the /retro skill, run before exit_session is ever called.
    // Validate token, action, and window liveness, then remove the token —
    // all in one write-lock so a concurrent second call can't observe a
    // half-consumed token.
    let (action, pr_url) = {
        let mut map = state.exit_tokens.write().unwrap_or_else(|e| e.into_inner());
        let stored_action = match map.get(&task_id) {
            None => return JsonRpcResponse::err(id, -32602, ERR_NO_TOKEN),
            Some(et) if et.token != token => {
                return JsonRpcResponse::err(id, -32602, "invalid exit token")
            }
            Some(et) => et.action,
        };
        let action = match parsed.action {
            Some(a) => a,
            None => {
                return JsonRpcResponse::err(
                    id,
                    -32602,
                    "action is required — pass the same action used in wrap_up",
                )
            }
        };
        if action != stored_action {
            return JsonRpcResponse::err(
                id,
                -32602,
                format!(
                    "exit token was issued for wrap_up(action=\"{}\"), but exit_session was called \
                    with action=\"{}\"",
                    stored_action.as_str(),
                    action.as_str()
                ),
            );
        }
        if task.tmux_window.is_none() {
            return JsonRpcResponse::err(
                id,
                -32602,
                format!("task #{} has no active session", parsed.task_id),
            );
        }
        let pr_url = if action == WrapUpAction::Pr {
            match parsed.pr_url.filter(|u| !u.is_empty()) {
                Some(u) => Some(u),
                None => return JsonRpcResponse::err(
                    id,
                    -32602,
                    "pr_url is required for action 'pr' — pass the URL returned by `gh pr create`",
                ),
            }
        } else {
            None
        };
        map.remove(&task_id);
        (action, pr_url)
    };

    let outcome = match (action, pr_url) {
        (WrapUpAction::Pr, Some(pr_url)) => crate::service::CloseSessionOutcome::Review {
            pr_url: crate::models::TaskUrl::new(pr_url, crate::models::UrlType::Pr),
        },
        // pr_url is validated as required above whenever action = Pr, so this
        // (Pr, None) arm is unreachable in practice — Done is a safe, non-panicking
        // fallback rather than asserting an invariant the compiler can't see.
        (WrapUpAction::Pr, None) | (WrapUpAction::Rebase, _) | (WrapUpAction::Done, _) => {
            crate::service::CloseSessionOutcome::Done
        }
    };
    // `close_persisted` in `ExitSession` (docs/specs/pr-workflow.allium): the
    // terminal mutation, the tmux teardown and the trailing SessionClosed
    // emission are all gated on this single write landing. Only consuming the
    // token (already done above) happens either way. `close_session` exists so
    // that gate is sound — its `Err` means the write did not land and nothing
    // else. See its doc comment before swapping in a generic `update_task`.
    let close_result = state.task_svc.close_session(task_id, outcome).await;
    state.notify_task_changed(task_id);
    if let Some(epic_id) = task.epic_id {
        state.notify_epic_changed(epic_id);
    }

    let closed = match close_result {
        Ok(closed) => closed,
        Err(e) => {
            tracing::warn!(
                task_id = task_id.0,
                "exit_session: failed to apply closing patch: {e}"
            );
            // Still a successful response: the exit token was consumed above, so
            // an error would strand the agent with no retry path and no session.
            // The text is what carries the failure. Neither the teardown nor the
            // chain runs — the task keeps its live window and its `tmux_window`
            // reference, so it never satisfies `is_detached` and cannot drift
            // into the awaiting-merge rendering, and a broken close is never
            // compounded into a second dispatch.
            return JsonRpcResponse::ok(
                id,
                json!({"content": [{"type": "text", "text": format!(
                    "Task #{} could NOT be moved to its terminal status — the close did not take \
                     effect, and your tmux session is still alive. Do not treat this as a completed \
                     close: the task is still in its previous status and needs closing by hand.",
                    task_id.0
                )}]}),
            );
        }
    };

    // Past this point the close persisted, so both of the following are
    // unconditional. The window comes from the close itself, not from the
    // pre-read task: it is the row the close actually cleared.
    let tmux_window = closed.window;
    let runner = state.runner.clone();
    let bg_done = state.test_hooks.bg_write_done_tx.clone();
    tokio::task::spawn_blocking(move || {
        if let Some(window) = &tmux_window {
            let _ = crate::tmux::kill_window(window, &*runner);
        }
        if let Some(tx) = &bg_done {
            let _ = tx.send(crate::mcp::BackgroundWrite::KillWindow);
        }
    });

    // SessionClosed fires after the terminal patch and the change
    // notifications, so the next subtask's worktree is cut from a base_branch
    // that already contains this task's work. The kill-window teardown above is
    // issued before this point but is detached and never awaited, so its
    // completion is NOT part of this ordering — the window may die before or
    // after SessionClosed and whatever the chain does. See
    // AutoDispatchNextSubtask in docs/specs/epics.allium. Never fails the close:
    // `auto_dispatch_next` swallows every chain problem.
    let text = match task.epic_id {
        Some(epic_id) => match super::dispatch::auto_dispatch_next(state, epic_id).await {
            Some((next_id, next_title)) => format!(
                "Session closed. Dispatching next epic subtask #{} '{next_title}'.",
                next_id.0
            ),
            None => "Session closed.".to_string(),
        },
        None => "Session closed.".to_string(),
    };
    JsonRpcResponse::ok(id, json!({"content": [{"type": "text", "text": text}]}))
}
