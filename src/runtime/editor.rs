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
use crate::tui::messages::LearningMessage;
use crate::tui::{App, Command, EditKind, EditorOutcome, Message};
use crate::{models, tmux};

/// Interval between `has_window` polls while waiting for the editor to exit.
const POLL_INTERVAL: Duration = Duration::from_millis(300);

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
        EditKind::Learning(learning) => {
            let prefix = format!("learning-{}-", learning.id);
            let content = crate::editor::format_learning_for_editor(learning);
            (prefix, content)
        }
    }
}

/// Resolve the `$EDITOR` command to launch, falling back to `vim` when the
/// variable is unset or invalid. Takes the lookup result rather than reading
/// the environment itself so it's directly unit-testable.
fn resolve_editor_command(env_result: Result<String, std::env::VarError>) -> String {
    env_result.unwrap_or_else(|_| "vim".to_string())
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
        let editor_cmd = resolve_editor_command(std::env::var("EDITOR"));
        let cwd = std::env::temp_dir();
        let cwd_str = cwd.to_string_lossy().into_owned();
        let temp_str = temp_path.to_string_lossy().into_owned();

        if let Err(e) = tmux::new_window_running(
            &window_name,
            &cwd_str,
            &[&editor_cmd, &temp_str],
            &*self.runner,
        ) {
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
            let outcome = watch_editor(
                || tmux::has_window(&window, &*runner).unwrap_or(false),
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
            EditKind::TaskEdit(task) => self.finalize_task_edit(app, task, outcome).await,
            EditKind::EpicEdit(epic) => self.finalize_epic_edit(app, epic, outcome).await,
            EditKind::Description { .. } => {
                tracing::warn!("FinalizeEditorResult received Description kind; ignoring");
                vec![]
            }
            EditKind::Learning(learning) => {
                self.finalize_learning_edit(app, learning, outcome).await;
                vec![]
            }
        }
    }

    async fn finalize_learning_edit(
        &self,
        app: &mut App,
        learning: models::Learning,
        outcome: EditorOutcome,
    ) {
        let EditorOutcome::Saved(content) = outcome else {
            return; // Cancelled — no-op
        };
        let fields = crate::editor::parse_learning_editor_output(&content);

        let params = crate::service::UpdateLearningParams {
            id: learning.id,
            summary: if fields.summary.is_empty() {
                None
            } else {
                Some(fields.summary)
            },
            kind: fields.kind,
            tags: fields.tags,
            detail: fields.detail,
        };

        let db: Arc<dyn crate::db::TaskReadStore> = self.database.clone();
        match self.learning_svc.update_learning(params).await {
            Ok(()) => {
                if let Ok(Some(updated)) = db.get_learning(learning.id).await {
                    app.update(Message::Learning(LearningMessage::Edited(updated)));
                }
            }
            Err(e) => {
                app.update(Message::System(
                    crate::tui::messages::SystemMessage::StatusInfo(format!("Edit failed: {e}")),
                ));
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
        } = applied;

        let mut params = UpdateTaskParams::for_task(task_id)
            .status(status)
            .plan_path(plan_path)
            .title(title.clone())
            .description(description.clone())
            .repo_path(repo_path.clone())
            .tag(Some(tag))
            .base_branch(base_branch.clone())
            .wrap_up_mode(wrap_up_mode);
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
mod learning_editor_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::db::{CreateLearningRow, Database, LearningStore};
    use crate::models::{Learning, LearningId, LearningKind, LearningScope, LearningStatus};
    use crate::tui::ViewMode;
    use crate::tui::{App, Message};
    use chrono::Utc;
    use std::sync::Arc;

    fn make_learning(id: LearningId) -> Learning {
        Learning {
            id,
            kind: LearningKind::Convention,
            summary: "original summary".to_string(),
            detail: None,
            scope: LearningScope::Repo,
            scope_ref: Some("/repo".to_string()),
            tags: vec![],
            status: LearningStatus::Approved,
            source_task_id: None,
            upvote_count: 0,
            last_upvoted_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn make_runtime(db: Arc<Database>) -> TuiRuntime {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (feed_tx, _) = tokio::sync::mpsc::unbounded_channel();
        let db_arc: Arc<dyn crate::db::TaskStore> = db.clone();
        let runner: Arc<dyn crate::process::ProcessRunner> =
            Arc::new(crate::process::MockProcessRunner::new(vec![]));
        let emb_svc = EmbeddingService::new_noop();
        TuiRuntime {
            feed_db: db_arc.clone(),
            database: db_arc.clone(),
            task_svc: Arc::new(crate::service::TaskService::new(
                db_arc.clone(),
                runner.clone(),
            )),
            epic_svc: Arc::new(crate::service::EpicService::new(db_arc.clone())),
            todo_svc: Arc::new(crate::service::TodoService::new(db.clone())),
            learning_svc: Arc::new(crate::service::LearningService::new(
                db_arc.clone(),
                emb_svc.clone(),
            )),
            feed_runner: Some(crate::feed::FeedRunner::new(
                db_arc.clone(),
                feed_tx,
                runner.clone(),
            )),
            feed_invalidate_tx: None,
            msg_tx: tx,
            runner,
            editor_session: Arc::new(std::sync::Mutex::new(None)),
            emb_svc,
            last_change_count: std::sync::atomic::AtomicI64::new(-1),
        }
    }

    #[tokio::test]
    async fn initial_content_includes_summary_and_kind() {
        let l = make_learning(LearningId(1));
        let (prefix, content) = initial_content_for(&EditKind::Learning(l));
        assert!(content.contains("original summary"));
        assert!(content.contains("convention"));
        assert!(prefix.starts_with("learning-1-"));
    }

    #[tokio::test]
    async fn saved_valid_content_updates_db_and_sends_learning_edited() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let id = db
            .create_learning(CreateLearningRow {
                kind: LearningKind::Convention,
                summary: "original summary",
                detail: None,
                scope: LearningScope::User,
                scope_ref: None,
                tags: &[],
                source_task_id: None,
                embedding: None,
            })
            .await
            .unwrap();
        let learning = make_learning(id);
        let rt = make_runtime(db.clone());
        // Put app into Learnings view
        let mut app = App::new(vec![]);
        app.update(Message::Learning(LearningMessage::Show(vec![
            learning.clone()
        ])));

        let updated_content = "--- SUMMARY ---\nnew summary\n--- KIND ---\npitfall\n--- TAGS ---\nrust\n--- DETAIL ---\nsome detail\n".to_string();
        rt.exec_finalize_editor_result(
            &mut app,
            EditKind::Learning(learning),
            EditorOutcome::Saved(updated_content),
        )
        .await;

        let updated = db.get_learning(id).await.unwrap().unwrap();
        assert_eq!(updated.summary, "new summary");
        assert_eq!(updated.kind, LearningKind::Pitfall);

        // Snapshot in overlay should be updated
        assert!(matches!(
            app.view_mode(),
            ViewMode::Learnings { learnings, .. }
                if learnings.iter().find(|l| l.id == id)
                    .map(|l| l.summary.as_str()) == Some("new summary")
        ));
    }

    #[tokio::test]
    async fn saved_empty_summary_preserves_original() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let id = db
            .create_learning(CreateLearningRow {
                kind: LearningKind::Convention,
                summary: "original summary",
                detail: None,
                scope: LearningScope::User,
                scope_ref: None,
                tags: &[],
                source_task_id: None,
                embedding: None,
            })
            .await
            .unwrap();
        let learning = make_learning(id);
        let rt = make_runtime(db.clone());
        let mut app = App::new(vec![]);
        app.update(Message::Learning(LearningMessage::Show(vec![
            learning.clone()
        ])));

        let content_empty_summary =
            "--- SUMMARY ---\n\n--- KIND ---\npitfall\n--- TAGS ---\n\n--- DETAIL ---\n\n"
                .to_string();
        rt.exec_finalize_editor_result(
            &mut app,
            EditKind::Learning(learning),
            EditorOutcome::Saved(content_empty_summary),
        )
        .await;

        let updated = db.get_learning(id).await.unwrap().unwrap();
        assert_eq!(updated.summary, "original summary");
        assert_eq!(updated.kind, LearningKind::Pitfall);
    }

    #[tokio::test]
    async fn cancelled_edit_is_noop() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let id = db
            .create_learning(CreateLearningRow {
                kind: LearningKind::Convention,
                summary: "original summary",
                detail: None,
                scope: LearningScope::User,
                scope_ref: None,
                tags: &[],
                source_task_id: None,
                embedding: None,
            })
            .await
            .unwrap();
        let learning = make_learning(id);
        let rt = make_runtime(db.clone());
        let mut app = App::new(vec![]);

        rt.exec_finalize_editor_result(
            &mut app,
            EditKind::Learning(learning),
            EditorOutcome::Cancelled,
        )
        .await;

        let unchanged = db.get_learning(id).await.unwrap().unwrap();
        assert_eq!(unchanged.summary, "original summary");
    }

    #[tokio::test]
    async fn saved_content_for_rejected_learning_shows_status_error() {
        // update_learning rejects learnings with status Rejected — verify the
        // error surfaces as StatusInfo and does not update DB.
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let id = db
            .create_learning(CreateLearningRow {
                kind: LearningKind::Convention,
                summary: "original summary",
                detail: None,
                scope: LearningScope::User,
                scope_ref: None,
                tags: &[],
                source_task_id: None,
                embedding: None,
            })
            .await
            .unwrap();
        crate::service::LearningService::new(
            db.clone() as Arc<dyn crate::db::TaskStore>,
            EmbeddingService::new_noop(),
        )
        .reject_learning(id)
        .await
        .unwrap();
        let learning = make_learning(id);
        let rt = make_runtime(db.clone());
        let mut app = App::new(vec![]);

        rt.exec_finalize_editor_result(
            &mut app,
            EditKind::Learning(learning),
            EditorOutcome::Saved(
                "--- SUMMARY ---\nnew summary\n--- KIND ---\nconvention\n--- TAGS ---\n\n--- DETAIL ---\n\n"
                    .to_string(),
            ),
        )
        .await;

        let msg = app.status_message().unwrap_or_default();
        assert!(
            msg.contains("Edit failed"),
            "expected 'Edit failed' in status message, got: {msg}"
        );
        let unchanged = db.get_learning(id).await.unwrap().unwrap();
        assert_eq!(unchanged.summary, "original summary");
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

        let mock = Arc::new(MockProcessRunner::new(vec![MockProcessRunner::ok()]));
        let session = EditorSession {
            window_name: "edit-window".to_string(),
            temp_path: None,
            cleanup_runner: Some(mock.clone()),
        };
        drop(session);
        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "tmux");
        assert_eq!(calls[0].1, vec!["kill-window", "-t", "edit-window"]);
    }

    #[test]
    fn resolve_editor_command_uses_env_value_when_present() {
        assert_eq!(
            resolve_editor_command(Ok("nano".to_string())),
            "nano".to_string()
        );
    }

    #[test]
    fn resolve_editor_command_falls_back_to_vim_when_absent() {
        assert_eq!(
            resolve_editor_command(Err(std::env::VarError::NotPresent)),
            "vim".to_string()
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

    async fn runtime_with_runner(runner: Arc<dyn ProcessRunner>) -> (TuiRuntime, App) {
        let db: Arc<dyn crate::db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
        let (tx, _rx) = unbounded_channel();
        let (feed_tx, _) = unbounded_channel();
        let rt = TuiRuntime {
            task_svc: Arc::new(crate::service::TaskService::new(db.clone(), runner.clone())),
            epic_svc: Arc::new(crate::service::EpicService::new(db.clone())),
            todo_svc: Arc::new(crate::service::TodoService::new(Arc::new(
                Database::open_in_memory().await.unwrap(),
            )
                as Arc<dyn crate::db::TodoStore>)),
            feed_runner: Some(crate::feed::FeedRunner::new(
                db.clone(),
                feed_tx,
                runner.clone(),
            )),
            feed_invalidate_tx: None,
            learning_svc: Arc::new(crate::service::MockLearningService),
            feed_db: db.clone(),
            database: db,
            msg_tx: tx,
            runner,
            editor_session: Arc::new(Mutex::new(None)),
            emb_svc: EmbeddingService::new_noop(),
            last_change_count: std::sync::atomic::AtomicI64::new(-1),
        };
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
        let (feed_tx, _) = unbounded_channel();
        let rt = TuiRuntime {
            task_svc: Arc::new(crate::service::TaskService::new(db.clone(), runner.clone())),
            epic_svc: Arc::new(crate::service::EpicService::new(db.clone())),
            todo_svc: Arc::new(crate::service::TodoService::new(Arc::new(
                Database::open_in_memory().await.unwrap(),
            )
                as Arc<dyn crate::db::TodoStore>)),
            feed_runner: Some(crate::feed::FeedRunner::new(
                db.clone(),
                feed_tx,
                runner.clone(),
            )),
            feed_invalidate_tx: None,
            learning_svc: Arc::new(crate::service::MockLearningService),
            feed_db: db.clone(),
            database: db.clone(),
            msg_tx: tx,
            runner,
            editor_session: Arc::new(Mutex::new(None)),
            emb_svc: EmbeddingService::new_noop(),
            last_change_count: std::sync::atomic::AtomicI64::new(-1),
        };
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
            EditKind::TaskEdit(task.clone()),
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
        let (feed_tx, _) = unbounded_channel();
        let rt = TuiRuntime {
            task_svc: Arc::new(crate::service::TaskService::new(db.clone(), runner.clone())),
            epic_svc: Arc::new(crate::service::EpicService::new(db.clone())),
            todo_svc: Arc::new(crate::service::TodoService::new(Arc::new(
                Database::open_in_memory().await.unwrap(),
            )
                as Arc<dyn crate::db::TodoStore>)),
            feed_runner: Some(crate::feed::FeedRunner::new(
                db.clone(),
                feed_tx,
                runner.clone(),
            )),
            feed_invalidate_tx: None,
            learning_svc: Arc::new(crate::service::MockLearningService),
            feed_db: db.clone(),
            database: db.clone(),
            msg_tx: tx,
            runner,
            editor_session: Arc::new(Mutex::new(None)),
            emb_svc: EmbeddingService::new_noop(),
            last_change_count: std::sync::atomic::AtomicI64::new(-1),
        };
        let mut app = App::new(vec![task.clone()]);

        let edited_text = "--- TITLE ---\n\n\
            --- URL ---\nhttps://github.com/o/r/pull/9\n\
            --- URL_TYPE ---\npr\n";

        rt.exec_finalize_editor_result(
            &mut app,
            EditKind::TaskEdit(task.clone()),
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
        let (feed_tx, _) = unbounded_channel();
        let rt = TuiRuntime {
            task_svc: Arc::new(crate::service::TaskService::new(db.clone(), runner.clone())),
            epic_svc: Arc::new(crate::service::EpicService::new(db.clone())),
            todo_svc: Arc::new(crate::service::TodoService::new(Arc::new(
                Database::open_in_memory().await.unwrap(),
            )
                as Arc<dyn crate::db::TodoStore>)),
            feed_runner: Some(crate::feed::FeedRunner::new(
                db.clone(),
                feed_tx,
                runner.clone(),
            )),
            feed_invalidate_tx: None,
            learning_svc: Arc::new(crate::service::MockLearningService),
            feed_db: db.clone(),
            database: db.clone(),
            msg_tx: tx,
            runner,
            editor_session: Arc::new(Mutex::new(None)),
            emb_svc: EmbeddingService::new_noop(),
            last_change_count: std::sync::atomic::AtomicI64::new(-1),
        };
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
            EditKind::TaskEdit(task.clone()),
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
        let (feed_tx, _) = unbounded_channel();
        let rt = TuiRuntime {
            task_svc: Arc::new(crate::service::TaskService::new(db.clone(), runner.clone())),
            epic_svc: Arc::new(crate::service::EpicService::new(db.clone())),
            todo_svc: Arc::new(crate::service::TodoService::new(Arc::new(
                Database::open_in_memory().await.unwrap(),
            )
                as Arc<dyn crate::db::TodoStore>)),
            feed_runner: Some(crate::feed::FeedRunner::new(
                db.clone(),
                feed_tx,
                runner.clone(),
            )),
            feed_invalidate_tx: None,
            learning_svc: Arc::new(crate::service::MockLearningService),
            feed_db: db.clone(),
            database: db.clone(),
            msg_tx: tx,
            runner,
            editor_session: Arc::new(Mutex::new(None)),
            emb_svc: EmbeddingService::new_noop(),
            last_change_count: std::sync::atomic::AtomicI64::new(-1),
        };
        let mut app = App::new(vec![task.clone()]);

        // PLAN section present but empty → clear.
        let edited_text = "--- TITLE ---\n\n--- PLAN ---\n\n";
        rt.exec_finalize_editor_result(
            &mut app,
            EditKind::TaskEdit(task.clone()),
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
        let (feed_tx, _) = unbounded_channel();
        let rt = TuiRuntime {
            task_svc: Arc::new(crate::service::TaskService::new(db.clone(), runner.clone())),
            epic_svc: Arc::new(crate::service::EpicService::new(db.clone())),
            todo_svc: Arc::new(crate::service::TodoService::new(Arc::new(
                Database::open_in_memory().await.unwrap(),
            )
                as Arc<dyn crate::db::TodoStore>)),
            feed_runner: Some(crate::feed::FeedRunner::new(
                db.clone(),
                feed_tx,
                runner.clone(),
            )),
            feed_invalidate_tx: None,
            learning_svc: Arc::new(crate::service::MockLearningService),
            feed_db: db.clone(),
            database: db.clone(),
            msg_tx: tx,
            runner,
            editor_session: Arc::new(Mutex::new(None)),
            emb_svc: EmbeddingService::new_noop(),
            last_change_count: std::sync::atomic::AtomicI64::new(-1),
        };
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
            EditKind::TaskEdit(task.clone()),
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
        let (feed_tx, _) = unbounded_channel();
        let rt = TuiRuntime {
            task_svc: Arc::new(crate::service::TaskService::new(db.clone(), runner.clone())),
            epic_svc: Arc::new(crate::service::EpicService::new(db.clone())),
            todo_svc: Arc::new(crate::service::TodoService::new(Arc::new(
                Database::open_in_memory().await.unwrap(),
            )
                as Arc<dyn crate::db::TodoStore>)),
            feed_runner: Some(crate::feed::FeedRunner::new(
                db.clone(),
                feed_tx,
                runner.clone(),
            )),
            feed_invalidate_tx: None,
            learning_svc: Arc::new(crate::service::MockLearningService),
            feed_db: db.clone(),
            database: db.clone(),
            msg_tx: tx,
            runner,
            editor_session: Arc::new(Mutex::new(None)),
            emb_svc: EmbeddingService::new_noop(),
            last_change_count: std::sync::atomic::AtomicI64::new(-1),
        };
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
            EditKind::TaskEdit(task.clone()),
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
        let (feed_tx, _) = unbounded_channel();
        let rt = TuiRuntime {
            task_svc: Arc::new(crate::service::TaskService::new(db.clone(), runner.clone())),
            epic_svc: Arc::new(crate::service::EpicService::new(db.clone())),
            todo_svc: Arc::new(crate::service::TodoService::new(Arc::new(
                Database::open_in_memory().await.unwrap(),
            )
                as Arc<dyn crate::db::TodoStore>)),
            feed_runner: Some(crate::feed::FeedRunner::new(
                db.clone(),
                feed_tx,
                runner.clone(),
            )),
            feed_invalidate_tx: None,
            learning_svc: Arc::new(crate::service::MockLearningService),
            feed_db: db.clone(),
            database: db.clone(),
            msg_tx: tx,
            runner,
            editor_session: Arc::new(Mutex::new(None)),
            emb_svc: EmbeddingService::new_noop(),
            last_change_count: std::sync::atomic::AtomicI64::new(-1),
        };
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
            EditKind::TaskEdit(task.clone()),
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
        let (feed_tx, _) = unbounded_channel();
        let rt = TuiRuntime {
            task_svc: Arc::new(crate::service::TaskService::new(db.clone(), runner.clone())),
            epic_svc: Arc::new(crate::service::EpicService::new(db.clone())),
            todo_svc: Arc::new(crate::service::TodoService::new(Arc::new(
                Database::open_in_memory().await.unwrap(),
            )
                as Arc<dyn crate::db::TodoStore>)),
            feed_runner: Some(crate::feed::FeedRunner::new(
                db.clone(),
                feed_tx,
                runner.clone(),
            )),
            feed_invalidate_tx: None,
            learning_svc: Arc::new(crate::service::MockLearningService),
            feed_db: db.clone(),
            database: db.clone(),
            msg_tx: tx,
            runner,
            editor_session: Arc::new(Mutex::new(None)),
            emb_svc: EmbeddingService::new_noop(),
            last_change_count: std::sync::atomic::AtomicI64::new(-1),
        };
        let mut app = App::new(vec![task.clone()]);

        rt.exec_finalize_editor_result(
            &mut app,
            EditKind::TaskEdit(task.clone()),
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
