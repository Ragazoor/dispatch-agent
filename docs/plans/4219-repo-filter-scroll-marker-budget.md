# 4219 — Repo-filter overlay clips its help footer while the repo list is scrolling

## The defect

`repo_filter_layout` (`src/tui/ui/kanban/popups/repo_filter.rs::repo_filter_layout`)
budgets the popup's content rows as `non_repo_rows + visible_repos`, but
`append_repo_list` draws up to two *additional* rows the budget never reserved:
the `↑ N more` and `↓ N more` scroll markers. With 40 repos on a 20-row board the
popup content height is 14, chrome takes 5, so 9 repos are drawn plus both
markers = 16 rows in a 14-row box. The overflow falls off the bottom, taking the
two-row help footer (and the bottom border) with it.

## Why it isn't a one-liner

The formulation is circular: whether a marker row appears depends on the scroll
offset, which depends on how many repos are visible, which depends on how many
marker rows were reserved.

## The resolution

Settle the window by **fixed point over the reservation**, smallest first.

For each `reserved` in `0..=2`:

- `visible = budget.saturating_sub(reserved).max(1)`
- `scroll  = scroll_offset(repo_cursor, repo_count, visible)`
- `markers = (scroll > 0) + (repo_count > scroll + visible)`

Take the first `reserved` where `markers <= reserved` — the reservation covers
the markers the resulting window actually draws, so `visible + markers <= budget`
holds. `reserved = 2` always satisfies the test (there are only two markers), so
the search always terminates; it is the fallback when nothing smaller settles.

This is strictly tighter than "always reserve both markers whenever the list
scrolls": at the top or bottom of a long list only one marker is drawn, and the
fixed point notices, keeping one more repo on screen instead of leaving a blank
row.

The marker flags are computed once in the layout and returned on
`RepoFilterLayout`, so the renderer draws exactly what the budget reserved
rather than re-deriving the condition. On degenerate boards where the
one-visible-row floor already fills (or exceeds) the budget, both markers are
suppressed — the repo row is the more useful of the two.

## Steps (TDD — test first in every step)

1. **Unit test, both markers.** In the module's `tests`, assert that for a
   scrolling window the settled `visible_repos` plus the markers the layout
   reports never exceeds the row budget, swept over cursor positions (so the
   top / middle / bottom marker combinations are all covered). Fails today.
2. **Unit test, existing expectation updated.**
   `window_scrolls_to_keep_the_repo_cursor_visible` currently asserts 9 visible
   repos for a budget of 9 — which is exactly the over-budget number. With the
   cursor at the end only the `↑` marker draws, so the settled answer is 8
   visible with `scroll = 32`. Update the assertion and its comment.
3. **Render test.** Extend
   `repo_filter_renders_its_whole_footer_in_every_mode` in
   `src/tui/tests/rendering.rs` to sweep a scrolling board (40 repos, 20 rows)
   at cursor positions top / middle / bottom, keeping the existing
   non-scrolling case. The footer text and the bottom-left corner assertions
   already there are the oracle.
4. **Implement** `settle_repo_window` and thread `show_scroll_up` /
   `show_scroll_down` through `RepoFilterLayout` into `append_repo_list`.
5. **Spec.** Add a `repo_filter_overlay_layout` prose section to
   `docs/specs/core.allium` next to the other `[f]`-overlay notes, stating the
   invariant (the footer is never clipped; markers are part of the row budget)
   and the settle rule.
6. **Verify.** `cargo test`, `cargo fmt`, `cargo clippy --all-targets -D warnings`.
