//! Feed ingestion: upsert an emission's items into the correct epic subtree.
//!
//! The pipeline has two independent sync strategies plus a shared entry type:
//!
//! - [`grouped`] — the `group_by_repo` path ([`grouped::sync_grouped_feed`]):
//!   group items by repo name into per-repo sub-epics.
//! - [`role_routed`] — the `reviews_parent` path
//!   ([`run_role_routed_feed_sync`]): route each PR to its role sub-epic with
//!   global `external_id` identity, moving cross-role tasks in place. Its phases
//!   live in [`routing`] (route/group), [`upsert`] (insert/update), and
//!   [`stale`] (delete absent tasks + clear the parent).
//!
//! [`FeedItemWithTarget`] is the shared entry — a feed item paired with its
//! resolved repo path and base branch — assembled once at the feed boundary.
//! [`run_feed_sync_by_role`] dispatches an emission to the right strategy.

mod grouped;
mod role_routed;
mod routing;
mod stale;
mod upsert;

#[cfg(test)]
mod tests;

use role_routed::run_role_routed_feed_sync;

use crate::db::{RemovedFeedTask, TaskStore};
use crate::models::{EpicId, FeedItem};
use anyhow::Result;

/// Whether a sync pass may act on what its emission OMITS.
///
/// Decided once per feed cycle and threaded unchanged through every sync path
/// — no path re-decides it. TWO independent causes select [`Additive`], and
/// this type deliberately does not record which: the degraded-emission
/// predicates in [`crate::feed::exec`] (this EMISSION is not trusted), and an
/// epic's `feed_append_only` flag (this EPIC never mirrors, because its source
/// emits events that are never retracted). See feeds.allium:
/// `DegradedNonEmptyEmission` and `AppendOnlyFeed`.
///
/// [`Additive`]: SyncMode::Additive
///
/// An enum rather than a `bool` because both readings of a bare flag
/// (`delete_absent` vs `additive`) are plausible at a call site, and picking the
/// wrong one here force-removes live agents' worktrees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyncMode {
    /// The emission is trusted in full: absent feed tasks are stale and are
    /// deleted (and then torn down). The ordinary mode.
    Reconcile,
    /// Act only on what the emission contains. Inserts, field refreshes and
    /// cross-role moves still happen; every removal is skipped, so
    /// [`FeedSyncOutcome::removed`] comes back empty. Reached either because
    /// the emission is untrusted or because the epic never mirrors — see the
    /// type's docs for why the difference is not recorded here.
    Additive,
}

impl SyncMode {
    /// Whether this mode may remove feed tasks absent from the emission.
    fn removes_absent(self) -> bool {
        matches!(self, SyncMode::Reconcile)
    }
}

/// What one sync pass did: which epics it wrote to, and which task rows it
/// removed that still owned on-disk or in-tmux state.
///
/// `affected_epics` drives one TUI notification per epic. `removed` is handed
/// to [`crate::feed::cleanup_removed_feed_tasks`] — a feed-driven removal owes
/// the same teardown as `ArchiveTask`/`DeleteTask`, and before this existed it
/// orphaned the worktree and tmux window on disk.
///
/// A task MOVED between role sub-epics is never in `removed`: the move lands
/// before every delete phase, and each delete filters on the task's current
/// `epic_id`. See `run_role_routed_feed_sync`'s phase-order note.
pub(crate) struct FeedSyncOutcome {
    pub(crate) affected_epics: Vec<EpicId>,
    pub(crate) removed: Vec<RemovedFeedTask>,
}

/// A feed item paired with its resolved repo path and base branch. Assembled
/// once at the `FeedCommandCompleted` boundary (see [`FeedItemWithTarget::zip`])
/// so the three values travel together as a unit through the rest of the feed
/// pipeline — there is no parallel-slice length invariant left to police.
pub(crate) struct FeedItemWithTarget {
    item: FeedItem,
    repo_path: String,
    base_branch: String,
}

impl FeedItemWithTarget {
    /// Zip co-indexed `items`/`repo_paths`/`base_branches` into paired entries.
    /// `repo_paths` and `base_branches` are derived one-per-item upstream
    /// (`resolve_feed_item_repo_paths` / `resolve_base_branches`), so the zip
    /// is lossless. Called once per emission, at each `FeedCommandCompleted`
    /// call site (`FeedRunner::tick`, `exec_trigger_epic_feed`) — the only
    /// place three parallel collections still exist, immediately before they
    /// collapse into these paired entries for the rest of the pipeline.
    pub(crate) fn zip(
        items: Vec<FeedItem>,
        repo_paths: Vec<String>,
        base_branches: Vec<String>,
    ) -> Vec<Self> {
        items
            .into_iter()
            .zip(repo_paths)
            .zip(base_branches)
            .map(|((item, repo_path), base_branch)| Self {
                item,
                repo_path,
                base_branch,
            })
            .collect()
    }

    /// Split paired entries back into the three slices `TaskStore::upsert_feed_tasks`
    /// still takes (a DB-layer concern, out of scope for this pipeline refactor).
    fn unzip(entries: Vec<Self>) -> (Vec<FeedItem>, Vec<String>, Vec<String>) {
        let mut items = Vec::with_capacity(entries.len());
        let mut repo_paths = Vec::with_capacity(entries.len());
        let mut base_branches = Vec::with_capacity(entries.len());
        for entry in entries {
            items.push(entry.item);
            repo_paths.push(entry.repo_path);
            base_branches.push(entry.base_branch);
        }
        (items, repo_paths, base_branches)
    }
}

/// Upsert feed items using the correct strategy for `epic.group_by_repo`.
///
/// - `group_by_repo = false`: FlatFeedReconcile (feeds.allium) — any active
///   RepoGroup sub-epic left over from a prior grouped state is flattened
///   back onto the parent first (re-homing its tasks, deleting it if it ends
///   up empty), then a flat upsert runs directly on the parent epic. This is
///   the symmetric OFF-side counterpart to the ON-side migration below: it
///   makes toggling group_by_repo off on a feed epic self-healing on the next
///   poll, rather than leaving tasks stranded in their old repo sub-epics.
/// - `group_by_repo = true`: group by repo name, upsert into per-repo sub-epics,
///   then clear flat tasks from the parent.
///
/// Returns `epic_id` plus any sub-epic IDs written to (grouped path only) as
/// `affected_epics` — callers use this list to send one TUI notification per
/// affected epic — and every removed row still owning a worktree or tmux window
/// as `removed`, for teardown.
pub(crate) async fn run_feed_sync(
    db: &dyn TaskStore,
    epic_id: EpicId,
    group_by_repo: bool,
    entries: Vec<FeedItemWithTarget>,
    mode: SyncMode,
) -> Result<FeedSyncOutcome> {
    if group_by_repo {
        Ok(grouped::sync_grouped_feed(db, epic_id, entries, mode).await)
    } else {
        // FlatFeedReconcile: reconcile any leftover RepoGroup sub-epics back
        // onto the parent before the flat upsert. Reuses flatten_epic (shared
        // with the manual FlattenEpic path) — idempotent no-op when no
        // RepoGroup sub-epics exist, so this is safe to run on every flat
        // sync, not only the first one after group_by_repo is toggled off.
        // Gated on an active RepoGroup sub-epic actually existing: flatten_epic
        // always ends with a recalculate_epic_status call, which would
        // otherwise duplicate the recalc callers already run right after
        // run_feed_sync returns (feed/mod.rs, runtime/epics.rs) on every poll,
        // not just the one cycle after a toggle.
        let has_repo_group_sub_epic = db.list_sub_epics(epic_id).await?.iter().any(|e| {
            e.origin == crate::models::EpicOrigin::RepoGroup
                && e.status != crate::models::TaskStatus::Archived
        });
        if has_repo_group_sub_epic {
            crate::service::flatten_epic(db, epic_id).await?;
        }
        let (items, repo_paths, base_branches) = FeedItemWithTarget::unzip(entries);
        // The flat path's stale delete lives inside upsert_feed_tasks, so the
        // mode is honoured by picking the variant rather than by skipping a
        // step. The re-home above still runs either way: it moves tasks and
        // deletes only sub-epics it has just emptied, so it destroys nothing
        // and never reads the emission.
        let removed = if mode.removes_absent() {
            db.upsert_feed_tasks(epic_id, &items, &repo_paths, &base_branches)
                .await?
        } else {
            db.upsert_feed_tasks_additive(epic_id, &items, &repo_paths, &base_branches)
                .await?
        };
        Ok(FeedSyncOutcome {
            affected_epics: vec![epic_id],
            removed,
        })
    }
}

/// Dispatch a feed emission to the correct sync strategy for `feed_role`. This
/// is the SINGLE authoritative role→sync-path mapping, shared by both the
/// auto-poll ([`crate::feed::FeedRunner`] tick) and the manual "r" refresh
/// (`exec_trigger_epic_feed`) so the two paths cannot drift — a `reviews_parent`
/// epic ALWAYS routes through [`run_role_routed_feed_sync`], never a flat upsert
/// onto the parent (feeds.allium: FeedSync dispatch).
///
/// The role→strategy mapping is TOTAL: the three role sub-epic roles are
/// rejected here rather than by callers. A My/Team/Bots epic carrying a
/// feed_command is a provisioning bug (they are reconciled only via their
/// reviews_parent), and the `_` arm below would flat-upsert into one. Guarding
/// in the dispatcher rather than in front of it means a future third caller
/// inherits the guard instead of having to remember it.
pub(crate) async fn run_feed_sync_by_role(
    db: &dyn TaskStore,
    epic_id: EpicId,
    feed_role: crate::models::FeedRole,
    group_by_repo: bool,
    entries: Vec<FeedItemWithTarget>,
    mode: SyncMode,
) -> Result<FeedSyncOutcome> {
    use crate::models::FeedRole;
    match feed_role {
        FeedRole::ReviewsParent => run_role_routed_feed_sync(db, epic_id, entries, mode).await,
        role @ (FeedRole::MyReviews | FeedRole::TeamReviews | FeedRole::Bots) => {
            debug_assert!(
                false,
                "role sub-epic {} (feed_role={role:?}) must not carry a feed_command",
                epic_id.0
            );
            anyhow::bail!(
                "role sub-epic carries a feed command; it is reconciled only via its reviews parent"
            )
        }
        _ => run_feed_sync(db, epic_id, group_by_repo, entries, mode).await,
    }
}
