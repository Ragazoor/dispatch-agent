# Plan: Review/Security Column Repo Grouping

## Context

The review and security board columns currently show a flat list of cards, each displaying a `repo_short` label on its metadata line. When monitoring multiple repos, this makes it hard to visually distinguish which cards belong to which repo. The task board already solves an analogous problem with substatus grouping (visual headers injected between groups). We apply the same pattern here: group by repo, inject non-selectable headers, and remove the now-redundant repo label from each card.

## Approach

Sort items by repo within each column's data accessor, then inject repo group headers during rendering — exactly mirroring the task board's substatus grouping pattern. Navigation continues to operate on data indices; only the visual layer changes.

## Steps

### 1. Sort by repo in data accessors (`src/tui/mod.rs`)

**`active_prs_for_column` (~line 3064):** After `.filter().collect()`, sort the result by `a.repo.cmp(&b.repo)`.

**`security_alerts_for_column` (~line 298):** Same — sort by `a.repo.cmp(&b.repo)` after collect.

This ensures `selected_review_pr()` / `selected_security_alert()` (which call `.nth(row)` on the same accessor) agree with what the renderer shows.

### 2. Inject repo group headers in `render_review_columns` (`src/tui/ui.rs` ~line 2798)

Replace the flat `.map().collect()` with the grouping loop pattern from `render_columns`. This applies uniformly to all three modes (Reviewer, Author, Dependabot) since they share the same render function and `active_prs_for_column` accessor.

- Track `current_repo: Option<&str>` and `list_selection_idx: Option<usize>`
- When `pr.repo` changes, inject `render_substatus_header(repo_short, color)` using `review_column_color(decision_for_color)` for the color
- Map `selected_row` (data index) → `list_selection_idx` (visual index) for `ListState`
- Continue using the local `ListState` + write-back pattern (existing lines 2842-2852), but select `list_selection_idx` instead of `Some(selected_row)`

### 3. Inject repo group headers in `render_security_columns` (`src/tui/ui.rs` ~line 3122)

Same pattern as step 2, using `security_column_color(severity)` for header color.

### 4. Remove repo from review card metadata (`src/tui/ui.rs`, `build_review_pr_item`)

Change line 2927 from:
```rust
let before_age = format!("  {} · @{} · ", repo_short, pr.author);
```
to:
```rust
let before_age = format!("  @{} · ", pr.author);
```
Remove the unused `repo_short` variable (line 2885).

### 5. Remove repo from security card metadata (`src/tui/ui.rs`, `build_security_alert_item`)

Change line 3198 from:
```rust
let before_age = format!("  {repo_short} · [{kind_indicator}] {pkg} {cvss_str} ");
```
to:
```rust
let before_age = format!("  [{kind_indicator}] {pkg} {cvss_str} ");
```
Remove the unused `repo_short` variable (line 3169).

## No changes needed

- **Navigation functions** (`navigate_review_row`, `navigate_security_row`): operate on data indices, unaffected by visual headers
- **Clamp functions** (`clamp_review_selection`, `clamp_security_selection`): count data items, not visual items
- **Selection structs** (`ReviewBoardSelection`, `SecurityBoardSelection`): no structural changes

## Edge cases

- **Single repo in column**: header still shown for consistency
- **Empty column**: loop doesn't execute, no headers injected, `list_state.select(None)` — correct
- **Repo filter active**: grouping works on already-filtered items; single-repo filter = one group with one header

### 6. Add unit tests (`src/tui/tests.rs`)

**`active_prs_for_column_sorts_by_repo`**: Create 3 PRs in the same ReviewDecision column with repos "org/zebra", "org/alpha", "org/middle". Call `active_prs_for_column(col)` and assert order is alpha, middle, zebra.

**`selected_review_pr_agrees_with_sorted_order`**: Load PRs from different repos into the same column. Set `selected_row[col] = 1`. Call `selected_review_pr()` and assert it returns the second PR in sorted-by-repo order (not insertion order).

**`active_prs_for_column_preserves_order_within_same_repo`**: Create 3 PRs from "org/alpha" with numbers 10, 5, 20. After sorting, they should maintain 10, 5, 20 order within the group (stable sort).

**`security_alerts_for_column_sorts_by_repo`**: Same as the first test but for security alerts. Requires a `make_security_alert` helper (model after `make_review_pr`).

**`selected_security_alert_agrees_with_sorted_order`**: Same as the review selection test but for security alerts.

## Verification

1. `cargo build` — compiles without warnings
2. `cargo test` — all existing + new tests pass
3. `cargo clippy -- -D warnings` — no new warnings
4. Manual: open TUI with review/security boards that have PRs/alerts from multiple repos, verify headers appear and cards no longer show repo
5. Manual: verify keyboard navigation (up/down) correctly skips headers

## Files to modify

- `src/tui/mod.rs` — `active_prs_for_column`, `security_alerts_for_column` (add sort)
- `src/tui/ui.rs` — `render_review_columns`, `render_security_columns` (add grouping), `build_review_pr_item`, `build_security_alert_item` (remove repo label)
- `src/tui/tests.rs` — add unit tests for sorted order, selection agreement, and stable sort; add `make_security_alert` helper
