#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;

mod async_handle;
mod epics;
mod hooks;
mod learnings;
mod migrations;
mod read_pool;
mod schema_template;
mod settings;
mod shells;
mod subagents;
mod tasks;
mod todos;
mod usage;

pub(super) async fn in_memory_db() -> Database {
    Database::open_in_memory().await.unwrap()
}

/// Run `sql` on the writer connection with CHECK constraints disabled, so a
/// test can plant a row the application-level API would never produce (e.g. an
/// unrecognised `status` string). Used to exercise the decode-failure policy —
/// see the decode-failure-policy section of `docs/conventions.md`.
pub(super) async fn write_corrupt_row(db: &Database, sql: &'static str) {
    db.db_call(move |conn| {
        conn.execute_batch("PRAGMA ignore_check_constraints = ON;")?;
        let result = conn.execute_batch(sql);
        conn.execute_batch("PRAGMA ignore_check_constraints = OFF;")?;
        result.map_err(anyhow::Error::from)
    })
    .await
    .unwrap();
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
            phoenix: false,
        })
        .await?;
    db.get_task(id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Task {id} vanished after insert"))
}

/// Create a backlog task and unwrap, for tests that don't care about
/// [`create_task_returning`]'s `Result`. Shared by `subagents` and `shells`,
/// which otherwise each declared an identical private copy.
pub(super) async fn make_task(db: &Database, title: &str) -> Task {
    create_task_returning(db, title, "desc", "/repo", None, TaskStatus::Backlog)
        .await
        .unwrap()
}
