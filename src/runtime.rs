use anyhow::Result;
use crossterm::{
    event::{self, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::interval;

use crate::editor::{format_editor_content, parse_editor_content};
use crate::tui::{self, App, Command, Message};
use crate::{db, dispatch, models, mcp, tmux};

// ---------------------------------------------------------------------------
// run_tui — entry point for the TUI mode
// ---------------------------------------------------------------------------

pub async fn run_tui(db_path: &Path, port: u16) -> Result<()> {
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
    let paths = database.list_repo_paths().unwrap_or_default();
    app.update(Message::RepoPathsUpdated(paths));

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
    let runtime = TuiRuntime {
        database,
        msg_tx,
        port,
        input_paused,
    };
    let result = run_loop(
        &mut app,
        &mut terminal,
        &mut key_rx,
        &mut msg_rx,
        &mut tick_interval,
        &runtime,
    )
    .await;

    // 8. Cleanup terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

// ---------------------------------------------------------------------------
// TuiRuntime — shared context for command execution
// ---------------------------------------------------------------------------

struct TuiRuntime {
    database: Arc<db::Database>,
    msg_tx: mpsc::UnboundedSender<Message>,
    port: u16,
    input_paused: Arc<AtomicBool>,
}

// ---------------------------------------------------------------------------
// run_loop — select over key events, async messages, and tick timer
// ---------------------------------------------------------------------------

async fn run_loop(
    app: &mut App,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    key_rx: &mut mpsc::UnboundedReceiver<crossterm::event::KeyEvent>,
    msg_rx: &mut mpsc::UnboundedReceiver<Message>,
    tick_interval: &mut tokio::time::Interval,
    rt: &TuiRuntime,
) -> Result<()> {
    loop {
        // Draw the current frame
        terminal.draw(|frame| tui::ui::render(frame, app))?;

        if app.should_quit() {
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

        execute_commands(app, commands, rt, terminal).await?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// execute_commands — run side effects for each Command
// ---------------------------------------------------------------------------

async fn execute_commands(
    app: &mut App,
    commands: Vec<Command>,
    rt: &TuiRuntime,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> Result<()> {
    for command in commands {
        match command {
            Command::PersistTask(mut task) => {
                if task.id == 0 {
                    // New task — insert into db and update the in-app id
                    let new_id = rt.database.create_task(&task.title, &task.description, &task.repo_path, task.plan.as_deref())?;
                    task.id = new_id;
                    // Update the placeholder task in app.tasks (id 0) with the real id.
                    // There may be multiple id=0 tasks if rapid creation; update the first one.
                    app.update(Message::TaskIdAssigned { placeholder_id: 0, real_id: new_id });
                } else {
                    // Existing task — update its status and dispatch fields
                    if let Err(e) = rt.database.update_status(task.id, task.status) {
                        app.update(Message::Error(format!("DB error updating status: {e}")));
                    }
                    if let Err(e) = rt.database.update_dispatch(
                        task.id,
                        task.worktree.as_deref(),
                        task.tmux_window.as_deref(),
                    ) {
                        app.update(Message::Error(format!("DB error updating dispatch: {e}")));
                    }
                }
            }

            Command::DeleteTask(id) => {
                if let Err(e) = rt.database.delete_task(id) {
                    // id=0 tasks were never persisted — not a real error
                    if id != 0 {
                        app.update(Message::Error(format!("DB error deleting task: {e}")));
                    }
                }
            }

            Command::Dispatch { task } => {
                let tx = rt.msg_tx.clone();
                let port = rt.port;

                tokio::task::spawn_blocking(move || {
                    let id = task.id;
                    match dispatch::dispatch_agent(&task, port) {
                        Ok(result) => {
                            let _ = tx.send(Message::Dispatched {
                                id,
                                worktree: result.worktree_path,
                                tmux_window: result.tmux_window,
                            });
                        }
                        Err(e) => {
                            let _ = tx.send(Message::Error(format!("Dispatch failed: {e:#}")));
                        }
                    }
                });
            }

            Command::CaptureTmux { id, window } => {
                let tx = rt.msg_tx.clone();

                tokio::task::spawn_blocking(move || {
                    // Check if the window is still alive first to avoid
                    // capturing from a dead window (which would error).
                    if let Ok(false) = tmux::has_window(&window) {
                        let _ = tx.send(Message::WindowGone(id));
                        return;
                    }

                    match tmux::capture_pane(&window, 5) {
                        Ok(output) => {
                            let _ = tx.send(Message::TmuxOutput { id, output });
                        }
                        Err(e) => {
                            let _ = tx.send(Message::Error(format!(
                                "tmux capture failed for window {window}: {e}"
                            )));
                        }
                    }
                });
            }

            Command::EditTaskInEditor(task) => {
                handle_edit_in_editor(app, task, rt, terminal)?;
            }

            Command::SaveRepoPath(path) => {
                let _ = rt.database.save_repo_path(&path);
                let paths = rt.database.list_repo_paths().unwrap_or_default();
                app.update(Message::RepoPathsUpdated(paths));
            }

            Command::LoadNotes(task_id) => {
                let db = rt.database.clone();
                let tx = rt.msg_tx.clone();
                tokio::task::spawn_blocking(move || {
                    match db.list_notes(task_id) {
                        Ok(notes) => {
                            let _ = tx.send(Message::NotesLoaded { task_id, notes });
                        }
                        Err(e) => {
                            let _ = tx.send(Message::Error(format!("Failed to load notes: {e}")));
                        }
                    }
                });
            }

            Command::RefreshFromDb => {
                // Re-read all tasks from SQLite to pick up MCP/CLI updates
                match rt.database.list_all() {
                    Ok(tasks) => {
                        let cmds = app.update(Message::RefreshTasks(tasks));
                        // Don't recurse into execute_commands for RefreshTasks
                        // since it only updates in-memory state (no side effects)
                        let _ = cmds;
                    }
                    Err(e) => {
                        app.update(Message::Error(format!("DB refresh failed: {e}")));
                    }
                }
            }

            Command::Cleanup { repo_path, worktree, tmux_window } => {
                let tx = rt.msg_tx.clone();
                tokio::task::spawn_blocking(move || {
                    if let Err(e) = dispatch::cleanup_task(&repo_path, &worktree, tmux_window.as_deref()) {
                        let _ = tx.send(Message::Error(format!("Cleanup failed: {e:#}")));
                    }
                });
            }

            Command::Resume { task } => {
                let tx = rt.msg_tx.clone();
                let id = task.id;
                let worktree_path = task.worktree.clone().unwrap_or_default();

                tokio::task::spawn_blocking(move || {
                    match dispatch::resume_agent(id, &worktree_path) {
                        Ok(result) => {
                            let _ = tx.send(Message::Resumed {
                                id,
                                tmux_window: result.tmux_window,
                            });
                        }
                        Err(e) => {
                            let _ = tx.send(Message::Error(format!("Resume failed: {e:#}")));
                        }
                    }
                });
            }

            Command::JumpToTmux { window } => {
                if let Err(e) = tmux::select_window(&window) {
                    app.update(Message::Error(format!("Jump failed: {e:#}")));
                }
            }

        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// handle_edit_in_editor — open $EDITOR for a task and apply changes
// ---------------------------------------------------------------------------

fn handle_edit_in_editor(
    app: &mut App,
    task: models::Task,
    rt: &TuiRuntime,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> Result<()> {
    let task_id = task.id;
    let tmp = std::env::temp_dir().join(format!("task-{task_id}.txt"));
    let content = format_editor_content(&task.title, &task.description, &task.repo_path, task.status.as_str());
    std::fs::write(&tmp, &content)?;

    // Pause the input polling thread so vim can read keypresses
    rt.input_paused.store(true, Ordering::Relaxed);
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
    rt.input_paused.store(false, Ordering::Relaxed);

    if let Ok(exit) = status {
        if exit.success() {
            // Parse the edited file
            if let Ok(edited) = std::fs::read_to_string(&tmp) {
                let mut title = task.title.clone();
                let mut description = task.description.clone();
                let mut repo_path = task.repo_path.clone();
                let mut new_status = task.status;

                let fields = parse_editor_content(&edited);
                if !fields.title.is_empty() {
                    title = fields.title;
                }
                if !fields.description.is_empty() {
                    description = fields.description;
                }
                if !fields.repo_path.is_empty() {
                    repo_path = fields.repo_path;
                }
                if let Some(s) = models::TaskStatus::parse(&fields.status) {
                    new_status = s;
                }

                // Update DB and in-memory state
                if let Err(e) = rt.database.update_task(task_id, &title, &description, &repo_path, new_status, task.plan.as_deref()) {
                    app.update(Message::Error(format!("DB error updating task: {e}")));
                }
                app.update(Message::TaskEdited {
                    id: task_id,
                    title,
                    description,
                    repo_path,
                    status: new_status,
                });
            }
        }
    }

    let _ = std::fs::remove_file(&tmp);
    Ok(())
}
