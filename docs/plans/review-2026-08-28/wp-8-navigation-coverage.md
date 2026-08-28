# Navigation Coverage

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Cover the untested branches in `src/tui/update/navigation.rs` — the lowest-coverage module under `src/tui/update/`, on the most-exercised code path in the product.

## Context

This work package addresses findings from the whole-repository codebase review of 2026-08-28 (`docs/plans/2026-08-28-codebase-review.md`, section 3).

`src/tui/update/navigation.rs` is **75.3%** covered (116/154 lines, 38 uncovered) — the lowest of the `src/tui/update/` modules. It is also the code every user touches on every keystroke: `j`/`k`/`h`/`l`, `gg`/`G`, and item reordering.

Unlike `src/setup/` and the render modules, this gap is **not** excused by `docs/testing.md`'s policy. There is no OS interaction and no terminal here — these are pure `&mut App` handlers returning `Vec<Command>`, which is the most testable shape in the codebase. The existing `src/tui/tests/navigation.rs` is already 2,602 lines, so the harness is in place; these branches were simply never reached.

Tarpaulin (llvm engine) reports the uncovered lines precisely:

```
56-58, 60-65, 69, 73, 96-97, 119, 122, 150, 153, 168, 171, 189, 192,
196, 199, 204-205, 207, 227-232, 245-250
```

**This work package adds tests only. No production code changes.** If a test reveals a genuine bug, stop, report it, and open a separate task — do not fix it here, because a behavioural fix hidden inside a coverage commit is invisible to review.

## Findings

### 💡 `handle_reorder_item`'s epic branches are almost entirely uncovered (`src/tui/update/navigation.rs:165`)

**Issue:** The largest single gap. `handle_reorder_item` is 99 lines (`:165`–`:263`) and lines 168, 171, 189, 192, 196, 199, 204–205, 207, 227–232, 245–250 are all unhit. The function does four independent persistence branches:

```rust
if let Some(tid) = a_task_id { … }   // covered
if let Some(eid) = a_epic_id { … }   // 189-207: uncovered
if let Some(tid) = b_task_id { … }   // partly covered
if let Some(eid) = b_epic_id { … }   // 227-250: uncovered
```

So **reordering an epic card has no test at all**, while reordering a task card does. Epics and tasks share a column, so this is a real user path.

Three distinct behaviours are untested:

1. **Epic reordering** — the `a_epic_id` / `b_epic_id` branches, which mutate `board.epics` directly and emit `EpicCommand::Persist { id, status: None, sort_order }`. Note the `status: None` — a reorder must not change status, and nothing currently asserts that.
2. **Mixed task/epic swap** — swapping a task card with an epic card in the same column exercises one task branch and one epic branch together. Two of the four `if let`s fire, in a combination no test reaches.
3. **The equal-`sort_order` offset rule** (`:203`–`:210`):

```rust
let (new_a, new_b) = if a_eff == b_eff {
    if direction > 0 { (a_eff + 1, b_eff) } else { (a_eff - 1, b_eff) }
} else { (b_eff, a_eff) };
```

Both items having the same effective sort value is the common case for freshly-created items (`sort_order` is `None`, so `unwrap_or(id.0)` falls back to the ID). The `direction < 0` half of that branch is unhit.

**Fix:** Add tests to `src/tui/tests/navigation.rs`:

- Reorder an epic up and down; assert its `sort_order` changed, that `EpicCommand::Persist` was emitted with `status: None`, and that the cursor followed via `set_row(col, target_row)`.
- Swap a task with an adjacent epic in both directions; assert both items' `sort_order` values and that **two** `Persist` commands were emitted.
- Two items with equal effective sort values, reordered in **both** directions; assert the `+1` / `-1` offset. Construct these with `sort_order: None` so the `unwrap_or(id.0)` fallback is what produces the tie — that is the realistic path.
- Assert `invalidate_layout_cache()` took effect. `cached_epic_stats()` self-heals on a fingerprint mismatch, so the observable is that the next read returns re-sorted order, not that a flag flipped.

Also cover the non-selectable early returns at `:186`–`:192` and `:196`–`:199` — attempting to reorder an `EpicHeader`, `SubstatusLabel`, or `OrphanSeparator` must return `vec![]`.

### 💡 The archive column branch of `handle_navigate_row` is uncovered (`src/tui/update/navigation.rs:52`)

**Issue:** Lines 56–58, 60–65, 69 and 73 are unhit. They are the archive-column special case:

```rust
if col == TaskStatus::COLUMN_COUNT + 1 {
    let count = self.archived_tasks().len();
    if count == 0 { return vec![]; }
    let new_row = (self.selection().row(…) as isize + delta).clamp(0, count as isize - 1) as usize;
    self.selection_mut().set_row(TaskStatus::COLUMN_COUNT + 1, new_row);
    self.archive.list_state.select(Some(new_row));
    return vec![];
}
```

Plus the `col == 0` guard (`:69`) and the `TaskStatus::from_column_index` `None` arm (`:73`).

`src/tui/tests/archive.rs` exists (1,560 lines) but evidently drives the archive view without moving the row through this handler.

**Fix:** Add tests for: navigating rows in the archive column with archived tasks present (clamped at both ends), navigating with **zero** archived tasks (the `count == 0` early return), and confirming `archive.list_state` is kept in sync with `selection().row(...)` — that dual bookkeeping is the bug-prone part and nothing currently asserts the two agree.

Put these in `src/tui/tests/navigation.rs` if they are about the handler, or `src/tui/tests/archive.rs` if they read more naturally as archive-view behaviour. Either is fine; pick one and be consistent.

### 💡 Scattered clamp and edge branches (`src/tui/update/navigation.rs`)

**Issue:** Lines 96–97 (`handle_navigate_row`'s tail), 119 and 122 (`handle_navigate_row_first`), 150 and 153 (`handle_navigate_row_last`).

`handle_navigate_row_first` / `_last` are the `gg` and `G` handlers. Commit `244f7293` recently added `gg`, `G` and half-page motions to the companion pane, so these are live, recently-touched code.

**Fix:** Cover the edge cases these lines represent — most likely the empty-column and already-at-the-boundary paths. Read each line before writing the test; do not guess from the line number.

## Changes

| File | Change |
|------|--------|
| `src/tui/tests/navigation.rs` | Tests for epic reordering, mixed task/epic swap, the equal-`sort_order` offset in both directions, and the non-selectable early returns |
| `src/tui/tests/navigation.rs` | Tests for `handle_navigate_row_first` / `_last` edge branches (`:119`, `:122`, `:150`, `:153`) and `handle_navigate_row`'s tail (`:96`–`:97`) |
| `src/tui/tests/navigation.rs` or `src/tui/tests/archive.rs` | Tests for the archive-column navigation branch, including zero archived tasks and `archive.list_state` sync |
| `src/tui/update/navigation.rs` | **No change.** Tests only |

## Verification

- [ ] `cargo test` — all pass
- [ ] `cargo tarpaulin --engine llvm --out stdout` — `src/tui/update/navigation.rs` meaningfully above 75.3%. Quote the engine in any number you report; the default `Auto` engine reads ~1.8 points lower and must not be compared against the CI floor
- [ ] Confirm the specific lines are now hit — re-read tarpaulin's uncovered-line list for this file and check that the epic branches (`:189`–`:207`, `:227`–`:250`) and the archive branch (`:56`–`:73`) have gone
- [ ] `git diff --stat` shows changes **only** under `src/tui/tests/`. Any production diff means this work package overstepped
- [ ] `cargo clippy --all-targets -- -D warnings` — clean
- [ ] `cargo fmt` before committing
- [ ] `./scripts/check-no-test-sleep.sh` and its self-test pass — no wall-clock sleep in any new test
- [ ] If any new test reveals a genuine bug, it is **reported and deferred**, not fixed here. Say so explicitly in the wrap-up
