use super::*;

/// Spawn a blocking dispatch call, sending `Dispatched`/`DispatchFailed`/`Error`
/// back via `msg_tx`. Handles `catch_unwind` and panic-string extraction so
/// callers only supply the label, `switch_focus` flag, and the dispatch closure.
fn run_blocking_dispatch(
    id: TaskId,
    label: &'static str,
    switch_focus: bool,
    msg_tx: tokio::sync::mpsc::UnboundedSender<Message>,
    f: impl FnOnce() -> anyhow::Result<models::DispatchResult> + Send + 'static,
) {
    tokio::task::spawn_blocking(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        match result {
            Ok(Ok(r)) => {
                let _ = msg_tx.send(Message::Task(
                    crate::tui::messages::TaskMessage::Dispatched {
                        id,
                        worktree: r.worktree_path,
                        tmux_window: r.tmux_window,
                        switch_focus,
                    },
                ));
            }
            Ok(Err(e)) => {
                let _ = msg_tx.send(Message::Task(
                    crate::tui::messages::TaskMessage::DispatchFailed(id),
                ));
                let _ = msg_tx.send(Message::System(crate::tui::messages::SystemMessage::Error(
                    format!("{label} failed: {e:#}"),
                )));
            }
            Err(panic) => {
                let detail = panic
                    .downcast_ref::<&'static str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown".to_string());
                tracing::error!(task_id = id.0, label, "dispatch panicked: {detail}");
                let _ = msg_tx.send(Message::Task(
                    crate::tui::messages::TaskMessage::DispatchFailed(id),
                ));
                let _ = msg_tx.send(Message::System(crate::tui::messages::SystemMessage::Error(
                    format!("{label} panicked: {detail}"),
                )));
            }
        }
    });
}

fn run_quick_dispatch(
    task: models::Task,
    runner: Arc<dyn ProcessRunner>,
    epic_ctx: Option<dispatch::EpicContext>,
    injected: Vec<crate::models::Learning>,
    verify_command: Option<String>,
    msg_tx: tokio::sync::mpsc::UnboundedSender<Message>,
) {
    let id = task.id;
    run_blocking_dispatch(id, "Quick dispatch", true, msg_tx, move || {
        let injections = dispatch::LearningInjections::from(injected.as_slice());
        dispatch::quick_dispatch_agent(
            &task,
            &*runner,
            epic_ctx.as_ref(),
            &injections,
            verify_command.as_deref(),
        )
    });
}

impl TuiRuntime {
    pub(super) async fn exec_insert_task(
        &self,
        app: &mut App,
        draft: tui::TaskDraft,
        epic_id: Option<models::EpicId>,
    ) {
        use crate::service::CreateTaskParams;
        let params = CreateTaskParams {
            title: draft.title,
            description: draft.description,
            repo_path: draft.repo_path,
            plan_path: None,
            epic_id,
            sort_order: None,
            tag: draft.tag,
            base_branch: Some(draft.base_branch),
            wrap_up_mode: draft.wrap_up_mode,
        };
        if let Some(task) = self.create_task(app, params).await {
            app.update(Message::Task(crate::tui::messages::TaskMessage::Created {
                task,
            }));
        }
    }

    pub(super) async fn exec_quick_dispatch(
        &self,
        app: &mut App,
        draft: tui::TaskDraft,
        epic_id: Option<models::EpicId>,
    ) {
        use crate::service::CreateTaskParams;
        let repo_path = draft.repo_path.clone();
        let expanded = models::expand_tilde(&repo_path);
        // detect_default_branch calls `git symbolic-ref` synchronously — run it
        // on the blocking thread pool so it never stalls the tokio event loop.
        // Falls back to "main" when origin/HEAD is unavailable.
        let runner_for_branch = Arc::clone(&self.runner);
        let expanded_for_branch = expanded.clone();
        let base_branch = tokio::task::spawn_blocking(move || {
            crate::git::detect_default_branch(&expanded_for_branch, &*runner_for_branch)
        })
        .await
        .unwrap_or_else(|_| "main".to_string());
        let Some(task) = self
            .create_task(
                app,
                CreateTaskParams {
                    title: draft.title,
                    description: draft.description,
                    repo_path: draft.repo_path,
                    plan_path: None,
                    epic_id,
                    sort_order: None,
                    tag: None,
                    base_branch: Some(base_branch),
                    wrap_up_mode: None,
                },
            )
            .await
        else {
            return;
        };
        app.update(Message::Task(crate::tui::messages::TaskMessage::Created {
            task: task.clone(),
        }));
        app.update(Message::Task(
            crate::tui::messages::TaskMessage::MarkDispatching(task.id),
        ));
        let _ = self.database.save_repo_path(&expanded).await;
        let paths = self.database.list_repo_paths().await.unwrap_or_default();
        app.update(Message::RepoPathsUpdated(paths));
        let db = Arc::clone(&self.database);
        let emb_svc = Arc::clone(&self.emb_svc);
        let msg_tx = self.msg_tx.clone();
        let runner = Arc::clone(&self.runner);

        // Spawn a background task so the TUI command loop is never blocked
        // waiting for the embedding thread (which may be busy with index_repo).
        tokio::spawn(async move {
            let epic_ctx = dispatch::EpicContext::from_db(&task, &*db).await;
            let injected = dispatch::build_and_record_injections(&*db, &task, &emb_svc).await;
            let verify_command = dispatch::fetch_verify_command(&*db, &task.repo_path).await;
            run_quick_dispatch(task, runner, epic_ctx, injected, verify_command, msg_tx);
        });
    }

    pub(super) async fn exec_persist_task(&self, app: &mut App, task: models::Task) {
        use crate::service::UpdateTaskParams;
        // `last_pre_tool_use_at` is intentionally omitted: hooks own that
        // column. Writing it here would let a stale in-memory snapshot
        // (e.g. from a tick reclassification or sort_order swap) overwrite
        // a fresher hook write, flipping the task to Stale on the next tick.
        // Backlog→Running seeds go through `SeedActivity` instead.
        let mut p = UpdateTaskParams::for_task(task.id)
            .status(task.status)
            .sub_status(task.sub_status)
            .worktree(option_to_field_update(task.worktree.clone()))
            .tmux_window(option_to_field_update(task.tmux_window.clone()));
        // No UrlUpdate::Clear is emitted for the None branch intentionally: no
        // runtime/persist flow removes a task URL. If that ever changes, emit
        //   p = p.url(crate::service::UrlUpdate::Clear);
        // here so the clear is persisted.
        if let Some(u) = task.url.clone() {
            p = p.url(crate::service::UrlUpdate::Set(u));
        }
        if let Some(so) = task.sort_order {
            p = p.sort_order(so);
        }
        if let Err(e) = self.task_svc.update_task(p).await {
            app.update(Message::System(crate::tui::messages::SystemMessage::Error(
                Self::db_error("persisting task", e),
            )));
        } else {
            app.dirty_since_refresh = true;
        }
    }

    /// Write `last_pre_tool_use_at` for a freshly running task. Used after
    /// Backlog→Running transitions so the tick classifier sees a recent
    /// activity stamp through the ACTIVE_THRESHOLD window before the agent's
    /// first PreToolUse hook fires.
    pub(super) async fn exec_seed_activity(
        &self,
        app: &mut App,
        id: models::TaskId,
        at: chrono::DateTime<chrono::Utc>,
    ) {
        use crate::service::UpdateTaskParams;
        if let Err(e) = self
            .task_svc
            .update_task(UpdateTaskParams::for_task(id).last_pre_tool_use_at(Some(at)))
            .await
        {
            app.update(Message::System(crate::tui::messages::SystemMessage::Error(
                Self::db_error("seeding activity timestamp", e),
            )));
        }
    }

    pub(super) async fn exec_patch_sub_status(
        &self,
        app: &mut App,
        id: models::TaskId,
        sub_status: models::SubStatus,
    ) {
        use crate::service::UpdateTaskParams;
        if let Err(e) = self
            .task_svc
            .update_task(UpdateTaskParams::for_task(id).sub_status(sub_status))
            .await
        {
            app.update(Message::System(crate::tui::messages::SystemMessage::Error(
                Self::db_error("patching sub_status", e),
            )));
        }
    }

    /// Write all pending tick-driven sub_status reclassifications in a single
    /// transaction instead of N individual DB round-trips.
    pub(super) async fn exec_batch_patch_sub_status(
        &self,
        app: &mut App,
        updates: Vec<(models::TaskId, models::SubStatus)>,
    ) {
        if let Err(e) = self.task_svc.batch_patch_sub_status(&updates).await {
            app.update(Message::System(crate::tui::messages::SystemMessage::Error(
                Self::db_error("batch patching sub_status", e),
            )));
        } else {
            app.dirty_since_refresh = true;
        }
    }

    /// Move a task to a different epic (or detach it when `new_epic` is None),
    /// then refresh the board so the new membership and recalculated epic
    /// statuses are reflected. Returns the refresh follow-on commands.
    pub(super) async fn exec_move_task_to_epic(
        &self,
        app: &mut App,
        id: models::TaskId,
        new_epic: Option<models::EpicId>,
    ) -> Vec<Command> {
        if let Err(e) = self.task_svc.move_task_to_epic(id, new_epic).await {
            app.update(Message::System(crate::tui::messages::SystemMessage::Error(
                Self::db_error("moving task to epic", e),
            )));
            return vec![];
        }
        self.exec_refresh_from_db(app).await
    }

    pub(super) async fn exec_delete_task(&self, app: &mut App, id: TaskId) {
        if let Err(e) = self.task_svc.delete_task(id).await {
            app.update(Message::System(crate::tui::messages::SystemMessage::Error(
                Self::db_error("deleting task", e),
            )));
        }
    }

    pub(super) fn exec_dispatch_agent(&self, task: models::Task, mode: models::DispatchMode) {
        let db = Arc::clone(&self.database);
        let emb_svc = Arc::clone(&self.emb_svc);
        let msg_tx = self.msg_tx.clone();
        let runner = Arc::clone(&self.runner);

        // Spawn a background task so the TUI command loop is never blocked
        // waiting for the embedding thread (which may be busy with index_repo).
        tokio::spawn(async move {
            let epic_ctx = dispatch::EpicContext::from_db(&task, &*db).await;
            let injected = dispatch::build_and_record_injections(&*db, &task, &emb_svc).await;
            let verify_command = dispatch::fetch_verify_command(&*db, &task.repo_path).await;
            let label = mode.label();
            let id = task.id;
            tracing::info!(task_id = id.0, label, "dispatching");
            run_blocking_dispatch(id, label, false, msg_tx, move || {
                let injections = dispatch::LearningInjections::from(injected.as_slice());
                match mode {
                    models::DispatchMode::Dispatch => dispatch::dispatch_agent(
                        &task,
                        &*runner,
                        epic_ctx.as_ref(),
                        &injections,
                        verify_command.as_deref(),
                    ),
                    models::DispatchMode::Research => dispatch::research_agent(
                        &task,
                        &*runner,
                        epic_ctx.as_ref(),
                        verify_command.as_deref(),
                    ),
                }
            });
        });
    }

    pub(super) fn exec_check_window(
        &self,
        id: TaskId,
        window: String,
    ) -> tokio::task::JoinHandle<()> {
        let tx = self.msg_tx.clone();
        let runner = self.runner.clone();

        tokio::task::spawn_blocking(move || {
            if let Ok(false) = tmux::has_window(&window, &*runner) {
                let _ = tx.send(Message::Task(
                    crate::tui::messages::TaskMessage::WindowGone(id),
                ));
            }
        })
    }

    /// Check all task windows with a single `tmux list-windows -a` call,
    /// then send `WindowGone` for any task whose window is absent.
    pub(super) fn exec_batch_check_windows(
        &self,
        windows: Vec<(TaskId, String)>,
    ) -> tokio::task::JoinHandle<()> {
        let tx = self.msg_tx.clone();
        let runner = self.runner.clone();

        tokio::task::spawn_blocking(move || {
            let live: std::collections::HashSet<String> =
                match tmux::list_all_window_names(&*runner) {
                    Ok(names) => names.into_iter().collect(),
                    Err(_) => return,
                };
            for (id, window) in windows {
                if !live.contains(&window) {
                    let _ = tx.send(Message::Task(
                        crate::tui::messages::TaskMessage::WindowGone(id),
                    ));
                }
            }
        })
    }

    /// Records `branch` into `repo_path`'s most-recently-used base_branch
    /// history (see docs/specs/dispatch.allium: rule RecordBaseBranch), then
    /// refreshes `app.board.repo_base_branches` from the DB. Mirrors
    /// `exec_save_repo_path`'s upsert-then-refresh shape.
    pub(super) async fn exec_save_base_branch(
        &self,
        app: &mut App,
        repo_path: String,
        branch: String,
    ) {
        if let Err(e) = self.database.record_base_branch(&repo_path, &branch).await {
            app.update(Message::System(crate::tui::messages::SystemMessage::Error(
                Self::db_error("saving base branch", e),
            )));
        }
        match self.database.list_all_base_branches().await {
            Ok(pairs) => {
                app.update(Message::BaseBranchesUpdated(
                    super::group_base_branches_by_repo(pairs),
                ));
            }
            Err(e) => {
                app.update(Message::System(crate::tui::messages::SystemMessage::Error(
                    Self::db_error("listing base branches", e),
                )));
            }
        }
    }

    pub(super) async fn exec_save_repo_path(&self, app: &mut App, path: String) {
        let path = models::expand_tilde(&path);
        if let Err(e) = self.database.save_repo_path(&path).await {
            app.update(Message::System(crate::tui::messages::SystemMessage::Error(
                Self::db_error("saving repo path", e),
            )));
        }
        match self.database.list_repo_paths().await {
            Ok(paths) => {
                app.update(Message::RepoPathsUpdated(paths));
            }
            Err(e) => {
                app.update(Message::System(crate::tui::messages::SystemMessage::Error(
                    Self::db_error("listing repo paths", e),
                )));
            }
        }
    }

    /// Runs the board DB reads (tasks + epics) and sends results via `tx`.
    /// Shared by `spawn_refresh_from_db` and the `None` fallback paths in
    /// `spawn_refresh_task`/`spawn_refresh_epic`.
    async fn do_full_board_refresh(
        db: Arc<dyn crate::db::TaskReadStore>,
        tx: tokio::sync::mpsc::UnboundedSender<Message>,
    ) {
        match db.list_all().await {
            Ok(tasks) => {
                let _ = tx.send(Message::Task(crate::tui::messages::TaskMessage::Refresh(
                    tasks,
                )));
            }
            Err(e) => {
                let _ = tx.send(Message::System(crate::tui::messages::SystemMessage::Error(
                    TuiRuntime::db_error("refreshing tasks", e),
                )));
            }
        }
        match db.list_epics().await {
            Ok(epics) => {
                let _ = tx.send(Message::Epic(crate::tui::messages::EpicMessage::Refresh(
                    epics,
                )));
            }
            Err(e) => {
                let _ = tx.send(Message::System(crate::tui::messages::SystemMessage::Error(
                    TuiRuntime::db_error("refreshing epics", e),
                )));
            }
        }
    }

    /// Spawn the board DB reads (tasks + epics) on a tokio task
    /// and send the results back as messages via `msg_tx`. Returns immediately so
    /// the caller's select! arm never blocks on DB I/O.
    pub(super) fn spawn_refresh_from_db(&self) -> tokio::task::JoinHandle<()> {
        let db = Arc::clone(&self.database);
        let tx = self.msg_tx.clone();
        tokio::spawn(TuiRuntime::do_full_board_refresh(db, tx))
    }

    /// Spawn a single-task reload. Sends `TaskMessage::Updated` on success.
    /// Falls back to a full board refresh if the task is gone.
    pub(super) fn spawn_refresh_task(
        &self,
        task_id: crate::models::TaskId,
    ) -> tokio::task::JoinHandle<()> {
        let db = Arc::clone(&self.database);
        let tx = self.msg_tx.clone();
        tokio::spawn(async move {
            match db.get_task(task_id).await {
                Ok(Some(task)) => {
                    let _ = tx.send(Message::Task(crate::tui::messages::TaskMessage::Updated(
                        task,
                    )));
                }
                Ok(None) => {
                    TuiRuntime::do_full_board_refresh(db, tx).await;
                }
                Err(e) => {
                    let _ = tx.send(Message::System(crate::tui::messages::SystemMessage::Error(
                        TuiRuntime::db_error("refreshing task", e),
                    )));
                }
            }
        })
    }

    /// Spawn an epic + its tasks reload. Falls back to full refresh if epic is gone.
    pub(super) fn spawn_refresh_epic(
        &self,
        epic_id: crate::models::EpicId,
    ) -> tokio::task::JoinHandle<()> {
        let db = Arc::clone(&self.database);
        let tx = self.msg_tx.clone();
        tokio::spawn(async move {
            match db.get_epic(epic_id).await {
                Ok(Some(epic)) => {
                    let _ = tx.send(Message::Epic(crate::tui::messages::EpicMessage::Updated(
                        epic,
                    )));
                    match db.list_tasks_for_epic(epic_id).await {
                        Ok(tasks) => {
                            for task in tasks {
                                let _ = tx.send(Message::Task(
                                    crate::tui::messages::TaskMessage::Updated(task),
                                ));
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(Message::System(
                                crate::tui::messages::SystemMessage::Error(TuiRuntime::db_error(
                                    "listing epic tasks",
                                    e,
                                )),
                            ));
                        }
                    }
                }
                Ok(None) => {
                    TuiRuntime::do_full_board_refresh(db, tx).await;
                }
                Err(e) => {
                    let _ = tx.send(Message::System(crate::tui::messages::SystemMessage::Error(
                        TuiRuntime::db_error("refreshing epic", e),
                    )));
                }
            }
        })
    }

    pub(super) async fn exec_refresh_from_db(&self, app: &mut App) -> Vec<Command> {
        // Watermark guard: skip the full DB read when nothing has changed since
        // the last tick-driven refresh. The change counter is the cumulative
        // INSERT/UPDATE/DELETE count on this connection; it advances on every
        // mutation (hook writes, MCP calls, service operations). Comparing it
        // before and after is safe: if writes race with the read we just do one
        // extra refresh on the next tick, which is harmless.
        let current_changes = self.database.get_total_changes().await.unwrap_or(-1);
        let last = self.last_change_count.load(Ordering::Relaxed);
        if last != -1 && current_changes == last {
            return vec![];
        }

        let mut cmds = Vec::new();
        match self.database.list_all().await {
            Ok(tasks) => {
                cmds = app.update(Message::Task(crate::tui::messages::TaskMessage::Refresh(
                    tasks,
                )));
            }
            Err(e) => {
                app.update(Message::System(crate::tui::messages::SystemMessage::Error(
                    Self::db_error("refreshing tasks", e),
                )));
            }
        }
        self.exec_refresh_epics_from_db(app).await;
        // Snapshot the change counter *after* the refresh so the next tick only
        // re-reads when a new write has occurred after this point.
        let post_changes = self.database.get_total_changes().await.unwrap_or(-1);
        self.last_change_count
            .store(post_changes, Ordering::Relaxed);
        cmds
    }

    pub(super) async fn exec_delete_repo_path(&self, app: &mut App, path: &str) {
        if let Err(e) = self.database.delete_repo_path(path).await {
            app.update(Message::System(crate::tui::messages::SystemMessage::Error(
                Self::db_error("deleting repo path", e),
            )));
            return;
        }
        match self.database.list_repo_paths().await {
            Ok(paths) => {
                app.update(Message::RepoPathsUpdated(paths));
            }
            Err(e) => {
                app.update(Message::System(crate::tui::messages::SystemMessage::Error(
                    Self::db_error("listing repo paths", e),
                )));
            }
        }
        // Refresh presets since delete_repo_path cleans them
        if let Ok(raw) = self.database.list_filter_presets().await {
            let known: HashSet<String> = app.repo_paths().iter().cloned().collect();
            let presets = parse_raw_presets(raw, Some(&known));
            app.update(Message::RepoFilter(
                crate::tui::messages::RepoFilterMessage::PresetsLoaded(presets),
            ));
        }
    }

    /// Detach a task from its worktree and tmux window by clearing both fields
    /// in the DB. Used when a worktree is shared — full cleanup is deferred to
    /// the last task that holds the worktree.
    pub(super) async fn detach_only(&self, id: TaskId) {
        if let Err(e) = self
            .task_svc
            .update_task(
                crate::service::UpdateTaskParams::for_task(id)
                    .worktree(FieldUpdate::Clear)
                    .tmux_window(FieldUpdate::Clear),
            )
            .await
        {
            self.send_system_error(format!("Detach failed: {e:#}"));
        }
    }

    pub(super) async fn exec_cleanup(
        &self,
        id: TaskId,
        repo_path: String,
        worktree: String,
        tmux_window: Option<String>,
    ) {
        let shared = match self
            .database
            .has_other_tasks_with_worktree(&worktree, id)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                self.send_system_error(format!("Cleanup check failed: {e:#}"));
                return;
            }
        };

        if shared {
            tracing::info!(task_id = id.0, "worktree shared, detaching only");
            self.detach_only(id).await;
            return;
        }

        // No other active tasks — full cleanup
        let tx = self.msg_tx.clone();
        let runner = self.runner.clone();

        tokio::task::spawn_blocking(move || {
            if let Err(e) =
                dispatch::cleanup_task(&repo_path, &worktree, tmux_window.as_deref(), &*runner)
            {
                let _ = tx.send(Message::System(crate::tui::messages::SystemMessage::Error(
                    format!("Cleanup failed: {e:#}"),
                )));
            }
        });
    }

    pub(super) async fn exec_finish(
        &self,
        id: TaskId,
        repo_path: String,
        branch: String,
        base_branch: String,
        worktree: String,
        tmux_window: Option<String>,
    ) {
        let shared = match self
            .database
            .has_other_tasks_with_worktree(&worktree, id)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                self.send_system_error(format!("Finish check failed: {e:#}"));
                return;
            }
        };

        if shared {
            tracing::info!(
                task_id = id.0,
                "worktree shared, detaching only (no rebase)"
            );
            self.detach_only(id).await;
            let _ = self.msg_tx.send(Message::Task(
                crate::tui::messages::TaskMessage::FinishComplete(id),
            ));
            return;
        }

        let tx = self.msg_tx.clone();
        let runner = self.runner.clone();

        tokio::task::spawn_blocking(move || {
            match dispatch::finish_task(
                &dispatch::FinishContext {
                    repo_path: &repo_path,
                    worktree: &worktree,
                    branch: &branch,
                    base_branch: &base_branch,
                    tmux_window: tmux_window.as_deref(),
                },
                &*runner,
            ) {
                Ok(()) => {
                    let _ = tx.send(Message::Task(
                        crate::tui::messages::TaskMessage::FinishComplete(id),
                    ));
                }
                Err(e) => {
                    let is_conflict = matches!(e, dispatch::FinishError::RebaseConflict(_));
                    let _ = tx.send(Message::Task(
                        crate::tui::messages::TaskMessage::FinishFailed {
                            id,
                            error: e.to_string(),
                            is_conflict,
                        },
                    ));
                }
            }
        });
    }

    pub(super) fn exec_resume(&self, task: models::Task) {
        let tx = self.msg_tx.clone();
        let id = task.id;
        let worktree_path = task.worktree.clone().unwrap_or_default();
        let runner = self.runner.clone();

        tokio::task::spawn_blocking(move || {
            tracing::info!(task_id = id.0, "resuming task");
            match dispatch::resume_agent(id, &worktree_path, &*runner) {
                Ok(result) => {
                    let _ = tx.send(Message::Task(crate::tui::messages::TaskMessage::Resumed {
                        id,
                        tmux_window: result.tmux_window,
                    }));
                }
                Err(e) => {
                    let _ = tx.send(Message::System(crate::tui::messages::SystemMessage::Error(
                        format!("Resume failed: {e:#}"),
                    )));
                }
            }
        });
    }
}
