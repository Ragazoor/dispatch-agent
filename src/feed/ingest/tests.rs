#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::sync::Arc;

use super::*;
use crate::db::{CreateTaskRequest, Database, EpicCrud, EpicPatch, EpicRead, TaskCrud, TaskPatch};
use crate::models::{test_tmux_window, FeedRole, Signal, TaskStatus, TaskTag};

fn make_item(external_id: &str, url: &str) -> FeedItem {
    FeedItem {
        external_id: external_id.to_string(),
        title: external_id.to_string(),
        description: String::new(),
        url: url.to_string(),
        url_type: None,
        status: crate::models::TaskStatus::Backlog,
        tag: TaskTag::PrReview,
        labels: vec![],
        sort_order: None,
        signals: vec![],
        wrap_up_mode: None,
    }
}

fn make_signal_item(external_id: &str, url: &str, signals: Vec<Signal>) -> FeedItem {
    FeedItem {
        signals,
        ..make_item(external_id, url)
    }
}

/// Zip three parallel test slices into paired [`FeedItemWithTarget`]
/// entries. Mirrors the assembly `FeedItemWithTarget::zip` performs at
/// the feed boundary.
fn entries(
    items: &[FeedItem],
    repo_paths: &[&str],
    base_branches: &[&str],
) -> Vec<FeedItemWithTarget> {
    items
        .iter()
        .zip(repo_paths.iter())
        .zip(base_branches.iter())
        .map(|((i, rp), bb)| FeedItemWithTarget {
            item: i.clone(),
            repo_path: rp.to_string(),
            base_branch: bb.to_string(),
        })
        .collect()
}

// Default-mode shims. Every test written before SyncMode existed means
// `SyncMode::Reconcile` — the ordinary trusted-emission sync — so these
// same-named wrappers supply it and shadow the glob-imported originals from
// `use super::*`. A test that exercises `SyncMode::Additive`
// (DegradedNonEmptyEmission) calls the real function by its `super::` path, so
// the mode under test is always visible at the call site.
async fn run_role_routed_feed_sync(
    db: &dyn crate::db::TaskStore,
    parent_id: EpicId,
    entries: Vec<FeedItemWithTarget>,
) -> anyhow::Result<FeedSyncOutcome> {
    super::role_routed::run_role_routed_feed_sync(db, parent_id, entries, SyncMode::Reconcile).await
}

async fn run_feed_sync(
    db: &dyn crate::db::TaskStore,
    epic_id: EpicId,
    group_by_repo: bool,
    entries: Vec<FeedItemWithTarget>,
) -> anyhow::Result<FeedSyncOutcome> {
    super::run_feed_sync(db, epic_id, group_by_repo, entries, SyncMode::Reconcile).await
}

async fn run_feed_sync_by_role(
    db: &dyn crate::db::TaskStore,
    epic_id: EpicId,
    feed_role: FeedRole,
    group_by_repo: bool,
    entries: Vec<FeedItemWithTarget>,
) -> anyhow::Result<FeedSyncOutcome> {
    super::run_feed_sync_by_role(
        db,
        epic_id,
        feed_role,
        group_by_repo,
        entries,
        SyncMode::Reconcile,
    )
    .await
}

async fn sync_grouped_feed(
    db: &dyn crate::db::TaskStore,
    parent_id: EpicId,
    entries: Vec<FeedItemWithTarget>,
) -> FeedSyncOutcome {
    super::grouped::sync_grouped_feed(db, parent_id, entries, SyncMode::Reconcile).await
}

/// Find the sub-epic of `parent` carrying `role`, asserting exactly one.
async fn role_sub_epic(db: &Database, parent: EpicId, role: FeedRole) -> EpicId {
    let subs = db.list_sub_epics(parent).await.unwrap();
    let matching: Vec<_> = subs.iter().filter(|e| e.feed_role == role).collect();
    assert_eq!(
        matching.len(),
        1,
        "expected exactly one {role:?} sub-epic, got {subs:?}"
    );
    matching[0].id
}

// --- run_role_routed_feed_sync (WP3) ---

/// Task 2 (B0): an emitted PR routes into the sub-epic for its role and the
/// other role sub-epics stay empty.
#[tokio::test]
async fn route_routed_inserts_into_role_sub_epic() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let parent = db.create_epic("Reviews", "", None).await.unwrap();
    db.patch_epic(
        parent.id,
        &EpicPatch::new().feed_role(FeedRole::ReviewsParent),
    )
    .await
    .unwrap();

    let items = vec![make_signal_item(
        "pr-1",
        "https://github.com/org/repo/pull/1",
        vec![Signal::DirectRequest],
    )];

    run_role_routed_feed_sync(&*db, parent.id, entries(&items, &[""], &["main"]))
        .await
        .unwrap();

    let my = role_sub_epic(&db, parent.id, FeedRole::MyReviews).await;
    let team = role_sub_epic(&db, parent.id, FeedRole::TeamReviews).await;
    let bots = role_sub_epic(&db, parent.id, FeedRole::Bots).await;

    let my_tasks = db.list_tasks_for_epic(my).await.unwrap();
    assert_eq!(my_tasks.len(), 1, "direct-request PR lands in My Reviews");
    assert_eq!(my_tasks[0].external_id.as_deref(), Some("pr-1"));
    assert!(db.list_tasks_for_epic(team).await.unwrap().is_empty());
    assert!(db.list_tasks_for_epic(bots).await.unwrap().is_empty());
}

/// Task 3 (B2): a PR whose role changes is MOVED, preserving its in-flight
/// status, sub_status, worktree, and tmux_window (agent session survives).
#[tokio::test]
async fn route_routed_moves_task_preserving_state() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let parent = db.create_epic("Reviews", "", None).await.unwrap();

    // Cycle 1: a team-requested PR lands in Team Reviews.
    let cycle1 = vec![make_signal_item(
        "pr-1",
        "https://github.com/org/repo/pull/1",
        vec![Signal::TeamRequest],
    )];
    run_role_routed_feed_sync(&*db, parent.id, entries(&cycle1, &[""], &["main"]))
        .await
        .unwrap();

    let team = role_sub_epic(&db, parent.id, FeedRole::TeamReviews).await;
    let task = db.list_tasks_for_epic(team).await.unwrap().remove(0);

    // Simulate in-flight dispatched work on the task.
    db.patch_task(
        task.id,
        &TaskPatch::new()
            .status(TaskStatus::Running)
            .sub_status(crate::models::SubStatus::Active)
            .worktree(Some("/tmp/wt-pr-1"))
            .tmux_window(Some(&test_tmux_window("dispatch:7"))),
    )
    .await
    .unwrap();

    // Cycle 2: the same PR is now also reviewed -> routes to My Reviews.
    let cycle2 = vec![make_signal_item(
        "pr-1",
        "https://github.com/org/repo/pull/1",
        vec![Signal::TeamRequest, Signal::Reviewed],
    )];
    run_role_routed_feed_sync(&*db, parent.id, entries(&cycle2, &[""], &["main"]))
        .await
        .unwrap();

    let my = role_sub_epic(&db, parent.id, FeedRole::MyReviews).await;
    let my_tasks = db.list_tasks_for_epic(my).await.unwrap();
    assert_eq!(my_tasks.len(), 1, "exactly one task, moved into My Reviews");
    let moved = &my_tasks[0];
    assert_eq!(moved.id, task.id, "same task row, not a recreate");
    assert_eq!(moved.external_id.as_deref(), Some("pr-1"));
    assert_eq!(moved.status, TaskStatus::Running, "status preserved");
    assert_eq!(
        moved.sub_status,
        crate::models::SubStatus::Active,
        "sub_status preserved"
    );
    assert_eq!(moved.worktree.as_deref(), Some("/tmp/wt-pr-1"));
    assert_eq!(
        moved.tmux_window.as_ref().map(|w| w.as_str()),
        Some("dispatch:7")
    );

    assert!(
        db.list_tasks_for_epic(team).await.unwrap().is_empty(),
        "old role sub-epic no longer holds the moved task"
    );
}

/// Task 4 (B1): the moved task is NOT deleted by the same cycle even though
/// it is absent from its losing role's group.
#[tokio::test]
async fn route_routed_move_not_deleted_same_cycle() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let parent = db.create_epic("Reviews", "", None).await.unwrap();

    let cycle1 = vec![make_signal_item(
        "pr-1",
        "https://github.com/org/repo/pull/1",
        vec![Signal::TeamRequest],
    )];
    run_role_routed_feed_sync(&*db, parent.id, entries(&cycle1, &[""], &["main"]))
        .await
        .unwrap();

    let cycle2 = vec![make_signal_item(
        "pr-1",
        "https://github.com/org/repo/pull/1",
        vec![Signal::Reviewed],
    )];
    run_role_routed_feed_sync(&*db, parent.id, entries(&cycle2, &[""], &["main"]))
        .await
        .unwrap();

    let my = role_sub_epic(&db, parent.id, FeedRole::MyReviews).await;
    let team = role_sub_epic(&db, parent.id, FeedRole::TeamReviews).await;
    assert_eq!(
        db.list_tasks_for_epic(my).await.unwrap().len(),
        1,
        "moved PR survives in its new role"
    );
    assert!(db.list_tasks_for_epic(team).await.unwrap().is_empty());
}

/// A task MOVED between role sub-epics during a sync must NEVER appear in the
/// removed set. The ordering that makes this true is not compiler-enforced:
/// `apply_move`'s `set_task_epic_id` lands before `upsert_role_groups`,
/// `delete_stale_subtree` and `clear_parent_stranded_tasks`, each of whose SQL
/// filters on the task's CURRENT `epic_id`. Before feed-task teardown existed a
/// mis-ordering merely deleted a row; now it would force-remove a live review
/// agent's worktree and kill its tmux window. Pin it.
///
/// `pr-2` stays in Team Reviews on purpose: it keeps a non-empty group on the
/// LOSING role sub-epic, so `upsert_role_groups`' per-epic stale-delete would
/// actually sweep `pr-1` out of Team if the move had not already landed. Drop
/// `pr-2` and the test stops detecting the mis-ordering.
#[tokio::test]
async fn moved_task_is_never_reported_as_removed() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let parent = db.create_epic("Reviews", "", None).await.unwrap();
    db.patch_epic(
        parent.id,
        &EpicPatch::new().feed_role(FeedRole::ReviewsParent),
    )
    .await
    .unwrap();

    // Cycle 1: both PRs are team-requested, so both land in Team Reviews.
    let cycle1 = vec![
        make_signal_item(
            "pr-1",
            "https://github.com/org/repo/pull/1",
            vec![Signal::TeamRequest],
        ),
        make_signal_item(
            "pr-2",
            "https://github.com/org/repo/pull/2",
            vec![Signal::TeamRequest],
        ),
    ];
    let outcome = run_feed_sync_by_role(
        &*db,
        parent.id,
        FeedRole::ReviewsParent,
        false,
        entries(&cycle1, &["", ""], &["main", "main"]),
    )
    .await
    .unwrap();
    assert!(
        outcome.removed.is_empty(),
        "nothing removed on first sight, got: {:?}",
        outcome.removed
    );

    // Give pr-1 an in-flight worktree and tmux window, as a dispatched review
    // agent would.
    let team = role_sub_epic(&db, parent.id, FeedRole::TeamReviews).await;
    let task = db
        .list_tasks_for_epic(team)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.external_id.as_deref() == Some("pr-1"))
        .expect("pr-1 landed in Team Reviews");
    db.patch_task(
        task.id,
        &TaskPatch::new()
            .worktree(Some("/repo/a/.worktrees/7-pr-1"))
            .tmux_window(Some(&test_tmux_window("dispatch:pr-1"))),
    )
    .await
    .unwrap();

    // Cycle 2: pr-1 is now also reviewed, so it routes to My Reviews — a MOVE
    // across role sub-epics, not a delete-and-reinsert. pr-2 stays in Team.
    let cycle2 = vec![
        make_signal_item(
            "pr-1",
            "https://github.com/org/repo/pull/1",
            vec![Signal::TeamRequest, Signal::Reviewed],
        ),
        make_signal_item(
            "pr-2",
            "https://github.com/org/repo/pull/2",
            vec![Signal::TeamRequest],
        ),
    ];
    let outcome = run_feed_sync_by_role(
        &*db,
        parent.id,
        FeedRole::ReviewsParent,
        false,
        entries(&cycle2, &["", ""], &["main", "main"]),
    )
    .await
    .unwrap();

    assert!(
        outcome.removed.is_empty(),
        "a moved task must not be reported for teardown, got: {:?}",
        outcome.removed
    );

    let my = role_sub_epic(&db, parent.id, FeedRole::MyReviews).await;
    let moved = db.list_tasks_for_epic(my).await.unwrap().remove(0);
    assert_eq!(moved.id, task.id, "same task row, not a recreate");
    assert_eq!(
        moved.worktree.as_deref(),
        Some("/repo/a/.worktrees/7-pr-1"),
        "the move must preserve the in-flight worktree"
    );
    assert_eq!(
        moved.tmux_window.as_ref().map(|w| w.as_str()),
        Some("dispatch:pr-1")
    );
}

/// A PR that genuinely leaves the emission (merged/closed) IS reported, so the
/// caller can tear its worktree down — the counterpart to the move case above.
#[tokio::test]
async fn absent_task_with_worktree_is_reported_as_removed() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let parent = db.create_epic("Reviews", "", None).await.unwrap();
    db.patch_epic(
        parent.id,
        &EpicPatch::new().feed_role(FeedRole::ReviewsParent),
    )
    .await
    .unwrap();

    let cycle1 = vec![make_signal_item(
        "pr-1",
        "https://github.com/org/repo/pull/1",
        vec![Signal::TeamRequest],
    )];
    run_feed_sync_by_role(
        &*db,
        parent.id,
        FeedRole::ReviewsParent,
        false,
        entries(&cycle1, &[""], &["main"]),
    )
    .await
    .unwrap();

    let team = role_sub_epic(&db, parent.id, FeedRole::TeamReviews).await;
    let task = db.list_tasks_for_epic(team).await.unwrap().remove(0);
    db.patch_task(
        task.id,
        &TaskPatch::new()
            .worktree(Some("/repo/a/.worktrees/7-pr-1"))
            .tmux_window(Some(&test_tmux_window("dispatch:pr-1"))),
    )
    .await
    .unwrap();

    // Cycle 2: the PR merged, so it is gone from the emission.
    let outcome = run_feed_sync_by_role(&*db, parent.id, FeedRole::ReviewsParent, false, vec![])
        .await
        .unwrap();

    assert_eq!(
        outcome.removed.len(),
        1,
        "the merged PR's worktree must be reported for teardown, got: {:?}",
        outcome.removed
    );
    assert_eq!(outcome.removed[0].id, task.id);
    assert_eq!(
        outcome.removed[0].worktree.as_deref(),
        Some("/repo/a/.worktrees/7-pr-1")
    );
    assert_eq!(
        outcome.removed[0].tmux_window.as_ref().map(|w| w.as_str()),
        Some("dispatch:pr-1")
    );
}

/// The flat (non-reviews_parent) path reports its removals too — the flat
/// branch of `run_feed_sync` is a `RemovedFeedTask` producer like the rest.
#[tokio::test]
async fn flat_sync_reports_removed_task_with_worktree() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let epic = db.create_epic("Flat Feed", "", None).await.unwrap();

    let items = vec![make_item("pr-1", "https://github.com/org/repo/pull/1")];
    run_feed_sync(&*db, epic.id, false, entries(&items, &[""], &["main"]))
        .await
        .unwrap();

    let task = db.list_tasks_for_epic(epic.id).await.unwrap().remove(0);
    db.patch_task(
        task.id,
        &TaskPatch::new()
            .worktree(Some("/repo/a/.worktrees/7-pr-1"))
            .tmux_window(Some(&test_tmux_window("dispatch:pr-1"))),
    )
    .await
    .unwrap();

    let outcome = run_feed_sync(&*db, epic.id, false, vec![]).await.unwrap();

    assert_eq!(
        outcome.removed.len(),
        1,
        "flat stale-delete must report its removal, got: {:?}",
        outcome.removed
    );
    assert_eq!(outcome.removed[0].id, task.id);
}

/// Count feed-managed tasks (external_id set) sitting DIRECTLY on an epic.
async fn flat_feed_task_count(db: &Database, epic: EpicId) -> usize {
    db.list_tasks_for_epic(epic)
        .await
        .unwrap()
        .into_iter()
        .filter(|t| t.external_id.is_some())
        .count()
}

/// Bug B (parent-stranded rescue): a feed task sitting flat on the
/// reviews_parent epic itself is MOVED down into its routed role sub-epic —
/// same row, in-flight state preserved — not left to deadlock the
/// subtree-uniqueness trigger. Enforces NoFlatFeedTasksOnReviewsParent.
#[tokio::test]
async fn route_routed_rescues_flat_task_stranded_on_parent() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let parent = db.create_epic("Reviews", "", None).await.unwrap();
    db.patch_epic(
        parent.id,
        &EpicPatch::new().feed_role(FeedRole::ReviewsParent),
    )
    .await
    .unwrap();

    // Strand a flat feed task directly on the parent — exactly what an
    // out-of-band flat upsert (the manual-trigger bug, or an older binary)
    // produces. Inserting into the reviews_parent epic does not fire the
    // subtree-uniqueness trigger, so this is a valid starting state.
    let item = make_signal_item(
        "pr-1",
        "https://github.com/org/repo/pull/1",
        vec![Signal::DirectRequest],
    );
    db.upsert_feed_tasks(
        parent.id,
        std::slice::from_ref(&item),
        &["".into()],
        &["main".into()],
    )
    .await
    .unwrap();
    let stranded = db.list_tasks_for_epic(parent.id).await.unwrap().remove(0);

    // Simulate in-flight dispatched work on the stranded task.
    db.patch_task(
        stranded.id,
        &TaskPatch::new()
            .status(TaskStatus::Running)
            .sub_status(crate::models::SubStatus::Active)
            .worktree(Some("/tmp/wt-pr-1"))
            .tmux_window(Some(&test_tmux_window("dispatch:7"))),
    )
    .await
    .unwrap();

    // Reconcile with the same PR present in the emission.
    run_role_routed_feed_sync(&*db, parent.id, entries(&[item], &[""], &["main"]))
        .await
        .unwrap();

    assert_eq!(
        flat_feed_task_count(&db, parent.id).await,
        0,
        "reviews_parent must hold no flat feed task after reconcile"
    );

    let my = role_sub_epic(&db, parent.id, FeedRole::MyReviews).await;
    let my_tasks = db.list_tasks_for_epic(my).await.unwrap();
    assert_eq!(my_tasks.len(), 1, "the rescued PR lands in My Reviews once");
    let moved = &my_tasks[0];
    assert_eq!(moved.id, stranded.id, "same task row, not delete+recreate");
    assert_eq!(moved.status, TaskStatus::Running, "status preserved");
    assert_eq!(
        moved.sub_status,
        crate::models::SubStatus::Active,
        "sub_status preserved"
    );
    assert_eq!(moved.worktree.as_deref(), Some("/tmp/wt-pr-1"));
    assert_eq!(
        moved.tmux_window.as_ref().map(|w| w.as_str()),
        Some("dispatch:7")
    );
}

/// Bug B (parent-stranded stale delete): a feed task stranded on the parent
/// that no current item names is removed as stale by the subtree delete,
/// whose scope must include the parent epic itself.
#[tokio::test]
async fn route_routed_deletes_stale_flat_task_on_parent() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let parent = db.create_epic("Reviews", "", None).await.unwrap();
    db.patch_epic(
        parent.id,
        &EpicPatch::new().feed_role(FeedRole::ReviewsParent),
    )
    .await
    .unwrap();

    let gone = make_item("pr-gone", "https://github.com/org/repo/pull/9");
    db.upsert_feed_tasks(parent.id, &[gone], &["".into()], &["main".into()])
        .await
        .unwrap();
    assert_eq!(flat_feed_task_count(&db, parent.id).await, 1);

    // Emission no longer contains pr-gone (merged/closed).
    run_role_routed_feed_sync(&*db, parent.id, entries(&[], &[], &[]))
        .await
        .unwrap();

    assert_eq!(
        flat_feed_task_count(&db, parent.id).await,
        0,
        "stale parent-stranded feed task must be deleted"
    );
}

/// Bug B guard: a MANUAL task (external_id = null) on the parent is never
/// touched by the parent-inclusive reconcile — only feed-managed tasks are.
#[tokio::test]
async fn route_routed_preserves_manual_task_on_parent() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let parent = db.create_epic("Reviews", "", None).await.unwrap();
    db.patch_epic(
        parent.id,
        &EpicPatch::new().feed_role(FeedRole::ReviewsParent),
    )
    .await
    .unwrap();

    let manual_id = db
        .create_task(CreateTaskRequest {
            title: "Manual note on parent",
            description: "",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: Some(parent.id),
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
            phoenix: false,
        })
        .await
        .unwrap();

    run_role_routed_feed_sync(&*db, parent.id, entries(&[], &[], &[]))
        .await
        .unwrap();

    let survivors = db.list_tasks_for_epic(parent.id).await.unwrap();
    assert_eq!(survivors.len(), 1, "manual task must survive");
    assert_eq!(survivors[0].id, manual_id);
    assert!(
        survivors[0].external_id.is_none(),
        "the survivor is the manual (non-feed) task"
    );
}

/// Bug B (legacy duplicate convergence): the corrupt state the old
/// flat-upsert bug produced — the SAME PR present BOTH flat on the parent
/// AND routed in a role sub-epic. The reconcile must converge to a single
/// copy in the sub-epic and clear the parent duplicate.
#[tokio::test]
async fn route_routed_clears_parent_duplicate_when_canonical_in_sub_epic() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let parent = db.create_epic("Reviews", "", None).await.unwrap();
    db.patch_epic(
        parent.id,
        &EpicPatch::new().feed_role(FeedRole::ReviewsParent),
    )
    .await
    .unwrap();

    let item = make_signal_item(
        "pr-1",
        "https://github.com/org/repo/pull/1",
        vec![Signal::DirectRequest],
    );

    // Cycle 1: route the PR into My Reviews (the canonical copy).
    run_role_routed_feed_sync(
        &*db,
        parent.id,
        entries(std::slice::from_ref(&item), &[""], &["main"]),
    )
    .await
    .unwrap();
    let my = role_sub_epic(&db, parent.id, FeedRole::MyReviews).await;
    assert_eq!(db.list_tasks_for_epic(my).await.unwrap().len(), 1);

    // Corrupt the state: plant a duplicate flat copy on the parent, as the
    // old manual-trigger flat upsert did (inserting onto a reviews_parent
    // epic does not fire the subtree-uniqueness trigger).
    db.upsert_feed_tasks(
        parent.id,
        std::slice::from_ref(&item),
        &["".into()],
        &["main".into()],
    )
    .await
    .unwrap();
    assert_eq!(
        flat_feed_task_count(&db, parent.id).await,
        1,
        "duplicate planted"
    );

    // Cycle 2: reconcile with the same PR present. Must converge.
    run_role_routed_feed_sync(
        &*db,
        parent.id,
        entries(std::slice::from_ref(&item), &[""], &["main"]),
    )
    .await
    .unwrap();

    assert_eq!(
        flat_feed_task_count(&db, parent.id).await,
        0,
        "parent duplicate cleared"
    );
    assert_eq!(
        db.list_tasks_for_epic(my).await.unwrap().len(),
        1,
        "exactly one canonical copy remains in My Reviews"
    );
}

/// Task 4: a PR present in cycle 1 but absent from cycle 2 (merged/closed)
/// is removed from the subtree; a manual task (external_id NULL) survives.
#[tokio::test]
async fn route_routed_removes_merged_pr_keeps_manual() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let parent = db.create_epic("Reviews", "", None).await.unwrap();

    let cycle1 = vec![
        make_signal_item(
            "pr-1",
            "https://github.com/org/repo/pull/1",
            vec![Signal::DirectRequest],
        ),
        make_signal_item(
            "pr-2",
            "https://github.com/org/repo/pull/2",
            vec![Signal::TeamRequest],
        ),
    ];
    run_role_routed_feed_sync(
        &*db,
        parent.id,
        entries(&cycle1, &["", ""], &["main", "main"]),
    )
    .await
    .unwrap();

    let my = role_sub_epic(&db, parent.id, FeedRole::MyReviews).await;
    // A manual task the user added under a role sub-epic.
    let manual_id = db
        .create_task(CreateTaskRequest {
            title: "Manual",
            description: "",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: Some(my),
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
            phoenix: false,
        })
        .await
        .unwrap();

    // Cycle 2: pr-2 merged/closed (absent). pr-1 still direct-requested.
    let cycle2 = vec![make_signal_item(
        "pr-1",
        "https://github.com/org/repo/pull/1",
        vec![Signal::DirectRequest],
    )];
    run_role_routed_feed_sync(&*db, parent.id, entries(&cycle2, &[""], &["main"]))
        .await
        .unwrap();

    let team = role_sub_epic(&db, parent.id, FeedRole::TeamReviews).await;
    assert!(
        db.list_tasks_for_epic(team).await.unwrap().is_empty(),
        "merged pr-2 removed from Team Reviews"
    );

    let my_tasks = db.list_tasks_for_epic(my).await.unwrap();
    assert!(
        my_tasks.iter().any(|t| t.id == manual_id),
        "manual task survives reconcile"
    );
    assert!(
        my_tasks
            .iter()
            .any(|t| t.external_id.as_deref() == Some("pr-1")),
        "still-open pr-1 retained"
    );
}

/// WP2 regression: each item must land with ITS OWN repo_path/base_branch,
/// never a neighbour's. Three items across three different roles (so they
/// land in three different sub-epics) each carry a distinct repo_path and
/// base_branch; a mis-paired zip (the footgun the old parallel-slice
/// length guard only detected after the fact) would surface here as a
/// task holding the wrong branch or repo_path.
#[tokio::test]
async fn route_routed_preserves_per_item_repo_path_and_base_branch() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let parent = db.create_epic("Reviews", "", None).await.unwrap();
    db.patch_epic(
        parent.id,
        &EpicPatch::new().feed_role(FeedRole::ReviewsParent),
    )
    .await
    .unwrap();

    let items = vec![
        make_signal_item(
            "pr-my",
            "https://github.com/org/repo-my/pull/1",
            vec![Signal::DirectRequest],
        ),
        make_signal_item(
            "pr-team",
            "https://github.com/org/repo-team/pull/2",
            vec![Signal::TeamRequest],
        ),
        make_signal_item(
            "pr-bots",
            "https://github.com/org/repo-bots/pull/3",
            vec![Signal::AuthorBot],
        ),
    ];
    let entries = entries(
        &items,
        &["/repo-my", "/repo-team", "/repo-bots"],
        &["my-branch", "team-branch", "bots-branch"],
    );
    run_role_routed_feed_sync(&*db, parent.id, entries)
        .await
        .unwrap();

    let my = role_sub_epic(&db, parent.id, FeedRole::MyReviews).await;
    let team = role_sub_epic(&db, parent.id, FeedRole::TeamReviews).await;
    let bots = role_sub_epic(&db, parent.id, FeedRole::Bots).await;

    let task_by_ext = |tasks: &[crate::models::Task], ext: &str| {
        tasks
            .iter()
            .find(|t| t.external_id.as_deref() == Some(ext))
            .unwrap()
            .clone()
    };

    let my_tasks = db.list_tasks_for_epic(my).await.unwrap();
    let my_task = task_by_ext(&my_tasks, "pr-my");
    assert_eq!(my_task.repo_path, "/repo-my");
    assert_eq!(my_task.base_branch, "my-branch");

    let team_tasks = db.list_tasks_for_epic(team).await.unwrap();
    let team_task = task_by_ext(&team_tasks, "pr-team");
    assert_eq!(team_task.repo_path, "/repo-team");
    assert_eq!(team_task.base_branch, "team-branch");

    let bots_tasks = db.list_tasks_for_epic(bots).await.unwrap();
    let bots_task = task_by_ext(&bots_tasks, "pr-bots");
    assert_eq!(bots_task.repo_path, "/repo-bots");
    assert_eq!(bots_task.base_branch, "bots-branch");
}

// --- group_by_repo on role sub-epics ---

/// When a role sub-epic has `group_by_repo = true`, feed items must be
/// routed into per-repo sub-epics rather than into the role sub-epic directly.
#[tokio::test]
async fn role_routed_group_by_repo_routes_into_repo_sub_epic() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let parent = db.create_epic("Reviews", "", None).await.unwrap();
    db.patch_epic(
        parent.id,
        &EpicPatch::new().feed_role(FeedRole::ReviewsParent),
    )
    .await
    .unwrap();

    // First cycle — creates role sub-epics.
    let items1 = vec![make_signal_item(
        "pr-1",
        "https://github.com/org/myrepo/pull/1",
        vec![Signal::TeamRequest],
    )];
    run_role_routed_feed_sync(&*db, parent.id, entries(&items1, &[""], &["main"]))
        .await
        .unwrap();

    // Enable group_by_repo on Team Reviews.
    let team = role_sub_epic(&db, parent.id, FeedRole::TeamReviews).await;
    db.patch_epic(team, &EpicPatch::new().group_by_repo(true))
        .await
        .unwrap();

    // Second cycle — same PR. Should now land in a repo-group sub-epic.
    run_role_routed_feed_sync(&*db, parent.id, entries(&items1, &[""], &["main"]))
        .await
        .unwrap();

    let team_direct = db.list_tasks_for_epic(team).await.unwrap();
    assert!(
        team_direct.is_empty(),
        "Team Reviews must have no direct tasks when group_by_repo is active"
    );

    let repo_subs = db.list_sub_epics(team).await.unwrap();
    assert_eq!(
        repo_subs.len(),
        1,
        "one repo-group sub-epic under Team Reviews"
    );
    assert_eq!(repo_subs[0].title, "myrepo");
    let repo_tasks = db.list_tasks_for_epic(repo_subs[0].id).await.unwrap();
    assert_eq!(repo_tasks.len(), 1, "PR landed in the repo-group sub-epic");
    assert_eq!(repo_tasks[0].external_id.as_deref(), Some("pr-1"));
}

/// Re-running the feed when group_by_repo is active must not create
/// duplicate tasks in the role sub-epic — the `existing` map must reach
/// into repo-group sub-epics so the PR is recognised as already present.
#[tokio::test]
async fn role_routed_group_by_repo_no_duplicate_on_resync() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let parent = db.create_epic("Reviews", "", None).await.unwrap();
    db.patch_epic(
        parent.id,
        &EpicPatch::new().feed_role(FeedRole::ReviewsParent),
    )
    .await
    .unwrap();

    let items = vec![make_signal_item(
        "pr-1",
        "https://github.com/org/myrepo/pull/1",
        vec![Signal::TeamRequest],
    )];

    // First cycle — creates role sub-epics.
    run_role_routed_feed_sync(&*db, parent.id, entries(&items, &[""], &["main"]))
        .await
        .unwrap();

    let team = role_sub_epic(&db, parent.id, FeedRole::TeamReviews).await;
    db.patch_epic(team, &EpicPatch::new().group_by_repo(true))
        .await
        .unwrap();

    // Second cycle — lands in repo-group sub-epic.
    run_role_routed_feed_sync(&*db, parent.id, entries(&items, &[""], &["main"]))
        .await
        .unwrap();

    // Third cycle — must not duplicate.
    run_role_routed_feed_sync(&*db, parent.id, entries(&items, &[""], &["main"]))
        .await
        .unwrap();

    let repo_subs = db.list_sub_epics(team).await.unwrap();
    assert_eq!(repo_subs.len(), 1);
    let tasks = db.list_tasks_for_epic(repo_subs[0].id).await.unwrap();
    assert_eq!(tasks.len(), 1, "exactly one task after three cycles");
    assert!(
        db.list_tasks_for_epic(team).await.unwrap().is_empty(),
        "no duplicate in role sub-epic"
    );
}

/// When a PR disappears from the feed and group_by_repo is active, the
/// stale deletion must reach into the repo-group sub-epic grandchildren
/// and remove the task.
#[tokio::test]
async fn role_routed_group_by_repo_stale_deletion_reaches_grandchildren() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let parent = db.create_epic("Reviews", "", None).await.unwrap();
    db.patch_epic(
        parent.id,
        &EpicPatch::new().feed_role(FeedRole::ReviewsParent),
    )
    .await
    .unwrap();

    let items = vec![make_signal_item(
        "pr-1",
        "https://github.com/org/myrepo/pull/1",
        vec![Signal::TeamRequest],
    )];

    // First cycle — creates role sub-epics.
    run_role_routed_feed_sync(&*db, parent.id, entries(&items, &[""], &["main"]))
        .await
        .unwrap();

    let team = role_sub_epic(&db, parent.id, FeedRole::TeamReviews).await;
    db.patch_epic(team, &EpicPatch::new().group_by_repo(true))
        .await
        .unwrap();

    // Second cycle — PR lands in repo-group sub-epic.
    run_role_routed_feed_sync(&*db, parent.id, entries(&items, &[""], &["main"]))
        .await
        .unwrap();

    let repo_subs = db.list_sub_epics(team).await.unwrap();
    assert_eq!(
        db.list_tasks_for_epic(repo_subs[0].id).await.unwrap().len(),
        1,
        "task present before stale deletion cycle"
    );

    // Third cycle — PR absent (merged/closed).
    run_role_routed_feed_sync(&*db, parent.id, entries(&[], &[], &[]))
        .await
        .unwrap();

    let tasks_after = db.list_tasks_for_epic(repo_subs[0].id).await.unwrap();
    assert!(
        tasks_after.is_empty(),
        "stale PR must be removed from repo-group sub-epic"
    );
}

/// Regression: archived sub-epics must not be reused when a new cycle runs.
///
/// The lookup must use `active_sub_epics` (status != Archived), not the full
/// list — otherwise an archived sub-epic with the same repo name is matched
/// and reused instead of creating a fresh active one.
#[tokio::test]
async fn archived_sub_epic_not_reused() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let parent = db.create_epic("Reviews", "", None).await.unwrap();

    // Create a sub-epic that is then archived.
    let archived_sub = db.create_epic("repo-a", "", Some(parent.id)).await.unwrap();
    db.patch_epic(
        archived_sub.id,
        &EpicPatch::new().status(TaskStatus::Archived),
    )
    .await
    .unwrap();

    let items = vec![make_item("pr-1", "https://github.com/org/repo-a/pull/1")];

    let outcome = sync_grouped_feed(&*db, parent.id, entries(&items, &[""], &["main"])).await;
    // affected_epics leads with the parent; this assertion is about sub-epics.
    let sub_ids: Vec<_> = outcome
        .affected_epics
        .iter()
        .copied()
        .filter(|id| *id != parent.id)
        .collect();

    assert_eq!(sub_ids.len(), 1, "should return exactly one sub-epic ID");
    let new_id = sub_ids[0];
    assert_ne!(
        new_id, archived_sub.id,
        "must create a new sub-epic, not reuse the archived one"
    );

    let all_subs = db.list_sub_epics(parent.id).await.unwrap();
    let active: Vec<_> = all_subs
        .iter()
        .filter(|e| e.status != TaskStatus::Archived)
        .collect();
    assert_eq!(active.len(), 1, "exactly one active sub-epic after sync");
    assert_eq!(active[0].title, "repo-a");
    assert_eq!(active[0].id, new_id);

    let tasks = db.list_tasks_for_epic(new_id).await.unwrap();
    assert_eq!(tasks.len(), 1, "new sub-epic must have the feed task");
    assert_eq!(tasks[0].external_id.as_deref(), Some("pr-1"));
}

#[tokio::test]
async fn items_grouped_by_repo_name() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let parent = db.create_epic("Reviews", "", None).await.unwrap();

    let items = vec![
        make_item("1", "https://github.com/org/repo-a/pull/1"),
        make_item("2", "https://github.com/org/repo-b/pull/1"),
    ];

    sync_grouped_feed(
        &*db,
        parent.id,
        entries(&items, &["", ""], &["main", "main"]),
    )
    .await;

    let subs = db.list_sub_epics(parent.id).await.unwrap();
    assert_eq!(subs.len(), 2);
    let names: Vec<&str> = subs.iter().map(|e| e.title.as_str()).collect();
    assert!(names.contains(&"repo-a"), "got {names:?}");
    assert!(names.contains(&"repo-b"), "got {names:?}");

    for sub in &subs {
        let tasks = db.list_tasks_for_epic(sub.id).await.unwrap();
        assert_eq!(tasks.len(), 1, "sub-epic {} should have 1 task", sub.title);
    }

    let parent_tasks = db.list_tasks_for_epic(parent.id).await.unwrap();
    assert_eq!(parent_tasks.len(), 0, "parent should have no direct tasks");
}

#[tokio::test]
async fn no_url_groups_as_other() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let parent = db.create_epic("Reviews", "", None).await.unwrap();

    let items = vec![FeedItem {
        external_id: "x".into(),
        title: "X".into(),
        description: String::new(),
        url: String::new(),
        url_type: None,
        status: TaskStatus::Backlog,
        tag: TaskTag::Bug,
        labels: vec![],
        sort_order: None,
        signals: vec![],
        wrap_up_mode: None,
    }];

    sync_grouped_feed(&*db, parent.id, entries(&items, &[""], &["main"])).await;

    let subs = db.list_sub_epics(parent.id).await.unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].title, "other");
}

#[tokio::test]
async fn existing_active_sub_epic_reused() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let parent = db.create_epic("Reviews", "", None).await.unwrap();

    // Pre-create the sub-epic as active.
    let pre_existing = db.create_epic("repo-a", "", Some(parent.id)).await.unwrap();

    let items = vec![make_item("1", "https://github.com/org/repo-a/pull/1")];

    let outcome = sync_grouped_feed(&*db, parent.id, entries(&items, &[""], &["main"])).await;
    // affected_epics leads with the parent; this assertion is about sub-epics.
    let sub_ids: Vec<_> = outcome
        .affected_epics
        .iter()
        .copied()
        .filter(|id| *id != parent.id)
        .collect();

    assert_eq!(
        sub_ids,
        vec![pre_existing.id],
        "should reuse existing active sub-epic"
    );
    let subs = db.list_sub_epics(parent.id).await.unwrap();
    assert_eq!(subs.len(), 1, "no duplicate sub-epic should be created");
}

#[tokio::test]
async fn run_feed_sync_flat_upserts_to_parent_epic() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let epic = db.create_epic("Feed", "", None).await.unwrap();
    let items = vec![crate::models::FeedItem {
        external_id: "1".into(),
        title: "T".into(),
        description: String::new(),
        url: String::new(),
        url_type: None,
        status: crate::models::TaskStatus::Backlog,
        tag: crate::models::TaskTag::Bug,
        labels: vec![],
        sort_order: None,
        signals: vec![],
        wrap_up_mode: None,
    }];

    let outcome = run_feed_sync(&*db, epic.id, false, entries(&items, &[""], &["main"]))
        .await
        .unwrap();

    assert_eq!(outcome.affected_epics, vec![epic.id]);
    let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].external_id.as_deref(), Some("1"));
}

#[tokio::test]
async fn run_feed_sync_grouped_puts_tasks_in_sub_epics() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let epic = db.create_epic("Reviews", "", None).await.unwrap();
    let items = vec![crate::models::FeedItem {
        external_id: "pr-1".into(),
        title: "PR 1".into(),
        description: String::new(),
        url: "https://github.com/org/repo-a/pull/1".into(),
        url_type: None,
        status: crate::models::TaskStatus::Backlog,
        tag: crate::models::TaskTag::PrReview,
        labels: vec![],
        sort_order: None,
        signals: vec![],
        wrap_up_mode: None,
    }];

    let outcome = run_feed_sync(&*db, epic.id, true, entries(&items, &[""], &["main"]))
        .await
        .unwrap();

    assert!(outcome.affected_epics.contains(&epic.id));
    assert_eq!(outcome.affected_epics.len(), 2, "parent id + 1 sub-epic id");

    let parent_tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    assert_eq!(parent_tasks.len(), 0, "parent should have no direct tasks");

    let sub_epics = db.list_sub_epics(epic.id).await.unwrap();
    assert_eq!(sub_epics.len(), 1);
    assert_eq!(sub_epics[0].title, "repo-a");
    let sub_tasks = db.list_tasks_for_epic(sub_epics[0].id).await.unwrap();
    assert_eq!(sub_tasks.len(), 1);
}

/// An empty emission must clear feed tasks from EVERY active sub-epic —
/// the feed is the source of truth for the whole grouped subtree, not just
/// the repos present in the current batch. The sub-epic rows themselves
/// remain (not auto-deleted).
#[tokio::test]
async fn sync_grouped_feed_empty_emission_clears_all_sub_epics() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let parent = db.create_epic("Reviews", "", None).await.unwrap();

    let items = vec![
        make_item("1", "https://github.com/org/repo-a/pull/1"),
        make_item("2", "https://github.com/org/repo-b/pull/1"),
    ];
    sync_grouped_feed(
        &*db,
        parent.id,
        entries(&items, &["", ""], &["main", "main"]),
    )
    .await;

    assert_eq!(db.list_sub_epics(parent.id).await.unwrap().len(), 2);

    // Second cycle: the feed now returns nothing.
    sync_grouped_feed(&*db, parent.id, vec![]).await;

    let subs = db.list_sub_epics(parent.id).await.unwrap();
    assert_eq!(
        subs.len(),
        2,
        "sub-epic rows remain, only their tasks clear"
    );
    for sub in &subs {
        let tasks = db.list_tasks_for_epic(sub.id).await.unwrap();
        assert_eq!(
            tasks.len(),
            0,
            "sub-epic {} should have no feed tasks after empty emission",
            sub.title
        );
    }
}

/// A partial emission clears only the sub-epics whose repo dropped out;
/// repos still present keep their tasks.
#[tokio::test]
async fn sync_grouped_feed_partial_emission_clears_dropped_repo() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let parent = db.create_epic("Reviews", "", None).await.unwrap();

    let items = vec![
        make_item("1", "https://github.com/org/repo-a/pull/1"),
        make_item("2", "https://github.com/org/repo-b/pull/1"),
    ];
    sync_grouped_feed(
        &*db,
        parent.id,
        entries(&items, &["", ""], &["main", "main"]),
    )
    .await;

    // Second cycle: only repo-a still has an open item.
    let items2 = vec![make_item("1", "https://github.com/org/repo-a/pull/1")];
    sync_grouped_feed(&*db, parent.id, entries(&items2, &[""], &["main"])).await;

    let subs = db.list_sub_epics(parent.id).await.unwrap();
    let repo_a = subs.iter().find(|e| e.title == "repo-a").unwrap();
    let repo_b = subs.iter().find(|e| e.title == "repo-b").unwrap();
    assert_eq!(
        db.list_tasks_for_epic(repo_a.id).await.unwrap().len(),
        1,
        "repo-a still in feed, task kept"
    );
    assert_eq!(
        db.list_tasks_for_epic(repo_b.id).await.unwrap().len(),
        0,
        "repo-b dropped out, task cleared"
    );
}

/// When group_by_repo is toggled OFF, the next feed cycle must move tasks
/// from the orphaned repo-group sub-epic onto the role sub-epic — no duplicate.
#[tokio::test]
async fn role_routed_group_by_repo_off_rehomes_repo_tasks_no_duplicate() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let parent = db.create_epic("Reviews", "", None).await.unwrap();
    db.patch_epic(
        parent.id,
        &EpicPatch::new().feed_role(FeedRole::ReviewsParent),
    )
    .await
    .unwrap();

    let items = vec![make_signal_item(
        "pr-1",
        "https://github.com/org/myrepo/pull/1",
        vec![Signal::TeamRequest],
    )];

    // Cycle 1 — team_reviews has group_by_repo OFF, task lands flat.
    run_role_routed_feed_sync(&*db, parent.id, entries(&items, &[""], &["main"]))
        .await
        .unwrap();

    let team = role_sub_epic(&db, parent.id, FeedRole::TeamReviews).await;

    // Enable group_by_repo → cycle 2 moves task into repo sub-epic.
    db.patch_epic(team, &EpicPatch::new().group_by_repo(true))
        .await
        .unwrap();
    run_role_routed_feed_sync(&*db, parent.id, entries(&items, &[""], &["main"]))
        .await
        .unwrap();

    let repo_subs = db.list_sub_epics(team).await.unwrap();
    assert_eq!(repo_subs.len(), 1);
    assert_eq!(
        db.list_tasks_for_epic(repo_subs[0].id).await.unwrap().len(),
        1
    );
    assert!(db.list_tasks_for_epic(team).await.unwrap().is_empty());

    // Disable group_by_repo → cycle 3 must re-home task to role sub-epic, no duplicate.
    db.patch_epic(team, &EpicPatch::new().group_by_repo(false))
        .await
        .unwrap();
    run_role_routed_feed_sync(&*db, parent.id, entries(&items, &[""], &["main"]))
        .await
        .unwrap();

    let team_tasks = db.list_tasks_for_epic(team).await.unwrap();
    assert_eq!(
        team_tasks.len(),
        1,
        "exactly one task on role sub-epic after toggle-off cycle"
    );
    assert_eq!(team_tasks[0].external_id.as_deref(), Some("pr-1"));

    // No tasks remain in any repo-group sub-epic.
    for sub in db.list_sub_epics(team).await.unwrap() {
        assert!(
            db.list_tasks_for_epic(sub.id).await.unwrap().is_empty(),
            "repo-group sub-epic {} must be empty after group_by_repo turned off",
            sub.title
        );
    }
}

/// When orphaned repo-group sub-epic tasks pre-exist (simulating a state from
/// before the fix), the next feed cycle must re-home them without duplicating.
#[tokio::test]
async fn role_routed_orphaned_repo_tasks_rehosted_on_next_sync() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let parent = db.create_epic("Reviews", "", None).await.unwrap();
    db.patch_epic(
        parent.id,
        &EpicPatch::new().feed_role(FeedRole::ReviewsParent),
    )
    .await
    .unwrap();

    // Manually create the role sub-epic and an orphaned repo-group sub-epic
    // with a task, simulating the pre-fix state.
    //
    // We use raw SQL because:
    //   - create_epic sets origin='manual', not 'repo-group'; the feed code
    //     identifies repo sub-epics by origin='repo-group'.
    //   - CreateTaskRequest has no external_id field (always NULL); tasks must
    //     have a non-NULL external_id to be visible to the existing-task index.
    //   - Insert the task with external_id=NULL then UPDATE to avoid the v72
    //     BEFORE INSERT trigger (same pattern as the v71 test).
    let (team_id, repo_sub_id) = db.db_call(|conn| {
        conn.execute_batch(
            "INSERT INTO epics (title, description, status, feed_role, origin, parent_epic_id)
             VALUES ('Team Reviews', '', 'backlog', 'team-reviews', 'manual', 1);
             INSERT INTO epics (title, description, status, feed_role, origin, parent_epic_id, group_by_repo)
             VALUES ('myrepo', '', 'backlog', 'none', 'repo-group',
                     (SELECT id FROM epics WHERE feed_role = 'team-reviews'), 0);",
        )
        .map_err(anyhow::Error::from)?;
        let team_id: i64 = conn.query_row(
            "SELECT id FROM epics WHERE feed_role = 'team-reviews'",
            [],
            |r| r.get(0),
        )?;
        let repo_sub_id: i64 = conn.query_row(
            "SELECT id FROM epics WHERE origin = 'repo-group'",
            [],
            |r| r.get(0),
        )?;
        Ok::<_, anyhow::Error>((team_id, repo_sub_id))
    })
    .await
    .unwrap();
    let team = EpicId(team_id);
    let repo_sub_id = EpicId(repo_sub_id);

    db.db_call(move |conn| {
        conn.execute_batch(&format!(
            "INSERT INTO tasks (title, description, repo_path, status, base_branch, epic_id)
             VALUES ('PR #1', '', '/r', 'backlog', 'main', {repo});
             UPDATE tasks SET external_id = 'pr-1' WHERE epic_id = {repo};",
            repo = repo_sub_id.0
        ))
        .map_err(anyhow::Error::from)
    })
    .await
    .unwrap();

    // Feed cycle with group_by_repo=false — should find the orphaned task
    // and re-home it, producing exactly one task on the role sub-epic.
    let items = vec![make_signal_item(
        "pr-1",
        "https://github.com/org/myrepo/pull/1",
        vec![Signal::TeamRequest],
    )];
    run_role_routed_feed_sync(&*db, parent.id, entries(&items, &[""], &["main"]))
        .await
        .unwrap();

    let team_tasks = db.list_tasks_for_epic(team).await.unwrap();
    assert_eq!(team_tasks.len(), 1, "task re-homed to role sub-epic");
    assert_eq!(team_tasks[0].external_id.as_deref(), Some("pr-1"));
    assert!(
        db.list_tasks_for_epic(repo_sub_id)
            .await
            .unwrap()
            .is_empty(),
        "orphaned repo-group sub-epic now empty"
    );
}

/// Clearing a dropped sub-epic removes only feed tasks (external_id set);
/// a manually-added task (external_id = null) in that sub-epic survives.
#[tokio::test]
async fn sync_grouped_feed_preserves_manual_task_in_dropped_sub_epic() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let parent = db.create_epic("Reviews", "", None).await.unwrap();

    let items = vec![make_item("1", "https://github.com/org/repo-a/pull/1")];
    sync_grouped_feed(&*db, parent.id, entries(&items, &[""], &["main"])).await;

    let subs = db.list_sub_epics(parent.id).await.unwrap();
    let repo_a = subs.iter().find(|e| e.title == "repo-a").unwrap();

    // A manual task the user added under the repo sub-epic (no external_id).
    let manual_id = db
        .create_task(CreateTaskRequest {
            title: "Manual",
            description: "",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: Some(repo_a.id),
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
            phoenix: false,
        })
        .await
        .unwrap();

    // Empty emission clears the feed task but must spare the manual one.
    sync_grouped_feed(&*db, parent.id, vec![]).await;

    let tasks = db.list_tasks_for_epic(repo_a.id).await.unwrap();
    assert_eq!(tasks.len(), 1, "only the manual task survives");
    assert_eq!(tasks[0].id, manual_id);
}

// --- FlatFeedReconcile (feeds.allium) ---
//
// Toggling group_by_repo OFF on a feed epic only flips the flag; these
// tests cover the flat sync path's reconciliation of pre-existing
// RepoGroup sub-epics (docs/specs/feeds.allium: FlatFeedReconcile).

/// A feed epic with group_by_repo=false and an existing active RepoGroup
/// sub-epic: the flat sync path re-homes the sub-epic's task back to the
/// parent (as the SAME row, not a delete+recreate) and deletes the
/// now-empty sub-epic, then upserts the current emission onto the parent.
#[tokio::test]
async fn flat_sync_rehomes_tasks_from_existing_repo_group_subepic_and_deletes_it() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let parent = db.create_epic("CVE", "", None).await.unwrap();

    // Simulate a pre-existing grouped state: a RepoGroup sub-epic holding
    // a feed task, as if group_by_repo had been on for a prior poll.
    let sub = db
        .create_repo_group_sub_epic(parent.id, "myrepo")
        .await
        .unwrap();
    let seed = vec![make_item("cve-1", "https://github.com/org/myrepo/pull/1")];
    db.upsert_feed_tasks(sub, &seed, &["".into()], &["main".into()])
        .await
        .unwrap();
    let pre_existing = db.list_tasks_for_epic(sub).await.unwrap().remove(0);

    // Flat sync (group_by_repo=false) with the same item still emitted.
    let items = vec![make_item("cve-1", "https://github.com/org/myrepo/pull/1")];
    run_feed_sync(&*db, parent.id, false, entries(&items, &[""], &["main"]))
        .await
        .unwrap();

    let parent_tasks = db.list_tasks_for_epic(parent.id).await.unwrap();
    assert_eq!(parent_tasks.len(), 1, "task re-homed onto the parent");
    assert_eq!(
        parent_tasks[0].id, pre_existing.id,
        "re-home is a move (same task row), not a delete+recreate"
    );
    assert_eq!(parent_tasks[0].external_id.as_deref(), Some("cve-1"));

    assert!(
        db.get_epic(sub).await.unwrap().is_none(),
        "emptied RepoGroup sub-epic is deleted"
    );
}

/// Regression: a feed epic with group_by_repo=false and NO existing
/// RepoGroup sub-epics behaves exactly as a plain flat upsert (no-op
/// reconciliation, not vacuous — the emission still lands on the parent).
#[tokio::test]
async fn flat_sync_with_no_repo_group_subepics_is_unaffected() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let parent = db.create_epic("CVE", "", None).await.unwrap();

    let items = vec![make_item("cve-2", "https://github.com/org/other/pull/2")];
    run_feed_sync(&*db, parent.id, false, entries(&items, &[""], &["main"]))
        .await
        .unwrap();

    let parent_tasks = db.list_tasks_for_epic(parent.id).await.unwrap();
    assert_eq!(parent_tasks.len(), 1, "flat upsert still lands on parent");
    assert_eq!(parent_tasks[0].external_id.as_deref(), Some("cve-2"));
    assert!(
        db.list_sub_epics(parent.id).await.unwrap().is_empty(),
        "no sub-epics created or left behind"
    );
}

/// A manually-created (non-RepoGroup) sub-epic under a feed epic is never
/// touched by flat-path reconciliation, mirroring
/// `flatten_preserves_manual_sub_epics` in src/service/grouping.rs.
#[tokio::test]
async fn flat_sync_preserves_manual_sub_epic() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let parent = db.create_epic("CVE", "", None).await.unwrap();
    let manual = db.create_epic("notes", "", Some(parent.id)).await.unwrap();
    let manual_task = db
        .create_task(CreateTaskRequest {
            title: "Manual note",
            description: "",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: Some(manual.id),
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
            phoenix: false,
        })
        .await
        .unwrap();

    let items = vec![make_item("cve-3", "https://github.com/org/other/pull/3")];
    run_feed_sync(&*db, parent.id, false, entries(&items, &[""], &["main"]))
        .await
        .unwrap();

    assert!(
        db.get_epic(manual.id).await.unwrap().is_some(),
        "manual sub-epic survives flat-path reconciliation"
    );
    let manual_tasks = db.list_tasks_for_epic(manual.id).await.unwrap();
    assert_eq!(manual_tasks.len(), 1, "manual task stays put");
    assert_eq!(manual_tasks[0].id, manual_task);
}

// --- SyncMode::Additive (feeds.allium: DegradedNonEmptyEmission) ---
//
// One test per removal mechanism, because they are three separate code paths
// that happen to share an outcome: the role-routed subtree delete, the parent
// sweep, the grouped absent-sub-epic clear, and the flat/per-epic stale delete
// inside upsert_feed_tasks. A single end-to-end test would leave three of them
// free to regress independently.

/// The role-routed subtree delete is skipped: a PR omitted by a partially
/// degraded emission keeps its row AND its agent state, and is not reported for
/// teardown. This is the exact scenario #4095 exists for — `pr-1` is a live
/// review agent whose PR one soft-failed sub-query dropped.
#[tokio::test]
async fn additive_role_routed_sync_keeps_a_task_absent_from_the_emission() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let parent = db.create_epic("Reviews", "", None).await.unwrap();
    db.patch_epic(
        parent.id,
        &EpicPatch::new().feed_role(FeedRole::ReviewsParent),
    )
    .await
    .unwrap();

    let cycle1 = vec![
        make_signal_item(
            "pr-1",
            "https://github.com/org/repo/pull/1",
            vec![Signal::DirectRequest],
        ),
        make_signal_item(
            "pr-2",
            "https://github.com/org/repo/pull/2",
            vec![Signal::DirectRequest],
        ),
    ];
    run_role_routed_feed_sync(
        &*db,
        parent.id,
        entries(&cycle1, &["", ""], &["main", "main"]),
    )
    .await
    .unwrap();

    let my = role_sub_epic(&db, parent.id, FeedRole::MyReviews).await;
    let live = db
        .list_tasks_for_epic(my)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.external_id.as_deref() == Some("pr-1"))
        .unwrap();
    db.patch_task(
        live.id,
        &TaskPatch::new()
            .status(TaskStatus::Running)
            .sub_status(crate::models::SubStatus::Active)
            .worktree(Some("/tmp/wt-pr-1"))
            .tmux_window(Some(&test_tmux_window("dispatch:7"))),
    )
    .await
    .unwrap();

    // Cycle 2: a degraded emission that lost pr-1.
    let cycle2 = vec![make_signal_item(
        "pr-2",
        "https://github.com/org/repo/pull/2",
        vec![Signal::DirectRequest],
    )];
    let outcome = super::role_routed::run_role_routed_feed_sync(
        &*db,
        parent.id,
        entries(&cycle2, &[""], &["main"]),
        SyncMode::Additive,
    )
    .await
    .unwrap();

    let tasks = db.list_tasks_for_epic(my).await.unwrap();
    let survivor = tasks
        .iter()
        .find(|t| t.external_id.as_deref() == Some("pr-1"))
        .expect("a task omitted by a degraded emission must not be deleted");
    assert_eq!(survivor.worktree.as_deref(), Some("/tmp/wt-pr-1"));
    assert_eq!(survivor.status, TaskStatus::Running);
    assert!(
        outcome.removed.is_empty(),
        "an additive sync reports nothing for teardown, got {:?}",
        outcome.removed
    );
}

/// The parent sweep is skipped too. It deletes on position alone — every feed
/// task sitting directly on the reviews_parent — so it would wipe a stranded
/// task on a degraded cycle without ever consulting the emission.
#[tokio::test]
async fn additive_role_routed_sync_keeps_parent_stranded_tasks() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let parent = db.create_epic("Reviews", "", None).await.unwrap();
    db.patch_epic(
        parent.id,
        &EpicPatch::new().feed_role(FeedRole::ReviewsParent),
    )
    .await
    .unwrap();

    // A feed task stranded flat on the parent, absent from the next emission.
    db.upsert_feed_tasks(
        parent.id,
        &[make_item("stranded", "https://github.com/org/repo/pull/9")],
        &["".to_string()],
        &["main".to_string()],
    )
    .await
    .unwrap();

    let items = vec![make_signal_item(
        "pr-2",
        "https://github.com/org/repo/pull/2",
        vec![Signal::DirectRequest],
    )];
    super::role_routed::run_role_routed_feed_sync(
        &*db,
        parent.id,
        entries(&items, &[""], &["main"]),
        SyncMode::Additive,
    )
    .await
    .unwrap();

    let on_parent = db.list_tasks_for_epic(parent.id).await.unwrap();
    assert_eq!(
        on_parent.len(),
        1,
        "the parent sweep must not run on a degraded cycle, got {on_parent:?}"
    );
    assert_eq!(on_parent[0].external_id.as_deref(), Some("stranded"));
}

/// Additive is "no removals", not "no writes": a cross-role MOVE still happens,
/// because it follows from what the emission CONTAINS. Withholding moves would
/// make a degraded cycle actively wrong rather than merely conservative.
#[tokio::test]
async fn additive_role_routed_sync_still_moves_and_inserts() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let parent = db.create_epic("Reviews", "", None).await.unwrap();

    let cycle1 = vec![make_signal_item(
        "pr-1",
        "https://github.com/org/repo/pull/1",
        vec![Signal::TeamRequest],
    )];
    run_role_routed_feed_sync(&*db, parent.id, entries(&cycle1, &[""], &["main"]))
        .await
        .unwrap();

    let cycle2 = vec![
        make_signal_item(
            "pr-1",
            "https://github.com/org/repo/pull/1",
            vec![Signal::Reviewed],
        ),
        make_signal_item(
            "pr-new",
            "https://github.com/org/repo/pull/5",
            vec![Signal::DirectRequest],
        ),
    ];
    super::role_routed::run_role_routed_feed_sync(
        &*db,
        parent.id,
        entries(&cycle2, &["", ""], &["main", "main"]),
        SyncMode::Additive,
    )
    .await
    .unwrap();

    let my = role_sub_epic(&db, parent.id, FeedRole::MyReviews).await;
    let team = role_sub_epic(&db, parent.id, FeedRole::TeamReviews).await;
    let my_ids: Vec<String> = db
        .list_tasks_for_epic(my)
        .await
        .unwrap()
        .into_iter()
        .filter_map(|t| t.external_id)
        .collect();
    assert!(my_ids.contains(&"pr-1".to_string()), "move still applied");
    assert!(
        my_ids.contains(&"pr-new".to_string()),
        "first-sight insert still applied"
    );
    assert!(
        db.list_tasks_for_epic(team).await.unwrap().is_empty(),
        "the moved task left its old role sub-epic, and nothing was deleted to \
         achieve that"
    );
}

/// The grouped path's two clearing passes — absent sub-epics and the parent's
/// flat tasks — are both skipped.
#[tokio::test]
async fn additive_grouped_sync_keeps_tasks_in_a_dropped_repo_sub_epic() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let parent = db.create_epic("Reviews", "", None).await.unwrap();

    let items = vec![
        make_item("1", "https://github.com/org/repo-a/pull/1"),
        make_item("2", "https://github.com/org/repo-b/pull/1"),
    ];
    sync_grouped_feed(
        &*db,
        parent.id,
        entries(&items, &["", ""], &["main", "main"]),
    )
    .await;

    // Degraded cycle: repo-b's query soft-failed, so only repo-a is emitted.
    let degraded = vec![make_item("1", "https://github.com/org/repo-a/pull/1")];
    let outcome = super::grouped::sync_grouped_feed(
        &*db,
        parent.id,
        entries(&degraded, &[""], &["main"]),
        SyncMode::Additive,
    )
    .await;

    let subs = db.list_sub_epics(parent.id).await.unwrap();
    let repo_b = subs.iter().find(|e| e.title == "repo-b").unwrap();
    assert_eq!(
        db.list_tasks_for_epic(repo_b.id).await.unwrap().len(),
        1,
        "a repo absent from a degraded emission keeps its tasks"
    );
    assert!(outcome.removed.is_empty());
}

/// The flat path's stale delete lives inside `upsert_feed_tasks`, so it is
/// gated by the additive variant of that call rather than by a skipped step.
#[tokio::test]
async fn additive_flat_sync_keeps_a_task_absent_from_the_emission() {
    let db = Arc::new(Database::open_in_memory().await.unwrap());
    let epic = db.create_epic("CVE", "", None).await.unwrap();

    let items = vec![make_item("cve-1", ""), make_item("cve-2", "")];
    run_feed_sync(
        &*db,
        epic.id,
        false,
        entries(&items, &["", ""], &["main", "main"]),
    )
    .await
    .unwrap();

    let degraded = vec![make_item("cve-2", "")];
    let outcome = super::run_feed_sync(
        &*db,
        epic.id,
        false,
        entries(&degraded, &[""], &["main"]),
        SyncMode::Additive,
    )
    .await
    .unwrap();

    let ids: Vec<String> = db
        .list_tasks_for_epic(epic.id)
        .await
        .unwrap()
        .into_iter()
        .filter_map(|t| t.external_id)
        .collect();
    assert_eq!(ids.len(), 2, "nothing removed on a degraded flat sync");
    assert!(ids.contains(&"cve-1".to_string()));
    assert!(outcome.removed.is_empty());
}

// A `reviews_parent` epic's exclusion from flat-path reconciliation
// (docs/specs/feeds.allium: FlatFeedReconcile requires feed_role !=
// reviews_parent) is structural, not a runtime branch inside
// FlatFeedReconcile itself: `run_feed_sync_by_role`'s match arm (above)
// routes `FeedRole::ReviewsParent` to `run_role_routed_feed_sync`
// exclusively, so `run_feed_sync`'s flat branch is never reached for a
// reviews_parent epic. That dispatch is already covered by
// `route_routed_inserts_into_role_sub_epic` and the other
// `route_routed_*` / `role_routed_*` tests above, which exercise
// `run_role_routed_feed_sync` directly — no separate test needed here.
