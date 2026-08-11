# 3895 — Hoist the epic search index out of the per-status column-build loop

Follow-up to #3869. Pure performance refactor: no behaviour change, no spec change
(`board_search_filter` in `docs/specs/core.allium` stays as-is).

## Problem

`visible_epics_for_effective_view` builds its own `EpicSearchIndex` per call
(`src/tui/mod.rs:1044`). Callers that iterate statuses therefore rebuild it N times
per action:

- `ColumnLayout::build` (`src/tui/types.rs:908`) — 4 columns → 4 × O(tasks + epics)
  per rendered frame while a query is live. It already hoists `tasks_for_current_view()`
  out of the same loop.
- `cached_epic_stats`'s `column_anchor_cache` loop (`src/tui/mod.rs:1277`) — same 4×,
  on every layout-cache rebuild.
- `clamp_selection` (`src/tui/mod.rs:1651`) and `sync_board_selection`
  (`src/tui/mod.rs:1746`) — 4 × `column_item_count`, each rebuilding.

## Approach

Build the index once per pass and thread it through, exactly as `view_tasks` is.

A newtype, not a bare `Option`, so a caller cannot accidentally pass "no query
active" while a query *is* active:

```rust
/// One pass's epic-search state: `Some` when a query is live, `None` when not.
pub(in crate::tui) struct EpicSearchPass<'a>(Option<EpicSearchIndex<'a>>);
```

Its only constructor is `App::epic_search_pass()`, which keeps the existing
`self.search_active().then(|| self.epic_search_index())` logic. The predicate move
onto the pass: `EpicSearchPass::admits(&self, app, epic_id) -> bool` (`None` admits
everything).

Signature changes:

| fn | change |
|---|---|
| `visible_epics_for_effective_view` | takes `pass: &'a EpicSearchPass<'a>` |
| `column_items_for_status_with_view_tasks` | takes `pass: &EpicSearchPass<'_>` |
| `column_items_for_status_with_stats` | unchanged — builds `view_tasks` and the pass itself |
| `column_item_count` | unchanged — delegates to new `column_item_count_with_pass(status, pass)` |
| `column_item_counts` | new — all four counts from one pass, for the clamp paths |
| `column_items_for_visual_column` | unchanged — builds its own pass |

`column_items_for_visual_column` is left alone deliberately: grep shows it has no
production caller (only `src/tui/tests/repo_filter.rs`), so there is no loop to hoist
out of. `column_items_for_status_with_stats` likewise stays a one-shot convenience.

Hoisting call sites:

1. `ColumnLayout::build` — `let pass = app.epic_search_pass();` next to `view_tasks`,
   passed into all four `from_fn` iterations.
2. `cached_epic_stats` — build the pass next to `view_tasks` before the status loop.
   Both borrows end before `self.layout.column_anchor_cache = …`, so no borrowck
   conflict.
3. `clamp_selection` / `sync_board_selection` — these interleave `selection_mut()`
   with the count, so an immutable pass cannot be held across the mutation. A new
   `App::column_item_counts() -> [usize; TaskStatus::COLUMN_COUNT]` takes all four
   counts from one pass up front; the existing clamp loop then runs over that array.
   Same clamp semantics, one index build per action.
4. `src/tui/update/navigation.rs` — single status per call; leave the existing
   `column_item_count` calls untouched (the task explicitly says not to contort the
   navigation path).

## Hard constraint

The pass stays intra-frame: a local value threaded by parameter, never stored on
`App` and never in `App.layout`. `compute_layout_fingerprint` folds neither titles
nor the query.

## TDD steps

Step 1 (red) — instrument and assert the build count. Add, in `src/tui/mod.rs`:

```rust
#[cfg(test)]
thread_local! {
    /// Counts `App::epic_search_index()` builds so tests can pin the
    /// once-per-pass shape. Per-thread, so parallel tests don't interfere.
    pub(in crate::tui) static EPIC_SEARCH_INDEX_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
```

incremented at the top of `epic_search_index()` under `#[cfg(test)]`. Then new tests
in `src/tui/tests/search.rs`:

- `column_layout_build_builds_the_search_index_once` — query live, two epics + tasks
  spread across statuses; `ColumnLayout::build` → count == 1 (currently 4).
- `column_layout_build_builds_no_search_index_without_a_query` — count == 0.
- `clamp_selection_builds_the_search_index_once` — count == 1 (currently 4).
- `cached_epic_stats_builds_the_search_index_once` — count == 1 (currently 4).

Step 2 (green) — implement the newtype, the signature changes, and the four hoists.
Update the test-only call sites of `visible_epics_for_effective_view`
(`src/tui/tests/search.rs:458`, `src/tui/tests/scenarios.rs:339`) and
`column_items_for_status_with_view_tasks` (`src/tui/tests/layout_cache.rs:372`).

Step 3 — regression net. These must stay green unchanged:
`epic_search_matches_does_not_read_the_layout_cache`,
`visible_epic_cards_do_not_read_the_layout_cache_for_search`,
`visible_epic_cards_agree_with_the_single_epic_predicate` (all
`src/tui/tests/search.rs`), plus the `layout_cache.rs` view-tasks equivalence test.

Step 4 — docs. Update the doc comments on `EpicSearchIndex`,
`visible_epics_for_effective_view`, and `ColumnLayout::build` to say the index is
built once per *frame* and threaded, keeping the "never cached across renders"
warning verbatim.

Verify: `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`
(plus `cargo clippy --all-targets -- -D warnings` via the pre-push hook).
