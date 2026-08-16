# Migration Tests

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add direct tests for database migration functions to prevent data loss on schema upgrades.

## Context

This work package addresses findings from a code review. `src/db/migrations.rs` contains 25 migration functions (516 lines) with zero direct tests. Migrations are only exercised indirectly when `Database::open_in_memory()` runs the full migration chain. Individual migration correctness — especially data-preserving migrations — is unverified.

## Findings

### :bulb: No direct migration tests (`src/db/migrations.rs`)

**Issue:** 25 migration functions completely untested in isolation. Schema upgrades, column additions, and table renames all have zero direct coverage. The only related test is `fresh_db_has_latest_schema_version` which checks the final version number but not individual migration correctness.

**Fix:** For each migration (or at least the non-trivial ones), write a test that:
1. Creates a DB at the pre-migration schema version
2. Inserts representative test data
3. Runs the single migration function
4. Verifies the schema changed correctly AND data was preserved

Focus on migrations that modify existing data (ALTER TABLE, data copies, column renames) rather than simple ADD COLUMN migrations. Follow the pattern documented in CLAUDE.md's "Adding a Database Migration" guide.

## Changes

| File | Change |
|------|--------|
| `src/db/tests.rs` | Add migration-specific tests for non-trivial migrations |
| `src/db/migrations.rs` | May need to make individual migration functions `pub(crate)` if currently private |

## Verification

- [ ] Run existing tests — all pass
- [ ] Each non-trivial migration has a dedicated test
- [ ] Tests verify both schema changes and data preservation
- [ ] `cargo clippy -- -D warnings` passes
