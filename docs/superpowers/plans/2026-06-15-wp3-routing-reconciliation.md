# WP3 — Routing reconciliation (move-aware, subtree-scoped)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans. TDD: test first.
> This is the highest-risk WP — the adversarial review (B1/B2/B3) flagged the wrong approaches. Read the spec §5 in full before coding.

**Goal:** Reconcile a `reviews_parent` epic's whole role-sub-epic subtree from one emission, so that a PR whose role changes is **moved** (state preserved) and merged/closed PRs are removed — without the per-epic delete pass killing a just-moved task.

**Spec:** `docs/superpowers/specs/2026-06-15-pr-review-feed-routing-design.md` §5.
**Depends on:** WP1 (`feed_role`, `Signal`), WP2 (`route`).

**Interface this WP exposes:** `pub(crate) async fn run_role_routed_feed_sync(db, parent_id, items, repo_paths, base_branches) -> Result<Vec<EpicId>>` in `src/feed/ingest.rs`. Returns parent + affected sub-epic ids (for TUI notify), mirroring `sync_grouped_feed`'s return contract.

**Key facts from the codebase (cite, don't re-derive):**
- `upsert_feed_tasks` delete pass is per-epic: `DELETE … WHERE epic_id = ?1 AND external_id NOT IN (keep)` (`src/db/queries/tasks.rs:484`). **Do NOT reuse it per role** — it would delete a moved task.
- `set_task_epic_id` (`src/db/queries/epics.rs:192`) updates only `epic_id`/`updated_at` → `status`/`sub_status`/`worktree`/`tmux_window`/`sort_order` survive a move.
- `move_task_to_epic` lives in `src/service/tasks/crud.rs:122`; at the DB layer the move is `set_task_epic_id`.
- The conflict index is `UNIQUE(epic_id, external_id) WHERE external_id IS NOT NULL` (`src/db/migrations.rs:943`).

---

### Task 1: Subtree-scoped stale delete query

**Files:**
- Modify: `src/db/queries/tasks.rs` (new method on the task store trait; declare in `src/db/mod.rs` trait)
- Test: `src/db/tests/tasks.rs`

- [ ] **Step 1 — failing test.** Create a parent with two child epics, insert feed tasks (external_id set) into both + one manual task (external_id null) in one child. Call `delete_stale_subtree_feed_tasks(parent, keep=[id_in_child_a])`. Assert: the kept task survives, the other feed task is deleted, the manual task survives.
- [ ] **Step 2 — run, expect fail.**
- [ ] **Step 3 — implement:**
```rust
async fn delete_stale_subtree_feed_tasks(&self, parent_id: EpicId, keep_external_ids: &[String]) -> Result<()> {
    let keep = serde_json::to_string(keep_external_ids)?;
    self.db_call(move |conn| {
        conn.execute(
            "DELETE FROM tasks
             WHERE epic_id IN (SELECT id FROM epics WHERE parent_epic_id = ?1)
               AND external_id IS NOT NULL
               AND external_id NOT IN (SELECT value FROM json_each(?2))",
            params![parent_id.0, keep],
        )?;
        Ok(())
    }).await
}
```
- [ ] **Step 4 — run.** `cargo test -p dispatch db::tests::tasks`
- [ ] **Step 5 — commit:** `feat(db): subtree-scoped stale feed-task delete`

### Task 2: `run_role_routed_feed_sync` — insert + in-place update

**Files:**
- Modify: `src/feed/ingest.rs`
- Test: inline `#[cfg(test)] mod tests` in `ingest.rs` (follow the `sync_grouped_feed` test patterns already there)

- [ ] **Step 1 — failing test.** `route_routed_inserts_into_role_sub_epic`: parent with `feed_role=reviews_parent`; emit one PR with `signals=[DirectRequest]`; assert a `my_reviews` sub-epic is created/used and holds the task; team/bots empty.
- [ ] **Step 2 — run, expect fail.**
- [ ] **Step 3 — implement** the skeleton: ensure the three role sub-epics exist (look up by `feed_role` under the parent; create with `create_epic` + patch `feed_role` if absent — note WP5 also creates them, this is the idempotent same-path helper), group emitted items by `route(item.signals)`, and `upsert_feed_tasks(target_sub_epic, group_items, …)` for INSERT/UPDATE **only** (you will replace its delete behavior in Task 4 by using a no-op keep-set per role — see Task 4). For now, per role, call the existing upsert.
- [ ] **Step 4 — run, expect pass.**
- [ ] **Step 5 — commit:** `feat(feed): role-routed feed sync — insert/update`

### Task 3: Move on role change (preserve in-flight state) — B2

**Files:** `src/feed/ingest.rs`

- [ ] **Step 1 — failing test.** `route_routed_moves_task_preserving_state`:
  - Cycle 1: emit PR `pr#1` with `signals=[TeamRequest]` → lands in `team_reviews`.
  - Simulate in-flight work: set its `status=running`, `sub_status=active`, and a `worktree`/`tmux_window` via the DB (use the existing task patch/setters).
  - Cycle 2: emit same `pr#1` with `signals=[TeamRequest, Reviewed]` → routes to `my_reviews`.
  - Assert: exactly one task with external_id `pr#1`; its `epic_id` is `my_reviews`; `status==running`, `sub_status==active`, `worktree`/`tmux_window` unchanged.
- [ ] **Step 2 — run, expect fail** (currently delete+insert loses state, or duplicates).
- [ ] **Step 3 — implement.** Before per-role upsert, build a subtree index of existing feed tasks by `external_id` (across the three role sub-epics). For each emitted PR whose existing task is in a *different* role than `route(...)`: `set_task_epic_id(task_id, target)` then apply field updates via `patch_task` (title/description/tag/labels/sort_order). Then run per-role upsert for inserts + same-role updates.
- [ ] **Step 4 — run, expect pass.**
- [ ] **Step 5 — commit:** `feat(feed): move task on role change, preserving agent state`

### Task 4: Single subtree delete (don't kill moved tasks) — B1

**Files:** `src/feed/ingest.rs`

- [ ] **Step 1 — failing test.** `route_routed_move_not_deleted_same_cycle`: cycle 2 from Task 3 must NOT delete `pr#1` even though it's absent from `team_reviews`'s own group. Also `route_routed_removes_merged_pr`: a PR present in cycle 1 but absent in cycle 2 is deleted from the subtree; a manual task (external_id null) in a sub-epic survives.
- [ ] **Step 2 — run, expect fail** if per-role delete is still active.
- [ ] **Step 3 — implement.** Ensure the per-role upserts do **not** delete (pass each role its full intended keep-set, OR — cleaner — switch the per-role write to an insert/update-only path and perform deletion exactly once via `delete_stale_subtree_feed_tasks(parent, all_emitted_external_ids)` after all moves/upserts). Recalculate epic statuses for the parent + each affected sub-epic (`recalculate_epic_status_after_feed`, used by `sync_grouped_feed`). Return parent + affected ids.
- [ ] **Step 4 — run, expect pass.**
- [ ] **Step 5 — commit:** `feat(feed): single subtree-scoped delete after routing`

### Task 5: Wire the FeedRunner + concurrency guard — B3, H2

**Files:**
- Modify: `src/feed/mod.rs:191` (the `run_feed_sync` call site) and `src/feed/ingest.rs:175` (`run_feed_sync`)

- [ ] **Step 1 — failing test.** In `src/feed/mod.rs` tests (alongside the existing group_by_repo feed tests ~line 507): a parent epic with `feed_role=reviews_parent` and a `feed_command` emitting two PRs (different roles) routes into the right sub-epics after one tick. Plus `two_ticks_lose_nothing`: run `tick()` twice back-to-back (zero interval) and assert no task is dropped.
- [ ] **Step 2 — run, expect fail.**
- [ ] **Step 3 — implement.** In the runner, branch on `epic.feed_role == FeedRole::ReviewsParent`: call `run_role_routed_feed_sync` instead of `run_feed_sync`. Capture `epic.feed_role` alongside `epic_group_by_repo` at line 167. **Guard:** role sub-epics must never have a `feed_command` (enforced in WP5); add a debug assertion / skip if a sub-epic of a reviews_parent somehow has one, and document it. Confirm the subtree reconcile runs as a single awaited unit per tick (it already does — one `spawn` per parent).
- [ ] **Step 4 — run.** `cargo test -p dispatch feed::`
- [ ] **Step 5 — commit:** `feat(feed): route reviews_parent epics through role router`

---

## Done when
- `cargo test && ./scripts/check-doc-paths.sh` passes.
- Tests prove: insert, in-place update, **move with status/worktree/tmux preserved**, moved task not deleted same cycle, merged PR removed, manual task preserved, two-tick no-loss.
- `run_feed_sync`'s existing `group_by_repo` path is untouched (its tests still green).
