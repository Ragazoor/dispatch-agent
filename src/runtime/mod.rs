use anyhow::Result;
use crossterm::{
    event::{self, DisableFocusChange, EnableFocusChange, Event},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::Terminal;
use std::collections::HashSet;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::interval;

/// Interval between TUI tick events (captures tmux output, checks staleness, etc.).
const TICK_INTERVAL: Duration = Duration::from_secs(2);

/// Minimum time between rendered frames (~60 fps cap).  Rapid key-repeat events
/// that arrive faster than this are processed but coalesced into a single render.
const MIN_FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// Sleep duration when the input thread is paused (e.g. while an editor is open).
const INPUT_PAUSE_SLEEP: Duration = Duration::from_millis(100);

/// Poll timeout for crossterm input events.
const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Name used for the TUI's tmux window (visible in tmux status bar).
const TUI_WINDOW_NAME: &str = "TUI";

/// Key (after the tmux prefix) that toggles a companion agent-tree pane's
/// visibility in whichever agent window it's pressed in. Matches
/// config.agent_tree_toggle_key in docs/specs/agent-tree.allium.
const AGENT_TREE_TOGGLE_KEY: &str = "e";

/// Command bound to [`AGENT_TREE_TOGGLE_KEY`]. `#{window_name}` is expanded
/// by tmux itself, before invoking the shell, to the name of whichever window
/// was focused when the key was pressed — so `dispatch toggle-agent-tree-pane`
/// is handed the target window without this process ever having to ask tmux
/// which window is focused. `-b` backgrounds the shell job so the keypress
/// doesn't block the tmux client.
const AGENT_TREE_TOGGLE_COMMAND: &str =
    "run-shell -b \"dispatch toggle-agent-tree-pane '#{window_name}'\"";

use crate::db::{SettingsStore, TaskRead};
use crate::models::TaskId;
use crate::process::{ProcessRunner, RealProcessRunner};
use crate::service::embeddings::EmbeddingService;
use crate::service::FieldUpdate;
use crate::tui::{self, App, Command, Message, RepoFilterMode};
use crate::{db, dispatch, mcp, models, tmux};

/// Convert `Option<String>` to `FieldUpdate`: `Some(v)` → `Set(v)`, `None` → `Clear`.
fn option_to_field_update(opt: Option<String>) -> FieldUpdate {
    match opt {
        Some(v) => FieldUpdate::Set(v),
        None => FieldUpdate::Clear,
    }
}

/// Fold `(repo_path, branch)` pairs (as returned by `list_all_base_branches`,
/// ordered by `last_used DESC`) into a per-repo history map, preserving
/// recency order within each repo's `Vec`.
pub(super) fn group_base_branches_by_repo(
    pairs: Vec<(String, String)>,
) -> std::collections::HashMap<String, Vec<String>> {
    let mut map: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    for (repo, branch) in pairs {
        map.entry(repo).or_default().push(branch);
    }
    map
}

/// Set up tmux for the TUI: rename the current window and bind Prefix+Space
/// to jump back to the TUI window.
fn setup_tmux_for_tui(runner: &dyn ProcessRunner) {
    // Use the pane ID of this process's own pane as the rename target. An empty-string
    // target resolves to the session's focused window, which renames the wrong window
    // when the user has a different window active at startup.
    let target = tmux::current_pane_id(runner).unwrap_or_default();
    let _ = tmux::rename_window(&target, TUI_WINDOW_NAME, runner);
    // `=` anchors the target to an exact name match. tmux otherwise resolves a
    // `-t <name>` by prefix, so a window whose name merely starts with
    // TUI_WINDOW_NAME could absorb this jump. Unlike every other window target
    // in the codebase this one cannot go through `tmux::window_target`: the
    // binding is a string tmux executes later, and a pane ID captured now would
    // be stale by then. The sigil does work for `select-window` specifically
    // (verified against tmux 3.5a; it does not for `send-keys` or
    // `set-option -w`). See `tmux::window_target` for the full picture.
    let _ = tmux::bind_key(
        "space",
        &format!("select-window -t ={TUI_WINDOW_NAME}"),
        runner,
    );
    let _ = tmux::bind_key(AGENT_TREE_TOGGLE_KEY, AGENT_TREE_TOGGLE_COMMAND, runner);
}

/// Tear down tmux TUI state: unbind the keys and restore the original window name.
fn teardown_tmux_for_tui(original_name: Option<&str>, runner: &dyn ProcessRunner) {
    let _ = tmux::unbind_key("space", runner);
    let _ = tmux::unbind_key(AGENT_TREE_TOGGLE_KEY, runner);
    if let Some(name) = original_name {
        let _ = tmux::rename_window(TUI_WINDOW_NAME, name, runner);
    }
}

/// Best-effort recreation of `~/.claude/dispatch-statusline.json`, the
/// dispatch-owned statusLine settings file every dispatch-spawned Claude
/// session is launched with via `--settings`. `dispatch setup` normally
/// writes this file (`setup::run_setup_in`); this is a safety net for a user
/// who pulled a branch introducing that dependency without re-running setup.
///
/// Reuses `setup::statusline`'s command-building logic rather than
/// duplicating the format string, which would let the two drift.
///
/// Synchronous (touches the filesystem) — callers must run this on a
/// blocking thread (`tokio::task::spawn_blocking`), never inline in an async
/// context.
fn ensure_statusline_settings_file(snapshot_path: &Path) -> Result<()> {
    let claude_dir = crate::setup::claude_dir()?;
    ensure_statusline_settings_file_in(&claude_dir, snapshot_path)
}

/// Injectable core of [`ensure_statusline_settings_file`]: takes the
/// `~/.claude` directory explicitly so tests can point it at a temp dir
/// instead of the real `$HOME`.
fn ensure_statusline_settings_file_in(claude_dir: &Path, snapshot_path: &Path) -> Result<()> {
    // No `create_dir_all` here: `write_settings_file` creates its parent, and
    // `discover_chain` treats a missing directory as "nothing to chain to".
    let settings_path = claude_dir.join(crate::setup::statusline::SETTINGS_FILE_NAME);
    let chain = crate::setup::statusline::discover_chain(claude_dir);
    crate::setup::statusline::write_settings_file(&settings_path, snapshot_path, chain.as_deref())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Bootstrap — composition root for TuiRuntime startup
// ---------------------------------------------------------------------------

/// Everything built by `TuiRuntime::bootstrap` that `run_tui` needs after
/// the composition root returns.
struct Bootstrap {
    app: App,
    runtime: TuiRuntime,
    mcp_notify_rx: mpsc::UnboundedReceiver<mcp::McpEvent>,
    msg_rx: mpsc::UnboundedReceiver<Message>,
}

// ---------------------------------------------------------------------------
// run_tui — entry point for the TUI mode
// ---------------------------------------------------------------------------

pub async fn run_tui(db_path: &Path, port: u16) -> Result<()> {
    if std::env::var("TMUX").is_err() {
        anyhow::bail!("dispatch tui must be run inside a tmux session (TMUX is not set)");
    }

    let Bootstrap {
        mut app,
        mut runtime,
        mut mcp_notify_rx,
        mut msg_rx,
    } = TuiRuntime::bootstrap(db_path, port).await?;

    // Set up terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableFocusChange)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Set up tmux keybinding: Prefix+Space → jump back to this window.
    // Best-effort: failures don't prevent the TUI from starting.
    let tmux_runner = runtime.runner.clone();
    let original_window_name = tmux::current_window_name(&*tmux_runner).ok();
    setup_tmux_for_tui(&*tmux_runner);

    // Create two channels:
    //    - key_rx: raw crossterm KeyEvents from the blocking poll thread
    //    - msg_rx: higher-level Messages (e.g. from dispatch results)
    let (key_tx, mut key_rx) = mpsc::unbounded_channel::<crossterm::event::KeyEvent>();

    // crossterm::event::poll/read are blocking; run them in a dedicated thread
    // so they don't block the async runtime. The thread can be paused (e.g. when
    // opening an external editor) via the input_paused flag.
    let input_paused = Arc::new(AtomicBool::new(false));
    let paused_clone = input_paused.clone();
    let resize_tx = runtime.msg_tx.clone();
    tokio::task::spawn_blocking(move || loop {
        if paused_clone.load(Ordering::Relaxed) {
            std::thread::sleep(INPUT_PAUSE_SLEEP);
            continue;
        }
        if event::poll(EVENT_POLL_INTERVAL).unwrap_or(false) {
            match event::read() {
                Ok(Event::Key(key)) if key_tx.send(key).is_err() => break,
                Ok(Event::Key(_)) => {}
                Ok(Event::Resize(..)) => {
                    let _ = resize_tx.send(Message::System(
                        crate::tui::messages::SystemMessage::TerminalResized,
                    ));
                }
                Ok(Event::FocusGained) => {
                    let _ = resize_tx.send(Message::System(
                        crate::tui::messages::SystemMessage::FocusChanged(true),
                    ));
                }
                Ok(Event::FocusLost) => {
                    let _ = resize_tx.send(Message::System(
                        crate::tui::messages::SystemMessage::FocusChanged(false),
                    ));
                }
                _ => {}
            }
        }
    });

    // Tick interval (2 seconds)
    let mut tick_interval = interval(TICK_INTERVAL);

    tracing::info!(port, db = %db_path.display(), "TUI started, MCP server on port {port}");

    let result = run_loop(
        &mut app,
        &mut terminal,
        &mut key_rx,
        &mut msg_rx,
        &mut mcp_notify_rx,
        &mut tick_interval,
        &mut runtime,
    )
    .await;

    // Tear down tmux keybinding and restore the original window name.
    teardown_tmux_for_tui(original_window_name.as_deref(), &*tmux_runner);

    // Cleanup terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableFocusChange,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;

    result
}

// ---------------------------------------------------------------------------
// Embedding backfill — run at startup to embed learnings missing vectors
// ---------------------------------------------------------------------------

/// Backfills embeddings for any learnings that have no embedding stored.
///
/// Runs at startup in a background task. Failures are logged via `tracing::warn`
/// by the caller; this function propagates errors so the caller can decide.
pub(crate) async fn backfill_embeddings(
    db: Arc<dyn crate::db::LearningStore + Send + Sync>,
    emb_svc: Arc<EmbeddingService>,
) -> Result<()> {
    use crate::service::embeddings::{embed_text_for_learning, serialize_embedding};

    let missing = db.list_learnings_missing_embedding().await?;
    if missing.is_empty() {
        return Ok(());
    }
    tracing::info!("Backfilling embeddings for {} learnings", missing.len());
    let texts: Vec<String> = missing
        .iter()
        .map(|l| embed_text_for_learning(l.kind, &l.summary, &l.tags, l.detail.as_deref()))
        .collect();
    let embeddings = emb_svc.embed_batch(texts).await?;
    for (learning, emb_vec) in missing.iter().zip(embeddings.iter()) {
        let emb_bytes = serialize_embedding(emb_vec);
        db.patch_learning(
            learning.id,
            &crate::db::LearningPatch::new().embedding(&emb_bytes),
        )
        .await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// TuiRuntime — shared context for command execution
// ---------------------------------------------------------------------------

struct TuiRuntime {
    // Read-only DB handle: queries only. Task/epic mutations go through
    // `task_svc` / `epic_svc`, which own the `recalculate_epic_status` invariant
    // — calling a mutating method on `database` is a compile error. See the
    // mutation-boundary section of docs/conventions.md.
    database: Arc<dyn db::TaskReadStore>,
    /// Write-capable handle reserved for the feed subsystem (the manual
    /// `exec_trigger_epic_feed` path), which upserts tasks and recalculates epic
    /// status itself — exactly like `FeedRunner`. This is the one sanctioned
    /// direct-mutation handle on the runtime; general command handlers hold only
    /// the read-only `database` above. See the mutation-boundary section of
    /// docs/conventions.md. In test builds it also backs the `#[cfg(test)]`
    /// `db_write()` accessor used to seed fixtures.
    feed_db: Arc<dyn db::TaskStore>,
    task_svc: Arc<dyn crate::service::TaskServiceApi>,
    epic_svc: Arc<dyn crate::service::EpicServiceApi>,
    todo_svc: Arc<dyn crate::service::TodoServiceApi>,
    learning_svc: Arc<dyn crate::service::LearningServiceApi>,
    msg_tx: mpsc::UnboundedSender<Message>,
    runner: Arc<dyn ProcessRunner>,
    /// Holds the in-flight pop-out editor session, if any. `None` means no
    /// editor is currently open. We enforce "at most one editor at a time"
    /// by refusing to start a new one while this slot is populated.
    editor_session: Arc<std::sync::Mutex<Option<editor::EditorSession>>>,
    feed_runner: Option<crate::feed::FeedRunner>,
    /// Taken and started alongside `feed_runner`, for the same reason it is an
    /// `Option`: a `TuiRuntime` built directly by a test must not spawn a
    /// background loop.
    scheduler_runner: Option<crate::scheduler::SchedulerRunner>,
    /// Fires the `FeedRunner`'s feed-command cache invalidation. Cloned from
    /// `feed_runner.epic_invalidate_tx()` at construction so both mutation-
    /// carrying MCP events (`Refresh` and `EpicChanged`) can reset the cache
    /// through `invalidate_feed_cache()` — keeping the runner from stranding a
    /// freshly-enabled feed behind `any_feed_cmds == Some(false)`.
    feed_invalidate_tx: Option<tokio::sync::watch::Sender<()>>,
    /// Per-epic feed-cycle claims, shared with `feed_runner` so a manual "r"
    /// refresh and an auto-poll tick serialise against each other
    /// (feeds.allium: SerialisedFeedCycle).
    ///
    /// This MUST be the same `Arc` the `FeedRunner` holds. A separate registry
    /// type-checks and compiles, and silently serialises nothing — always take
    /// it from `FeedRunner::sync_guard()`, never construct one here.
    feed_sync_guard: std::sync::Arc<crate::feed::FeedSyncGuard>,
    /// Shared embedding service for RAG-based learning injection and editor updates.
    emb_svc: Arc<EmbeddingService>,
    /// Snapshot of `total_changes()` after the last tick-driven full refresh.
    /// `-1` means no snapshot has been taken yet (always refresh on the first tick).
    /// Stored as an `AtomicI64` so it can be updated through the shared `&self`
    /// reference used in `execute_commands`.
    last_change_count: AtomicI64,
    /// Path to the budget snapshot file (`<data_dir>/rate-limits.json`), written
    /// by the statusLine hook of every dispatch-spawned Claude session. Read off
    /// the event loop by `exec_refresh_budget`. See docs/specs/dispatch.allium:
    /// TokenBudgetIndicator.
    budget_snapshot_path: std::path::PathBuf,
}

mod agents;
mod budget;
mod commands;
mod editor;
mod epics;
mod learnings;
mod pr;
mod repo_sync;
mod settings;
mod split;
mod tasks;
#[cfg(test)]
mod tests;
mod todos;

impl TuiRuntime {
    fn db_error(action: &str, e: impl std::fmt::Display) -> String {
        format!("DB error {action}: {e}")
    }

    /// Test-only write handle for seeding DB fixtures directly. Backed by the
    /// feed subsystem's write handle; not available in production builds, so
    /// command handlers keep going through the services.
    #[cfg(test)]
    pub(super) fn db_write(&self) -> &Arc<dyn db::TaskStore> {
        &self.feed_db
    }

    fn send_system_error(&self, msg: impl Into<String>) {
        let _ = self
            .msg_tx
            .send(Message::System(crate::tui::messages::SystemMessage::Error(
                msg.into(),
            )));
    }

    /// Build a fully-initialised runtime and its companion `App` from a database
    /// path and MCP port. Encapsulates all startup I/O — database open, embedding
    /// model load, MCP server spawn, and settings hydration — so `run_tui` reads
    /// as a sequence of named steps rather than an inline setup blob.
    ///
    /// The `#[cfg(test)]` / `#[cfg(not(test))]` embedding-service split lives
    /// here so call sites don't branch on `cfg`.
    async fn bootstrap(db_path: &Path, port: u16) -> Result<Bootstrap> {
        // Open database and load initial tasks.
        let database = Arc::new(db::Database::open(db_path).await?);
        let tasks = database.list_all().await?;

        // Provision the managed feed-epic tree from the reviews/CVE config.
        // Idempotent and best-effort: a failure here must not block startup.
        if let Err(e) = crate::service::provision_managed_feeds_from_settings(&*database).await {
            tracing::warn!("Managed feed provisioning failed: {e:#}");
        }

        // Initialise the embedding model (blocks until loaded; may download on first run).
        // Tests bypass run_tui entirely and construct TuiRuntime directly, so
        // the non-test branch is only reached in production.
        #[cfg(not(test))]
        let emb_svc = {
            eprintln!("Loading embedding model...");
            tokio::task::spawn_blocking(EmbeddingService::new)
                .await
                .map_err(|e| anyhow::anyhow!("Embedding thread panicked: {e}"))?
                .map_err(|e| {
                    anyhow::anyhow!(
                        "Failed to initialise embedding model: {e}\n\
                         Clear cache with: rm -rf ~/.cache/huggingface/hub/"
                    )
                })?
        };
        #[cfg(test)]
        let emb_svc = EmbeddingService::new_noop();

        // Backfill embeddings for any learnings that were created before the model
        // was available. Fire-and-forget: partial work is retried on next startup.
        tokio::spawn({
            let db = database.clone();
            let emb = emb_svc.clone();
            async move {
                if let Err(e) = backfill_embeddings(db, emb).await {
                    tracing::warn!("Embedding backfill failed: {e}");
                }
            }
        });

        // Spawn MCP server with notification channel.
        let runner: Arc<dyn ProcessRunner> = Arc::new(RealProcessRunner);
        let data_dir = db_path
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf();
        let budget_snapshot_path = data_dir
            .clone()
            .join(crate::setup::statusline::RATE_LIMITS_FILE_NAME);

        // Best-effort: recreate ~/.claude/dispatch-statusline.json if it's
        // missing. Every dispatch-spawned Claude session is launched with
        // `--settings ~/.claude/dispatch-statusline.json` (the spawn constant
        // in src/dispatch/prompts.rs); `claude` refuses to start at all if
        // that file doesn't exist. Normally `dispatch setup` writes it, but a
        // user who pulls a branch that added this dependency without
        // re-running setup would otherwise get a dead pane on every dispatch.
        // Must never block or fail startup — see docs/reference.md
        // Troubleshooting for the user-facing recovery path.
        {
            let snapshot_path = budget_snapshot_path.clone();
            match tokio::task::spawn_blocking(move || {
                ensure_statusline_settings_file(&snapshot_path)
            })
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::warn!("Failed to ensure statusline settings file: {e:#}"),
                Err(e) => tracing::warn!("Statusline settings bootstrap task panicked: {e}"),
            }
        }

        let (mcp_notify_tx, mcp_notify_rx) = mpsc::unbounded_channel::<mcp::McpEvent>();
        let feed_notify_tx = mcp_notify_tx.clone();
        let mcp_deps = mcp::McpDeps {
            db: database.clone(),
            runner: runner.clone(),
            embedding_service: emb_svc.clone(),
            data_dir,
        };
        tokio::spawn(async move {
            if let Err(e) = mcp::serve(mcp_deps, port, mcp_notify_tx).await {
                eprintln!("MCP server error: {e}");
            }
        });

        // Create App and hydrate all persisted settings.
        let mut app = App::new(tasks);
        let (paths, base_branch_pairs) = tokio::join!(
            database.list_repo_paths(),
            database.list_all_base_branches()
        );
        app.update(Message::RepoPathsUpdated(paths.unwrap_or_default()));
        app.update(Message::BaseBranchesUpdated(group_base_branches_by_repo(
            base_branch_pairs.unwrap_or_default(),
        )));
        load_notifications_pref(&*database, &mut app).await;
        load_repo_filter(&*database, &mut app).await;
        load_main_session(&*database, &mut app).await;
        for msg in [
            load_filter_presets(&*database, &mut app).await,
            apply_tmux_focus_warning(&*runner),
        ]
        .into_iter()
        .flatten()
        {
            app.update(msg);
        }

        // Build TuiRuntime.
        let (msg_tx, msg_rx) = mpsc::unbounded_channel::<Message>();
        let scheduler_runner = crate::scheduler::SchedulerRunner::new(
            database.clone(),
            feed_notify_tx.clone(),
            runner.clone(),
        );
        let feed_runner =
            crate::feed::FeedRunner::new(database.clone(), feed_notify_tx, runner.clone());
        let feed_invalidate_tx = Some(feed_runner.epic_invalidate_tx());
        let feed_sync_guard = feed_runner.sync_guard();
        let runtime = TuiRuntime {
            task_svc: Arc::new(crate::service::TaskService::new(
                database.clone(),
                runner.clone(),
            )),
            epic_svc: Arc::new(crate::service::EpicService::new(database.clone())),
            todo_svc: Arc::new(crate::service::TodoService::new(database.clone())),
            learning_svc: Arc::new(crate::service::LearningService::new(
                database.clone(),
                emb_svc.clone(),
            )),
            feed_runner: Some(feed_runner),
            scheduler_runner: Some(scheduler_runner),
            feed_invalidate_tx,
            feed_sync_guard,
            feed_db: database.clone(),
            database,
            msg_tx,
            runner,
            editor_session: Arc::new(std::sync::Mutex::new(None)),
            emb_svc,
            last_change_count: AtomicI64::new(-1),
            budget_snapshot_path,
        };

        // Load initial todo open-count so the board footer shows it immediately.
        runtime.exec_load_todo_count(&mut app).await;

        // RefreshRepoSyncStateOnStartup: the only genuinely new network traffic
        // this feature introduces — one fetch per saved repo path. Fire-and-forget,
        // so a slow or offline network never delays startup.
        let saved_repo_paths = app.repo_paths().to_vec();
        drop(runtime.exec_refresh_all_repo_sync(&saved_repo_paths));

        Ok(Bootstrap {
            app,
            runtime,
            mcp_notify_rx,
            msg_rx,
        })
    }

    /// Invalidate the `FeedRunner`'s `any_feed_cmds` cache so its next tick
    /// re-queries for feed commands. Call after any managed-feed mutation that
    /// may have enabled the first feed on a previously feed-less instance —
    /// otherwise the runner short-circuits on a stale `Some(false)` and never
    /// starts polling until an unrelated event or a restart. Best-effort: a
    /// dropped receiver (no running runner) is a no-op.
    fn invalidate_feed_cache(&self) {
        if let Some(tx) = &self.feed_invalidate_tx {
            let _ = tx.send(());
        }
    }

    async fn create_task(
        &self,
        app: &mut App,
        params: crate::service::CreateTaskParams,
    ) -> Option<models::Task> {
        match self.task_svc.create_task_returning(params).await {
            Ok(task) => Some(task),
            Err(e) => {
                app.update(Message::System(crate::tui::messages::SystemMessage::Error(
                    Self::db_error("creating task", e),
                )));
                None
            }
        }
    }
}

// ---------------------------------------------------------------------------
// run_loop — select over key events, async messages, and tick timer
// ---------------------------------------------------------------------------

/// One input the TUI event loop reacts to, drawn from any of its four sources
/// (keys, async messages, MCP notifications, the periodic tick). Naming the
/// event explicitly lets the loop body — `apply_loop_event` — be unit-tested
/// without a running `select!` or a real terminal.
///
/// `Message` is the largest variant, but this enum is a short-lived stack value
/// produced and consumed once per loop iteration — and `Message` is already
/// passed by value throughout the TUI (it flows through an
/// `UnboundedReceiver<Message>` unboxed). Boxing it here would add an allocation
/// per event for no benefit, so the size skew is accepted.
#[cfg_attr(test, derive(Debug))]
#[allow(clippy::large_enum_variant)]
enum LoopEvent {
    Key(crossterm::event::KeyEvent),
    Message(Message),
    Mcp(mcp::McpEvent),
    Tick,
}

/// Await the next event from any input source. The tick arm is always enabled,
/// so this never resolves on an all-channels-closed condition — the loop exits
/// via `App::should_quit`, not channel closure.
async fn next_loop_event(
    key_rx: &mut mpsc::UnboundedReceiver<crossterm::event::KeyEvent>,
    msg_rx: &mut mpsc::UnboundedReceiver<Message>,
    mcp_notify_rx: &mut mpsc::UnboundedReceiver<mcp::McpEvent>,
    tick_interval: &mut tokio::time::Interval,
) -> LoopEvent {
    tokio::select! {
        // Key events from the blocking poll thread.
        Some(key) = key_rx.recv() => LoopEvent::Key(key),
        // Async messages (e.g., from dispatch results).
        Some(msg) = msg_rx.recv() => LoopEvent::Message(msg),
        // MCP event notification.
        Some(event) = mcp_notify_rx.recv() => LoopEvent::Mcp(event),
        // Periodic tick for tmux capture and feed polling.
        _ = tick_interval.tick() => LoopEvent::Tick,
    }
}

/// Apply one loop event to `app`, returning the commands it produced. Mirrors
/// the per-arm `dirty` bookkeeping and MCP-event side effects (refresh spawns,
/// feed-cache invalidation) of the original `select!` body, kept as a separate
/// function so the routing is directly testable.
fn apply_loop_event(app: &mut App, event: LoopEvent, rt: &TuiRuntime) -> Vec<Command> {
    match event {
        // handle_key sets app.dirty unconditionally, same as the Message/Mcp
        // arms below — see the render-dirty-flag section of docs/architecture.md.
        LoopEvent::Key(key) => app.handle_key(key),
        LoopEvent::Message(msg) => {
            // Async messages typically carry visible state changes.
            app.dirty = true;
            app.update(msg)
        }
        LoopEvent::Mcp(event) => {
            // Spawn DB work so this never blocks key-event processing. Results
            // arrive back via msg_rx and are applied on the next iteration.
            app.dirty = true;
            match event {
                mcp::McpEvent::Refresh => {
                    // A broad refresh may follow a managed-feed config save
                    // (set_managed_feed_config) that enabled a feed on a
                    // previously feed-less instance. Invalidate the FeedRunner
                    // cache so the next tick re-queries for feed commands and
                    // starts polling the freshly-provisioned epics rather than
                    // short-circuiting on a stale any_feed_cmds == Some(false).
                    rt.invalidate_feed_cache();
                    drop(rt.spawn_refresh_from_db());
                    vec![]
                }
                mcp::McpEvent::TaskChanged(task_id) => {
                    drop(rt.spawn_refresh_task(task_id));
                    vec![]
                }
                mcp::McpEvent::EpicChanged(epic_id) => {
                    // Invalidate the FeedRunner's cache so the next tick re-queries
                    // for feed commands (e.g. a newly added feed_command becomes visible).
                    rt.invalidate_feed_cache();
                    drop(rt.spawn_refresh_epic(epic_id));
                    vec![]
                }
                mcp::McpEvent::BranchRebased { repo_path } => {
                    // A rebase wrap-up pulled origin/<base> and fast-forwarded
                    // local <base>, so the refs are current and no fetch is
                    // needed. An unresolved repository measures nothing.
                    if !repo_path.is_empty() {
                        drop(rt.exec_refresh_repo_sync(repo_path, false));
                    }
                    vec![]
                }
                mcp::McpEvent::AgentLaunched { repo_path } => {
                    // RefreshRepoSyncStateAfterDispatch: provisioning the agent's
                    // worktree already fetched origin/<base>, so this is a local
                    // ref read at no network cost. The board's own dispatch takes
                    // the same refresh through a command; these are the off-board
                    // launches (dispatch_task, epic auto-dispatch chaining).
                    drop(rt.exec_refresh_repo_sync(repo_path, false));
                    vec![]
                }
                mcp::McpEvent::AutoDispatchFailed {
                    task_id,
                    epic_id,
                    reason,
                } => {
                    // No refresh is spawned here: the chain sends TaskChanged
                    // for the released subtask right behind this, so reloading
                    // the row is already covered.
                    app.update(Message::Task(
                        crate::tui::messages::TaskMessage::AutoDispatchFailed {
                            task_id,
                            epic_id,
                            reason,
                        },
                    ))
                }
            }
        }
        // Handlers set app.dirty themselves when they detect visible changes.
        LoopEvent::Tick => app.update(Message::System(crate::tui::messages::SystemMessage::Tick)),
    }
}

/// Commands executed once, before the event loop's first iteration.
///
/// The list exists so "what runs at startup" is one value a test can read,
/// rather than a sequence of inline calls in `run_loop`. Everything here must be
/// something the tick loop would do anyway, just sooner: startup priming, never
/// startup-only behaviour.
fn startup_commands() -> Vec<Command> {
    // Read the budget snapshot now instead of waiting out the first
    // BUDGET_POLL_TICKS, so a snapshot already on disk shows on the first frame.
    vec![Command::Budget(
        crate::tui::commands::BudgetCommand::Refresh,
    )]
}

async fn run_loop<B: Backend>(
    app: &mut App,
    terminal: &mut Terminal<B>,
    key_rx: &mut mpsc::UnboundedReceiver<crossterm::event::KeyEvent>,
    msg_rx: &mut mpsc::UnboundedReceiver<Message>,
    mcp_notify_rx: &mut mpsc::UnboundedReceiver<mcp::McpEvent>,
    tick_interval: &mut tokio::time::Interval,
    rt: &mut TuiRuntime,
) -> Result<()> {
    // Here (not in TuiRuntime::new) so tests that construct TuiRuntime directly
    // don't accidentally spawn background tasks. The invalidation sender is held
    // on `rt.feed_invalidate_tx` (cloned at construction), so it survives the
    // runner being moved into its background task here.
    if let Some(feed_runner) = rt.feed_runner.take() {
        feed_runner.start();
    }
    if let Some(scheduler_runner) = rt.scheduler_runner.take() {
        scheduler_runner.start();
    }

    execute_commands(app, startup_commands(), rt, terminal, key_rx).await?;

    let mut last_render = std::time::Instant::now() - MIN_FRAME_INTERVAL; // allow first frame

    loop {
        // Redraw only when state changed since the last frame AND the frame interval has elapsed.
        // frame_ready coalesces rapid key-repeat events (holding j) into at most ~60 renders/s.
        if frame_ready(last_render.elapsed(), app.dirty) {
            terminal.draw(|frame| tui::ui::render(frame, app))?;
            app.dirty = false;
            last_render = std::time::Instant::now();
        }

        if app.should_quit() {
            break;
        }

        let event = next_loop_event(key_rx, msg_rx, mcp_notify_rx, tick_interval).await;
        let commands = apply_loop_event(app, event, rt);

        execute_commands(app, commands, rt, terminal, key_rx).await?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// execute_commands — run side effects for each Command
// ---------------------------------------------------------------------------

async fn execute_commands<B: Backend>(
    app: &mut App,
    cmds: Vec<Command>,
    rt: &TuiRuntime,
    _terminal: &mut Terminal<B>,
    _key_rx: &mut mpsc::UnboundedReceiver<crossterm::event::KeyEvent>,
) -> Result<()> {
    let mut queue = std::collections::VecDeque::from(cmds);
    while let Some(command) = queue.pop_front() {
        let extra = commands::dispatch(command, app, rt).await;
        queue.extend(extra);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// init load helpers — extracted from run_tui's startup block
// ---------------------------------------------------------------------------

async fn load_main_session(db: &dyn db::SettingsStore, app: &mut App) {
    // Only the configured directory is persisted. The window identity is not
    // stored — `:` derives liveness via a live tmux check on the fixed window
    // name (see `exec_open_main_session`).
    if let Some(dir) = db
        .get_setting_string("main_session.dir")
        .await
        .ok()
        .flatten()
    {
        if !dir.is_empty() {
            app.set_main_session_dir(Some(dir));
        }
    }
}

async fn load_notifications_pref(db: &dyn db::SettingsStore, app: &mut App) {
    let enabled = db
        .get_setting_bool("notifications_enabled")
        .await
        .unwrap_or(None)
        .unwrap_or(false);
    app.set_notifications_enabled(enabled);
}

async fn load_repo_filter(db: &dyn db::SettingsStore, app: &mut App) {
    if let Ok(Some(val)) = db.get_setting_string("repo_filter").await {
        if let Ok(paths) = serde_json::from_str::<Vec<String>>(&val) {
            app.set_repo_filter(paths.into_iter().collect());
        }
    }
    if let Ok(Some(mode_str)) = db.get_setting_string("repo_filter_mode").await {
        if let Ok(mode) = mode_str.parse::<RepoFilterMode>() {
            app.set_repo_filter_mode(mode);
        }
    }
}

async fn load_filter_presets(db: &dyn db::SettingsStore, app: &mut App) -> Option<Message> {
    match db.list_filter_presets().await {
        Ok(raw) => {
            let _ = app.update(Message::RepoFilter(
                crate::tui::messages::RepoFilterMessage::PresetsLoaded(parse_raw_presets(
                    raw, None,
                )),
            ));
            None
        }
        Err(e) => Some(Message::System(
            crate::tui::messages::SystemMessage::StatusInfo(format!(
                "Failed to load filter presets: {e}"
            )),
        )),
    }
}

fn apply_tmux_focus_warning(runner: &dyn ProcessRunner) -> Option<Message> {
    if !crate::tmux::focus_events_enabled(runner) {
        Some(Message::System(crate::tui::messages::SystemMessage::StatusInfo(
            "tmux focus-events is off \u{2014} split-view focus indicator won't work. Run: tmux set -g focus-events on".to_string(),
        )))
    } else {
        None
    }
}

/// Convert raw DB preset tuples into typed presets.
///
/// When `known_repos` is `Some`, each preset's paths are filtered to only
/// include paths present in the set. When `None`, all paths are kept.
fn parse_raw_presets(
    raw: Vec<(String, Vec<String>, String)>,
    known_repos: Option<&HashSet<String>>,
) -> Vec<(String, HashSet<String>, RepoFilterMode)> {
    raw.into_iter()
        .map(|(name, paths, mode_str)| {
            let set: HashSet<String> = if let Some(known) = known_repos {
                paths.into_iter().filter(|p| known.contains(p)).collect()
            } else {
                paths.into_iter().collect()
            };
            let mode = mode_str.parse().unwrap_or_default();
            (name, set, mode)
        })
        .collect()
}

/// Returns `true` when the render loop should draw a new frame.
///
/// Both conditions must hold: the app state changed (`dirty`) *and* enough
/// time has elapsed since the last render (`elapsed >= MIN_FRAME_INTERVAL`).
/// The interval coalesces rapid key-repeat events (≥30/s) into at most
/// one render per 16 ms (~60 fps) without adding perceptible latency to
/// single keypresses.
pub(crate) fn frame_ready(elapsed_since_render: Duration, dirty: bool) -> bool {
    dirty && elapsed_since_render >= MIN_FRAME_INTERVAL
}
