//! Pop-out editor: spawn `$EDITOR` in a separate tmux window while the TUI
//! keeps running, then apply the edit when the editor window closes.

use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tempfile::Builder as TempfileBuilder;

use super::{TuiRuntime, TUI_WINDOW_NAME};
use crate::editor::{
    apply_epic_editor_fields, apply_task_editor_fields, format_description_for_editor,
    format_editor_content, format_epic_for_editor, parse_editor_content, parse_epic_editor_output,
    TaskEditApplied,
};
use crate::process::ProcessRunner;
#[cfg(test)]
use crate::service::embeddings::EmbeddingService;
use crate::service::{UpdateEpicParams, UpdateTaskParams};
use crate::tui::{App, Command, EditKind, EditorOutcome, Message};
use crate::{models, tmux};

/// Interval between `has_window` polls while waiting for the editor to exit.
const POLL_INTERVAL: Duration = Duration::from_millis(300);

/// Consecutive tmux query failures [`window_alive_with_bounded_retry`]
/// tolerates before giving up and reporting "not alive".
///
/// `tmux::has_window_or_assume_present` (query failure -> alive) is the right
/// default for the other liveness call sites (`main_session_window_alive`,
/// `exec_check_window`) because those are periodic re-checks — a false
/// "alive" there just delays detection by one tick. `watch_editor`'s loop
/// below has no other exit condition, so treating a *permanently* broken
/// tmux as alive forever would hang it indefinitely; bounding the retries
/// keeps the "don't overreact to one blip" behaviour while still
/// terminating on a sustained failure.
const MAX_CONSECUTIVE_QUERY_FAILURES: u32 = 5;

/// Liveness check for [`watch_editor`]'s poll loop. On a successful query,
/// reports the real state and resets `consecutive_failures`. On a query
/// error, assumes "still alive" for up to [`MAX_CONSECUTIVE_QUERY_FAILURES`]
/// in a row, then gives up and reports "not alive".
fn window_alive_with_bounded_retry(
    window: &str,
    runner: &dyn ProcessRunner,
    consecutive_failures: &mut u32,
) -> bool {
    match tmux::has_window(window, runner) {
        Ok(alive) => {
            *consecutive_failures = 0;
            alive
        }
        Err(_) => {
            *consecutive_failures += 1;
            *consecutive_failures < MAX_CONSECUTIVE_QUERY_FAILURES
        }
    }
}

/// Message shown when a second editor is requested while one is already open.
pub const EDITOR_ALREADY_OPEN_MSG: &str = "Editor already open — close it first";

/// Tracks a live editor session.
///
/// The tempfile is kept alive here so that the watcher task can read it after
/// the editor closes. Dropping this struct deletes the tempfile and
/// best-effort kills the tmux window, covering TUI shutdown while an editor
/// is still open.
pub struct EditorSession {
    pub window_name: String,
    /// The temp path owning the file on disk. `Some` until the watcher task
    /// reads and consumes it.
    pub temp_path: Option<PathBuf>,
    /// Process runner used by `Drop` to best-effort kill the tmux window.
    /// `None` in tests that construct sessions without a real runner.
    cleanup_runner: Option<Arc<dyn ProcessRunner>>,
}

#[cfg(test)]
impl EditorSession {
    /// Test-only constructor for an *occupied* session slot: no tempfile and no
    /// cleanup runner, so `Drop` is a no-op. Lets tests outside this module
    /// (notably `runtime::tests`) exercise the "one editor at a time" guard in
    /// `exec_pop_out_editor` without spawning a real editor window.
    pub(super) fn occupied_for_test(window_name: &str) -> Self {
        Self {
            window_name: window_name.to_string(),
            temp_path: None,
            cleanup_runner: None,
        }
    }
}

impl Drop for EditorSession {
    fn drop(&mut self) {
        if let Some(path) = self.temp_path.take() {
            let _ = std::fs::remove_file(&path);
        }
        if let Some(runner) = self.cleanup_runner.take() {
            let _ = tmux::kill_window(&self.window_name, &*runner);
        }
    }
}

/// Poll `is_window_alive` until it returns `false`, then read the tempfile.
/// Returns `Cancelled` if the read fails (tempfile was deleted or unreadable),
/// otherwise `Saved(content)`.
///
/// Extracted as a pure function so the polling behaviour is testable without
/// any tmux/tokio involvement.
pub fn watch_editor<FA, FS, FR>(
    mut is_window_alive: FA,
    sleep: FS,
    read_tempfile: FR,
) -> EditorOutcome
where
    FA: FnMut() -> bool,
    FS: Fn(),
    FR: FnOnce() -> io::Result<String>,
{
    while is_window_alive() {
        sleep();
    }
    match read_tempfile() {
        Ok(text) => EditorOutcome::Saved(text),
        Err(_) => EditorOutcome::Cancelled,
    }
}

/// Build the initial content and tempfile prefix for a given [`EditKind`].
///
/// For `GithubQueries` / `SecurityQueries` variants this reads from the
/// database settings layer. Returns `(prefix, content)`.
fn initial_content_for(kind: &EditKind) -> (String, String) {
    match kind {
        EditKind::TaskEdit(task) => {
            let prefix = format!("task-{}-", task.id.0);
            let content = format_editor_content(task);
            (prefix, content)
        }
        EditKind::EpicEdit(epic) => {
            let prefix = format!("epic-{}-", epic.id.0);
            let content = format_epic_for_editor(epic);
            (prefix, content)
        }
        EditKind::Description { .. } => (
            "description-".to_string(),
            format_description_for_editor(""),
        ),
    }
}

/// Surface a pop-out editor failure as a status error and let the caller
/// `return`. Funnels the several early-return error paths in
/// `exec_pop_out_editor` through one place instead of repeating the
/// `app.update(Message::System(...))` boilerplate at each site.
fn emit_pop_out_error(app: &mut App, message: String) {
    app.update(Message::System(crate::tui::messages::SystemMessage::Error(
        message,
    )));
}

/// Generate a unique tmux window name for a new editor session.
fn new_window_name() -> String {
    // Nanoseconds since the process began are plenty unique for a single
    // dispatch run; collisions would require the same nanosecond tick.
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("dispatch-edit-{nanos}")
}

impl TuiRuntime {
    /// Entry point for `EditorCommand::PopOut`. Opens the editor in a new
    /// tmux window, spawns a watcher task, and emits an
    /// [`EditorMessage::Result`] when the editor exits.
    pub(super) fn exec_pop_out_editor(&self, app: &mut App, kind: EditKind) {
        // Enforce "one editor at a time".
        let mut guard = match self.editor_session.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guard.is_some() {
            app.update(Message::System(
                crate::tui::messages::SystemMessage::StatusInfo(
                    EDITOR_ALREADY_OPEN_MSG.to_string(),
                ),
            ));
            return;
        }

        let (prefix, content) = initial_content_for(&kind);

        // Write tempfile.
        let mut tmp = match TempfileBuilder::new()
            .prefix(&prefix)
            .suffix(".md")
            .tempfile()
        {
            Ok(f) => f,
            Err(e) => {
                emit_pop_out_error(app, Self::db_error("creating editor tempfile", e));
                return;
            }
        };
        if let Err(e) = std::io::Write::write_all(tmp.as_file_mut(), content.as_bytes()) {
            emit_pop_out_error(app, Self::db_error("writing editor tempfile", e));
            return;
        }

        let (_file, temp_path) = match tmp.keep() {
            Ok(p) => p,
            Err(e) => {
                emit_pop_out_error(app, Self::db_error("persisting editor tempfile", e.error));
                return;
            }
        };

        let window_name = new_window_name();
        // Same resolution as the agent-tree editor pane — one `$EDITOR` means
        // one thing (docs/specs/core.allium: `editor_fallback`). Never empty,
        // so `new_window_running`'s empty-command guard is unreachable here.
        let editor = crate::editor::editor_from_env();
        let cwd = std::env::temp_dir();
        let cwd_str = cwd.to_string_lossy().into_owned();
        let temp_str = temp_path.to_string_lossy().into_owned();

        let mut command: Vec<&str> = editor.iter().map(String::as_str).collect();
        command.push(&temp_str);

        if let Err(e) = tmux::new_window_running(&window_name, &cwd_str, &command, &*self.runner) {
            let _ = std::fs::remove_file(&temp_path);
            emit_pop_out_error(app, format!("Failed to open editor window: {e}"));
            return;
        }

        // Best-effort: switch tmux focus to the editor window. Failing to
        // switch isn't fatal — the window still exists.
        let _ = tmux::select_window(&window_name, &*self.runner);

        *guard = Some(EditorSession {
            window_name: window_name.clone(),
            temp_path: Some(temp_path.clone()),
            cleanup_runner: Some(self.runner.clone()),
        });
        drop(guard);

        // Spawn the watcher on a blocking thread so it doesn't tie up the
        // async runtime.
        let runner = self.runner.clone();
        let msg_tx = self.msg_tx.clone();
        let session = self.editor_session.clone();
        let window = window_name;
        let path = temp_path;
        let kind_for_result = kind;
        tokio::task::spawn_blocking(move || {
            let mut consecutive_failures = 0;
            let outcome = watch_editor(
                || window_alive_with_bounded_retry(&window, &*runner, &mut consecutive_failures),
                || std::thread::sleep(POLL_INTERVAL),
                || std::fs::read_to_string(&path),
            );

            // Restore focus to the TUI window. Best-effort.
            let _ = tmux::select_window(TUI_WINDOW_NAME, &*runner);

            clear_session_slot(&session);
            // Clean up the tempfile explicitly now that we have the contents;
            // Drop on the session would also do it, but we want it gone before
            // the handler runs so retries don't pick up a stale file.
            let _ = std::fs::remove_file(&path);

            let _ = msg_tx.send(Message::Editor(
                crate::tui::messages::EditorMessage::Result {
                    kind: kind_for_result,
                    outcome,
                },
            ));
        });
    }

    /// Apply the editor result for the given [`EditKind`].
    pub(super) async fn exec_finalize_editor_result(
        &self,
        app: &mut App,
        kind: EditKind,
        outcome: EditorOutcome,
    ) -> Vec<Command> {
        match kind {
            EditKind::TaskEdit(task) => self.finalize_task_edit(app, *task, outcome).await,
            EditKind::EpicEdit(epic) => self.finalize_epic_edit(app, *epic, outcome).await,
            EditKind::Description { .. } => {
                tracing::warn!("FinalizeEditorResult received Description kind; ignoring");
                vec![]
            }
        }
    }

    async fn finalize_task_edit(
        &self,
        app: &mut App,
        task: models::Task,
        outcome: EditorOutcome,
    ) -> Vec<Command> {
        let Some(text) = saved_text(outcome) else {
            return vec![];
        };
        let mut fields = parse_editor_content(&text);
        let parse_errors = std::mem::take(&mut fields.errors);
        let applied = apply_task_editor_fields(&task, fields);
        emit_parse_errors(app, &parse_errors);

        let task_id = task.id;
        let prior_repo_path = task.repo_path.clone();

        // Single source of truth: destructure `TaskEditApplied` exhaustively
        // (no `..`) so adding an editable field is a compile error (E0027)
        // here rather than a silently-dropped field. The `UpdateTaskParams`
        // patch and the in-memory `TaskEdit` event are both derived from these
        // bindings. `resolved_plan_path`/`resolved_url` are the post-edit
        // values `editor.rs` already computed — consumed here, not re-derived.
        let TaskEditApplied {
            title,
            description,
            repo_path,
            status,
            plan_path,
            resolved_plan_path,
            tag,
            base_branch,
            wrap_up_mode,
            url,
            resolved_url,
            schedule_interval_secs,
            pinned_branch,
        } = applied;

        let mut params = UpdateTaskParams::for_task(task_id)
            .status(status)
            .plan_path(plan_path)
            .title(title.clone())
            .description(description.clone())
            .repo_path(repo_path.clone())
            .tag(Some(tag))
            .base_branch(base_branch.clone())
            .wrap_up_mode(wrap_up_mode)
            .schedule_interval_secs(schedule_interval_secs)
            .pinned_branch(pinned_branch.clone());
        // Only forward a url change when the edit actually altered it.
        if let Some(url_update) = url {
            params = params.url(url_update);
        }

        if let Err(e) = self.task_svc.update_task(params).await {
            app.update(Message::System(crate::tui::messages::SystemMessage::Error(
                Self::db_error("updating task", e),
            )));
        }

        // Persist non-empty edited repo_path to the known list so sibling
        // feed items (e.g. other Dependabot PRs in the same repo) can be
        // resolved on the next feed sync.
        if !repo_path.is_empty() && repo_path != prior_repo_path {
            self.exec_save_repo_path(app, repo_path.clone()).await;
        }

        app.update(Message::Task(crate::tui::messages::TaskMessage::Edited(
            crate::tui::TaskEdit {
                id: task_id,
                title,
                description,
                repo_path,
                status,
                plan_path: resolved_plan_path,
                tag,
                base_branch,
                wrap_up_mode,
                url: resolved_url,
                schedule_interval_secs,
                pinned_branch,
            },
        )))
    }

    async fn finalize_epic_edit(
        &self,
        app: &mut App,
        epic: models::Epic,
        outcome: EditorOutcome,
    ) -> Vec<Command> {
        let Some(text) = saved_text(outcome) else {
            return vec![];
        };
        let mut fields = parse_epic_editor_output(&text);
        let parse_errors = std::mem::take(&mut fields.errors);
        let applied = apply_epic_editor_fields(&epic, fields);
        emit_parse_errors(app, &parse_errors);

        let epic_id = epic.id;
        if let Err(e) = self
            .epic_svc
            .update_epic(UpdateEpicParams {
                epic_id,
                title: Some(applied.title.clone()),
                description: Some(applied.description.clone()),
                status: None,
                plan_path: None,
                sort_order: None,
                auto_dispatch: None,
                feed_command: Some(applied.feed_command.clone()),
                feed_interval_secs: Some(applied.feed_interval_secs),
                group_by_repo: None,
                parent_epic_id: None,
            })
            .await
        {
            app.update(Message::System(crate::tui::messages::SystemMessage::Error(
                Self::db_error("updating epic", e),
            )));
        }
        let mut updated = epic;
        updated.title = applied.title;
        updated.description = applied.description;
        if let crate::service::FieldUpdate::Set(ref cmd) = applied.feed_command {
            updated.feed_command = Some(cmd.clone());
        } else {
            updated.feed_command = None;
        }
        updated.feed_interval_secs = applied.feed_interval_secs;
        app.update(Message::Epic(crate::tui::messages::EpicMessage::Edited(
            updated,
        )))
    }
}

/// Surface accumulated editor parse errors as a status message. No-op when
/// the slice is empty so callers don't need to guard the call themselves.
fn emit_parse_errors(app: &mut App, errors: &[crate::editor::EditorParseError]) {
    if errors.is_empty() {
        return;
    }
    let summary = errors
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("; ");
    app.update(Message::System(
        crate::tui::messages::SystemMessage::StatusInfo(format!(
            "Edit accepted with parse errors — {summary}"
        )),
    ));
}

/// Extract the saved text from an [`EditorOutcome`], returning `None` if
/// cancelled.
fn saved_text(outcome: EditorOutcome) -> Option<String> {
    match outcome {
        EditorOutcome::Saved(text) => Some(text),
        EditorOutcome::Cancelled => None,
    }
}

/// Best-effort clear of the session slot. Logs if the mutex is poisoned but
/// keeps going so the watcher doesn't leave the slot stuck populated.
fn clear_session_slot(slot: &Arc<Mutex<Option<EditorSession>>>) {
    match slot.lock() {
        Ok(mut g) => {
            // Take the session out and drop it outside the lock so Drop
            // side-effects (tempfile removal, kill-window) don't run while
            // holding the mutex.
            let taken = g.take();
            drop(g);
            drop(taken);
        }
        Err(poisoned) => {
            let mut g = poisoned.into_inner();
            g.take();
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::cell::Cell;

    #[tokio::test]
    async fn watch_editor_returns_saved_when_window_gone_and_read_ok() {
        let iterations = Cell::new(0);
        let outcome = watch_editor(
            || {
                let n = iterations.get();
                iterations.set(n + 1);
                n < 3
            },
            || {},
            || Ok("hello".to_string()),
        );
        assert!(matches!(outcome, EditorOutcome::Saved(s) if s == "hello"));
        // Ran 3 alive-checks (returning true) + 1 more that returned false.
        assert_eq!(iterations.get(), 4);
    }

    #[tokio::test]
    async fn watch_editor_returns_cancelled_when_read_fails() {
        let outcome = watch_editor(
            || false,
            || {},
            || Err(io::Error::new(io::ErrorKind::NotFound, "missing")),
        );
        assert!(matches!(outcome, EditorOutcome::Cancelled));
    }

    #[tokio::test]
    async fn watch_editor_stops_polling_once_window_gone() {
        let iterations = Cell::new(0);
        let sleep_calls = Cell::new(0);
        watch_editor(
            || {
                iterations.set(iterations.get() + 1);
                false
            },
            || sleep_calls.set(sleep_calls.get() + 1),
            || Ok(String::new()),
        );
        // Single check, no sleeps.
        assert_eq!(iterations.get(), 1);
        assert_eq!(sleep_calls.get(), 0);
    }

    // --- window_alive_with_bounded_retry ---

    #[test]
    fn window_alive_with_bounded_retry_true_when_present() {
        use crate::process::MockProcessRunner;
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"task-42\n")]);
        let mut failures = 0;
        assert!(window_alive_with_bounded_retry(
            "task-42",
            &mock,
            &mut failures
        ));
        assert_eq!(failures, 0);
    }

    #[test]
    fn window_alive_with_bounded_retry_false_when_absent() {
        use crate::process::MockProcessRunner;
        let mock = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(b"other\n")]);
        let mut failures = 0;
        assert!(!window_alive_with_bounded_retry(
            "task-42",
            &mock,
            &mut failures
        ));
        assert_eq!(failures, 0);
    }

    #[test]
    fn window_alive_with_bounded_retry_true_on_transient_failure() {
        use crate::process::MockProcessRunner;
        let mock = MockProcessRunner::new(vec![Err(anyhow::anyhow!("tmux: command not found"))]);
        let mut failures = 0;
        assert!(
            window_alive_with_bounded_retry("task-42", &mock, &mut failures),
            "a single query failure should not be treated as the window closing"
        );
        assert_eq!(failures, 1);
    }

    #[test]
    fn window_alive_with_bounded_retry_gives_up_after_max_consecutive_failures() {
        use crate::process::MockProcessRunner;
        let mock = MockProcessRunner::new(
            (0..MAX_CONSECUTIVE_QUERY_FAILURES)
                .map(|_| Err(anyhow::anyhow!("tmux: command not found")))
                .collect(),
        );
        let mut failures = 0;
        for _ in 0..MAX_CONSECUTIVE_QUERY_FAILURES - 1 {
            assert!(window_alive_with_bounded_retry(
                "task-42",
                &mock,
                &mut failures
            ));
        }
        assert!(
            !window_alive_with_bounded_retry("task-42", &mock, &mut failures),
            "a permanently broken tmux must eventually be treated as closed, \
             or watch_editor's loop would hang forever"
        );
    }

    #[test]
    fn window_alive_with_bounded_retry_resets_count_after_success() {
        use crate::process::MockProcessRunner;
        let mock = MockProcessRunner::new(vec![
            Err(anyhow::anyhow!("tmux: command not found")),
            MockProcessRunner::ok_with_stdout(b"task-42\n"),
        ]);
        let mut failures = 0;
        assert!(window_alive_with_bounded_retry(
            "task-42",
            &mock,
            &mut failures
        ));
        assert_eq!(failures, 1);
        assert!(window_alive_with_bounded_retry(
            "task-42",
            &mock,
            &mut failures
        ));
        assert_eq!(failures, 0, "a successful query should reset the counter");
    }

    #[tokio::test]
    async fn editor_session_drop_removes_tempfile() {
        use tempfile::NamedTempFile;

        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        // Consume the NamedTempFile without deleting so only EditorSession
        // owns the file.
        let (_file, persisted) = tmp.keep().unwrap();
        assert_eq!(persisted, path);
        assert!(path.exists());

        let session = EditorSession {
            window_name: "test-window".to_string(),
            temp_path: Some(path.clone()),
            cleanup_runner: None,
        };
        drop(session);
        assert!(!path.exists(), "tempfile should be removed on drop");
    }

    #[tokio::test]
    async fn editor_session_drop_kills_tmux_window_when_runner_set() {
        use crate::process::MockProcessRunner;

        let mock = Arc::new(
            MockProcessRunner::new(vec![MockProcessRunner::ok()]).with_windows(&["edit-window"]),
        );
        let session = EditorSession {
            window_name: "edit-window".to_string(),
            temp_path: None,
            cleanup_runner: Some(mock.clone()),
        };
        drop(session);
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        // Targeted by the window's resolved pane ID — see `tmux::window_target`.
        assert_eq!(
            calls[0].1,
            vec!["kill-window", "-t", &mock.pane_id_of("edit-window")]
        );
    }

    #[test]
    fn emit_pop_out_error_surfaces_error_popup() {
        let mut app = App::new(vec![]);
        emit_pop_out_error(&mut app, "boom".to_string());
        let msg = app.error_popup().unwrap_or_default();
        assert!(
            msg.contains("boom"),
            "expected 'boom' in error popup, got {msg:?}"
        );
    }

    #[tokio::test]
    async fn saved_text_extracts_from_saved() {
        assert_eq!(
            saved_text(EditorOutcome::Saved("x".into())),
            Some("x".into())
        );
    }

    #[tokio::test]
    async fn saved_text_returns_none_for_cancelled() {
        assert_eq!(saved_text(EditorOutcome::Cancelled), None);
    }

    #[tokio::test]
    async fn editor_already_open_msg_is_stable() {
        // Pinned so a future rename is a deliberate act, not an accident.
        assert_eq!(
            EDITOR_ALREADY_OPEN_MSG,
            "Editor already open — close it first"
        );
    }

    // --- TuiRuntime-level tests -------------------------------------------
    //
    // The watcher task inside exec_pop_out_editor is async (spawn_blocking);
    // these tests cover the synchronous parts: the guard and the
    // finalize-result dispatch. The watcher itself is covered by the pure
    // watch_editor tests above.

    use crate::db::{CreateTaskRequest, Database};
    use crate::models::TaskStatus;
    use crate::process::MockProcessRunner;
    use crate::tui::{App, EditKind};
    use tokio::sync::mpsc::unbounded_channel;

    /// Build a `TuiRuntime` for these editor tests.
    ///
    /// One fixture rather than a literal per test: every `TuiRuntime` field
    /// addition would otherwise be a nine-site edit, and one of those fields —
    /// `feed_sync_guard` — has to be the `FeedRunner`'s own registry or the two
    /// feed surfaces silently stop serialising against each other. Wiring that
    /// correctly once beats warning about it nine times.
    fn editor_runtime(
        db: Arc<dyn crate::db::TaskStore>,
        runner: Arc<dyn ProcessRunner>,
        msg_tx: tokio::sync::mpsc::UnboundedSender<crate::tui::Message>,
        todo_db: Arc<dyn crate::db::TodoStore>,
    ) -> TuiRuntime {
        let (feed_tx, _) = unbounded_channel();
        let feed_runner = crate::feed::FeedRunner::new(db.clone(), feed_tx, runner.clone());
        let feed_sync_guard = feed_runner.sync_guard();
        TuiRuntime {
            task_svc: Arc::new(crate::service::TaskService::new(db.clone(), runner.clone())),
            epic_svc: Arc::new(crate::service::EpicService::new(db.clone())),
            todo_svc: Arc::new(crate::service::TodoService::new(todo_db)),
            feed_runner: Some(feed_runner),
            // Never started by these fixtures — see the field's doc comment.
            scheduler_runner: None,
            feed_sync_guard,
            feed_invalidate_tx: None,
            learning_svc: Arc::new(crate::service::MockLearningService),
            feed_db: db.clone(),
            database: db,
            msg_tx,
            runner,
            editor_session: Arc::new(Mutex::new(None)),
            emb_svc: EmbeddingService::new_noop(),
            last_change_count: std::sync::atomic::AtomicI64::new(-1),
            budget_snapshot_path: std::path::PathBuf::from(
                "/nonexistent-test-path/rate-limits.json",
            ),
            claude_json_path: std::path::PathBuf::from("/nonexistent-test-path/.claude.json"),
        }
    }

    async fn runtime_with_runner(runner: Arc<dyn ProcessRunner>) -> (TuiRuntime, App) {
        let db: Arc<dyn crate::db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
        let (tx, _rx) = unbounded_channel();
        let rt = editor_runtime(
            db,
            runner.clone(),
            tx,
            Arc::new(Database::open_in_memory().await.unwrap()) as Arc<dyn crate::db::TodoStore>,
        );
        let app = App::new(vec![]);
        (rt, app)
    }

    #[tokio::test]
    async fn exec_pop_out_editor_is_noop_when_session_occupied() {
        let mock = Arc::new(MockProcessRunner::new(vec![]));
        let (rt, mut app) = runtime_with_runner(mock.clone()).await;

        // Pre-populate the session slot.
        *rt.editor_session.lock().unwrap() = Some(EditorSession {
            window_name: "already-open".into(),
            temp_path: None,
            cleanup_runner: None,
        });

        rt.exec_pop_out_editor(&mut app, EditKind::Description { is_epic: false });

        // No tmux calls should have been issued.
        assert_eq!(mock.recorded_calls().len(), 0);
        // A status message should surface the "already open" notice.
        let msg = app.status_message().unwrap_or_default();
        assert!(
            msg.contains("Editor already open"),
            "expected 'Editor already open' in status, got {msg:?}"
        );
    }

    /// The pop-out editor and the agent-tree editor pane resolve one setting,
    /// so they must resolve it the same way: `crate::editor::editor_from_env`.
    /// Asserted as argv *elements* — a multi-word `$EDITOR` ("vim -p") passed
    /// as a single string would be looked up as a binary of that name and fail
    /// to launch, which is the bug this pins.
    ///
    /// Written against whatever the ambient environment resolves to rather than
    /// a fixed editor name: `std::env::set_var` is `unsafe` and races the other
    /// harness threads, so the environment is read here, never written.
    #[tokio::test]
    async fn exec_pop_out_editor_launches_the_resolved_editor_argv() {
        let mock = Arc::new(
            MockProcessRunner::new(vec![
                MockProcessRunner::ok(), // new-window
                MockProcessRunner::ok(), // select-window (focus the editor)
                MockProcessRunner::ok(), // watcher: list-windows -> none, so it exits
                MockProcessRunner::ok(), // select-window (focus back to the TUI)
            ])
            // Window-name lookups are answered out of band, so the session's
            // best-effort kill-window on drop cannot exhaust the queue above.
            .with_windows(&[]),
        );
        let (rt, mut app) = runtime_with_runner(mock.clone()).await;

        rt.exec_pop_out_editor(&mut app, EditKind::Description { is_epic: false });

        // calls[0] is issued synchronously, before the watcher thread starts.
        let calls = mock.recorded_calls();
        assert_eq!(calls[0].0, "tmux");
        let args = &calls[0].1;
        assert_eq!(args[0], "new-window");
        let sep = args
            .iter()
            .position(|a| a == "--")
            .unwrap_or_else(|| panic!("no exec separator in {args:?}"));
        let expected = crate::editor::editor_from_env();
        assert_eq!(
            args[sep + 1..args.len() - 1],
            expected[..],
            "the editor must reach tmux as the resolved argv; got {args:?}"
        );
        assert!(
            args[args.len() - 1].ends_with(".md"),
            "the tempfile must be the final argv element; got {args:?}"
        );
    }

    async fn seed_task(db: &dyn crate::db::TaskStore) -> models::Task {
        let id = db
            .create_task(CreateTaskRequest {
                title: "Original title",
                description: "Original desc",
                repo_path: "/orig/repo",
                plan: Some("docs/plan.md"),
                status: TaskStatus::Backlog,
                base_branch: "main",
                epic_id: None,
                sort_order: None,
                tag: None,

                wrap_up_mode: None,
                auto_run_plan: false,
                schedule_interval_secs: None,
                pinned_branch: None,
            })
            .await
            .unwrap();
        db.get_task(id).await.unwrap().unwrap()
    }

    #[tokio::test]
    async fn finalize_task_edit_persists_changes() {
        let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
        let db: Arc<dyn crate::db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
        let task = seed_task(&*db).await;

        let (tx, _rx) = unbounded_channel();
        let rt = editor_runtime(
            db.clone(),
            runner.clone(),
            tx,
            Arc::new(Database::open_in_memory().await.unwrap()) as Arc<dyn crate::db::TodoStore>,
        );
        let mut app = App::new(vec![task.clone()]);

        let edited_text = "--- TITLE ---\nNew title\n\
            --- DESCRIPTION ---\nNew description\n\
            --- REPO_PATH ---\n/new/repo\n\
            --- STATUS ---\nrunning\n\
            --- PLAN ---\n\n\
            --- TAG ---\nbug\n\
            --- BASE_BRANCH ---\n\n";

        rt.exec_finalize_editor_result(
            &mut app,
            EditKind::TaskEdit(Box::new(task.clone())),
            EditorOutcome::Saved(edited_text.into()),
        )
        .await;

        // The DB row should reflect the edits.
        let updated = db.get_task(task.id).await.unwrap().unwrap();
        assert_eq!(updated.title, "New title");
        assert_eq!(updated.description, "New description");
        assert_eq!(updated.repo_path, "/new/repo");
        assert_eq!(updated.status, TaskStatus::Running);
        // Empty BASE_BRANCH → preserved prior value at the runtime layer
        // (service treats None as "don't touch" rather than "clear").
        assert_eq!(updated.base_branch, "main");
    }

    #[tokio::test]
    async fn finalize_task_edit_persists_url() {
        use crate::models::{TaskUrl, UrlType};
        let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
        let db: Arc<dyn crate::db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
        let task = seed_task(&*db).await; // Backlog → no was_pr_finalisation path
        assert!(task.url.is_none());

        let (tx, _rx) = unbounded_channel();
        let rt = editor_runtime(
            db.clone(),
            runner.clone(),
            tx,
            Arc::new(Database::open_in_memory().await.unwrap()) as Arc<dyn crate::db::TodoStore>,
        );
        let mut app = App::new(vec![task.clone()]);

        let edited_text = "--- TITLE ---\n\n\
            --- URL ---\nhttps://github.com/o/r/pull/9\n\
            --- URL_TYPE ---\npr\n";

        rt.exec_finalize_editor_result(
            &mut app,
            EditKind::TaskEdit(Box::new(task.clone())),
            EditorOutcome::Saved(edited_text.into()),
        )
        .await;

        let updated = db.get_task(task.id).await.unwrap().unwrap();
        assert_eq!(
            updated.url,
            Some(TaskUrl::new("https://github.com/o/r/pull/9", UrlType::Pr))
        );
        // In-memory snapshot updated too.
        assert_eq!(app.tasks()[0].url, updated.url);
    }

    #[tokio::test]
    async fn finalize_task_edit_clears_url_when_section_emptied() {
        use crate::models::{TaskUrl, UrlType};
        use crate::service::{UpdateTaskParams, UrlUpdate};
        let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
        let db: Arc<dyn crate::db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
        let task = seed_task(&*db).await;

        let (tx, _rx) = unbounded_channel();
        let rt = editor_runtime(
            db.clone(),
            runner.clone(),
            tx,
            Arc::new(Database::open_in_memory().await.unwrap()) as Arc<dyn crate::db::TodoStore>,
        );
        // Pre-set a url on the task.
        rt.task_svc
            .update_task(
                UpdateTaskParams::for_task(task.id).url(UrlUpdate::Set(TaskUrl::new(
                    "https://github.com/o/r/pull/1",
                    UrlType::Pr,
                ))),
            )
            .await
            .unwrap();
        let task = db.get_task(task.id).await.unwrap().unwrap();
        assert!(task.url.is_some());
        let mut app = App::new(vec![task.clone()]);

        // URL section present but empty → clear.
        let edited_text = "--- TITLE ---\n\n--- URL ---\n\n--- URL_TYPE ---\n\n";
        rt.exec_finalize_editor_result(
            &mut app,
            EditKind::TaskEdit(Box::new(task.clone())),
            EditorOutcome::Saved(edited_text.into()),
        )
        .await;

        let updated = db.get_task(task.id).await.unwrap().unwrap();
        assert_eq!(updated.url, None);
        assert_eq!(app.tasks()[0].url, None);
    }

    #[tokio::test]
    async fn finalize_task_edit_clears_plan_when_section_emptied() {
        // Regression: blanking the PLAN section must clear plan_path in the
        // DB, not just the in-memory snapshot. The editor expresses "clear"
        // via FieldUpdate::Clear, which must reach the DB patch.
        let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
        let db: Arc<dyn crate::db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
        let task = seed_task(&*db).await; // seeded with plan docs/plan.md
        assert!(task.plan_path.is_some(), "precondition: task has a plan");

        let (tx, _rx) = unbounded_channel();
        let rt = editor_runtime(
            db.clone(),
            runner.clone(),
            tx,
            Arc::new(Database::open_in_memory().await.unwrap()) as Arc<dyn crate::db::TodoStore>,
        );
        let mut app = App::new(vec![task.clone()]);

        // PLAN section present but empty → clear.
        let edited_text = "--- TITLE ---\n\n--- PLAN ---\n\n";
        rt.exec_finalize_editor_result(
            &mut app,
            EditKind::TaskEdit(Box::new(task.clone())),
            EditorOutcome::Saved(edited_text.into()),
        )
        .await;

        let updated = db.get_task(task.id).await.unwrap().unwrap();
        assert_eq!(updated.plan_path, None, "DB plan_path should be cleared");
        assert_eq!(app.tasks()[0].plan_path, None);
    }

    #[tokio::test]
    async fn finalize_task_edit_clears_tag_when_section_emptied() {
        // Regression: blanking the TAG section must clear the tag in the DB,
        // not just the in-memory snapshot.
        use crate::models::TaskTag;
        let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
        let db: Arc<dyn crate::db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
        let task = seed_task(&*db).await;

        let (tx, _rx) = unbounded_channel();
        let rt = editor_runtime(
            db.clone(),
            runner.clone(),
            tx,
            Arc::new(Database::open_in_memory().await.unwrap()) as Arc<dyn crate::db::TodoStore>,
        );
        // Pre-set a tag on the task.
        rt.task_svc
            .update_task(UpdateTaskParams::for_task(task.id).tag(Some(Some(TaskTag::Bug))))
            .await
            .unwrap();
        let task = db.get_task(task.id).await.unwrap().unwrap();
        assert_eq!(task.tag, Some(TaskTag::Bug), "precondition: task has a tag");
        let mut app = App::new(vec![task.clone()]);

        // TAG section present but empty → clear.
        let edited_text = "--- TITLE ---\n\n--- TAG ---\n\n";
        rt.exec_finalize_editor_result(
            &mut app,
            EditKind::TaskEdit(Box::new(task.clone())),
            EditorOutcome::Saved(edited_text.into()),
        )
        .await;

        let updated = db.get_task(task.id).await.unwrap().unwrap();
        assert_eq!(updated.tag, None, "DB tag should be cleared");
        assert_eq!(app.tasks()[0].tag, None);
    }

    #[tokio::test]
    async fn finalize_task_edit_persists_new_repo_path_to_known_list() {
        // Edits that change repo_path must also add the new path to the
        // saved repo_paths list, so sibling feed items (e.g. other
        // Dependabot PRs in the same repo) can be auto-resolved.
        let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
        let db: Arc<dyn crate::db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
        let task = seed_task(&*db).await;
        // Precondition: known repo_paths does not contain the new path.
        assert!(
            !db.list_repo_paths()
                .await
                .unwrap()
                .iter()
                .any(|p| p == "/new/repo"),
            "precondition: /new/repo should not be in known list yet"
        );

        let (tx, _rx) = unbounded_channel();
        let rt = editor_runtime(
            db.clone(),
            runner.clone(),
            tx,
            Arc::new(Database::open_in_memory().await.unwrap()) as Arc<dyn crate::db::TodoStore>,
        );
        let mut app = App::new(vec![task.clone()]);

        let edited_text = "--- TITLE ---\n\n\
            --- DESCRIPTION ---\n\n\
            --- REPO_PATH ---\n/new/repo\n\
            --- STATUS ---\n\n\
            --- PLAN ---\n\n\
            --- TAG ---\n\n\
            --- BASE_BRANCH ---\n\n";

        rt.exec_finalize_editor_result(
            &mut app,
            EditKind::TaskEdit(Box::new(task.clone())),
            EditorOutcome::Saved(edited_text.into()),
        )
        .await;

        let paths = db.list_repo_paths().await.unwrap();
        assert!(
            paths.iter().any(|p| p == "/new/repo"),
            "expected /new/repo in known repo_paths, got {paths:?}"
        );
    }

    #[tokio::test]
    async fn finalize_task_edit_unchanged_repo_path_does_not_save() {
        // When repo_path is unchanged (empty section preserves the prior
        // value), we must not re-save it. Avoids spurious writes when
        // editing unrelated fields.
        let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
        let db: Arc<dyn crate::db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
        let task = seed_task(&*db).await;

        let (tx, _rx) = unbounded_channel();
        let rt = editor_runtime(
            db.clone(),
            runner.clone(),
            tx,
            Arc::new(Database::open_in_memory().await.unwrap()) as Arc<dyn crate::db::TodoStore>,
        );
        let mut app = App::new(vec![task.clone()]);

        // Title change only — REPO_PATH section is empty so the editor
        // applier preserves the prior /orig/repo value.
        let edited_text = "--- TITLE ---\nNew title\n\
            --- DESCRIPTION ---\n\n\
            --- REPO_PATH ---\n\n\
            --- STATUS ---\n\n\
            --- PLAN ---\n\n\
            --- TAG ---\n\n\
            --- BASE_BRANCH ---\n\n";

        rt.exec_finalize_editor_result(
            &mut app,
            EditKind::TaskEdit(Box::new(task.clone())),
            EditorOutcome::Saved(edited_text.into()),
        )
        .await;

        // /orig/repo was never in the known list, and a no-op edit must
        // not add it.
        let paths = db.list_repo_paths().await.unwrap();
        assert!(
            !paths.iter().any(|p| p == "/orig/repo"),
            "unchanged repo_path must not be added to known list, got {paths:?}"
        );
    }

    #[tokio::test]
    async fn finalize_task_edit_cancelled_does_not_change_db() {
        let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
        let db: Arc<dyn crate::db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
        let task = seed_task(&*db).await;

        let (tx, _rx) = unbounded_channel();
        let rt = editor_runtime(
            db.clone(),
            runner.clone(),
            tx,
            Arc::new(Database::open_in_memory().await.unwrap()) as Arc<dyn crate::db::TodoStore>,
        );
        let mut app = App::new(vec![task.clone()]);

        rt.exec_finalize_editor_result(
            &mut app,
            EditKind::TaskEdit(Box::new(task.clone())),
            EditorOutcome::Cancelled,
        )
        .await;

        let still = db.get_task(task.id).await.unwrap().unwrap();
        assert_eq!(still.title, task.title);
        assert_eq!(still.description, task.description);
    }

    #[tokio::test]
    async fn finalize_description_kind_is_noop() {
        // Description edits are finalized inside App::update (not here).
        // If a FinalizeEditorResult with Description leaks through, it
        // should not crash or produce commands.
        let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
        let (rt, mut app) = runtime_with_runner(runner).await;
        let cmds = rt
            .exec_finalize_editor_result(
                &mut app,
                EditKind::Description { is_epic: false },
                EditorOutcome::Saved("ignored".into()),
            )
            .await;
        assert!(cmds.is_empty());
    }
}
