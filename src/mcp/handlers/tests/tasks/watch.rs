#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;

async fn make_task(
    state: &Arc<McpState>,
    title: &str,
    status: TaskStatus,
) -> crate::models::TaskId {
    state
        .db_write()
        .create_task(CreateTaskRequest {
            title,
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
            phoenix: false,
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn subscribe_to_task_registers_watch() {
    let state = test_state().await;
    let watcher = make_task(&state, "Watcher", TaskStatus::Running).await;
    let target = make_task(&state, "Target", TaskStatus::Running).await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "subscribe_to_task",
            "arguments": {"watcher_task_id": watcher.0, "target_task_id": target.0}
        })),
    )
    .await;

    let text = extract_response_text(&resp);
    assert!(text.contains("Now watching"), "got: {text}");
    let watchers = state.db_write().list_watchers_of(target).await.unwrap();
    assert_eq!(watchers, vec![watcher]);
}

#[tokio::test]
async fn subscribe_to_task_already_finished_does_not_register() {
    let state = test_state().await;
    let watcher = make_task(&state, "Watcher", TaskStatus::Running).await;
    let target = make_task(&state, "Target", TaskStatus::Done).await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "subscribe_to_task",
            "arguments": {"watcher_task_id": watcher.0, "target_task_id": target.0}
        })),
    )
    .await;

    let text = extract_response_text(&resp);
    assert!(text.contains("already finished"), "got: {text}");
    assert!(state
        .db_write()
        .list_watchers_of(target)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn subscribe_to_task_rejects_self_watch() {
    let state = test_state().await;
    let task = make_task(&state, "Solo", TaskStatus::Running).await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "subscribe_to_task",
            "arguments": {"watcher_task_id": task.0, "target_task_id": task.0}
        })),
    )
    .await;

    // Self-watch is rejected as a domain validation error, which
    // `service_err_to_response` surfaces as a JSON-RPC tool error (see
    // `assert_error`/`error_message` usage elsewhere in this suite).
    assert_error(&resp, "watch itself");
}

#[tokio::test]
async fn unsubscribe_from_task_removes_watch() {
    let state = test_state().await;
    let watcher = make_task(&state, "Watcher", TaskStatus::Running).await;
    let target = make_task(&state, "Target", TaskStatus::Running).await;
    state
        .db_write()
        .create_task_watcher(watcher, target)
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "unsubscribe_from_task",
            "arguments": {"watcher_task_id": watcher.0, "target_task_id": target.0}
        })),
    )
    .await;

    let text = extract_response_text(&resp);
    assert!(text.contains("No longer watching"), "got: {text}");
    assert!(state
        .db_write()
        .list_watchers_of(target)
        .await
        .unwrap()
        .is_empty());
}
