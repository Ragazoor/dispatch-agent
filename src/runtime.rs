use anyhow::Result;
use crossterm::{
    event::{self, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::collections::HashSet;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::interval;

use tempfile::Builder as TempfileBuilder;

use crate::db::{EpicPatch, TaskStore};
use crate::editor::{format_editor_content, parse_editor_content, format_epic_for_editor, parse_epic_editor_output};
use crate::process::{ProcessRunner, RealProcessRunner};
use crate::tui::{self, App, Command, Message};
use crate::models::TaskId;
use crate::{db, dispatch, models, mcp, tmux};

// ---------------------------------------------------------------------------
// run_tui — entry point for the TUI mode
// ---------------------------------------------------------------------------

pub async fn run_tui(db_path: &Path, port: u16, inactivity_timeout: u64) -> Result<()> {
    // 1. Open database and load initial tasks
    let database = Arc::new(db::Database::open(db_path)?);
    let tasks = database.list_all()?;

    // 2. Spawn MCP server with notification channel
    let runner: Arc<dyn ProcessRunner> = Arc::new(RealProcessRunner);
    let mcp_db = database.clone();
    let mcp_runner = runner.clone();
    let (mcp_notify_tx, mut mcp_notify_rx) = mpsc::unbounded_channel::<()>();
    tokio::spawn(async move {
        if let Err(e) = mcp::serve(mcp_db, port, mcp_notify_tx, mcp_runner).await {
            eprintln!("MCP server error: {e}");
        }
    });

    // 3. Create App and load saved repo paths
    let mut app = App::new(tasks, Duration::from_secs(inactivity_timeout));
    let paths = database.list_repo_paths().unwrap_or_default();
    app.update(Message::RepoPathsUpdated(paths));
    let usage = database.get_all_usage().unwrap_or_default();
    app.update(Message::RefreshUsage(usage));

    // Load notification preference
    let notif_enabled = database.get_setting_bool("notifications_enabled")
        .unwrap_or(None)
        .unwrap_or(true);
    app.set_notifications_enabled(notif_enabled);

    // Load repo filter (intersect with known repo_paths to prune stale entries)
    if let Some(filter_str) = database.get_setting_string("repo_filter").unwrap_or(None) {
        if !filter_str.is_empty() {
            let known: HashSet<&str> = app.repo_paths().iter().map(|s| s.as_str()).collect();
            let filter: HashSet<String> = filter_str
                .split('\n')
                .filter(|s| known.contains(s))
                .map(|s| s.to_string())
                .collect();
            app.set_repo_filter(filter);
        }
    }

    // Load saved filter presets
    match database.list_filter_presets() {
        Ok(raw) => {
            let presets: Vec<(String, HashSet<String>)> = raw
                .into_iter()
                .map(|(name, paths_str)| {
                    let set: HashSet<String> = paths_str
                        .split('\n')
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string())
                        .collect();
                    (name, set)
                })
                .collect();
            app.update(Message::FilterPresetsLoaded(presets));
        }
        Err(e) => {
            tracing::warn!("Failed to load filter presets: {e}");
        }
    }

    // Load cached review PRs from database
    match database.load_review_prs() {
        Ok(prs) => app.set_review_prs(prs),
        Err(e) => tracing::warn!("Failed to load cached review PRs: {e}"),
    }

    // 4. Set up terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Set up tmux keybinding: Prefix+g → jump back to this window.
    // Best-effort: failures don't prevent the TUI from starting.
    let tmux_runner = runner.clone();
    let original_window_name = tmux::current_window_name(&*tmux_runner).ok();
    let _ = tmux::rename_window("", "dispatch", &*tmux_runner);
    let _ = tmux::bind_key("g", "select-window -t dispatch", &*tmux_runner);

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
        input_paused,
        runner,
    };
    let result = run_loop(
        &mut app,
        &mut terminal,
        &mut key_rx,
        &mut msg_rx,
        &mut mcp_notify_rx,
        &mut tick_interval,
        &runtime,
    )
    .await;

    // Tear down tmux keybinding and restore the original window name.
    let _ = tmux::unbind_key("g", &*tmux_runner);
    if let Some(ref name) = original_window_name {
        let _ = tmux::rename_window("dispatch", name, &*tmux_runner);
    }

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
// InputPausedGuard — RAII guard for pausing input + suspending the terminal
// ---------------------------------------------------------------------------

struct InputPausedGuard<'a> {
    input_paused: Arc<AtomicBool>,
    terminal_guard: Option<TerminalSuspend<'a>>,
}

impl<'a> InputPausedGuard<'a> {
    fn new(
        input_paused: &Arc<AtomicBool>,
        terminal: &'a mut Terminal<CrosstermBackend<io::Stdout>>,
        key_rx: &mut mpsc::UnboundedReceiver<crossterm::event::KeyEvent>,
    ) -> Result<Self> {
        input_paused.store(true, Ordering::Relaxed);
        while key_rx.try_recv().is_ok() {}
        let terminal_guard = TerminalSuspend::new(terminal)?;
        Ok(InputPausedGuard {
            input_paused: Arc::clone(input_paused),
            terminal_guard: Some(terminal_guard),
        })
    }
}

impl Drop for InputPausedGuard<'_> {
    fn drop(&mut self) {
        // Restore terminal first, then resume input (matches original ordering)
        drop(self.terminal_guard.take());
        self.input_paused.store(false, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// TuiRuntime — shared context for command execution
// ---------------------------------------------------------------------------

struct TuiRuntime {
    database: Arc<dyn db::TaskStore>,
    msg_tx: mpsc::UnboundedSender<Message>,
    input_paused: Arc<AtomicBool>,
    runner: Arc<dyn ProcessRunner>,
}

impl TuiRuntime {
    fn db_error(action: &str, e: impl std::fmt::Display) -> String {
        format!("DB error {action}: {e}")
    }

    fn create_task(
        &self,
        app: &mut App,
        title: &str,
        description: &str,
        repo_path: &str,
        tag: Option<&str>,
        epic_id: Option<models::EpicId>,
    ) -> Option<models::Task> {
        let mut task = match self.database.create_task_returning(
            title,
            description,
            repo_path,
            None,
            models::TaskStatus::Backlog,
        ) {
            Ok(task) => task,
            Err(e) => {
                app.update(Message::Error(Self::db_error("creating task", e)));
                return None;
            }
        };
        if let Some(eid) = epic_id {
            if let Err(e) = self.database.set_task_epic_id(task.id, Some(eid)) {
                app.update(Message::Error(Self::db_error("linking task to epic", e)));
                return None;
            }
            task.epic_id = Some(eid);
        }
        if let Some(t) = tag {
            let patch = db::TaskPatch::new().tag(Some(t));
            if let Err(e) = self.database.patch_task(task.id, &patch) {
                app.update(Message::Error(Self::db_error("setting task tag", e)));
                return None;
            }
            task.tag = Some(t.to_string());
        }
        Some(task)
    }

    fn exec_insert_task(
        &self,
        app: &mut App,
        title: String,
        description: String,
        repo_path: String,
        tag: Option<String>,
        epic_id: Option<models::EpicId>,
    ) {
        if let Some(task) =
            self.create_task(app, &title, &description, &repo_path, tag.as_deref(), epic_id)
        {
            app.update(Message::TaskCreated { task });
        }
    }

    fn exec_quick_dispatch(
        &self,
        app: &mut App,
        title: String,
        description: String,
        repo_path: String,
        epic_id: Option<models::EpicId>,
    ) {
        let Some(task) = self.create_task(app, &title, &description, &repo_path, None, epic_id)
        else {
            return;
        };
        app.update(Message::TaskCreated { task: task.clone() });
        let _ = self.database.save_repo_path(&repo_path);
        let paths = self.database.list_repo_paths().unwrap_or_default();
        app.update(Message::RepoPathsUpdated(paths));
        let tx = self.msg_tx.clone();
        let runner = self.runner.clone();
        tokio::task::spawn_blocking(move || {
            let id = task.id;
            match dispatch::quick_dispatch_agent(&task, &*runner) {
                Ok(result) => {
                    let _ = tx.send(Message::Dispatched {
                        id,
                        worktree: result.worktree_path,
                        tmux_window: result.tmux_window,
                        switch_focus: true,
                    });
                }
                Err(e) => {
                    let _ = tx.send(Message::Error(format!("Quick dispatch failed: {e:#}")));
                }
            }
        });
    }

    fn exec_persist_task(&self, app: &mut App, task: models::Task) {
        if let Err(e) = self.database.patch_task(
            task.id,
            &db::TaskPatch::new()
                .status(task.status)
                .sub_status(task.sub_status)
                .worktree(task.worktree.as_deref())
                .tmux_window(task.tmux_window.as_deref())
                .pr_url(task.pr_url.as_deref())
                .sort_order(task.sort_order),
        ) {
            app.update(Message::Error(Self::db_error("persisting task", e)));
        }
        if let Some(epic_id) = task.epic_id {
            let _ = self.database.recalculate_epic_status(epic_id);
        }
    }

    fn exec_patch_sub_status(&self, app: &mut App, id: models::TaskId, sub_status: models::SubStatus) {
        if let Err(e) = self.database.patch_task(
            id,
            &db::TaskPatch::new().sub_status(sub_status),
        ) {
            app.update(Message::Error(Self::db_error("patching sub_status", e)));
        }
    }

    fn exec_delete_task(&self, app: &mut App, id: TaskId) {
        if let Err(e) = self.database.delete_task(id) {
            app.update(Message::Error(Self::db_error("deleting task", e)));
        }
    }

    fn spawn_dispatch<F>(&self, task: models::Task, dispatch_fn: F, label: &'static str)
    where
        F: FnOnce(&models::Task, &dyn ProcessRunner) -> Result<models::DispatchResult>
            + Send
            + 'static,
    {
        let tx = self.msg_tx.clone();
        let runner = self.runner.clone();

        tokio::task::spawn_blocking(move || {
            let id = task.id;
            tracing::info!(task_id = id.0, label, "dispatching");
            match dispatch_fn(&task, &*runner) {
                Ok(result) => {
                    // receiver dropped = app shutting down; nothing to log
                    let _ = tx.send(Message::Dispatched {
                        id,
                        worktree: result.worktree_path,
                        tmux_window: result.tmux_window,
                        switch_focus: false,
                    });
                }
                Err(e) => {
                    let _ = tx.send(Message::Error(format!("{label} failed: {e:#}")));
                }
            }
        });
    }

    fn exec_dispatch(&self, task: models::Task) {
        self.spawn_dispatch(task, dispatch::dispatch_agent, "Dispatch");
    }

    fn exec_brainstorm(&self, task: models::Task) {
        self.spawn_dispatch(task, dispatch::brainstorm_agent, "Brainstorm");
    }

    fn exec_plan(&self, task: models::Task) {
        self.spawn_dispatch(task, dispatch::plan_agent, "Plan");
    }

    fn exec_capture_tmux(&self, id: TaskId, window: String) {
        let tx = self.msg_tx.clone();
        let runner = self.runner.clone();

        tokio::task::spawn_blocking(move || {
            if let Ok(false) = tmux::has_window(&window, &*runner) {
                let _ = tx.send(Message::WindowGone(id));
                return;
            }

            // Activity timestamp for staleness detection (fall back to 0 on error
            // so we never falsely mark an agent as stale).
            let activity_ts = tmux::window_activity(&window, &*runner).unwrap_or(0);

            match tmux::capture_pane(&window, 5, &*runner) {
                Ok(output) => {
                    let _ = tx.send(Message::TmuxOutput { id, output, activity_ts });
                }
                Err(e) => {
                    let _ = tx.send(Message::Error(format!(
                        "tmux capture failed for window {window}: {e}"
                    )));
                }
            }
        });
    }

    /// Suspend the TUI, open content in $EDITOR, return edited text (or None).
    fn run_editor(
        &self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
        key_rx: &mut mpsc::UnboundedReceiver<crossterm::event::KeyEvent>,
        prefix: &str,
        content: &str,
    ) -> Result<Option<String>> {
        let mut tmp = TempfileBuilder::new()
            .prefix(prefix)
            .suffix(".md")
            .tempfile()?;
        std::io::Write::write_all(tmp.as_file_mut(), content.as_bytes())?;

        let _guard = InputPausedGuard::new(&self.input_paused, terminal, key_rx)?;
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vim".to_string());
        let status = std::process::Command::new(&editor)
            .arg(tmp.path())
            .status();
        drop(_guard);

        match status {
            Ok(exit) if exit.success() => Ok(std::fs::read_to_string(tmp.path()).ok()),
            Ok(exit) => {
                tracing::warn!(?exit, "editor exited with non-zero status");
                Ok(None)
            }
            Err(e) => {
                tracing::warn!("failed to spawn editor: {e}");
                Ok(None)
            }
        }
    }

    fn exec_edit_in_editor(
        &self,
        app: &mut App,
        task: models::Task,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
        key_rx: &mut mpsc::UnboundedReceiver<crossterm::event::KeyEvent>,
    ) -> Result<()> {
        let task_id = task.id;
        let content = format_editor_content(&task);
        let Some(edited) = self.run_editor(terminal, key_rx, &format!("task-{task_id}-"), &content)? else {
            return Ok(());
        };

        let fields = parse_editor_content(&edited);
        let title = if fields.title.is_empty() { task.title.clone() } else { fields.title };
        let description = if fields.description.is_empty() { task.description.clone() } else { fields.description };
        let repo_path = if fields.repo_path.is_empty() { task.repo_path.clone() } else { fields.repo_path };
        let new_status = models::TaskStatus::parse(&fields.status).unwrap_or(task.status);
        let plan = if fields.plan.is_empty() { None } else { Some(fields.plan) };
        let tag = if fields.tag.is_empty() { None } else { Some(fields.tag) };

        if let Err(e) = self.database.patch_task(
            task_id,
            &db::TaskPatch::new()
                .status(new_status)
                .title(&title)
                .description(&description)
                .repo_path(&repo_path)
                .plan(plan.as_deref())
                .tag(tag.as_deref()),
        ) {
            app.update(Message::Error(Self::db_error("updating task", e)));
        }
        app.update(Message::TaskEdited(tui::TaskEdit {
            id: task_id,
            title,
            description,
            repo_path,
            status: new_status,
            plan,
            tag,
        }));
        Ok(())
    }

    fn exec_save_repo_path(&self, app: &mut App, path: String) {
        if let Err(e) = self.database.save_repo_path(&path) {
            tracing::warn!("failed to save repo path: {e}");
        }
        let paths = self.database.list_repo_paths().unwrap_or_else(|e| {
            tracing::warn!("failed to list repo paths: {e}");
            vec![]
        });
        app.update(Message::RepoPathsUpdated(paths));
    }

    fn exec_refresh_from_db(&self, app: &mut App) -> Vec<Command> {
        let mut cmds = Vec::new();
        // Re-read all tasks from SQLite to pick up MCP/CLI updates
        match self.database.list_all() {
            Ok(tasks) => {
                cmds = app.update(Message::RefreshTasks(tasks));
            }
            Err(e) => {
                app.update(Message::Error(Self::db_error("refreshing tasks", e)));
            }
        }
        // Also refresh epics
        self.exec_refresh_epics_from_db(app);
        self.exec_refresh_usage_from_db(app);
        cmds
    }

    fn exec_send_notification(&self, title: &str, body: &str, urgent: bool) {
        let urgency = if urgent { "critical" } else { "normal" };
        if let Err(e) = self.runner.run("notify-send", &["-u", urgency, title, body]) {
            tracing::warn!("notify-send failed: {e}");
        }
    }

    fn exec_persist_setting(&self, app: &mut App, key: &str, value: bool) {
        if let Err(e) = self.database.set_setting_bool(key, value) {
            app.update(Message::Error(Self::db_error("persisting setting", e)));
        }
    }

    fn exec_persist_string_setting(&self, app: &mut App, key: &str, value: &str) {
        if let Err(e) = self.database.set_setting_string(key, value) {
            app.update(Message::Error(Self::db_error("persisting setting", e)));
        }
    }

    fn exec_persist_filter_preset(&self, app: &mut App, name: &str, repo_paths: &str) {
        if let Err(e) = self.database.save_filter_preset(name, repo_paths) {
            app.update(Message::Error(Self::db_error("saving filter preset", e)));
        }
    }

    fn exec_delete_filter_preset(&self, app: &mut App, name: &str) {
        if let Err(e) = self.database.delete_filter_preset(name) {
            app.update(Message::Error(Self::db_error("deleting filter preset", e)));
        }
    }

    fn exec_insert_epic(&self, app: &mut App, title: String, description: String, repo_path: String) {
        match self.database.create_epic(&title, &description, &repo_path) {
            Ok(epic) => {
                app.update(Message::EpicCreated(epic));
            }
            Err(e) => {
                app.update(Message::Error(Self::db_error("creating epic", e)));
            }
        }
    }

    fn exec_edit_epic_in_editor(
        &self,
        app: &mut App,
        epic: models::Epic,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
        key_rx: &mut mpsc::UnboundedReceiver<crossterm::event::KeyEvent>,
    ) -> Result<()> {
        let epic_id = epic.id;
        let content = format_epic_for_editor(&epic);
        let Some(edited) = self.run_editor(terminal, key_rx, &format!("epic-{epic_id}-"), &content)? else {
            return Ok(());
        };

        let fields = parse_epic_editor_output(&edited);
        let title = if fields.title.is_empty() { epic.title.clone() } else { fields.title };
        let description = if fields.description.is_empty() { epic.description.clone() } else { fields.description };
        let repo_path = if fields.repo_path.is_empty() { epic.repo_path.clone() } else { fields.repo_path };

        if let Err(e) = self.database.patch_epic(
            epic_id,
            &EpicPatch::new().title(&title).description(&description).repo_path(&repo_path),
        ) {
            app.update(Message::Error(Self::db_error("updating epic", e)));
        }
        let mut updated = epic;
        updated.title = title;
        updated.description = description;
        updated.repo_path = repo_path;
        app.update(Message::EpicEdited(updated));
        Ok(())
    }

    fn exec_delete_epic(&self, app: &mut App, id: models::EpicId) {
        if let Err(e) = self.database.delete_epic(id) {
            app.update(Message::Error(Self::db_error("deleting epic", e)));
        }
    }

    fn exec_persist_epic(&self, app: &mut App, id: models::EpicId, status: Option<models::TaskStatus>, sort_order: Option<i64>) {
        let mut patch = EpicPatch::new();
        if let Some(s) = status {
            patch = patch.status(s);
        }
        if let Some(so) = sort_order {
            patch = patch.sort_order(Some(so));
        }
        if patch.has_changes() {
            if let Err(e) = self.database.patch_epic(id, &patch) {
                app.update(Message::Error(Self::db_error("updating epic", e)));
            }
        }
    }

    fn exec_refresh_epics_from_db(&self, app: &mut App) {
        match self.database.list_epics() {
            Ok(epics) => {
                app.update(Message::RefreshEpics(epics));
            }
            Err(e) => {
                app.update(Message::Error(Self::db_error("refreshing epics", e)));
            }
        }
    }

    fn exec_refresh_usage_from_db(&self, app: &mut App) {
        match self.database.get_all_usage() {
            Ok(usage) => {
                app.update(Message::RefreshUsage(usage));
            }
            Err(e) => {
                tracing::warn!("Failed to refresh usage from db: {e}");
            }
        }
    }

    fn exec_cleanup(&self, id: TaskId, repo_path: String, worktree: String, tmux_window: Option<String>) {
        let shared = self
            .database
            .has_other_tasks_with_worktree(&worktree, id)
            .unwrap_or(false);

        if shared {
            // Other active tasks share this worktree — just detach this task
            tracing::info!(task_id = id.0, "worktree shared, detaching only");
            if let Err(e) = self.database.patch_task(id, &db::TaskPatch::new().worktree(None).tmux_window(None)) {
                let _ = self.msg_tx.send(Message::Error(format!("Detach failed: {e:#}")));
            }
            return;
        }

        // No other active tasks — full cleanup
        let tx = self.msg_tx.clone();
        let runner = self.runner.clone();

        tokio::task::spawn_blocking(move || {
            if let Err(e) = dispatch::cleanup_task(&repo_path, &worktree, tmux_window.as_deref(), &*runner) {
                let _ = tx.send(Message::Error(format!("Cleanup failed: {e:#}")));
            }
        });
    }

    fn exec_finish(
        &self,
        id: TaskId,
        repo_path: String,
        branch: String,
        worktree: String,
        tmux_window: Option<String>,
    ) {
        let shared = self
            .database
            .has_other_tasks_with_worktree(&worktree, id)
            .unwrap_or(false);

        if shared {
            tracing::info!(task_id = id.0, "worktree shared, detaching only (no rebase)");
            if let Err(e) = self
                .database
                .patch_task(id, &db::TaskPatch::new().worktree(None).tmux_window(None))
            {
                let _ = self
                    .msg_tx
                    .send(Message::Error(format!("Detach failed: {e:#}")));
            }
            let _ = self.msg_tx.send(Message::FinishComplete(id));
            return;
        }

        let tx = self.msg_tx.clone();
        let runner = self.runner.clone();

        tokio::task::spawn_blocking(move || {
            match dispatch::finish_task(
                &repo_path,
                &worktree,
                &branch,
                tmux_window.as_deref(),
                &*runner,
            ) {
                Ok(()) => {
                    let _ = tx.send(Message::FinishComplete(id));
                }
                Err(e) => {
                    let is_conflict = matches!(e, dispatch::FinishError::RebaseConflict(_));
                    let _ = tx.send(Message::FinishFailed {
                        id,
                        error: e.to_string(),
                        is_conflict,
                    });
                }
            }
        });
    }

    fn exec_resume(&self, task: models::Task) {
        let tx = self.msg_tx.clone();
        let id = task.id;
        let worktree_path = task.worktree.clone().unwrap_or_default();
        let runner = self.runner.clone();

        tokio::task::spawn_blocking(move || {
            tracing::info!(task_id = id.0, "resuming task");
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

    fn exec_dispatch_epic(&self, app: &mut App, epic: models::Epic) {
        let title = format!("Plan: {}", epic.title);
        let description = format!(
            "Planning subtask for epic: {}\n\n{}",
            epic.title, epic.description
        );

        // Create the planning subtask in DB as Backlog
        let task = match self.database.create_task_returning(
            &title,
            &description,
            &epic.repo_path,
            None,
            models::TaskStatus::Backlog,
        ) {
            Ok(mut task) => {
                if let Err(e) = self.database.set_task_epic_id(task.id, Some(epic.id)) {
                    app.update(Message::Error(Self::db_error("linking planning task to epic", e)));
                    return;
                }
                task.epic_id = Some(epic.id);
                task
            }
            Err(e) => {
                app.update(Message::Error(Self::db_error("creating planning task", e)));
                return;
            }
        };

        app.update(Message::TaskCreated { task: task.clone() });

        // Dispatch the planning subtask asynchronously
        let tx = self.msg_tx.clone();
        let runner = self.runner.clone();
        let epic_id = epic.id;
        let epic_title = epic.title.clone();
        let epic_description = epic.description.clone();

        tokio::task::spawn_blocking(move || {
            let id = task.id;
            tracing::info!(task_id = id.0, epic_id = epic_id.0, "dispatching epic planning agent");
            match dispatch::epic_planning_agent(&task, epic_id, &epic_title, &epic_description, &*runner) {
                Ok(result) => {
                    let _ = tx.send(Message::Dispatched {
                        id,
                        worktree: result.worktree_path,
                        tmux_window: result.tmux_window,
                        switch_focus: true,
                    });
                }
                Err(e) => {
                    let _ = tx.send(Message::Error(format!("Epic planning dispatch failed: {e:#}")));
                }
            }
        });
    }

    fn exec_kill_tmux_window(&self, window: String) {
        let runner = self.runner.clone();
        let tx = self.msg_tx.clone();

        tokio::task::spawn_blocking(move || {
            if let Err(e) = tmux::kill_window(&window, &*runner) {
                let _ = tx.send(Message::Error(format!("Kill window failed: {e:#}")));
            }
        });
    }

    fn exec_create_pr(
        &self,
        id: TaskId,
        repo_path: String,
        branch: String,
        title: String,
        description: String,
    ) {
        let tx = self.msg_tx.clone();
        let runner = self.runner.clone();

        tokio::task::spawn_blocking(move || {
            match dispatch::create_pr(&repo_path, &branch, &title, &description, &*runner) {
                Ok(result) => {
                    let _ = tx.send(Message::PrCreated {
                        id,
                        pr_url: result.pr_url,
                    });
                }
                Err(e) => {
                    let _ = tx.send(Message::PrFailed {
                        id,
                        error: e.to_string(),
                    });
                }
            }
        });
    }

    fn exec_check_pr_status(
        &self,
        id: TaskId,
        pr_url: String,
    ) {
        let tx = self.msg_tx.clone();
        let runner = self.runner.clone();

        tokio::task::spawn_blocking(move || {
            match dispatch::check_pr_status(&pr_url, &*runner) {
                Ok(status) => {
                    if status.state == dispatch::PrState::Merged {
                        let _ = tx.send(Message::PrMerged(id));
                    } else if status.state == dispatch::PrState::Open {
                        let _ = tx.send(Message::PrReviewState {
                            id,
                            review_decision: status.review_decision,
                        });
                    }
                    // Closed PRs: no message
                }
                Err(e) => {
                    tracing::warn!(task_id = id.0, "PR status check failed: {e}");
                }
            }
        });
    }

    fn exec_fetch_review_prs(&self) {
        let tx = self.msg_tx.clone();
        let runner = self.runner.clone();
        tokio::task::spawn_blocking(move || {
            tracing::info!("fetching review PRs via gh");
            match crate::github::fetch_review_prs(&*runner) {
                Ok(prs) => {
                    tracing::info!(count = prs.len(), "review PRs fetched successfully");
                    let _ = tx.send(Message::ReviewPrsLoaded(prs));
                }
                Err(e) => {
                    tracing::warn!(error = %e, "review PR fetch failed");
                    let _ = tx.send(Message::ReviewPrsFetchFailed(e));
                }
            }
        });
    }

    fn exec_persist_review_prs(&self, prs: Vec<crate::models::ReviewPr>) {
        if let Err(e) = self.database.save_review_prs(&prs) {
            tracing::warn!("Failed to persist review PRs: {e}");
        }
    }

    fn exec_fetch_my_prs(&self) {
        let tx = self.msg_tx.clone();
        let runner = self.runner.clone();
        tokio::task::spawn_blocking(move || {
            tracing::info!("fetching my PRs via gh");
            match crate::github::fetch_my_prs(&*runner) {
                Ok(prs) => {
                    tracing::info!(count = prs.len(), "my PRs fetched successfully");
                    let _ = tx.send(Message::MyPrsLoaded(prs));
                }
                Err(e) => {
                    tracing::warn!(error = %e, "my PR fetch failed");
                    let _ = tx.send(Message::MyPrsFetchFailed(e));
                }
            }
        });
    }

    fn exec_persist_my_prs(&self, prs: Vec<crate::models::ReviewPr>) {
        if let Err(e) = self.database.save_my_prs(&prs) {
            tracing::warn!("Failed to persist my PRs: {e}");
        }
    }

    fn exec_open_in_browser(&self, url: String) {
        let runner = self.runner.clone();
        tokio::task::spawn_blocking(move || {
            if let Err(e) = runner.run("xdg-open", &[&url]) {
                tracing::warn!("Failed to open browser: {e}");
            }
        });
    }

    fn exec_dispatch_review_agent(
        &self,
        repo_path: String,
        number: i64,
        title: String,
        body: String,
        head_ref: String,
    ) {
        let tx = self.msg_tx.clone();
        let runner = self.runner.clone();
        tokio::task::spawn_blocking(move || {
            match crate::dispatch::dispatch_review_agent(
                &repo_path, number, &title, &body, &head_ref, &*runner,
            ) {
                Ok(result) => {
                    let _ = tx.send(Message::ReviewAgentDispatched {
                        repo: repo_path,
                        number,
                        tmux_window: result.tmux_window,
                    });
                }
                Err(e) => {
                    let _ = tx.send(Message::ReviewAgentFailed {
                        error: format!("{e:#}"),
                    });
                }
            }
        });
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
    mcp_notify_rx: &mut mpsc::UnboundedReceiver<()>,
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

            // Async messages (e.g., from dispatch results)
            Some(msg) = msg_rx.recv() => {
                app.update(msg)
            }

            // MCP mutation notification — immediate refresh
            Some(()) = mcp_notify_rx.recv() => {
                rt.exec_refresh_from_db(app)
            }

            // Periodic tick for tmux capture
            _ = tick_interval.tick() => {
                app.update(Message::Tick)
            }
        };

        execute_commands(app, commands, rt, terminal, key_rx).await?;
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
    key_rx: &mut mpsc::UnboundedReceiver<crossterm::event::KeyEvent>,
) -> Result<()> {
    let mut queue = std::collections::VecDeque::from(commands);
    while let Some(command) = queue.pop_front() {
        match command {
            Command::PersistTask(task) => rt.exec_persist_task(app, task),
            Command::InsertTask { draft, epic_id } =>
                rt.exec_insert_task(app, draft.title, draft.description, draft.repo_path, draft.tag, epic_id),
            Command::DeleteTask(id) => rt.exec_delete_task(app, id),
            Command::Dispatch { task } => rt.exec_dispatch(task),
            Command::Brainstorm { task } => rt.exec_brainstorm(task),
            Command::Plan { task } => rt.exec_plan(task),
            Command::CaptureTmux { id, window } => rt.exec_capture_tmux(id, window),
            Command::EditTaskInEditor(task) => rt.exec_edit_in_editor(app, task, terminal, key_rx)?,
            Command::SaveRepoPath(path) => rt.exec_save_repo_path(app, path),
            Command::RefreshFromDb => {
                let extra = rt.exec_refresh_from_db(app);
                queue.extend(extra);
            }
            Command::Cleanup { id, repo_path, worktree, tmux_window } =>
                rt.exec_cleanup(id, repo_path, worktree, tmux_window),
            Command::Resume { task } => rt.exec_resume(task),
            Command::JumpToTmux { window } => rt.exec_jump_to_tmux(app, window),
            Command::QuickDispatch { draft, epic_id } =>
                rt.exec_quick_dispatch(app, draft.title, draft.description, draft.repo_path, epic_id),
            Command::KillTmuxWindow { window } => rt.exec_kill_tmux_window(window),
            Command::Finish { id, repo_path, branch, worktree, tmux_window } =>
                rt.exec_finish(id, repo_path, branch, worktree, tmux_window),
            // Epic commands
            Command::InsertEpic(draft) => {
                rt.exec_insert_epic(app, draft.title, draft.description, draft.repo_path)
            }
            Command::EditEpicInEditor(epic) => rt.exec_edit_epic_in_editor(app, epic, terminal, key_rx)?,
            Command::DeleteEpic(id) => rt.exec_delete_epic(app, id),
            Command::PersistEpic { id, status, sort_order } => rt.exec_persist_epic(app, id, status, sort_order),
            Command::RefreshEpicsFromDb => rt.exec_refresh_epics_from_db(app),
            Command::DispatchEpic { epic } => rt.exec_dispatch_epic(app, epic),
            Command::SendNotification { title, body, urgent } =>
                rt.exec_send_notification(&title, &body, urgent),
            Command::PersistSetting { key, value } =>
                rt.exec_persist_setting(app, &key, value),
            Command::CreatePr { id, repo_path, branch, title, description } =>
                rt.exec_create_pr(id, repo_path, branch, title, description),
            Command::CheckPrStatus { id, pr_url } =>
                rt.exec_check_pr_status(id, pr_url),
            Command::PersistStringSetting { key, value } =>
                rt.exec_persist_string_setting(app, &key, &value),
            Command::FetchReviewPrs => rt.exec_fetch_review_prs(),
            Command::PersistReviewPrs(prs) => rt.exec_persist_review_prs(prs),
            Command::FetchMyPrs => rt.exec_fetch_my_prs(),
            Command::PersistMyPrs(prs) => rt.exec_persist_my_prs(prs),
            Command::OpenInBrowser { url } => rt.exec_open_in_browser(url),
            Command::PersistFilterPreset { name, repo_paths } => {
                rt.exec_persist_filter_preset(app, &name, &repo_paths)
            }
            Command::DeleteFilterPreset(name) => rt.exec_delete_filter_preset(app, &name),
            Command::PatchSubStatus { id, sub_status } => {
                rt.exec_patch_sub_status(app, id, sub_status)
            }
            Command::DispatchReviewAgent { repo, number, title, body, head_ref } => {
                rt.exec_dispatch_review_agent(repo, number, title, body, head_ref)
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::db::Database;
    use crate::process::MockProcessRunner;

    #[test]
    fn db_error_formats_consistently() {
        assert_eq!(
            TuiRuntime::db_error("creating task", "disk full"),
            "DB error creating task: disk full"
        );
    }

    fn test_runtime() -> (TuiRuntime, App) {
        let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().unwrap());
        let (tx, _rx) = mpsc::unbounded_channel();
        let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
        let rt = TuiRuntime {
            database: db.clone(),
            msg_tx: tx,

            input_paused: Arc::new(AtomicBool::new(false)),
            runner,
        };
        let tasks = db.list_all().unwrap();
        let app = App::new(tasks, Duration::from_secs(300));
        (rt, app)
    }

    #[test]
    fn exec_insert_task_adds_to_db_and_app() {
        let (rt, mut app) = test_runtime();
        rt.exec_insert_task(&mut app, "Test".into(), "Desc".into(), "/repo".into(), None, None);
        assert_eq!(app.tasks().len(), 1);
        assert_eq!(app.tasks()[0].title, "Test");
        assert_eq!(rt.database.list_all().unwrap().len(), 1);
    }

    #[test]
    fn exec_delete_task_removes_from_db() {
        let (rt, mut app) = test_runtime();
        rt.exec_insert_task(&mut app, "Test".into(), "Desc".into(), "/repo".into(), None, None);
        let id = app.tasks()[0].id;
        rt.exec_delete_task(&mut app, id);
        assert!(rt.database.list_all().unwrap().is_empty());
    }

    #[test]
    fn exec_persist_task_saves_status_to_db() {
        let (rt, mut app) = test_runtime();
        rt.exec_insert_task(&mut app, "Test".into(), "Desc".into(), "/repo".into(), None, None);
        let mut task = app.tasks()[0].clone();
        task.status = models::TaskStatus::Running;
        task.sub_status = models::SubStatus::Active;
        task.worktree = Some("/repo/.worktrees/1-test".into());
        rt.exec_persist_task(&mut app, task);
        let db_task = rt.database.get_task(app.tasks()[0].id).unwrap().unwrap();
        assert_eq!(db_task.status, models::TaskStatus::Running);
        assert_eq!(db_task.worktree.as_deref(), Some("/repo/.worktrees/1-test"));
    }

    #[test]
    fn exec_persist_task_preserves_sub_status() {
        let (rt, mut app) = test_runtime();
        rt.exec_insert_task(&mut app, "PR Task".into(), "Desc".into(), "/repo".into(), None, None);
        let id = app.tasks()[0].id;
        // Put task in Review+Approved state in DB, then sync to app
        rt.database.patch_task(id, &db::TaskPatch::new()
            .status(models::TaskStatus::Review)
            .sub_status(models::SubStatus::Approved)
            .pr_url(Some("https://github.com/org/repo/pull/42")))
        .unwrap();
        rt.exec_refresh_from_db(&mut app);
        assert_eq!(app.tasks()[0].sub_status, models::SubStatus::Approved);

        // Persist the in-memory task (simulates handle_pr_review_state saving after PR approval)
        let task = app.tasks()[0].clone();
        rt.exec_persist_task(&mut app, task);

        // sub_status must survive the round-trip to DB
        let db_task = rt.database.get_task(id).unwrap().unwrap();
        assert_eq!(db_task.sub_status, models::SubStatus::Approved);
    }

    #[test]
    fn exec_save_repo_path_updates_app_state() {
        let (rt, mut app) = test_runtime();
        rt.exec_save_repo_path(&mut app, "/repo".into());
        assert!(app.repo_paths().contains(&"/repo".to_string()));
    }

    #[test]
    fn exec_refresh_from_db_syncs_external_changes() {
        let (rt, mut app) = test_runtime();
        // Insert directly into DB, bypassing app
        rt.database
            .create_task("External", "Added via CLI", "/repo", None, models::TaskStatus::Backlog)
            .unwrap();
        assert!(app.tasks().is_empty());
        rt.exec_refresh_from_db(&mut app);
        assert_eq!(app.tasks().len(), 1);
        assert_eq!(app.tasks()[0].title, "External");
    }

    #[test]
    fn exec_refresh_from_db_returns_commands_from_refresh() {
        let (rt, mut app) = test_runtime();
        // Insert a task directly into DB as Running
        rt.database
            .create_task("Test", "Desc", "/repo", None, models::TaskStatus::Running)
            .unwrap();
        // Load it into app
        let cmds = rt.exec_refresh_from_db(&mut app);
        assert!(cmds.is_empty()); // First load — no transition

        // Now update it to Review directly in DB
        let task = rt.database.list_all().unwrap()[0].clone();
        rt.database.patch_task(task.id, &db::TaskPatch::new().status(models::TaskStatus::Review)).unwrap();

        // Refresh should detect the transition and return a SendNotification
        let cmds = rt.exec_refresh_from_db(&mut app);
        assert!(cmds.iter().any(|c| matches!(c, Command::SendNotification { .. })));
    }

    #[test]
    fn exec_delete_task_nonexistent_shows_error() {
        let (rt, mut app) = test_runtime();
        rt.exec_delete_task(&mut app, TaskId(999));
        assert!(app.error_popup().is_some());
    }

    #[test]
    fn exec_jump_to_tmux_calls_select_window() {
        let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().unwrap());
        let (tx, _rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // for select-window
        ]));
        let rt = TuiRuntime {
            database: db.clone(),
            msg_tx: tx,

            input_paused: Arc::new(AtomicBool::new(false)),
            runner: mock.clone(),
        };
        let tasks = db.list_all().unwrap();
        let mut app = App::new(tasks, Duration::from_secs(300));

        rt.exec_jump_to_tmux(&mut app, "my-window".to_string());

        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].1.contains(&"select-window".to_string()));
        assert!(calls[0].1.contains(&"my-window".to_string()));
        assert!(app.error_popup().is_none());
    }

    #[tokio::test]
    async fn exec_dispatch_sends_dispatched_message() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().to_str().unwrap();
        // Create .worktrees/ and fake worktree directory so file writes succeed
        std::fs::create_dir_all(format!("{repo}/.worktrees/1-test-task")).unwrap();

        let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().unwrap());
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![
            MockProcessRunner::fail("not a git repo"), // detect_default_branch (fallback to "main")
            // git worktree add is skipped (dir pre-created above)
            MockProcessRunner::ok(),  // tmux new-window
            MockProcessRunner::ok(),  // tmux set-option @dispatch_dir
            MockProcessRunner::ok(),  // tmux set-hook (after-split-window)
            MockProcessRunner::ok(),  // tmux send-keys -l
            MockProcessRunner::ok(),  // tmux send-keys Enter
        ]));
        let rt = TuiRuntime {
            database: db.clone(),
            msg_tx: tx,

            input_paused: Arc::new(AtomicBool::new(false)),
            runner: mock,
        };

        let task = db.create_task_returning("Test Task", "desc", repo, None, models::TaskStatus::Backlog).unwrap();
        rt.exec_dispatch(task);

        let msg = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap();
        assert!(matches!(msg, Message::Dispatched { .. }), "Expected Dispatched, got: {msg:?}");
    }

    #[tokio::test]
    async fn exec_dispatch_sends_error_on_failure() {
        let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().unwrap());
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![
            MockProcessRunner::fail("fatal: not a git repository"),  // git worktree add fails
        ]));
        let rt = TuiRuntime {
            database: db.clone(),
            msg_tx: tx,

            input_paused: Arc::new(AtomicBool::new(false)),
            runner: mock,
        };

        let task = db.create_task_returning("Fail Task", "desc", "/nonexistent", None, models::TaskStatus::Backlog).unwrap();
        rt.exec_dispatch(task);

        let msg = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap();
        assert!(matches!(msg, Message::Error(_)), "Expected Error, got: {msg:?}");
    }

    #[tokio::test]
    async fn exec_capture_tmux_sends_output() {
        let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().unwrap());
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![
            // has_window: list-windows returns the window name
            MockProcessRunner::ok_with_stdout(b"test-window\n"),
            // window_activity: display-message returns a timestamp
            MockProcessRunner::ok_with_stdout(b"1711700000\n"),
            // capture-pane
            MockProcessRunner::ok_with_stdout(b"Hello from tmux\n"),
        ]));
        let rt = TuiRuntime {
            database: db.clone(),
            msg_tx: tx,

            input_paused: Arc::new(AtomicBool::new(false)),
            runner: mock,
        };

        rt.exec_capture_tmux(TaskId(1), "test-window".to_string());

        let msg = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap();
        let Message::TmuxOutput { id, output, activity_ts } = msg else {
            panic!("Expected TmuxOutput, got: {msg:?}");
        };
        assert_eq!(id, TaskId(1));
        assert!(output.contains("Hello from tmux"));
        assert_eq!(activity_ts, 1711700000);
    }

    #[tokio::test]
    async fn exec_capture_tmux_window_gone() {
        let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().unwrap());
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![
            // has_window: list-windows returns other window names (not our window)
            MockProcessRunner::ok_with_stdout(b"other-window\n"),
        ]));
        let rt = TuiRuntime {
            database: db.clone(),
            msg_tx: tx,

            input_paused: Arc::new(AtomicBool::new(false)),
            runner: mock,
        };

        rt.exec_capture_tmux(TaskId(1), "gone-window".to_string());

        let msg = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap();
        assert!(matches!(msg, Message::WindowGone(TaskId(1))), "Expected WindowGone, got: {msg:?}");
    }

    #[test]
    fn exec_jump_to_tmux_failure_shows_error() {
        let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().unwrap());
        let (tx, _rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![
            MockProcessRunner::fail("no such window"), // simulate tmux failure
        ]));
        let rt = TuiRuntime {
            database: db.clone(),
            msg_tx: tx,

            input_paused: Arc::new(AtomicBool::new(false)),
            runner: mock.clone(),
        };
        let tasks = db.list_all().unwrap();
        let mut app = App::new(tasks, Duration::from_secs(300));

        rt.exec_jump_to_tmux(&mut app, "nonexistent-window".to_string());

        assert!(app.error_popup().is_some());
    }

    #[test]
    fn exec_cleanup_detaches_when_shared() {
        let (rt, mut app) = test_runtime();

        // Create two tasks sharing the same worktree
        rt.exec_insert_task(&mut app, "Task A".into(), "desc".into(), "/repo".into(), None, None);
        rt.exec_insert_task(&mut app, "Task B".into(), "desc".into(), "/repo".into(), None, None);

        let id_a = app.tasks()[0].id;
        let id_b = app.tasks()[1].id;

        let worktree = "/repo/.worktrees/1-task-a";
        rt.database.patch_task(id_a, &db::TaskPatch::new().status(models::TaskStatus::Running).worktree(Some(worktree)).tmux_window(Some("task-1"))).unwrap();
        rt.database.patch_task(id_b, &db::TaskPatch::new().status(models::TaskStatus::Running).worktree(Some(worktree)).tmux_window(Some("task-1"))).unwrap();

        // Cleanup task A — should detach only (worktree is shared)
        rt.exec_cleanup(id_a, "/repo".into(), worktree.into(), Some("task-1".into()));

        let task_a = rt.database.get_task(id_a).unwrap().unwrap();
        assert!(task_a.worktree.is_none(), "task A should be detached");
        assert!(task_a.tmux_window.is_none(), "task A tmux should be cleared");

        // Task B should still have the worktree
        let task_b = rt.database.get_task(id_b).unwrap().unwrap();
        assert_eq!(task_b.worktree.as_deref(), Some(worktree));
    }

    #[tokio::test]
    async fn exec_finish_happy_path_sends_complete() {
        let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().unwrap());
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![
            MockProcessRunner::fail(""),                   // symbolic-ref (no remote → fallback to "main")
            MockProcessRunner::ok_with_stdout(b"main\n"), // rev-parse HEAD
            MockProcessRunner::fail(""),                   // remote get-url (no remote)
            MockProcessRunner::ok(),                       // git rebase main (from worktree)
            MockProcessRunner::ok(),                       // git merge --ff-only (fast-forward)
            // Worktree is preserved; cleanup happens later during archive.
        ]));
        let rt = TuiRuntime {
            database: db.clone(),
            msg_tx: tx,

            input_paused: Arc::new(AtomicBool::new(false)),
            runner: mock,
        };

        let task = db
            .create_task_returning("Test", "desc", "/repo", None, models::TaskStatus::Done)
            .unwrap();
        let id = task.id;

        rt.exec_finish(
            id,
            "/repo".into(),
            "1-test".into(),
            "/repo/.worktrees/1-test".into(),
            None,
        );

        let msg = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(msg, Message::FinishComplete(tid) if tid == id),
            "Expected FinishComplete, got: {msg:?}"
        );
    }

    #[tokio::test]
    async fn exec_finish_conflict_sends_failed() {
        use crate::process::exit_fail;
        use std::process::Output;

        let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().unwrap());
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![
            MockProcessRunner::fail(""),                   // symbolic-ref (no remote → fallback to "main")
            MockProcessRunner::ok_with_stdout(b"main\n"), // rev-parse HEAD
            MockProcessRunner::fail(""),                   // remote get-url (no remote)
            Ok(Output {
                status: exit_fail(),
                stdout: b"".to_vec(),
                stderr: b"CONFLICT (content): Merge conflict in file.rs\nerror: could not apply abc1234\n".to_vec(),
            }),
            MockProcessRunner::ok(), // git rebase --abort
        ]));
        let rt = TuiRuntime {
            database: db.clone(),
            msg_tx: tx,

            input_paused: Arc::new(AtomicBool::new(false)),
            runner: mock,
        };

        let task = db
            .create_task_returning("Test", "desc", "/repo", None, models::TaskStatus::Done)
            .unwrap();
        let id = task.id;

        rt.exec_finish(
            id,
            "/repo".into(),
            "1-test".into(),
            "/repo/.worktrees/1-test".into(),
            None,
        );

        let msg = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        let Message::FinishFailed { id: tid, is_conflict, .. } = msg else {
            panic!("Expected FinishFailed, got: {msg:?}");
        };
        assert_eq!(tid, id);
        assert!(is_conflict, "Expected is_conflict=true");
    }

    #[tokio::test]
    async fn exec_dispatch_epic_creates_planning_subtask() {
        let (rt, mut app) = test_runtime();

        // Create an epic in the DB
        let epic = rt.database.create_epic("Auth redesign", "Rework login", "/repo").unwrap();

        rt.exec_dispatch_epic(&mut app, epic.clone());

        // Planning subtask was created in DB and added to app
        assert_eq!(app.tasks().len(), 1);
        let task = &app.tasks()[0];
        assert_eq!(task.title, "Plan: Auth redesign");
        assert_eq!(task.epic_id, Some(epic.id));
        assert_eq!(task.repo_path, "/repo");
        assert_eq!(task.status, models::TaskStatus::Backlog);

        // Verify description contains epic info
        assert!(task.description.contains("Auth redesign"));
        assert!(task.description.contains("Rework login"));

        // Verify the task is also in the DB
        let db_tasks = rt.database.list_all().unwrap();
        assert_eq!(db_tasks.len(), 1);
        assert_eq!(db_tasks[0].title, "Plan: Auth redesign");
    }

    #[tokio::test]
    async fn exec_finish_not_on_main_sends_failed() {
        let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().unwrap());
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/main\n"), // symbolic-ref (detect default branch)
            MockProcessRunner::ok_with_stdout(b"feature-branch\n"), // rev-parse HEAD (not main)
        ]));
        let rt = TuiRuntime {
            database: db.clone(),
            msg_tx: tx,

            input_paused: Arc::new(AtomicBool::new(false)),
            runner: mock,
        };

        let task = db
            .create_task_returning("Test", "desc", "/repo", None, models::TaskStatus::Done)
            .unwrap();
        let id = task.id;

        rt.exec_finish(
            id,
            "/repo".into(),
            "1-test".into(),
            "/repo/.worktrees/1-test".into(),
            None,
        );

        let msg = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        let Message::FinishFailed { id: tid, is_conflict, .. } = msg else {
            panic!("Expected FinishFailed, got: {msg:?}");
        };
        assert_eq!(tid, id);
        assert!(!is_conflict, "Expected is_conflict=false for not-on-main");
    }

    #[test]
    fn exec_send_notification_calls_notify_send() {
        let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().unwrap());
        let (tx, _rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![
            MockProcessRunner::ok(), // notify-send call
        ]));
        let rt = TuiRuntime {
            database: db,
            msg_tx: tx,

            input_paused: Arc::new(AtomicBool::new(false)),
            runner: mock.clone(),
        };
        rt.exec_send_notification("Task #1: Fix bug", "Ready for review", false);
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "notify-send");
        assert!(calls[0].1.contains(&"Task #1: Fix bug".to_string()));
        assert!(calls[0].1.contains(&"Ready for review".to_string()));
    }

    #[test]
    fn exec_send_notification_urgent_uses_critical() {
        let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().unwrap());
        let (tx, _rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![
            MockProcessRunner::ok(),
        ]));
        let rt = TuiRuntime {
            database: db,
            msg_tx: tx,

            input_paused: Arc::new(AtomicBool::new(false)),
            runner: mock.clone(),
        };
        rt.exec_send_notification("Task #1: Fix bug", "Agent needs your input", true);
        let calls = mock.recorded_calls();
        assert!(calls[0].1.contains(&"critical".to_string()));
    }

    #[test]
    fn exec_send_notification_failure_does_not_panic() {
        let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().unwrap());
        let (tx, _rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![
            MockProcessRunner::fail("command not found"),
        ]));
        let rt = TuiRuntime {
            database: db,
            msg_tx: tx,

            input_paused: Arc::new(AtomicBool::new(false)),
            runner: mock.clone(),
        };
        // Should not panic — just logs a warning
        rt.exec_send_notification("Task #1: Fix bug", "Ready for review", false);
    }

    #[test]
    fn exec_persist_setting_writes_to_db() {
        let (rt, mut app) = test_runtime();
        rt.exec_persist_setting(&mut app, "notifications_enabled", true);
        assert_eq!(
            rt.database.get_setting_bool("notifications_enabled").unwrap(),
            Some(true)
        );
    }

    #[tokio::test]
    async fn exec_create_pr_happy_path() {
        let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().unwrap());
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/main\n"), // symbolic-ref (detect default branch)
            MockProcessRunner::ok(),  // git push
            MockProcessRunner::ok_with_stdout(b"git@github.com:org/repo.git\n"),  // git remote get-url
            MockProcessRunner::ok_with_stdout(b"https://github.com/org/repo/pull/42\n"),  // gh pr create
        ]));
        let rt = TuiRuntime {
            database: db,
            msg_tx: tx,

            input_paused: Arc::new(AtomicBool::new(false)),
            runner: mock,
        };

        rt.exec_create_pr(
            TaskId(1),
            "/repo".to_string(),
            "1-task".to_string(),
            "Fix bug".to_string(),
            "Description".to_string(),
        );

        let msg = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap();
        assert!(matches!(msg, Message::PrCreated { id: TaskId(1), .. }));
    }

    #[tokio::test]
    async fn exec_create_pr_push_fails() {
        let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().unwrap());
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/main\n"), // symbolic-ref (detect default branch)
            MockProcessRunner::fail("fatal: no remote"),
        ]));
        let rt = TuiRuntime {
            database: db,
            msg_tx: tx,

            input_paused: Arc::new(AtomicBool::new(false)),
            runner: mock,
        };

        rt.exec_create_pr(
            TaskId(1),
            "/repo".to_string(),
            "1-task".to_string(),
            "Fix bug".to_string(),
            "Description".to_string(),
        );

        let msg = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap();
        assert!(matches!(msg, Message::PrFailed { .. }));
    }

    #[tokio::test]
    async fn exec_check_pr_status_sends_merged() {
        let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().unwrap());
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"MERGED\n"),  // gh pr view (no review decision line)
        ]));
        let rt = TuiRuntime {
            database: db,
            msg_tx: tx,

            input_paused: Arc::new(AtomicBool::new(false)),
            runner: mock,
        };

        rt.exec_check_pr_status(TaskId(1), "https://github.com/org/repo/pull/42".to_string());

        let msg = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap();
        assert!(matches!(msg, Message::PrMerged(TaskId(1))));
    }

    #[tokio::test]
    async fn exec_check_pr_status_open_sends_review_state() {
        let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().unwrap());
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mock = Arc::new(MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"OPEN\nAPPROVED\n"),  // gh pr view
        ]));
        let rt = TuiRuntime {
            database: db,
            msg_tx: tx,

            input_paused: Arc::new(AtomicBool::new(false)),
            runner: mock,
        };

        rt.exec_check_pr_status(TaskId(1), "https://github.com/org/repo/pull/42".to_string());

        let msg = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap();
        match msg {
            Message::PrReviewState { id, review_decision } => {
                assert_eq!(id, TaskId(1));
                assert_eq!(review_decision, Some(dispatch::PrReviewDecision::Approved));
            }
            other => panic!("Expected PrReviewState, got {:?}", other),
        }
    }

    #[test]
    fn exec_persist_string_setting_writes_to_db() {
        let (rt, mut app) = test_runtime();
        rt.exec_persist_string_setting(&mut app, "repo_filter", "/repo1\n/repo2");
        assert_eq!(
            rt.database.get_setting_string("repo_filter").unwrap(),
            Some("/repo1\n/repo2".to_string())
        );
    }

    #[test]
    fn startup_loads_cached_review_prs() {
        use crate::models::{CiStatus, ReviewDecision, ReviewPr};
        use chrono::Utc;

        let (rt, mut app) = test_runtime();

        // Pre-populate the database with a cached review PR
        let pr = ReviewPr {
            number: 42,
            title: "Fix bug".to_string(),
            author: "alice".to_string(),
            repo: "acme/app".to_string(),
            url: "https://github.com/acme/app/pull/42".to_string(),
            is_draft: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            additions: 10,
            deletions: 5,
            review_decision: ReviewDecision::ReviewRequired,
            labels: vec![],
            body: String::new(),
            head_ref: String::new(),
            ci_status: CiStatus::None,
            reviewers: vec![],
        };
        rt.database.save_review_prs(&[pr]).unwrap();

        // Simulate what run_tui does: load cached reviews
        let cached = rt.database.load_review_prs().unwrap();
        app.set_review_prs(cached);

        assert_eq!(app.review_prs().len(), 1);
        assert_eq!(app.review_prs()[0].number, 42);
    }
}
