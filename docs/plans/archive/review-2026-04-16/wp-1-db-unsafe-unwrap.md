# DB Queries: Fix unsafe unwrap()

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace 5 panic-on-poisoned-mutex calls with the safe `conn()` accessor.

## Context

This work package addresses findings from a code review.

## Findings

### :warning: Unsafe `conn.lock().unwrap()` in settings methods (`src/db/queries.rs:276,290,300,311,321`)

**Issue:** Five methods in the settings-store section (`get_setting_bool`, `set_setting_bool`, `get_setting_string`, `set_setting_string`, `seed_github_query_defaults`) call `self.conn.lock().unwrap()` directly. Every other method in the file uses `self.conn()` (defined at `src/db/mod.rs:416`), which converts a poisoned mutex into an `anyhow::Error` instead of panicking. A poisoned mutex is rare but possible if a thread panics while holding the lock — these 5 call sites would crash the entire process.

**Fix:** Replace each `self.conn.lock().unwrap()` with `self.conn()?`. The return types already return `Result`, so the `?` propagation works without signature changes.

## Changes

| File | Change |
|------|--------|
| `src/db/queries.rs:276` | `self.conn.lock().unwrap()` → `self.conn()?` in `get_setting_bool` |
| `src/db/queries.rs:290` | `self.conn.lock().unwrap()` → `self.conn()?` in `set_setting_bool` |
| `src/db/queries.rs:300` | `self.conn.lock().unwrap()` → `self.conn()?` in `get_setting_string` |
| `src/db/queries.rs:311` | `self.conn.lock().unwrap()` → `self.conn()?` in `set_setting_string` |
| `src/db/queries.rs:321` | `self.conn.lock().unwrap()` → `self.conn()?` in `seed_github_query_defaults` |

## Verification

- [ ] Run existing tests — `cargo test db::tests` — all pass
- [ ] `grep "conn.lock().unwrap()" src/db/queries.rs` returns no matches
- [ ] `cargo clippy -- -D warnings` passes
