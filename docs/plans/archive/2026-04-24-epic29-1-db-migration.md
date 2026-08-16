# Task: DB Migration + Data Model

## Context

First phase of the "programmable epics" refactor. Adds the columns and Rust types
needed before any other phase can proceed.

## Changes

### DB migration (src/db/migrations.rs)

Add a new migration `migrate_vN_feed_epic_columns`:

```sql
ALTER TABLE epics ADD COLUMN feed_command TEXT;
ALTER TABLE epics ADD COLUMN feed_interval_secs INTEGER;
ALTER TABLE tasks ADD COLUMN external_id TEXT;
CREATE UNIQUE INDEX IF NOT EXISTS tasks_epic_external_id
    ON tasks (epic_id, external_id)
    WHERE external_id IS NOT NULL;
```

Bump `MIGRATIONS` array entry to `N` and update the schema version assertion in
`fresh_db_has_latest_schema_version`.

### Rust model updates (src/models.rs)

```rust
pub struct Epic {
    // existing fields ...
    pub feed_command: Option<String>,
    pub feed_interval_secs: Option<i64>,
}

pub struct Task {
    // existing fields ...
    pub external_id: Option<String>,
}
```

### DB layer (src/db/mod.rs, src/db/queries.rs)

- Add `feed_command` and `feed_interval_secs` to `EpicPatch` (double-Option pattern)
- Add `external_id` to `TaskPatch`
- Read new columns in `row_to_epic()` and `row_to_task()` helpers
- Add `upsert_feed_tasks(epic_id: EpicId, items: &[FeedItem]) -> Result<()>` to
  `TaskStore` trait and implement it:
  - For each item: INSERT or UPDATE matching `(epic_id, external_id)`
  - On insert: set status from item; on conflict: update title and description only
    (preserve user-managed status, sub_status, worktree, tmux_window, pr_url)
  - `FeedItem` struct lives in `src/feed.rs` (or `src/models.rs`)

### Service layer (src/service.rs)

Expose `feed_command` and `feed_interval_secs` in `UpdateEpicParams` / `CreateEpicParams`
using `Option<FieldUpdate>` for the command (clearable) and `Option<i64>` for the interval.

## TDD Checklist

- [ ] Write migration test: create DB at version N-1, apply migration, assert columns exist
- [ ] Write `upsert_feed_tasks` test: first call creates tasks with correct status
- [ ] Write upsert idempotency test: second call with same items doesn't change status
- [ ] Write upsert status-preservation test: update item status in feed, re-run — task
      status unchanged; title/description updated
- [ ] Write upsert new-item test: adding a new item in subsequent call creates it
- [ ] Update `fresh_db_has_latest_schema_version` assertion
- Then implement the minimum code to pass each test

## Files

- `src/db/migrations.rs` — new migration function + MIGRATIONS entry
- `src/db/mod.rs` — `EpicPatch`, `TaskPatch`, `upsert_feed_tasks` trait method
- `src/db/queries.rs` — row helpers, `upsert_feed_tasks` impl
- `src/db/tests.rs` — migration and upsert tests
- `src/models.rs` — `Epic`, `Task` struct fields; `FeedItem` struct
- `src/service.rs` — `UpdateEpicParams`, `CreateEpicParams`
