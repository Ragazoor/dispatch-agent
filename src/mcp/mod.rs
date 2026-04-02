pub mod handlers;

use std::sync::Arc;

use axum::{routing::post, Router};
use tokio::sync::mpsc;

use crate::db;
use crate::models::TaskId;
use crate::process::ProcessRunner;

/// Events sent from the MCP server to the TUI runtime.
#[derive(Debug)]
pub enum McpEvent {
    /// A mutation occurred — trigger a database refresh.
    Refresh,
    /// A message was sent to an agent — flash the target task's card.
    MessageSent { to_task_id: TaskId },
}

pub struct McpState {
    pub db: Arc<dyn db::TaskStore>,
    /// When set, MCP sends events after mutations to trigger TUI updates.
    pub notify_tx: Option<mpsc::UnboundedSender<McpEvent>>,
    /// Process runner shared with TuiRuntime for executing git/tmux operations.
    pub runner: Arc<dyn ProcessRunner>,
}

impl McpState {
    pub fn notify(&self) {
        if let Some(tx) = &self.notify_tx {
            let _ = tx.send(McpEvent::Refresh);
        }
    }

    pub fn notify_message_sent(&self, to_task_id: TaskId) {
        if let Some(tx) = &self.notify_tx {
            let _ = tx.send(McpEvent::MessageSent { to_task_id });
        }
    }
}

pub fn router(
    db: Arc<dyn db::TaskStore>,
    notify_tx: Option<mpsc::UnboundedSender<McpEvent>>,
    runner: Arc<dyn ProcessRunner>,
) -> Router {
    let state = Arc::new(McpState {
        db,
        notify_tx,
        runner,
    });
    Router::new()
        .route("/mcp", post(handlers::handle_mcp))
        .with_state(state)
}

pub async fn serve(
    db: Arc<dyn db::TaskStore>,
    port: u16,
    notify_tx: mpsc::UnboundedSender<McpEvent>,
    runner: Arc<dyn ProcessRunner>,
) -> anyhow::Result<()> {
    let app = router(db, Some(notify_tx), runner);
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
