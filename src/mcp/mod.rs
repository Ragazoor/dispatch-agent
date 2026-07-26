pub mod handlers;
pub mod identity;
pub mod middleware;
pub mod trajectory;

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use uuid::Uuid;

use axum::{routing::post, Router};
use tokio::sync::mpsc;

use crate::db;
use crate::models::{EpicId, TaskId};
use crate::process::ProcessRunner;
use crate::service::embeddings::EmbeddingService;
use crate::service::{
    EpicService, EpicServiceApi, LearningService, LearningServiceApi, TaskService, TaskServiceApi,
};

/// Events sent from the MCP server to the TUI runtime.
#[derive(Debug)]
pub enum McpEvent {
    /// Catch-all "I don't know what changed" — full reload of tasks, epics, and usage.
    /// Prefer the targeted variants below when the changed entity is known.
    Refresh,
    /// A single task changed — reload just that row.
    TaskChanged(TaskId),
    /// A single epic changed — reload just that row (and the epic's task list,
    /// since feed-sync changes appear here as a batch update for the epic).
    EpicChanged(EpicId),
    /// A message was sent to an agent — flash the target task's card.
    MessageSent { to_task_id: TaskId },
}

/// Identifies a fire-and-forget background write performed by the MCP handler.
///
/// Production code never observes these; the variants exist so tests can await
/// a specific detached write deterministically (via `bg_write_done_tx`) instead
/// of sleeping. See `docs/conventions.md` ("No `tokio::time::sleep` in tests").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundWrite {
    /// A usage event was recorded.
    Usage,
    /// A trajectory entry was appended.
    Trajectory,
}

/// The wrap-up action a task is being closed out with. Shared between
/// `wrap_up` (which issues an `ExitToken` recording it) and `exit_session`
/// (which validates the closing call's action against it), so it lives here
/// rather than in a handler submodule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum WrapUpAction {
    Rebase,
    Done,
    Pr,
}

impl WrapUpAction {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            WrapUpAction::Rebase => "rebase",
            WrapUpAction::Done => "done",
            WrapUpAction::Pr => "pr",
        }
    }
}

/// One-time token linking a wrap_up call to its exit_session close.
/// `action` records which wrap_up action issued it, so exit_session can
/// reject a call whose action doesn't match (e.g. issued for "rebase" but
/// closed with "pr").
pub(crate) struct ExitToken {
    pub(crate) token: String,
    pub(crate) action: WrapUpAction,
}

/// Shared dependencies threaded through the MCP entry points.
/// Bundles the four fields that appear in every signature so callers
/// construct one struct instead of passing a 5–6-argument list.
pub struct McpDeps {
    pub db: Arc<dyn db::TaskStore>,
    pub runner: Arc<dyn ProcessRunner>,
    pub embedding_service: Arc<EmbeddingService>,
    pub data_dir: std::path::PathBuf,
}

pub struct McpState {
    /// Read-only DB handle. Task/epic *mutations* must go through `task_svc` /
    /// `epic_svc` — calling a mutating method here is a compile error. See the
    /// mutation-boundary section of `docs/conventions.md`.
    pub db: Arc<dyn db::TaskReadStore>,
    pub task_svc: Arc<dyn TaskServiceApi>,
    pub epic_svc: Arc<dyn EpicServiceApi>,
    pub learning_svc: Arc<dyn LearningServiceApi>,
    /// When set, MCP sends events after mutations to trigger TUI updates.
    pub notify_tx: Option<mpsc::UnboundedSender<McpEvent>>,
    /// Process runner shared with TuiRuntime for executing git/tmux operations.
    pub runner: Arc<dyn ProcessRunner>,
    /// Embedding service used for RAG-based query_learnings and for computing
    /// embeddings when a learning is recorded via MCP.
    pub embedding_service: Arc<EmbeddingService>,
    /// In-memory tokens issued by wrap_up, consumed by exit_session.
    pub(crate) exit_tokens: Arc<RwLock<HashMap<TaskId, ExitToken>>>,
    /// Dispatch data directory (parent of the SQLite DB). Trajectory files are
    /// written here under `trajectories/<task_id>.jsonl`.
    pub data_dir: std::path::PathBuf,
    /// Fields that exist only to make async tests deterministic. See [`TestHooks`].
    pub(crate) test_hooks: TestHooks,
}

/// Test-support fields grouped out of [`McpState`]'s field list. Not all of
/// these are `#[cfg(test)]` — `bg_write_done_tx` is read unconditionally by
/// production code (`handlers/dispatch.rs`, `router_with_bg_done`) and is
/// simply always `None` outside tests, whereas `db_write` is compiled only
/// under `#[cfg(test)]`.
pub(crate) struct TestHooks {
    /// Fires with a [`BackgroundWrite`] tag after each fire-and-forget
    /// background write (usage, trajectory) lands, so tests can await it
    /// deterministically instead of sleeping.
    pub(crate) bg_write_done_tx: Option<mpsc::UnboundedSender<BackgroundWrite>>,
    /// Write-capable handle for seeding DB fixtures directly (production
    /// mutations go through `task_svc`/`epic_svc`). Reachable only via
    /// [`McpState::db_write`].
    #[cfg(test)]
    pub(crate) db_write: Arc<dyn db::TaskStore>,
}

impl McpState {
    pub fn new(deps: McpDeps, notify_tx: Option<mpsc::UnboundedSender<McpEvent>>) -> Self {
        let task_svc: Arc<dyn TaskServiceApi> =
            Arc::new(TaskService::new(deps.db.clone(), deps.runner.clone()));
        let epic_svc: Arc<dyn EpicServiceApi> = Arc::new(EpicService::new(deps.db.clone()));
        let learning_svc: Arc<dyn LearningServiceApi> = Arc::new(LearningService::new(
            deps.db.clone(),
            deps.embedding_service.clone(),
        ));
        // Narrow the write-capable dependency handle to the read-only surface
        // consumers are allowed to touch. Mutations go through the services above.
        let db: Arc<dyn db::TaskReadStore> = deps.db.clone();
        Self {
            db,
            task_svc,
            epic_svc,
            learning_svc,
            notify_tx,
            runner: deps.runner,
            embedding_service: deps.embedding_service,
            exit_tokens: Arc::new(RwLock::new(HashMap::new())),
            data_dir: deps.data_dir,
            test_hooks: TestHooks {
                bg_write_done_tx: None,
                #[cfg(test)]
                db_write: deps.db,
            },
        }
    }

    pub fn notify(&self) {
        if let Some(tx) = &self.notify_tx {
            let _ = tx.send(McpEvent::Refresh);
        }
    }

    /// Test-only write handle for seeding DB fixtures directly. Not available in
    /// production builds, so handler code keeps going through the services.
    #[cfg(test)]
    pub(crate) fn db_write(&self) -> &Arc<dyn db::TaskStore> {
        &self.test_hooks.db_write
    }

    pub fn notify_message_sent(&self, to_task_id: TaskId) {
        if let Some(tx) = &self.notify_tx {
            let _ = tx.send(McpEvent::MessageSent { to_task_id });
        }
    }

    /// Notify the runtime that a single task changed. Prefer this over
    /// `notify()` whenever the affected `task_id` is known: it lets the
    /// runtime reload one row instead of all tasks.
    pub fn notify_task_changed(&self, task_id: TaskId) {
        if let Some(tx) = &self.notify_tx {
            let _ = tx.send(McpEvent::TaskChanged(task_id));
        }
    }

    /// Notify the runtime that a single epic changed. Use this for epic
    /// updates and for feed-sync batches (one event per sync, not per task).
    pub fn notify_epic_changed(&self, epic_id: EpicId) {
        if let Some(tx) = &self.notify_tx {
            let _ = tx.send(McpEvent::EpicChanged(epic_id));
        }
    }

    /// Issue a fresh exit token for a task, overwriting any existing one.
    /// Records which action issued it (validated against on exit_session).
    /// Returns the token string to embed in the response.
    pub(crate) fn issue_exit_token(&self, task_id: TaskId, action: WrapUpAction) -> String {
        let token = Uuid::new_v4().to_string();
        self.exit_tokens
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                task_id,
                ExitToken {
                    token: token.clone(),
                    action,
                },
            );
        token
    }
}

pub fn router(deps: McpDeps, notify_tx: Option<mpsc::UnboundedSender<McpEvent>>) -> Router {
    router_with_bg_done(deps, notify_tx, None)
}

/// Like [`router`], but installs a test-only completion signal that fires after
/// each fire-and-forget background write (usage, trajectory). Lets integration
/// tests await detached writes deterministically instead of sleeping.
pub fn router_with_bg_done(
    deps: McpDeps,
    notify_tx: Option<mpsc::UnboundedSender<McpEvent>>,
    bg_write_done_tx: Option<mpsc::UnboundedSender<BackgroundWrite>>,
) -> Router {
    let mut state = McpState::new(deps, notify_tx);
    state.test_hooks.bg_write_done_tx = bg_write_done_tx;
    let state = Arc::new(state);
    Router::new()
        .route("/mcp", post(handlers::handle_mcp))
        .layer(axum::middleware::from_fn(
            middleware::extract_caller_identity,
        ))
        .with_state(state)
}

pub async fn serve(
    deps: McpDeps,
    port: u16,
    notify_tx: mpsc::UnboundedSender<McpEvent>,
) -> anyhow::Result<()> {
    let app = router(deps, Some(notify_tx));
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
