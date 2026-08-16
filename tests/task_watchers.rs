#![allow(clippy::unwrap_used, clippy::expect_used)]
//! End-to-end integration test: full task-watcher flow — subscribe, finish,
//! notified. Exercises the production MCP router (`subscribe_to_task`) plus
//! `TaskService::update_task` (the ConfirmDone-equivalent status transition)
//! together, over a real in-memory `Database` and a `MockProcessRunner`, to
//! lock in the wiring from `docs/specs/task-watchers.allium` as a regression
//! guard.
//!
//! `McpState::db_write()` is `#[cfg(test)]`-gated and unavailable to
//! integration tests under `tests/` (which link against the crate compiled
//! without `cfg(test)`), so every fixture here is seeded through the public
//! API only: `Database::open_in_memory()` directly, the MCP router's
//! JSON-RPC calls, and `TaskService` directly.

mod common;

use std::sync::Arc;

use serde_json::json;

use dispatch_tui::db::{self, CreateTaskRequest, Database, TaskCrud};
use dispatch_tui::mcp::identity::HEADER_KIND;
use dispatch_tui::mcp::McpDeps;
use dispatch_tui::models::TaskStatus;
use dispatch_tui::process::{MockProcessRunner, ProcessRunner};
use dispatch_tui::service::embeddings::EmbeddingService;
use dispatch_tui::service::{TaskService, UpdateTaskParams};

#[tokio::test]
async fn subscribe_then_finish_delivers_notification() {
    // 1. Set up an in-memory DB + MockProcessRunner-backed MCP router.
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let mock = Arc::new(
        MockProcessRunner::new(vec![
            // tmux capture-pane -p — reports the watcher's pane idle at its
            // own chat input, so notify::notify_tmux's readiness probe lets
            // the nudge through.
            MockProcessRunner::ok_with_stdout(b"> \nauto mode on (shift+tab to cycle) - 1 agent\n"),
            MockProcessRunner::ok(), // tmux send-keys -l (notification text)
            MockProcessRunner::ok(), // tmux send-keys Enter
        ])
        .with_windows(&["task-watcher"]),
    );
    let runner: Arc<dyn ProcessRunner> = mock.clone();

    let router = dispatch_tui::mcp::router(
        McpDeps {
            db: db.clone() as Arc<dyn db::TaskStore>,
            runner: runner.clone(),
            embedding_service: EmbeddingService::new_noop(),
            data_dir: std::env::temp_dir(),
        },
        None,
    );

    // 2. Create task A (the watcher) with a worktree + tmux_window set
    //    (simulating a running agent).
    let tmp = tempfile::tempdir().unwrap();
    let watcher_worktree = tmp.path().to_str().unwrap().to_string();

    let watcher_id = db
        .create_task(CreateTaskRequest {
            title: "Watcher agent",
            description: "watches task B",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
            schedule_interval_secs: None,
            pinned_branch: None,
        })
        .await
        .unwrap();
    db.patch_task(
        watcher_id,
        &db::TaskPatch::new()
            .worktree(Some(&watcher_worktree))
            .tmux_window(Some("task-watcher")),
    )
    .await
    .unwrap();

    // 3. Create task B (the target), status Running.
    let target_id = db
        .create_task(CreateTaskRequest {
            title: "Target task",
            description: "gets watched",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
            schedule_interval_secs: None,
            pinned_branch: None,
        })
        .await
        .unwrap();

    // 4. Call subscribe_to_task(A, B) via the MCP router.
    let resp = common::post_mcp(
        router,
        &[(HEADER_KIND, "session")],
        json!({
            "jsonrpc": "2.0", "id": 1,
            "method": "tools/call",
            "params": {
                "name": "subscribe_to_task",
                "arguments": {
                    "watcher_task_id": watcher_id.0,
                    "target_task_id": target_id.0
                }
            }
        }),
    )
    .await;
    assert!(resp.get("error").is_none(), "got: {resp}");
    let text = resp["result"]["content"][0]["text"]
        .as_str()
        .expect("expected text content");
    assert!(text.contains("Now watching"), "got: {text}");

    let watchers = db.list_watchers_of(target_id).await.unwrap();
    assert_eq!(
        watchers,
        vec![watcher_id],
        "subscription should be registered before the target finishes"
    );

    // 5. Move B to Done via TaskService::update_task directly — the MCP
    //    update_task tool refuses to set status to done/archived (agents
    //    must go through the TUI's ConfirmDone flow instead), so this is the
    //    same status-transition path ConfirmDone/wrap-up ultimately drive.
    let task_svc = TaskService::new(db.clone(), runner);
    task_svc
        .update_task(UpdateTaskParams::for_task(target_id).status(TaskStatus::Done))
        .await
        .unwrap();

    // 6. Assert the MockProcessRunner recorded a capture-pane readiness check
    //    followed by a tmux send-keys call for A's window (3 calls total:
    //    capture-pane, then the established send_keys convention of `-l`
    //    then `Enter`).
    let calls = mock.recorded_calls();
    assert_eq!(
        calls.len(),
        3,
        "expected a capture-pane check plus one tmux notification (send-keys -l + send-keys Enter): {calls:?}"
    );
    assert_eq!(calls[0].0, "tmux");
    assert!(
        calls[0].1.contains(&"capture-pane".to_string()),
        "first call should be the pane-readiness check: {:?}",
        calls[0]
    );
    assert_eq!(calls[1].0, "tmux");
    assert!(
        calls[1].1.contains(&"-l".to_string()),
        "second call should be send-keys -l: {:?}",
        calls[1]
    );
    // A's window is targeted by its resolved pane ID, not its name — tmux
    // resolves a bare `-t <name>` by prefix, so every window target goes through
    // `tmux::window_target` first.
    assert!(
        calls[1].1.contains(&mock.pane_id_of("task-watcher")),
        "send-keys -l should target A's tmux window: {:?}",
        calls[1]
    );
    assert_eq!(calls[2].0, "tmux");
    assert!(
        calls[2].1.contains(&"Enter".to_string()),
        "third call should be send-keys Enter: {:?}",
        calls[2]
    );

    // 7. Assert a file was written under A's worktree's .claude-messages/ directory.
    let messages_dir = tmp.path().join(".claude-messages");
    assert!(
        messages_dir.is_dir(),
        ".claude-messages directory should exist under A's worktree"
    );
    let entries: Vec<_> = std::fs::read_dir(&messages_dir).unwrap().collect();
    assert_eq!(entries.len(), 1, "should have exactly one message file");
    let message_path = entries[0].as_ref().unwrap().path();
    let file_name = message_path.file_name().unwrap().to_str().unwrap();
    assert!(
        file_name.starts_with(&format!("watch-finished-{}-", target_id.0)),
        "filename should identify the finished watch target: {file_name}"
    );
    assert!(file_name.ends_with(".md"), "filename should end with .md");
    let content = std::fs::read_to_string(&message_path).unwrap();
    assert!(
        content.contains("Target task"),
        "message should mention the target task's title: {content}"
    );
    assert!(
        content.contains("done"),
        "message should mention the new status: {content}"
    );

    // 8. Assert list_watchers_of(B) is now empty (subscription consumed).
    assert!(
        db.list_watchers_of(target_id).await.unwrap().is_empty(),
        "subscription should be cleared after the notification fires"
    );
}
