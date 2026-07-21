//! Tests for the layout cache on App.
//!
//! Two caches live and die together (all cleared by `invalidate_layout_cache`):
//!   - `epic_stats_cache`    — O(1) Arc clone instead of full HashMap copy
//!   - `column_anchor_cache` — sorted selectable items per status (O(1) nav)
//!
//! Core invariants:
//!   1. Both caches are empty after invalidation.
//!   2. `cached_epic_stats()` populates both caches atomically.
//!   3. Navigation never invalidates a populated cache.
//!   4. Board mutations always invalidate and repopulate the caches.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use super::*;
use crate::models::{EpicId, TaskId, TaskStatus};
use crate::tui::messages::{EpicMessage, TaskMessage};
use crate::tui::types::{ColumnAnchor, Message};

// ---------------------------------------------------------------------------
// Startup state
// ---------------------------------------------------------------------------

#[test]
fn epic_stats_cache_is_populated_after_new() {
    // App::new() calls cached_epic_stats() to seed the anchor, so the cache
    // is warm (not None) from the very first render onward.
    let app = make_app();
    assert!(app.layout.epic_stats_cache.is_some());
}

// ---------------------------------------------------------------------------
// cached_epic_stats
// ---------------------------------------------------------------------------

#[test]
fn cached_epic_stats_populates_cache_after_invalidation() {
    let mut app = make_app();
    app.invalidate_layout_cache();
    assert!(app.layout.epic_stats_cache.is_none());
    let _ = app.cached_epic_stats();
    assert!(app.layout.epic_stats_cache.is_some());
}

#[test]
fn cached_epic_stats_returns_consistent_value_on_repeated_calls() {
    let mut app = make_app();
    app.board.epics = vec![make_epic(10)];
    let first = app.cached_epic_stats();
    let second = app.cached_epic_stats();
    assert_eq!(first.len(), second.len());
    assert_eq!(
        first.contains_key(&EpicId(10)),
        second.contains_key(&EpicId(10))
    );
}

// ---------------------------------------------------------------------------
// invalidate_layout_cache
// ---------------------------------------------------------------------------

#[test]
fn invalidate_layout_cache_clears_populated_cache() {
    let mut app = make_app();
    app.board.epics = vec![make_epic(10)];
    let _ = app.cached_epic_stats();
    assert!(app.layout.epic_stats_cache.is_some());

    app.invalidate_layout_cache();
    assert!(app.layout.epic_stats_cache.is_none());
}

#[test]
fn invalidate_layout_cache_is_idempotent() {
    let mut app = make_app();
    // First invalidate on a warm cache.
    app.invalidate_layout_cache();
    assert!(app.layout.epic_stats_cache.is_none());
    // Second invalidate on an already-empty cache should not panic.
    app.invalidate_layout_cache();
    assert!(app.layout.epic_stats_cache.is_none());
}

// ---------------------------------------------------------------------------
// Navigation must NOT invalidate the cache
// ---------------------------------------------------------------------------

#[test]
fn navigate_row_does_not_invalidate_populated_cache() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Backlog),
    ]);
    let _ = app.cached_epic_stats();
    assert!(app.layout.epic_stats_cache.is_some());

    app.update(Message::NavigateRow(1));
    assert!(
        app.layout.epic_stats_cache.is_some(),
        "navigate_row must not clear the cache"
    );
}

#[test]
fn navigate_column_does_not_invalidate_populated_cache() {
    let mut app = make_app();
    let _ = app.cached_epic_stats();
    assert!(app.layout.epic_stats_cache.is_some());

    app.update(Message::NavigateColumn(1));
    assert!(
        app.layout.epic_stats_cache.is_some(),
        "navigate_column must not clear the cache"
    );
}

// ---------------------------------------------------------------------------
// Mutations must repopulate the cache with fresh data
// ---------------------------------------------------------------------------

#[test]
fn refresh_tasks_repopulates_cache_with_new_task_stats() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    // Direct field mutation bypasses the message system, so invalidate manually.
    app.board.epics = vec![make_epic(10)];
    app.invalidate_layout_cache();

    let before = app.cached_epic_stats();
    assert_eq!(before[&EpicId(10)].backlog, 0, "epic has no subtasks yet");

    // Refresh board with a new subtask belonging to epic 10
    let mut subtask = make_task(2, TaskStatus::Backlog);
    subtask.epic_id = Some(EpicId(10));
    app.update(Message::Task(TaskMessage::Refresh(vec![
        make_task(1, TaskStatus::Backlog),
        subtask,
    ])));

    let after = app.cached_epic_stats();
    assert_eq!(
        after[&EpicId(10)].backlog,
        1,
        "cache must reflect the new subtask"
    );
}

#[test]
fn refresh_epics_invalidates_and_repopulates_cache() {
    let mut app = make_app();
    app.board.epics = vec![make_epic(10)];

    let _ = app.cached_epic_stats();
    assert!(app.layout.epic_stats_cache.is_some());

    // Replace epics: remove epic 10, add epic 20
    app.update(Message::Epic(EpicMessage::Refresh(vec![make_epic(20)])));

    let after = app.cached_epic_stats();
    assert!(after.contains_key(&EpicId(20)), "new epic must be in cache");
    assert!(
        !after.contains_key(&EpicId(10)),
        "removed epic must not be in cache"
    );
}

#[test]
fn task_created_repopulates_cache() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    app.board.epics = vec![make_epic(10)];

    let _ = app.cached_epic_stats();
    assert!(app.layout.epic_stats_cache.is_some());

    let mut new_subtask = make_task(99, TaskStatus::Backlog);
    new_subtask.epic_id = Some(EpicId(10));
    app.update(Message::Task(TaskMessage::Created { task: new_subtask }));

    let after = app.cached_epic_stats();
    assert_eq!(
        after[&EpicId(10)].backlog,
        1,
        "created subtask must be reflected in cache"
    );
}

#[test]
fn epic_created_invalidates_cache() {
    let mut app = make_app();

    let _ = app.cached_epic_stats();
    assert!(app.layout.epic_stats_cache.is_some());

    app.update(Message::Epic(EpicMessage::Created(make_epic(42))));

    // Cache should be None (invalidated) or Some with new epic — either is acceptable;
    // the key property is that it reflects the new state.
    let after = app.cached_epic_stats();
    assert!(
        after.contains_key(&EpicId(42)),
        "created epic must appear in cache"
    );
}

// ---------------------------------------------------------------------------
// Arc — cached_epic_stats returns the same Arc on repeated calls
// ---------------------------------------------------------------------------

#[test]
fn cached_epic_stats_returns_same_arc_on_repeated_calls() {
    let mut app = make_app();
    let first = app.cached_epic_stats();
    let second = app.cached_epic_stats();
    assert!(
        Arc::ptr_eq(&first, &second),
        "second call should return a clone of the same Arc, not a new allocation"
    );
}

#[test]
fn invalidate_then_reprime_produces_new_arc() {
    let mut app = make_app();
    let before = app.cached_epic_stats();
    app.invalidate_layout_cache();
    let after = app.cached_epic_stats();
    assert!(
        !Arc::ptr_eq(&before, &after),
        "after invalidation a new Arc must be allocated"
    );
}

// ---------------------------------------------------------------------------
// column_anchor_cache — built and read correctly
// ---------------------------------------------------------------------------

#[test]
fn column_anchor_cache_starts_empty_after_invalidation() {
    let mut app = make_app();
    app.invalidate_layout_cache();
    assert!(
        app.layout.column_anchor_cache.is_none(),
        "invalidate_layout_cache must clear column_anchor_cache"
    );
}

#[test]
fn cached_epic_stats_populates_column_anchor_cache() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Backlog),
    ]);
    app.invalidate_layout_cache();
    assert!(app.layout.column_anchor_cache.is_none());
    let _ = app.cached_epic_stats();
    assert!(
        app.layout.column_anchor_cache.is_some(),
        "cached_epic_stats must populate column_anchor_cache"
    );
}

#[test]
fn column_anchor_cache_lists_tasks_in_correct_order() {
    let mut t1 = make_task(1, TaskStatus::Backlog);
    t1.sort_order = Some(10);
    let mut t2 = make_task(2, TaskStatus::Backlog);
    t2.sort_order = Some(5); // t2 sorts before t1

    let mut app = App::new(vec![t1, t2]);
    let _ = app.cached_epic_stats();

    let anchors = app
        .layout
        .column_anchor_cache
        .as_ref()
        .unwrap()
        .get(&TaskStatus::Backlog)
        .unwrap();
    assert_eq!(anchors.len(), 2);
    assert_eq!(
        anchors[0],
        ColumnAnchor::Task(TaskId(2)),
        "lower sort_order first"
    );
    assert_eq!(anchors[1], ColumnAnchor::Task(TaskId(1)));
}

#[test]
fn navigate_row_does_not_clear_column_anchor_cache() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Backlog),
    ]);
    let _ = app.cached_epic_stats();
    assert!(app.layout.column_anchor_cache.is_some());

    app.update(Message::NavigateRow(1));
    assert!(
        app.layout.column_anchor_cache.is_some(),
        "navigation must not clear column_anchor_cache"
    );
}

#[test]
fn update_anchor_from_current_sets_anchor_using_cache() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Backlog),
    ]);
    // prime cache and set cursor to row 1 in Backlog column
    let _ = app.cached_epic_stats();
    app.selection_mut().set_column(1);
    app.selection_mut().set_row(1, 1);

    app.update_anchor_from_current();

    let anchor = app.selection().anchor;
    // The anchor should be set to the task at selectable row 1 in Backlog
    assert!(
        anchor.is_some(),
        "anchor should be set after update_anchor_from_current"
    );
}

#[test]
fn column_anchor_cache_invalidated_on_task_mutation() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    let _ = app.cached_epic_stats();
    assert!(app.layout.column_anchor_cache.is_some());

    app.update(Message::Task(TaskMessage::Refresh(vec![
        make_task(1, TaskStatus::Backlog),
        make_task(99, TaskStatus::Backlog),
    ])));

    // After cache is repopulated, new task should appear
    let _ = app.cached_epic_stats();
    let anchors = app
        .layout
        .column_anchor_cache
        .as_ref()
        .unwrap()
        .get(&TaskStatus::Backlog)
        .unwrap();
    let ids: Vec<_> = anchors
        .iter()
        .filter_map(|a| match a {
            ColumnAnchor::Task(id) => Some(id),
            ColumnAnchor::Epic(_) => None,
        })
        .collect();
    assert!(
        ids.iter().any(|id| **id == TaskId(99)),
        "new task must appear in anchor cache"
    );
}

// ---------------------------------------------------------------------------
// column_items_for_status_with_view_tasks — hoisted tasks_for_current_view
// ---------------------------------------------------------------------------

#[test]
fn column_items_with_precomputed_tasks_matches_standard_path() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Running),
        make_task(3, TaskStatus::Backlog),
    ]);
    let stats = app.cached_epic_stats();
    let view_tasks = app.tasks_for_current_view();

    let via_precomputed = app.column_items_for_status_with_view_tasks(
        TaskStatus::Backlog,
        Some(&*stats),
        &view_tasks,
    );
    let via_standard = app.column_items_for_status_with_stats(TaskStatus::Backlog, Some(&*stats));

    assert_eq!(
        via_precomputed.len(),
        via_standard.len(),
        "pre-computed path must return same items as standard path"
    );
}

// ---------------------------------------------------------------------------
// children_map_cache — built and cleared with the rest of the layout cache
// ---------------------------------------------------------------------------

#[test]
fn children_map_cache_starts_empty_after_invalidation() {
    let mut app = make_app();
    app.invalidate_layout_cache();
    assert!(
        app.layout.children_map_cache.is_none(),
        "invalidate_layout_cache must clear children_map_cache"
    );
}

#[test]
fn cached_epic_stats_populates_children_map_cache() {
    let mut app = make_app();
    app.invalidate_layout_cache();
    assert!(app.layout.children_map_cache.is_none());
    let _ = app.cached_epic_stats();
    assert!(
        app.layout.children_map_cache.is_some(),
        "cached_epic_stats must populate children_map_cache"
    );
}

#[test]
fn children_map_cache_reflects_parent_child_structure() {
    use crate::models::EpicId;
    let mut app = make_app();
    let mut parent = make_epic(1);
    parent.parent_epic_id = None;
    let mut child = make_epic(2);
    child.parent_epic_id = Some(EpicId(1));
    app.board.epics = vec![parent, child];
    app.invalidate_layout_cache();

    let _ = app.cached_epic_stats();

    let map = app.layout.children_map_cache.as_ref().unwrap();
    let children = map.get(&EpicId(1)).expect("parent must have an entry");
    assert!(
        children.contains(&EpicId(2)),
        "child epic must appear under parent"
    );
}

// ---------------------------------------------------------------------------
// epic_filter_cache — populated by cached_epic_stats, cleared by invalidate
// ---------------------------------------------------------------------------

#[test]
fn epic_filter_cache_is_none_before_first_cached_epic_stats_call() {
    let mut app = make_app();
    app.board.epics = vec![make_epic(10)];
    app.invalidate_layout_cache();
    assert!(
        app.layout.epic_filter_cache.is_none(),
        "epic_filter_cache must be None after invalidation"
    );
}

#[test]
fn cached_epic_stats_populates_epic_filter_cache() {
    let mut app = make_app();
    app.board.epics = vec![make_epic(10)];
    app.invalidate_layout_cache();
    assert!(app.layout.epic_filter_cache.is_none());
    let _ = app.cached_epic_stats();
    assert!(
        app.layout.epic_filter_cache.is_some(),
        "cached_epic_stats must populate epic_filter_cache"
    );
}

#[test]
fn invalidate_layout_cache_clears_epic_filter_cache() {
    let mut app = make_app();
    app.board.epics = vec![make_epic(10)];
    let _ = app.cached_epic_stats();
    assert!(app.layout.epic_filter_cache.is_some());
    app.invalidate_layout_cache();
    assert!(
        app.layout.epic_filter_cache.is_none(),
        "invalidate_layout_cache must clear epic_filter_cache"
    );
}

#[test]
fn epic_filter_cache_repo_matches_agrees_with_direct_call_no_filter() {
    let mut app = make_app();
    app.board.epics = vec![make_epic(10)];
    // Direct field mutation bypasses message system; must invalidate to force rebuild.
    app.invalidate_layout_cache();
    // No repo filter: every epic should repo-match.
    let _ = app.cached_epic_stats();
    let cached = app
        .layout
        .epic_filter_cache
        .as_ref()
        .unwrap()
        .get(&EpicId(10))
        .copied()
        .unwrap();
    assert_eq!(
        cached.0,
        app.epic_repo_matches(EpicId(10)),
        "cached repo_matches must equal direct epic_repo_matches"
    );
}

#[test]
fn epic_filter_cache_active_matches_agrees_with_direct_call_no_filter() {
    let mut app = make_app();
    app.board.epics = vec![make_epic(10)];
    // Direct field mutation bypasses message system; must invalidate to force rebuild.
    app.invalidate_layout_cache();
    // No only_active filter: every epic should match.
    let _ = app.cached_epic_stats();
    let cached = app
        .layout
        .epic_filter_cache
        .as_ref()
        .unwrap()
        .get(&EpicId(10))
        .copied()
        .unwrap();
    assert_eq!(
        cached.1,
        app.epic_matches(EpicId(10)),
        "cached active_matches must equal direct epic_matches"
    );
}

// ---------------------------------------------------------------------------
// Auto-invalidation — cached_epic_stats() must self-heal even when a
// mutation forgets to call invalidate_layout_cache(). This is the safety
// net for the "silently stale UI" failure mode: a handler that mutates
// board.tasks/board.epics without invalidating must not be able to make
// cached_epic_stats() return data computed before the mutation.
// ---------------------------------------------------------------------------

#[test]
fn cached_epic_stats_self_heals_after_status_mutation_without_invalidate() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Running)]);
    let before = app.cached_epic_stats();
    assert!(before.is_empty(), "no epics yet");

    // Mutate task status directly via find_task_mut, mirroring a handler
    // that forgets to call invalidate_layout_cache() / sync_board_selection()
    // (this is exactly the bug found in handle_retry_fresh).
    app.board.epics = vec![make_epic(10)];
    {
        let task = app.find_task_mut(TaskId(1)).expect("task 1 must exist");
        task.epic_id = Some(EpicId(10));
        task.status = TaskStatus::Backlog;
    }
    // NOTE: no invalidate_layout_cache() call here — this is the point.

    let after = app.cached_epic_stats();
    assert_eq!(
        after[&EpicId(10)].backlog,
        1,
        "cached_epic_stats must reflect the status mutation even though \
         invalidate_layout_cache() was never called"
    );
}

#[test]
fn column_anchor_cache_self_heals_after_mutation_without_invalidate() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    let _ = app.cached_epic_stats();
    assert!(app.layout.column_anchor_cache.is_some());

    {
        let task = app.find_task_mut(TaskId(1)).expect("task 1 must exist");
        task.status = TaskStatus::Running;
    }
    // NOTE: no invalidate_layout_cache() call here.

    let _ = app.cached_epic_stats();
    let anchors = app.layout.column_anchor_cache.as_ref().unwrap();
    assert!(
        anchors[&TaskStatus::Running].contains(&ColumnAnchor::Task(TaskId(1))),
        "column_anchor_cache must reflect the moved task even without an \
         explicit invalidate"
    );
    assert!(
        !anchors[&TaskStatus::Backlog].contains(&ColumnAnchor::Task(TaskId(1))),
        "task must no longer be anchored under its old status"
    );
}

#[test]
fn cached_epic_stats_still_serves_from_cache_when_nothing_changed() {
    let mut app = make_app();
    app.board.epics = vec![make_epic(10)];
    app.invalidate_layout_cache();

    let first = app.cached_epic_stats();
    let second = app.cached_epic_stats();
    assert!(
        Arc::ptr_eq(&first, &second),
        "unchanged board must still hit the cache (same Arc), not rebuild every call"
    );
}

// ---------------------------------------------------------------------------
// fuzzy_matches_lower — pre-lowercased query variant for the render hot path
// ---------------------------------------------------------------------------

#[test]
fn fuzzy_matches_lower_empty_query_matches_anything() {
    assert!(
        super::fuzzy_matches_lower("SomePath", ""),
        "empty query must match everything"
    );
}

#[test]
fn fuzzy_matches_lower_subsequence_match() {
    assert!(
        super::fuzzy_matches_lower("dispatch task", "dsk"),
        "subsequence must match"
    );
}

#[test]
fn fuzzy_matches_lower_no_match() {
    assert!(
        !super::fuzzy_matches_lower("dispatch", "zz"),
        "non-subsequence must not match"
    );
}

#[test]
fn fuzzy_matches_lower_accepts_already_lowercased_query() {
    // The caller pre-lowercases; the function must not re-lowercase.
    // Query "DPT" would match "dispatch" if lowercased (d,p,t all present) but
    // must NOT match when taken literally (uppercase chars vs lowercase path).
    assert!(
        !super::fuzzy_matches_lower("dispatch", "DPT"),
        "fuzzy_matches_lower must treat query as already lowercased"
    );
    // Lowercase query "dpt" must match "dispatch" (d→0, p→3, t→5).
    assert!(
        super::fuzzy_matches_lower("dispatch", "dpt"),
        "lowercase query must match"
    );
}

// ---------------------------------------------------------------------------
// task_index — O(1) lookup in find_task_mut
// ---------------------------------------------------------------------------

#[test]
fn task_index_is_none_initially() {
    let app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    assert!(
        app.layout.task_index.is_none(),
        "task_index must not be primed in App::new"
    );
}

#[test]
fn find_task_mut_populates_task_index() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    assert!(app.layout.task_index.is_none());
    let _ = app.find_task_mut(TaskId(1));
    assert!(
        app.layout.task_index.is_some(),
        "find_task_mut must build task_index on first call"
    );
}

#[test]
fn find_task_mut_returns_correct_task_via_index() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Running),
        make_task(3, TaskStatus::Done),
    ]);
    let task = app.find_task_mut(TaskId(2)).expect("task 2 must be found");
    assert_eq!(task.id, TaskId(2));
}

#[test]
fn invalidate_layout_cache_clears_task_index() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    let _ = app.find_task_mut(TaskId(1));
    assert!(app.layout.task_index.is_some());
    app.invalidate_layout_cache();
    assert!(
        app.layout.task_index.is_none(),
        "invalidate_layout_cache must clear task_index"
    );
}

#[test]
fn find_task_mut_rebuilds_index_after_invalidation() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    let _ = app.find_task_mut(TaskId(1));
    app.invalidate_layout_cache();
    // task_index is now None; find_task_mut must rebuild
    let task = app
        .find_task_mut(TaskId(1))
        .expect("task must still be found");
    assert_eq!(task.id, TaskId(1));
    assert!(
        app.layout.task_index.is_some(),
        "task_index must be rebuilt by find_task_mut"
    );
}

#[test]
fn find_task_mut_rebuilds_after_direct_tasks_push() {
    // Simulates a test mutating board.tasks directly (length increases).
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    // Prime the index.
    let _ = app.find_task_mut(TaskId(1));
    assert!(app.layout.task_index.is_some());
    // Directly push a task (bypassing message system).
    app.board.tasks.push(make_task(99, TaskStatus::Running));
    // find_task_mut must detect the length mismatch and rebuild.
    let task = app
        .find_task_mut(TaskId(99))
        .expect("task 99 must be found after push");
    assert_eq!(task.id, TaskId(99));
}

#[test]
fn find_task_mut_rebuilds_after_same_length_id_replacement() {
    // A same-length wholesale replacement of board.tasks with a different id
    // set would defeat a length-only staleness check: len() before == len()
    // after, but the old TaskId → index mapping now points at the wrong ids.
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Backlog),
    ]);
    let _ = app.find_task_mut(TaskId(1));
    assert!(app.layout.task_index.is_some());

    // Replace with two different tasks, same length.
    app.board.tasks = vec![
        make_task(10, TaskStatus::Backlog),
        make_task(20, TaskStatus::Backlog),
    ];

    assert!(
        app.find_task_mut(TaskId(10)).is_some(),
        "find_task_mut must find the new task even though the length did not change"
    );
    assert!(
        app.find_task_mut(TaskId(1)).is_none(),
        "the old task id must no longer resolve after a same-length replacement"
    );
}

#[test]
fn find_task_mut_returns_none_for_unknown_id() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
    assert!(
        app.find_task_mut(TaskId(999)).is_none(),
        "must return None for unknown id"
    );
}
