//! The `group_by_repo` sync strategy: group an emission's items by repo name
//! and upsert each group into its own per-repo sub-epic, reconciling the whole
//! grouped subtree so the feed stays the source of truth.

use std::collections::HashMap;

use super::FeedItemWithTarget;
use crate::db::{RemovedFeedTask, TaskStore};
use crate::models::{EpicId, FeedItem};

/// Upsert `items` into `sub_epic_id`, then recalculate its status on success
/// (which propagates upward to the parent). Logs a warning on failure. Shared
/// by both reconciliation paths of [`sync_grouped_feed`]: the present-group
/// upsert and the absent-sub-epic clear (called with empty slices).
///
/// Returns the stale rows the upsert deleted that still owned a worktree or
/// tmux window, for the caller to tear down (empty on failure).
/// On [`SyncMode::Additive`] the additive upsert variant is used, so an item
/// that dropped out of this group's emission is left in place. The mode picks
/// the DB variant and nothing else: the recalculate-on-success rule and the
/// failure reporting are one shared tail, so they cannot come to mean different
/// things in the two modes.
///
/// [`SyncMode::Additive`]: super::SyncMode::Additive
async fn upsert_sub_epic_and_recalc(
    db: &dyn TaskStore,
    parent_id: EpicId,
    sub_epic_id: EpicId,
    items: &[FeedItem],
    repo_paths: &[String],
    base_branches: &[String],
    mode: super::SyncMode,
) -> Vec<RemovedFeedTask> {
    let result = if mode.removes_absent() {
        db.upsert_feed_tasks(sub_epic_id, items, repo_paths, base_branches)
            .await
    } else {
        db.upsert_feed_tasks_additive(sub_epic_id, items, repo_paths, base_branches)
            .await
    };
    if result.is_ok() {
        crate::feed::recalculate_epic_status_after_feed(db, sub_epic_id, "sync_grouped_feed").await;
    }
    crate::feed::removed_or_warn(
        result,
        parent_id,
        Some(sub_epic_id),
        "sync_grouped_feed: upsert_feed_tasks failed",
    )
}

/// Clear a sub-epic's feed tasks: the empty-emission idiom, which relies on
/// `upsert_feed_tasks`' stale-delete removing every feed task on the epic.
///
/// Deliberately mode-free rather than taking a `SyncMode` and passing
/// `Reconcile`. CLEARING IS A RECONCILE ACT BY DEFINITION — there is no
/// additive way to clear — so the mode question belongs at the caller that
/// decides whether to clear at all, not inside a helper whose every call would
/// have to name `Reconcile` and be trusted not to name the other one.
async fn clear_sub_epic_and_recalc(
    db: &dyn TaskStore,
    parent_id: EpicId,
    sub_epic_id: EpicId,
) -> Vec<RemovedFeedTask> {
    upsert_sub_epic_and_recalc(
        db,
        parent_id,
        sub_epic_id,
        &[],
        &[],
        &[],
        super::SyncMode::Reconcile,
    )
    .await
}

/// Phase 1: group entries by repo name, moving each entry into its group (no
/// clone). Takes a single owned `Vec` of paired entries rather than three
/// parallel slices, so per-index alignment is structural — the old
/// length-mismatch guard (and the silent-truncation footgun it papered over)
/// is gone.
fn group_by_repo(entries: Vec<FeedItemWithTarget>) -> HashMap<String, Vec<FeedItemWithTarget>> {
    let mut groups: HashMap<String, Vec<FeedItemWithTarget>> = HashMap::new();
    for entry in entries {
        let name = crate::models::repo_name_from_url(&entry.item.url);
        groups.entry(name).or_default().push(entry);
    }
    groups
}

/// Phase 2: find-or-create each present group's sub-epic and upsert its
/// items. Returns the sub-epic IDs written to (used by the caller to notify
/// the TUI, even when an individual upsert fails — partial writes are still
/// visible), paired with the removed rows needing teardown.
async fn upsert_present_groups(
    db: &dyn TaskStore,
    parent_id: EpicId,
    groups: HashMap<String, Vec<FeedItemWithTarget>>,
    active_sub_epics: &[&crate::models::Epic],
    mode: super::SyncMode,
) -> (Vec<EpicId>, Vec<RemovedFeedTask>) {
    let mut sub_epic_ids = Vec::new();
    let mut removed = Vec::new();
    for (repo_name, group) in groups {
        let (group_items, group_repo_paths, group_base_branches) = FeedItemWithTarget::unzip(group);

        let sub_epic_id =
            if let Some(existing) = active_sub_epics.iter().find(|e| e.title == repo_name) {
                existing.id
            } else {
                match db.create_epic(&repo_name, "", Some(parent_id)).await {
                    Ok(e) => e.id,
                    Err(err) => {
                        tracing::warn!(
                            epic_id = parent_id.0,
                            repo = %repo_name,
                            "sync_grouped_feed: create_epic failed: {err:#}"
                        );
                        continue;
                    }
                }
            };

        sub_epic_ids.push(sub_epic_id);

        // New backlog tasks may regress a done sub-epic; the recalculation
        // inside the helper propagates upward to the parent.
        removed.extend(
            upsert_sub_epic_and_recalc(
                db,
                parent_id,
                sub_epic_id,
                &group_items,
                &group_repo_paths,
                &group_base_branches,
                mode,
            )
            .await,
        );
    }
    (sub_epic_ids, removed)
}

/// Phase 3: clear feed tasks from any active sub-epic whose repo contributed
/// no item this emission (`group_names`), so feed-as-source-of-truth holds
/// across the whole grouped subtree, not just the present repos. When
/// `group_names` is empty (the feed returned nothing) every active sub-epic
/// is cleared. `upsert_feed_tasks` with an empty item list reuses the
/// external_id-based deletion, so manually-added tasks (external_id = NULL)
/// are preserved and the sub-epic row itself is left in place.
async fn clear_absent_sub_epics(
    db: &dyn TaskStore,
    parent_id: EpicId,
    active_sub_epics: &[&crate::models::Epic],
    group_names: &std::collections::HashSet<String>,
) -> (Vec<EpicId>, Vec<RemovedFeedTask>) {
    let mut sub_epic_ids = Vec::new();
    let mut removed = Vec::new();
    for sub_epic in active_sub_epics
        .iter()
        .filter(|e| !group_names.contains(&e.title))
    {
        // Surface the cleared sub-epic to the caller so the TUI refreshes it.
        sub_epic_ids.push(sub_epic.id);
        removed.extend(clear_sub_epic_and_recalc(db, parent_id, sub_epic.id).await);
    }
    (sub_epic_ids, removed)
}

/// Phase 4: clear any flat feed tasks left on the parent (migration + ongoing
/// hygiene), regardless of per-group failures in phases 2/3.
async fn clear_parent_flat_tasks(db: &dyn TaskStore, parent_id: EpicId) -> Vec<RemovedFeedTask> {
    let result = db.upsert_feed_tasks(parent_id, &[], &[], &[]).await;
    if result.is_ok() {
        // Recalculate the parent's status after its flat tasks are cleared.
        // Sub-epic recalculations above already propagate upward, but this
        // handles the edge case where all sub-epics failed their upserts and
        // the parent's flat task list is now empty.
        crate::feed::recalculate_epic_status_after_feed(db, parent_id, "sync_grouped_feed").await;
    }
    crate::feed::removed_or_warn(
        result,
        parent_id,
        None,
        "sync_grouped_feed: failed to clear parent feed tasks",
    )
}

/// Group feed items by repo name and upsert each group into its own sub-epic.
/// Clears any flat feed tasks on the parent epic (migration + ongoing
/// hygiene). Returns the IDs of all sub-epics that were found or created,
/// paired with every removed row that still owned a worktree or tmux window
/// (for teardown by the caller).
///
/// Orchestrates four phases, in order: [`group_by_repo`] (pure),
/// [`upsert_present_groups`], [`clear_absent_sub_epics`], then
/// [`clear_parent_flat_tasks`]. `group_names` must be captured from `groups`
/// before it is moved into `upsert_present_groups` — that dependency is
/// structural (ownership), not just prose; the present-vs-absent phase order
/// itself is not compiler-enforced, only asserted by this doc comment.
pub(super) async fn sync_grouped_feed(
    db: &dyn TaskStore,
    parent_id: EpicId,
    entries: Vec<FeedItemWithTarget>,
    mode: super::SyncMode,
) -> super::FeedSyncOutcome {
    let groups = group_by_repo(entries);

    let existing_sub_epics = match db.list_sub_epics(parent_id).await {
        Ok(e) => e,
        Err(err) => {
            tracing::warn!(
                epic_id = parent_id.0,
                "sync_grouped_feed: list_sub_epics failed: {err:#}"
            );
            // list_sub_epics failed: no sub-epic was written, so none is
            // notified. The parent stays in `affected_epics` regardless — the
            // caller has always notified it unconditionally.
            return super::FeedSyncOutcome {
                affected_epics: vec![parent_id],
                removed: vec![],
            };
        }
    };

    let active_sub_epics: Vec<_> = existing_sub_epics
        .iter()
        .filter(|e| e.status != crate::models::TaskStatus::Archived)
        .collect();

    // Repo names contributing an item this emission, captured before `groups`
    // is consumed by value in `upsert_present_groups`.
    let group_names: std::collections::HashSet<String> = groups.keys().cloned().collect();

    let (sub_epic_ids, mut removed) =
        upsert_present_groups(db, parent_id, groups, &active_sub_epics, mode).await;

    // Phases 3 and 4 are the grouped path's two removal mechanisms, so both are
    // skipped when the sync may not act on omissions (feeds.allium:
    // DegradedNonEmptyEmission). That also defers grouping MIGRATION — which is
    // phase 4 in disguise, deleting flat parent tasks so the sub-epic copies
    // stand alone — to the next trusted emission, rather than half-performing
    // it against a degraded one.
    //
    // Only the DEGRADED cause reaches this branch, and for it the deferral is
    // exactly right. The other cause of additivity cannot arrive here at all:
    // the service refuses feed_append_only on a grouped epic, permanently,
    // because repo is a mirroring feed's key and an append-only feed's items
    // are events keyed by where in the code they fired (feeds.allium:
    // AppendOnlyFeed).
    //
    // NOTE, and it is independent of append-only: phase 4 conflates a stale
    // delete with the grouping MIGRATION, and as a migration it is wrong for
    // MIRRORING epics too. Phase 2 inserts a fresh row into the sub-epic
    // rather than moving the parent's, so migrating a task discards its status
    // and hands its worktree and tmux window to teardown. The other two paths
    // re-home by MOVING the row (`flatten_epic`, `apply_move`); this one
    // should as well. See task #4647.
    let absent_ids = if mode.removes_absent() {
        let (absent_ids, absent_removed) =
            clear_absent_sub_epics(db, parent_id, &active_sub_epics, &group_names).await;
        removed.extend(absent_removed);
        removed.extend(clear_parent_flat_tasks(db, parent_id).await);
        absent_ids
    } else {
        Vec::new()
    };

    // Parent first, mirroring `run_role_routed_feed_sync`'s contract, so the
    // caller forwards this outcome rather than reassembling one.
    let mut affected_epics = vec![parent_id];
    affected_epics.extend(sub_epic_ids);
    affected_epics.extend(absent_ids);

    super::FeedSyncOutcome {
        affected_epics,
        removed,
    }
}
