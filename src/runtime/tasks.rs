use super::*;

/// Spawn a blocking dispatch call, sending `Dispatched`/`DispatchFailed`/`Error`
/// back via `msg_tx`. Handles `catch_unwind` and panic-string extraction so
/// callers only supply the label, `switch_focus` flag, and the dispatch closure.
///
/// `pub(super)` so `runtime::tests` can drive all three arms — in particular the
/// `Err(panic)` arm, which no production caller can trigger on demand.
pub(super) fn run_blocking_dispatch(
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
    inputs: dispatch::DispatchInputs,
    msg_tx: tokio::sync::mpsc::UnboundedSender<Message>,
) {
    let id = task.id;
    run_blocking_dispatch(id, "Quick dispatch", true, msg_tx, move || {
        let dispatch::DispatchInputs { epic_ctx, injected } = inputs;
        let injections = dispatch::LearningInjections::from(injected.as_slice());
        dispatch::quick_dispatch_agent(&task, &*runner, epic_ctx.as_ref(), &injections)
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
            auto_run_plan: false,
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
                    auto_run_plan: false,
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
        // Claim before provisioning, exactly as exec_dispatch_agent does. This
        // task was created moments ago, so the claim is effectively uncontended
        // — it is taken anyway so that the Running write, the activity stamp and
        // the release-on-failure unwind are identical across entry points rather
        // than quick dispatch keeping a second, subtly different shape.
        if !self.claim_for_dispatch(task.id).await {
            return;
        }
        let db = Arc::clone(&self.database);
        let emb_svc = Arc::clone(&self.emb_svc);
        let msg_tx = self.msg_tx.clone();
        let runner = Arc::clone(&self.runner);

        // Spawn a background task so the TUI command loop is never blocked
        // waiting for the embedding thread (which may be busy with index_repo).
        tokio::spawn(async move {
            let inputs = dispatch::prepare_inputs(&*db, &task, &emb_svc).await;
            run_quick_dispatch(task, runner, inputs, msg_tx);
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
        match self.task_svc.update_task(p).await {
            Ok(result) => {
                app.dirty_since_refresh = true;
                self.write_back_task_sort_order(app, result.task_id, result.sort_order_after_write);
            }
            Err(e) => {
                app.update(Message::System(crate::tui::messages::SystemMessage::Error(
                    Self::db_error("persisting task", e),
                )));
            }
        }
    }

    /// Clear a task's subagent entries, draining or not per
    /// [`DrainMode`](crate::models::DrainMode). `SessionStart` also
    /// clears without draining, but via the hook CLI rather than this command —
    /// see `cmd_hook_subagent`. A failed clear is logged, not surfaced — it
    /// degrades to a phantom count, which is recoverable, and an error popup on
    /// a background cleanup would be worse than the drift.
    pub(super) async fn exec_clear_subagents(&self, id: models::TaskId, mode: models::DrainMode) {
        let result = match mode {
            models::DrainMode::Drain => {
                self.task_svc
                    .record_subagent_event(id, models::SubagentEvent::Clear)
                    .await
            }
            models::DrainMode::NoDrain => self.task_svc.clear_subagents_no_drain(id).await,
        };
        if let Err(e) = result {
            tracing::warn!(task_id = id.0, error = %e, "failed to clear subagent entries");
        }
    }

    /// If the write carried a `sort_order`, patch that one field onto
    /// the in-memory task immediately. The service — not the caller — computes
    /// it on a Done transition (`sort_order_for_status_transition`, run inside
    /// `update_task`), so without this the board keeps
    /// whatever the caller's snapshot held until the next refresh ~2s later: a
    /// freshly-completed task renders at the *bottom* of Done, and one that just
    /// left Done stays pinned to the top of the column it landed in.
    ///
    /// The task twin of `write_back_epic_sort_order` (src/runtime/epics.rs),
    /// and identical in the two details that matter. It clones the **live
    /// board task**, not the caller's snapshot: `TaskMessage::Updated`
    /// replaces the board slot wholesale, so splicing a snapshot would
    /// re-impose every field it holds — including hook-owned
    /// `last_pre_tool_use_at` — reintroducing in memory the clobber
    /// `exec_persist_task` deliberately avoids on the DB write. And it bails
    /// when the task is absent from the board, because `handle_task_updated`
    /// *pushes* an unknown id, which would resurrect a ghost card for a task
    /// deleted or archived while this write was in flight.
    ///
    /// Routed through `TaskMessage::Updated` — the same splice
    /// `spawn_refresh_task` uses — rather than reaching into `App.board`
    /// directly: see the "Visibility convention" in docs/conventions.md, only
    /// `crate::tui` code may mutate `App.board`.
    fn write_back_task_sort_order(
        &self,
        app: &mut App,
        task_id: TaskId,
        sort_order_after_write: Option<Option<i64>>,
    ) {
        let Some(new_sort_order) = sort_order_after_write else {
            return;
        };
        let Some(mut task) = app.tasks().iter().find(|t| t.id == task_id).cloned() else {
            return;
        };
        task.sort_order = new_sort_order;
        app.update(Message::Task(crate::tui::messages::TaskMessage::Updated(
            task,
        )));
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

    /// Claim `task` for dispatch, then provision it in the background.
    ///
    /// The claim is the guard, not the caller's snapshot: `handle_dispatch_task`
    /// filters on the board's copy of the status, which can be stale by the time
    /// this runs, and a chain or MCP `dispatch_task` landing in between would
    /// otherwise get the same task provisioned twice
    /// (`DispatchClaimExclusive` in `docs/specs/dispatch.allium`).
    ///
    /// A lost claim reports `DispatchAbandoned` plus an error naming why, so the
    /// spinner never outlives the attempt.
    pub(super) async fn exec_dispatch_agent(&self, task: models::Task, mode: models::DispatchMode) {
        if !self.claim_for_dispatch(task.id).await {
            return;
        }
        let db = Arc::clone(&self.database);
        let emb_svc = Arc::clone(&self.emb_svc);
        let msg_tx = self.msg_tx.clone();
        let runner = Arc::clone(&self.runner);

        // Spawn a background task so the TUI command loop is never blocked
        // waiting for the embedding thread (which may be busy with index_repo).
        tokio::spawn(async move {
            let dispatch::DispatchInputs { epic_ctx, injected } =
                dispatch::prepare_inputs(&*db, &task, &emb_svc).await;
            let label = mode.label();
            let id = task.id;
            tracing::info!(task_id = id.0, label, "dispatching");
            run_blocking_dispatch(id, label, false, msg_tx, move || {
                let injections = dispatch::LearningInjections::from(injected.as_slice());
                match mode {
                    models::DispatchMode::Dispatch => {
                        dispatch::dispatch_agent(&task, &*runner, epic_ctx.as_ref(), &injections)
                    }
                    models::DispatchMode::Research => {
                        dispatch::research_agent(&task, &*runner, epic_ctx.as_ref())
                    }
                }
            });
        });
    }

    /// Take the pre-provisioning claim. Returns whether the caller may provision.
    ///
    /// Shared by [`Self::exec_dispatch_agent`] and [`Self::exec_quick_dispatch`]
    /// so both entry points claim identically.
    async fn claim_for_dispatch(&self, id: TaskId) -> bool {
        // Both failure modes are `DispatchAbandoned`: the claim is a single
        // statement, so neither a lost claim nor an errored one leaves this
        // caller holding anything to release.
        let reason = match self.task_svc.claim_backlog_task(id).await {
            Ok(true) => return true,
            Ok(false) => format!("Task #{} was already dispatched by something else", id.0),
            Err(e) => Self::db_error("claiming task for dispatch", e),
        };
        let _ = self.msg_tx.send(Message::Task(
            crate::tui::messages::TaskMessage::DispatchAbandoned(id),
        ));
        self.send_system_error(reason);
        false
    }

    /// Undo a claim whose dispatch never provisioned anything, returning the
    /// task to `Backlog`. Driven by `TaskCommand::ReleaseClaim`, which
    /// `handle_dispatch_failed` emits. The dispatch watchdog deliberately does
    /// not — see `tick_dispatching`.
    ///
    /// Conditional in the service layer: a task that *was* provisioned, or that
    /// moved on meanwhile, is left alone. `Ok(false)` is therefore an expected
    /// outcome, not an error — a lost claim releases nothing, and neither does a
    /// dispatch that failed after provisioning.
    pub(super) async fn exec_release_claim(&self, app: &mut App, id: TaskId) {
        if let Err(e) = self.task_svc.release_claim(id).await {
            app.update(Message::System(crate::tui::messages::SystemMessage::Error(
                Self::db_error("releasing dispatch claim", e),
            )));
        }
    }

    pub(super) fn exec_check_window(
        &self,
        id: TaskId,
        window: String,
    ) -> tokio::task::JoinHandle<()> {
        let tx = self.msg_tx.clone();
        let runner = self.runner.clone();

        tokio::task::spawn_blocking(move || {
            // A tmux query failure is treated as "still present" (see
            // `has_window_or_assume_present`) so a transient hiccup never
            // fires WindowGone and gets mistaken for a crashed agent.
            if !tmux::has_window_or_assume_present(&window, &*runner) {
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
    ///
    /// # Why this is the *unguarded* twin of [`Self::exec_refresh_from_db`]
    ///
    /// Both functions do the same two reads, but they sit on opposite sides of
    /// the render thread and are reached for opposite reasons:
    ///
    /// - `exec_refresh_from_db` is the **command-queue** path. It runs inline on
    ///   the render thread (see the command-queue section of
    ///   `docs/architecture.md`) and fires speculatively — every 5 ticks as a
    ///   catch-all, whether or not anything changed. Its `get_total_changes`
    ///   watermark exists to make that speculative case free: skipping the read
    ///   is worth two extra writer round-trips.
    /// - `do_full_board_refresh` is the **detached** path, always reached from a
    ///   `tokio::spawn` and only *after* something already told us the board
    ///   moved (an MCP notification, or a targeted refresh whose task/epic had
    ///   vanished). A watermark check here would cost the same two writer
    ///   round-trips to answer a question we already know the answer to.
    ///
    /// So the guard is deliberately not unified: it is a property of *why* the
    /// refresh was requested, not of the reads themselves.
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

    /// Full board refresh on the command-queue path, i.e. inline on the render
    /// thread. See [`Self::do_full_board_refresh`] for why that twin has no
    /// watermark guard and this one does.
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

    /// Clear a task's `worktree` and `tmux_window` columns: the DB half of
    /// `CleanupFollowUp::ClearPointer`, i.e. the write a teardown earns by
    /// *succeeding*, applied from `handle_cleanup_succeeded`.
    pub(super) async fn clear_worktree_pointer(&self, id: TaskId) {
        if let Err(e) = self
            .task_svc
            .update_task(
                crate::service::UpdateTaskParams::for_task(id)
                    .worktree(FieldUpdate::Clear)
                    .tmux_window(FieldUpdate::Clear),
            )
            .await
        {
            self.send_system_error(format!("Clearing the worktree pointer failed: {e:#}"));
        }
    }

    /// Tear a task's live resources down, then report the outcome so the caller's
    /// follow-up write can depend on it.
    ///
    /// Kills the tmux window, removes the git worktree, deletes the branch
    /// best-effort. This is `TaskTeardown` from the head of the archive section
    /// of `docs/specs/tasks.allium`, and step 2 is unconditional: there is
    /// deliberately no shared-worktree check, because no two tasks can name the
    /// same worktree. The argument is `WorktreeIsNeverShared` in that spec — read
    /// it there before adding a guard here.
    ///
    /// `worktree` is optional because a task can own a window and no worktree, and
    /// still owes step 1 (`TeardownIsOwedWheneverThereIsSomethingToRelease`). This
    /// function does not branch on it — `dispatch::teardown_task` does; the branch
    /// here is only over what a failure means for the follow-up.
    ///
    /// The removal shells out to git, so it runs detached — awaiting it here
    /// would stall the command drain (and with it input and rendering) for as
    /// long as git takes. That is why `follow_up` travels *with* the cleanup and
    /// is applied on its completion path: nothing that forgets the worktree path
    /// may run beside a removal that might not have happened
    /// (`WorktreeReleaseIsGated` in docs/specs/tasks.allium).
    ///
    /// Returns the removal's handle. Callers `drop` it; tests await it.
    pub(super) fn exec_cleanup(
        &self,
        id: TaskId,
        repo_path: String,
        worktree: Option<String>,
        tmux_window: Option<String>,
        follow_up: crate::tui::commands::CleanupFollowUp,
    ) -> tokio::task::JoinHandle<()> {
        let tx = self.msg_tx.clone();
        let runner = self.runner.clone();

        tokio::task::spawn_blocking(move || {
            let result = dispatch::teardown_task(
                &repo_path,
                worktree.as_deref(),
                tmux_window.as_deref(),
                &*runner,
            );
            let msg = match (result, worktree) {
                (Ok(()), _) => {
                    crate::tui::messages::TaskMessage::CleanupSucceeded { id, follow_up }
                }
                // No worktree means nothing to release, so the gate does not
                // apply and the follow-up still stands: withholding it would
                // strand the row rather than protect a resource. Warn-logged
                // only, matching the feed wrapper.
                (Err(e), None) => {
                    tracing::warn!(
                        task_id = id.0,
                        "tmux window teardown failed for a task with no worktree: {e:#}"
                    );
                    crate::tui::messages::TaskMessage::CleanupSucceeded { id, follow_up }
                }
                (Err(e), Some(worktree)) => {
                    let error = format!("{e:#}");
                    // The only durable record of a failed teardown. Without it a
                    // leftover worktree cannot be attributed to anything after
                    // the fact — see
                    // docs/plans/2026-08-11-3897-worktree-cleanup-investigation.md.
                    tracing::error!(
                        task_id = id.0,
                        worktree_path = %worktree,
                        %error,
                        "worktree cleanup failed, worktree left on disk"
                    );
                    crate::tui::messages::TaskMessage::CleanupFailed {
                        id,
                        worktree,
                        error,
                    }
                }
            };
            let _ = tx.send(Message::Task(msg));
        })
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
