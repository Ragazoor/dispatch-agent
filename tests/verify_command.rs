#![allow(clippy::unwrap_used)]
//! Integration: verify_command stored under a path string equal to
//! task.repo_path is found by the wrap-up lookup when called with
//! the repo_path from a real task row.

use dispatch_tui::db::{CreateTaskRequest, Database, SettingsStore, TaskCrud, TaskRead};
use dispatch_tui::dispatch::fetch_verify_command;
use dispatch_tui::models::TaskStatus;

#[tokio::test]
async fn verify_command_lookup_matches_task_repo_path() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let db = Database::open(tmp.path()).await.unwrap();

    // Register a verify command for the repo path.
    db.set_verify_command("/home/me/repo", Some("cargo test"))
        .await
        .unwrap();

    // Create a real task row with repo_path pointing at that repo.
    let task_id = db
        .create_task(CreateTaskRequest {
            title: "Test task",
            description: "desc",
            repo_path: "/home/me/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    // Load the task back from the DB — this is the same call chain as dispatch.
    let task = db.get_task(task_id).await.unwrap().unwrap();

    // Use task.repo_path as the key, exactly as exec_dispatch_agent does.
    let fetched = fetch_verify_command(&db, &task.repo_path).await;
    assert_eq!(fetched.as_deref(), Some("cargo test"));
}

#[tokio::test]
async fn verify_command_lookup_returns_none_for_unregistered_path() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let db = Database::open(tmp.path()).await.unwrap();

    let fetched = fetch_verify_command(&db, "/not/registered").await;
    assert_eq!(fetched, None);
}

#[tokio::test]
async fn verify_command_lookup_requires_exact_path_match() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let db = Database::open(tmp.path()).await.unwrap();

    // Store under expanded path
    db.set_verify_command("/home/me/repo", Some("cargo test"))
        .await
        .unwrap();

    // Exact match works
    let found = fetch_verify_command(&db, "/home/me/repo").await;
    assert_eq!(found.as_deref(), Some("cargo test"));

    // Different string (e.g. trailing slash) does NOT match
    let not_found = fetch_verify_command(&db, "/home/me/repo/").await;
    assert_eq!(not_found, None, "trailing slash must not match");
}
