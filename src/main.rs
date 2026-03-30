use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing::Level;
use tracing_subscriber::EnvFilter;

use dispatch::{db, models, plan, runtime};
use dispatch::db::TaskStore;

#[derive(Parser)]
#[command(name = "dispatch")]
#[command(about = "A terminal kanban board for dispatching and managing AI agents")]
struct Cli {
    /// Path to the database file
    #[arg(long, env = "DISPATCH_DB", default_value_os_t = default_db_path())]
    db: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Launch the TUI interface
    Tui {
        /// MCP server port
        #[arg(long, env = "DISPATCH_PORT", default_value = "3142")]
        port: u16,
        /// Seconds of unchanged tmux output before marking agent stale
        #[arg(long, env = "DISPATCH_INACTIVITY_TIMEOUT", default_value = "180")]
        inactivity_timeout: u64,
    },
    /// Update a task's status
    Update {
        /// Task ID
        id: i64,
        /// New status
        status: String,
        /// Only update if current status matches this value
        #[arg(long)]
        only_if: Option<String>,
        /// Mark the task as needing human input (e.g. permission prompt)
        #[arg(long)]
        needs_input: bool,
    },
    /// List tasks
    List {
        /// Filter by status
        #[arg(long)]
        status: Option<String>,
    },
    /// Create a task from a plan file
    Create {
        /// Path to the plan markdown file
        #[arg(long)]
        from_plan: PathBuf,

        /// Target repository path (defaults to current directory)
        #[arg(long)]
        repo_path: Option<PathBuf>,

        /// Override the title extracted from the plan
        #[arg(long)]
        title: Option<String>,

        /// Override the description extracted from the plan
        #[arg(long)]
        description: Option<String>,
    },
    /// Attach a plan file to an existing task
    Plan {
        /// Task ID
        id: i64,
        /// Path to the plan file
        path: PathBuf,
    },
    /// Configure Claude Code to allow agents to use the MCP server
    Setup {
        /// MCP server port
        #[arg(long, env = "DISPATCH_PORT", default_value = "3142")]
        port: u16,
    },
}

fn parse_status(s: &str) -> anyhow::Result<models::TaskStatus> {
    models::TaskStatus::parse(s).ok_or_else(|| {
        anyhow::anyhow!(
            "Unknown status: {s}. Valid values: backlog, ready, running, review, done"
        )
    })
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
    base.join("dispatch").join("tasks.db")
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Tui { port, inactivity_timeout } => {
            let data_dir = cli.db.parent().unwrap_or(std::path::Path::new("."));
            std::fs::create_dir_all(data_dir)?;
            let log_path = data_dir.join("app.log");
            let log_file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)?;
            tracing_subscriber::fmt()
                .with_writer(log_file)
                .with_ansi(false)
                .with_env_filter(
                    EnvFilter::from_default_env().add_directive(Level::INFO.into()),
                )
                .init();
            runtime::run_tui(&cli.db, port, inactivity_timeout).await?;
        }
        Commands::Update { id, status, only_if, needs_input } => {
            let new_status = parse_status(&status)?;
            let db = db::Database::open(&cli.db)?;
            let task_id = models::TaskId(id);
            if let Some(ref condition) = only_if {
                let expected = parse_status(condition)?;
                let updated = db.update_status_if(task_id, new_status, expected)?;
                if updated {
                    db.patch_task(task_id, &db::TaskPatch::new().needs_input(needs_input))?;
                    println!("Task {} updated to {}", id, status);
                } else {
                    println!("Task {} not updated (status is not {})", id, condition);
                }
            } else {
                db.patch_task(task_id, &db::TaskPatch::new().status(new_status).needs_input(needs_input))?;
                println!("Task {} updated to {}", id, status);
            }
        }
        Commands::List { status } => {
            let db = db::Database::open(&cli.db)?;
            let tasks = match status {
                Some(s) => {
                    let filter = parse_status(&s)?;
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
        Commands::Create { from_plan, repo_path, title, description } => {
            let content = std::fs::read_to_string(&from_plan)
                .map_err(|e| anyhow::anyhow!("Failed to read plan file {}: {}", from_plan.display(), e))?;

            let metadata = plan::parse_plan(&content)?;

            let title = title.unwrap_or(metadata.title);
            let description = description.unwrap_or(metadata.description);

            let repo_path = repo_path
                .or_else(|| std::env::current_dir().ok())
                .ok_or_else(|| anyhow::anyhow!("Could not determine repo path. Use --repo-path."))?;
            let repo_path_str = repo_path.to_string_lossy();

            let plan_path = std::fs::canonicalize(&from_plan)
                .map_err(|e| anyhow::anyhow!("Failed to resolve plan path {}: {}", from_plan.display(), e))?;
            let plan_str = plan_path.to_string_lossy();

            let db = db::Database::open(&cli.db)?;

            if let Some(existing) = db.find_task_by_plan(&plan_str)? {
                println!("Task #{} already exists for this plan [{}]", existing.id, existing.status.as_str());
                return Ok(());
            }

            let id = db.create_task(&title, &description, &repo_path_str, Some(&plan_str), models::TaskStatus::Ready)?;
            println!("Created task #{}: \"{}\" [ready]", id, title);
        }
        Commands::Setup { port } => {
            dispatch::setup::run_setup(port)?;
        }
        Commands::Plan { id, path } => {
            if !path.exists() {
                anyhow::bail!("Plan file not found: {}", path.display());
            }
            let plan_path = std::fs::canonicalize(&path)
                .map_err(|e| anyhow::anyhow!("Failed to resolve plan path {}: {}", path.display(), e))?;
            let plan_str = plan_path.to_string_lossy();

            let db = db::Database::open(&cli.db)?;
            db.patch_task(models::TaskId(id), &db::TaskPatch::new().plan(Some(&plan_str)))?;
            println!("Plan attached to task #{}: {}", id, plan_str);
        }
    }

    Ok(())
}


