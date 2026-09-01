//! The `reviews_parent` sync strategy: reconcile a review epic's whole
//! role-sub-epic subtree from one emission, with global `external_id` identity
//! across the role sub-epics.
//!
//! This module owns the orchestration ([`run_role_routed_feed_sync`]) and the
//! role sub-epic scaffolding ([`RoleSubEpics`], [`ensure_role_sub_epics`],
//! [`build_existing_task_index`], [`recalculate_subtree`]). The per-emission
//! phases live in sibling modules: [`super::routing`] (route/group),
//! [`super::upsert`] (insert/update), [`super::stale`] (delete absent + clear
//! the parent).

use std::collections::HashMap;

use super::routing::route_and_group_entries;
use super::stale::{clear_parent_stranded_tasks, delete_stale_subtree};
use super::upsert::upsert_role_groups;
use super::{FeedItemWithTarget, FeedSyncOutcome};
use crate::db::TaskStore;
use crate::models::{Epic, EpicId, Task};
use anyhow::Result;

/// Display title used when the role sub-epic must be created. The role
/// identity (`feed_role`) is stable; the title is user-editable afterwards.
fn role_sub_epic_title(role: crate::models::FeedRole) -> &'static str {
    use crate::models::FeedRole;
    match role {
        FeedRole::MyReviews => "My Reviews",
        FeedRole::TeamReviews => "Team Reviews",
        FeedRole::Bots => "Bots",
        // Not reachable from `route` (which only yields the three above), but
        // keep the match total without a misleading title.
        _ => "Reviews",
    }
}

/// Find the role sub-epic carrying `role` among `existing_subs`, creating it
/// under `parent_id` if absent. Idempotent and matched by `feed_role` (not
/// title), so a user rename is preserved. Reuses any existing sub-epic with the
/// role — including an archived one — because the partial unique index on
/// `(parent_epic_id, feed_role)` forbids a second sub-epic with the same role.
///
/// `existing_subs` is the parent's sub-epic list, fetched once by the caller
/// and shared across the three role lookups (the roles are distinct, so a
/// create for one role never invalidates the lookup of another).
async fn ensure_role_sub_epic(
    db: &dyn TaskStore,
    parent_id: EpicId,
    existing_subs: &[crate::models::Epic],
    role: crate::models::FeedRole,
) -> Result<EpicId> {
    if let Some(existing) = existing_subs.iter().find(|e| e.feed_role == role) {
        return Ok(existing.id);
    }
    let created = db
        .create_epic(role_sub_epic_title(role), "", Some(parent_id))
        .await?;
    db.patch_epic(created.id, &crate::db::EpicPatch::new().feed_role(role))
        .await?;
    Ok(created.id)
}

/// The three role sub-epics of a `reviews_parent` epic, plus the parent's
/// sub-epic list fetched once and reused for `can_auto_group` lookups (the
/// roles are distinct, so creating one never affects another).
pub(super) struct RoleSubEpics {
    pub(super) my: EpicId,
    pub(super) team: EpicId,
    pub(super) bots: EpicId,
    existing_subs: Vec<Epic>,
}

impl RoleSubEpics {
    pub(super) fn ids(&self) -> [EpicId; 3] {
        [self.my, self.team, self.bots]
    }

    /// `route` only ever yields My/Team/Bots; My is also the safe fallback.
    pub(super) fn target_for(&self, role: crate::models::FeedRole) -> EpicId {
        use crate::models::FeedRole;
        match role {
            FeedRole::TeamReviews => self.team,
            FeedRole::Bots => self.bots,
            _ => self.my,
        }
    }

    /// Extract can_auto_group from `existing_subs` (already loaded), avoiding
    /// an extra get_epic round-trip. A newly-created role sub-epic is not in
    /// the list and defaults to false (group_by_repo is false on creation).
    pub(super) fn can_auto_group(&self, id: EpicId) -> bool {
        self.existing_subs
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.can_auto_group())
            .unwrap_or(false)
    }
}

/// Ensure the three role sub-epics exist (idempotent, matched by feed_role).
async fn ensure_role_sub_epics(db: &dyn TaskStore, parent_id: EpicId) -> Result<RoleSubEpics> {
    use crate::models::FeedRole;
    let existing_subs = db.list_sub_epics(parent_id).await?;
    let my = ensure_role_sub_epic(db, parent_id, &existing_subs, FeedRole::MyReviews).await?;
    let team = ensure_role_sub_epic(db, parent_id, &existing_subs, FeedRole::TeamReviews).await?;
    let bots = ensure_role_sub_epic(db, parent_id, &existing_subs, FeedRole::Bots).await?;
    Ok(RoleSubEpics {
        my,
        team,
        bots,
        existing_subs,
    })
}

/// Index existing subtree feed tasks by external_id (global identity across
/// the role sub-epics). Scan repo-group sub-epics FIRST so that role sub-epic
/// copies overwrite them below — ensuring role sub-epic copies win when both
/// exist. This prevents duplicate-insert constraint violations on the MOVE
/// path when group_by_repo is off but orphaned repo-group tasks still exist.
///
/// Also returns the pre-existing repo-group child IDs so the caller can
/// recalculate them after the stale-deletion pass — even for sub-epics not
/// written this cycle.
///
/// Scans the PARENT itself FIRST (lowest priority): a feed task stranded flat
/// on the reviews_parent epic — e.g. from an out-of-band flat upsert — is
/// folded into the index so the routing loop MOVES it down into its role
/// sub-epic (never a duplicate insert, so no subtree-uniqueness collision).
/// Scanned before the sub-epics so a genuine sub-epic copy wins the identity
/// if both somehow exist. A parent-stranded task absent from this emission is
/// instead removed by the parent clear in [`clear_parent_stranded_tasks`].
async fn build_existing_task_index(
    db: &dyn TaskStore,
    parent_id: EpicId,
    roles: &RoleSubEpics,
) -> Result<(HashMap<String, Task>, Vec<EpicId>)> {
    let mut existing: HashMap<String, Task> = HashMap::new();
    let mut pre_existing_repo_group_ids: Vec<EpicId> = Vec::new();

    for task in db.list_tasks_for_epic(parent_id).await? {
        if let Some(ext) = task.external_id.clone() {
            existing.insert(ext, task);
        }
    }
    for sub in roles.ids() {
        let children = db.list_sub_epics(sub).await?;
        for child in &children {
            for task in db.list_tasks_for_epic(child.id).await? {
                if let Some(ext) = task.external_id.clone() {
                    existing.insert(ext, task);
                }
            }
        }
        pre_existing_repo_group_ids.extend(children.iter().map(|e| e.id));
    }
    // Role sub-epics scanned last — their copies overwrite repo-group copies.
    for sub in roles.ids() {
        for task in db.list_tasks_for_epic(sub).await? {
            if let Some(ext) = task.external_id.clone() {
                existing.insert(ext, task);
            }
        }
    }

    Ok((existing, pre_existing_repo_group_ids))
}

/// Recalculate: repo-group sub-epics first (they propagate upward to role
/// sub-epics), then role sub-epics, then the parent.
async fn recalculate_subtree(
    db: &dyn TaskStore,
    parent_id: EpicId,
    roles: &RoleSubEpics,
    repo_group_ids: &std::collections::HashSet<EpicId>,
) {
    for id in repo_group_ids {
        crate::feed::recalculate_epic_status_after_feed(db, *id, "run_role_routed_feed_sync").await;
    }
    for sub in roles.ids() {
        crate::feed::recalculate_epic_status_after_feed(db, sub, "run_role_routed_feed_sync").await;
    }
    crate::feed::recalculate_epic_status_after_feed(db, parent_id, "run_role_routed_feed_sync")
        .await;
}

/// Reconcile a `reviews_parent` epic's whole role-sub-epic subtree from one
/// emission, with **global `external_id` identity** across the role sub-epics:
///
/// - A PR whose `route(signals)` differs from its current role is **moved**
///   (`set_task_epic_id` + `patch_task`), preserving status / sub_status /
///   worktree / tmux_window / sort_order — an in-flight review agent keeps its
///   session.
/// - A PR the user authored themselves (`Signal::AuthorMe`) is dropped before
///   any of this: see [`crate::feed::excluded_from_reviews`]. Because it is
///   absent from the keep-set, an existing task for one is removed like a
///   merged PR's.
/// - A PR seen for the first time is inserted into its target role sub-epic.
/// - A PR absent from the emission (merged/closed) is removed by a **single**
///   subtree-scoped delete, run once with the union of all emitted ids so a
///   just-moved task is never deleted. Manual tasks (`external_id IS NULL`) are
///   preserved.
///
/// Steps run as one non-interleaved unit per parent tick. Returns a
/// [`FeedSyncOutcome`] whose `affected_epics` is the parent id plus the three
/// role sub-epic ids (for TUI notification), mirroring
/// [`super::grouped::sync_grouped_feed`]'s return contract, and whose `removed`
/// is every row the three write phases deleted that still owned a worktree or
/// tmux window (for teardown).
///
/// **Phase order is load-bearing and NOT compiler-enforced.** `apply_move`'s
/// `set_task_epic_id` (inside [`route_and_group_entries`]) must land BEFORE
/// [`upsert_role_groups`], [`delete_stale_subtree`] and
/// [`clear_parent_stranded_tasks`], each of whose SQL filters on the task's
/// CURRENT `epic_id`. Mis-ordering them would report a MOVED task as removed
/// and so force-remove a live review agent's worktree. Pinned by
/// `moved_task_is_never_reported_as_removed` in `super::tests`.
///
/// Takes a single owned `Vec<FeedItemWithTarget>` rather than three parallel
/// slices, so per-index alignment with `repo_path`/`base_branch` is
/// structural — there is no length-mismatch guard here, unlike the earlier
/// version of this function (per-item pairing cannot come apart).
pub(super) async fn run_role_routed_feed_sync(
    db: &dyn TaskStore,
    parent_id: EpicId,
    entries: Vec<FeedItemWithTarget>,
    mode: super::SyncMode,
) -> Result<FeedSyncOutcome> {
    let roles = ensure_role_sub_epics(db, parent_id).await?;
    let (existing, pre_existing_repo_group_ids) =
        build_existing_task_index(db, parent_id, &roles).await?;

    // Phase 0: drop own-authored items (feeds.allium ExcludeOwnAuthored). This
    // runs BEFORE routing and before the keep-set is built, so an excluded item
    // is neither inserted nor retained — an existing task for one is removed by
    // the stale pass below exactly as a merged PR would be.
    let entries: Vec<FeedItemWithTarget> = entries
        .into_iter()
        .filter(|e| !crate::feed::excluded_from_reviews(&e.item.signals))
        .collect();

    let routed = route_and_group_entries(db, parent_id, entries, &existing, &roles).await?;

    // Phase 2 runs in both modes — inserts, field refreshes and the moves
    // already applied above all follow from what the emission CONTAINS. Phases
    // 3 and 4 are the two removal mechanisms and are skipped wholesale when the
    // emission's omissions are not trusted (feeds.allium:
    // DegradedNonEmptyEmission).
    let mut removed = upsert_role_groups(db, parent_id, routed.groups, mode).await;
    if mode.removes_absent() {
        removed.extend(delete_stale_subtree(db, parent_id, &roles, &routed.all_external_ids).await);
        removed.extend(clear_parent_stranded_tasks(db, parent_id).await);
    }

    let repo_group_ids: std::collections::HashSet<EpicId> = routed
        .repo_group_cache
        .into_values()
        .chain(pre_existing_repo_group_ids)
        .collect();
    recalculate_subtree(db, parent_id, &roles, &repo_group_ids).await;

    let mut affected_epics = vec![parent_id, roles.my, roles.team, roles.bots];
    affected_epics.extend(repo_group_ids);
    Ok(FeedSyncOutcome {
        affected_epics,
        removed,
    })
}
