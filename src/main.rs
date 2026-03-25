mod db;
mod dispatch;
mod mcp;
mod models;
mod tmux;
mod tui;

use anyhow::Result;
use clap::{Parser, Subcommand};
use crossterm::{
    event::{self, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::interval;

use tui::{App, Command, Message};

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
            run_tui(&cli.db, port).await?;
        }
        Commands::Update { id, status } => {
            let new_status = models::TaskStatus::from_str(&status)
                .ok_or_else(|| anyhow::anyhow!("Unknown status: {}", status))?;
            let db = db::Database::open(&cli.db)?;
            db.update_status(id, new_status)?;
            println!("Task {} updated to {}", id, status);
        }
        Commands::List { status } => {
            let db = db::Database::open(&cli.db)?;
            let tasks = match status {
                Some(s) => {
                    let filter = models::TaskStatus::from_str(&s)
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

// ---------------------------------------------------------------------------
// TUI main loop
// ---------------------------------------------------------------------------

async fn run_tui(db_path: &Path, port: u16) -> Result<()> {
    // 1. Open database and load initial tasks
    let database = Arc::new(db::Database::open(db_path)?);
    let tasks = database.list_all()?;

    // 2. Spawn MCP server
    let mcp_db = database.clone();
    tokio::spawn(async move {
        if let Err(e) = mcp::serve(mcp_db, port).await {
            eprintln!("MCP server error: {e}");
        }
    });

    // 3. Create App and load saved repo paths
    let mut app = App::new(tasks);
    app.repo_paths = database.list_repo_paths().unwrap_or_default();

    // 4. Set up terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // 5. Create two channels:
    //    - key_rx: raw crossterm KeyEvents from the blocking poll thread
    //    - msg_rx: higher-level Messages (e.g. from dispatch results in Phase 3)
    let (key_tx, mut key_rx) = mpsc::unbounded_channel::<crossterm::event::KeyEvent>();
    let (msg_tx, mut msg_rx) = mpsc::unbounded_channel::<Message>();

    // crossterm::event::poll/read are blocking; run them in a dedicated thread
    // so they don't block the async runtime. The thread can be paused (e.g. when
    // opening an external editor) via the input_paused flag.
    let input_paused = Arc::new(AtomicBool::new(false));
    let paused_clone = input_paused.clone();
    tokio::task::spawn_blocking(move || {
        loop {
            if paused_clone.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            if event::poll(Duration::from_millis(50)).unwrap_or(false) {
                if let Ok(Event::Key(key)) = event::read() {
                    if key_tx.send(key).is_err() {
                        break;
                    }
                }
            }
        }
    });

    // 6. Tick interval (2 seconds)
    let mut tick_interval = interval(Duration::from_secs(2));

    // 7. Main loop
    let result = run_loop(
        &mut app,
        &mut terminal,
        &mut key_rx,
        &mut msg_rx,
        &msg_tx,
        &mut tick_interval,
        &database,
        port,
        &input_paused,
    )
    .await;

    // 8. Cleanup terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

async fn run_loop(
    app: &mut App,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    key_rx: &mut mpsc::UnboundedReceiver<crossterm::event::KeyEvent>,
    msg_rx: &mut mpsc::UnboundedReceiver<Message>,
    msg_tx: &mpsc::UnboundedSender<Message>,
    tick_interval: &mut tokio::time::Interval,
    database: &Arc<db::Database>,
    port: u16,
    input_paused: &Arc<AtomicBool>,
) -> Result<()> {
    loop {
        // Draw the current frame
        terminal.draw(|frame| tui::ui::render(frame, app))?;

        if app.should_quit {
            break;
        }

        let commands = tokio::select! {
            // Key events from the blocking poll thread
            Some(key) = key_rx.recv() => {
                app.handle_key(key)
            }

            // Async messages (e.g., from dispatch results in Phase 3)
            Some(msg) = msg_rx.recv() => {
                app.update(msg)
            }

            // Periodic tick for tmux capture
            _ = tick_interval.tick() => {
                app.update(Message::Tick)
            }
        };

        execute_commands(app, commands, database, msg_tx, port, terminal, input_paused).await?;
    }

    Ok(())
}

async fn execute_commands(
    app: &mut App,
    commands: Vec<Command>,
    database: &Arc<db::Database>,
    msg_tx: &mpsc::UnboundedSender<Message>,
    port: u16,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    input_paused: &Arc<AtomicBool>,
) -> Result<()> {
    for command in commands {
        match command {
            Command::PersistTask(mut task) => {
                if task.id == 0 {
                    // New task — insert into db and update the in-app id
                    let new_id = database.create_task(&task.title, &task.description, &task.repo_path)?;
                    task.id = new_id;
                    // Update the placeholder task in app.tasks (id 0) with the real id.
                    // There may be multiple id=0 tasks if rapid creation; update the first one.
                    if let Some(t) = app.tasks.iter_mut().find(|t| t.id == 0) {
                        t.id = new_id;
                    }
                } else {
                    // Existing task — update its status and dispatch fields
                    let _ = database.update_status(task.id, task.status);
                    let _ = database.update_dispatch(
                        task.id,
                        task.worktree.as_deref(),
                        task.tmux_window.as_deref(),
                    );
                }
            }

            Command::DeleteTask(id) => {
                // Ignore errors for tasks that don't exist (e.g. never persisted)
                let _ = database.delete_task(id);
            }

            Command::Dispatch { task } => {
                let tx = msg_tx.clone();
                let title = task.title.clone();
                let description = task.description.clone();
                let repo_path = task.repo_path.clone();
                let id = task.id;

                tokio::task::spawn_blocking(move || {
                    match dispatch::dispatch_agent(id, &title, &description, &repo_path, port) {
                        Ok(result) => {
                            let _ = tx.send(Message::Dispatched {
                                id,
                                worktree: result.worktree_path,
                                tmux_window: result.tmux_window,
                            });
                        }
                        Err(e) => {
                            let _ = tx.send(Message::Error(format!("Dispatch failed: {e}")));
                        }
                    }
                });
            }

            Command::CaptureTmux { id, window } => {
                let tx = msg_tx.clone();

                tokio::task::spawn_blocking(move || {
                    match tmux::capture_pane(&window, 5) {
                        Ok(output) => {
                            let _ = tx.send(Message::TmuxOutput { id, output });
                        }
                        Err(e) => {
                            // Non-fatal: log as a status message rather than crashing.
                            let _ = tx.send(Message::Error(format!(
                                "tmux capture failed for window {window}: {e}"
                            )));
                        }
                    }

                    // Check if the window is still alive. If it's gone, signal
                    // that the window exited. The main loop will check the task's
                    // current status before advancing.
                    match tmux::has_window(&window) {
                        Ok(false) => {
                            let _ = tx.send(Message::WindowGone(id));
                        }
                        _ => {} // still running or error — leave it
                    }
                });
            }

            Command::EditTaskInEditor(task) => {
                let task_id = task.id;
                let tmp = std::env::temp_dir().join(format!("task-{task_id}.txt"));
                let content = format!(
                    "title: {}\ndescription: {}\nrepo_path: {}\nstatus: {}\n",
                    task.title, task.description, task.repo_path, task.status.as_str()
                );
                std::fs::write(&tmp, &content)?;

                // Pause the input polling thread so vim can read keypresses
                input_paused.store(true, Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(150));

                // Suspend TUI
                disable_raw_mode()?;
                execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
                terminal.show_cursor()?;

                // Open editor
                let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vim".to_string());
                let status = std::process::Command::new(&editor)
                    .arg(&tmp)
                    .status();

                // Resume TUI
                enable_raw_mode()?;
                execute!(terminal.backend_mut(), EnterAlternateScreen)?;
                terminal.hide_cursor()?;
                terminal.clear()?;

                // Resume input polling thread
                input_paused.store(false, Ordering::Relaxed);

                if let Ok(exit) = status {
                    if exit.success() {
                        // Parse the edited file
                        if let Ok(edited) = std::fs::read_to_string(&tmp) {
                            let mut title = task.title.clone();
                            let mut description = task.description.clone();
                            let mut repo_path = task.repo_path.clone();
                            let mut new_status = task.status;

                            for line in edited.lines() {
                                if let Some((key, value)) = line.split_once(':') {
                                    let value = value.trim().to_string();
                                    match key.trim() {
                                        "title" => title = value,
                                        "description" => description = value,
                                        "repo_path" => repo_path = value,
                                        "status" => {
                                            if let Some(s) = models::TaskStatus::from_str(&value) {
                                                new_status = s;
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }

                            // Update DB and in-memory state
                            let _ = database.update_task(task_id, &title, &description, &repo_path, new_status);
                            if let Some(t) = app.tasks.iter_mut().find(|t| t.id == task_id) {
                                t.title = title;
                                t.description = description;
                                t.repo_path = repo_path;
                                t.status = new_status;
                                t.updated_at = chrono::Utc::now();
                            }
                            app.clamp_selection();
                        }
                    }
                }

                let _ = std::fs::remove_file(&tmp);
            }

            Command::SaveRepoPath(path) => {
                let _ = database.save_repo_path(&path);
                app.repo_paths = database.list_repo_paths().unwrap_or_default();
            }

            Command::RefreshFromDb => {
                // Re-read all tasks from SQLite to pick up MCP/CLI updates
                match database.list_all() {
                    Ok(tasks) => {
                        let cmds = app.update(Message::RefreshTasks(tasks));
                        // Don't recurse into execute_commands for RefreshTasks
                        // since it only updates in-memory state (no side effects)
                        let _ = cmds;
                    }
                    Err(e) => {
                        app.status_message = Some(format!("DB refresh failed: {e}"));
                    }
                }
            }

            Command::None => {}
        }
    }

    Ok(())
}
