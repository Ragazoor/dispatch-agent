pub mod handlers;

use std::sync::Arc;

use axum::{Router, routing::post};
use tokio::sync::mpsc;

use crate::db;
use crate::process::ProcessRunner;

pub struct McpState {
    pub db: Arc<dyn db::TaskStore>,
    /// When set, MCP sends a `()` after mutations to trigger an immediate TUI refresh.
    pub notify_tx: Option<mpsc::UnboundedSender<()>>,
    /// Process runner shared with TuiRuntime for executing git/tmux operations.
    pub runner: Arc<dyn ProcessRunner>,
}

impl McpState {
    pub fn notify(&self) {
        if let Some(tx) = &self.notify_tx {
            let _ = tx.send(());
        }
    }
}

pub fn router(
    db: Arc<dyn db::TaskStore>,
    notify_tx: Option<mpsc::UnboundedSender<()>>,
    runner: Arc<dyn ProcessRunner>,
) -> Router {
    let state = Arc::new(McpState { db, notify_tx, runner });
    Router::new()
        .route("/mcp", post(handlers::handle_mcp))
        .with_state(state)
}

pub async fn serve(
    db: Arc<dyn db::TaskStore>,
    port: u16,
    notify_tx: mpsc::UnboundedSender<()>,
    runner: Arc<dyn ProcessRunner>,
) -> anyhow::Result<()> {
    let app = router(db, Some(notify_tx), runner);
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
