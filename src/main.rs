use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing::Level;
use tracing_subscriber::EnvFilter;

use dispatch_tui::db::{SettingsStore, TaskRead};
use dispatch_tui::models::expand_tilde;
use dispatch_tui::tui::ui::truncate;
use dispatch_tui::{db, dispatch, models, runtime, service};

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
    /// Record a Claude Code subagent lifecycle event (SubagentStart /
    /// SubagentStop / SessionStart) for a task. Maintains the live subagent
    /// count that gates staleness and the deferred Stop-to-Review flip; see
    /// `docs/specs/agent-health.allium`.
    HookSubagent {
        /// Task ID
        id: i64,
        /// Action: start | stop | clear
        action: String,
        /// Subagent identifier from the payload's `agent_id` field. Required
        /// for start and stop; ignored for clear.
        #[arg(long = "agent-id")]
        agent_id: Option<String>,
        /// Session identifier from the payload's `session_id` field. Used to
        /// fence entries left behind by a dead session.
        #[arg(long = "session-id")]
        session_id: Option<String>,
    },
    /// Append a file-touch event (Read/Write/Edit/NotebookEdit) to a task's
    /// file-events JSONL log (see `docs/specs/agent-tree.allium`). Deliberately
    /// independent of `Hook`/`HookEventKind` — this command never touches
    /// `TaskService` or the database, and does not affect task-activity
    /// classification.
    HookFileEvent {
        /// Task ID
        id: i64,
        /// Claude Code tool name: Read, Write, Edit, or NotebookEdit
        #[arg(long)]
        tool: String,
        /// File path from the tool's payload (`tool_input.file_path`, or
        /// `tool_input.notebook_path` for NotebookEdit)
        #[arg(long)]
        path: String,
    },
    /// Render a standalone companion file-tree pane for one task's agent,
    /// fed by its file-events JSONL log (see docs/specs/agent-tree.allium).
    /// A small ratatui loop — deliberately not part of the board TUI's
    /// App/message loop; runs as its own process in a tmux pane.
    AgentTree {
        /// Task ID whose file-events log to render
        task_id: i64,
    },
    /// statusLine decorator for Claude Code: record the subscription
    /// rate-limit windows from the hook payload on stdin, then run the
    /// user's previous statusLine command and print its output verbatim.
    /// Always exits 0 — never breaks the user's status line. Opens no
    /// database (it runs several times a second per session).
    Statusline {
        /// Where to publish the snapshot JSON
        #[arg(long)]
        snapshot: String,
        /// The previous statusLine command to run and echo
        #[arg(long)]
        chain: Option<String>,
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
    /// Toggle the companion agent-tree pane in a tmux window. Invoked by the
    /// global toggle keybinding's bound run-shell command; not meant to be
    /// run by hand.
    ToggleAgentTreePane {
        /// tmux window name (e.g. "task-42"), supplied by tmux's own
        /// #{window_name} expansion at the moment the toggle key was pressed.
        window: String,
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
    /// Show each saved repo path's drift against origin on its default branch.
    /// Read-only: it measures and prints, it never merges or pushes.
    /// See docs/specs/repo-sync.allium (surface RepoStatusCli).
    Status {
        /// Skip the fetch and report whatever the local refs say — for use
        /// offline or in a tight loop.
        #[arg(long)]
        no_fetch: bool,
    },
    /// Bring one saved repo path — or every one of them — into step with origin
    /// on its default branch. See docs/specs/repo-sync.allium (surface
    /// RepoSyncCli).
    Sync {
        /// The repo path to sync. Omitted, every saved repo path is attempted.
        path: Option<String>,
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
        // Same silent-skip contract as the hook commands (see
        // `report_hook_outcome`), matched on the typed variant rather than the
        // message text — `cli_update_task` returns a `ServiceError`, so there is
        // no reason for this to break when a message is reworded.
        Err(service::ServiceError::NotFound(_)) => {
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

/// Shared prologue for the `hook*` commands: resolve the data dir, point the app
/// log at it, and hand it back. Every hook runs as its own short-lived process,
/// so each one has to install the subscriber itself or its warnings go nowhere.
fn hook_data_dir(db: &std::path::Path) -> Result<&std::path::Path> {
    let data_dir = db.parent().unwrap_or(std::path::Path::new("."));
    init_app_log_subscriber(data_dir)?;
    Ok(data_dir)
}

/// [`hook_data_dir`] plus the service the hook writes through.
async fn open_hook_service(db: &std::path::Path) -> Result<service::TaskService> {
    hook_data_dir(db)?;
    let database = db::Database::open(db).await?;
    Ok(service::TaskService::new_with_real_runner(
        std::sync::Arc::new(database),
    ))
}

/// Every hook command's outcome contract: a missing task is a silent skip, not a
/// failure. A hook fires from a session whose task may since have been archived
/// or deleted, and a non-zero exit there would surface in the agent's own
/// terminal for something it cannot act on.
fn report_hook_outcome(id: i64, outcome: Result<(), service::ServiceError>) -> Result<()> {
    match outcome {
        Ok(()) => Ok(()),
        Err(service::ServiceError::NotFound(_)) => {
            eprintln!("Task {} not found, skipping", id);
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
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
    let svc = open_hook_service(db).await?;
    let outcome = svc.record_hook_event(models::TaskId(id), parsed).await;
    report_hook_outcome(id, outcome)
}

async fn cmd_hook_subagent(
    db: &std::path::Path,
    id: i64,
    action: String,
    agent_id: Option<String>,
    session_id: Option<String>,
) -> Result<()> {
    // A start/stop with no agent_id/session_id carries no information — the
    // shell hook already guards this, but a bare CLI call must not panic or
    // half-write.
    //
    // `clear` (SessionStart) is deliberately the *non-draining* variant. A new,
    // resumed or cleared session means the previous turn is over, so a Stop
    // deferred by that turn is stale and must be voided rather than applied:
    // resume in particular keeps the task Running on purpose (see
    // `handle_retry_resume`), and draining here would strand it in Review with
    // a live agent and no UserPromptSubmit coming. The draining variant
    // (`SubagentEvent::Clear`) is reached only from detach, whose rule owns no
    // status of its own. See `ClearSubagentsOnSessionStart` in
    // `docs/specs/agent-health.allium`.
    let event = match action.as_str() {
        "clear" => None,
        "start" | "stop" => {
            let (Some(agent_id), Some(session_id)) = (agent_id, session_id) else {
                return Ok(());
            };
            if action == "start" {
                Some(models::SubagentEvent::Start {
                    agent_id,
                    session_id,
                })
            } else {
                Some(models::SubagentEvent::Stop {
                    agent_id,
                    session_id,
                })
            }
        }
        other => anyhow::bail!("Invalid subagent action: {other}. Valid: start, stop, clear"),
    };
    let svc = open_hook_service(db).await?;
    let outcome = match event {
        Some(event) => svc.record_subagent_event(models::TaskId(id), event).await,
        None => svc.clear_subagents_no_drain(models::TaskId(id)).await,
    };
    report_hook_outcome(id, outcome)
}

async fn cmd_hook_file_event(
    db: &std::path::Path,
    id: i64,
    tool: String,
    path: String,
) -> Result<()> {
    let data_dir = hook_data_dir(db)?;
    dispatch_tui::file_events::append_file_event(data_dir, id, &tool, &path).await;
    Ok(())
}

async fn cmd_agent_tree(db: &std::path::Path, task_id: i64) -> Result<()> {
    // The renderer owns the alternate screen, so its warnings cannot go to
    // stderr — they go to `app.log` next to the database, like the board's.
    // Without this every `tracing::warn!` in the renderer went nowhere, which
    // included the only report of a file it could not open.
    // Best-effort: a renderer that cannot open the log still renders.
    let data_dir = db.parent().unwrap_or(std::path::Path::new("."));
    let _ = init_app_log_subscriber(data_dir);
    dispatch_tui::cli::agent_tree::run(db, task_id).await
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

/// The statusLine decorator: read the hook payload from stdin, record it, chain,
/// exit 0. Never returns — the exit code is unconditional (see
/// `docs/specs/dispatch.allium`: StatusLineDecorator, `@guarantee
/// AlwaysSucceeds`). Fully synchronous, and routed by `main` before any runtime
/// exists (`@guarantee StartsNoAsyncRuntime`).
fn cmd_statusline(snapshot: &str, chain: Option<&str>) -> ! {
    let mut stdin = String::new();
    let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut stdin);
    let now = chrono::Utc::now().timestamp();
    let code =
        dispatch_tui::cli::statusline::run(&stdin, std::path::Path::new(snapshot), chain, now);
    std::process::exit(code);
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
        RepoAction::Status { no_fetch } => {
            cmd_repo_status(&database, no_fetch).await?;
        }
        RepoAction::Sync { path } => {
            cmd_repo_sync(&database, path).await?;
        }
    }
    Ok(())
}

/// `dispatch repo status [--no-fetch]` — one row per saved repo path.
///
/// Fetches before measuring unless suppressed, so the counts are current. A
/// repository that could not be measured shows no ahead/behind figures at all
/// (`UnmeasuredIsNeverPresentedAsClean`) and, when the fetch was the cause, its
/// fetch error instead.
async fn cmd_repo_status(database: &db::Database, no_fetch: bool) -> Result<()> {
    let paths = database.list_repo_paths().await?;
    if paths.is_empty() {
        println!("No repo paths configured.");
        return Ok(());
    }
    // Every repo is measured concurrently: with a fetch this is a network
    // round-trip each, so N repos sequentially would cost N latencies for work
    // that has no ordering between repositories. Mirrors the board's startup
    // fan-out (`exec_refresh_all_repo_sync`). Handles are spawned up front and
    // awaited in `paths` order, so the table stays deterministic regardless of
    // which repository answers first.
    let handles: Vec<_> = paths
        .iter()
        .map(|path| {
            let expanded = expand_tilde(path);
            tokio::task::spawn_blocking(move || {
                let runner = dispatch_tui::process::RealProcessRunner;
                dispatch_tui::repo_sync::measure_repo(&expanded, !no_fetch, &runner)
            })
        })
        .collect();

    let mut cache = dispatch_tui::repo_sync::RepoSyncCache::default();
    for (path, handle) in paths.iter().zip(handles) {
        let expanded = expand_tilde(path);
        cache.apply(handle.await?);
        // `measure_repo` keys the state by the path it was handed.
        let Some(state) = cache.get(&expanded) else {
            continue;
        };
        match state.counts {
            Some(counts) => println!(
                "{}\t{}\t\u{2191}{} \u{2193}{}",
                state.repo_path, state.base_branch, counts.ahead, counts.behind
            ),
            None => match &state.last_fetch_error {
                Some(err) => println!("{}\t{}\tunknown\t{err}", state.repo_path, state.base_branch),
                None => println!("{}\t{}\tunknown", state.repo_path, state.base_branch),
            },
        }
    }
    Ok(())
}

/// `dispatch repo sync [<path>]` — sync one saved repo path or every one.
///
/// Every target is attempted; one failure does not abandon the rest. The exit
/// code is non-zero when any target failed, so the command is usable from a
/// script.
async fn cmd_repo_sync(database: &db::Database, path: Option<String>) -> Result<()> {
    let saved = database.list_repo_paths().await?;
    let targets: Vec<String> = match &path {
        Some(p) => {
            let expanded = expand_tilde(p);
            saved
                .into_iter()
                .filter(|s| expand_tilde(s) == expanded)
                .collect()
        }
        None => saved,
    };
    if targets.is_empty() {
        match path {
            Some(p) => anyhow::bail!("{p} is not a saved repo path"),
            None => anyhow::bail!("No repo paths configured."),
        }
    }

    let runner = dispatch_tui::process::RealProcessRunner;
    let mut failed = 0;
    for target in &targets {
        let expanded = expand_tilde(target);
        let base = tokio::task::block_in_place(|| {
            dispatch_tui::git::detect_default_branch(&expanded, &runner)
        });
        let result = tokio::task::block_in_place(|| {
            dispatch_tui::repo_sync::sync_repo(&expanded, &base, &runner)
        });
        match result {
            Ok(dispatch_tui::repo_sync::SyncOutcome::AlreadyInSync) => {
                println!("{expanded}\t{base}\tnothing to do");
            }
            Ok(dispatch_tui::repo_sync::SyncOutcome::Synced { pulled, pushed }) => {
                println!("{expanded}\t{base}\tpulled {pulled}, pushed {pushed}");
            }
            Err(e) => {
                failed += 1;
                eprintln!("{expanded}\t{base}\tfailed: {e}");
            }
        }
    }
    if failed > 0 {
        anyhow::bail!("{failed} of {} repo(s) failed to sync", targets.len());
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

/// Toggle the companion agent-tree pane in `window`. Best-effort: this runs
/// detached via the global keybinding's `run-shell -b`, so a failure has
/// nowhere useful to surface — it's logged to app.log and swallowed rather
/// than returned, matching `spawn_agent_tree_pane`'s own best-effort stance.
fn cmd_toggle_agent_tree_pane(db: &std::path::Path, window: String) -> Result<()> {
    let data_dir = db.parent().unwrap_or(std::path::Path::new("."));
    let _ = init_app_log_subscriber(data_dir);
    let runner = dispatch_tui::process::RealProcessRunner;
    if let Err(e) = dispatch::toggle_agent_tree_pane(&window, &runner) {
        tracing::warn!(%window, error = %e, "failed to toggle agent-tree companion pane");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// main — thin dispatcher
// ---------------------------------------------------------------------------

/// Argv in, one of two dispatchers out.
///
/// The subcommands handled here run entirely synchronously, so they must not pay
/// for a tokio runtime: a multi-thread runtime costs a worker thread per core
/// plus the reactor, built and torn down per process. `statusline` runs on Claude
/// Code's ~300 ms statusLine debounce in every session concurrently, and
/// `caller-headers` on every MCP session start/reconnect, so that setup is pure
/// waste at exactly the frequency that matters. See `docs/specs/dispatch.allium`:
/// StatusLineDecorator (`@guarantee StartsNoAsyncRuntime`).
///
/// This match is the only classifier: everything not named here falls through to
/// the runtime and [`run_async`]. A subcommand added later therefore lands on the
/// async path by default — correct, merely unoptimised.
fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Statusline { snapshot, chain } => cmd_statusline(&snapshot, chain.as_deref()),
        Commands::CallerHeaders => cmd_caller_headers(),
        Commands::VerifyFeed { command } => cmd_verify_feed(command),
        Commands::Uninstall { yes, purge } => dispatch_tui::setup::run_uninstall(yes, purge),
        Commands::ToggleAgentTreePane { window } => cmd_toggle_agent_tree_pane(&cli.db, window),
        command => tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(run_async(&cli.db, command)),
    }
}

async fn run_async(db: &std::path::Path, command: Commands) -> Result<()> {
    match command {
        Commands::Tui { port } => cmd_tui(db, port).await?,
        Commands::Update {
            id,
            status,
            only_if,
            sub_status,
            needs_input,
        } => cmd_update(db, id, status, only_if, sub_status, needs_input).await?,
        Commands::Hook {
            id,
            kind,
            notification_kind,
        } => cmd_hook(db, id, kind, notification_kind).await?,
        Commands::HookSubagent {
            id,
            action,
            agent_id,
            session_id,
        } => cmd_hook_subagent(db, id, action, agent_id, session_id).await?,
        Commands::HookFileEvent { id, tool, path } => {
            cmd_hook_file_event(db, id, tool, path).await?
        }
        Commands::AgentTree { task_id } => cmd_agent_tree(db, task_id).await?,
        Commands::PrGate { id } => cmd_pr_gate(db, id).await?,
        Commands::List { status } => cmd_list(db, status).await?,
        Commands::Setup { port, yes } => {
            dispatch_tui::setup::run_setup(port, yes, db).await?;
        }
        Commands::Repo { action } => cmd_repo(db, action).await?,
        Commands::PruneRepoPaths => cmd_prune_repo_paths(db).await?,
        Commands::Plan { id, path } => cmd_plan(db, id, path).await?,
        // Unreachable by construction: `main` matches these same patterns before
        // any runtime exists, so they never reach the async path.
        Commands::Statusline { .. }
        | Commands::CallerHeaders
        | Commands::VerifyFeed { .. }
        | Commands::Uninstall { .. }
        | Commands::ToggleAgentTreePane { .. } => {
            unreachable!("synchronous subcommands are routed by main, not run_async")
        }
    }

    Ok(())
}
