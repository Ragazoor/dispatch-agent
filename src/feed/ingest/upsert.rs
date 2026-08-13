//! Role-routed phase 2: insert/update the present role groups.

use std::collections::HashMap;

use super::FeedItemWithTarget;
use crate::db::{RemovedFeedTask, TaskStore};
use crate::models::EpicId;

/// Insert/update present roles. Because every cross-role task was already
/// moved out of its losing epic by [`super::routing::route_and_group_entries`],
/// `upsert_feed_tasks`' per-epic delete only ever removes genuinely-stale
/// rows here — never a moved task.
///
/// Returns those genuinely-stale removals that still owned a worktree or tmux
/// window, for the caller to tear down. On [`SyncMode::Additive`] the additive
/// variant of the upsert is used instead, so present roles are still written
/// but nothing absent from the emission is deleted — the return is then always
/// empty.
///
/// [`SyncMode::Additive`]: super::SyncMode::Additive
pub(super) async fn upsert_role_groups(
    db: &dyn TaskStore,
    parent_id: EpicId,
    groups: HashMap<EpicId, Vec<FeedItemWithTarget>>,
    mode: super::SyncMode,
) -> Vec<RemovedFeedTask> {
    let mut removed = Vec::new();
    for (sub_id, group) in groups {
        let (items, repo_paths, base_branches) = FeedItemWithTarget::unzip(group);
        if mode.removes_absent() {
            removed.extend(crate::feed::removed_or_warn(
                db.upsert_feed_tasks(sub_id, &items, &repo_paths, &base_branches)
                    .await,
                parent_id,
                Some(sub_id),
                "run_role_routed_feed_sync: upsert_feed_tasks failed",
            ));
        } else if let Err(err) = db
            .upsert_feed_tasks_additive(sub_id, &items, &repo_paths, &base_branches)
            .await
        {
            tracing::warn!(
                epic_id = parent_id.0,
                sub_epic_id = sub_id.0,
                "run_role_routed_feed_sync: upsert_feed_tasks_additive failed: {err:#}"
            );
        }
    }
    removed
}
