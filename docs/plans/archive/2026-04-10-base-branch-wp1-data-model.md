# WP1: Base Branch — Data Model & Migration

## Context

First work package for per-task base branch support. Adds the `base_branch` column to the database and updates the Rust data model. No behavior changes — the field exists but isn't consumed yet. All existing tasks get `'main'` as the default.

## Files to Modify

| File | Change |
|------|--------|
| `src/models.rs` | Add `base_branch: String` to `Task` struct |
| `src/db/migrations.rs` | Migration v32: add `base_branch` column |
| `src/db/mod.rs` | Add `base_branch` to `TASK_COLUMNS`, `TaskPatch` |
| `src/db/queries.rs` | Update `create_task()`, `row_to_task()`, `patch_task()` |
| `src/db/tests.rs` | Migration test, schema version bump |

## Steps

### 1. Migration (test first)

**Test:** In `src/db/tests.rs`, add `test_migrate_v32_base_branch`:
- Create DB at v31 schema
- Insert a task
- Run migration v32
- Verify `base_branch` column exists and existing task has value `'main'`
- Update `fresh_db_has_latest_schema_version` to expect v32

**Implement:** In `src/db/migrations.rs`:
```rust
fn migrate_v32_add_base_branch(conn: &Connection) -> Result<()> {
    conn.execute_batch("ALTER TABLE tasks ADD COLUMN base_branch TEXT NOT NULL DEFAULT 'main';")
}
```
Register in `MIGRATIONS` array.

### 2. Model update

**`src/models.rs`:** Add `pub base_branch: String` to `Task` struct.

### 3. Query updates

**`src/db/mod.rs`:**
- Add `base_branch` to `TASK_COLUMNS`
- Add `pub base_branch: Option<String>` to `TaskPatch`

**`src/db/queries.rs`:**
- `row_to_task()`: Read `base_branch` from row
- `create_task()`: Accept `base_branch: &str` parameter, include in INSERT
- `patch_task()`: Handle `base_branch` field in patch application

### 4. Fix compilation

Update all call sites of `create_task()` to pass a `base_branch` argument. These are likely in:
- `src/mcp/handlers/tasks.rs` (MCP create handler)
- `src/tui/input.rs` or `src/tui/mod.rs` (TUI creation)
- Test files

For now, pass `"main"` as the default at all call sites. WP2 and WP3 will make these dynamic.

## Verification

1. `cargo test` — all tests pass including new migration test
2. `cargo clippy -- -D warnings` — no warnings
3. `cargo fmt --check` — formatted
