# Dispatch and Feed Decomposition

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Reduce the size and nesting of `provision_worktree` and `FeedRunner::tick` — the two long, deeply-nested functions that sit on safety-critical paths — **without scattering the hazard documentation they carry.**

## Context

This work package addresses findings from the whole-repository codebase review of 2026-08-28 (`docs/plans/2026-08-28-codebase-review.md`, section 4).

| Function | Lines | Cyc (rough) | Nesting |
|---|---:|---:|---:|
| `src/dispatch/worktree.rs::provision_worktree` | 168 | ~31 | 6 |
| `src/feed/mod.rs::tick` | 130 | ~25 | 6 |

Why these two and not the longer functions above them in the ranking: `provision_worktree` sits inside the dispatch seam, which `CLAUDE.md` names as holding **the most safety-critical rule in the system** (`DispatchClaimExclusive`), and `tick` is the feed poll loop that upserts tasks from external commands.

## Read this before touching either function

**These two are long for a good reason, and that reason is not sloppiness.** Both carry dense inline comments explaining non-obvious hazards that were discovered the hard way. A sample from `provision_worktree`:

> The fetch runs unconditionally — even when reusing an existing worktree directory — so `origin/<base>` stays fresh for whatever rebases onto it later. On a fresh worktree an unreachable origin aborts here rather than quietly producing a worktree based on a stale local ref; on the reuse path it is downgraded to a warning.

> `select_start_point` (below) runs on the reuse path too, and that is deliberate rather than wasted work: `reused_rebase_preamble` targets whatever `start_point` reports, so skipping the measurement here would leave that preamble pointing at the wrong ref — the same `git rebase origin/main`-onto-a-local-based-branch history-duplication hazard this branch exists to remove.

A naive extraction that moves the code and leaves the comment behind — or splits one hazard's explanation across two functions — makes this codebase **worse**, not better, even though the line count improves.

**The rule for this work package: every extracted helper takes its explanatory comment with it, promoted to a doc comment on the new function.** If a comment explains the relationship *between* two extracted pieces, it stays at the call site. If you cannot find a seam where the reasoning stays intact, **leave that part alone and say so in the wrap-up.** A partial job here is the correct outcome; forcing a clean line count is not.

Read `docs/specs/dispatch.allium` (`DispatchClaimExclusive`, `FetchPolicy`) and `docs/specs/feeds.allium` before starting.

## Findings

### 💡 `provision_worktree` — 168 lines, cyc~31, depth 6 (`src/dispatch/worktree.rs:325`)

**Issue:** One function does path derivation, reuse detection, fetch-policy selection, the fetch itself, start-point selection, `git worktree add`, and result assembly. Nesting reaches 6.

The good news is that the phases are already sequential and already commented as phases, and some of the work is already extracted (`validate_repo_path`, `slugify`, `build_tmux_window_name`, `select_start_point`, `FetchPolicy`, `reused_rebase_preamble`). The refactor is finishing a job that is well begun, not starting one.

**Fix:** Look for these seams, in order of confidence:

1. **Path/name derivation** — `repo_path`, `slug`, `worktree_name`, `worktree_path`, `tmux_window`. Five lines with no branching that produce a small struct. Extract as `fn worktree_paths(task: &Task) -> Result<WorktreePaths>`. Highest confidence, zero risk, and it gives the later phases a single named input.
2. **Fetch phase** — reuse detection, `FetchPolicy` selection, and the fetch call, returning whether the fetch succeeded. The two long comments quoted above belong on this helper as its doc comment; they explain exactly what its policy argument means.
3. **The `git worktree add` invocation and its retry/error classification** — if there is a nested error-handling block driving the depth-6 measurement, this is where it will be.

Do **not** extract `select_start_point`'s call site away from the fetch phase. The second comment quoted above exists precisely because those two are coupled, and separating them is the hazard it warns about.

### 💡 `FeedRunner::tick` — 130 lines, cyc~25, depth 6 (`src/feed/mod.rs:282`)

**Issue:** The poll loop body: cache invalidation on `EpicChanged`, an early bail when no epic has a feed command, list epics, prune `last_run`, fetch `repo_paths` once, then per-epic cadence checks and spawning.

Unlike `provision_worktree`, the **first half is already flat and idiomatic** — a sequence of guard clauses with early returns:

```rust
if self.any_feed_cmds == Some(false) { return; }
let epics = match self.db.list_epics().await { Ok(e) => e, Err(err) => { tracing::warn!(…); return; } };
…
if !has_feed_cmd { return; }
```

So the depth-6 measurement comes from the **second half** — the per-epic loop. Read from roughly line 320 onward and locate the actual nesting before planning any extraction. Do not restructure the guard-clause prologue; it is already the shape this work package is trying to produce.

**Fix:** Two likely seams in the back half:

1. **The cadence decision** — whether an individual epic is due this tick, given `last_run`, its interval, and the 60s service-boundary minimum. That is a pure predicate over `(&Epic, &HashMap<EpicId, Instant>, now)` and is worth being independently testable. Note the 60s floor is enforced at the service boundary (commit `d8296884`); do not duplicate the check here, call it.
2. **The per-epic spawn** — building and spawning one epic's feed run.

Preserve exactly:

- The **cache-invalidation ordering**: `has_changed()` → `borrow_and_update()` → clear `any_feed_cmds`. The `unwrap_or(true)` on `has_changed()` is a deliberate fail-open (a closed channel means "assume changed"), not a missing error path.
- The **`any_feed_cmds == Some(false)` fast path**, whose comment says it exists to skip *all* DB work. An extraction that moves the `list_epics()` call above this check silently removes the optimisation.
- The **`known_paths` fetch-once-per-tick** behaviour and its empty-vec sentinel on failure, comment included ("so N concurrent spawned tasks don't each hit the DB").
- The `last_run.retain(…)` prune, which is what stops the map growing without bound as epics are deleted.

### Out of scope

`src/db/mod.rs::create_task` (238 lines) and `src/service/api.rs::update_task` (204 lines) are longer than both of these but are **flat field plumbing at nesting depth 0**. They shrink as a side effect of WP2 (Task Fixture Consolidation). Do not touch them here.

## Changes

| File | Change |
|------|--------|
| `src/dispatch/worktree.rs` | Extract path/name derivation into `worktree_paths(&Task) -> Result<WorktreePaths>` |
| `src/dispatch/worktree.rs` | Extract the fetch phase (reuse detection + `FetchPolicy` + fetch), carrying both long hazard comments onto it as doc comments |
| `src/dispatch/worktree.rs` | Extract the `git worktree add` invocation and its error classification if that is where the depth-6 nesting lives |
| `src/feed/mod.rs` | Extract the per-epic cadence predicate, delegating to the existing service-boundary 60s floor rather than re-implementing it |
| `src/feed/mod.rs` | Extract the per-epic spawn; leave the guard-clause prologue untouched |

## Verification

- [ ] `cargo test` — all pass. Behaviour-preserving refactor: any test needing a change means you altered semantics
- [ ] `cargo test --no-fail-fast` specifically — the `tmux_*` targets are relevant to `provision_worktree`, and without `--no-fail-fast` one blocked target hides the six after it
- [ ] Confirm `tmux` is on `PATH` before believing a green run: without it those targets print `skipping: tmux not available on PATH` **and pass**
- [ ] `cargo clippy --all-targets -- -D warnings` — clean
- [ ] `cargo fmt` before committing
- [ ] Re-measure: both functions should be meaningfully shorter and at nesting ≤ 4. If one is not, that is an acceptable outcome — record why in the wrap-up
- [ ] **Comment audit** — diff the removed comment text against the added comment text and confirm nothing was dropped. This is the primary review criterion for this work package, ahead of the line count
- [ ] Confirm the `any_feed_cmds == Some(false)` fast path still short-circuits *before* any DB call (read the reordered code, don't assume)
- [ ] Confirm `select_start_point` still runs on the reuse path — the history-duplication hazard the comment describes
- [ ] Run `allium:weed` over `docs/specs/dispatch.allium` and `docs/specs/feeds.allium` to confirm no guarantee drifted
