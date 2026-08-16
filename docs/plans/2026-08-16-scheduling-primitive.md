# Generic Scheduling Primitive — Implementation Plan

> **For agentic workers:** Use TDD throughout — write the failing test before the code that makes it pass. Update `docs/specs/core.allium` and `docs/specs/dispatch.allium` (and `docs/specs/repo-sync.allium` for the one cross-reference noted below) via the `allium:tend` skill as part of this work, then verify with `allium:weed`.

**Goal:** Let dispatch periodically (re)dispatch a task on its own, and let a task's worktree optionally track an existing branch literally instead of creating a fresh per-task branch.

**Architecture:** Four new nullable `Task` columns (migration v88); a third `BaseRef` variant (`Pinned`) with its own `git worktree add` code path (checks out the literal branch, no `-B`); a new `DispatchScheduledTask`-equivalent Rust function that performs a **fresh** dispatch (not `resume_agent`'s `--continue`) for idle scheduled tasks; a new background `SchedulerRunner` tokio loop, structurally parallel to `FeedRunner`, that gates on elapsed time and skips the git-fetch-and-dispatch work entirely when nothing has changed on the pinned branch.

**Tech Stack:** Rust, rusqlite (via `tokio_rusqlite::Connection`), tokio background task, existing `ProcessRunner`/`MockProcessRunner` test harness.

**Spec:** `docs/superpowers/specs/2026-08-16-staging-pipeline-scheduled-agents-design.md` (Part A)

## Global Constraints

- Every new nullable-column migration is a plain additive `ALTER TABLE tasks ADD COLUMN` — no `BEGIN`/`COMMIT` wrapping (the migration runner already wraps it), no `CHECK` constraints.
- `DispatchTask`, `ResumeTask`, `RetryResume`, and `ExitSession` must NOT be modified by this plan — the scheduling primitive is purely additive (new rule, new function, new columns). Do not loosen `DispatchTask`'s `status = backlog` precondition.
- Every subprocess this plan adds must go through `ProcessRunner` (never raw `std::process::Command`) so it's testable via `MockProcessRunner`, and must carry a bounded timeout — no unbounded git call.
- `resume_agent` (`src/dispatch/agents.rs:503`) must NOT be reused for scheduled ticks — it does `claude --continue`, resuming a stale conversation. Scheduled ticks need a fresh prompt each time, via a new function built the same way as `research_agent`/`quick_dispatch_agent` (`agents.rs:383-424`), which call the shared `dispatch_with_prompt` (`agents.rs:239`).

---

## File Structure

- Modify `src/models/tasks.rs` — 4 new `Task` fields.
- Create `src/db/migrations.rs` — `migrate_v88_add_scheduling_fields`, registered in `MIGRATIONS`.
- Modify `src/db/mod.rs` — `TaskPatch` gains 4 nullable fields (mirroring `wrap_up_mode` at `:109`); `CreateTaskRequest` gains `schedule_interval_secs` / `pinned_branch`.
- Modify `src/dispatch/worktree.rs` — new `BaseRef::Pinned(&str)` variant (alongside `Branch`/`PrHead` at `:260-268`) and its own `git worktree add` code path inside `provision_worktree` (`:300`, the `if reused_worktree {..} else {..}` block at `:400-431`).
- Modify `src/dispatch/agents.rs` — new `pub fn pipeline_agent(task: &Task, runner: &dyn ProcessRunner) -> Result<DispatchResult>`, built like `research_agent`/`quick_dispatch_agent` but selecting `BaseRef::Pinned` when `task.pinned_branch` is set (this requires `dispatch_with_prompt`, `agents.rs:239`, to grow a branch for that case — see Task 2 below).
- Create `src/scheduler/mod.rs` — new `SchedulerRunner`, modeled on `src/feed/mod.rs`'s `FeedRunner` (tick loop, `last_scheduled_check: HashMap<TaskId, Instant>`, elapsed-gate-before-spawn, background-spawned per-task work).
- Modify `src/main.rs` (or wherever `FeedRunner` is started alongside the TUI) — start `SchedulerRunner` the same way.
- Test: `src/db/tests/migrations.rs`, `src/dispatch/tests.rs` (or a new `src/dispatch/worktree_tests.rs` if that's where `provision_worktree` tests already live — check first), `src/scheduler/mod.rs` inline `mod tests`.

**Interfaces produced for sibling subtasks:**
- `Task.schedule_interval_secs: Option<i64>`, `Task.pinned_branch: Option<String>`, `Task.last_processed_sha: Option<String>`, `Task.last_scheduled_check_at: Option<DateTime<Utc>>` — subtask B's TUI editor sets `schedule_interval_secs`/`pinned_branch` through the same `TaskPatch`/editor-diff mechanism used for every other field.
- `pub fn pipeline_agent(task: &Task, runner: &dyn ProcessRunner) -> Result<DispatchResult>` in `src/dispatch/agents.rs` — subtask C's `wrap_up(merge)` work does not call this directly, but the design doc's "auto-push scoped to the pipeline" step (recording `last_processed_sha` and pushing `base_branch`) hooks into whatever event `SchedulerRunner` emits when a scheduled+pinned task's session closes successfully — implement that hook here too (Task 6 below), even though `wrap_up(merge)` itself (subtask C) is a separate task.

---

## Task 1: `Task` model + migration v88

**Files:**
- Modify: `src/models/tasks.rs:344-390` (Task struct), and its `Default`/test-builder impl if one exists (grep for `last_pre_tool_use_at: None,` at `:1877` — the same struct-literal test helper needs the 4 new fields).
- Create/Modify: `src/db/migrations.rs` (append `migrate_v88_add_scheduling_fields`, register `(88, migrate_v88_add_scheduling_fields)` following the `(87, migrate_v87_add_peer_message_columns)` entry at `:150`).
- Test: `src/db/tests/migrations.rs` (new test following the `migration_v52_adds_verify_command_to_repo_paths` pattern).

- [ ] **Step 1: Write the failing migration test**

```rust
#[test]
fn migration_v88_adds_scheduling_fields_to_tasks() {
    let conn = seed_schema_before(87); // however the existing v52 test seeds a pre-migration schema
    migrate_v88_add_scheduling_fields(&conn).expect("migration should succeed");

    let mut stmt = conn
        .prepare("SELECT schedule_interval_secs, pinned_branch, last_processed_sha, last_scheduled_check_at FROM tasks LIMIT 0")
        .expect("new columns should exist");
    // Existing rows get NULL for all four columns.
    let cols: Vec<String> = stmt.column_names().into_iter().map(String::from).collect();
    assert!(cols.contains(&"schedule_interval_secs".to_string()));
    assert!(cols.contains(&"pinned_branch".to_string()));
    assert!(cols.contains(&"last_processed_sha".to_string()));
    assert!(cols.contains(&"last_scheduled_check_at".to_string()));
}
```

- [ ] **Step 2: Run test, confirm it fails** with "no such column" or similar.

- [ ] **Step 3: Implement the migration**

```rust
pub(super) fn migrate_v88_add_scheduling_fields(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "ALTER TABLE tasks ADD COLUMN schedule_interval_secs INTEGER;
         ALTER TABLE tasks ADD COLUMN pinned_branch TEXT;
         ALTER TABLE tasks ADD COLUMN last_processed_sha TEXT;
         ALTER TABLE tasks ADD COLUMN last_scheduled_check_at TEXT;",
    )
}
```

Register in `MIGRATIONS`: `(88, migrate_v88_add_scheduling_fields),`

- [ ] **Step 4: Run test, confirm it passes.**

- [ ] **Step 5: Add the 4 fields to the `Task` struct**

```rust
pub schedule_interval_secs: Option<i64>,
pub pinned_branch: Option<String>,
pub last_processed_sha: Option<String>,
pub last_scheduled_check_at: Option<DateTime<Utc>>,
```

Add them (as `None`) to every existing `Task { .. }` struct-literal test builder that currently lists every field explicitly (grep `last_pre_tool_use_at: None,` to find them all — do not use `..Default::default()` if the existing helpers don't already use it, to stay consistent with the file's style).

- [ ] **Step 6: `cargo build` — fix every struct-literal call site the compiler flags** (exhaustive field lists mean the compiler will list every location).

- [ ] **Step 7: Commit**

```bash
git add src/models/tasks.rs src/db/migrations.rs src/db/tests/migrations.rs
git commit -m "feat(db): add scheduling fields to Task (migration v88)"
```

---

## Task 2: `TaskPatch` / `CreateTaskRequest` wiring

**Files:**
- Modify: `src/db/mod.rs:77-132` (`TaskPatch` macro invocation, `CreateTaskRequest`).
- Test: wherever existing `TaskPatch` round-trip tests for `wrap_up_mode`/`sort_order` live (grep `nullable wrap_up_mode`'s test coverage) — add the same shape of test for the 4 new fields.

**Interfaces:**
- Consumes: `Task.schedule_interval_secs: Option<i64>` etc. from Task 1.
- Produces: `TaskPatch { schedule_interval_secs: Option<Option<i64>>, pinned_branch: Option<Option<String>>, last_processed_sha: Option<Option<String>>, last_scheduled_check_at: Option<Option<DateTime<Utc>>>, .. }` — subtask B's TUI editor code patches through these.

- [ ] **Step 1: Write the failing patch round-trip test** (mirror the existing `wrap_up_mode` patch test exactly, substituting field names).

- [ ] **Step 2: Run test, confirm it fails to compile** (fields don't exist on `TaskPatch` yet).

- [ ] **Step 3: Add the 4 fields to the `patch_struct!` invocation**

```rust
nullable schedule_interval_secs: i64,
nullable pinned_branch: &'a str,
nullable last_processed_sha: &'a str,
nullable last_scheduled_check_at: chrono::DateTime<chrono::Utc>,
```

Add `schedule_interval_secs: Option<i64>` and `pinned_branch: Option<String>` to `CreateTaskRequest<'a>` (`:120-132`) too — these should be settable at creation, matching the design doc's `create_task(... schedule_interval_secs: 600, pinned_branch: "staging" ...)` example. `last_processed_sha`/`last_scheduled_check_at` are NOT creation-time fields (they only ever get written by the scheduler/merge-progress code), so they stay off `CreateTaskRequest`.

- [ ] **Step 4: Run test, confirm it passes.**

- [ ] **Step 5: Commit**

```bash
git add src/db/mod.rs
git commit -m "feat(db): thread scheduling fields through TaskPatch/CreateTaskRequest"
```

---

## Task 3: `BaseRef::Pinned` worktree carve-out

**Files:**
- Modify: `src/dispatch/worktree.rs:260-268` (`BaseRef` enum), `:300-431` (`provision_worktree`).
- Test: wherever `provision_worktree`'s existing `BaseRef::Branch`/`BaseRef::PrHead` tests live (same file's `mod tests`, or a sibling test file — check `src/dispatch/worktree.rs` for an inline `#[cfg(test)] mod tests` first).

**Interfaces:**
- Consumes: `BaseRef` enum (existing `Branch(&'a str)`, `PrHead(&'a str)`), `provision_worktree(task: &Task, runner: &dyn ProcessRunner, base: Option<BaseRef<'_>>, timeout: Duration) -> Result<ProvisionResult>` (exact existing signature — read `src/dispatch/worktree.rs:300` before writing this task's code to confirm field/type names haven't drifted).
- Produces: `BaseRef::Pinned(&'a str)` variant; `ProvisionResult` unchanged in shape, but for a `Pinned` base its `worktree_path` is still `<repo_path>/.worktrees/<task.id>-<slug>` (task-id-keyed, unchanged) while the branch actually checked out inside it is the literal pinned branch.

- [ ] **Step 1: Write the failing test** — using `MockProcessRunner`, assert that provisioning a task with `BaseRef::Pinned("staging")` issues `git worktree add <path> staging` (no `-B`, no task-id-slug branch name anywhere in the argv), against a mock sequence where `staging` exists locally.

```rust
#[test]
fn provision_worktree_with_pinned_branch_checks_out_the_literal_branch() {
    let runner = MockProcessRunner::new(vec![
        // fetch origin staging (existing fetch-then-select dance, reused as-is)
        ok(),
        // git worktree add <path> staging  -- NOT `-B <id>-<slug> ... staging`
        ok(),
    ]);
    let task = test_task(); // existing test helper
    let result = provision_worktree(&task, &runner, Some(BaseRef::Pinned("staging")), Duration::from_secs(30))
        .expect("provisioning should succeed");
    let calls = runner.calls(); // however MockProcessRunner exposes recorded argv
    let worktree_add = calls.iter().find(|c| c.contains("worktree add")).expect("should call worktree add");
    assert!(worktree_add.contains("staging"));
    assert!(!worktree_add.contains("-B"));
}
```

- [ ] **Step 2: Run test, confirm it fails** (variant doesn't exist / compile error).

- [ ] **Step 3: Add the variant and the code fork**

```rust
pub(super) enum BaseRef<'a> {
    Branch(&'a str),
    PrHead(&'a str),
    Pinned(&'a str),
}
```

Inside `provision_worktree`'s fresh-provisioning branch (where today's code always does `git worktree add <path> -B <worktree_name> <selected_ref>`), fork on the base:

```rust
match base {
    Some(BaseRef::Pinned(branch)) => {
        // No -B: check out the literal existing branch, never a derived one.
        // The branch must already exist (locally, from a prior tick, or via
        // origin/<branch> fetched by the existing fetch-then-select dance
        // above, which BaseRef::Pinned reuses unchanged for its fetch/select
        // half — only this final `git worktree add` invocation differs).
        run_bounded(runner, repo_path, &["worktree", "add", &worktree_path, branch], timeout)?;
    }
    _ => {
        // existing `-B <worktree_name> <selected_ref>` path, unchanged
    }
}
```

(Read the actual current fresh-provisioning code at `worktree.rs:400-431` before writing this — match its existing helper names, e.g. whatever wraps `run_bounded`/`ProcessRunner::run`, exactly.)

- [ ] **Step 4: Run test, confirm it passes.**

- [ ] **Step 5: Add a reused-worktree test** — a second tick where the worktree directory already exists must skip `git worktree add` entirely (this already falls out of the existing `reused_worktree` check at `:317`, but add a regression test asserting no `worktree add` call happens on the second provision of the same pinned-branch task).

- [ ] **Step 6: Commit**

```bash
git add src/dispatch/worktree.rs
git commit -m "feat(dispatch): add BaseRef::Pinned worktree carve-out for pinned-branch tasks"
```

---

## Task 4: Fresh-dispatch function for pinned/scheduled tasks

**Files:**
- Modify: `src/dispatch/agents.rs:239-352` (`dispatch_with_prompt`), `:354-424` (add `pipeline_agent` alongside `dispatch_agent`/`research_agent`/`quick_dispatch_agent`).
- Test: wherever `dispatch_agent`/`research_agent` are tested (grep for their existing test module).

**Interfaces:**
- Consumes: `BaseRef::Pinned` from Task 3; `task.pinned_branch: Option<String>` from Task 1.
- Produces: `pub fn pipeline_agent(task: &Task, runner: &dyn ProcessRunner) -> Result<DispatchResult>`.

- [ ] **Step 1: Write the failing test** — dispatching a task with `pinned_branch = Some("staging")` via `pipeline_agent` should provision with `BaseRef::Pinned("staging")`, not `BaseRef::Branch`/`BaseRef::PrHead`, and must NOT send a `claude ... --continue` command (assert the sent keys contain the fresh-launch `bash -c '...' claude` shape from `dispatch_with_prompt`, not `resume_agent`'s `--continue` shape).

- [ ] **Step 2: Run test, confirm it fails.**

- [ ] **Step 3: Implement.** Inside `dispatch_with_prompt` (`agents.rs:239`), the existing base-ref selection block:

```rust
let base_ref = match pr_branch.as_deref() {
    Some(branch) => BaseRef::PrHead(branch),
    None => BaseRef::Branch(&resolved),
};
```

needs a preceding check for `task.pinned_branch`:

```rust
let base_ref = match (task.pinned_branch.as_deref(), pr_branch.as_deref()) {
    (Some(pinned), _) => BaseRef::Pinned(pinned),
    (None, Some(branch)) => BaseRef::PrHead(branch),
    (None, None) => BaseRef::Branch(&resolved),
};
```

(`pinned_branch` takes priority — a pinned-branch task is never also a PR-review task in practice, but if both were somehow set, pinning must win since it's the more specific configuration.)

Then add the new entry point, mirroring `research_agent`:

```rust
pub fn pipeline_agent(task: &Task, runner: &dyn ProcessRunner) -> Result<DispatchResult> {
    dispatch_with_prompt(
        task,
        || build_pipeline_prompt(task.id, &task.title, &task.pinned_branch, &task.base_branch),
        runner,
        Some(&task.base_branch),
        None,
    )
}
```

(`build_pipeline_prompt` itself is written in a later step of this task or deferred to subtask C/the pipeline prompt work — for THIS task, a minimal placeholder-free stub is fine as long as it produces real, non-empty prompt text; do not leave a `todo!()`. Minimal acceptable version:

```rust
fn build_pipeline_prompt(id: TaskId, title: &str, pinned_branch: &Option<String>, base_branch: &str) -> String {
    let branch = pinned_branch.as_deref().unwrap_or("(none)");
    format!(
        "Your task is:\n\nTask:\n  ID: {id}\n  Title: {title}\n\n\
         This is a recurring pipeline task tracking branch `{branch}`. New commits \
         have landed on it since your last run. Run the full verify command (see \
         `get_task`'s \"Full verify command\" line), fix any failures with ordinary \
         commits directly on this branch, then call wrap_up(action=\"merge\") \
         targeting base_branch `{base_branch}`, followed by exit_session."
    )
}
```

This is intentionally minimal here — the polished, spec-complete prompt (skipping the plan/epic addenda, matching the unified-skeleton discipline) is this same file's responsibility but can be refined once subtask C's `wrap_up(merge)` and subtask D's `full_verify_command` land; this task's job is only to prove the fresh-dispatch-with-pinned-branch wiring works end-to-end.)

- [ ] **Step 4: Run test, confirm it passes.**

- [ ] **Step 5: Commit**

```bash
git add src/dispatch/agents.rs
git commit -m "feat(dispatch): add pipeline_agent — fresh dispatch for pinned-branch tasks"
```

---

## Task 5: `SchedulerRunner` background loop

**Files:**
- Create: `src/scheduler/mod.rs`.
- Modify: wherever `FeedRunner` is constructed and spawned at startup (grep `FeedRunner::new` — likely `src/runtime/mod.rs` or `src/main.rs`) — start `SchedulerRunner` alongside it.
- Test: inline `mod tests` in `src/scheduler/mod.rs`, mirroring `src/feed/mod.rs`'s `tick_does_not_block_event_loop` and `tick_skips_an_epic_whose_cycle_is_already_in_flight` tests.

**Interfaces:**
- Consumes: `Task.schedule_interval_secs`, `Task.pinned_branch`, `Task.last_processed_sha`, `Task.last_scheduled_check_at` (Task 1); `pipeline_agent` (Task 4); a `TaskReadStore`/`TaskCrud`-style DB handle to list tasks with `schedule_interval_secs != null` and to bump `last_scheduled_check_at`.
- Produces: `pub struct SchedulerRunner { .. }` with a `pub async fn tick(&mut self)` (or however `FeedRunner`'s method is shaped — match its exact signature) and a way to start it as a background tokio task, e.g. `pub fn spawn(db: Arc<dyn ...>, runner: Arc<dyn ProcessRunner>) -> tokio::task::JoinHandle<()>`.

- [ ] **Step 1: Write the failing test — tick skips a task whose branch hasn't changed**

```rust
#[tokio::test]
async fn tick_skips_dispatch_when_pinned_branch_unchanged() {
    // Seed a task with schedule_interval_secs = 1, pinned_branch = "staging",
    // last_processed_sha = Some("abc123"), last_scheduled_check_at far in the past.
    // MockProcessRunner sequence: only ONE call expected (the lightweight
    // `git fetch`/`rev-parse origin/staging` check), which returns "abc123" —
    // i.e. unchanged. Assert no `worktree add` / no tmux/claude launch call happens.
}
```

- [ ] **Step 2: Run test, confirm it fails** (function doesn't exist yet).

- [ ] **Step 3: Implement `SchedulerRunner`**, mirroring `FeedRunner`'s shape (`src/feed/mod.rs:171-340`):

```rust
pub struct SchedulerRunner {
    last_scheduled_check: HashMap<TaskId, Instant>,
    db: Arc<dyn crate::db::TaskCrud>, // exact trait name — confirm against src/db/mod.rs
    runner: Arc<dyn ProcessRunner>,
}

const SCHEDULER_POLL_INTERVAL: Duration = Duration::from_secs(2); // independent constant, same reasoning as FEED_POLL_INTERVAL vs TICK_INTERVAL

impl SchedulerRunner {
    pub async fn tick(&mut self) {
        let tasks = self.db.list_scheduled_tasks().await.unwrap_or_default(); // new DB query: schedule_interval_secs IS NOT NULL AND status IN ('backlog','done') AND tmux_window IS NULL
        for task in tasks {
            let Some(interval) = task.schedule_interval_secs else { continue };
            let elapsed = self
                .last_scheduled_check
                .get(&task.id)
                .map(|t| t.elapsed())
                .unwrap_or(Duration::MAX);
            if elapsed < Duration::from_secs(interval as u64) {
                continue;
            }
            self.last_scheduled_check.insert(task.id, Instant::now());
            let db = self.db.clone();
            let runner = self.runner.clone();
            tokio::spawn(async move { Self::check_and_dispatch(db, runner, task).await });
        }
    }

    async fn check_and_dispatch(db: Arc<dyn crate::db::TaskCrud>, runner: Arc<dyn ProcessRunner>, task: Task) {
        if let Some(pinned) = task.pinned_branch.clone() {
            // Lightweight check: fetch + rev-parse origin/<pinned>, compare to last_processed_sha.
            let current_sha = fetch_and_resolve_sha(&task.repo_path, &pinned, runner.as_ref());
            match (current_sha, task.last_processed_sha.as_deref()) {
                (Ok(sha), Some(last)) if sha == last => {
                    let _ = db.bump_last_scheduled_check(task.id).await;
                    return; // nothing new — no agent dispatched, no cost beyond one fetch
                }
                _ => {} // changed, or never processed — fall through to dispatch
            }
        }
        let _ = tokio::task::spawn_blocking(move || pipeline_agent(&task, runner.as_ref())).await;
        let _ = db.bump_last_scheduled_check(task.id).await;
    }
}
```

(Write `fetch_and_resolve_sha` as a small helper alongside this, using `ProcessRunner` for `git fetch origin <branch>` + `git rev-parse origin/<branch>`, bounded by a timeout constant — mirror `repo_sync.rs`'s style for a two-subprocess, bounded, `Result`-returning helper.)

Add the new DB query methods (`list_scheduled_tasks`, `bump_last_scheduled_check`) to whichever trait owns task mutations (`TaskCrud` per the mutation-boundary rule in CLAUDE.md — `state.db` is read-only `TaskReadStore`, so these are new methods added to the write-side trait, not raw SQL from the scheduler module).

- [ ] **Step 4: Run test, confirm it passes.**

- [ ] **Step 5: Write the "due and changed" test** — same setup but the mock fetch/rev-parse returns a SHA different from `last_processed_sha`; assert `pipeline_agent`'s dispatch path IS invoked (a `worktree add`/tmux-launch call happens).

- [ ] **Step 6: Write the "never processed" test** — `last_processed_sha = None`; assert dispatch happens regardless of what the fetch returns (first run always dispatches).

- [ ] **Step 7: Write the "not yet due" test** — `last_scheduled_check_at` recent, `elapsed < interval`; assert `tick()` makes zero subprocess calls at all (not even the lightweight fetch) for that task.

- [ ] **Step 8: Wire `SchedulerRunner::spawn` into startup**, alongside `FeedRunner`'s own spawn point.

- [ ] **Step 9: Commit**

```bash
git add src/scheduler/mod.rs src/db/mod.rs src/main.rs # or wherever startup wiring lives
git commit -m "feat(scheduler): add SchedulerRunner background loop for scheduled/pinned-branch tasks"
```

---

## Task 6: `docs/specs` alignment

- [ ] Use `allium:tend` to add the 4 new `Task` fields to `core.allium`'s `Task` entity, and a new `DispatchScheduledTask` rule to `dispatch.allium` (mirroring the shape of `ResumeTask`/`RetryResume`, with the `requires: task.status in {backlog, done}` precondition and the skip-if-unchanged guidance from the design doc).
- [ ] Run `allium:weed` to confirm the spec now matches Tasks 1-5's implementation, and fix any drift found.
- [ ] Commit the spec changes separately from code (`docs: update dispatch.allium/core.allium for scheduling primitive`).

---

## Self-Review Notes (for whoever executes this plan)

- `pipeline_agent`'s prompt (Task 4, Step 3) is deliberately minimal — do not treat it as final. Subtask C (wrap_up merge) and subtask D (verify tiering) will each need to extend it (pointing at `full_verify_command`, referencing `wrap_up(action="merge")` by its real, landed name). Leave a short comment noting this in the code, not a `TODO`/`unimplemented!()`.
- Auto-push (design doc's "Auto-push completes the loop" — pushing `base_branch` to origin after a successful pinned-branch merge) is explicitly OUT of this plan's scope; it depends on subtask C's `wrap_up(merge)` landing and on `BranchMerged`/`SessionClosed` existing. Track it as a follow-up task once C is done, not silently skipped — mention it in the epic when this task closes.
