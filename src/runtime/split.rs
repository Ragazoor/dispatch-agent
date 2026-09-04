use super::*;

impl TuiRuntime {
    pub(super) fn exec_jump_to_tmux(&self, app: &mut App, window: TmuxWindow) {
        if let Err(e) = tmux::select_window(&window, &*self.runner) {
            app.update(Message::System(crate::tui::messages::SystemMessage::Error(
                format!("Jump failed: {e:#}"),
            )));
        }
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
        window: &TmuxWindow,
    ) -> tokio::task::JoinHandle<()> {
        let tx = self.msg_tx.clone();
        let runner = Arc::clone(&self.runner);
        let window = window.clone();
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
        restore_window: Option<&TmuxWindow>,
    ) -> tokio::task::JoinHandle<()> {
        let tx = self.msg_tx.clone();
        let runner = Arc::clone(&self.runner);
        let pane_id = pane_id.to_owned();
        let restore_window = restore_window.cloned();
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
        new_window: &TmuxWindow,
        old_pane_id: Option<&str>,
        old_task: Option<(&TmuxWindow, &str)>,
    ) -> tokio::task::JoinHandle<()> {
        let tx = self.msg_tx.clone();
        let runner = Arc::clone(&self.runner);
        let new_window = new_window.clone();
        let old_pane_id = old_pane_id.map(str::to_owned);
        let old_task = old_task.map(|(window, worktree)| (window.clone(), worktree.to_owned()));

        tokio::task::spawn_blocking(move || {
            let Some(right_pane) = old_pane_id else {
                return;
            };

            match dispatch::swap_task_window_into_pane(
                &new_window,
                &right_pane,
                old_task
                    .as_ref()
                    .map(|(window, worktree)| (window, worktree.as_str())),
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

    pub(super) fn exec_kill_tmux_window(&self, window: TmuxWindow) -> tokio::task::JoinHandle<()> {
        let runner = self.runner.clone();

        tokio::task::spawn_blocking(move || {
            if let Err(e) = tmux::kill_window(&window, &*runner) {
                // An already-absent window is the outcome this call wanted, so
                // it is logged at debug. Only a kill that was attempted and
                // failed leaves a real window behind and warrants a warning.
                if tmux::is_window_absent_error(&e) {
                    tracing::debug!(%window, "tmux window was already gone, nothing to kill");
                } else {
                    tracing::warn!(%window, "failed to kill tmux window (best-effort): {e:#}");
                }
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
