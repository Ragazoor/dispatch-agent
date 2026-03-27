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

use crate::db::TaskStore;
use crate::editor::{format_editor_content, parse_editor_content};
use crate::process::{ProcessRunner, RealProcessRunner};
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
    tracing::info!(port, db = %db_path.display(), "TUI started, MCP server on port {port}");

    let runtime = TuiRuntime {
        database,
        msg_tx,
        port,
        input_paused,
        runner: Arc::new(RealProcessRunner),
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
// TerminalSuspend — RAII guard for leaving/re-entering the alternate screen
// ---------------------------------------------------------------------------

struct TerminalSuspend<'a> {
    terminal: &'a mut Terminal<CrosstermBackend<io::Stdout>>,
}

impl<'a> TerminalSuspend<'a> {
    fn new(terminal: &'a mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<Self> {
        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        terminal.show_cursor()?;
        Ok(TerminalSuspend { terminal })
    }
}

impl Drop for TerminalSuspend<'_> {
    fn drop(&mut self) {
        let _ = enable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), EnterAlternateScreen);
        let _ = self.terminal.hide_cursor();
        let _ = self.terminal.clear();
    }
}

// ---------------------------------------------------------------------------
// TuiRuntime — shared context for command execution
// ---------------------------------------------------------------------------

struct TuiRuntime {
    database: Arc<dyn db::TaskStore>,
    msg_tx: mpsc::UnboundedSender<Message>,
    port: u16,
    input_paused: Arc<AtomicBool>,
    runner: Arc<dyn ProcessRunner>,
}

impl TuiRuntime {
    fn exec_insert_task(&self, app: &mut App, title: String, description: String, repo_path: String) {
        match self.database.create_task(&title, &description, &repo_path, None, models::TaskStatus::Backlog) {
            Ok(new_id) => {
                let now = chrono::Utc::now();
                let task = models::Task {
                    id: new_id,
                    title,
                    description,
                    repo_path,
                    status: models::TaskStatus::Backlog,
                    worktree: None,
                    tmux_window: None,
                    plan: None,
                    created_at: now,
                    updated_at: now,
                };
                app.update(Message::TaskCreated { task });
            }
            Err(e) => {
                app.update(Message::Error(format!("DB error creating task: {e}")));
            }
        }
    }

    fn exec_quick_dispatch(&self, app: &mut App, title: String, description: String, repo_path: String) {
        // 1. Create task in DB with status Ready
        match self.database.create_task(&title, &description, &repo_path, None, models::TaskStatus::Ready) {
            Ok(new_id) => {
                let now = chrono::Utc::now();
                let task = models::Task {
                    id: new_id,
                    title,
                    description,
                    repo_path: repo_path.clone(),
                    status: models::TaskStatus::Ready,
                    worktree: None,
                    tmux_window: None,
                    plan: None,
                    created_at: now,
                    updated_at: now,
                };
                // 2. Add task to in-memory state
                app.update(Message::TaskCreated { task: task.clone() });
                // 3. Save repo path
                let _ = self.database.save_repo_path(&repo_path);
                let paths = self.database.list_repo_paths().unwrap_or_default();
                app.update(Message::RepoPathsUpdated(paths));
                // 4. Dispatch the agent
                let tx = self.msg_tx.clone();
                let port = self.port;
                let runner = self.runner.clone();
                tokio::task::spawn_blocking(move || {
                    let id = task.id;
                    match dispatch::quick_dispatch_agent(&task, port, &*runner) {
                        Ok(result) => {
                            let _ = tx.send(Message::Dispatched {
                                id,
                                worktree: result.worktree_path,
                                tmux_window: result.tmux_window,
                            });
                        }
                        Err(e) => {
                            let _ = tx.send(Message::Error(format!("Quick dispatch failed: {e:#}")));
                        }
                    }
                });
            }
            Err(e) => {
                app.update(Message::Error(format!("DB error creating task: {e}")));
            }
        }
    }

    fn exec_persist_task(&self, app: &mut App, task: models::Task) {
        if let Err(e) = self.database.persist_task(
            task.id,
            task.status,
            task.worktree.as_deref(),
            task.tmux_window.as_deref(),
        ) {
            app.update(Message::Error(format!("DB error persisting task: {e}")));
        }
    }

    fn exec_delete_task(&self, app: &mut App, id: i64) {
        if let Err(e) = self.database.delete_task(id) {
            app.update(Message::Error(format!("DB error deleting task: {e}")));
        }
    }

    fn exec_dispatch(&self, task: models::Task) {
        let tx = self.msg_tx.clone();
        let port = self.port;
        let runner = self.runner.clone();

        tokio::task::spawn_blocking(move || {
            let id = task.id;
            tracing::info!(task_id = id, "dispatching task");
            match dispatch::dispatch_agent(&task, port, &*runner) {
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

    fn exec_brainstorm(&self, task: models::Task) {
        let tx = self.msg_tx.clone();
        let port = self.port;
        let runner = self.runner.clone();

        tokio::task::spawn_blocking(move || {
            let id = task.id;
            tracing::info!(task_id = id, "brainstorming task");
            match dispatch::brainstorm_agent(&task, port, &*runner) {
                Ok(result) => {
                    let _ = tx.send(Message::Dispatched {
                        id,
                        worktree: result.worktree_path,
                        tmux_window: result.tmux_window,
                    });
                }
                Err(e) => {
                    let _ = tx.send(Message::Error(format!("Brainstorm dispatch failed: {e:#}")));
                }
            }
        });
    }

    fn exec_capture_tmux(&self, id: i64, window: String) {
        let tx = self.msg_tx.clone();
        let runner = self.runner.clone();

        tokio::task::spawn_blocking(move || {
            if let Ok(false) = tmux::has_window(&window, &*runner) {
                let _ = tx.send(Message::WindowGone(id));
                return;
            }

            match tmux::capture_pane(&window, 5, &*runner) {
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

    fn exec_edit_in_editor(
        &self,
        app: &mut App,
        task: models::Task,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> Result<()> {
        let task_id = task.id;
        let tmp = std::env::temp_dir().join(format!("task-{task_id}.txt"));
        let content = format_editor_content(&task.title, &task.description, &task.repo_path, task.status.as_str(), task.plan.as_deref().unwrap_or(""));
        std::fs::write(&tmp, &content)?;

        // Pause the input polling thread so vim can read keypresses
        self.input_paused.store(true, Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(150));

        // Suspend TUI (RAII guard restores on drop, even if editor panics)
        let _guard = TerminalSuspend::new(terminal)?;

        // Open editor
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vim".to_string());
        let status = std::process::Command::new(&editor)
            .arg(&tmp)
            .status();

        // Guard restores terminal on drop
        drop(_guard);

        // Resume input polling thread
        self.input_paused.store(false, Ordering::Relaxed);

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
                    let plan = if fields.plan.is_empty() { None } else { Some(fields.plan) };

                    // Update DB and in-memory state
                    if let Err(e) = self.database.update_task(task_id, &title, &description, &repo_path, new_status, plan.as_deref()) {
                        app.update(Message::Error(format!("DB error updating task: {e}")));
                    }
                    app.update(Message::TaskEdited {
                        id: task_id,
                        title,
                        description,
                        repo_path,
                        status: new_status,
                        plan,
                    });
                }
            }
        }

        let _ = std::fs::remove_file(&tmp);
        Ok(())
    }

    fn exec_save_repo_path(&self, app: &mut App, path: String) {
        let _ = self.database.save_repo_path(&path);
        let paths = self.database.list_repo_paths().unwrap_or_default();
        app.update(Message::RepoPathsUpdated(paths));
    }

    fn exec_refresh_from_db(&self, app: &mut App) {
        // Re-read all tasks from SQLite to pick up MCP/CLI updates
        match self.database.list_all() {
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

    fn exec_cleanup(&self, repo_path: String, worktree: String, tmux_window: Option<String>) {
        let tx = self.msg_tx.clone();
        let runner = self.runner.clone();

        tokio::task::spawn_blocking(move || {
            if let Err(e) = dispatch::cleanup_task(&repo_path, &worktree, tmux_window.as_deref(), &*runner) {
                let _ = tx.send(Message::Error(format!("Cleanup failed: {e:#}")));
            }
        });
    }

    fn exec_resume(&self, task: models::Task) {
        let tx = self.msg_tx.clone();
        let id = task.id;
        let worktree_path = task.worktree.clone().unwrap_or_default();
        let runner = self.runner.clone();

        tokio::task::spawn_blocking(move || {
            tracing::info!(task_id = id, "resuming task");
            match dispatch::resume_agent(id, &worktree_path, &*runner) {
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

    fn exec_jump_to_tmux(&self, app: &mut App, window: String) {
        if let Err(e) = tmux::select_window(&window, &*self.runner) {
            app.update(Message::Error(format!("Jump failed: {e:#}")));
        }
    }
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
            Command::PersistTask(task) => rt.exec_persist_task(app, task),
            Command::InsertTask { title, description, repo_path } =>
                rt.exec_insert_task(app, title, description, repo_path),
            Command::DeleteTask(id) => rt.exec_delete_task(app, id),
            Command::Dispatch { task } => rt.exec_dispatch(task),
            Command::Brainstorm { task } => rt.exec_brainstorm(task),
            Command::CaptureTmux { id, window } => rt.exec_capture_tmux(id, window),
            Command::EditTaskInEditor(task) => rt.exec_edit_in_editor(app, task, terminal)?,
            Command::SaveRepoPath(path) => rt.exec_save_repo_path(app, path),
            Command::RefreshFromDb => rt.exec_refresh_from_db(app),
            Command::Cleanup { repo_path, worktree, tmux_window } =>
                rt.exec_cleanup(repo_path, worktree, tmux_window),
            Command::Resume { task } => rt.exec_resume(task),
            Command::JumpToTmux { window } => rt.exec_jump_to_tmux(app, window),
            Command::QuickDispatch { title, description, repo_path } =>
                rt.exec_quick_dispatch(app, title, description, repo_path),
        }
    }

    Ok(())
}
