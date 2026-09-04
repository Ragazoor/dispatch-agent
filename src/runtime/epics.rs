use super::*;

impl TuiRuntime {
    pub(super) async fn exec_insert_epic(
        &self,
        app: &mut App,
        title: String,
        description: String,
        parent_epic_id: Option<crate::models::EpicId>,
    ) {
        match self
            .epic_svc
            .create_epic(crate::service::CreateEpicParams {
                title,
                description,
                sort_order: None,
                parent_epic_id,
                feed_command: None,
                feed_interval_secs: None,
            })
            .await
        {
            Ok(epic) => {
                app.update(Message::Epic(crate::tui::messages::EpicMessage::Created(
                    epic,
                )));
            }
            Err(e) => {
                app.update(Message::System(crate::tui::messages::SystemMessage::Error(
                    Self::db_error("creating epic", e),
                )));
            }
        }
    }

    pub(super) async fn exec_delete_epic(&self, app: &mut App, id: models::EpicId) {
        if let Err(e) = self.epic_svc.delete_epic(id).await {
            app.update(Message::System(crate::tui::messages::SystemMessage::Error(
                Self::db_error("deleting epic", e),
            )));
        }
    }

    pub(super) async fn exec_persist_epic(
        &self,
        app: &mut App,
        id: models::EpicId,
        status: Option<models::TaskStatus>,
        sort_order: Option<i64>,
    ) {
        if status.is_none() && sort_order.is_none() {
            return;
        }
        self.exec_patch_epic(
            app,
            crate::service::UpdateEpicParams {
                epic_id: id,
                title: None,
                description: None,
                status,
                plan_path: None,
                sort_order,
                auto_dispatch: None,
                feed_command: None,
                feed_interval_secs: None,
                group_by_repo: None,
                feed_append_only: None,
                parent_epic_id: None,
            },
            "updating epic",
        )
        .await;
    }

    /// Routing chokepoint for `exec_persist_epic`, `exec_toggle_epic_auto_dispatch`,
    /// and `exec_reparent_epic`. Writes any service-computed `sort_order` (the
    /// Done-transition rule in `EpicService::update_epic`) back into the
    /// in-memory board immediately, via the same `EpicMessage::Updated` splice
    /// `spawn_refresh_epic` uses — not a direct `App.board` mutation, since only
    /// `crate::tui` code may touch that field (see docs/conventions.md
    /// "Visibility convention").
    async fn exec_patch_epic(
        &self,
        app: &mut App,
        params: crate::service::UpdateEpicParams,
        context: &str,
    ) {
        match self.epic_svc.update_epic(params).await {
            Ok(result) => self.write_back_epic_sort_order(app, result),
            Err(e) => {
                app.update(Message::System(crate::tui::messages::SystemMessage::Error(
                    Self::db_error(context, e),
                )));
            }
        }
    }

    /// If `result` carries a written `sort_order`, splice an updated copy of
    /// the in-memory epic into `App.board.epics` immediately (rather than
    /// waiting for the next DB refresh). No-op when `sort_order_after_write`
    /// is `None` (this call's patch didn't touch it) or the epic isn't
    /// currently in memory.
    fn write_back_epic_sort_order(&self, app: &mut App, result: crate::service::UpdateEpicResult) {
        let Some(new_sort_order) = result.sort_order_after_write else {
            return;
        };
        let Some(mut epic) = app.epics().iter().find(|e| e.id == result.epic_id).cloned() else {
            return;
        };
        epic.sort_order = new_sort_order;
        app.update(Message::Epic(crate::tui::messages::EpicMessage::Updated(
            epic,
        )));
    }

    pub(super) async fn exec_toggle_epic_auto_dispatch(
        &self,
        app: &mut App,
        id: models::EpicId,
        auto_dispatch: bool,
    ) {
        self.exec_patch_epic(
            app,
            crate::service::UpdateEpicParams {
                epic_id: id,
                title: None,
                description: None,
                status: None,
                plan_path: None,
                sort_order: None,
                auto_dispatch: Some(auto_dispatch),
                feed_command: None,
                feed_interval_secs: None,
                group_by_repo: None,
                feed_append_only: None,
                parent_epic_id: None,
            },
            "toggling auto dispatch",
        )
        .await;
    }

    pub(super) async fn exec_toggle_epic_group_by_repo(
        &self,
        app: &mut App,
        id: models::EpicId,
        group_by_repo: bool,
    ) {
        let params = crate::service::UpdateEpicParams {
            epic_id: id,
            title: None,
            description: None,
            status: None,
            plan_path: None,
            sort_order: None,
            auto_dispatch: None,
            feed_command: None,
            feed_interval_secs: None,
            group_by_repo: Some(group_by_repo),
            feed_append_only: None,
            parent_epic_id: None,
        };
        match self.epic_svc.update_epic(params).await {
            Ok(result) => self.write_back_epic_sort_order(app, result),
            Err(e) => {
                app.update(Message::System(crate::tui::messages::SystemMessage::Error(
                    Self::db_error("toggling group by repo", e),
                )));
                return;
            }
        }
        // Deliberately inlines update + migrate + single board refresh instead
        // of delegating to exec_patch_epic: exec_patch_epic does not trigger
        // regroup/flatten, and calling exec_refresh_epics_from_db once here
        // avoids a double board refresh that would otherwise flash the UI.
        // Apply migration only for non-feed epics (feed epics group via ingestion).
        match self.epic_svc.get_epic(id).await {
            Ok(epic) if epic.feed_command.is_none() => {
                let res = if group_by_repo {
                    self.epic_svc.regroup_epic(id).await
                } else {
                    self.epic_svc.flatten_epic(id).await
                };
                if let Err(e) = res {
                    app.update(Message::System(crate::tui::messages::SystemMessage::Error(
                        Self::db_error("regrouping epic", e),
                    )));
                    return;
                }
            }
            Ok(_) => {}
            Err(e) => {
                app.update(Message::System(crate::tui::messages::SystemMessage::Error(
                    Self::db_error("loading epic after toggle", e),
                )));
                return;
            }
        }
        self.exec_refresh_epics_from_db(app).await;
    }

    pub(super) async fn exec_reparent_epic(
        &self,
        app: &mut App,
        id: models::EpicId,
        new_parent: Option<models::EpicId>,
    ) {
        self.exec_patch_epic(
            app,
            crate::service::UpdateEpicParams {
                epic_id: id,
                title: None,
                description: None,
                status: None,
                plan_path: None,
                sort_order: None,
                auto_dispatch: None,
                feed_command: None,
                feed_interval_secs: None,
                group_by_repo: None,
                feed_append_only: None,
                parent_epic_id: Some(new_parent),
            },
            "reparenting epic",
        )
        .await;
        // Refresh so the board reflects the new hierarchy.
        self.exec_refresh_epics_from_db(app).await;
    }

    pub(super) async fn exec_refresh_epics_from_db(&self, app: &mut App) {
        match self.database.list_epics().await {
            Ok(epics) => {
                app.update(Message::Epic(crate::tui::messages::EpicMessage::Refresh(
                    epics,
                )));
            }
            Err(e) => {
                app.update(Message::System(crate::tui::messages::SystemMessage::Error(
                    Self::db_error("refreshing epics", e),
                )));
            }
        }
    }

    /// Run one feed cycle for `epic_id` and present its outcome in the status
    /// bar. Every step of the cycle lives in [`crate::feed::FeedCycle`], shared
    /// with the auto-poll path, so the two cannot drift; this function owns only
    /// the presentation (feeds.allium: ManualFeedTrigger).
    ///
    /// Takes no `feed_command` or `group_by_repo`: those are read from the epic
    /// inside the cycle, so a refresh cannot run a command the user has since
    /// changed. `epic_title` is passed for the status lines only.
    pub(super) fn exec_trigger_epic_feed(&self, epic_id: models::EpicId, epic_title: String) {
        // Feed subsystem path: upserts tasks and recalculates epic status, so it
        // uses the write-capable `feed_db` handle (mirrors `FeedRunner`).
        let cycle = crate::feed::FeedCycle {
            db: self.feed_db.clone(),
            runner: self.runner.clone(),
            guard: self.feed_sync_guard.clone(),
            epic_id,
            epic_title: epic_title.clone(),
            // Resolved inside the cycle, after the claim, so a dropped refresh
            // does no DB work.
            known_paths: None,
            command_timeout: crate::feed::FEED_COMMAND_TIMEOUT,
        };
        let tx = self.msg_tx.clone();

        tokio::spawn(async move {
            use crate::tui::messages::FeedMessage;

            // The cycle has already torn down every removed task's worktree by
            // the time it returns, so "N task(s) synced" means reconciled AND
            // cleaned up (feeds.allium: RoleRoutedFeedSync).
            let message = match cycle.run().await {
                crate::feed::FeedCycleOutcome::Synced {
                    count, degraded, ..
                } => FeedMessage::Refreshed {
                    epic_title,
                    // Items the feed command emitted, not tasks inserted.
                    count,
                    // Some(reason) when the cycle ran additively and so removed
                    // nothing (feeds.allium: DegradedNonEmptyEmission). Carried
                    // to the status line rather than dropped: an unchanged board
                    // must not read as a reconciled one.
                    degraded,
                },
                // Neither a success nor a failure: nothing ran, because a cycle
                // for this epic was already in flight. A distinct variant, not a
                // Failed with a special string, so the status bar cannot blame
                // the user's feed command for a serialisation decision.
                crate::feed::FeedCycleOutcome::Busy => {
                    FeedMessage::AlreadyRefreshing { epic_title }
                }
                // Already logged by the cycle; we add the status-bar surface.
                crate::feed::FeedCycleOutcome::Failed(error) => {
                    FeedMessage::Failed { epic_title, error }
                }
            };
            let _ = tx.send(Message::Feed(message));
        });
    }
}
