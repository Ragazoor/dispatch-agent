use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing::Level;
use tracing_subscriber::EnvFilter;

use dispatch_tui::db::{SettingsStore, TaskRead};
use dispatch_tui::models::expand_tilde;
use dispatch_tui::tui::ui::truncate;
use dispatch_tui::{db, models, runtime, service};

#[derive(Parser)]
#[command(name = "dispatch")]
#[command(about = "A terminal kanban board for dispatching and managing AI agents")]
#[command(version)]
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
        #[arg(long, env = "DISPATCH_PORT", default_value_t = dispatch_tui::DEFAULT_PORT)]
        port: u16,
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
        /// Set the sub-status (e.g. active, needs_input, stale, crashed, awaiting_review)
        #[arg(long)]
        sub_status: Option<String>,
        /// Mark the task as needing human input (deprecated, use --sub-status needs_input)
        #[arg(long)]
        needs_input: bool,
    },
    /// List tasks
    List {
        /// Filter by status
        #[arg(long)]
        status: Option<String>,
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
        #[arg(long, env = "DISPATCH_PORT", default_value_t = dispatch_tui::DEFAULT_PORT)]
        port: u16,
        /// Skip confirmation prompts
        #[arg(long, short)]
        yes: bool,
    },
    /// Remove dispatch configuration from Claude Code
    Uninstall {
        /// Skip confirmation prompt
        #[arg(long, short)]
        yes: bool,
        /// Also delete the database and log files
        #[arg(long)]
        purge: bool,
    },
    /// Record a Claude Code hook event for a task
    Hook {
        /// Task ID
        id: i64,
        /// Hook event kind: pre_tool_use | notification | stop
        kind: String,
        /// Notification subtype from the payload's `notification_type` field
        /// (e.g. permission_prompt, auth_success, elicitation_complete). Only
        /// meaningful for the `notification` event; ignored otherwise. Absent
        /// or unrecognised values fall back to the `needs_input` behaviour.
        #[arg(long = "kind")]
        notification_kind: Option<String>,
    },
    /// Gate `gh pr create`: block the first attempt for a task with a reminder
    /// to consult PR learnings, then allow subsequent attempts. Exits 2 to
    /// block (Claude Code PreToolUse block signal), 0 to allow.
    PrGate {
        /// Task ID
        id: i64,
    },
    /// Run a feed command and validate its output as FeedItem JSON
    VerifyFeed {
        /// Shell command to run (executed via sh -c)
        command: String,
    },
    /// Emit a JSON object of HTTP headers identifying the current caller.
    ///
    /// Used as a headersHelper in Claude Code's ~/.claude.json — invoked on every
    /// MCP session start and reconnect. Pure path parser; reads only $PWD,
    /// no DB access.
    CallerHeaders,
    /// Manage per-repo settings (verify command, etc.).
    Repo {
        #[command(subcommand)]
        action: RepoAction,
    },
    /// Remove repo paths that no longer exist on the filesystem.
    PruneRepoPaths,
    /// Self-diagnosis: detect (and optionally repair) common install inconsistencies.
    Doctor {
        #[command(subcommand)]
        check: Option<DoctorCheck>,
        /// Emit structured JSON instead of human-readable lines.
        #[arg(long)]
        json: bool,
        /// Explicitly request detection-only mode; overrides --repair if both are set.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum RepoAction {
    /// Set the verify command for a repo path. Creates the path entry if it doesn't exist.
    SetVerify { path: String, command: String },
    /// Clear the verify command for a repo path.
    ClearVerify { path: String },
    /// List known repo paths and their verify commands.
    List,
}

#[derive(Subcommand)]
enum DoctorCheck {
    /// Compare .worktrees/ directories against DB task rows.
    Worktrees {
        /// Apply available repairs (default is dry-run: detect only).
        #[arg(long)]
        repair: bool,
        /// Skip confirmation prompts when using --repair.
        #[arg(long)]
        force: bool,
        /// Emit structured JSON instead of human-readable lines.
        #[arg(long)]
        json: bool,
    },
    /// Compare tasks.tmux_window values against live tmux windows.
    Sessions {
        /// Apply available repairs (default is dry-run: detect only).
        #[arg(long)]
        repair: bool,
        /// Skip confirmation prompts when using --repair.
        #[arg(long)]
        force: bool,
        /// Emit structured JSON instead of human-readable lines.
        #[arg(long)]
        json: bool,
    },
    /// Verify git config core.hooksPath = .githooks for each known repo.
    Hooks {
        /// Apply available repairs (default is dry-run: detect only).
        #[arg(long)]
        repair: bool,
        /// Skip confirmation prompts when using --repair.
        #[arg(long)]
        force: bool,
        /// Emit structured JSON instead of human-readable lines.
        #[arg(long)]
        json: bool,
    },
}

fn parse_status(s: &str) -> anyhow::Result<models::TaskStatus> {
    models::TaskStatus::parse(s).ok_or_else(|| {
        anyhow::anyhow!("Unknown status: {s}. Valid values: backlog, running, review, done")
    })
}

fn default_db_path() -> PathBuf {
    dispatch_tui::default_db_path()
}

// ---------------------------------------------------------------------------
// Per-subcommand handlers
// ---------------------------------------------------------------------------

/// Initialise a `tracing_subscriber` appending to `<data_dir>/app.log`, so
/// this process's `tracing::warn!`/`info!` calls (including a slow `db_call`
/// warning — see `docs/specs/observability.allium`'s `DbCallSlowWarning`
/// rule) are actually persisted rather than silently dropped.
fn init_app_log_subscriber(data_dir: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let log_path = data_dir.join("app.log");
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    tracing_subscriber::fmt()
        .with_writer(log_file)
        .with_ansi(false)
        .with_env_filter(EnvFilter::from_default_env().add_directive(Level::INFO.into()))
        .init();
    Ok(())
}

async fn cmd_tui(db: &std::path::Path, port: u16) -> Result<()> {
    let data_dir = db.parent().unwrap_or(std::path::Path::new("."));
    init_app_log_subscriber(data_dir)?;
    runtime::run_tui(db, port).await
}

async fn cmd_update(
    db: &std::path::Path,
    id: i64,
    status: String,
    only_if: Option<String>,
    sub_status: Option<String>,
    needs_input: bool,
) -> Result<()> {
    let new_status = parse_status(&status)?;
    let database = db::Database::open(db).await?;
    let task_id = models::TaskId(id);
    let resolved_sub_status = if let Some(ref ss) = sub_status {
        Some(
            models::SubStatus::parse(ss)
                .ok_or_else(|| anyhow::anyhow!("Invalid sub-status: {}", ss))?,
        )
    } else if needs_input {
        Some(models::SubStatus::NeedsInput)
    } else {
        None
    };
    let only_if_status = only_if.as_deref().map(parse_status).transpose()?;
    let svc = service::TaskService::new_with_real_runner(std::sync::Arc::new(database));
    match svc
        .cli_update_task(task_id, new_status, only_if_status, resolved_sub_status)
        .await
    {
        Ok(true) => println!("Task {} updated to {}", id, status),
        Ok(false) => println!(
            "Task {} not updated (status is not {})",
            id,
            only_if.as_deref().unwrap_or("?")
        ),
        Err(e) if e.to_string().contains("not found") => {
            eprintln!("Task {} not found, skipping", id);
        }
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

async fn cmd_pr_gate(db: &std::path::Path, id: i64) -> Result<()> {
    let database = db::Database::open(db).await?;
    let svc = service::TaskService::new_with_real_runner(std::sync::Arc::new(database));
    let first_time = svc.mark_pr_learnings_gate_shown(models::TaskId(id)).await?;
    if first_time {
        eprintln!(
            "Before creating this PR, consult the knowledge base for PR conventions: \
             call the dispatch `query_learnings` MCP tool (e.g. tag_filter: [\"pr\"]), \
             apply what you find to the PR title and body, then re-run `gh pr create`."
        );
        std::process::exit(2);
    }
    Ok(())
}

async fn cmd_hook(
    db: &std::path::Path,
    id: i64,
    kind: String,
    notification_kind: Option<String>,
) -> Result<()> {
    // The notification subtype (from `--kind`) is only meaningful for the
    // `notification` event; build it directly instead of parsing then
    // overwriting. An absent or unrecognised value stays `None`, which the
    // service maps to the raise/`needs_input` path for backward compatibility.
    let parsed = if kind == "notification" {
        models::HookEventKind::Notification(
            notification_kind
                .as_deref()
                .and_then(models::NotificationKind::parse),
        )
    } else {
        models::HookEventKind::parse(&kind).ok_or_else(|| {
            anyhow::anyhow!(
                "Invalid hook kind: {kind}. Valid: pre_tool_use, notification, stop, user_prompt_submit"
            )
        })?
    };
    let data_dir = db.parent().unwrap_or(std::path::Path::new("."));
    init_app_log_subscriber(data_dir)?;
    let database = db::Database::open(db).await?;
    let svc = service::TaskService::new_with_real_runner(std::sync::Arc::new(database));
    match svc.record_hook_event(models::TaskId(id), parsed).await {
        Ok(()) => {}
        Err(service::ServiceError::NotFound(_)) => {
            eprintln!("Task {} not found, skipping", id);
        }
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

async fn cmd_list(db: &std::path::Path, status: Option<String>) -> Result<()> {
    let database = db::Database::open(db).await?;
    let tasks = match status {
        Some(s) => {
            let filter = parse_status(&s)?;
            database.list_by_status(filter).await?
        }
        None => database.list_all().await?,
    };
    if tasks.is_empty() {
        println!("No tasks found.");
    } else {
        for task in tasks {
            println!(
                "[{}] {} - {} ({})",
                task.id,
                task.title,
                task.status.as_str(),
                task.repo_path
            );
        }
    }
    Ok(())
}

fn cmd_verify_feed(command: String) -> Result<()> {
    let output = std::process::Command::new("sh")
        .args(["-c", &command])
        .output()
        .context("failed to spawn command")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!(
            "verify-feed: command exited with {}\n{}",
            output.status, stderr
        );
        std::process::exit(1);
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    match serde_json::from_str::<Vec<models::FeedItem>>(stdout.trim()) {
        Ok(items) => {
            if items.is_empty() {
                eprintln!(
                    "verify-feed: command produced 0 items \
                     (empty feed — likely a misconfigured feed command)"
                );
                std::process::exit(1);
            }
            println!("{:<52} {:<55} {:<10} STATUS", "EXTERNAL_ID", "TITLE", "TAG");
            for item in &items {
                let id = truncate(&item.external_id, 50);
                let title = truncate(&item.title, 53);
                println!(
                    "{:<52} {:<55} {:<10} {}",
                    id,
                    title,
                    item.tag.as_str(),
                    item.status.as_str()
                );
            }
            println!();
            let s = if items.len() == 1 { "" } else { "s" };
            println!("✓ {} valid item{s}", items.len());
        }
        Err(e) => {
            let preview: String = stdout.trim().chars().take(500).collect();
            eprintln!("verify-feed: failed to parse output as FeedItem array: {e}");
            eprintln!("Output (first 500 chars):\n{preview}");
            std::process::exit(1);
        }
    }
    Ok(())
}

fn cmd_caller_headers() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let (stdout, code) = dispatch_tui::cli::caller_headers::resolve_headers_for_path(&cwd);
    if code == 0 {
        println!("{stdout}");
    } else {
        eprintln!("{stdout}");
    }
    std::process::exit(code);
}

async fn cmd_repo(db: &std::path::Path, action: RepoAction) -> Result<()> {
    let database = db::Database::open(db).await?;
    match action {
        RepoAction::SetVerify { path, command } => {
            let path = expand_tilde(&path);
            database.set_verify_command(&path, Some(&command)).await?;
            println!("verify_command set for {path}");
        }
        RepoAction::ClearVerify { path } => {
            let path = expand_tilde(&path);
            database.set_verify_command(&path, None).await?;
            println!("verify_command cleared for {path}");
        }
        RepoAction::List => {
            let paths = database.list_repo_paths().await?;
            if paths.is_empty() {
                println!("No repo paths configured.");
            } else {
                for p in paths {
                    match database.get_verify_command(&p).await? {
                        Some(cmd) => println!("{p}\tverify: {cmd}"),
                        None => println!("{p}"),
                    }
                }
            }
        }
    }
    Ok(())
}

async fn cmd_prune_repo_paths(db: &std::path::Path) -> Result<()> {
    let database = db::Database::open(db).await?;
    let paths = database.list_repo_paths().await?;
    let total = paths.len();
    let mut removed = 0;
    for p in &paths {
        let expanded = expand_tilde(p);
        if !std::path::Path::new(&expanded).exists() {
            database.delete_repo_path(p).await?;
            println!("removed: {p}");
            removed += 1;
        }
    }
    println!("{removed} path(s) removed, {} kept.", total - removed);
    Ok(())
}

async fn cmd_plan(db: &std::path::Path, id: i64, path: PathBuf) -> Result<()> {
    if !path.exists() {
        anyhow::bail!("Plan file not found: {}", path.display());
    }
    let plan_path = std::fs::canonicalize(&path)
        .map_err(|e| anyhow::anyhow!("Failed to resolve plan path {}: {}", path.display(), e))?;
    let plan_str = plan_path.to_string_lossy();
    let database = db::Database::open(db).await?;
    let svc = service::TaskService::new_with_real_runner(std::sync::Arc::new(database));
    match svc.attach_plan(models::TaskId(id), &plan_str).await {
        Ok(()) => println!("Plan attached to task #{}: {}", id, plan_str),
        Err(service::ServiceError::NotFound(_)) => {
            anyhow::bail!("Task {} not found", id);
        }
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

async fn cmd_doctor(
    db: &std::path::Path,
    check: Option<DoctorCheck>,
    json: bool,
    dry_run: bool,
) -> Result<()> {
    use dispatch_tui::cli::doctor::{
        check_hooks, check_sessions, check_worktrees, format_human, format_json, has_problems,
        repair_hooks_set_path, repair_sessions_orphan_db_row, repair_sessions_stale_window,
        repair_worktrees_orphan_db_row, repair_worktrees_remove, CheckKind, FindingStatus,
    };
    use dispatch_tui::process::RealProcessRunner;

    let database = db::Database::open(db).await?;
    let tasks = database.list_all().await?;
    let repo_paths = database.list_repo_paths().await?;
    let runner = RealProcessRunner;

    let mut all_repos: Vec<String> = repo_paths;
    for t in &tasks {
        if !all_repos.contains(&t.repo_path) {
            all_repos.push(t.repo_path.clone());
        }
    }

    let mut findings = Vec::new();

    // Resolve json: subcommand flag takes precedence over parent flag.
    let json = match &check {
        None => json,
        Some(DoctorCheck::Worktrees { json, .. }) => *json,
        Some(DoctorCheck::Sessions { json, .. }) => *json,
        Some(DoctorCheck::Hooks { json, .. }) => *json,
    };

    let (run_worktrees, run_sessions, run_hooks, repair, force) = match &check {
        None => (true, true, true, false, false),
        Some(DoctorCheck::Worktrees { repair, force, .. }) => (true, false, false, *repair, *force),
        Some(DoctorCheck::Sessions { repair, force, .. }) => (false, true, false, *repair, *force),
        Some(DoctorCheck::Hooks { repair, force, .. }) => (false, false, true, *repair, *force),
    };
    // --dry-run always overrides --repair/--force
    let (repair, force) = if dry_run {
        (false, false)
    } else {
        (repair, force)
    };

    if run_worktrees {
        findings.extend(check_worktrees(&tasks, &all_repos));
    }
    if run_sessions {
        findings.extend(check_sessions(&tasks, &runner));
    }
    if run_hooks {
        findings.extend(check_hooks(&all_repos, &runner));
    }

    if repair {
        if !force {
            let repairable: Vec<_> = findings.iter().filter(|f| f.repair_available).collect();
            if !repairable.is_empty() {
                if json {
                    println!("{}", format_json(&findings));
                } else {
                    println!("{}", format_human(&findings));
                }
                eprintln!("The following repairs would be applied (re-run with --force to apply):");
                for f in &repairable {
                    eprintln!(
                        "  would repair: {} {}  —  {}",
                        f.check.as_str(),
                        f.target,
                        f.message
                    );
                }
                std::process::exit(1);
            }
        } else {
            let mut any_repair_failed = false;
            for f in &findings {
                if !f.repair_available {
                    continue;
                }
                let result: anyhow::Result<()> = match f.check {
                    CheckKind::Hooks => repair_hooks_set_path(&f.target, &runner),
                    CheckKind::Sessions => match f.status {
                        FindingStatus::Error => {
                            let Some(task) = tasks
                                .iter()
                                .find(|t| t.tmux_window.as_deref() == Some(f.target.as_str()))
                            else {
                                eprintln!(
                                    "repair skipped for {}: no matching task found",
                                    f.target
                                );
                                continue;
                            };
                            repair_sessions_orphan_db_row(task.id, &database).await
                        }
                        FindingStatus::Warn => repair_sessions_stale_window(&f.target, &runner),
                        FindingStatus::Ok => Ok(()),
                    },
                    CheckKind::Worktrees => match f.status {
                        FindingStatus::Error => {
                            let Some(task) = tasks
                                .iter()
                                .find(|t| t.worktree.as_deref() == Some(f.target.as_str()))
                            else {
                                eprintln!(
                                    "repair skipped for {}: no matching task found",
                                    f.target
                                );
                                continue;
                            };
                            repair_worktrees_orphan_db_row(task.id, &database).await
                        }
                        FindingStatus::Warn => {
                            let Some(repo) = all_repos
                                .iter()
                                .find(|r| f.target.starts_with(&format!("{r}/.worktrees/")))
                                .cloned()
                            else {
                                eprintln!(
                                    "repair skipped for {}: no matching repo found",
                                    f.target
                                );
                                continue;
                            };
                            repair_worktrees_remove(&repo, &f.target, &runner)
                        }
                        FindingStatus::Ok => Ok(()),
                    },
                };
                match result {
                    Err(e) => {
                        eprintln!("repair failed for {}: {e}", f.target);
                        any_repair_failed = true;
                    }
                    Ok(()) if !json => {
                        println!("repaired: {} {}", f.check.as_str(), f.target)
                    }
                    Ok(()) => {}
                }
            }
            if any_repair_failed {
                std::process::exit(1);
            }
            if json {
                println!("{}", format_json(&findings));
            }
            return Ok(());
        }
    }

    if json {
        println!("{}", format_json(&findings));
    } else if findings.is_empty() {
        println!("all checks passed");
    } else {
        println!("{}", format_human(&findings));
    }

    if has_problems(&findings) {
        std::process::exit(1);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// main — thin dispatcher
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Tui { port } => cmd_tui(&cli.db, port).await?,
        Commands::Update {
            id,
            status,
            only_if,
            sub_status,
            needs_input,
        } => cmd_update(&cli.db, id, status, only_if, sub_status, needs_input).await?,
        Commands::Hook {
            id,
            kind,
            notification_kind,
        } => cmd_hook(&cli.db, id, kind, notification_kind).await?,
        Commands::PrGate { id } => cmd_pr_gate(&cli.db, id).await?,
        Commands::List { status } => cmd_list(&cli.db, status).await?,
        Commands::Setup { port, yes } => {
            dispatch_tui::setup::run_setup(port, yes, &cli.db).await?;
        }
        Commands::Uninstall { yes, purge } => dispatch_tui::setup::run_uninstall(yes, purge)?,
        Commands::VerifyFeed { command } => cmd_verify_feed(command)?,
        Commands::CallerHeaders => cmd_caller_headers()?,
        Commands::Repo { action } => cmd_repo(&cli.db, action).await?,
        Commands::PruneRepoPaths => cmd_prune_repo_paths(&cli.db).await?,
        Commands::Doctor {
            check,
            json,
            dry_run,
        } => cmd_doctor(&cli.db, check, json, dry_run).await?,
        Commands::Plan { id, path } => cmd_plan(&cli.db, id, path).await?,
    }

    Ok(())
}
