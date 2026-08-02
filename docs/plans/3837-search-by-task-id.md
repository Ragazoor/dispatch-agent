# 3837 — Board search should also match task IDs

## Problem

The board's `/` search (`SearchState.query`) filters tasks by a case-insensitive
fuzzy subsequence over `task.title` only (`tasks_for_current_view` in
`src/tui/mod.rs`). Cards display their id as `#3837`, so the natural move —
typing the id you just read in a dispatch prompt or a notification — finds
nothing unless the digits happen to appear in a title.

## Decision

A task matches the query when **either** predicate holds (logical OR):

1. **Title** — the existing case-insensitive forward subsequence
   (`fuzzy_matches_lower`), unchanged.
2. **ID prefix** — the query, after stripping one optional leading `#`, is
   non-empty and all-ASCII-digits, and the task id's decimal string *starts
   with* those digits.

Prefix (not exact, not contains) so the board narrows progressively as the user
types: `38` → `#38, #380, #3837`; `383` → `#3837`; `#3837` → `#3837`.

Scope is **tasks only**. Epic cards are not narrowed by the search query today
(`visible_epics_for_effective_view` consults only the repo / only-active
filters) and this change does not introduce epic search.

The id predicate composes with the repo and only-active filters exactly as the
title predicate does — all three AND together, the two search predicates OR
within the search step.

## Steps

Order is spec → tests → code, per repo convention.

1. **Spec** (`docs/specs/core.allium`) — rewrite the `title_search_filter`
   block as `task_search_filter`: state the OR of title-subsequence and
   id-prefix, the optional `#`, the all-digits requirement, the
   tasks-only scope, and that an empty query still matches everything.
   Verify with `allium check` / `allium:weed`.

2. **Tests** (`src/tui/tests/search.rs`) — add, before touching production code:
   - `search_query_matches_task_id_prefix` — query `38` over ids 38 / 380 /
     3837 / 9 returns the first three.
   - `search_query_matches_task_id_with_hash_prefix` — `#3837` returns #3837.
   - `search_query_id_match_unions_with_title_match` — a digits query returns
     both the id-prefix hit and a task whose *title* fuzzy-matches those
     digits.
   - `search_query_non_numeric_does_not_id_match` — `3a` (mixed) falls back to
     title-only, so an id-prefix-looking task is not returned.
   - `search_query_bare_hash_does_not_id_match` — `#` alone is title-only
     (empty digit remainder), i.e. it does not match every task by id.
   - `search_id_match_composes_with_repo_filter` — id hit in an excluded repo
     is filtered out.
   - `search_query_id_prefix_is_not_a_substring_match` — `38` does not match
     `#1385` (guards against sliding to `contains`).

3. **Code** (`src/tui/mod.rs`) — add a free function
   `id_digits_query(query: &str) -> Option<&str>` returning the digit payload
   (after one optional `#`) when the whole remainder is non-empty ASCII digits,
   plus `id_prefix_matches(id: i64, digits: &str) -> bool`. Compute the digit
   payload **once** outside the per-task closure in `tasks_for_current_view`
   (the render hot path — same reason `query_lower` is hoisted), then widen
   `search_match` to `title_match || id_match`. Unit-test the two helpers
   inline next to `fuzzy_matches_lower`'s tests.

4. **Copy** — help overlay (`src/tui/ui/kanban/popups/help.rs`) `search titles`
   → `search titles/ids`; accept the one affected snapshot
   (`snapshot_help_overlay`) and delete the `.snap.new`. Add the missing `/`
   row to the board keybinding table in `docs/reference.md` describing
   title-or-id search.

5. **Verify** — `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`.

## Risks

- **Perf**: one extra integer→string comparison per task per frame, only when a
  digits query is active; the digit payload is parsed once per call, not per
  task.
- **Surprise matches**: a digits query now returns id hits *in addition to*
  title hits, never fewer results than before — the change is a strict
  widening, so no existing search test should flip.
