# Feed Pipeline Untangling

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Separate routing decisions from side effects in the feed ingest pipeline, extract the per-item dispatch job from the poll loop, and remove duplicated error-logging idioms.

## Context

This work package addresses findings from a code review in `src/feed/ingest.rs` and `src/feed/mod.rs`.
The feed system is spec'd in `docs/specs/feeds.allium` — consult it before changing the
feed-as-source-of-truth invariant, and update via `allium:tend` if behaviour moves.

## Findings

### 💡 `route_and_group_entries` tangles decision with side effect (`src/feed/ingest.rs:434-501`)

**Issue:** One loop nests `can_auto_group` → cache lookup → `match` on epic creation with
error-fallback → `if let Some(task)` → conditional `set_task_epic_id` + chained `patch_task`, ~4
levels deep. The routing *decision* and the in-place move *side effect* are tangled, so neither can
change independently.

**Fix:** Split into a pure decision phase (compute a `Vec` of routing actions) and an apply phase
(perform the DB moves). The decision phase becomes unit-testable without a DB.

### 💡 `sync_grouped_feed` god function (`src/feed/ingest.rs:94-198`)

**Issue:** ~100 lines doing five things (group-by-repo, active-sub-epic filter, find-or-create epic,
present-group upsert, absent reconcile, parent flat-clear). Ordering dependencies are documented only
in prose comments — fragile to reorder.

**Fix:** Extract the five phases into named helpers with explicit inputs/outputs so the ordering
contract is expressed in types/signatures rather than prose. Add tests pinning the ordering-sensitive
behaviour.

### 💡 `FeedRunner::tick` mixes concerns (`src/feed/mod.rs:109-253`)

**Issue:** ~145 lines mixing scheduling, capability-caching, and per-item dispatch; manually clones 8
fields into a spawned closure.

**Fix:** Extract a `FeedJob` struct capturing the per-item dispatch context (the 8 cloned fields), so
`tick` builds `FeedJob`s and spawns them. Separates scheduling from dispatch.

### 💡 Duplicated warn-on-error idiom + `unzip` parallel-Vec undo (`src/feed/ingest.rs`)

**Issue:** ~9 near-identical `if let Err { tracing::warn!(...) }` blocks (`:24,108,138,184,462,514,541,551,575`).
Separately, `FeedItemWithTarget::unzip` (`:71`) is torn back into 3 parallel Vecs at 3 call sites —
the abstraction meant to kill the parallel-slice footgun is undone at the DB boundary.

**Fix:** Extract a `warn_on_err(result, context)` helper. Push the `unzip` boundary down so callers
consume the structured type instead of re-splitting into parallel Vecs (or change the DB call to
accept the structured type).

## Changes

| File | Change |
|------|--------|
| `src/feed/ingest.rs` | Split `route_and_group_entries` into decision + apply; extract `sync_grouped_feed` phases; add `warn_on_err` helper; fix `unzip` parallel-Vec undo. |
| `src/feed/mod.rs` | Extract `FeedJob` from `FeedRunner::tick`. |
| `docs/specs/feeds.allium` | Update via `allium:tend` if behaviour changes; verify with `allium:weed`. |

## Verification

- [ ] TDD: add tests for the pure routing-decision phase and ordering-sensitive sync BEFORE refactoring
- [ ] `cargo test feed` and `cargo test --test feed_sync` / `--test managed_feeds` — all pass
- [ ] `cargo test && ./scripts/check-doc-paths.sh` — all pass
- [ ] Feed-as-source-of-truth invariant unchanged (`allium:weed` clean)
- [ ] `cargo clippy --all-targets -- -D warnings` clean
