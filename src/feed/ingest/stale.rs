//! Role-routed phase 3: delete tasks absent from the emission and clear any
//! feed task stranded flat on the reviews_parent epic.

use super::role_routed::RoleSubEpics;
use crate::db::TaskStore;
use crate::models::EpicId;

/// Subtree-scoped delete: removes merged/closed PRs from flat role sub-epics
/// and clears role sub-epics absent from this emission (moved tasks are in
/// the keep-set, so they are never deleted here), then a second pass at the
/// role level to cover repo-group grandchildren — always run, not only for
/// grouped roles, so orphaned repo-group tasks are cleaned up when
/// group_by_repo is off. The SQL is one level deep, so calling it with the
/// role sub-epic as root reaches its repo-group children — exactly the
/// grandchild level relative to the parent.
pub(super) async fn delete_stale_subtree(
    db: &dyn TaskStore,
    parent_id: EpicId,
    roles: &RoleSubEpics,
    all_external_ids: &[String],
) {
    crate::feed::warn_on_err(
        db.delete_stale_subtree_feed_tasks(parent_id, all_external_ids)
            .await,
        parent_id,
        None,
        "run_role_routed_feed_sync: delete_stale_subtree_feed_tasks failed",
    );

    for sub in roles.ids() {
        crate::feed::warn_on_err(
            db.delete_stale_subtree_feed_tasks(sub, all_external_ids)
                .await,
            parent_id,
            Some(sub),
            "run_role_routed_feed_sync: delete_stale_subtree_feed_tasks (role level) failed",
        );
    }
}

/// Parent sweep: a reviews_parent epic must hold NO feed-managed task
/// directly (NoFlatFeedTasksOnReviewsParent). Present parent-stranded tasks
/// were already MOVED down by [`super::routing::route_and_group_entries`], so
/// anything left here is either a merged/closed stray or a legacy duplicate
/// whose routed copy already lives in a sub-epic — both must go. An empty
/// upsert reuses `upsert_feed_tasks`' per-epic stale-delete (external_id NOT
/// IN {} deletes every feed task on the epic) in ONE statement, preserving
/// manual tasks (external_id IS NULL). Same idiom
/// [`super::grouped::sync_grouped_feed`] uses to clear the parent on the
/// grouped path.
pub(super) async fn clear_parent_stranded_tasks(db: &dyn TaskStore, parent_id: EpicId) {
    crate::feed::warn_on_err(
        db.upsert_feed_tasks(parent_id, &[], &[], &[]).await,
        parent_id,
        None,
        "run_role_routed_feed_sync: failed to clear parent-stranded feed tasks",
    );
}
