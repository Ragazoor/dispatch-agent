#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;

mod async_handle;
mod epics;
mod learnings;
mod migrations;
mod read_pool;
mod settings;
mod tasks;
mod todos;
mod usage;

pub(super) async fn in_memory_db() -> Database {
    Database::open_in_memory().await.unwrap()
}

pub(super) async fn create_task_returning(
    db: &Database,
    title: &str,
    description: &str,
    repo_path: &str,
    plan: Option<&str>,
    status: TaskStatus,
) -> anyhow::Result<Task> {
    let id = db
        .create_task(CreateTaskRequest {
            title,
            description,
            repo_path,
            plan,
            status,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await?;
    db.get_task(id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Task {id} vanished after insert"))
}
