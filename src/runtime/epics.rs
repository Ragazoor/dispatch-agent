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

    pub(super) fn exec_trigger_epic_feed(
        &self,
        epic_id: models::EpicId,
        epic_title: String,
        feed_command: String,
        group_by_repo: bool,
    ) {
        // Feed subsystem path: upserts tasks and recalculates epic status, so it
        // uses the write-capable `feed_db` handle (mirrors `FeedRunner`).
        let db = self.feed_db.clone();
        let tx = self.msg_tx.clone();
        let runner = self.runner.clone();

        tokio::spawn(async move {
            let fail = |error: String| {
                let _ = tx.send(Message::Feed(crate::tui::messages::FeedMessage::Failed {
                    epic_title: epic_title.clone(),
                    error,
                }));
            };

            // The SAME exec the auto-poll FeedRunner uses, so neither path can
            // drop a feed command's stderr again (feeds.allium:
            // FeedCommandStderrOnSuccess). It logs spawn/non-zero failures and
            // stderr-on-success itself; we add the status-bar surface.
            let output =
                match crate::feed::exec_feed_command(&feed_command, epic_id.0, &epic_title).await {
                    Ok(o) => o,
                    Err(e) => return fail(e),
                };
            let wrote_stderr = !output.stderr.is_empty();

            let items: Vec<models::FeedItem> = match serde_json::from_slice(&output.stdout) {
                Ok(i) => i,
                Err(e) => return fail(e.to_string()),
            };

            let count = items.len(); // items emitted by the feed command, not tasks inserted
            let known_paths = db.list_repo_paths().await.unwrap_or_default();
            let repo_paths = dispatch::resolve_feed_item_repo_paths(&items, &known_paths);
            let base_branches = crate::feed::resolve_base_branches(&repo_paths, &*runner);
            let entries = crate::feed::FeedItemWithTarget::zip(items, repo_paths, base_branches);
            // Dispatch by feed_role through the shared dispatcher — the SAME
            // role→sync-path mapping the auto-poll FeedRunner uses — so a
            // reviews_parent epic routes its emission through the subtree role
            // router, never a flat upsert onto the parent (feeds.allium: FeedSync
            // dispatch). group_by_repo is only consulted on the non-reviews_parent
            // path.
            let feed_role = match db.get_epic(epic_id).await {
                Ok(Some(e)) => e.feed_role,
                _ => crate::models::FeedRole::None,
            };
            let sync_result = crate::feed::run_feed_sync_by_role(
                &*db,
                epic_id,
                feed_role,
                group_by_repo,
                entries,
            )
            .await;
            match sync_result {
                Ok(_) => {
                    crate::feed::recalculate_epic_status_after_feed(
                        &*db,
                        epic_id,
                        "exec_trigger_epic_feed",
                    )
                    .await;
                    let _ = tx.send(Message::Feed(
                        crate::tui::messages::FeedMessage::Refreshed {
                            epic_title,
                            count,
                            wrote_stderr,
                        },
                    ));
                }
                Err(e) => fail(e.to_string()),
            }
        });
    }
}
