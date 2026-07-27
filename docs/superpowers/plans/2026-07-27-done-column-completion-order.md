# Done Column Completion-Order Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Done column (for both tasks and epics) default-sort by completion recency (most-recently-completed first) instead of falling back to id order, without changing any other column's default ordering or breaking the existing manual reorder feature.

**Architecture:** Reuse the existing `sort_order: Option<i64>` field. A single pure function, `sort_order_for_status_transition`, decides what to do to `sort_order` given a status transition: entering Done sets it to `-now.timestamp_millis()` (negative so the existing ascending comparators put the most recent first); leaving Done clears it to `None`; anything else leaves it untouched. This function is plugged into every place a task's or epic's status changes — two chokepoints for tasks (`TaskService::update_task`, `TaskService::cli_update_task`), two for epics (`EpicService::update_epic`, the raw-SQL `recalculate_epic_status_inner`) — plus a small guard in the feed re-poll upsert and a one-time backfill migration for pre-existing Done rows. No rendering/comparator code changes anywhere.

**Tech Stack:** Rust (2021 edition), rusqlite, tokio, chrono.

## Global Constraints

- Never `git add`/`git commit` anything under `docs/plans/` (this plan lives under `docs/superpowers/plans/`, which is fine).
- Follow TDD: write the failing test before the implementation in every task.
- `cargo fmt` runs automatically pre-push; don't hand-format.
- Inline `mod tests` blocks need `#[allow(clippy::unwrap_used, clippy::expect_used)]` at the top (already present in every file this plan touches).
- Migrations are append-only: never renumber or edit an existing migration entry.
- Run `cargo test && ./scripts/check-doc-paths.sh` before declaring the overall task done (per this task's verification command).

---

### Task 1: Core transition rule — `sort_order_for_status_transition`

**Files:**
- Modify: `src/models/tasks.rs` (add the function right after the `define_str_enum!(TaskStatus, ...)` block, i.e. after line 86)
- Test: `src/models/tasks.rs` (existing `#[cfg(test)] mod tests` block in this file — if none exists yet at the bottom of the file, check for one before adding; if none, add one following the pattern used elsewhere in this codebase: `#[cfg(test)] #[allow(clippy::unwrap_used, clippy::expect_used)] mod tests { use super::*; ... }`)

**Interfaces:**
- Produces: `pub fn sort_order_for_status_transition(prior: TaskStatus, next: TaskStatus, now: DateTime<Utc>) -> Option<Option<i64>>` — every later task in this plan calls this exact function via `crate::models::sort_order_for_status_transition`.

- [ ] **Step 1: Write the failing tests**

Add to `src/models/tasks.rs` (inside a `#[cfg(test)] mod tests` block at the bottom of the file — check first whether one already exists there; if it does, add these functions to it rather than creating a second block):

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn ts(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(seconds, 0).unwrap()
    }

    #[test]
    fn entering_done_sets_negative_millis_timestamp() {
        let now = ts(1_700_000_000);
        let result = sort_order_for_status_transition(TaskStatus::Review, TaskStatus::Done, now);
        assert_eq!(result, Some(Some(-now.timestamp_millis())));
    }

    #[test]
    fn leaving_done_clears_to_none() {
        let now = ts(1_700_000_000);
        let result = sort_order_for_status_transition(TaskStatus::Done, TaskStatus::Review, now);
        assert_eq!(result, Some(None));
    }

    #[test]
    fn staying_in_done_is_untouched() {
        let now = ts(1_700_000_000);
        let result = sort_order_for_status_transition(TaskStatus::Done, TaskStatus::Done, now);
        assert_eq!(result, None);
    }

    #[test]
    fn staying_outside_done_is_untouched() {
        let now = ts(1_700_000_000);
        let result =
            sort_order_for_status_transition(TaskStatus::Backlog, TaskStatus::Running, now);
        assert_eq!(result, None);
        let result =
            sort_order_for_status_transition(TaskStatus::Running, TaskStatus::Archived, now);
        assert_eq!(result, None);
    }

    #[test]
    fn entering_done_value_is_negative_and_more_recent_sorts_first() {
        let earlier = sort_order_for_status_transition(
            TaskStatus::Review,
            TaskStatus::Done,
            ts(1_700_000_000),
        )
        .unwrap()
        .unwrap();
        let later = sort_order_for_status_transition(
            TaskStatus::Review,
            TaskStatus::Done,
            ts(1_700_000_100),
        )
        .unwrap()
        .unwrap();
        assert!(
            later < earlier,
            "a more recent completion must sort before ({later}) an older one ({earlier}) under ascending sort_by_key"
        );
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib models::tasks::tests -- --nocapture`
Expected: FAIL with "cannot find function `sort_order_for_status_transition`"

- [ ] **Step 3: Implement the function**

Add to `src/models/tasks.rs` right after the `define_str_enum!(TaskStatus, "status" { ... });` block (after line 86):

```rust
/// Decides what a status transition should do to `sort_order`, expressed as
/// an instruction for `TaskPatch`/`EpicPatch`'s nullable `.sort_order()`
/// setter: `None` = don't touch it, `Some(v)` = write `v` (where `v` may
/// itself be `None` to clear, or `Some(ts)` to set).
///
/// The value on entering Done is the negated Unix timestamp in
/// **milliseconds** (not seconds): the existing ascending `sort_by_key`
/// comparators used throughout the Done column already put the most
/// negative (= most recent) value first, with no comparator changes needed.
/// Millisecond precision (rather than the more obvious seconds) shrinks the
/// same-tick tie window for bulk actions (multi-select "confirm done", the
/// PR-poller detecting several merges in one 30s tick) — a same-millisecond
/// tie is still possible in principle and degrades gracefully to the
/// existing id tie-break, rather than being eliminated outright.
pub fn sort_order_for_status_transition(
    prior: TaskStatus,
    next: TaskStatus,
    now: DateTime<Utc>,
) -> Option<Option<i64>> {
    match (prior == TaskStatus::Done, next == TaskStatus::Done) {
        (false, true) => Some(Some(-now.timestamp_millis())),
        (true, false) => Some(None),
        _ => None,
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib models::tasks::tests -- --nocapture`
Expected: PASS (5 tests)

- [ ] **Step 5: Commit**

```bash
git add src/models/tasks.rs
git commit -m "feat: add sort_order_for_status_transition core rule

Pure function deciding what a Done-transition should do to sort_order.
Foundation for the Done-column completion-order feature; not yet wired
into any call site."
```

---

### Task 2: `TaskService::update_task` — apply the transition rule

**Files:**
- Modify: `src/service/tasks/crud.rs:83-162` (the `update_task` method)
- Test: `src/service/tasks/tests.rs`

**Interfaces:**
- Consumes: `crate::models::sort_order_for_status_transition(prior: TaskStatus, next: TaskStatus, now: DateTime<Utc>) -> Option<Option<i64>>` (Task 1)
- Consumes: `TaskPatch::sort_order(self, v: Option<i64>) -> Self` (existing, `src/db/mod.rs:82`)
- Consumes: `TaskService.clock: Arc<dyn crate::service::Clock>` (existing field, `.now() -> DateTime<Utc>`)

- [ ] **Step 1: Write the failing tests**

Add to `src/service/tasks/tests.rs` (near the other `update_task_status*` tests, e.g. after `update_task_invalid_substatus_for_status` around line 247):

```rust
#[tokio::test]
async fn update_task_entering_done_sets_sort_order() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Review))
        .await
        .unwrap();
    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Done))
        .await
        .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Done);
    assert!(
        task.sort_order.is_some_and(|so| so < 0),
        "expected a negative sort_order on entering Done, got {:?}",
        task.sort_order
    );
}

#[tokio::test]
async fn update_task_leaving_done_clears_sort_order() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Done))
        .await
        .unwrap();
    assert!(svc.get_task(id).await.unwrap().sort_order.is_some());

    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Review))
        .await
        .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Review);
    assert_eq!(task.sort_order, None);
}

#[tokio::test]
async fn update_task_leaving_done_clears_sort_order_even_with_stale_caller_sort_order() {
    // Reproduces the exec_persist_task shape: a caller sends both a status
    // change AND a stale sort_order left over from when the task entered
    // Done, exactly as exec_persist_task (src/runtime/tasks.rs) forwards
    // whatever sort_order is sitting on the in-memory Task struct. The
    // "leaving Done" clear must win over this caller-supplied value.
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Done))
        .await
        .unwrap();
    let stale_sort_order = svc.get_task(id).await.unwrap().sort_order.unwrap();

    svc.update_task(
        UpdateTaskParams::for_task(id)
            .status(TaskStatus::Review)
            .sort_order(stale_sort_order),
    )
    .await
    .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(
        task.sort_order, None,
        "the leaving-Done clear must win over a caller-supplied stale sort_order"
    );
}

#[tokio::test]
async fn update_task_status_change_within_done_leaves_sort_order_untouched() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Done))
        .await
        .unwrap();
    let sort_order_after_entry = svc.get_task(id).await.unwrap().sort_order;

    // An unrelated field edit while already Done (no status change at all).
    svc.update_task(UpdateTaskParams::for_task(id).title("Renamed".to_string()))
        .await
        .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.sort_order, sort_order_after_entry);
}

#[tokio::test]
async fn update_task_archived_to_backlog_is_unaffected_by_done_rule() {
    // The task editor's freeform STATUS field can retype an Archived task's
    // status back to any value (no transition-legality validation), which
    // routes through this same update_task — a reachable "un-archive" path.
    // sort_order is already None by the time a task reaches Archived (it
    // was cleared on the Done -> Archived leg), so Archived -> Backlog must
    // be a no-op for sort_order and must not error.
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Done))
        .await
        .unwrap();
    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Archived))
        .await
        .unwrap();
    assert_eq!(svc.get_task(id).await.unwrap().sort_order, None);

    svc.update_task(UpdateTaskParams::for_task(id).status(TaskStatus::Backlog))
        .await
        .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert_eq!(task.status, TaskStatus::Backlog);
    assert_eq!(task.sort_order, None);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib service::tasks::tests::update_task_entering_done_sets_sort_order service::tasks::tests::update_task_leaving_done_clears_sort_order service::tasks::tests::update_task_leaving_done_clears_sort_order_even_with_stale_caller_sort_order service::tasks::tests::update_task_status_change_within_done_leaves_sort_order_untouched service::tasks::tests::update_task_archived_to_backlog_is_unaffected_by_done_rule`
Expected: FAIL — `sort_order` stays `None` on entering Done, and the "stale caller value" test fails because the clear doesn't happen yet. (`update_task_archived_to_backlog_is_unaffected_by_done_rule` should already pass — it documents existing-correct behavior for the leaving-Done-then-Archived case and guards it against regression.)

- [ ] **Step 3: Implement the change**

In `src/service/tasks/crud.rs`, add the import (top of file, extend the existing `use crate::models::{...}` block at line 10-13):

```rust
use crate::models::{
    classify_agent_activity, sort_order_for_status_transition, EpicId, HookEventKind,
    NotificationBehavior, SubStatus, Task, TaskId, TaskStatus, DEFAULT_BASE_BRANCH,
};
```

Replace lines 97-120 (from `let patch = build_task_patch(...)` through the `was_pr_finalisation` binding) with:

```rust
        let mut patch = build_task_patch(&params, expanded_repo_path.as_deref(), validated_sub_status);

        // Snapshot the task before the patch. Needed whenever `epic_id` is
        // relinked (existing reason), whenever `status` changes and sets a
        // PR-typed url (existing PR-finalisation check), and now whenever
        // `status` changes at all — to detect a transition into/out of Done
        // for the sort_order-on-completion rule below.
        let is_pr_url_set = matches!(
            params.url.as_ref(),
            Some(UrlUpdate::Set(u)) if u.is_pr()
        );
        let needs_prior = params.epic_id.is_some() || params.status.is_some();
        let prior = if needs_prior {
            self.db.get_task(task_id).await?
        } else {
            None
        };
        let was_pr_finalisation = params.status == Some(TaskStatus::Review)
            && is_pr_url_set
            && prior.as_ref().is_some_and(|t| t.url.is_none());

        // The Done-transition rule must win over anything the caller's
        // params already set for sort_order — exec_persist_task
        // (src/runtime/tasks.rs) unconditionally forwards whatever
        // sort_order is sitting on the in-memory Task struct alongside a
        // status change, so a defensive-only override would leave a task
        // that just left Done permanently pinned to the top of whatever
        // column it lands in next.
        if let (Some(new_status), Some(p)) = (params.status, prior.as_ref()) {
            if let Some(so) = sort_order_for_status_transition(p.status, new_status, self.clock.now()) {
                patch = patch.sort_order(so);
            }
        }
```

(Note: this removes the now-unused `is_finishing_status` local — `needs_prior` is simplified since `params.status.is_some()` is a superset of both the old `is_finishing_status` check and the `params.status == Some(Review) && is_pr_url_set` check.)

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib service::tasks::tests::`
Expected: PASS — all tests in this module, including the 4 new ones and every pre-existing `update_task*` test (confirms no regression to the PR-finalisation or epic-relink logic sharing the same `prior` fetch).

- [ ] **Step 5: Commit**

```bash
git add src/service/tasks/crud.rs src/service/tasks/tests.rs
git commit -m "feat: set/clear task sort_order on Done transition in update_task

Widens the existing conditional prior-task fetch to fire on any status
change (not just entering a finishing status), and applies
sort_order_for_status_transition as an unconditional override — the
in-memory task's stale sort_order is otherwise forwarded on every
TUI persist regardless of what changed."
```

---

### Task 3: `TaskService::cli_update_task` — apply the transition rule

**Files:**
- Modify: `src/service/tasks/crud.rs:318-361` (the `cli_update_task` method)
- Test: `src/service/tasks/tests.rs`

**Interfaces:**
- Consumes: same as Task 2.

- [ ] **Step 1: Write the failing tests**

Add to `src/service/tasks/tests.rs`, near the other `cli_update_task_*` tests (after `cli_update_task_updates_status_unconditionally` around line 2785):

```rust
#[tokio::test]
async fn cli_update_task_entering_done_sets_sort_order() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    svc.cli_update_task(id, TaskStatus::Done, None, None)
        .await
        .unwrap();

    let task = svc.get_task(id).await.unwrap();
    assert!(
        task.sort_order.is_some_and(|so| so < 0),
        "expected a negative sort_order on entering Done, got {:?}",
        task.sort_order
    );
}

#[tokio::test]
async fn cli_update_task_leaving_done_clears_sort_order() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    svc.cli_update_task(id, TaskStatus::Done, None, None)
        .await
        .unwrap();
    assert!(svc.get_task(id).await.unwrap().sort_order.is_some());

    svc.cli_update_task(id, TaskStatus::Backlog, None, None)
        .await
        .unwrap();

    assert_eq!(svc.get_task(id).await.unwrap().sort_order, None);
}

#[tokio::test]
async fn cli_update_task_only_if_not_matching_does_not_touch_sort_order() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    // Precondition doesn't match (task is Backlog, not Running) — the
    // conditional write must be a full no-op, including sort_order.
    let updated = svc
        .cli_update_task(id, TaskStatus::Done, Some(TaskStatus::Running), None)
        .await
        .unwrap();

    assert!(!updated);
    assert_eq!(svc.get_task(id).await.unwrap().sort_order, None);
}

#[tokio::test]
async fn cli_update_task_only_if_matching_entering_done_sets_sort_order() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc.create_task(make_task_params("/repo")).await.unwrap();

    let updated = svc
        .cli_update_task(id, TaskStatus::Done, Some(TaskStatus::Backlog), None)
        .await
        .unwrap();

    assert!(updated);
    assert!(svc.get_task(id).await.unwrap().sort_order.is_some_and(|so| so < 0));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib service::tasks::tests::cli_update_task_entering_done_sets_sort_order service::tasks::tests::cli_update_task_leaving_done_clears_sort_order service::tasks::tests::cli_update_task_only_if_not_matching_does_not_touch_sort_order service::tasks::tests::cli_update_task_only_if_matching_entering_done_sets_sort_order`
Expected: FAIL — `sort_order` stays `None` in all cases.

- [ ] **Step 3: Implement the change**

Replace `src/service/tasks/crud.rs:318-361` (the full `cli_update_task` method body) with:

```rust
    pub async fn cli_update_task(
        &self,
        task_id: TaskId,
        new_status: TaskStatus,
        only_if: Option<TaskStatus>,
        sub_status: Option<SubStatus>,
    ) -> Result<bool, ServiceError> {
        // Always fetched (not just for finishing statuses): needed to
        // detect a transition away from Done regardless of what the new
        // status is, per sort_order_for_status_transition.
        let prior = self.db.get_task(task_id).await?;
        let sort_order_override = prior
            .as_ref()
            .and_then(|p| sort_order_for_status_transition(p.status, new_status, self.clock.now()));

        let updated = if let Some(expected) = only_if {
            let changed = self
                .db
                .update_status_if(task_id, new_status, expected)
                .await?;
            if changed {
                let mut patch = crate::db::TaskPatch::new();
                if let Some(ss) = sub_status {
                    patch = patch.sub_status(ss);
                }
                if let Some(so) = sort_order_override {
                    patch = patch.sort_order(so);
                }
                if patch.has_changes() {
                    self.db.patch_task(task_id, &patch).await?;
                }
            }
            changed
        } else {
            let mut patch = crate::db::TaskPatch::new().status(new_status);
            if let Some(ss) = sub_status {
                patch = patch.sub_status(ss);
            }
            if let Some(so) = sort_order_override {
                patch = patch.sort_order(so);
            }
            self.db.patch_task(task_id, &patch).await?;
            true
        };

        if updated {
            self.notify_watchers_after_status_write(prior.as_ref(), Some(new_status))
                .await;
            self.recalculate_epic_for_task(task_id).await;
        }

        Ok(updated)
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib service::tasks::tests::`
Expected: PASS — all tests in this module, including the 4 new ones and every pre-existing `cli_update_task*` test.

- [ ] **Step 5: Commit**

```bash
git add src/service/tasks/crud.rs src/service/tasks/tests.rs
git commit -m "feat: set/clear task sort_order on Done transition in cli_update_task

Fetches the prior task unconditionally (previously only for finishing
statuses) so the leaving-Done clear can fire regardless of target
status, mirroring update_task's fix."
```

---

### Task 4: `EpicService::update_epic` — clock injection + apply the transition rule

**Files:**
- Modify: `src/service/epics.rs` (struct fields/constructor around lines 100-107, and `update_epic` at lines 275-340)
- Test: `src/service/epics.rs` (inline `mod tests` block, lines 401+)

**Interfaces:**
- Consumes: `crate::models::sort_order_for_status_transition` (Task 1)
- Produces: `EpicService::with_clock(self, clock: Arc<dyn crate::service::Clock>) -> Self` — a new builder, mirroring `TaskService::with_clock`, used by this task's own tests.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `src/service/epics.rs` (near `update_epic_sets_group_by_repo`, around line 578):

```rust
    fn epic_svc_with_clock(
        db: Arc<Database>,
        clock: Arc<dyn crate::service::Clock>,
    ) -> EpicService {
        EpicService::new(db).with_clock(clock)
    }

    #[tokio::test]
    async fn update_epic_entering_done_sets_sort_order() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("Test", "", None).await.unwrap();
        let clock = Arc::new(crate::service::FixedClock::new(chrono::Utc::now()));
        let svc = epic_svc_with_clock(db.clone(), clock);

        svc.update_epic(UpdateEpicParams {
            status: Some(TaskStatus::Done),
            ..base_params(epic.id)
        })
        .await
        .unwrap();

        let updated = db.get_epic(epic.id).await.unwrap().unwrap();
        assert!(
            updated.sort_order.is_some_and(|so| so < 0),
            "expected a negative sort_order on entering Done, got {:?}",
            updated.sort_order
        );
    }

    #[tokio::test]
    async fn update_epic_leaving_done_clears_sort_order() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("Test", "", None).await.unwrap();
        let svc = EpicService::new(db.clone());

        svc.update_epic(UpdateEpicParams {
            status: Some(TaskStatus::Done),
            ..base_params(epic.id)
        })
        .await
        .unwrap();
        assert!(db.get_epic(epic.id).await.unwrap().unwrap().sort_order.is_some());

        svc.update_epic(UpdateEpicParams {
            status: Some(TaskStatus::Backlog),
            ..base_params(epic.id)
        })
        .await
        .unwrap();

        let updated = db.get_epic(epic.id).await.unwrap().unwrap();
        assert_eq!(updated.sort_order, None);
    }

    #[tokio::test]
    async fn update_epic_unrelated_field_edit_while_done_leaves_sort_order_untouched() {
        let db = Arc::new(Database::open_in_memory().await.unwrap());
        let epic = db.create_epic("Test", "", None).await.unwrap();
        let svc = EpicService::new(db.clone());

        svc.update_epic(UpdateEpicParams {
            status: Some(TaskStatus::Done),
            ..base_params(epic.id)
        })
        .await
        .unwrap();
        let sort_order_after_entry = db.get_epic(epic.id).await.unwrap().unwrap().sort_order;

        svc.update_epic(UpdateEpicParams {
            title: Some("Renamed".to_string()),
            ..base_params(epic.id)
        })
        .await
        .unwrap();

        let updated = db.get_epic(epic.id).await.unwrap().unwrap();
        assert_eq!(updated.sort_order, sort_order_after_entry);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib service::epics::tests::update_epic_entering_done_sets_sort_order service::epics::tests::update_epic_leaving_done_clears_sort_order service::epics::tests::update_epic_unrelated_field_edit_while_done_leaves_sort_order_untouched`
Expected: FAIL to compile — `EpicService::with_clock` doesn't exist yet — then, after Step 3's constructor addition alone, FAIL at runtime because `update_epic` doesn't apply the rule yet.

- [ ] **Step 3: Implement the change**

In `src/service/epics.rs`, update the imports (top of file):

```rust
use std::sync::Arc;

use crate::db::{self, EpicPatch};
use crate::models::{sort_order_for_status_transition, Epic, EpicId, Task, TaskStatus};

use super::{FieldUpdate, ServiceError};
```

Replace the `EpicService` struct and its `new` constructor (lines 100-107):

```rust
pub struct EpicService {
    pub db: Arc<dyn db::TaskAndEpicStore>,
    clock: Arc<dyn crate::service::Clock>,
}

impl EpicService {
    pub fn new(db: Arc<dyn db::TaskAndEpicStore>) -> Self {
        Self {
            db,
            clock: Arc::new(crate::service::SystemClock),
        }
    }

    /// Override the clock used for the Done-transition sort_order rule.
    /// Tests inject a `FixedClock` for determinism; mirrors
    /// `TaskService::with_clock`.
    pub fn with_clock(mut self, clock: Arc<dyn crate::service::Clock>) -> Self {
        self.clock = clock;
        self
    }
```

In `update_epic` (lines 275-340), insert the prior-fetch and override. After the `let mut patch = EpicPatch::new();` line and its existing field-mapping block (through `if let Some(gbr) = params.group_by_repo { patch = patch.group_by_repo(gbr); }`, i.e. right before the `// Prevent reparenting...` comment at line 311), add:

```rust
        // Fetch the prior epic whenever status changes, to detect a
        // transition into/out of Done for the sort_order-on-completion
        // rule. This method has no other prior-fetch to reuse (the
        // RepoGroup-reparent guard below does its own, gated on a
        // different condition).
        if let Some(new_status) = params.status {
            if let Some(prior_epic) = self.db.get_epic(params.epic_id).await? {
                if let Some(so) =
                    sort_order_for_status_transition(prior_epic.status, new_status, self.clock.now())
                {
                    patch = patch.sort_order(so);
                }
            }
        }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib service::epics::tests::`
Expected: PASS — all tests in this module, including the 3 new ones and every pre-existing `update_epic*`/`create_epic*`/parent-cycle test (confirms the added prior-fetch doesn't disturb the reparent-guard logic below it, since that guard already does its own independent `self.db.get_epic` call).

- [ ] **Step 5: Commit**

```bash
git add src/service/epics.rs
git commit -m "feat: set/clear epic sort_order on Done transition in update_epic

Adds a clock field (defaulting to SystemClock, mirroring TaskService) and
a with_clock builder for test determinism — zero changes needed at any
of EpicService::new's 29 existing call sites."
```

---

### Task 5: `recalculate_epic_status_inner` — apply the transition rule (automatic rollup)

**Files:**
- Modify: `src/db/queries/epics.rs:376-457` (the `recalculate_epic_status_inner` function)
- Test: `src/db/tests/epics.rs`

**Interfaces:**
- Consumes: `crate::models::sort_order_for_status_transition` (Task 1)
- Note: this function runs synchronously inside a `db_call` closure against a raw `&rusqlite::Connection` and has no access to a service-layer `Clock`. It uses `chrono::Utc::now()` directly — this is DB-layer code with no existing clock-injection convention, and the value only needs to be a monotonically-advancing timestamp, not exactly reproducible; tests assert `Some`/`None`-ness and relative before/after bounds, not an exact injected value.

- [ ] **Step 1: Write the failing tests**

Add to `src/db/tests/epics.rs`, near `recalculate_epic_status_all_done` (after line 458):

```rust
#[tokio::test]
async fn recalculate_epic_status_all_done_sets_sort_order() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let t1 = create_task_returning(&db, "T1", "", "/repo", None, TaskStatus::Backlog)
        .await
        .unwrap();
    db.set_task_epic_id(t1.id, Some(epic.id)).await.unwrap();
    db.patch_task(t1.id, &TaskPatch::new().status(TaskStatus::Done))
        .await
        .unwrap();

    let before = chrono::Utc::now().timestamp_millis();
    db.recalculate_epic_status(epic.id).await.unwrap();
    let after = chrono::Utc::now().timestamp_millis();

    let epic = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(epic.status, TaskStatus::Done);
    let sort_order = epic
        .sort_order
        .expect("sort_order should be set on entering Done");
    assert!(
        (-after..=-before).contains(&sort_order),
        "sort_order {sort_order} should be -now_millis, within [{}, {}]",
        -after,
        -before
    );
}

#[tokio::test]
async fn recalculate_epic_status_done_regression_clears_sort_order() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    db.patch_epic(
        epic.id,
        &EpicPatch::new().status(TaskStatus::Done).sort_order(Some(-1)),
    )
    .await
    .unwrap();

    let task = create_task_returning(&db, "T1", "", "/repo", None, TaskStatus::Backlog)
        .await
        .unwrap();
    db.set_task_epic_id(task.id, Some(epic.id)).await.unwrap();
    db.patch_task(task.id, &TaskPatch::new().status(TaskStatus::Running))
        .await
        .unwrap();

    db.recalculate_epic_status(epic.id).await.unwrap();

    let epic = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(epic.status, TaskStatus::Backlog);
    assert_eq!(epic.sort_order, None);
}

#[tokio::test]
async fn recalculate_epic_status_already_done_noop_leaves_sort_order_untouched() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let task = create_task_returning(&db, "T1", "", "/repo", None, TaskStatus::Done)
        .await
        .unwrap();
    db.set_task_epic_id(task.id, Some(epic.id)).await.unwrap();
    db.patch_epic(
        epic.id,
        &EpicPatch::new().status(TaskStatus::Done).sort_order(Some(-42)),
    )
    .await
    .unwrap();

    db.recalculate_epic_status(epic.id).await.unwrap();

    let epic = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(epic.status, TaskStatus::Done);
    assert_eq!(
        epic.sort_order,
        Some(-42),
        "a no-op recalculation (already Done, still all-done) must not touch sort_order"
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib db::tests::epics::recalculate_epic_status_all_done_sets_sort_order db::tests::epics::recalculate_epic_status_done_regression_clears_sort_order db::tests::epics::recalculate_epic_status_already_done_noop_leaves_sort_order_untouched`
Expected: FAIL — the first test fails because `sort_order` stays `None`; the second fails because `sort_order` stays `Some(-1)` instead of clearing; the third should already pass (documents existing correct behavior — the `if target != epic.status` guard already prevents touching anything on a no-op).

- [ ] **Step 3: Implement the change**

In `src/db/queries/epics.rs`, add imports at the top (extend line 1 and line 8):

```rust
use std::collections::HashSet;

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, OptionalExtension};

use crate::set_field;

use crate::models::{sort_order_for_status_transition, EpicId, TaskId, TaskStatus};
```

Replace the `if target != epic.status { ... }` block inside `recalculate_epic_status_inner` (lines 439-448 per the current source) with:

```rust
    if target != epic.status {
        let now = Utc::now();
        let rows = match sort_order_for_status_transition(epic.status, target, now) {
            Some(sort_order) => conn
                .execute(
                    "UPDATE epics SET status = ?1, sort_order = ?2, updated_at = datetime('now') WHERE id = ?3",
                    params![target.as_str(), sort_order, epic_id.0],
                )
                .context("Failed to update epic status (recalc)")?,
            None => conn
                .execute(
                    "UPDATE epics SET status = ?1, updated_at = datetime('now') WHERE id = ?2",
                    params![target.as_str(), epic_id.0],
                )
                .context("Failed to update epic status (recalc)")?,
        };
        if rows == 0 {
            anyhow::bail!("Epic {epic_id} not found");
        }
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib db::tests::epics::`
Expected: PASS — all tests in this module, including the 3 new ones and every pre-existing `recalculate_epic_status_*` test (confirms the regression-to-Backlog and no-active-children-unchanged and cycle-safety tests still hold).

- [ ] **Step 5: Commit**

```bash
git add src/db/queries/epics.rs src/db/tests/epics.rs
git commit -m "feat: set/clear epic sort_order in automatic Done rollup

recalculate_epic_status_inner already gates its status write on
target != epic.status, so the no-op-recalculation case naturally never
touches sort_order — no separate guard needed for that."
```

---

### Task 6: Feed re-poll guard — don't clobber a Done task's `sort_order`

**Files:**
- Modify: `src/db/queries/tasks.rs:469-488` (the `upsert_feed_tasks` SQL)
- Test: `src/db/tests/tasks.rs`

**Interfaces:** none (self-contained SQL change).

- [ ] **Step 1: Write the failing test**

Add to `src/db/tests/tasks.rs`, near `upsert_feed_tasks_preserves_status` (after line 1613):

```rust
#[tokio::test]
async fn upsert_feed_tasks_preserves_sort_order_when_task_is_done() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let items = vec![make_feed_item("ext-1", "Original Title")];

    db.upsert_feed_tasks(epic.id, &items, &["/repo".to_string()], &main_branches(1))
        .await
        .unwrap();

    // Simulate the task completing and getting a completion-order
    // sort_order, then the feed re-polling with its own severity-rank
    // sort_order — the completion value must survive.
    let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    db.patch_task(
        tasks[0].id,
        &TaskPatch::new()
            .status(TaskStatus::Done)
            .sort_order(Some(-1_700_000_000_000)),
    )
    .await
    .unwrap();

    let mut updated_item = make_feed_item("ext-1", "Original Title");
    updated_item.sort_order = Some(1); // feed severity rank
    db.upsert_feed_tasks(epic.id, &[updated_item], &["/repo".to_string()], &main_branches(1))
        .await
        .unwrap();

    let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(
        tasks[0].sort_order,
        Some(-1_700_000_000_000),
        "re-poll must not clobber a Done task's completion-order sort_order"
    );
}

#[tokio::test]
async fn upsert_feed_tasks_still_updates_sort_order_when_task_is_not_done() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let items = vec![make_feed_item("ext-1", "Original Title")];

    db.upsert_feed_tasks(epic.id, &items, &["/repo".to_string()], &main_branches(1))
        .await
        .unwrap();

    let mut updated_item = make_feed_item("ext-1", "Original Title");
    updated_item.sort_order = Some(7);
    db.upsert_feed_tasks(epic.id, &[updated_item], &["/repo".to_string()], &main_branches(1))
        .await
        .unwrap();

    let tasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    assert_eq!(tasks[0].sort_order, Some(7));
}
```

- [ ] **Step 2: Run the tests to verify they fail (the first) / pass (the second)**

Run: `cargo test --lib db::tests::tasks::upsert_feed_tasks_preserves_sort_order_when_task_is_done db::tests::tasks::upsert_feed_tasks_still_updates_sort_order_when_task_is_not_done`
Expected: `upsert_feed_tasks_preserves_sort_order_when_task_is_done` FAILs (current code unconditionally overwrites); `upsert_feed_tasks_still_updates_sort_order_when_task_is_not_done` already PASSes (documents existing correct behavior, guards against a future regression from the fix).

- [ ] **Step 3: Implement the change**

In `src/db/queries/tasks.rs`, change the `ON CONFLICT DO UPDATE SET` clause (line 485, currently `sort_order  = excluded.sort_order,`) to:

```sql
                         sort_order  = CASE WHEN tasks.status != 'done' THEN excluded.sort_order ELSE tasks.sort_order END,
```

So the full `INSERT ... ON CONFLICT` statement (lines 474-488) reads:

```rust
                    "INSERT INTO tasks
                         (title, description, repo_path, status, sub_status, base_branch,
                          epic_id, external_id, tag, labels, sort_order, url, url_type,
                          wrap_up_mode)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                     ON CONFLICT(epic_id, external_id) WHERE external_id IS NOT NULL
                     DO UPDATE SET
                         title       = excluded.title,
                         description = excluded.description,
                         tag         = excluded.tag,
                         labels      = excluded.labels,
                         sort_order  = CASE WHEN tasks.status != 'done' THEN excluded.sort_order ELSE tasks.sort_order END,
                         url      = CASE WHEN tasks.url IS NOT NULL THEN tasks.url      ELSE excluded.url      END,
                         url_type = CASE WHEN tasks.url IS NOT NULL THEN tasks.url_type ELSE excluded.url_type END,
                         updated_at  = datetime('now')",
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib db::tests::tasks::`
Expected: PASS — all tests in this module, including both new ones and every pre-existing `upsert_feed_tasks_*` test.

- [ ] **Step 5: Commit**

```bash
git add src/db/queries/tasks.rs src/db/tests/tasks.rs
git commit -m "fix: don't let feed re-poll clobber a Done task's sort_order

The CVE feed reuses sort_order for severity ranking and previously
overwrote it unconditionally on every re-poll, which would silently
erase a task's completion-order value the moment it finished."
```

---

### Task 7: Migration v79 — backfill `sort_order` for existing Done rows

**Files:**
- Modify: `src/db/migrations.rs` (add the migration function and register it in `MIGRATIONS`)
- Test: `src/db/tests/migrations.rs`

**Interfaces:** none (self-contained migration).

- [ ] **Step 1: Write the failing test**

Add to `src/db/tests/migrations.rs` (near `v64_backfills_url_type_from_pr_url`):

```rust
#[tokio::test]
async fn v79_backfills_sort_order_for_done_tasks_and_epics() {
    use rusqlite::Connection as RawConn;
    let conn = RawConn::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE tasks (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             status TEXT NOT NULL,
             sort_order INTEGER,
             updated_at TEXT NOT NULL
         );
         CREATE TABLE epics (
             id INTEGER PRIMARY KEY,
             title TEXT NOT NULL,
             status TEXT NOT NULL,
             sort_order INTEGER,
             updated_at TEXT NOT NULL
         );
         INSERT INTO tasks (title, status, sort_order, updated_at) VALUES
           ('done-no-sort-order', 'done', NULL, '2026-01-15 12:00:00'),
           ('done-already-sorted', 'done', -999, '2026-01-15 12:00:00'),
           ('not-done', 'backlog', NULL, '2026-01-15 12:00:00');
         INSERT INTO epics (title, status, sort_order, updated_at) VALUES
           ('epic-done-no-sort-order', 'done', NULL, '2026-02-01 08:30:00'),
           ('epic-not-done', 'running', NULL, '2026-02-01 08:30:00');",
    )
    .unwrap();

    crate::db::migrations::migrate_v79_backfill_done_sort_order(&conn).unwrap();

    let mut stmt = conn
        .prepare("SELECT title, sort_order FROM tasks ORDER BY title")
        .unwrap();
    let task_rows: Vec<(String, Option<i64>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(task_rows[0].0, "done-already-sorted");
    assert_eq!(
        task_rows[0].1,
        Some(-999),
        "an already-set sort_order must not be overwritten"
    );
    assert_eq!(task_rows[1].0, "done-no-sort-order");
    assert!(
        task_rows[1].1.is_some_and(|so| so < 0),
        "a null sort_order on a Done task must be backfilled to a negative value, got {:?}",
        task_rows[1].1
    );
    assert_eq!(task_rows[2].0, "not-done");
    assert_eq!(
        task_rows[2].1, None,
        "a non-Done task's null sort_order must be left alone"
    );

    let mut estmt = conn
        .prepare("SELECT title, sort_order FROM epics ORDER BY title")
        .unwrap();
    let epic_rows: Vec<(String, Option<i64>)> = estmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(epic_rows[0].0, "epic-done-no-sort-order");
    assert!(epic_rows[0].1.is_some_and(|so| so < 0));
    assert_eq!(epic_rows[1].0, "epic-not-done");
    assert_eq!(epic_rows[1].1, None);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --lib db::tests::migrations::v79_backfills_sort_order_for_done_tasks_and_epics`
Expected: FAIL with "cannot find function `migrate_v79_backfill_done_sort_order`"

- [ ] **Step 3: Implement the migration**

Add to `src/db/migrations.rs`, after `migrate_v78_create_task_watchers` (after the function that ends around line 1129):

```rust
/// Backfills `sort_order` for tasks and epics already sitting in `Done`
/// status, using their existing `updated_at` as an approximation of
/// completion time (no real completion timestamp exists for historical
/// data — see the Done-column completion-order design doc). Only fills in
/// `NULL` values; never overwrites an already-set `sort_order` (e.g. one
/// set by a prior manual reorder). Going forward, live transitions use
/// millisecond precision (`sort_order_for_status_transition`); this
/// backfill is deliberately seconds-scale, matching `updated_at`'s storage
/// precision — see the design doc for why mixing scales here is correct,
/// not a bug.
pub(super) fn migrate_v79_backfill_done_sort_order(conn: &Connection) -> Result<()> {
    let tasks_updated = conn
        .execute(
            "UPDATE tasks SET sort_order = -CAST(strftime('%s', updated_at) AS INTEGER)
             WHERE status = 'done' AND sort_order IS NULL",
            [],
        )
        .context("Failed to backfill sort_order for Done tasks (migration v79)")?;
    if tasks_updated > 0 {
        tracing::info!("Migration v79: backfilled sort_order for {tasks_updated} Done task(s)");
    }

    let epics_updated = conn
        .execute(
            "UPDATE epics SET sort_order = -CAST(strftime('%s', updated_at) AS INTEGER)
             WHERE status = 'done' AND sort_order IS NULL",
            [],
        )
        .context("Failed to backfill sort_order for Done epics (migration v79)")?;
    if epics_updated > 0 {
        tracing::info!("Migration v79: backfilled sort_order for {epics_updated} Done epic(s)");
    }

    Ok(())
}
```

Register it in `MIGRATIONS` (after `(78, migrate_v78_create_task_watchers),` at line 139):

```rust
    (79, migrate_v79_backfill_done_sort_order),
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --lib db::tests::migrations::`
Expected: PASS — the new test, plus `fresh_db_has_latest_schema_version` (which derives `LATEST_SCHEMA_VERSION` from the array automatically, so it needs no manual update) and every other migration test.

- [ ] **Step 5: Commit**

```bash
git add src/db/migrations.rs src/db/tests/migrations.rs
git commit -m "feat: migration v79 backfills sort_order for existing Done rows

One-time approximation from updated_at for tasks/epics already in Done
before this feature shipped; never touches an already-set sort_order."
```

---

### Task 8: TUI regression tests — rendering and manual reorder

No production TUI code changes in this task — this is confirmation-only, per the design's "no comparator changes needed" claim, expressed as tests.

**Files:**
- Test: `src/tui/tests/epics.rs` (rendering order)
- Test: `src/tui/tests/rendering.rs` (manual reorder within Done)

**Interfaces:** none (uses existing `make_app`/`make_task`/`ColumnItem` — no new production code).

- [ ] **Step 1: Write the tests**

Add to `src/tui/tests/epics.rs`, near `column_items_null_sort_order_uses_id` (after line 928):

```rust
#[test]
fn done_column_sorts_by_completion_recency_via_sort_order() {
    let mut app = make_app();
    // sort_order values as the service layer would set them: negative
    // milliseconds, more negative = more recently completed.
    let mut older = make_task(1, TaskStatus::Done);
    older.title = "Completed first".to_string();
    older.sort_order = Some(-1_700_000_000_000);
    let mut newer = make_task(2, TaskStatus::Done);
    newer.title = "Completed second".to_string();
    newer.sort_order = Some(-1_700_000_100_000);
    app.board.tasks = vec![older, newer];

    let items = app.column_items_for_status(TaskStatus::Done);
    assert_eq!(items.len(), 2);
    match &items[0] {
        ColumnItem::Task(t) => assert_eq!(
            t.title, "Completed second",
            "the more recently completed task must render first"
        ),
        _ => panic!("expected task"),
    }
    match &items[1] {
        ColumnItem::Task(t) => assert_eq!(t.title, "Completed first"),
        _ => panic!("expected task"),
    }
}
```

Add to `src/tui/tests/rendering.rs`, near `reorder_task_up_swaps_sort_order` (after its closing brace, around line 1005 — check the exact end of that test first):

```rust
#[tokio::test]
async fn reorder_task_down_swaps_sort_order_within_done_column() {
    let mut app = make_app();
    let mut t1 = make_task(1, TaskStatus::Done);
    t1.sort_order = Some(-1_700_000_000_000);
    let mut t2 = make_task(2, TaskStatus::Done);
    t2.sort_order = Some(-1_700_000_100_000);
    app.board.tasks = vec![t1, t2];
    app.selection_mut().set_column(4); // Done column
    app.selection_mut().set_row(4, 0);

    let cmds = app.update(Message::Task(
        crate::tui::messages::TaskMessage::ReorderItem(1),
    ));

    let t1 = app.find_task(TaskId(1)).unwrap();
    let t2 = app.find_task(TaskId(2)).unwrap();
    let eff1 = t1.sort_order.unwrap_or(t1.id.0);
    let eff2 = t2.sort_order.unwrap_or(t2.id.0);
    assert!(
        eff1 > eff2,
        "task 1 ({eff1}) should be after task 2 ({eff2}) after move down"
    );
    assert_eq!(
        cmds.iter()
            .filter(|c| matches!(
                c,
                Command::Task(crate::tui::commands::TaskCommand::Persist(_))
            ))
            .count(),
        2
    );
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test --lib tui::tests::epics::done_column_sorts_by_completion_recency_via_sort_order tui::tests::rendering::reorder_task_down_swaps_sort_order_within_done_column`
Expected: PASS immediately — no production code changed in this task, this confirms the design's "no rendering/comparator changes needed" claim and guards it against future regression.

- [ ] **Step 3: Commit**

```bash
git add src/tui/tests/epics.rs src/tui/tests/rendering.rs
git commit -m "test: confirm Done column renders by recency, reorder still works

No production code change — column_items_for_status/handle_reorder_item
already sort by (sort_order, id) unconditionally; these tests pin down
that the completion-order sort_order values populated by the service
layer render and reorder correctly with zero comparator changes."
```

---

### Task 9: Allium spec updates

**Files:**
- Modify: `docs/specs/tasks.allium` (`ConfirmDone`, `MoveTaskBackward`, `ArchiveTask` rules)
- Modify: `docs/specs/pr-workflow.allium` (`PrMerged`, `PrClosed`, `FinishTaskSuccess`, `ExitSession` rules)
- Modify: `docs/specs/epics.allium` (`EpicStatusRecalculation` invariant prose, `MoveEpicForward`, `MoveEpicBackward`, `ArchiveEpic`, `UpdateEpicViaMcp` rules)
- Modify: `docs/specs/feeds.allium` (near the `UpsertFeedTasks` rule)

Per this repo's CLAUDE.md, Allium spec syntax is authored via the `allium:tend` skill (which validates with `allium check`/`allium analyse`), not hand-edited — guessing at spec syntax risks producing invalid or inconsistent spec files.

- [ ] **Step 1: Invoke `allium:tend`**

Use the `allium:tend` skill with this brief:

> Add the Done-column completion-order invariant across four spec files, reflecting a shipped code change (already implemented and tested in `src/models/tasks.rs`'s `sort_order_for_status_transition`, `src/service/tasks/crud.rs`, `src/service/epics.rs`, and `src/db/queries/epics.rs`):
>
> **`docs/specs/tasks.allium`:**
> - `ConfirmDone` (review → done): add an `ensures` clause that `task.sort_order` is set to reflect completion recency (most-recently-completed sorts first; implementation detail: negated current timestamp in milliseconds — same mechanism referenced below for the other Done-entry rules).
> - `MoveTaskBackward`: when the task's prior status was `done` (i.e. `prev = review`), add an `ensures` clause that `task.sort_order` is cleared to null.
> - `ArchiveTask`: when the task's prior status was `done`, add an `ensures` clause that `task.sort_order` is cleared to null.
>
> **`docs/specs/pr-workflow.allium`:**
> - `PrMerged`, `PrClosed`, `FinishTaskSuccess` (all review → done): each needs the same completion-recency `ensures` clause as `ConfirmDone`.
> - `ExitSession`: only the `else` branch (`action != pr`, which sets `task.status = done`) needs the completion-recency `ensures` clause; the `pr` branch (which sets `task.status = review`) does not enter Done and needs nothing.
>
> **`docs/specs/epics.allium`:**
> - The `EpicStatusRecalculation` invariant (prose block near the end of the file): add a sentence noting that the two auto-transitions (all-active-children-done → done; done-with-active-non-done-children → backlog) also set/clear `epic.sort_order` using the identical rule as tasks, and that a no-op recalculation (status doesn't change) never touches `sort_order`.
> - `MoveEpicForward`: when `next = done`, add an `ensures` clause for the completion-recency `sort_order` set (this rule allows manual forward movement directly into Done, unlike `MoveTaskForward` which excludes Done).
> - `MoveEpicBackward`: when the epic's prior status was `done`, add an `ensures` clause clearing `sort_order`.
> - `ArchiveEpic`: when the epic's prior status was `done`, add an `ensures` clause clearing `sort_order` (note `ArchiveEpic` sets `epic.status = archived` unconditionally today with no prior-status guard in the rule text — the sort_order clause should be conditional on the prior status being done).
> - `UpdateEpicViaMcp`: add an `ensures` clause covering both directions (entering/leaving done via the `status` param), matching `MoveEpicForward`/`MoveEpicBackward`'s semantics.
>
> **`docs/specs/feeds.allium`:** near the `UpsertFeedTasks` rule, add a note that the re-poll `sort_order` update is skipped when the task's current status is `done`, so a completion-order value already on a task is never clobbered by a feed's severity-rank re-poll (only newly-inserted tasks, or re-polls of non-Done tasks, take the feed's `sort_order`).

- [ ] **Step 2: Verify alignment**

Use the `allium:weed` skill to check the four modified spec files against the implementation from Tasks 1-6 (`src/models/tasks.rs`, `src/service/tasks/crud.rs`, `src/service/epics.rs`, `src/db/queries/epics.rs`, `src/db/queries/tasks.rs`). Resolve any divergence it reports before proceeding.

- [ ] **Step 3: Commit**

```bash
git add docs/specs/tasks.allium docs/specs/pr-workflow.allium docs/specs/epics.allium docs/specs/feeds.allium
git commit -m "docs: spec the Done-column completion-order invariant

Covers every Done-entry/exit rule across tasks, epics, pr-workflow, and
the feed re-poll guard."
```

---

## Final Verification

- [ ] Run the full suite and the doc-path checker:

```bash
cargo test
./scripts/check-doc-paths.sh
```

- [ ] Run `cargo fmt` and `cargo clippy --all-targets -- -D warnings` (what the pre-push hook runs) to catch anything the individual task steps missed:

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
```

If clippy flags anything, fix it in place rather than suppressing.
