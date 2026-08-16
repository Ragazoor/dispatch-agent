# Fix non-atomic migration + user_version bump race (#3724)

## Problem

`init_schema_sync` (`src/db/mod.rs:919-963`) reads `user_version` once, then for
each pending migration runs `migrate_fn(conn)?` followed by a *separate*
`conn.pragma_update(None, "user_version", version)` call. The two are not in a
shared transaction and there's no cross-process lock around the pair. Two
processes opening the DB concurrently just after a binary upgrade (e.g. the
long-lived TUI process and a hook-spawned one-shot CLI process) can both read
the old `user_version`, and both attempt to apply the same migration.

Most migration bodies are idempotent (`CREATE TABLE IF NOT EXISTS`, guarded
`ALTER TABLE` via `column_exists`/`table_exists`), so this is largely
survivable today. But `migrate_v39_add_projects` is a concrete
counter-example: it does an unconditional
`INSERT INTO projects (name, sort_order, is_default) VALUES ('Default', 0, 1)`
with no existence guard. If two processes both observe `user_version < 39`
they will both run it, producing two `Default` project rows.

## Fix direction

Wrap each migration's DDL and its `user_version` bump in one `BEGIN
IMMEDIATE` transaction, and re-check the version *inside* that transaction
before running the migration body. `BEGIN IMMEDIATE` acquires SQLite's
reserved write lock up front, so a second process that reaches the same
migration blocks (per `busy_timeout`, already 5000ms) until the first
transaction commits — then sees the bumped version inside its own
transaction and skips the migration entirely. No new cross-process lock file
is needed; SQLite's existing single-writer file locking provides it once the
transaction boundary is correct.

### The `PRAGMA foreign_keys` complication

7 of the ~85 migration bodies (`migrate_v4_add_needs_input_drop_epic_plan`,
`migrate_v16_add_status_check_constraint`,
`migrate_v17_add_conflict_sub_status`, `migrate_v20_epic_status_enum`,
`migrate_v30_allow_conflict_for_review`, `migrate_v35_add_self_ref_check`,
`migrate_v39_add_projects`) already wrap their own DDL in an explicit
`PRAGMA foreign_keys = OFF; BEGIN; ... COMMIT; PRAGMA foreign_keys = ON;`
(the FK toggle is needed because SQLite can't `DROP COLUMN`, so these rebuild
the table wholesale and need FK checks off across the `DROP TABLE` /
`RENAME TO` swap).

Two SQLite rules interact here:
- `PRAGMA foreign_keys` is a no-op if executed while a transaction is already
  open.
- `BEGIN` while already inside a transaction is an error ("cannot start a
  transaction within a transaction").

So we cannot just wrap the *whole* `migrate_fn(conn)` call in an outer
transaction started before invoking it — the inner `BEGIN` would error, and
even if it didn't, the FK toggle would silently no-op.

Fix: hoist the `PRAGMA foreign_keys = OFF` / `= ON` toggle and the
`BEGIN`/`COMMIT` pair out of each of those 7 migration bodies and into the
shared loop in `init_schema_sync`, unconditionally around every migration
(harmless for migrations that don't touch FKs — it's toggled back on before
the next migration or before any subsequent DB use). The 7 bodies keep their
DDL as-is, just without the wrapper statements.

`migrate_v39_add_projects` also has a plain `BEGIN;...COMMIT;` (no FK
toggle) for its own atomicity — same treatment, strip the inner wrapper.

### Resulting structure

```rust
fn apply_pending_migrations(conn: &mut Connection, migrations: &[Migration]) -> Result<()> {
    let current_version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;

    for &(version, migrate_fn) in migrations {
        if current_version >= version {
            continue; // fast path — already applied before we even started
        }

        conn.execute_batch("PRAGMA foreign_keys = OFF")?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let version_in_tx: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version_in_tx < version {
            migrate_fn(&tx)?;
            tx.pragma_update(None, "user_version", version)?;
        }
        tx.commit()?;
        conn.execute_batch("PRAGMA foreign_keys = ON")?;
    }
    Ok(())
}
```

`init_schema_sync` keeps its existing PRAGMA setup and base-table creation,
then delegates to `apply_pending_migrations(conn, migrations::MIGRATIONS)`.
`Transaction<'_>` derefs to `Connection`, so `migrate_fn(&tx)` type-checks
unchanged (`Migration = fn(&Connection) -> Result<()>`).

Extracting this into its own function (rather than inlining in
`init_schema_sync`) lets the test suite exercise the transaction/re-check
mechanism directly with a synthetic, deliberately non-idempotent migration —
without needing to fabricate a realistic pre-migration schema for a real
`MIGRATIONS` entry.

## Signature change ripple

`init_schema_sync` moves from `&Connection` to `&mut Connection` (needed for
`conn.transaction_with_behavior`). 21 existing tests in
`src/db/tests/migrations.rs` call `super::super::init_schema_sync(&conn)`
directly against a hand-built `rusqlite::Connection` — each of those bindings
becomes `let mut conn = ...` and the call becomes `&mut conn`. Mechanical,
no behavior change for those tests.

## Test plan (TDD)

1. **New test first** (fails against current code, passes after the fix):
   `concurrent_open_applies_pending_migration_exactly_once` in
   `src/db/tests/migrations.rs`. Builds a real file-backed DB (via
   `tempfile::NamedTempFile`), opens two independent `rusqlite::Connection`s
   against the same file on two OS threads, and races
   `apply_pending_migrations` on both with a single synthetic migration
   whose body unconditionally inserts a row into a marker table (mirrors
   `migrate_v39`'s unguarded INSERT). Asserts exactly one row exists
   afterward. This is deterministic, not timing-flaky: the pre-fix code's
   race is structural (both threads decide "pending" from an un-rechecked
   snapshot), not a narrow timing window.
2. Existing 21 `init_schema_sync` tests continue to pass after the
   `&mut conn` mechanical update — they cover that the full migration chain
   still produces the right schema/data.
3. Full `cargo test` plus `./scripts/check-doc-paths.sh`.

## Out of scope

- No change to migration numbering, `MIGRATIONS` ordering, or any migration's
  actual DDL/data effects beyond removing the now-redundant inner
  `BEGIN`/`COMMIT`/FK-toggle wrapper.
- No new advisory/file lock — SQLite's own write-lock (`BEGIN IMMEDIATE` +
  existing `busy_timeout=5000`) is sufficient once the transaction boundary
  is correct.
- Not touching `docs/specs/` — this is an internal correctness fix with no
  observable domain-behavior change (migrations still apply the same schema,
  just safely under concurrency).
