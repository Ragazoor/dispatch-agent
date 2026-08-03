# Search on epics — design

Task #3839. Board search (`/`) currently narrows task cards only; epic cards are
never narrowed. This design makes epic cards participate in search.

## Current behaviour

- `App::tasks_for_current_view()` (`src/tui/mod.rs:941`) applies the search
  predicate — fuzzy title subsequence OR id-prefix — to tasks.
- Epic cards come from `App::visible_epics_for_effective_view()`
  (`src/tui/mod.rs:876`), which applies only `epic_matches` (only-active filter)
  and `epic_repo_matches` (repo filter). Search is absent.
- `docs/specs/core.allium` states this explicitly under `task_search_filter`:
  "The id-prefix predicate applies to tasks only. Epic cards are never narrowed
  by the search query." That sentence is what this change reverses.

Consequence today: with a live query, every epic card stays on the board
regardless of the query, so search does not narrow the board in hierarchical
mode — the columns still show the full set of epics.

## Matching rule

An epic is visible under a live query when **either**:

- **own match** — the query fuzzy-matches the epic's title (case-insensitive
  forward subsequence, `fuzzy_matches_lower`), or the query's digit payload
  (`id_digits_query`, one optional leading `#`) is a decimal prefix of the
  epic's id (`id_prefix_matches`); or
- **descendant match** — any non-archived descendant task has an own match, or
  any descendant sub-epic has an own match.

An empty query matches every epic. The predicate composes with the repo and
only-active filters by logical AND, exactly as the task predicate does.

Descendant traversal reuses `crate::models::descendant_epic_ids`, the same
primitive `epic_repo_matches` and `epic_matches` use, so "descendant" means the
epic plus its whole sub-epic subtree. Archived descendant tasks are excluded,
mirroring `epic_repo_matches_for_ids`.

### Why own-OR-descendant

In hierarchical board mode, tasks that belong to an epic are not rendered on the
board at all — the epic card is the only way to reach them. Hiding an epic whose
subtask matches would make the match unreachable. Keeping the epic visible
preserves the drill-down path.

## Task filtering is unchanged

Inside an epic view the task filter stays strict: an epic that matched only on
its own title shows only those subtasks that themselves match the query, which
may be none. The filter is uniform; the user clears the query with `Esc` to see
the epic's full contents. No "matched container suppresses the filter" rule.

## Id namespaces

Epic ids and task ids are separate sequences, so a query like `#40` can match
both epic #40 and task #40. Both are shown. This is accepted, not special-cased:
the id-prefix predicate is per-entity and the display already distinguishes
epic cards from task cards.

## Implementation shape

Add, next to the two existing sibling predicates:

- `App::epic_search_matches(epic_id: EpicId) -> bool` — the method form, used by
  `visible_epics_for_effective_view()`.
- `epic_search_matches_for_ids(tasks, epics, epic_ids, query_lower, id_digits)`
  — free function mirroring `epic_repo_matches_for_ids`, holding the actual
  predicate so it is unit-testable without an `App`.

`visible_epics_for_effective_view()` gains one `.filter()` alongside the
existing two. That single call site is the whole board surface: it already backs
`column_items_for_status_with_view_tasks`, `column_item_count`, and
`column_items_for_visual_column`, so every column narrows from one change.

### Not applied to the epic pickers

`reparent_target_epics` and `move_task_target_epics` keep their current
predicates. Those popups have their own input buffers and must not inherit the
board's query. This is why the new predicate is a third filter at the board call
site rather than folded into `epic_matches`.

### No caching

`epic_filter_cache` (`src/tui/types.rs:1103`) stores `(repo_matches,
active_matches)` per epic, guarded by `compute_layout_fingerprint()`
(`src/tui/mod.rs:1153`) which folds ids, status, parent, and sort order — but
neither titles nor the search query. A cached search verdict would therefore go
stale on a title edit or a keystroke in the search bar unless the fingerprint
grew to cover every title plus the query.

Instead the predicate is computed on demand, with an early `true` return when
the query is empty. The non-searching path — the overwhelmingly common one —
costs a single emptiness check per epic. While a query is live the descendant
walk runs per visible epic per render, which is bounded by the board size and
happens only while the user is actively searching.

## Testing

Spec first, then tests, then code.

1. `docs/specs/core.allium` — rewrite the `task_search_filter` block into a
   search filter that covers both entities: keep the task predicate as is,
   replace the "epic cards are never narrowed" sentence with the
   own-OR-descendant rule, and state the strict in-epic task filtering and the
   separate id namespaces. Cross-reference from `epics.allium` epic-visibility.
   Applied with `allium:tend`, verified with `allium:weed`.
2. `src/tui/tests/search.rs` — new cases:
   - epic visible on own fuzzy title match
   - epic visible on own id-prefix match (`38`, `#38`), not on a substring
     (`38` does not match epic 1385)
   - epic visible when a descendant task matches
   - epic visible when a descendant sub-epic matches by its own title
   - epic hidden when nothing in its subtree matches
   - archived descendant task does not keep an epic visible
   - empty query keeps every epic
   - search on epics composes with the repo filter and the only-active filter
   - epic view: sub-epic cards obey the same rule; direct subtasks stay
     strictly filtered
3. `src/tui/tests/scenarios.rs` — key-sequence test: `/` + query narrows epic
   cards live, `Enter` keeps the query, `Esc` restores the full board.
4. `src/tui/tests/snapshots.rs` — one snapshot of a board with epics under a
   live query, showing matched epics kept and unmatched dropped.

## Out of scope

- Ranking or ordering by match quality; visibility is boolean, order unchanged.
- Searching epic descriptions or plans.
- Any change to the search bar UI, the `[/query]` indicator, or key bindings.
- Semantic/RAG search (`search_docs`) — unrelated subsystem.
