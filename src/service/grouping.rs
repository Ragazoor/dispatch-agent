//! Repo-grouping: route tasks of a `group_by_repo` (non-feed) epic into
//! per-repo `RepoGroup` sub-epics. Operations only ever touch `RepoGroup`
//! sub-epics, never hand-made (`Manual`) ones. Each function recalculates the
//! epics it mutates, owning the status-rollup invariant.

use crate::db::TaskAndEpicStore;
use crate::models::{repo_name_from_path, EpicId, EpicOrigin, TaskId, TaskStatus};
use crate::service::ServiceError;

/// Resolve where a task assigned to `root_id` should actually live.
/// Routes into a per-repo sub-epic only when `root` is `group_by_repo` AND
/// non-feed; otherwise returns `root_id` unchanged.
pub async fn route_target(
    db: &dyn TaskAndEpicStore,
    root_id: EpicId,
    repo_path: &str,
) -> Result<EpicId, ServiceError> {
    let Some(root) = db.get_epic(root_id).await? else {
        return Ok(root_id);
    };
    if !root.can_auto_group() {
        return Ok(root_id);
    }
    ensure_repo_sub_epic(db, root_id, &repo_name_from_path(repo_path)).await
}

/// Find-or-create the `RepoGroup` sub-epic for `repo_name` under `root_id`.
pub async fn ensure_repo_sub_epic(
    db: &dyn TaskAndEpicStore,
    root_id: EpicId,
    repo_name: &str,
) -> Result<EpicId, ServiceError> {
    Ok(db.create_repo_group_sub_epic(root_id, repo_name).await?)
}

/// The grouping root governing `epic_id`, if any: the epic itself when it is a
/// grouped non-feed root, else its parent when `epic_id` is a `RepoGroup`
/// sub-epic, else `None`.
pub async fn grouping_root_of(
    db: &dyn TaskAndEpicStore,
    epic_id: EpicId,
) -> Result<Option<EpicId>, ServiceError> {
    let Some(epic) = db.get_epic(epic_id).await? else {
        return Ok(None);
    };
    if epic.can_auto_group() {
        return Ok(Some(epic.id));
    }
    if epic.origin == EpicOrigin::RepoGroup {
        if let Some(parent_id) = epic.parent_epic_id {
            if let Some(parent) = db.get_epic(parent_id).await? {
                if parent.can_auto_group() {
                    return Ok(Some(parent.id));
                }
            }
        }
    }
    Ok(None)
}

/// Migrate every non-archived direct task of `root_id` into its repo sub-epic.
pub async fn regroup_epic(db: &dyn TaskAndEpicStore, root_id: EpicId) -> Result<(), ServiceError> {
    let tasks = db.list_tasks_for_epic(root_id).await?;
    for task in tasks {
        if task.status == TaskStatus::Archived {
            continue;
        }
        let target = route_target(db, root_id, &task.repo_path).await?;
        if target != root_id {
            db.set_task_epic_id(task.id, Some(target)).await?;
            db.recalculate_epic_status(target).await?;
        }
    }
    db.recalculate_epic_status(root_id).await?;
    Ok(())
}

/// Re-home tasks from every active `RepoGroup` sub-epic back to `root_id`, then
/// delete those sub-epics if empty (no tasks, no child epics).
pub async fn flatten_epic(db: &dyn TaskAndEpicStore, root_id: EpicId) -> Result<(), ServiceError> {
    let subs = db.list_sub_epics(root_id).await?;
    for sub in &subs {
        if sub.origin != EpicOrigin::RepoGroup || sub.status == TaskStatus::Archived {
            continue;
        }
        // Re-home FIRST (delete_epic cascades to tasks — order is load-bearing).
        for task in db.list_tasks_for_epic(sub.id).await? {
            db.set_task_epic_id(task.id, Some(root_id)).await?;
        }
        delete_if_empty_repo_group(db, sub.id).await?;
    }
    db.recalculate_epic_status(root_id).await?;
    Ok(())
}

/// Move a task to the correct repo sub-epic after its repo changed; clean up the
/// now-empty source `RepoGroup` sub-epic. No-op if the task is not in a grouped
/// subtree (e.g. it sits in a `Manual` sub-epic).
pub async fn reroute_on_repo_change(
    db: &dyn TaskAndEpicStore,
    task_id: TaskId,
    new_repo: &str,
) -> Result<(), ServiceError> {
    let Some(task) = db.get_task(task_id).await? else {
        return Ok(());
    };
    let Some(current_epic) = task.epic_id else {
        return Ok(());
    };
    let Some(root) = grouping_root_of(db, current_epic).await? else {
        return Ok(());
    };
    let target = route_target(db, root, new_repo).await?;
    if target == current_epic {
        return Ok(());
    }
    db.set_task_epic_id(task_id, Some(target)).await?;
    db.recalculate_epic_status(target).await?;
    // Clean up the source only if it was a RepoGroup sub-epic (not the root).
    if current_epic != root {
        delete_if_empty_repo_group(db, current_epic).await?;
    }
    db.recalculate_epic_status(root).await?;
    Ok(())
}

/// Delete `epic_id` iff it is a `RepoGroup` sub-epic with no tasks and no
/// children; otherwise recalc it. Shared cleanup rule for flatten + reroute.
async fn delete_if_empty_repo_group(
    db: &dyn TaskAndEpicStore,
    epic_id: EpicId,
) -> Result<(), ServiceError> {
    let Some(epic) = db.get_epic(epic_id).await? else {
        return Ok(());
    };
    if epic.origin != EpicOrigin::RepoGroup {
        return Ok(());
    }
    let has_tasks = !db.list_tasks_for_epic(epic_id).await?.is_empty();
    let has_children = !db.list_sub_epics(epic_id).await?.is_empty();
    if has_tasks || has_children {
        db.recalculate_epic_status(epic_id).await?;
        return Ok(());
    }
    // Re-scope epic-scoped learnings to the parent BEFORE deleting the sub-epic,
    // so no learning is left with a dangling scope_ref (ReScopeLearningsOnRepoGroupDelete).
    if let Some(parent_id) = epic.parent_epic_id {
        db.rescope_epic_learnings(epic_id, parent_id).await?;
    }
    db.delete_epic(epic_id).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::db::{
        CreateLearningRow, Database, EpicCrud, EpicRead, LearningFilter, LearningStore, TaskCrud,
        TaskPatch,
    };
    use crate::models::{EpicId, LearningKind, LearningScope, LearningStatus, TaskStatus};

    async fn mk() -> Database {
        Database::open_in_memory().await.unwrap()
    }

    async fn add_task(db: &Database, epic: EpicId, repo: &str) -> crate::models::TaskId {
        db.create_task(crate::db::CreateTaskRequest {
            title: "t",
            description: "",
            repo_path: repo,
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: Some(epic),
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn route_target_creates_sub_epic_for_grouped_root() {
        let db = mk().await;
        let root = db.create_epic("root", "", None).await.unwrap();
        db.patch_epic(root.id, &crate::db::EpicPatch::new().group_by_repo(true))
            .await
            .unwrap();
        let target = route_target(&db, root.id, "/x/dispatch").await.unwrap();
        assert_ne!(target, root.id);
        let sub = db.get_epic(target).await.unwrap().unwrap();
        assert_eq!(sub.title, "dispatch");
        assert_eq!(sub.origin, crate::models::EpicOrigin::RepoGroup);
    }

    #[tokio::test]
    async fn route_target_noop_for_non_grouped_or_feed_root() {
        let db = mk().await;
        let plain = db.create_epic("plain", "", None).await.unwrap();
        assert_eq!(
            route_target(&db, plain.id, "/x/dispatch").await.unwrap(),
            plain.id
        );

        let feed = db.create_epic("feed", "", None).await.unwrap();
        db.patch_epic(
            feed.id,
            &crate::db::EpicPatch::new()
                .group_by_repo(true)
                .feed_command(Some("gh ...")),
        )
        .await
        .unwrap();
        assert_eq!(
            route_target(&db, feed.id, "/x/dispatch").await.unwrap(),
            feed.id
        );
    }

    #[tokio::test]
    async fn regroup_migrates_all_direct_tasks() {
        let db = mk().await;
        let root = db.create_epic("root", "", None).await.unwrap();
        db.patch_epic(root.id, &crate::db::EpicPatch::new().group_by_repo(true))
            .await
            .unwrap();
        add_task(&db, root.id, "/x/alpha").await;
        add_task(&db, root.id, "/x/beta").await;
        regroup_epic(&db, root.id).await.unwrap();
        assert!(
            db.list_tasks_for_epic(root.id).await.unwrap().is_empty(),
            "root has no direct tasks after regroup"
        );
        let subs = db.list_sub_epics(root.id).await.unwrap();
        assert_eq!(subs.len(), 2);
    }

    #[tokio::test]
    async fn flatten_rehomes_tasks_then_deletes_empty_repo_groups() {
        let db = mk().await;
        let root = db.create_epic("root", "", None).await.unwrap();
        db.patch_epic(root.id, &crate::db::EpicPatch::new().group_by_repo(true))
            .await
            .unwrap();
        add_task(&db, root.id, "/x/alpha").await;
        regroup_epic(&db, root.id).await.unwrap();

        flatten_epic(&db, root.id).await.unwrap();
        assert_eq!(
            db.list_tasks_for_epic(root.id).await.unwrap().len(),
            1,
            "task re-homed, not deleted"
        );
        assert!(
            db.list_sub_epics(root.id).await.unwrap().is_empty(),
            "emptied repo-group sub-epics deleted"
        );
    }

    #[tokio::test]
    async fn flatten_preserves_manual_sub_epics() {
        let db = mk().await;
        let root = db.create_epic("root", "", None).await.unwrap();
        db.patch_epic(root.id, &crate::db::EpicPatch::new().group_by_repo(true))
            .await
            .unwrap();
        let manual = db.create_epic("notes", "", Some(root.id)).await.unwrap(); // origin=Manual
        flatten_epic(&db, root.id).await.unwrap();
        assert!(
            db.get_epic(manual.id).await.unwrap().is_some(),
            "Manual sub-epic must survive flatten"
        );
    }

    #[tokio::test]
    async fn reroute_moves_task_to_correct_sub_epic_and_cleans_source() {
        let db = mk().await;
        let root = db.create_epic("root", "", None).await.unwrap();
        db.patch_epic(root.id, &crate::db::EpicPatch::new().group_by_repo(true))
            .await
            .unwrap();
        let t = add_task(&db, root.id, "/x/alpha").await;
        regroup_epic(&db, root.id).await.unwrap();
        // task now in "alpha" sub-epic; change its repo and reroute.
        // No set_task_repo_path helper exists; use patch_task with TaskPatch::repo_path.
        // reroute_on_repo_change itself does not write repo_path; the caller owns that.
        db.patch_task(t, &TaskPatch::new().repo_path("/x/beta"))
            .await
            .unwrap();
        reroute_on_repo_change(&db, t, "/x/beta").await.unwrap();
        let subs = db.list_sub_epics(root.id).await.unwrap();
        let titles: Vec<_> = subs.iter().map(|e| e.title.clone()).collect();
        assert!(titles.contains(&"beta".to_string()));
        assert!(
            !titles.contains(&"alpha".to_string()),
            "emptied source sub-epic cleaned up"
        );
    }

    #[tokio::test]
    async fn rescope_learnings_when_repo_group_sub_epic_deleted_by_flatten() {
        let db = mk().await;
        // Create a group_by_repo root epic.
        let root = db.create_epic("root", "", None).await.unwrap();
        db.patch_epic(root.id, &crate::db::EpicPatch::new().group_by_repo(true))
            .await
            .unwrap();
        // Route a task into a RepoGroup sub-epic so the sub-epic exists.
        add_task(&db, root.id, "/x/alpha").await;
        regroup_epic(&db, root.id).await.unwrap();

        // Capture the sub-epic's id before deletion.
        let subs = db.list_sub_epics(root.id).await.unwrap();
        let sub = subs
            .iter()
            .find(|e| e.title == "alpha")
            .expect("alpha sub-epic must exist after regroup");
        let sub_id = sub.id;

        // Create an epic-scoped learning pointing at the sub-epic.
        let sub_id_str = sub_id.0.to_string();
        let lid = db
            .create_learning(CreateLearningRow {
                kind: LearningKind::Convention,
                summary: "alpha repo convention",
                detail: None,
                scope: LearningScope::Epic,
                scope_ref: Some(&sub_id_str),
                tags: &[],
                source_task_id: None,
                embedding: None,
            })
            .await
            .unwrap();

        // Re-home tasks to root and flatten (this deletes the empty sub-epic).
        flatten_epic(&db, root.id).await.unwrap();

        // The sub-epic must be gone.
        assert!(
            db.get_epic(sub_id).await.unwrap().is_none(),
            "sub-epic should be deleted after flatten"
        );

        // The learning must now point at the root epic (re-scoped), not the deleted sub.
        let learning = db.get_learning(lid).await.unwrap().unwrap();
        assert_eq!(
            learning.scope,
            LearningScope::Epic,
            "scope must remain 'epic'"
        );
        assert_eq!(
            learning.scope_ref,
            Some(root.id.0.to_string()),
            "scope_ref must be re-scoped to the root epic"
        );
        assert_eq!(
            learning.status,
            LearningStatus::Approved,
            "status must be unchanged"
        );
    }

    #[tokio::test]
    async fn rescope_learnings_when_repo_group_sub_epic_deleted_by_reroute() {
        let db = mk().await;
        let root = db.create_epic("root", "", None).await.unwrap();
        db.patch_epic(root.id, &crate::db::EpicPatch::new().group_by_repo(true))
            .await
            .unwrap();
        let t = add_task(&db, root.id, "/x/alpha").await;
        regroup_epic(&db, root.id).await.unwrap();

        let subs = db.list_sub_epics(root.id).await.unwrap();
        let sub = subs
            .iter()
            .find(|e| e.title == "alpha")
            .expect("alpha sub-epic must exist after regroup");
        let sub_id = sub.id;
        let sub_id_str = sub_id.0.to_string();

        // Create an epic-scoped learning pointing at the alpha sub-epic.
        let lid = db
            .create_learning(CreateLearningRow {
                kind: LearningKind::Convention,
                summary: "alpha-specific learning",
                detail: None,
                scope: LearningScope::Epic,
                scope_ref: Some(&sub_id_str),
                tags: &[],
                source_task_id: None,
                embedding: None,
            })
            .await
            .unwrap();

        // Reroute the only task to beta — this should clean up the alpha sub-epic.
        db.patch_task(t, &TaskPatch::new().repo_path("/x/beta"))
            .await
            .unwrap();
        reroute_on_repo_change(&db, t, "/x/beta").await.unwrap();

        // Alpha sub-epic must be gone.
        assert!(
            db.get_epic(sub_id).await.unwrap().is_none(),
            "alpha sub-epic should be deleted after reroute"
        );

        // Learning must be re-scoped to root.
        let learning = db.get_learning(lid).await.unwrap().unwrap();
        assert_eq!(
            learning.scope_ref,
            Some(root.id.0.to_string()),
            "scope_ref must be re-scoped to root after reroute cleanup"
        );
    }

    #[tokio::test]
    async fn unscoped_and_other_learnings_untouched_during_rescope() {
        let db = mk().await;
        let root = db.create_epic("root", "", None).await.unwrap();
        db.patch_epic(root.id, &crate::db::EpicPatch::new().group_by_repo(true))
            .await
            .unwrap();
        add_task(&db, root.id, "/x/alpha").await;
        regroup_epic(&db, root.id).await.unwrap();

        let subs = db.list_sub_epics(root.id).await.unwrap();
        let sub_id = subs.iter().find(|e| e.title == "alpha").unwrap().id;
        let sub_id_str = sub_id.0.to_string();
        let root_id_str = root.id.0.to_string();

        // A user-scoped learning (no scope_ref) should be unaffected.
        let user_lid = db
            .create_learning(CreateLearningRow {
                kind: LearningKind::Convention,
                summary: "user learning",
                detail: None,
                scope: LearningScope::User,
                scope_ref: None,
                tags: &[],
                source_task_id: None,
                embedding: None,
            })
            .await
            .unwrap();

        // An epic-scoped learning pointing at the ROOT should also be unaffected.
        let root_lid = db
            .create_learning(CreateLearningRow {
                kind: LearningKind::Convention,
                summary: "root learning",
                detail: None,
                scope: LearningScope::Epic,
                scope_ref: Some(&root_id_str),
                tags: &[],
                source_task_id: None,
                embedding: None,
            })
            .await
            .unwrap();

        // The target learning on the sub-epic.
        let sub_lid = db
            .create_learning(CreateLearningRow {
                kind: LearningKind::Convention,
                summary: "sub learning",
                detail: None,
                scope: LearningScope::Epic,
                scope_ref: Some(&sub_id_str),
                tags: &[],
                source_task_id: None,
                embedding: None,
            })
            .await
            .unwrap();

        flatten_epic(&db, root.id).await.unwrap();

        // User-scoped learning: unchanged.
        let ul = db.get_learning(user_lid).await.unwrap().unwrap();
        assert_eq!(ul.scope, LearningScope::User);
        assert_eq!(ul.scope_ref, None);

        // Root-epic-scoped learning: still points at root, not modified.
        let rl = db.get_learning(root_lid).await.unwrap().unwrap();
        assert_eq!(rl.scope_ref, Some(root_id_str));

        // Sub-epic learning: re-scoped to root.
        let sl = db.get_learning(sub_lid).await.unwrap().unwrap();
        assert_eq!(sl.scope_ref, Some(root.id.0.to_string()));

        // Verify no spurious learnings created via filter.
        let all = db.list_learnings(LearningFilter::default()).await.unwrap();
        assert_eq!(all.len(), 3, "no extra learnings created");
    }
}
