use super::*;

impl TuiRuntime {
    pub(super) fn exec_jump_to_tmux(&self, app: &mut App, window: String) {
        if let Err(e) = tmux::select_window(&window, &*self.runner) {
            app.update(Message::System(crate::tui::messages::SystemMessage::Error(
                format!("Jump failed: {e:#}"),
            )));
        }
    }

    /// Handle `:`. Checks whether the main-session window is alive (via
    /// spawn_blocking so the event loop is not stalled) and either jumps to it
    /// or opens the repo picker so the user can (re)select a directory.
    pub(super) async fn exec_open_main_session(&self, app: &mut App) {
        enum OpenResult {
            Jumped,
            NeedsConfig,
            Failed(String),
        }

        let window = dispatch::MAIN_SESSION_WINDOW.to_string();
        let runner = Arc::clone(&self.runner);
        // Both tmux calls (has_window + select_window) are wrapped in a single
        // spawn_blocking so neither stalls the tokio event loop.
        let result = tokio::task::spawn_blocking(move || {
            if dispatch::main_session_window_alive(&*runner) {
                match tmux::select_window(&window, &*runner) {
                    Ok(()) => OpenResult::Jumped,
                    Err(e) => OpenResult::Failed(format!("{e:#}")),
                }
            } else {
                OpenResult::NeedsConfig
            }
        })
        .await
        .unwrap_or(OpenResult::NeedsConfig);

        match result {
            OpenResult::Jumped => {}
            OpenResult::NeedsConfig => {
                app.update(Message::MainSession(
                    crate::tui::messages::MainSessionMessage::Configure,
                ));
            }
            OpenResult::Failed(err) => {
                app.update(Message::System(crate::tui::messages::SystemMessage::Error(
                    format!("Jump to main session failed: {err}"),
                )));
            }
        }
    }

    /// Create a fresh main-session window in the configured directory and jump
    /// to it. The window identity is not persisted — it is a fixed constant
    /// name re-derived via the live tmux check in `exec_open_main_session`.
    pub(super) async fn exec_create_main_session(&self, app: &mut App) {
        let dir = match app.main_session_dir() {
            Some(d) => d.to_string(),
            None => {
                app.update(Message::System(crate::tui::messages::SystemMessage::Error(
                    "Main session directory not configured".to_string(),
                )));
                return;
            }
        };

        let runner = Arc::clone(&self.runner);
        // Both the session-creation and the subsequent window jump are sync —
        // run them together in a single spawn_blocking.
        let result = tokio::task::spawn_blocking(move || {
            let window = dispatch::create_main_session(&dir, &*runner)?;
            tmux::select_window(&window, &*runner)
                .map_err(|e| anyhow::anyhow!("Main session created but jump failed: {e:#}"))
        })
        .await
        .unwrap_or_else(|e| Err(anyhow::anyhow!("Failed to create main session: {e:#}")));

        if let Err(e) = result {
            app.update(Message::System(crate::tui::messages::SystemMessage::Error(
                format!("{e:#}"),
            )));
        }
    }

    /// Poll whether the fixed "dispatch-main" window is alive (a live tmux
    /// has-window check run off the event loop) and report the result via
    /// `MainSessionMessage::LivenessChanged`. Drives the status-bar main-session
    /// badge (docs/specs/dispatch.allium: MainSessionIndicator).
    pub(super) fn exec_check_main_session_liveness(&self) -> tokio::task::JoinHandle<()> {
        let tx = self.msg_tx.clone();
        let runner = Arc::clone(&self.runner);
        tokio::task::spawn_blocking(move || {
            let alive = dispatch::main_session_window_alive(&*runner);
            let _ = tx.send(Message::MainSession(
                crate::tui::messages::MainSessionMessage::LivenessChanged(alive),
            ));
        })
    }

    /// Open a split pane. Results (PaneOpened / StatusInfo) are sent via
    /// `msg_tx` from a `spawn_blocking` closure so the event loop is not stalled.
    pub(super) fn exec_enter_split_mode(&self) -> tokio::task::JoinHandle<()> {
        let tx = self.msg_tx.clone();
        let runner = Arc::clone(&self.runner);
        tokio::task::spawn_blocking(move || {
            let dispatch_pane = match tmux::current_pane_id(&*runner) {
                Ok(id) => id,
                Err(_) => {
                    let _ = tx.send(Message::System(
                        crate::tui::messages::SystemMessage::StatusInfo(
                            "Split mode requires tmux".to_string(),
                        ),
                    ));
                    return;
                }
            };
            match tmux::split_window_horizontal(&dispatch_pane, &*runner) {
                Ok(pane_id) => {
                    let _ = tx.send(Message::Split(
                        crate::tui::messages::SplitMessage::PaneOpened {
                            pane_id,
                            task_id: None,
                        },
                    ));
                }
                Err(e) => {
                    let _ = tx.send(Message::System(crate::tui::messages::SystemMessage::Error(
                        format!("Split failed: {e:#}"),
                    )));
                }
            }
        })
    }

    pub(super) fn exec_enter_split_mode_with_task(
        &self,
        task_id: TaskId,
        window: &str,
    ) -> tokio::task::JoinHandle<()> {
        let tx = self.msg_tx.clone();
        let runner = Arc::clone(&self.runner);
        let window = window.to_owned();
        tokio::task::spawn_blocking(move || {
            let dispatch_pane = match tmux::current_pane_id(&*runner) {
                Ok(id) => id,
                Err(_) => {
                    let _ = tx.send(Message::System(
                        crate::tui::messages::SystemMessage::StatusInfo(
                            "Split mode requires tmux".to_string(),
                        ),
                    ));
                    return;
                }
            };

            match dispatch::join_task_window_into_pane(&window, &dispatch_pane, &*runner) {
                Ok(pane_id) => {
                    let _ = tx.send(Message::Split(
                        crate::tui::messages::SplitMessage::PaneOpened {
                            pane_id,
                            task_id: Some(task_id),
                        },
                    ));
                }
                Err(e) => {
                    let _ = tx.send(Message::System(crate::tui::messages::SystemMessage::Error(
                        format!("Split with task failed: {e:#}"),
                    )));
                }
            }
        })
    }

    pub(super) fn exec_exit_split_mode(
        &self,
        pane_id: &str,
        restore_window: Option<&str>,
    ) -> tokio::task::JoinHandle<()> {
        let tx = self.msg_tx.clone();
        let runner = Arc::clone(&self.runner);
        let pane_id = pane_id.to_owned();
        let restore_window = restore_window.map(str::to_owned);
        tokio::task::spawn_blocking(move || {
            if let Some(window_name) = restore_window {
                if let Err(e) = tmux::break_pane_to_window(&pane_id, &window_name, &*runner) {
                    let _ = tx.send(Message::System(crate::tui::messages::SystemMessage::Error(
                        format!("Break pane failed: {e:#}"),
                    )));
                    return;
                }
            } else if let Err(e) = tmux::kill_pane(&pane_id, &*runner) {
                let _ = tx.send(Message::System(crate::tui::messages::SystemMessage::Error(
                    format!("Kill pane failed: {e:#}"),
                )));
                return;
            }
            let _ = tx.send(Message::Split(
                crate::tui::messages::SplitMessage::PaneClosed,
            ));
        })
    }

    pub(super) fn exec_swap_split_pane(
        &self,
        task_id: TaskId,
        new_window: &str,
        old_pane_id: Option<&str>,
        old_window: Option<&str>,
    ) -> tokio::task::JoinHandle<()> {
        let tx = self.msg_tx.clone();
        let runner = Arc::clone(&self.runner);
        let new_window = new_window.to_owned();
        let old_pane_id = old_pane_id.map(str::to_owned);
        let old_window = old_window.map(str::to_owned);

        tokio::task::spawn_blocking(move || {
            let Some(right_pane) = old_pane_id else {
                return;
            };

            match dispatch::swap_task_window_into_pane(
                &new_window,
                &right_pane,
                old_window.as_deref(),
                &*runner,
            ) {
                Ok(new_pane_id) => {
                    let _ = tx.send(Message::Split(
                        crate::tui::messages::SplitMessage::PaneOpened {
                            pane_id: new_pane_id,
                            task_id: Some(task_id),
                        },
                    ));
                }
                Err(e) => {
                    let _ = tx.send(Message::System(crate::tui::messages::SystemMessage::Error(
                        format!("Swap failed: {e:#}"),
                    )));
                }
            }
        })
    }

    pub(super) fn exec_check_split_pane(&self, pane_id: &str) -> tokio::task::JoinHandle<()> {
        let tx = self.msg_tx.clone();
        let runner = self.runner.clone();
        let pane_id = pane_id.to_owned();
        tokio::task::spawn_blocking(move || {
            if !tmux::pane_exists(&pane_id, &*runner) {
                let _ = tx.send(Message::Split(
                    crate::tui::messages::SplitMessage::PaneClosed,
                ));
            }
        })
    }

    pub(super) fn exec_respawn_split_pane(&self, pane_id: &str) -> tokio::task::JoinHandle<()> {
        let tx = self.msg_tx.clone();
        let runner = self.runner.clone();
        let pane_id = pane_id.to_owned();
        tokio::task::spawn_blocking(move || {
            if let Err(e) = tmux::respawn_pane(&pane_id, &*runner) {
                tracing::warn!("respawn-pane failed: {e:#}");
                let _ = tx.send(Message::Split(
                    crate::tui::messages::SplitMessage::PaneClosed,
                ));
            }
        })
    }

    pub(super) fn exec_kill_tmux_window(&self, window: String) -> tokio::task::JoinHandle<()> {
        let runner = self.runner.clone();

        tokio::task::spawn_blocking(move || {
            if let Err(e) = tmux::kill_window(&window, &*runner) {
                tracing::warn!(%window, "failed to kill tmux window (best-effort): {e:#}");
            }
        })
    }

    pub(super) fn exec_focus_split_pane(&self, pane_id: String) -> tokio::task::JoinHandle<()> {
        let runner = Arc::clone(&self.runner);
        tokio::task::spawn_blocking(move || {
            if let Err(e) = tmux::select_pane(&pane_id, &*runner) {
                tracing::warn!("select-pane failed: {e:#}");
            }
        })
    }
}
