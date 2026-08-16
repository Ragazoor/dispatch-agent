# Search on Epics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the board search query (`/`) narrow epic cards, not just task cards: an epic stays visible when it matches the query itself or when anything in its subtree matches.

**Architecture:** A new predicate `App::epic_search_matches(epic_id)` joins the two existing sibling predicates (`epic_matches` for the only-active filter, `epic_repo_matches` for the repo filter) as a third `.filter()` inside `App::visible_epics_for_effective_view()` — the single call site that backs every board column. The predicate itself lives in a free function `epic_search_matches_for_ids()`, mirroring the existing `epic_repo_matches_for_ids()`. Deliberately uncached, with an empty-query fast path.

**Tech Stack:** Rust 2021, ratatui TUI, `insta` snapshots, inline `#[test]` modules under `src/tui/tests/`.

**Design doc:** `docs/superpowers/specs/2026-08-03-search-on-epics-design.md`

## Global Constraints

- Spec first, then tests, then code. `docs/specs/*.allium` is the source of truth; Task 1 changes the spec before any Rust changes.
- TDD: every behaviour lands as a failing test before the implementation.
- Inline test modules need `#[allow(clippy::unwrap_used, clippy::expect_used)]` at the top. `src/tui/tests/search.rs` already has it — do not remove it.
- Snapshot tests render to a 120×40 `TestBackend`. **Do not change the backend size.** Always `rm src/tui/tests/snapshots/*.snap.new` after accepting.
- Clippy runs as `cargo clippy --all-targets -- -D warnings` in the pre-push hook. A plain `cargo build` will not catch violations.
- Verify command for this task: `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`
- The new predicate must **not** be applied to `reparent_target_epics` or `move_task_target_epics` (`src/tui/mod.rs:894` and `src/tui/mod.rs:915`). Those popups have their own input buffers.
- The new predicate must **not** consult `layout.epic_filter_cache`.
- Work only inside this worktree.

---

### Task 1: Spec — board search covers epics

The spec currently asserts the opposite of what we are building, so it changes first.

**Files:**
- Modify: `docs/specs/core.allium:815-841` (the `task_search_filter` comment block)
- Modify: `docs/specs/epics.allium:543-554` (`ReparentEpic` `@guidance`)
- Modify: `docs/specs/tasks.allium:414-429` (`MoveTaskToEpic` `@guidance`)

**Interfaces:**
- Consumes: nothing.
- Produces: the normative rule that Tasks 2–4 test against. No Rust symbols.

- [ ] **Step 1: Replace the `task_search_filter` block in `docs/specs/core.allium`**

Delete lines 815–841 (from `-- == Task Search Filter ==` through the `-- Cursor re-clamping on query change is covered by selection_preservation.` line) and put this in their place:

```
-- == Board Search Filter ==

-- board_search_filter:
--   When view.search_query is non-empty, the query narrows BOTH task cards and
--   epic cards. An empty query matches everything. This filter is applied in
--   addition to the repo filter and the only-active filter (logical AND).
--
--   own_match(entity) — the shared per-entity predicate. It holds when EITHER
--   sub-predicate holds (logical OR):
--     title match: a case-insensitive forward subsequence of entity.title (every
--       query char appears in the title, in order) — the same predicate used by
--       the repo-path picker (fuzzy_matches).
--     id-prefix match: the query, after stripping one optional leading '#', is
--       non-empty and consists entirely of ASCII digits, and the decimal
--       spelling of entity.id starts with those digits. So "38" matches #38,
--       #380 and #3837 (progressive narrowing as the user types) but not
--       #1385; "#3837" matches #3837; "3a" and a bare "#" have no digit
--       payload and so fall back to title matching alone.
--
--   Tasks: a task is included in tasks_for_current_view when own_match(task)
--   holds. This applies in the board view and inside an epic view alike: an
--   epic that was surfaced by its own title still shows only those subtasks
--   that match the query, which may be none. Clearing the query is how the
--   user sees an epic's full contents.
--
--   Epics: an epic card is visible when own_match(epic) holds, OR when any
--   descendant sub-epic satisfies own_match, OR when any non-archived task in
--   the epic's subtree satisfies own_match. Archived tasks never keep an epic
--   visible. The descendant clause exists because a hierarchical board does not
--   render epic-owned tasks at all — the epic card is the only path to them, so
--   hiding it would make the match unreachable.
--
--   Epic ids and task ids are separate sequences, so one digit payload can match
--   both an epic card and a task card. Both are shown.
--
--   Epic-target pickers (reparent, move-task-to-epic) are NOT narrowed by the
--   search query — they carry their own input buffers. See the guidance on
--   ReparentEpic in epics.allium and MoveTaskToEpic in tasks.allium.
--
--   Lifecycle: the [/] key opens a live search bar and the board narrows as
--   the user types. [Enter] keeps the query active and closes the bar (the
--   filter persists, with a [/query] indicator shown on the board). [Esc]
--   in the bar restores the query that was active before the bar opened.
--   [Esc] on the board (bar closed) with an active query clears it.
--   The query is session-scoped and does not persist across app restarts.
--   Cursor re-clamping on query change is covered by selection_preservation.
```

- [ ] **Step 2: Correct the reparent-picker guidance in `docs/specs/epics.allium`**

At line 548–551 the guidance reads "epics that are hidden by the active board filter (repo include/exclude and the only-active filter — the same predicates the board uses to decide epic visibility)". That parenthetical becomes wrong once the board also applies search, so make the exclusion explicit. Replace:

```
        -- hidden by the active board filter (repo include/exclude and the
        -- only-active filter — the same predicates the board uses to decide
        -- epic visibility). When status filtering removes a parent but keeps
```

with:

```
        -- hidden by the active board filter (repo include/exclude and the
        -- only-active filter). The board search query is deliberately NOT
        -- applied here — this picker has its own input buffer, so inheriting
        -- the board query would hide otherwise-valid targets. See
        -- board_search_filter in core.allium. When status filtering removes a
        -- parent but keeps
```

- [ ] **Step 3: Correct the move-task picker guidance in `docs/specs/tasks.allium`**

Same correction at lines 421–424. Replace:

```
        -- targets: it excludes epics in Done or Archived status and epics
        -- hidden by the active board filter (repo include/exclude and the
        -- only-active filter — the same predicates the board uses to decide
        -- epic visibility). Unlike epic reparenting there is no cycle
```

with:

```
        -- targets: it excludes epics in Done or Archived status and epics
        -- hidden by the active board filter (repo include/exclude and the
        -- only-active filter). The board search query is deliberately NOT
        -- applied here — this picker has its own input buffer. See
        -- board_search_filter in core.allium. Unlike epic reparenting there is
        -- no cycle
```

- [ ] **Step 4: Validate the specs**

Run: `allium check docs/specs/core.allium docs/specs/epics.allium docs/specs/tasks.allium`
Expected: no errors. (The edits are all inside comments and `@guidance` prose, so no structural change — if `allium` is unavailable on PATH, note it and continue; `./scripts/check-doc-paths.sh` in Step 5 still covers the path citations.)

- [ ] **Step 5: Run the doc checkers**

Run: `./scripts/check-doc-paths.sh` then `./scripts/check-doc-symbols.sh`
Expected: both pass. `check-doc-symbols.sh` rejects backticked snake_case identifiers that appear nowhere in the code — the new spec text uses `board_search_filter`, `own_match` and `fuzzy_matches` unbackticked in comment prose, which the checker does not flag. If it does flag something, unbacktick it rather than adding an allow marker.

- [ ] **Step 6: Commit**

```bash
git add docs/specs/core.allium docs/specs/epics.allium docs/specs/tasks.allium
git commit -m "spec(search): board search narrows epic cards, not just tasks"
```

---

### Task 2: The `epic_search_matches` predicate

Adds the predicate and its unit tests. Nothing consumes it yet, so no existing behaviour changes — this task is green on its own.

**Files:**
- Modify: `src/tui/mod.rs` — add `epic_search_matches_for_ids()` free function after `epic_active_matches_for_ids()` (ends at `src/tui/mod.rs:431`), and the `App::epic_search_matches()` method after `App::epic_matches()` (ends at `src/tui/mod.rs:868`)
- Test: `src/tui/tests/search.rs`

**Interfaces:**
- Consumes: `fuzzy_matches_lower(path: &str, query_lower: &str) -> bool` (`src/tui/mod.rs:353`), `id_digits_query(query: &str) -> Option<&str>` (`src/tui/mod.rs:371`), `id_prefix_matches(id: i64, digits: &str) -> bool` (`src/tui/mod.rs:379`), `crate::models::descendant_epic_ids(root: EpicId, epics: &[Epic]) -> HashSet<EpicId>`.
- Produces:
  - `pub(in crate::tui) fn epic_search_matches_for_ids(tasks: &[Task], epics: &[Epic], epic_ids: &HashSet<EpicId>, query_lower: &str, id_digits: Option<&str>) -> bool`
  - `pub(in crate::tui) fn App::epic_search_matches(&self, epic_id: EpicId) -> bool`

- [ ] **Step 1: Extend the imports in the test file**

In `src/tui/tests/search.rs`, change line 3 from:

```rust
use crate::models::TaskStatus;
```

to:

```rust
use crate::models::{EpicId, TaskStatus};
```

- [ ] **Step 2: Write the failing tests**

Append to `src/tui/tests/search.rs`:

```rust
// ---------------------------------------------------------------------------
// epic_search_matches — see board_search_filter in docs/specs/core.allium
// ---------------------------------------------------------------------------

/// A subtask of `epic` with an explicit title.
fn epic_child(id: i64, epic: i64, title: &str) -> Task {
    let mut t = test_task(id, title);
    t.epic_id = Some(EpicId(epic));
    t
}

#[test]
fn epic_search_matches_empty_query_matches_every_epic() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic_with_title(1, "Billing rework")];
    assert!(app.epic_search_matches(EpicId(1)));
}

#[test]
fn epic_search_matches_own_title_fuzzy() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![
        make_epic_with_title(1, "Login redesign"),
        make_epic_with_title(2, "Billing rework"),
    ];
    app.search.query = "lgn".to_string();
    assert!(app.epic_search_matches(EpicId(1)));
    assert!(!app.epic_search_matches(EpicId(2)));
}

#[test]
fn epic_search_matches_own_id_prefix() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![
        make_epic_with_title(38, "alpha"),
        make_epic_with_title(380, "beta"),
        make_epic_with_title(1385, "gamma"),
    ];
    app.search.query = "38".to_string();
    assert!(app.epic_search_matches(EpicId(38)));
    assert!(app.epic_search_matches(EpicId(380)));
    // Prefix, not substring: 1385 contains "38" but does not start with it.
    assert!(!app.epic_search_matches(EpicId(1385)));
}

#[test]
fn epic_search_matches_own_id_with_hash_prefix() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic_with_title(38, "alpha")];
    app.search.query = "#38".to_string();
    assert!(app.epic_search_matches(EpicId(38)));
}

#[test]
fn epic_search_matches_descendant_task_title() {
    let mut app = App::new(vec![epic_child(10, 1, "Fix login bug")]);
    app.board.epics = vec![make_epic_with_title(1, "Billing rework")];
    app.search.query = "login".to_string();
    // The epic's own title has no match; its subtask carries it.
    assert!(app.epic_search_matches(EpicId(1)));
}

#[test]
fn epic_search_matches_descendant_task_id() {
    let mut app = App::new(vec![epic_child(3837, 1, "alpha")]);
    app.board.epics = vec![make_epic_with_title(1, "Billing rework")];
    app.search.query = "3837".to_string();
    assert!(app.epic_search_matches(EpicId(1)));
}

#[test]
fn epic_search_matches_descendant_sub_epic_title() {
    let mut app = App::new(vec![]);
    let mut child = make_epic_with_title(2, "Login redesign");
    child.parent_epic_id = Some(EpicId(1));
    app.board.epics = vec![make_epic_with_title(1, "Billing rework"), child];
    app.search.query = "login".to_string();
    // Root epic kept because a sub-epic in its subtree matches.
    assert!(app.epic_search_matches(EpicId(1)));
}

#[test]
fn epic_search_matches_grandchild_task_title() {
    let mut app = App::new(vec![epic_child(10, 2, "Fix login bug")]);
    let mut child = make_epic_with_title(2, "Sub");
    child.parent_epic_id = Some(EpicId(1));
    app.board.epics = vec![make_epic_with_title(1, "Root"), child];
    app.search.query = "login".to_string();
    assert!(app.epic_search_matches(EpicId(1)));
}

#[test]
fn epic_search_matches_no_match_anywhere_is_false() {
    let mut app = App::new(vec![epic_child(10, 1, "Update invoices")]);
    app.board.epics = vec![make_epic_with_title(1, "Billing rework")];
    app.search.query = "login".to_string();
    assert!(!app.epic_search_matches(EpicId(1)));
}

#[test]
fn epic_search_matches_ignores_archived_descendant_task() {
    let mut archived = epic_child(10, 1, "Fix login bug");
    archived.status = TaskStatus::Archived;
    let mut app = App::new(vec![archived]);
    app.board.epics = vec![make_epic_with_title(1, "Billing rework")];
    app.search.query = "login".to_string();
    assert!(!app.epic_search_matches(EpicId(1)));
}

#[test]
fn epic_search_matches_ignores_task_in_a_different_epic() {
    let mut app = App::new(vec![epic_child(10, 2, "Fix login bug")]);
    app.board.epics = vec![
        make_epic_with_title(1, "Billing rework"),
        make_epic_with_title(2, "Auth"),
    ];
    app.search.query = "login".to_string();
    // Epic 2 is not a descendant of epic 1 (no parent link).
    assert!(!app.epic_search_matches(EpicId(1)));
    assert!(app.epic_search_matches(EpicId(2)));
}

#[test]
fn epic_search_matches_does_not_read_the_layout_cache() {
    // A stale cache must never answer for search: the layout fingerprint
    // covers neither titles nor the query.
    let mut app = App::new(vec![]);
    app.board.epics = vec![make_epic_with_title(1, "Login redesign")];
    let _ = app.cached_epic_stats(); // populates layout.epic_filter_cache
    app.search.query = "zzz".to_string();
    assert!(!app.epic_search_matches(EpicId(1)));
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test tui::tests::search 2>&1 | tail -20`
Expected: compile error — `no method named epic_search_matches found for struct App`.

- [ ] **Step 4: Add the free function**

In `src/tui/mod.rs`, immediately after `epic_active_matches_for_ids()` (which ends at line 431) insert:

```rust
/// Whether the epic (identified by `epic_ids` = the epic itself plus all its
/// descendant epics) matches the active board-search query. `query_lower` is the
/// lowercased query and `id_digits` its digit payload (see [`id_digits_query`]).
///
/// An epic matches when it or any descendant sub-epic has an own match (fuzzy
/// title subsequence or id-prefix), or when any non-archived task in the subtree
/// has an own match. Archived tasks never keep an epic visible. An empty query
/// matches every epic.
///
/// Epic ids and task ids are separate sequences, so one digit payload can match
/// both an epic card and a task card; both are shown. See `board_search_filter`
/// in `docs/specs/core.allium`.
pub(in crate::tui) fn epic_search_matches_for_ids(
    tasks: &[Task],
    epics: &[Epic],
    epic_ids: &HashSet<EpicId>,
    query_lower: &str,
    id_digits: Option<&str>,
) -> bool {
    if query_lower.is_empty() {
        return true;
    }
    let own_match = |title: &str, id: i64| {
        fuzzy_matches_lower(title, query_lower)
            || id_digits.is_some_and(|digits| id_prefix_matches(id, digits))
    };
    epics
        .iter()
        .any(|e| epic_ids.contains(&e.id) && own_match(&e.title, e.id.0))
        || tasks.iter().any(|t| {
            matches!(t.epic_id, Some(eid) if epic_ids.contains(&eid))
                && t.status != TaskStatus::Archived
                && own_match(&t.title, t.id.0)
        })
}
```

- [ ] **Step 5: Add the method**

In `src/tui/mod.rs`, immediately after `App::epic_matches()` (which ends at line 868) insert:

```rust
    /// Whether the epic should be shown under the active board-search query.
    ///
    /// Deliberately uncached, unlike [`Self::epic_matches`] and
    /// [`Self::epic_repo_matches`]: `layout.epic_filter_cache` is guarded by
    /// `compute_layout_fingerprint()`, which folds ids, status, parent and sort
    /// order but neither titles nor the query — a cached verdict would go stale
    /// on a title edit or a keystroke in the search bar. The empty-query fast
    /// path keeps the non-searching render free.
    pub(in crate::tui) fn epic_search_matches(&self, epic_id: EpicId) -> bool {
        if !self.search_active() {
            return true;
        }
        let query_lower = self.search.query.to_lowercase();
        let id_digits = id_digits_query(&self.search.query);
        let epic_ids = crate::models::descendant_epic_ids(epic_id, &self.board.epics);
        epic_search_matches_for_ids(
            &self.board.tasks,
            &self.board.epics,
            &epic_ids,
            &query_lower,
            id_digits,
        )
    }
```

If `HashSet` or `Epic` is not already in scope at the top of `src/tui/mod.rs`, add it to the existing `use` list rather than writing a fully-qualified path inline (both are already used by neighbouring code in this file: `epic_repo_matches_for_ids` takes `&HashSet<EpicId>`, and `visible_epics_for_effective_view` returns `&Epic`).

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test tui::tests::search`
Expected: PASS, all cases.

- [ ] **Step 7: Clippy and format**

Run: `cargo fmt` then `cargo clippy --all-targets -- -D warnings`
Expected: clean. `cargo fmt` may reformat unrelated files — check `git status` and only stage `src/tui/mod.rs` and `src/tui/tests/search.rs`.

- [ ] **Step 8: Commit**

```bash
git add src/tui/mod.rs src/tui/tests/search.rs
git commit -m "feat(search): add epic_search_matches predicate"
```

---

### Task 3: Wire the predicate into board epic visibility

**Files:**
- Modify: `src/tui/mod.rs:876-886` (`App::visible_epics_for_effective_view`)
- Test: `src/tui/tests/search.rs`

**Interfaces:**
- Consumes: `App::epic_search_matches(&self, epic_id: EpicId) -> bool` from Task 2.
- Produces: `visible_epics_for_effective_view()` narrowed by search. No signature change.

- [ ] **Step 1: Write the failing tests**

Append to `src/tui/tests/search.rs`:

```rust
// ---------------------------------------------------------------------------
// visible epic cards under a live query
// ---------------------------------------------------------------------------

/// Ids of the epic cards the current view would render, ascending.
fn visible_epic_ids(app: &App) -> Vec<i64> {
    let mut ids: Vec<i64> = app
        .visible_epics_for_effective_view()
        .map(|e| e.id.0)
        .collect();
    ids.sort_unstable();
    ids
}

#[test]
fn search_hides_non_matching_epic_cards_on_the_board() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![
        make_epic_with_title(1, "Login redesign"),
        make_epic_with_title(2, "Billing rework"),
    ];
    app.search.query = "login".to_string();
    assert_eq!(visible_epic_ids(&app), vec![1]);
}

#[test]
fn empty_search_keeps_every_epic_card() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![
        make_epic_with_title(1, "Login redesign"),
        make_epic_with_title(2, "Billing rework"),
    ];
    assert_eq!(visible_epic_ids(&app), vec![1, 2]);
}

#[test]
fn search_keeps_epic_whose_subtask_matches() {
    let mut app = App::new(vec![epic_child(10, 2, "Fix login bug")]);
    app.board.epics = vec![
        make_epic_with_title(1, "Docs cleanup"),
        make_epic_with_title(2, "Billing rework"),
    ];
    app.search.query = "login".to_string();
    // Epic 2 has no own match but owns the matching task; epic 1 has neither.
    assert_eq!(visible_epic_ids(&app), vec![2]);
}

#[test]
fn search_on_epics_composes_with_repo_filter() {
    let mut app = App::new(vec![]);
    let mut child = epic_child(10, 1, "Fix login bug");
    child.repo_path = "/repo/b".to_string();
    app.board.tasks = vec![child];
    app.board.epics = vec![make_epic_with_title(1, "Login redesign")];
    app.filter.repos.insert("/repo/a".to_string());
    app.filter.mode = RepoFilterMode::Include;
    app.search.query = "login".to_string();
    // Title matches, but the repo filter excludes the epic's only subtask.
    assert_eq!(visible_epic_ids(&app), Vec::<i64>::new());
}

#[test]
fn search_on_epics_composes_with_only_active_filter() {
    let mut idle = epic_child(10, 1, "Fix login bug");
    idle.tmux_window = None;
    let mut app = App::new(vec![idle]);
    app.board.epics = vec![make_epic_with_title(1, "Login redesign")];
    app.filter.only_active = true;
    app.search.query = "login".to_string();
    // Title matches, but no subtask has a live tmux window.
    assert_eq!(visible_epic_ids(&app), Vec::<i64>::new());
}

#[test]
fn search_narrows_sub_epic_cards_inside_an_epic_view() {
    use crate::tui::messages::EpicMessage;
    use crate::tui::types::Message;

    let mut app = App::new(vec![]);
    let mut a = make_epic_with_title(2, "Login redesign");
    a.parent_epic_id = Some(EpicId(1));
    let mut b = make_epic_with_title(3, "Billing rework");
    b.parent_epic_id = Some(EpicId(1));
    app.board.epics = vec![make_epic_with_title(1, "Root"), a, b];
    app.update(Message::Epic(EpicMessage::Enter(EpicId(1))));
    app.search.query = "login".to_string();
    assert_eq!(visible_epic_ids(&app), vec![2]);
}

#[test]
fn search_does_not_narrow_the_move_task_epic_picker() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![
        make_epic_with_title(1, "Login redesign"),
        make_epic_with_title(2, "Billing rework"),
    ];
    app.search.query = "login".to_string();
    let mut ids: Vec<i64> = app.move_task_target_epics().iter().map(|e| e.id.0).collect();
    ids.sort_unstable();
    assert_eq!(ids, vec![1, 2], "the picker has its own buffer");
}

#[test]
fn search_does_not_narrow_the_reparent_epic_picker() {
    let mut app = App::new(vec![]);
    app.board.epics = vec![
        make_epic_with_title(1, "Login redesign"),
        make_epic_with_title(2, "Billing rework"),
        make_epic_with_title(3, "Docs cleanup"),
    ];
    app.search.query = "login".to_string();
    // Reparent targets for epic 3: everything except itself, unfiltered by search.
    let mut ids: Vec<i64> = app
        .reparent_target_epics(EpicId(3))
        .iter()
        .map(|e| e.id.0)
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, vec![1, 2], "the picker has its own buffer");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test tui::tests::search`
Expected: FAIL. `search_hides_non_matching_epic_cards_on_the_board` reports `[1, 2]` where `[1]` was expected; the repo/only-active and sub-epic cases fail the same way. The two picker tests should already PASS (they guard against over-reach).

If `visible_epic_ids` does not compile because `visible_epics_for_effective_view` is not reachable, check its visibility — it is `pub(in crate::tui)` and the test module lives under `crate::tui`, so no visibility change should be needed.

- [ ] **Step 3: Add the filter**

In `src/tui/mod.rs`, in `visible_epics_for_effective_view()`, change:

```rust
            .filter(|e| self.epic_matches(e.id) && self.epic_repo_matches(e.id))
```

to:

```rust
            .filter(|e| {
                self.epic_matches(e.id)
                    && self.epic_repo_matches(e.id)
                    && self.epic_search_matches(e.id)
            })
```

Also extend that function's doc comment: after "filtered by the active repo / only-active filters", add "and the board-search query". Leave `reparent_target_epics` and `move_task_target_epics` untouched.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test tui::tests::search`
Expected: PASS.

- [ ] **Step 5: Run the full suite**

Run: `cargo test`
Expected: PASS. If a snapshot test breaks here, that is a real regression — no existing snapshot sets a search query, so nothing should shift. Investigate rather than accepting the new snapshot.

- [ ] **Step 6: Clippy and format**

Run: `cargo fmt` then `cargo clippy --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add src/tui/mod.rs src/tui/tests/search.rs
git commit -m "feat(search): narrow epic cards by the board search query"
```

---

### Task 4: End-to-end key-sequence and render coverage

Locks the behaviour at the two outer surfaces: real keystrokes, and the rendered board.

**Files:**
- Test: `src/tui/tests/scenarios.rs`
- Test: `src/tui/tests/snapshots.rs`
- Create: `src/tui/tests/snapshots/*.snap` (generated by `cargo insta`)

**Interfaces:**
- Consumes: the wired behaviour from Task 3; `Scenario::with_app`, `.key()`, `.char_keys()` (see `search_narrows_board_to_matching_titles` at `src/tui/tests/scenarios.rs:306`); `render_to_string(&mut app, 120, 40)` and `make_epic_with_title` (see `snapshot_group_indicator_on_non_feed_epic` at `src/tui/tests/snapshots.rs:386`).
- Produces: nothing consumed by later tasks.

- [ ] **Step 1: Write the failing scenario test**

Append to `src/tui/tests/scenarios.rs`:

```rust
#[test]
fn search_narrows_epic_cards_and_esc_restores_them() {
    use super::{make_epic_with_title, App};
    use crossterm::event::KeyCode;

    fn visible_epic_ids(app: &App) -> Vec<i64> {
        let mut ids: Vec<i64> = app
            .visible_epics_for_effective_view()
            .map(|e| e.id.0)
            .collect();
        ids.sort_unstable();
        ids
    }

    let mut app = App::new(vec![]);
    app.board.epics = vec![
        make_epic_with_title(1, "Login redesign"),
        make_epic_with_title(2, "Billing rework"),
    ];

    let mut s = Scenario::with_app(app);
    s.key(KeyCode::Char('/')).char_keys("login");
    assert_eq!(
        visible_epic_ids(&s.app),
        vec![1],
        "board narrows to matching epics while typing"
    );

    // Enter closes the bar but keeps the query active.
    s.key(KeyCode::Enter);
    assert_eq!(visible_epic_ids(&s.app), vec![1], "query persists on Enter");

    // Esc on the board clears the query.
    s.key(KeyCode::Esc);
    assert_eq!(
        visible_epic_ids(&s.app),
        vec![1, 2],
        "clearing the query restores every epic card"
    );
}
```

- [ ] **Step 2: Run it to verify it passes**

Run: `cargo test tui::tests::scenarios::search_narrows_epic_cards_and_esc_restores_them`
Expected: PASS — Task 3 already implemented the behaviour; this test exists to lock the key path. If it fails on the `Esc` step, read `handle_key_search` / the normal-mode `Esc` handler in `src/tui/input/normal.rs` and fix the assertion to match the specified lifecycle in `board_search_filter` (Esc on the board with an active query clears it) rather than weakening the test.

- [ ] **Step 3: Write the snapshot test**

Append to `src/tui/tests/snapshots.rs`:

```rust
#[test]
fn snapshot_board_search_narrows_epic_cards() {
    use super::make_epic_with_title;
    let mut app = App::new(vec![]);
    app.board.epics = vec![
        make_epic_with_title(1, "Login redesign"),
        make_epic_with_title(2, "Billing rework"),
    ];
    app.search.query = "login".to_string();
    let rendered = render_to_string(&mut app, 120, 40);
    insta::assert_snapshot!(rendered);
}
```

- [ ] **Step 4: Generate and inspect the snapshot**

Run: `cargo test tui::tests::snapshots::snapshot_board_search_narrows_epic_cards`
Expected: FAIL — new snapshot, insta writes a `.snap.new`.

Read the pending snapshot and confirm by eye: the backlog column shows `#1 Login redesign` and **not** `#2 Billing rework`, and the `[/login]` indicator is present. Then accept:

Run: `INSTA_UPDATE=always cargo test tui::tests::snapshots::snapshot_board_search_narrows_epic_cards`
Then: `rm -f src/tui/tests/snapshots/*.snap.new`

- [ ] **Step 5: Run the verify command**

Run: `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`
Expected: all pass. If `cargo fmt --check` fails, run `cargo fmt`, then diff-check that it did not reformat files unrelated to this change before staging.

- [ ] **Step 6: Commit**

```bash
git add src/tui/tests/scenarios.rs src/tui/tests/snapshots.rs src/tui/tests/snapshots/
git commit -m "test(search): key-sequence and render coverage for epic search"
```

---

### Task 5: Spec-code alignment check

**Files:**
- Possibly modify: `docs/specs/core.allium`

**Interfaces:**
- Consumes: everything above.
- Produces: a verified-aligned spec.

- [ ] **Step 1: Run the weeder**

Invoke the `allium:weed` skill scoped to `docs/specs/core.allium` and the search surface (`src/tui/mod.rs`, `src/tui/tests/search.rs`).
Expected: no divergence between `board_search_filter` and the implementation.

- [ ] **Step 2: Resolve any divergence**

If the weeder reports drift, fix it in the direction the design doc specifies (`docs/superpowers/specs/2026-08-03-search-on-epics-design.md` is authoritative on intent). Do not silently change behaviour to match a stale spec sentence.

- [ ] **Step 3: Commit any spec touch-ups**

```bash
git add docs/specs/core.allium
git commit -m "spec(search): align board_search_filter with implementation"
```

(Skip this step if the weeder found nothing.)

---

## Notes for the implementer

- **Why one filter site is enough.** `visible_epics_for_effective_view()` is the sole source of epic cards for the board: `column_items_for_status_with_view_tasks` (`src/tui/mod.rs:1230`), `column_item_count` (`src/tui/mod.rs:1351`) and `column_items_for_visual_column` (`src/tui/mod.rs:1371`) all go through it. Do not add the predicate at those three call sites.
- **Cursor behaviour is already covered.** An anchored cursor on an epic that the query filters away re-clamps through the existing `selection_preservation` machinery. No new code, no new test.
- **Flattened mode renders no epics at all** (`renders_epics: not active` in `core.allium`), so there is nothing to filter there — no flattened-mode test is needed.
- **Do not touch `tasks_for_current_view()`.** Task filtering is already correct and stays strict inside epic views by design.
