use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use task_orchestrator::{db, models, runtime};

#[derive(Parser)]
#[command(name = "task-orchestrator")]
#[command(about = "A TUI task orchestrator for managing agent-driven development tasks")]
struct Cli {
    /// Path to the database file
    #[arg(long, env = "TASK_ORCHESTRATOR_DB", default_value_os_t = default_db_path())]
    db: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Launch the TUI interface
    Tui {
        /// MCP server port
        #[arg(long, env = "TASK_ORCHESTRATOR_PORT", default_value = "3142")]
        port: u16,
    },
    /// Update a task's status
    Update {
        /// Task ID
        id: i64,
        /// New status
        status: String,
    },
    /// List tasks
    List {
        /// Filter by status
        #[arg(long)]
        status: Option<String>,
    },
}

fn default_db_path() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            home.join(".local").join("share")
        });
    base.join("task-orchestrator").join("tasks.db")
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Tui { port } => {
            runtime::run_tui(&cli.db, port).await?;
        }
        Commands::Update { id, status } => {
            let new_status = models::TaskStatus::parse(&status)
                .ok_or_else(|| anyhow::anyhow!("Unknown status: {}", status))?;
            let db = db::Database::open(&cli.db)?;
            db.update_status(id, new_status)?;
            println!("Task {} updated to {}", id, status);
        }
        Commands::List { status } => {
            let db = db::Database::open(&cli.db)?;
            let tasks = match status {
                Some(s) => {
                    let filter = models::TaskStatus::parse(&s)
                        .ok_or_else(|| anyhow::anyhow!("Unknown status: {}", s))?;
                    db.list_by_status(filter)?
                }
                None => db.list_all()?,
            };
            if tasks.is_empty() {
                println!("No tasks found.");
            } else {
                for task in tasks {
                    println!("[{}] {} - {} ({})", task.id, task.title, task.status.as_str(), task.repo_path);
                }
            }
        }
    }

    Ok(())
}


