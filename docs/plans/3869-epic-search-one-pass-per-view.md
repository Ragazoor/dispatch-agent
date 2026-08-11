# Collapse per-epic O(tasks) scan in epic search into one pass per view

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to
> implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make one board-view pass over epic cards under a live search query cost
a single O(tasks) scan plus O(descendants) per epic, instead of an O(tasks) scan
and two O(epics) scans per epic.

**Architecture:** Introduce an intra-call `EpicSearchIndex` built once per view
pass, holding the parsed query (`query_lower` / `id_digits`), an
`EpicId -> &Epic` lookup, an `EpicId -> Vec<EpicId>` children map, and a
`HashSet<EpicId>` of epics that *directly own* at least one non-archived,
query-matching, board-visible task. `visible_epics_for_effective_view` builds it
once and the per-epic predicate becomes: own-match lookup (O(1)), sub-epic scan
over the descendant set (O(descendants)), then a set-intersection test against
the owner set (O(descendants)). `App::epic_search_matches` is retained as a
single-epic convenience that builds a one-shot index, so every existing test and
the public predicate shape are unchanged.

**Tech Stack:** Rust 2021, ratatui TUI. No new dependencies.

## Global Constraints

- `board_search_filter` in `docs/specs/core.allium` is **normative and must not
  change**. This is a pure performance refactor; behaviour is unchanged and the
  existing tests in `src/tui/tests/search.rs` are the safety net.
- **No cross-render caching of the search verdict.** The index is built and
  dropped inside a single `visible_epics_for_effective_view` call. It must never
  be stored on `App` or in `App.layout`. `compute_layout_fingerprint` folds
  neither titles nor the search query, so a cached verdict would go stale on a
  title edit or a keystroke.
  `epic_search_matches_does_not_read_the_layout_cache`
  (`src/tui/tests/search.rs:413`) guards this and must keep passing.
- Do **not** reuse `layout.children_map_cache` here. Guarding that read with
  `compute_layout_fingerprint()` costs O(tasks + epics) per epic to avoid an
  O(epics) map rebuild — a net pessimisation, reverted once already during
  #3839's simplify pass. Build the children map locally, hoisted to the caller.
- The empty-query fast path must stay: when `search_active()` is false, no index
  is built and no scan happens.
- Inline test modules need `#![allow(clippy::unwrap_used, clippy::expect_used)]`
  (already present at the top of `src/tui/tests/search.rs`).
- Verify with `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`.
  The pre-push gate additionally runs `cargo clippy --all-targets -- -D warnings`
  and `./scripts/check-doc-symbols.sh` — every backticked snake_case identifier
  in a new doc comment must exist in the code.

---

### Task 1: Pin the invariant the refactor must preserve

The single-epic predicate and the view-pass filter must agree on every epic, and
neither may consult the layout cache for the search verdict. Two new tests lock
this before any production code moves.

**Files:**
- Test: `src/tui/tests/search.rs` (append to the
  "visible epic cards under a live query" section at the end of the file)

**Interfaces:**
- Consumes: `App::epic_search_matches(EpicId) -> bool` and
  `App::visible_epics_for_effective_view() -> impl Iterator<Item = &Epic>`
  (both `pub(in crate::tui)`, `src/tui/mod.rs`); the local test helpers
  `visible_epic_ids(&App) -> Vec<i64>`, `epic_child(id, epic, title) -> Task`,
  `make_epic_with_title(id, title) -> Epic`.
- Produces: nothing consumed by later tasks.

- [ ] **Step 1: Write the failing tests**

Append to `src/tui/tests/search.rs`:

```rust
#[test]
fn visible_epic_cards_agree_with_the_single_epic_predicate() {
    // The view pass and the per-epic predicate are two paths to the same
    // verdict: one builds an index once, the other builds a one-shot index.
    // A multi-level board with a hidden sub-epic, a matching grandchild task
    // and an unrelated subtree exercises every branch of both.
    let mut matching_grandchild = epic_child(10, 3, "Fix login bug");
    matching_grandchild.repo_path = "/repo/a".to_string();
    let mut hidden_task = epic_child(11, 4, "Some infra work");
    hidden_task.repo_path = "/repo/b".to_string();
    let unrelated = epic_child(12, 5, "Update invoices");
    let mut app = App::new(vec![matching_grandchild, hidden_task, unrelated]);

    let mut deep = make_epic_with_title(3, "Deep");
    deep.parent_epic_id = Some(EpicId(2));
    let mut mid = make_epic_with_title(2, "Mid");
    mid.parent_epic_id = Some(EpicId(1));
    // Epic 4 matches by title but its only subtask is excluded by the repo
    // filter, so it must not keep its parent (epic 6) visible.
    let mut hidden_sub = make_epic_with_title(4, "Login redesign");
    hidden_sub.parent_epic_id = Some(EpicId(6));
    app.board.epics = vec![
        make_epic_with_title(1, "Root"),
        mid,
        deep,
        hidden_sub,
        make_epic_with_title(5, "Billing rework"),
        make_epic_with_title(6, "Docs cleanup"),
    ];
    app.filter.repos.insert("/repo/a".to_string());
    app.filter.mode = RepoFilterMode::Include;
    app.search.query = "login".to_string();

    // Epic 1 is kept by its matching great-grandchild task; epics 5 and 6 are
    // not kept (5's subtask does not match, 6's matching sub-epic is hidden).
    assert_eq!(visible_epic_ids(&app), vec![1]);

    // Same verdict, epic by epic, from the single-epic predicate. Root epics
    // only — the view pass shows root epics in Board mode.
    let via_predicate: Vec<i64> = [1, 5, 6]
        .into_iter()
        .filter(|&id| {
            let eid = EpicId(id);
            app.epic_matches(eid) && app.epic_repo_matches(eid) && app.epic_search_matches(eid)
        })
        .collect();
    assert_eq!(via_predicate, vec![1]);
}

#[test]
fn visible_epic_cards_do_not_read_the_layout_cache_for_search() {
    // The view-pass counterpart of
    // epic_search_matches_does_not_read_the_layout_cache: the index the pass
    // builds is intra-call, so a cache populated before the keystroke must not
    // answer for search.
    let mut app = App::new(vec![]);
    app.board.epics = vec![
        make_epic_with_title(1, "Login redesign"),
        make_epic_with_title(2, "Billing rework"),
    ];
    let _ = app.cached_epic_stats(); // populates layout.epic_filter_cache
    app.search.query = "zzz".to_string();
    assert!(visible_epic_ids(&app).is_empty());
    app.search.query = "login".to_string();
    assert_eq!(visible_epic_ids(&app), vec![1]);
}
```

- [ ] **Step 2: Run the tests — they must PASS on the current code**

Run: `cargo test tui::tests::search`

Expected: PASS. These tests describe behaviour that already holds; they exist so
Task 2's rewrite cannot change it silently. If either fails now, stop — the
current behaviour is not what this plan assumes, and that is a finding to raise
before refactoring.

- [ ] **Step 3: Commit**

```bash
git add src/tui/tests/search.rs
git commit -m "test(search): pin view-pass/predicate agreement before the epic-search refactor"
```

---

### Task 2: One O(tasks) pass per view via `EpicSearchIndex`

**Files:**
- Modify: `src/tui/mod.rs` — replace `epic_search_matches_for_ids`
  (`src/tui/mod.rs:455`) with `epic_ids_owning_matching_task`; add
  `EpicSearchIndex`; rewrite `App::epic_search_matches`
  (`src/tui/mod.rs:925`) and `App::visible_epics_for_effective_view`
  (`src/tui/mod.rs:970`).
- Test: `src/tui/tests/search.rs` (Task 1's tests are the gate; add one unit
  test for the new free function)

**Interfaces:**
- Consumes: `own_search_match(&str, i64, &str, Option<&str>) -> bool`
  (`src/tui/mod.rs:437`), `id_digits_query(&str) -> Option<&str>`,
  `FilterState::matches(&str) -> bool`, `FilterState::task_matches(&Task) -> bool`,
  `crate::models::build_children_map(&[Epic]) -> HashMap<EpicId, Vec<EpicId>>`,
  `crate::models::descendant_epic_ids_with_map(EpicId, &HashMap<EpicId, Vec<EpicId>>) -> HashSet<EpicId>`.
- Produces:
  - `pub(in crate::tui) fn epic_ids_owning_matching_task(tasks: &[Task], filter: &FilterState, query_lower: &str, id_digits: Option<&str>) -> HashSet<EpicId>`
  - `pub(in crate::tui) struct EpicSearchIndex<'a>` with private fields
  - `pub(in crate::tui) fn App::epic_search_index(&self) -> EpicSearchIndex<'_>`
  - `pub(in crate::tui) fn App::epic_search_matches_indexed(&self, index: &EpicSearchIndex<'_>, epic_id: EpicId) -> bool`
  - `App::epic_search_matches(&self, epic_id: EpicId) -> bool` (signature unchanged)
- Removes: `epic_search_matches_for_ids` — it has exactly one caller
  (`src/tui/mod.rs:955`) and no test or doc references outside `docs/plans/`.

- [ ] **Step 1: Write the failing test for the owner-set pass**

Append to the `epic_search_matches` section of `src/tui/tests/search.rs` (before
the "visible epic cards" section):

```rust
#[test]
fn epic_ids_owning_matching_task_collects_only_board_visible_matches() {
    use crate::tui::epic_ids_owning_matching_task;

    let matching = epic_child(10, 1, "Fix login bug");
    let mut wrong_repo = epic_child(11, 2, "Fix login bug");
    wrong_repo.repo_path = "/repo/b".to_string();
    let mut archived = epic_child(12, 3, "Fix login bug");
    archived.status = TaskStatus::Archived;
    let no_match = epic_child(13, 4, "Update invoices");
    let standalone = test_task(14, "Fix login bug"); // epic_id is None

    let mut filter = FilterState::default();
    filter.repos.insert("/repo".to_string());
    filter.mode = RepoFilterMode::Include;

    let owners = epic_ids_owning_matching_task(
        &[matching, wrong_repo, archived, no_match, standalone],
        &filter,
        "login",
        None,
    );

    // Only epic 1: epic 2's match is outside the repo filter, epic 3's is
    // archived, epic 4 has no query match, and the standalone task owns no epic.
    assert_eq!(owners, [EpicId(1)].into_iter().collect());
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test tui::tests::search::epic_ids_owning_matching_task_collects_only_board_visible_matches`

Expected: FAIL to compile — `cannot find function 'epic_ids_owning_matching_task'
in crate 'tui'`.

- [ ] **Step 3: Replace `epic_search_matches_for_ids` with the owner-set pass**

In `src/tui/mod.rs`, delete `epic_search_matches_for_ids` (lines 447–469,
doc comment included) and put this in its place:

```rust
/// The epic ids that *directly own* at least one non-archived task carrying the
/// board-search match: the task has an own match (title or id-prefix) AND the
/// board would actually show it under the repo and only-active filters
/// (`FilterState::matches` on its repo_path, and `FilterState::task_matches`) —
/// the same two predicates `tasks_for_current_view` applies. A task the board
/// would hide cannot keep an ancestor epic's card alive: drilling into that
/// card would be a dead end. See board_search_filter in `docs/specs/core.allium`.
///
/// One O(tasks) pass for the whole board, so a view pass over N epic cards costs
/// one scan rather than N. An epic's own verdict is then a set-membership test
/// per descendant — see [`EpicSearchIndex`].
pub(in crate::tui) fn epic_ids_owning_matching_task(
    tasks: &[Task],
    filter: &FilterState,
    query_lower: &str,
    id_digits: Option<&str>,
) -> HashSet<EpicId> {
    tasks
        .iter()
        .filter(|t| {
            t.status != TaskStatus::Archived
                && own_search_match(&t.title, t.id.0, query_lower, id_digits)
                && filter.matches(&t.repo_path)
                && filter.task_matches(t)
        })
        .filter_map(|t| t.epic_id)
        .collect()
}
```

- [ ] **Step 4: Add `EpicSearchIndex`**

Add immediately after `epic_ids_owning_matching_task` in `src/tui/mod.rs`:

```rust
/// Per-view-pass state for the epic board-search predicate, built once by
/// [`App::epic_search_index`] and reused across every epic in the pass.
///
/// Collapses what used to be per-epic work: the query is parsed once (not once
/// per epic), the O(tasks) owner scan runs once (not once per epic), and the
/// epic children map and id lookup are built once so the own-match check is
/// O(1) and the sub-epic scan is O(descendants) rather than O(epics).
///
/// **Intra-call only.** This is a local value with the lifetime of one view
/// pass; it is never stored on `App` or in `App.layout`. `App.layout` is guarded
/// by `compute_layout_fingerprint()`, which folds ids, status, parent and sort
/// order but neither titles nor the query, so a cross-render cached search
/// verdict would go stale on a title edit or a keystroke in the search bar.
pub(in crate::tui) struct EpicSearchIndex<'a> {
    query_lower: String,
    id_digits: Option<&'a str>,
    by_id: HashMap<EpicId, &'a Epic>,
    children: HashMap<EpicId, Vec<EpicId>>,
    task_owners: HashSet<EpicId>,
}
```

Confirm the file's existing imports already cover `HashMap`, `HashSet`, `Epic`,
`EpicId`, `Task`, `TaskStatus` and `FilterState`; if `HashMap` is not in scope at
this position, add it to the existing `use std::collections::{...}` line rather
than writing a fully-qualified path.

- [ ] **Step 5: Rewrite the two `App` methods**

Replace the body of `App::epic_search_matches` (`src/tui/mod.rs:925`) and its
doc comment with the three items below, keeping the doc comment's existing
first two paragraphs (they document normative behaviour) and folding the
"Deliberately uncached" paragraph into `epic_search_matches_indexed`:

```rust
    /// Build the per-pass search index for the current board and query. Cheap
    /// when no query is live is *not* a property of this function — call it only
    /// behind a `search_active()` check.
    pub(in crate::tui) fn epic_search_index(&self) -> EpicSearchIndex<'_> {
        let query_lower = self.search.query.to_lowercase();
        // Parsed once per pass, not per epic: this is the render hot path.
        let id_digits = id_digits_query(&self.search.query);
        EpicSearchIndex {
            task_owners: epic_ids_owning_matching_task(
                &self.board.tasks,
                &self.filter,
                &query_lower,
                id_digits,
            ),
            by_id: self.board.epics.iter().map(|e| (e.id, e)).collect(),
            children: crate::models::build_children_map(&self.board.epics),
            query_lower,
            id_digits,
        }
    }

    /// Whether the epic should be shown under the active board-search query,
    /// answered from a prebuilt [`EpicSearchIndex`].
    ///
    /// `E`'s own title/id match needs no extra gating: callers (see
    /// [`Self::visible_epics_for_effective_view`]) already require
    /// `epic_matches(E) && epic_repo_matches(E)` before this predicate runs.
    /// A descendant sub-epic or descendant task only counts toward `E`'s
    /// match when it would itself be visible under the repo and only-active
    /// filters — a descendant the board hides cannot keep `E`'s card alive,
    /// since the card would then be a dead end. See board_search_filter in
    /// `docs/specs/core.allium`.
    ///
    /// Deliberately uncached across renders, unlike [`Self::epic_matches`] and
    /// [`Self::epic_repo_matches`]: see the note on [`EpicSearchIndex`].
    pub(in crate::tui) fn epic_search_matches_indexed(
        &self,
        index: &EpicSearchIndex<'_>,
        epic_id: EpicId,
    ) -> bool {
        let own_match = index.by_id.get(&epic_id).is_some_and(|e| {
            own_search_match(&e.title, e.id.0, &index.query_lower, index.id_digits)
        });
        if own_match {
            return true;
        }

        let epic_ids = crate::models::descendant_epic_ids_with_map(epic_id, &index.children);

        let sub_epic_matches = epic_ids.iter().any(|&id| {
            id != epic_id
                && index.by_id.get(&id).is_some_and(|e| {
                    own_search_match(&e.title, e.id.0, &index.query_lower, index.id_digits)
                })
                && self.epic_matches(id)
                && self.epic_repo_matches(id)
        });
        if sub_epic_matches {
            return true;
        }

        epic_ids.iter().any(|id| index.task_owners.contains(id))
    }

    /// Whether the epic should be shown under the active board-search query.
    ///
    /// Single-epic convenience: builds a one-shot [`EpicSearchIndex`] and
    /// delegates to [`Self::epic_search_matches_indexed`]. A caller filtering
    /// many epics at once should build the index once instead — see
    /// [`Self::visible_epics_for_effective_view`].
    pub(in crate::tui) fn epic_search_matches(&self, epic_id: EpicId) -> bool {
        if !self.search_active() {
            return true;
        }
        self.epic_search_matches_indexed(&self.epic_search_index(), epic_id)
    }
```

Then rewrite `App::visible_epics_for_effective_view` (`src/tui/mod.rs:970`),
keeping its existing doc comment and adding one sentence to it:

```rust
    /// Epics visible in the current board/epic view, filtered by the active
    /// repo / only-active filters and the board-search query: root epics (no
    /// parent) in `Board` mode, direct children of the current epic in `Epic`
    /// mode. Shared by `column_items_for_status_with_view_tasks`,
    /// `column_item_count`, and `column_items_for_visual_column` so an
    /// epic-visibility rule change is made in one place instead of three.
    ///
    /// The search index is built once for the whole pass (`None` when no query
    /// is live, so a non-searching render stays free), which is what keeps the
    /// pass at one O(tasks) scan rather than one per epic.
    pub(in crate::tui) fn visible_epics_for_effective_view(&self) -> impl Iterator<Item = &Epic> {
        let parent = match self.effective_view_mode() {
            BoardViewMode::Board(_) => None,
            BoardViewMode::Epic { epic_id, .. } => Some(epic_id),
        };
        let index = self.search_active().then(|| self.epic_search_index());
        self.board
            .epics
            .iter()
            .filter(move |e| e.parent_epic_id == parent)
            .filter(move |e| {
                self.epic_matches(e.id)
                    && self.epic_repo_matches(e.id)
                    && index
                        .as_ref()
                        .is_none_or(|idx| self.epic_search_matches_indexed(idx, e.id))
            })
    }
```

If `Option::is_none_or` is not available on this toolchain's MSRV, use
`index.as_ref().map_or(true, |idx| ...)` and keep clippy happy by matching
whatever form the rest of the file already uses.

- [ ] **Step 6: Run the tests**

Run: `cargo test tui::tests::search`

Expected: PASS, including Task 1's two tests and the new
`epic_ids_owning_matching_task_collects_only_board_visible_matches`.

- [ ] **Step 7: Run the full verification**

Run: `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`

Expected: PASS. Then run the clippy gate the pre-push hook applies:
`cargo clippy --all-targets -- -D warnings` and
`./scripts/check-doc-symbols.sh`.

If `check-doc-symbols.sh` flags a backticked identifier in a new doc comment,
fix the comment to name a symbol that exists — do not add an
`allow-phantom-symbol` marker, since every symbol referenced here is real.

- [ ] **Step 8: Commit**

```bash
git add src/tui/mod.rs src/tui/tests/search.rs
git commit -m "perf(search): collapse per-epic task scan into one pass per view"
```

---

### Task 3: Confirm the spec needs no change

**Files:**
- Read: `docs/specs/core.allium` (`board_search_filter`)

**Interfaces:**
- Consumes: nothing. Produces: nothing.

- [ ] **Step 1: Re-read `board_search_filter`**

Confirm every clause it states still holds after Task 2: own title/id match,
descendant sub-epic match gated by the epic's own visibility, descendant task
match gated by repo and only-active filters, archived descendants ignored, empty
query a no-op.

- [ ] **Step 2: Leave the spec untouched**

No spec edit is expected — this is a pure performance refactor and
`board_search_filter` is normative. If Task 2's rewrite turned out to require a
behaviour change, that is a stop condition: raise it rather than editing the
spec to match the code.

---

## Self-Review

**Spec coverage.** The task description asks for three things, all covered:
one O(tasks) pass per view pass (Task 2, `epic_ids_owning_matching_task` +
`task_owners`), `query_lower`/`id_digits` hoisted out of the per-epic path
(Task 2, `epic_search_index`), and an id->`&Epic` lookup plus children map that
turns the two O(epics) scans into O(1) and O(descendants) (Task 2, `by_id` /
`children`). The "do not cache across renders" constraint is enforced by the
`EpicSearchIndex` lifetime, documented on the type, and tested from both the
predicate and the view pass (Task 1).

**Placeholders.** None: every code step carries the actual code.

**Type consistency.** `epic_ids_owning_matching_task` returns
`HashSet<EpicId>`, stored as `EpicSearchIndex::task_owners` and read via
`contains(&EpicId)`. `descendant_epic_ids_with_map` returns `HashSet<EpicId>`
and takes `&HashMap<EpicId, Vec<EpicId>>`, matching `build_children_map`'s
return type. `id_digits` is `Option<&'a str>` throughout, borrowed from
`self.search.query`, matching `id_digits_query`'s return type and
`own_search_match`'s fourth parameter.
