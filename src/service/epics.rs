use std::sync::Arc;

use crate::db::{self, EpicPatch};
use crate::models::{sort_order_for_status_transition, Epic, EpicId, Task, TaskStatus};

use super::{FieldUpdate, ServiceError};

// ---------------------------------------------------------------------------
// UpdateEpicParams
// ---------------------------------------------------------------------------

pub struct UpdateEpicParams {
    pub epic_id: EpicId,
    pub title: Option<String>,
    pub description: Option<String>,
    pub status: Option<TaskStatus>,
    pub plan_path: Option<String>,
    pub sort_order: Option<i64>,
    pub auto_dispatch: Option<bool>,
    pub feed_command: Option<FieldUpdate>,
    pub feed_interval_secs: Option<Option<i64>>,
    pub group_by_repo: Option<bool>,
    /// Triple-state: None = no change, Some(Some(id)) = reparent, Some(None) = make root.
    pub parent_epic_id: Option<Option<EpicId>>,
}

impl UpdateEpicParams {
    pub(in crate::service) fn has_any_field(&self) -> bool {
        !self.updated_field_names().is_empty()
    }

    /// Names of the fields this params value actually sets. Mirrors
    /// [`UpdateTaskParams::updated_field_names`](crate::service::UpdateTaskParams::updated_field_names)
    /// — same compiler-enforced parity, same reason.
    pub fn updated_field_names(&self) -> Vec<&str> {
        let Self {
            epic_id: _,
            title,
            description,
            status,
            plan_path,
            sort_order,
            auto_dispatch,
            feed_command,
            feed_interval_secs,
            group_by_repo,
            parent_epic_id,
        } = self;

        [
            ("title", title.is_some()),
            ("description", description.is_some()),
            ("status", status.is_some()),
            ("plan_path", plan_path.is_some()),
            ("sort_order", sort_order.is_some()),
            ("auto_dispatch", auto_dispatch.is_some()),
            ("feed_command", feed_command.is_some()),
            ("feed_interval_secs", feed_interval_secs.is_some()),
            ("group_by_repo", group_by_repo.is_some()),
            ("parent_epic_id", parent_epic_id.is_some()),
        ]
        .into_iter()
        .filter_map(|(name, is_set)| is_set.then_some(name))
        .collect()
    }
}

/// Result of [`EpicService::update_epic`]. Mirrors
/// [`UpdateTaskResult`](crate::service::UpdateTaskResult) — same
/// capture-before-write shape, same reason: the service (not the caller)
/// computes `sort_order` on a Done-transition, so a caller holding its own
/// in-memory copy of the epic (the TUI's `App.board.epics`) needs a way to
/// learn that value without a second DB round-trip.
#[derive(Debug, Clone)]
pub struct UpdateEpicResult {
    pub epic_id: EpicId,
    /// `None` = this call's patch didn't touch `sort_order`. `Some(v)` = it
    /// did, where `v` is exactly what was written (`Some(None)` for a clear,
    /// `Some(Some(x))` for a set to `x`).
    pub sort_order_after_write: Option<Option<i64>>,
}

// ---------------------------------------------------------------------------
// CreateEpicParams
// ---------------------------------------------------------------------------

pub struct CreateEpicParams {
    pub title: String,
    pub description: String,
    pub sort_order: Option<i64>,
    pub parent_epic_id: Option<EpicId>,
    pub feed_command: Option<String>,
    pub feed_interval_secs: Option<i64>,
}

// ---------------------------------------------------------------------------
// Progress-rollup helper types
// ---------------------------------------------------------------------------

/// Tasks grouped by their epic id, for the progress rollup.
type TasksByEpic<'a> = std::collections::HashMap<EpicId, Vec<&'a Task>>;

/// (done, total) over a task slice, counting `TaskStatus::Done` as done.
fn count_progress(tasks: &[&Task]) -> (usize, usize) {
    (
        tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Done)
            .count(),
        tasks.len(),
    )
}

// ---------------------------------------------------------------------------
// EpicService
// ---------------------------------------------------------------------------

pub struct EpicService {
    pub db: Arc<dyn db::TaskAndEpicStore>,
    clock: Arc<dyn crate::service::Clock>,
}

impl EpicService {
    pub fn new(db: Arc<dyn db::TaskAndEpicStore>) -> Self {
        Self {
            db,
            clock: Arc::new(crate::service::SystemClock),
        }
    }

    /// Override the clock used for the Done-transition sort_order rule.
    /// Tests inject a `FixedClock` for determinism; mirrors
    /// `TaskService::with_clock`.
    pub fn with_clock(mut self, clock: Arc<dyn crate::service::Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Materialise the managed feed-epic tree from already-read settings.
    /// The epic writes go through the service's `EpicCrud` handle.
    pub async fn provision_managed_feeds(
        &self,
        settings: crate::service::ManagedFeedSettings,
    ) -> Result<(), ServiceError> {
        crate::service::ensure_managed_epics(
            &*self.db,
            settings.reviews_command.as_deref(),
            settings.reviews_interval_secs,
            settings.cve_command.as_deref(),
            settings.cve_interval_secs,
        )
        .await
        .map_err(ServiceError::from)
    }

    pub async fn create_epic(&self, params: CreateEpicParams) -> Result<Epic, ServiceError> {
        if let Some(parent_id) = params.parent_epic_id {
            self.db.get_epic(parent_id).await?.ok_or_else(|| {
                ServiceError::NotFound(format!("Parent epic {} not found", parent_id.0))
            })?;
        }

        let epic = self
            .db
            .create_epic(&params.title, &params.description, params.parent_epic_id)
            .await?;

        // A new sub-epic changes the parent's active_sub_epics set, so the
        // parent must be recalculated immediately (e.g. a Done parent with a
        // freshly-attached Backlog child regresses right away).
        if let Some(parent_id) = params.parent_epic_id {
            self.recalculate_epic(parent_id).await;
        }

        // The insert above only carries title/description/parent, so anything
        // else the caller supplied needs a follow-up write.
        let patch = EpicPatch {
            sort_order: params.sort_order.map(Some),
            feed_command: params.feed_command.as_deref().map(Some),
            feed_interval_secs: params.feed_interval_secs.map(Some),
            ..EpicPatch::new()
        };
        if !patch.has_changes() {
            return Ok(epic);
        }

        self.db.patch_epic(epic.id, &patch).await?;
        // Re-read so the returned Epic reflects the follow-up write rather
        // than the pre-patch insert result.
        self.get_epic(epic.id).await
    }

    pub async fn get_epic(&self, epic_id: EpicId) -> Result<Epic, ServiceError> {
        self.db
            .get_epic(epic_id)
            .await?
            .ok_or_else(|| ServiceError::NotFound(format!("Epic {} not found", epic_id.0)))
    }

    pub async fn get_epic_with_subtasks(
        &self,
        epic_id: EpicId,
    ) -> Result<(Epic, Vec<Task>), ServiceError> {
        let epic = self.get_epic(epic_id).await?;
        let subtasks = self
            .db
            .list_tasks_for_epic(epic.id)
            .await
            .unwrap_or_default();
        Ok((epic, subtasks))
    }

    /// Progress for a single epic, rolled up the same way as
    /// [`list_epics_with_progress`](Self::list_epics_with_progress): a
    /// `group_by_repo` epic's counts include its descendant sub-epics, not
    /// just its direct subtasks. Used by `get_epic` so it agrees with the
    /// board/`list_epics` view instead of undercounting to 0/0.
    ///
    /// Only `group_by_repo` epics need the whole-board rollup — every other
    /// epic still counts its own direct subtasks, so the common case stays as
    /// cheap as the old direct-subtasks-only lookup.
    pub async fn get_epic_with_progress(
        &self,
        epic_id: EpicId,
    ) -> Result<(Epic, usize, usize), ServiceError> {
        let epic = self.get_epic(epic_id).await?;
        if !epic.group_by_repo {
            let subtasks = self
                .db
                .list_tasks_for_epic(epic.id)
                .await
                .unwrap_or_default();
            let (done, total) = count_progress(&subtasks.iter().collect::<Vec<_>>());
            return Ok((epic, done, total));
        }

        let all_epics = self.db.list_epics().await?;
        let all_tasks = self.db.list_all_tasks_with_epic_id().await?;
        let tasks_by_epic = Self::group_tasks_by_epic(&all_tasks);
        let children = crate::models::build_children_map(&all_epics);
        let (done, total) = Self::epic_progress(&epic, &tasks_by_epic, &children);
        Ok((epic, done, total))
    }

    /// Group tasks by epic id, for the progress rollup.
    fn group_tasks_by_epic(tasks: &[Task]) -> TasksByEpic<'_> {
        let mut tasks_by_epic: TasksByEpic = std::collections::HashMap::new();
        for task in tasks {
            if let Some(eid) = task.epic_id {
                tasks_by_epic.entry(eid).or_default().push(task);
            }
        }
        tasks_by_epic
    }

    /// (done, total) for one epic: direct subtasks, plus — when the epic is
    /// `group_by_repo` — the rollup of all descendant sub-epics' tasks, via
    /// the shared [`crate::models::descendant_epic_ids_with_map`] traversal
    /// (the same one `SubtaskStats::for_epic` uses in the TUI).
    fn epic_progress(
        epic: &Epic,
        tasks_by_epic: &TasksByEpic<'_>,
        children: &std::collections::HashMap<EpicId, Vec<EpicId>>,
    ) -> (usize, usize) {
        if epic.group_by_repo {
            let ids = crate::models::descendant_epic_ids_with_map(epic.id, children);
            let tasks: Vec<&Task> = ids
                .iter()
                .filter_map(|id| tasks_by_epic.get(id))
                .flatten()
                .copied()
                .collect();
            count_progress(&tasks)
        } else {
            let empty = Vec::new();
            let tasks = tasks_by_epic.get(&epic.id).unwrap_or(&empty);
            count_progress(tasks)
        }
    }

    pub async fn list_epics(&self) -> Result<Vec<Epic>, ServiceError> {
        Ok(self.db.list_epics().await?)
    }

    pub async fn list_root_epics(&self) -> Result<Vec<Epic>, ServiceError> {
        Ok(self.db.list_root_epics().await?)
    }

    pub async fn list_sub_epics(&self, parent_id: EpicId) -> Result<Vec<Epic>, ServiceError> {
        Ok(self.db.list_sub_epics(parent_id).await?)
    }

    pub async fn list_epics_with_progress(
        &self,
    ) -> Result<Vec<(Epic, usize, usize)>, ServiceError> {
        let epics = self.list_epics().await?;
        let all_subtasks = self.db.list_all_tasks_with_epic_id().await?;
        let tasks_by_epic = Self::group_tasks_by_epic(&all_subtasks);
        let children = crate::models::build_children_map(&epics);

        let result = epics
            .into_iter()
            .filter(|e| e.status != TaskStatus::Archived)
            .map(|e| {
                let (done, total) = Self::epic_progress(&e, &tasks_by_epic, &children);
                (e, done, total)
            })
            .collect();
        Ok(result)
    }

    pub async fn update_epic(
        &self,
        params: UpdateEpicParams,
    ) -> Result<UpdateEpicResult, ServiceError> {
        if !params.has_any_field() {
            return Err(ServiceError::Validation(
                "At least one field must be provided".into(),
            ));
        }

        let epic_id = params.epic_id;
        let existing = self.db.get_epic(epic_id).await?;

        let mut patch = EpicPatch::new();
        if let Some(ref t) = params.title {
            patch = patch.title(t);
        }
        if let Some(ref d) = params.description {
            patch = patch.description(d);
        }
        if let Some(status) = params.status {
            patch = patch.status(status);
        }
        if let Some(ref p) = params.plan_path {
            patch = patch.plan_path(Some(p.as_str()));
        }
        if let Some(so) = params.sort_order {
            patch = patch.sort_order(Some(so));
        }
        if let Some(ad) = params.auto_dispatch {
            patch = patch.auto_dispatch(ad);
        }
        if let Some(ref fc) = params.feed_command {
            patch = patch.feed_command(fc.as_option());
        }
        if let Some(fi) = params.feed_interval_secs {
            patch = patch.feed_interval_secs(fi);
        }
        if let Some(gbr) = params.group_by_repo {
            patch = patch.group_by_repo(gbr);
        }

        // Fetch the prior epic whenever status changes, to detect a
        // transition into/out of Done for the sort_order-on-completion
        // rule. This method has no other prior-fetch to reuse (the
        // RepoGroup-reparent guard below does its own, gated on a
        // different condition).
        if let Some(new_status) = params.status {
            if let Some(prior_epic) = self.db.get_epic(params.epic_id).await? {
                if let Some(so) = sort_order_for_status_transition(
                    prior_epic.status,
                    new_status,
                    self.clock.now(),
                ) {
                    patch = patch.sort_order(so);
                }
            }
        }

        // Prevent reparenting or detaching a RepoGroup sub-epic: both
        // Some(Some(_)) (reparent) and Some(None) (detach to root) would
        // orphan an auto-created sub-epic outside its grouping root.
        if matches!(params.parent_epic_id, Some(Some(_)) | Some(None)) {
            if let Some(ref epic) = existing {
                if epic.origin == crate::models::EpicOrigin::RepoGroup {
                    return Err(ServiceError::Validation(
                        "Cannot reparent an auto-created repo-group sub-epic".into(),
                    ));
                }
            }
        }

        match params.parent_epic_id {
            Some(Some(new_parent_id)) => {
                let parent = self.get_epic(new_parent_id).await?;
                self.check_no_cycle(epic_id, &parent).await?;
                patch = patch.parent_epic_id(Some(new_parent_id));
            }
            Some(None) => {
                patch = patch.parent_epic_id(None);
            }
            None => {}
        }

        // Captured before the write so the caller can learn what this call
        // wrote to sort_order (including the Done-transition override
        // above) without a second DB round-trip. See `UpdateEpicResult`.
        let sort_order_after_write = patch.sort_order;
        self.db.patch_epic(epic_id, &patch).await?;

        // recalculate_epic_status must run whenever a sub-epic's status
        // changes or its parent membership changes, since either mutates a
        // parent's active_sub_epics rollup. Recalculate the *parent*, not
        // this epic itself — self-recalc would fight an explicit status
        // write with the children-derived target.
        if let Some(existing) = existing {
            if let Some(new_parent) = params.parent_epic_id {
                if let Some(old_parent) = existing.parent_epic_id {
                    self.recalculate_epic(old_parent).await;
                }
                if let Some(new_parent) = new_parent {
                    self.recalculate_epic(new_parent).await;
                }
            } else if params.status.is_some() {
                if let Some(parent) = existing.parent_epic_id {
                    self.recalculate_epic(parent).await;
                }
            }
        }

        Ok(UpdateEpicResult {
            epic_id,
            sort_order_after_write,
        })
    }

    /// Recalculate the given epic, logging any database error.
    async fn recalculate_epic(&self, epic_id: EpicId) {
        if let Err(err) = self.db.recalculate_epic_status(epic_id).await {
            tracing::warn!(
                "failed to recalculate epic status for epic {}: {err}",
                epic_id.0
            );
        }
    }

    /// Walk the ancestor chain of `proposed_parent` and return a Validation error
    /// if `epic_id` appears in it (which would create a cycle).
    /// Takes a pre-fetched `&Epic` to avoid an extra DB round-trip.
    async fn check_no_cycle(
        &self,
        epic_id: EpicId,
        proposed_parent: &Epic,
    ) -> Result<(), ServiceError> {
        if proposed_parent.id == epic_id {
            return Err(ServiceError::Validation(
                "Setting this parent would create a cycle in the epic hierarchy".into(),
            ));
        }
        let mut current_opt = proposed_parent.parent_epic_id;
        loop {
            let current = match current_opt {
                None => return Ok(()),
                Some(c) => c,
            };
            if current == epic_id {
                return Err(ServiceError::Validation(
                    "Setting this parent would create a cycle in the epic hierarchy".into(),
                ));
            }
            match self.db.get_epic(current).await? {
                Some(e) => current_opt = e.parent_epic_id,
                None => return Ok(()),
            }
        }
    }

    pub async fn regroup_epic(&self, root: EpicId) -> Result<(), ServiceError> {
        crate::service::regroup_epic(&*self.db, root).await
    }

    pub async fn flatten_epic(&self, root: EpicId) -> Result<(), ServiceError> {
        crate::service::flatten_epic(&*self.db, root).await
    }

    pub async fn reroute_on_repo_change(
        &self,
        task: crate::models::TaskId,
        new_repo: &str,
    ) -> Result<(), ServiceError> {
        crate::service::reroute_on_repo_change(&*self.db, task, new_repo).await
    }

    /// Recursively update project_id for all direct sub-epics and direct tasks
    pub async fn delete_epic(&self, epic_id: EpicId) -> Result<(), ServiceError> {
        // Verify epic exists
        self.get_epic(epic_id).await?;

        self.db
            .delete_epic(epic_id)
            .await
            .map_err(ServiceError::from)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::db::{Database, EpicCrud, EpicRead};

    fn base_params(epic_id: EpicId) -> UpdateEpicParams {
        UpdateEpicParams {
            epic_id,
            title: None,
            description: None,
            status: None,
            plan_path: None,
            sort_order: None,
            auto_dispatch: None,
            feed_command: None,
            feed_interval_secs: None,
            group_by_repo: None,
            parent_epic_id: None,
        }
    }

    #[test]
    fn update_epic_params_has_any_field_consistent_with_updated_field_names() {
        let with_field = UpdateEpicParams {
            title: Some("x".to_string()),
            ..base_params(EpicId(1))
        };
        assert!(
            with_field.has_any_field(),
            "has_any_field should be true when title is set"
        );
        assert!(
            !with_field.updated_field_names().is_empty(),
            "updated_field_names should be non-empty when title is set"
        );

        let empty = base_params(EpicId(1));
        assert!(
            !empty.has_any_field(),
            "has_any_field should be false when no fields are set"
        );
        assert!(
            empty.updated_field_names().is_empty(),
            "updated_field_names should be empty when no fields are set"
        );
    }

    #[test]
    fn update_epic_params_every_field_covered() {
        // The exhaustive destructuring in updated_field_names() already makes an
        // unhandled field a compile error; what this test uniquely covers is
        // that each field reports its *own* name.
        let cases: Vec<(&str, UpdateEpicParams)> = vec![
            (
                "title",
                UpdateEpicParams {
                    title: Some("t".to_string()),
                    ..base_params(EpicId(1))
                },
            ),
            (
                "description",
                UpdateEpicParams {
                    description: Some("d".to_string()),
                    ..base_params(EpicId(1))
                },
            ),
            (
                "status",
                UpdateEpicParams {
                    status: Some(TaskStatus::Backlog),
                    ..base_params(EpicId(1))
                },
            ),
            (
                "plan_path",
                UpdateEpicParams {
                    plan_path: Some("p".to_string()),
                    ..base_params(EpicId(1))
                },
            ),
            (
                "sort_order",
                UpdateEpicParams {
                    sort_order: Some(0),
                    ..base_params(EpicId(1))
                },
            ),
            (
                "auto_dispatch",
                UpdateEpicParams {
                    auto_dispatch: Some(true),
                    ..base_params(EpicId(1))
                },
            ),
            (
                "feed_command",
                UpdateEpicParams {
                    feed_command: Some(FieldUpdate::Set("cmd".to_string())),
                    ..base_params(EpicId(1))
                },
            ),
            (
                "feed_interval_secs",
                UpdateEpicParams {
                    feed_interval_secs: Some(Some(300)),
                    ..base_params(EpicId(1))
                },
            ),
            (
                "group_by_repo",
                UpdateEpicParams {
                    group_by_repo: Some(true),
                    ..base_params(EpicId(1))
                },
            ),
            (
                "parent_epic_id",
                UpdateEpicParams {
                    parent_epic_id: Some(Some(EpicId(2))),
                    ..base_params(EpicId(1))
                },
            ),
        ];
        for (expected, params) in &cases {
            assert!(
                params.has_any_field(),
                "has_any_field() should be true when {expected} is set"
            );
            assert_eq!(
                params.updated_field_names(),
                vec![*expected],
                "setting {expected} should report exactly that field name"
            );
        }
    }

    #[tokio::test]
    async fn create_epic_returns_the_post_patch_epic() {
        // sort_order / feed_command / feed_interval_secs are applied in a
        // second write; the returned Epic must carry them, not the pre-patch
        // insert result.
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let svc = EpicService::new(db.clone());

        let epic = svc
            .create_epic(CreateEpicParams {
                title: "E".to_string(),
                description: String::new(),
                sort_order: Some(42),
                parent_epic_id: None,
                feed_command: Some("gh api repos/x/pulls".to_string()),
                feed_interval_secs: Some(300),
            })
            .await
            .unwrap();

        assert_eq!(epic.sort_order, Some(42));
        assert_eq!(epic.feed_command.as_deref(), Some("gh api repos/x/pulls"));
        assert_eq!(epic.feed_interval_secs, Some(300));
    }

    #[tokio::test]
    async fn update_epic_sets_group_by_repo() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("Test", "", None).await.unwrap();
        assert!(!epic.group_by_repo);
        let svc = EpicService::new(db.clone());
        svc.update_epic(UpdateEpicParams {
            group_by_repo: Some(true),
            ..base_params(epic.id)
        })
        .await
        .unwrap();
        let updated = db.get_epic(epic.id).await.unwrap().unwrap();
        assert!(updated.group_by_repo);
    }

    fn epic_svc_with_clock(
        db: Arc<Database>,
        clock: Arc<dyn crate::service::Clock>,
    ) -> EpicService {
        EpicService::new(db).with_clock(clock)
    }

    #[tokio::test]
    async fn update_epic_entering_done_sets_sort_order() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("Test", "", None).await.unwrap();
        let clock = Arc::new(crate::service::FixedClock::new(chrono::Utc::now()));
        let svc = epic_svc_with_clock(db.clone(), clock);

        svc.update_epic(UpdateEpicParams {
            status: Some(TaskStatus::Done),
            ..base_params(epic.id)
        })
        .await
        .unwrap();

        let updated = db.get_epic(epic.id).await.unwrap().unwrap();
        assert!(
            updated.sort_order.is_some_and(|so| so < 0),
            "expected a negative sort_order on entering Done, got {:?}",
            updated.sort_order
        );
    }

    #[tokio::test]
    async fn update_epic_leaving_done_clears_sort_order() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("Test", "", None).await.unwrap();
        let svc = EpicService::new(db.clone());

        svc.update_epic(UpdateEpicParams {
            status: Some(TaskStatus::Done),
            ..base_params(epic.id)
        })
        .await
        .unwrap();
        assert!(db
            .get_epic(epic.id)
            .await
            .unwrap()
            .unwrap()
            .sort_order
            .is_some());

        svc.update_epic(UpdateEpicParams {
            status: Some(TaskStatus::Backlog),
            ..base_params(epic.id)
        })
        .await
        .unwrap();

        let updated = db.get_epic(epic.id).await.unwrap().unwrap();
        assert_eq!(updated.sort_order, None);
    }

    #[tokio::test]
    async fn update_epic_unrelated_field_edit_while_done_leaves_sort_order_untouched() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("Test", "", None).await.unwrap();
        let svc = EpicService::new(db.clone());

        svc.update_epic(UpdateEpicParams {
            status: Some(TaskStatus::Done),
            ..base_params(epic.id)
        })
        .await
        .unwrap();
        let sort_order_after_entry = db.get_epic(epic.id).await.unwrap().unwrap().sort_order;

        svc.update_epic(UpdateEpicParams {
            title: Some("Renamed".to_string()),
            ..base_params(epic.id)
        })
        .await
        .unwrap();

        let updated = db.get_epic(epic.id).await.unwrap().unwrap();
        assert_eq!(updated.sort_order, sort_order_after_entry);
    }

    #[tokio::test]
    async fn create_sub_epic_succeeds() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let svc = EpicService::new(db.clone());
        let parent = db.create_epic("Parent", "", None).await.unwrap();
        let sub = svc
            .create_epic(CreateEpicParams {
                title: "Sub".into(),
                description: "".into(),
                sort_order: None,
                parent_epic_id: Some(parent.id),
                feed_command: None,
                feed_interval_secs: None,
            })
            .await
            .unwrap();
        assert_eq!(sub.parent_epic_id, Some(parent.id));
    }

    #[tokio::test]
    async fn create_sub_epic_recalculates_done_parent() {
        // Regression guard: attaching a new (backlog) sub-epic to a Done
        // parent must regress the parent to Backlog immediately, not wait
        // for some unrelated task write to trigger a recalc.
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let svc = EpicService::new(db.clone());
        let parent = db.create_epic("Parent", "", None).await.unwrap();
        db.patch_epic(parent.id, &EpicPatch::new().status(TaskStatus::Done))
            .await
            .unwrap();

        svc.create_epic(CreateEpicParams {
            title: "Sub".into(),
            description: "".into(),
            sort_order: None,
            parent_epic_id: Some(parent.id),
            feed_command: None,
            feed_interval_secs: None,
        })
        .await
        .unwrap();

        let parent = db.get_epic(parent.id).await.unwrap().unwrap();
        assert_eq!(parent.status, TaskStatus::Backlog);
    }

    #[tokio::test]
    async fn create_sub_epic_missing_parent_returns_not_found() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let svc = EpicService::new(db.clone());
        let result = svc
            .create_epic(CreateEpicParams {
                title: "Sub".into(),
                description: "".into(),
                sort_order: None,
                parent_epic_id: Some(EpicId(9999)),
                feed_command: None,
                feed_interval_secs: None,
            })
            .await;
        assert!(
            matches!(result, Err(ServiceError::NotFound(_))),
            "expected NotFound for missing parent, got: {result:?}"
        );
    }

    #[tokio::test]
    async fn update_epic_sets_parent() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let svc = EpicService::new(db.clone());
        let parent = db.create_epic("Parent", "", None).await.unwrap();
        let child = db.create_epic("Child", "", None).await.unwrap();
        assert!(child.parent_epic_id.is_none());
        svc.update_epic(UpdateEpicParams {
            parent_epic_id: Some(Some(parent.id)),
            ..base_params(child.id)
        })
        .await
        .unwrap();
        let updated = db.get_epic(child.id).await.unwrap().unwrap();
        assert_eq!(updated.parent_epic_id, Some(parent.id));
    }

    #[tokio::test]
    async fn update_epic_clears_parent() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let svc = EpicService::new(db.clone());
        let parent = db.create_epic("Parent", "", None).await.unwrap();
        let child = db.create_epic("Child", "", Some(parent.id)).await.unwrap();
        assert_eq!(child.parent_epic_id, Some(parent.id));
        svc.update_epic(UpdateEpicParams {
            parent_epic_id: Some(None),
            ..base_params(child.id)
        })
        .await
        .unwrap();
        let updated = db.get_epic(child.id).await.unwrap().unwrap();
        assert!(updated.parent_epic_id.is_none());
    }

    #[tokio::test]
    async fn update_epic_parent_id_absent_is_noop() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let svc = EpicService::new(db.clone());
        let parent = db.create_epic("Parent", "", None).await.unwrap();
        let child = db.create_epic("Child", "", Some(parent.id)).await.unwrap();
        svc.update_epic(UpdateEpicParams {
            title: Some("New Title".to_string()),
            ..base_params(child.id)
        })
        .await
        .unwrap();
        let updated = db.get_epic(child.id).await.unwrap().unwrap();
        assert_eq!(updated.parent_epic_id, Some(parent.id), "parent unchanged");
    }

    #[tokio::test]
    async fn update_epic_reparent_recalculates_old_and_new_parent() {
        // Regression guard: reparenting a sub-epic changes both parents'
        // active_sub_epics set, so both must be recalculated immediately.
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let svc = EpicService::new(db.clone());
        let old_parent = db.create_epic("Old", "", None).await.unwrap();
        let new_parent = db.create_epic("New", "", None).await.unwrap();
        let child = db
            .create_epic("Child", "", Some(old_parent.id))
            .await
            .unwrap();
        // A second, still-Running child stays behind on old_parent after the
        // reparent below, so a correct recalc must regress old_parent from
        // its manually-forced Done — proving recalc actually ran rather than
        // old_parent merely keeping an unrelated status.
        let sibling = db
            .create_epic("Sibling", "", Some(old_parent.id))
            .await
            .unwrap();
        db.patch_epic(sibling.id, &EpicPatch::new().status(TaskStatus::Running))
            .await
            .unwrap();
        db.patch_epic(old_parent.id, &EpicPatch::new().status(TaskStatus::Done))
            .await
            .unwrap();
        db.patch_epic(new_parent.id, &EpicPatch::new().status(TaskStatus::Done))
            .await
            .unwrap();

        svc.update_epic(UpdateEpicParams {
            parent_epic_id: Some(Some(new_parent.id)),
            ..base_params(child.id)
        })
        .await
        .unwrap();

        let old_parent = db.get_epic(old_parent.id).await.unwrap().unwrap();
        let new_parent = db.get_epic(new_parent.id).await.unwrap().unwrap();
        assert_eq!(
            old_parent.status,
            TaskStatus::Backlog,
            "old parent still has a Running child and should regress from its stale Done"
        );
        assert_eq!(
            new_parent.status,
            TaskStatus::Backlog,
            "new parent gains a backlog child and should regress from done"
        );
    }

    #[tokio::test]
    async fn update_epic_status_change_recalculates_parent() {
        // Regression guard: explicitly setting a sub-epic's status changes
        // its parent's active_sub_epics rollup and must recalculate it.
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let svc = EpicService::new(db.clone());
        let parent = db.create_epic("Parent", "", None).await.unwrap();
        let child = db.create_epic("Child", "", Some(parent.id)).await.unwrap();

        svc.update_epic(UpdateEpicParams {
            status: Some(TaskStatus::Done),
            ..base_params(child.id)
        })
        .await
        .unwrap();
        let parent_after_child_done = db.get_epic(parent.id).await.unwrap().unwrap();
        assert_eq!(parent_after_child_done.status, TaskStatus::Done);

        // Regress the child back to Running — parent must be recalculated
        // immediately, not left stale at Done.
        svc.update_epic(UpdateEpicParams {
            status: Some(TaskStatus::Running),
            ..base_params(child.id)
        })
        .await
        .unwrap();

        let parent = db.get_epic(parent.id).await.unwrap().unwrap();
        assert_eq!(parent.status, TaskStatus::Backlog);
    }

    #[tokio::test]
    async fn update_epic_cycle_detection() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let svc = EpicService::new(db.clone());
        let a = db.create_epic("A", "", None).await.unwrap();
        let b = db.create_epic("B", "", Some(a.id)).await.unwrap();
        // Trying to set A's parent to B would create a cycle: A → B → A
        let result = svc
            .update_epic(UpdateEpicParams {
                parent_epic_id: Some(Some(b.id)),
                ..base_params(a.id)
            })
            .await;
        assert!(
            matches!(result, Err(ServiceError::Validation(_))),
            "expected Validation error for cycle, got: {:?}",
            result
        );
        // DB must be unchanged
        let a_after = db.get_epic(a.id).await.unwrap().unwrap();
        assert!(a_after.parent_epic_id.is_none());
    }

    #[tokio::test]
    async fn update_epic_self_parent_rejected() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let svc = EpicService::new(db.clone());
        let epic = db.create_epic("Epic", "", None).await.unwrap();
        let result = svc
            .update_epic(UpdateEpicParams {
                parent_epic_id: Some(Some(epic.id)),
                ..base_params(epic.id)
            })
            .await;
        assert!(
            matches!(result, Err(ServiceError::Validation(_))),
            "expected Validation error for self-parent, got: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn reparent_repo_group_sub_epic_is_rejected() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let svc = EpicService::new(db.clone());
        let root = db.create_epic("root", "", None).await.unwrap();
        let other = db.create_epic("other", "", None).await.unwrap();
        let sub = db
            .create_repo_group_sub_epic(root.id, "alpha")
            .await
            .unwrap();

        let err = svc
            .update_epic(UpdateEpicParams {
                epic_id: sub,
                parent_epic_id: Some(Some(other.id)),
                title: None,
                description: None,
                status: None,
                plan_path: None,
                sort_order: None,
                auto_dispatch: None,
                feed_command: None,
                feed_interval_secs: None,
                group_by_repo: None,
            })
            .await;
        assert!(
            matches!(err, Err(ServiceError::Validation(_))),
            "expected Validation error for reparenting a RepoGroup sub-epic, got: {:?}",
            err
        );
    }

    #[tokio::test]
    async fn detach_repo_group_sub_epic_is_rejected() {
        // Nice-to-have guard: detaching (Some(None)) a RepoGroup sub-epic to root
        // must be rejected, just like reparenting it to another epic.
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let svc = EpicService::new(db.clone());
        let root = db.create_epic("root", "", None).await.unwrap();
        let sub = db
            .create_repo_group_sub_epic(root.id, "alpha")
            .await
            .unwrap();

        let err = svc
            .update_epic(UpdateEpicParams {
                epic_id: sub,
                parent_epic_id: Some(None), // detach to root
                title: None,
                description: None,
                status: None,
                plan_path: None,
                sort_order: None,
                auto_dispatch: None,
                feed_command: None,
                feed_interval_secs: None,
                group_by_repo: None,
            })
            .await;
        assert!(
            matches!(err, Err(ServiceError::Validation(_))),
            "expected Validation error for detaching a RepoGroup sub-epic, got: {:?}",
            err
        );
    }

    #[tokio::test]
    async fn detach_manual_sub_epic_is_allowed() {
        // Regression guard: detaching a Manual sub-epic to root must still work.
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let svc = EpicService::new(db.clone());
        let parent = db.create_epic("parent", "", None).await.unwrap();
        let child = db.create_epic("child", "", Some(parent.id)).await.unwrap();
        assert_eq!(child.parent_epic_id, Some(parent.id));

        svc.update_epic(UpdateEpicParams {
            parent_epic_id: Some(None),
            ..base_params(child.id)
        })
        .await
        .unwrap();

        let updated = db.get_epic(child.id).await.unwrap().unwrap();
        assert!(
            updated.parent_epic_id.is_none(),
            "Manual sub-epic can be detached"
        );
    }

    #[tokio::test]
    async fn progress_aggregates_descendants_for_grouped_epic() {
        use crate::db::{EpicCrud as _, TaskCrud as _};
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let svc = EpicService::new(db.clone());
        let root = db.create_epic("root", "", None).await.unwrap();
        db.patch_epic(root.id, &crate::db::EpicPatch::new().group_by_repo(true))
            .await
            .unwrap();
        let sub = db
            .create_repo_group_sub_epic(root.id, "alpha")
            .await
            .unwrap();
        db.create_task(crate::db::CreateTaskRequest {
            title: "t",
            description: "",
            repo_path: "/x/alpha",
            plan: None,
            status: crate::models::TaskStatus::Backlog,
            base_branch: "main",
            epic_id: Some(sub),
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

        let rows = svc.list_epics_with_progress().await.unwrap();
        let (_, _done, total) = rows.iter().find(|(e, _, _)| e.id == root.id).unwrap();
        assert_eq!(*total, 1, "grouped root aggregates descendant task counts");
    }
}
