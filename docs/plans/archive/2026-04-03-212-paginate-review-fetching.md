# Plan: Paginate review PR fetching

## Context

`fetch_review_prs()` in `src/github.rs` makes a single GraphQL request with 3 aliased searches, each requesting `first: 100` PR nodes with heavy fields (reviews, comments, labels, commits). This produces a large response payload that is slow to return from GitHub's API. The TUI already loads cached PRs from SQLite on startup, so the user sees stale data instantly — but the background refresh takes noticeably long.

The goal is to reduce per-request payload by using smaller pages, following the existing pagination pattern from `fetch_security_alerts()`.

## Approach

Reduce `first` from 100 to 25 and paginate up to 3 pages per fetch cycle (matching `fetch_security_alerts`'s `MAX_PAGES`). Each alias (`requestedReview`, `alreadyReviewed`, `commented`) gets its own cursor. On subsequent pages, only aliases that still have `hasNextPage: true` are included in the query.

**Page size: 25, Max pages: 3** → up to 75 PRs per alias (225 total before dedup). Most users will complete in 1 page (<25 PRs per alias). The function signature and all downstream types remain unchanged.

## Changes

### 1. Add pagination loop to `fetch_review_prs()` — `src/github.rs`

Replace the single query construction and execution with a loop (up to `MAX_PAGES`):

```
const REVIEW_PAGE_SIZE: usize = 25;
const REVIEW_MAX_PAGES: usize = 3;
```

Track per-alias state:
- `has_next: bool` (initially `true`)
- `cursor: Option<String>` (initially `None`)

Each iteration:
1. Build query including only aliases where `has_next` is true, with optional `after: "<cursor>"`
2. Execute via `runner.run("gh", ...)`
3. Extract nodes (dedup by URL into shared `HashSet`) and `pageInfo` per alias
4. Update cursors; break if no alias has `hasNextPage`

### 2. Add `pageInfo` to query aliases — `src/github.rs`

Each alias block gains:
```graphql
pageInfo { hasNextPage endCursor }
```

Extract a helper to build alias fragments:
```rust
fn build_search_alias(name: &str, query: &str, page_size: usize, cursor: &Option<String>) -> String
```

The 3 search queries (constants or inline):
- `requestedReview`: `"is:pr is:open review-requested:@me -is:draft -author:app/dependabot -author:app/renovate archived:false"`
- `alreadyReviewed`: `"is:pr is:open reviewed-by:@me -author:@me -is:draft -author:app/dependabot -author:app/renovate archived:false"`
- `commented`: `"is:pr is:open commenter:@me -author:@me -is:draft -author:app/dependabot -author:app/renovate archived:false"`

### 3. Refactor `parse_review_prs` into composable parts — `src/github.rs`

Split into:
- **`extract_page_nodes(json, seen_urls) → (viewer_login, Vec<Value>, [AliasPageInfo; 3])`** — extracts unique nodes and pagination info from one page response
- **`build_review_prs(viewer_login, nodes) → Vec<ReviewPr>`** — constructs `ReviewPr` structs from raw nodes (the existing per-node logic)

Keep `parse_review_prs` as a thin wrapper calling both, so existing tests pass unchanged.

### 4. Apply same pattern to `fetch_my_prs()` — `src/github.rs`

Same issue: `first: 100` with heavy fields. Apply the same pagination (single alias, simpler — just one cursor to track).

### 5. Add tests — `src/github.rs` (tests module)

- **Multi-page fetch**: `MockProcessRunner` queued with 2 responses — page 1 has `hasNextPage: true`, page 2 has `hasNextPage: false`
- **Mixed exhaustion**: One alias done after page 1, others continue to page 2
- **Cross-page dedup**: PR appears in `requestedReview` page 1 and `commented` page 2
- **Single page (existing behavior)**: Existing `SAMPLE_RESPONSE` test continues to pass

## Files to modify

| File | Change |
|------|--------|
| `src/github.rs` | Pagination loop, query builder, parse refactor |

## What does NOT change

- `Message::ReviewPrsLoaded` / `Command::FetchReviewPrs` — same types
- `ReviewBoardState` — no new fields
- `src/runtime.rs` — `exec_fetch_review_prs` calls the same function signature
- `src/db/queries.rs` — `save_review_prs` / `load_review_prs` unchanged
- 30-second refresh interval unchanged
- Startup cache-then-fetch flow unchanged

## Verification

1. `cargo test` — all existing tests pass, new pagination tests pass
2. `cargo build` — compiles cleanly
3. `cargo clippy -- -D warnings` — no warnings
4. Manual: run `cargo run -- tui`, switch to review board, observe faster initial fetch in logs (`tracing::info` already logs PR count)
