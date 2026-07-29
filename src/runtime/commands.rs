use std::sync::Arc;

/// Full `Command` match dispatch — one entry per variant, in `Command` enum order.
///
/// Returns any follow-on commands that should be added to the execution queue.
/// The caller (`execute_commands`) extends the queue with the returned vec.
pub(super) async fn dispatch(
    command: super::Command,
    app: &mut super::App,
    rt: &super::TuiRuntime,
) -> Vec<super::Command> {
    use super::Command::*;
    match command {
        Task(cmd) => dispatch_task(rt, app, cmd).await,
        Editor(cmd) => dispatch_editor(rt, app, cmd).await,
        Feed(cmd) => {
            dispatch_feed(rt, cmd);
            vec![]
        }
        SaveRepoPath(path) => {
            rt.exec_save_repo_path(app, path).await;
            vec![]
        }
        SaveBaseBranch(repo_path, branch) => {
            rt.exec_save_base_branch(app, repo_path, branch).await;
            vec![]
        }
        MainSession(cmd) => {
            dispatch_main_session(rt, app, cmd).await;
            vec![]
        }
        // Epic commands
        Epic(cmd) => {
            dispatch_epic(rt, app, cmd).await;
            vec![]
        }
        System(cmd) => {
            dispatch_system(rt, cmd);
            vec![]
        }
        // Settings
        PersistSetting { key, value } => {
            rt.exec_persist_setting(app, &key, value).await;
            vec![]
        }
        PersistStringSetting { key, value } => {
            rt.exec_persist_string_setting(app, &key, &value).await;
            vec![]
        }
        RepoFilter(cmd) => {
            dispatch_repo_filter(rt, app, cmd).await;
            vec![]
        }
        // PR commands (creation is agent-driven via the /wrap-up skill)
        Pr(cmd) => {
            dispatch_pr(rt, cmd);
            vec![]
        }
        // Split mode
        Split(cmd) => {
            dispatch_split(rt, app, cmd);
            vec![]
        }
        // Tips
        Tips(cmd) => {
            dispatch_tips(rt, cmd).await;
            vec![]
        }
        Learning(cmd) => {
            dispatch_learning(rt, app, cmd).await;
            vec![]
        }
        RecordUsageEvent(event) => {
            let db = Arc::clone(&rt.database);
            tokio::spawn(async move {
                let _ = db.record_usage_event(&event).await;
            });
            vec![]
        }
        ManagedFeed(cmd) => {
            dispatch_managed_feed(rt, app, cmd).await;
            vec![]
        }
        Todo(cmd) => {
            dispatch_todo(rt, app, cmd).await;
            vec![]
        }
    }
}

/// Per-domain dispatcher for [`crate::tui::commands::ManagedFeedCommand`] variants.
async fn dispatch_managed_feed(
    rt: &super::TuiRuntime,
    app: &mut super::App,
    cmd: crate::tui::commands::ManagedFeedCommand,
) {
    use crate::tui::commands::ManagedFeedCommand::*;
    match cmd {
        PersistConfig {
            reviews_command,
            reviews_interval_secs,
            cve_command,
            cve_interval_secs,
        } => {
            rt.exec_persist_managed_feed_config(
                app,
                reviews_command,
                reviews_interval_secs,
                cve_command,
                cve_interval_secs,
            )
            .await
        }
        ProvisionAndRefresh => rt.exec_provision_and_refresh(app).await,
    }
}

fn dispatch_split(
    rt: &super::TuiRuntime,
    _app: &mut super::App,
    cmd: crate::tui::commands::SplitCommand,
) {
    use crate::tui::commands::SplitCommand::*;
    match cmd {
        Enter => drop(rt.exec_enter_split_mode()),
        EnterWithTask { task_id, window } => {
            drop(rt.exec_enter_split_mode_with_task(task_id, &window))
        }
        Exit {
            pane_id,
            restore_window,
        } => drop(rt.exec_exit_split_mode(&pane_id, restore_window.as_deref())),
        Swap {
            task_id,
            new_window,
            old_pane_id,
            old_window,
        } => drop(rt.exec_swap_split_pane(
            task_id,
            &new_window,
            old_pane_id.as_deref(),
            old_window.as_deref(),
        )),
        FocusPane { pane_id } => drop(rt.exec_focus_split_pane(pane_id)),
        CheckPaneExists { pane_id } => drop(rt.exec_check_split_pane(&pane_id)),
        RespawnPane { pane_id } => drop(rt.exec_respawn_split_pane(&pane_id)),
    }
}

async fn dispatch_main_session(
    rt: &super::TuiRuntime,
    app: &mut super::App,
    cmd: crate::tui::commands::MainSessionCommand,
) {
    use crate::tui::commands::MainSessionCommand::*;
    match cmd {
        Open => rt.exec_open_main_session(app).await,
        Create => rt.exec_create_main_session(app).await,
        CheckLiveness => drop(rt.exec_check_main_session_liveness()),
    }
}

async fn dispatch_tips(rt: &super::TuiRuntime, cmd: crate::tui::commands::TipsCommand) {
    use crate::tui::commands::TipsCommand::*;
    match cmd {
        SaveState {
            seen_up_to,
            show_mode,
        } => rt.exec_save_tips_state(seen_up_to, show_mode).await,
    }
}

async fn dispatch_learning(
    rt: &super::TuiRuntime,
    app: &mut super::App,
    cmd: crate::tui::commands::LearningCommand,
) {
    use crate::tui::commands::LearningCommand::*;
    match cmd {
        Load => rt.exec_load_learnings(app).await,
        Archive(id) => rt.exec_archive_learning(app, id).await,
        Reject(id) => rt.exec_reject_learning(app, id).await,
        ArchiveStale => rt.exec_archive_stale_learnings().await,
    }
}

/// Per-domain dispatcher for [`crate::tui::commands::TaskCommand`] variants.
async fn dispatch_task(
    rt: &super::TuiRuntime,
    app: &mut super::App,
    cmd: crate::tui::commands::TaskCommand,
) -> Vec<super::Command> {
    use crate::tui::commands::TaskCommand::*;
    match cmd {
        Persist(task) => {
            rt.exec_persist_task(app, task).await;
            vec![]
        }
        Insert { draft, epic_id } => {
            rt.exec_insert_task(app, draft, epic_id).await;
            vec![]
        }
        Delete(id) => {
            rt.exec_delete_task(app, id).await;
            vec![]
        }
        DispatchAgent { task, mode } => {
            rt.exec_dispatch_agent(task, mode);
            vec![]
        }
        TrustAndDispatch { task, mode } => {
            let id = task.id;
            let repo_path = task.repo_path.clone();
            let trust_result =
                tokio::task::spawn_blocking(move || crate::dispatch::trust_repo(&repo_path))
                    .await
                    .unwrap_or_else(|e| Err(anyhow::anyhow!("trust_repo panicked: {e}")));

            match trust_result {
                Ok(()) => {
                    rt.exec_dispatch_agent(task, mode);
                }
                Err(e) => {
                    app.update(crate::tui::Message::Task(
                        crate::tui::messages::TaskMessage::DispatchFailed(id),
                    ));
                    app.update(crate::tui::Message::System(
                        crate::tui::messages::SystemMessage::Error(format!(
                            "Failed to trust repo: {e:#}"
                        )),
                    ));
                }
            }
            vec![]
        }
        CheckTrustAndDispatch {
            id,
            repo_path,
            mode,
        } => {
            let (repo_path, trust_result) = tokio::task::spawn_blocking(move || {
                let result = crate::dispatch::is_repo_trusted(&repo_path);
                (repo_path, result)
            })
            .await
            .unwrap_or_else(|e| {
                (
                    String::new(),
                    Err(anyhow::anyhow!("is_repo_trusted panicked: {e}")),
                )
            });
            match trust_result {
                Ok(true) => app.update(crate::tui::Message::Task(
                    crate::tui::messages::TaskMessage::Dispatch(id, mode),
                )),
                Ok(false) => app.update(crate::tui::Message::Task(
                    crate::tui::messages::TaskMessage::TrustCheckUntrusted {
                        id,
                        mode,
                        repo_path,
                    },
                )),
                Err(e) => app.update(crate::tui::Message::System(
                    crate::tui::messages::SystemMessage::StatusInfo(format!(
                        "Trust check failed: {e}"
                    )),
                )),
            }
        }
        Cleanup {
            id,
            repo_path,
            worktree,
            tmux_window,
        } => {
            rt.exec_cleanup(id, repo_path, worktree, tmux_window).await;
            vec![]
        }
        Finish {
            id,
            repo_path,
            branch,
            base_branch,
            worktree,
            tmux_window,
        } => {
            rt.exec_finish(id, repo_path, branch, base_branch, worktree, tmux_window)
                .await;
            vec![]
        }
        CheckWindow { id, window } => {
            drop(rt.exec_check_window(id, window));
            vec![]
        }
        BatchCheckWindows { windows } => {
            drop(rt.exec_batch_check_windows(windows));
            vec![]
        }
        Resume { task } => {
            rt.exec_resume(task);
            vec![]
        }
        JumpToTmux { window } => {
            rt.exec_jump_to_tmux(app, window);
            vec![]
        }
        QuickDispatch { draft, epic_id } => {
            // Mirrors CheckTrustAndDispatch: a fresh worktree launched into an
            // untrusted repo would otherwise stall on Claude Code's own
            // interactive trust prompt (see src/dispatch/trust.rs), silently
            // defeating "quick" dispatch's unattended, immediate contract.
            let repo_path = draft.repo_path.clone();
            let trust_result =
                tokio::task::spawn_blocking(move || crate::dispatch::is_repo_trusted(&repo_path))
                    .await
                    .unwrap_or_else(|e| Err(anyhow::anyhow!("is_repo_trusted panicked: {e}")));
            match trust_result {
                Ok(true) => {
                    rt.exec_quick_dispatch(app, draft, epic_id).await;
                    vec![]
                }
                Ok(false) => app.update(crate::tui::Message::Task(
                    crate::tui::messages::TaskMessage::TrustCheckUntrustedForQuickDispatch {
                        draft,
                        epic_id,
                    },
                )),
                Err(e) => app.update(crate::tui::Message::System(
                    crate::tui::messages::SystemMessage::StatusInfo(format!(
                        "Trust check failed: {e}"
                    )),
                )),
            }
        }
        TrustAndQuickDispatch { draft, epic_id } => {
            let repo_path = draft.repo_path.clone();
            let trust_result =
                tokio::task::spawn_blocking(move || crate::dispatch::trust_repo(&repo_path))
                    .await
                    .unwrap_or_else(|e| Err(anyhow::anyhow!("trust_repo panicked: {e}")));
            match trust_result {
                Ok(()) => {
                    rt.exec_quick_dispatch(app, draft, epic_id).await;
                }
                Err(e) => {
                    app.update(crate::tui::Message::System(
                        crate::tui::messages::SystemMessage::Error(format!(
                            "Failed to trust repo: {e:#}"
                        )),
                    ));
                }
            }
            vec![]
        }
        KillTmuxWindow { window } => {
            drop(rt.exec_kill_tmux_window(window));
            vec![]
        }
        PatchSubStatus { id, sub_status } => {
            rt.exec_patch_sub_status(app, id, sub_status).await;
            vec![]
        }
        MoveToEpic { id, new_epic } => rt.exec_move_task_to_epic(app, id, new_epic).await,
        SeedActivity { id, at } => {
            rt.exec_seed_activity(app, id, at).await;
            vec![]
        }
        BatchPatchSubStatus { updates } => {
            rt.exec_batch_patch_sub_status(app, updates).await;
            vec![]
        }
        RefreshFromDb => rt.exec_refresh_from_db(app).await,
    }
}

/// Per-domain dispatcher for [`crate::tui::commands::EpicCommand`] variants.
async fn dispatch_epic(
    rt: &super::TuiRuntime,
    app: &mut super::App,
    cmd: crate::tui::commands::EpicCommand,
) {
    use crate::tui::commands::EpicCommand::*;
    match cmd {
        Insert(draft) => {
            rt.exec_insert_epic(app, draft.title, draft.description, draft.parent_epic_id)
                .await
        }
        Delete(id) => rt.exec_delete_epic(app, id).await,
        Persist {
            id,
            status,
            sort_order,
        } => rt.exec_persist_epic(app, id, status, sort_order).await,
        ToggleAutoDispatch { id, auto_dispatch } => {
            rt.exec_toggle_epic_auto_dispatch(app, id, auto_dispatch)
                .await
        }
        ToggleGroupByRepo { id, group_by_repo } => {
            rt.exec_toggle_epic_group_by_repo(app, id, group_by_repo)
                .await
        }
        RefreshFromDb => rt.exec_refresh_epics_from_db(app).await,
        Reparent { id, new_parent } => rt.exec_reparent_epic(app, id, new_parent).await,
    }
}

/// Per-domain dispatcher for [`crate::tui::commands::SystemCommand`] variants.
fn dispatch_system(rt: &super::TuiRuntime, cmd: crate::tui::commands::SystemCommand) {
    use crate::tui::commands::SystemCommand::*;
    match cmd {
        SendNotification {
            title,
            body,
            urgent,
        } => drop(rt.exec_send_notification(&title, &body, urgent)),
        OpenInBrowser { url } => drop(rt.exec_open_in_browser(url)),
    }
}

/// Per-domain dispatcher for [`crate::tui::commands::PrCommand`] variants.
fn dispatch_pr(rt: &super::TuiRuntime, cmd: crate::tui::commands::PrCommand) {
    use crate::tui::commands::PrCommand::*;
    match cmd {
        CheckStatus { id, url } => drop(rt.exec_check_pr_status(id, url)),
    }
}

/// Per-domain dispatcher for [`crate::tui::commands::FeedCommand`] variants.
fn dispatch_feed(rt: &super::TuiRuntime, cmd: crate::tui::commands::FeedCommand) {
    use crate::tui::commands::FeedCommand::*;
    match cmd {
        TriggerEpic {
            epic_id,
            epic_title,
            feed_command,
            group_by_repo,
        } => rt.exec_trigger_epic_feed(epic_id, epic_title, feed_command, group_by_repo),
    }
}

/// Per-domain dispatcher for [`crate::tui::commands::RepoFilterCommand`] variants.
async fn dispatch_repo_filter(
    rt: &super::TuiRuntime,
    app: &mut super::App,
    cmd: crate::tui::commands::RepoFilterCommand,
) {
    use crate::tui::commands::RepoFilterCommand::*;
    match cmd {
        PersistFilterPreset {
            name,
            repo_paths,
            mode,
        } => {
            rt.exec_persist_filter_preset(app, &name, &repo_paths, mode.as_str())
                .await
        }
        DeleteFilterPreset(name) => rt.exec_delete_filter_preset(app, &name).await,
        DeleteRepoPath(path) => rt.exec_delete_repo_path(app, &path).await,
    }
}

/// Per-domain dispatcher for [`crate::tui::commands::EditorCommand`] variants.
///
/// `FinalizeResult` re-enters the queue: post-edit `app.update(...)` calls
/// inside `exec_finalize_editor_result` can produce follow-on commands
/// (DB persistence, status messages), which the runtime queue then drains.
async fn dispatch_editor(
    rt: &super::TuiRuntime,
    app: &mut super::App,
    cmd: crate::tui::commands::EditorCommand,
) -> Vec<super::Command> {
    use crate::tui::commands::EditorCommand::*;
    match cmd {
        PopOut(kind) => {
            rt.exec_pop_out_editor(app, kind);
            vec![]
        }
        FinalizeResult { kind, outcome } => {
            rt.exec_finalize_editor_result(app, kind, outcome).await
        }
    }
}

/// Per-domain dispatcher for [`crate::tui::commands::TodoCommand`] variants.
async fn dispatch_todo(
    rt: &super::TuiRuntime,
    app: &mut super::App,
    cmd: crate::tui::commands::TodoCommand,
) {
    use crate::tui::commands::TodoCommand::*;
    match cmd {
        Load => rt.exec_load_todos(app).await,
        Create {
            title,
            linked,
            reopen,
        } => rt.exec_create_todo(app, title, linked, reopen).await,
        Update { id, update } => {
            if let Err(e) = rt.todo_svc.update_todo(id, update).await {
                tracing::warn!("update todo failed: {e}");
            }
        }
        Delete(id) => {
            if let Err(e) = rt.todo_svc.delete_todo(id).await {
                tracing::warn!("delete todo failed: {e}");
            }
        }
        ClearDone => {
            if let Err(e) = rt.todo_svc.clear_done().await {
                tracing::warn!("clear done failed: {e}");
            }
        }
        LoadCount => rt.exec_load_todo_count(app).await,
    }
}
