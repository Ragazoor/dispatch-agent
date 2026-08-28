# Dead and Test-Only Predicates

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Resolve five public functions that only test code calls — each one either by wiring it into the production reader that currently open-codes the same condition, or by deleting it from code and spec together.

## Context

This work package addresses findings from the whole-repository codebase review of 2026-08-28 (`docs/plans/2026-08-28-codebase-review.md`, section 5.7).

The review found **zero** unreferenced public functions — genuinely good. But five are referenced *only* from test code. That is a more interesting signal than plain dead code, because in this repo an Allium `derived.` clause is a legitimate reason for a function with no production caller. So this is not a deletion sweep. **Each of the five needs a decision, and the decisions differ.**

**Ordering note:** run WP2 (Task Fixture Consolidation) **before** this work package. Both touch `src/models/tasks.rs`, in different regions.

**Method:** this work package is a good fit for the `allium:weed` skill — it exists precisely to find where spec and code have diverged. Run it over `docs/specs/repo-sync.allium` and `docs/specs/core.allium` before deciding on the two `repo_sync` predicates.

## Findings

### 💡 `VisualColumn::parent_group_span` — no production reader (`src/models/columns.rs:78`)

**Issue:** Referenced only from `src/models/columns.rs:230–233`, its own test module.

Its sibling `parent_group_start` (`:71`) **is** used, at `src/tui/mod.rs:1578`:

```rust
} else if vcol_idx == VisualColumn::parent_group_start(epic_parent) {
```

So the pair was written together and only half of it landed a caller.

```rust
pub fn parent_group_span(status: TaskStatus) -> usize {
    Self::ALL.iter().filter(|vc| vc.parent_status == status).count()
}
```

**Fix:** Check whether the code near `src/tui/mod.rs:1578` open-codes a span calculation (counting sub-columns for a parent status) inline. If it does, call `parent_group_span` there — that is the intended reader and the duplication is the bug. If nothing needs a span, delete the function and its four assertions.

Decide by reading `src/tui/mod.rs` around the column-anchor logic, not by guessing.

### 💡 `ReviewDecision::from_db_str` — and its `as_db_str` partner (`src/models/review.rs:63`)

**Issue:** The larger finding of the five. `from_db_str` is referenced only from `src/models/review.rs:111–115` and `:196`, its own test module. But going further:

- **`as_db_str` (`:53`) has no production reference either.**
- **`ReviewDecision` never appears anywhere under `src/db/`.**

So this is a documented database-serialisation pair —

```rust
/// Stable string for database storage. Not the same as `as_str()` (display)
/// or `parse()` (GitHub wire format).
pub fn as_db_str(&self) -> &'static str { … }

/// Parse from database string. Inverse of `as_db_str`.
pub fn from_db_str(s: &str) -> Option<Self> { … }
```

— with **no database behind it**. `ReviewDecision` is not persisted. The third constructor, `parse` (`:76`, GitHub GraphQL `reviewDecision` wire format), is the one that is actually used.

**Fix:** Establish first whether `ReviewDecision` is *meant* to be persisted. Check `docs/specs/pr-workflow.allium` and the `tasks` table schema in `src/db/migrations.rs`. Two outcomes:

- **It should be persisted and isn't** — that is a real gap, and this work package is the wrong place to fix it. Do not implement persistence here. Open a separate task and note the finding in this one's wrap-up.
- **It is derived live from the GitHub API and never stored** (the likely answer, given `parse` is the live reader and the review state is polled) — then delete **both** `as_db_str` and `from_db_str` plus their tests, and remove the "for database storage" doc comments that imply a contract that does not exist. The misleading comment is worth more than the dead code.

### 💡 `TaskTag::short_label` — no production reader (`src/models/tasks.rs:630`)

**Issue:** Referenced only from `src/models/tasks.rs:2051` and `:2127`, its own test module — including a loop that asserts a short label for every variant in `TaskTag::ALL`, so the tests give a strong impression of a live feature.

`TaskTag` and its neighbours carry **four** label functions: `header_label` (`:254`), `label` (`:580`), `short_label` (`:630`), `as_str` (`:863`). The card renderer (`src/tui/ui/kanban/cards.rs`) uses `label()`. Nothing uses `short_label()`.

**Fix:** Most likely a leftover from a narrower card layout that no longer exists. Before deleting, grep `docs/specs/*.allium` for a surface that promises abbreviated tag labels (`"pr-rev"`, `"feat"`, `"dep"` are the distinctive strings) — if a spec names them, the renderer is the thing that drifted, not the function. Otherwise delete the function and both tests.

### 💡 `AheadBehind::is_diverged` — names a case the code handles implicitly (`src/repo_sync.rs:34`)

**Issue:** Referenced only from `src/repo_sync.rs:527–542`, its own test module.

```rust
/// Both sides non-zero: local and origin have each moved independently.
/// This is the case resolved by merging rather than rebasing.
pub fn is_diverged(&self) -> bool { self.ahead > 0 && self.behind > 0 }
```

`sync_repo` never calls it. It handles the same case in a **comment** instead, at `src/repo_sync.rs:248`:

```rust
// --- Merge (fast-forwards when ahead = 0, merge commit when diverged) ---
```

The sibling `has_drift` (`:28`) is used, at `:244`.

**Fix:** This is the clearest "wire it in" candidate of the five. The divergence case is real, load-bearing (it is why this repo merges rather than rebases — see the user's standing no-force-push rule), and currently documented only in a comment. Either:

- use `is_diverged()` in `sync_repo` where the merge strategy is chosen or logged, so the named concept appears in the code path it describes; or
- if the merge is genuinely unconditional and no branch exists, keep the function only if `docs/specs/repo-sync.allium` declares it as a `derived` value — and cite that clause in its doc comment the way `is_measured` does.

### 💡 `RepoSyncState::is_measured` — a spec-derived predicate with a production twin (`src/repo_sync.rs:348`)

**Issue:** Referenced only from `src/repo_sync.rs:1226–1227`, `:1349`, `:1424`, its own test module. Unlike the other four, it carries an explicit justification:

```rust
/// Whether the repository could be measured. An unmeasured repository is
/// distinct from a clean one and must never be presented as clean
/// (`UnmeasuredIsNeverPresentedAsClean`).
pub fn is_measured(&self) -> bool { self.counts.is_some() }
```

That guarantee is real — `docs/specs/repo-sync.allium:658` defines `UnmeasuredIsNeverPresentedAsClean`, and it is cited at lines 27, 473 and 567 too.

But the guarantee is actually *enforced* by the neighbouring `has_drift`, which is used in production:

```rust
pub fn has_drift(&self) -> bool { self.counts.is_some_and(|c| c.has_drift()) }
```

The `is_some_and` is what stops an unmeasured repo reading as clean. So `is_measured` names the concept while `has_drift` upholds it.

**Fix:** Keep it — this is the case where a test-only predicate is legitimate — but close the loop. Preferred: find where the TUI decides how to present a repo's sync state (status bar / repo-filter surfaces) and have it call `is_measured()` to distinguish "unmeasured" from "clean", instead of inspecting `counts.is_some()` inline. That makes the spec guarantee visible at the surface it protects.

If the presentation genuinely does not need the distinction, leave the function exactly as it is and add nothing. Do **not** delete it: `docs/specs/repo-sync.allium` declares it, and deleting code the spec names is a spec violation, not a cleanup.

## Changes

| File | Change |
|------|--------|
| `src/models/columns.rs` | Wire `parent_group_span` into the column-anchor logic, or delete it with its 4 assertions |
| `src/tui/mod.rs` | If a span calculation is open-coded near `:1578`, replace it with `parent_group_span` |
| `src/models/review.rs` | After checking `pr-workflow.allium` and the schema: delete `as_db_str` + `from_db_str` + tests and the misleading "database storage" comments, or open a separate task for missing persistence |
| `src/models/tasks.rs` | After grepping the specs for abbreviated tag labels: delete `short_label` and its 2 tests, or fix the renderer |
| `src/repo_sync.rs` | Use `is_diverged()` where `sync_repo` chooses/logs the merge strategy (`~:248`), or cite the spec clause in its doc comment |
| `src/repo_sync.rs` | Leave `is_measured` in place; optionally wire it into the sync-state presentation |
| `docs/specs/repo-sync.allium` | Update only if a `derived` clause is added or removed. Use `allium:tend`, never a hand edit |
| `docs/specs/core.allium` | Same, if `TaskTag` labels turn out to be spec'd |

## Verification

- [ ] `cargo test` — all pass
- [ ] `cargo clippy --all-targets -- -D warnings` — clean. A newly-unused import after a deletion will surface here
- [ ] `cargo fmt` before committing
- [ ] `./scripts/check-doc-symbols.sh` and its self-test pass — this checker rejects doc citations to symbols that no longer resolve, so **deleting any of these five will trip it if a doc or spec cites it**. That is the checker doing its job: if it fires, the spec needs updating via `allium:tend`, not an `allow-phantom-symbol` annotation
- [ ] `./scripts/check-doc-paths.sh` and its self-test pass
- [ ] Run `allium:weed` over `docs/specs/repo-sync.allium` and confirm no new divergence was introduced
- [ ] For each of the five, state the decision and its evidence in the wrap-up: wired in (where), or deleted (and why the spec did not name it). A silent deletion is the one outcome this work package must not produce
