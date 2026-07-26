# WP1: Add tests for `src/feed.rs`

## Context

`src/feed.rs` (741 LOC) drives feed-epic polling — the only path that creates tasks from external systems via user-supplied shell scripts. It currently has **zero direct tests**. A regression here is invisible until a real feed is configured and silently breaks.

This is the highest bug-reduction ROI item from the review.

## Findings

- **Severity:** High
- **File:** `src/feed.rs` (741 LOC, 0 tests)
- **Issue:** Feed sync logic — script execution, JSON parsing, `upsert_feed_tasks`, external_id matching — has no unit tests. Coverage relies entirely on manual verification via `verify-feed`.
- **Suggestion:** Add unit tests for the core sync paths.

## Plan (TDD)

Follow the project's TDD discipline: write tests first, then implement only what makes them pass. Most of the implementation already exists — the work is expressing intended behaviour as tests and patching anything the tests expose.

1. **Set up** — add a `#[cfg(test)] mod tests` block in `src/feed.rs` (or `src/feed/tests.rs` if the module needs splitting). Use `Database::open_in_memory()` and `MockProcessRunner` per project convention.
2. **Test: happy-path upsert** — given a feed epic and a JSON array of two `FeedItem`s, `sync_feed_for_epic` (or equivalent) creates two tasks with matching `external_id`s and `SubStatus::Feed`.
3. **Test: idempotent upsert** — running the same feed twice does not duplicate tasks; the second run updates titles/descriptions but keeps task IDs stable (external_id is the upsert key).
4. **Test: removed items don't delete tasks** — if a feed run no longer contains an external_id, the existing task is **not** deleted (per the `feeds.allium` spec).
5. **Test: malformed JSON** — script returns invalid JSON → returns a structured error, no DB writes.
6. **Test: script non-zero exit** — `MockProcessRunner` returns a non-zero exit code → returns an error, no DB writes.
7. **Test: empty array** — script returns `[]` → no error, no new tasks.
8. **Test: missing required `FeedItem` fields** — reject the whole batch, no partial writes.

## Files to change

| File | Change |
|---|---|
| `src/feed.rs` | Add `#[cfg(test)] mod tests` (or split into `src/feed/{mod,tests}.rs`) covering the 7 scenarios above. |
| `docs/specs/feeds.allium` | If any test exposes spec drift, update via `allium:tend` and re-check with `allium:weed`. |

## Verification

```bash
cargo test feed
cargo test --test lifecycle  # smoke
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

Pre-push hook should pass cleanly.
