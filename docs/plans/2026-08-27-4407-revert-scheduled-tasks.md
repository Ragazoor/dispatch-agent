# Revert the scheduled-tasks work (epic #289)

> **Task #4407.** The user wants the landed work of epic #289 ("Staging pipeline &
> scheduled agents") removed. A similar abstraction may be designed later, from
> scratch — so this is a removal, not a deprecation. Nothing is left behind
> "just in case".

**Goal:** Remove the generic scheduling primitive (`Task.schedule_interval_secs`,
`pinned_branch`, `last_processed_sha`, `last_scheduled_check_at`,
`SchedulerRunner`, `BaseRef::Pinned`, `pipeline_agent`) and every surface that
exposes it — DB, MCP, service, TUI, editor, prompts, specs and docs.

**Verify command:** `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`

---

## What actually landed

Only two of the epic's eight subtasks reached `main`:

| Task | What landed | Commits |
| --- | --- | --- |
| #4203 A: Generic scheduling primitive | migration v88, four `Task` columns, `BaseRef::Pinned`, `pipeline_agent`, `SchedulerRunner`, `remote_branch_sha`, MCP + service plumbing | `cecdd924`, `76d75977`, `49f3cd6b`, `c34e18c3`, `45e7cae2`, `f0e214c1`, `cfb5b00c`, `8fe96fa9` |
| #4204 B: TUI create/edit | creation-form schedule gate + two steps, editor sections, card badge, `src/models/interval.rs` extraction | `0b9ce886`, `fa2950be`, `14f04bb3` |

Never landed (plan docs only): #4205 `wrap_up(action="merge")`, #4206
verify-command tiering, #4230, #4233, #4234.

**Not a `git revert`.** Later commits on `main` (`84659245` boxing the
Message/Command payloads, `20758469` `PersistFields`, `a559b337` layout-cache
self-healing) rewrote the same call sites, so replaying eleven inverse patches
would conflict throughout. This plan removes the feature from the *current*
tree instead, using the commit range only as a checklist.

## Two things that must survive the removal

1. **`src/models/interval.rs`** was created by `fa2950be`, but it is *shared*:
   `Epic.feed_interval_secs`'s editor section parses through
   `parse_interval_secs` and the epic form renders through
   `format_interval_secs`. Keep the module; remove only the task-schedule
   callers and the task-schedule mentions in its doc comments.
2. **`docs/specs/core.allium`'s "Interval literals" section** likewise binds
   both the epic feed surface and the task schedule surfaces. Keep the section,
   drop the two task-schedule bullets from its "Surfaces bound by this grammar"
   list, and delete the "KNOWN GAP: positivity is not enforced everywhere"
   block outright — that gap is a property of the MCP `schedule_interval_secs`
   path, which ceases to exist (it is what task #4233 tracked).

## Global constraints

- Spec first, then tests, then code — per `CLAUDE.md`.
- Removal only. No behaviour change to feeds, PR-head worktrees
  (`BaseRef::PrHead`), quick dispatch, or epic feed intervals.
- `TaskReadStore`/`TaskServiceApi` narrowing stays intact: deleting
  `list_scheduled_tasks` / `try_claim_scheduled_task` from the traits must
  leave both traits still object-safe and still implemented.
- Migration numbering: `v89` is the latest registered migration
  (`migrate_v89_allow_pr_closed_for_review`). The drop migration is therefore
  `v90` — but derive it from the last `MIGRATIONS` entry at execution time
  rather than trusting this line, in case a sibling task lands one first.

---

## Task 1: Retire the scheduling rules from the Allium specs

**Files:**
- Modify: `docs/specs/core.allium`, `docs/specs/tasks.allium`,
  `docs/specs/dispatch.allium`.

- [ ] **Step 1: `docs/specs/core.allium`.**
  - Delete the four `Task` fields (`schedule_interval_secs`, `pinned_branch`,
    `last_processed_sha`, `last_scheduled_check_at`).
  - Delete the "Scheduled badge" section.
  - In "Interval literals": drop the two `tasks.allium` bullets from "Surfaces
    bound by this grammar", drop `Task.schedule_interval_secs` from the
    bare-integer rationale, and delete the whole "KNOWN GAP" block.
  - Leave `pinned_task` / `pinned_task_id` alone — that is the agent-tree split
    pane, an unrelated use of the word.

- [ ] **Step 2: `docs/specs/tasks.allium`.**
  - `CreateTask`: drop `schedule_interval_secs?` / `pinned_branch?` from the
    trigger signature and the two `ensures` field assignments, and delete "The
    schedule step (TUI creation form tail)" guidance.
  - Quick-create guidance: delete the "always null here" paragraph.
  - Copy-task guidance: delete the "NOT copied either" paragraph and restore the
    creation-flow step list to its pre-schedule ordering.
  - `EditTask`: drop the two field assignments and the
    `SCHEDULE_INTERVAL_SECS` / `PINNED_BRANCH` section guidance.

- [ ] **Step 3: `docs/specs/dispatch.allium`.**
  - Delete the entire `-- == Scheduled dispatch ==` section and the
    `DispatchScheduledTask` rule with its guidance.
  - Delete the `PinnedWorktreeBranchIsNeverDerived` guarantee.
  - Leave `DispatchClaimExclusive` and the `BaseRef::PrHead` guidance intact,
    removing only their pinned-branch cross-references.

- [ ] **Step 4:** run `allium check` on the three files; then the
  `allium:weed` skill once the code is removed (Task 9), not before — the specs
  are deliberately ahead of the code between here and there.

---

## Task 2: Migration v90 — drop the columns and the index

**Files:**
- Modify: `src/db/migrations.rs`.
- Test: `src/db/tests/migrations.rs`.

- [ ] **Step 1: Write the failing migration test.** Mirror the shape of the
  existing drop-migration tests (e.g. the `project_id` drops around
  `src/db/migrations.rs:1941`). Assert that after the migration the four
  columns are gone from `tasks` **and** that `idx_tasks_scheduled` is gone from
  `sqlite_master`.

```rust
#[test]
fn migration_v90_drops_the_scheduling_fields() {
    let conn = seed_schema_before(90);
    migrate_v90_drop_scheduling_fields(&conn).expect("migration should succeed");
    let mut stmt = conn.prepare("SELECT * FROM tasks LIMIT 0").unwrap();
    let names = stmt.column_names();
    for column in [
        "schedule_interval_secs",
        "pinned_branch",
        "last_processed_sha",
        "last_scheduled_check_at",
    ] {
        assert!(!names.contains(&column), "{column} should be dropped");
    }
    let index_count: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='index' AND name='idx_tasks_scheduled'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(index_count, 0);
}
```

- [ ] **Step 2: Run the test, confirm it fails** (no such function).

- [ ] **Step 3: Implement `migrate_v90_drop_scheduling_fields`.**
  - `DROP INDEX IF EXISTS idx_tasks_scheduled` **first** — SQLite refuses
    `DROP COLUMN` on an indexed column, so the order is load-bearing, not
    stylistic. Say so in a comment.
  - Then `ALTER TABLE tasks DROP COLUMN <c>` for each of the four, each guarded
    by `column_exists` so the migration is idempotent and safe on a database
    that predates v88.
  - Register `(90, migrate_v90_drop_scheduling_fields)` in `MIGRATIONS`.
  - Leave `migrate_v88_add_scheduling_fields` in place and untouched — the
    chain is append-only; a database that has never run must still walk through
    v88 before it reaches v90.

- [ ] **Step 4: Run the test, confirm it passes.** Also re-run the existing v88
  test — it asserts the *add* and must keep passing.

---

## Task 3: `Task` model and the DB layer

**Files:**
- Modify: `src/models/tasks.rs`, `src/db/mod.rs`, `src/db/queries/mod.rs`,
  `src/db/queries/tasks.rs`.
- Test: `src/db/tests/tasks.rs`, `src/db/tests/migrations.rs`, and the
  `field: None,` fixtures in `src/db/tests/{mod,epics,todos}.rs`.

- [ ] **Step 1: Delete the DB tests that assert scheduling behaviour** — the
  `list_scheduled_tasks` and `try_claim_scheduled_task` cases in
  `src/db/tests/tasks.rs`, plus any round-trip test asserting the four columns
  survive a patch. Run `cargo test`, confirm it fails to compile (the removal is
  driven by the compiler from here).

- [ ] **Step 2: `src/models/tasks.rs`** — delete the four fields and their doc
  comments, and the four `None` initialisers in the test constructor
  (`src/models/tasks.rs:1981`).

- [ ] **Step 3: `src/db/mod.rs`**
  - Remove the four `nullable` entries from the `patch_struct!` invocation.
  - Remove `schedule_interval_secs` / `pinned_branch` from `CreateTaskRequest`.
  - Remove `list_scheduled_tasks` and `try_claim_scheduled_task` from the store
    trait(s), along with their doc comments.

- [ ] **Step 4: `src/db/queries/mod.rs`** — drop the four columns from the
  shared `TASK_COLUMNS` list and the four `row.get` decodes.

- [ ] **Step 5: `src/db/queries/tasks.rs`** — delete the `list_scheduled_tasks`
  and `try_claim_scheduled_task` implementations, the two `INSERT` columns and
  their bindings, and the four `set_field!` lines.

- [ ] **Step 6:** delete every `schedule_interval_secs: None,` /
  `pinned_branch: None,` / `last_processed_sha: None,` /
  `last_scheduled_check_at: None,` fixture line the compiler now flags. These
  are mechanical; roughly 600 of them across `src/` and `tests/`.

- [ ] **Step 7: `cargo test --lib db`** green.

---

## Task 4: Service layer

**Files:**
- Modify: `src/service/tasks/params.rs`, `src/service/tasks/crud.rs`,
  `src/service/tasks/validators.rs`, `src/service/tasks/dispatch.rs`,
  `src/service/api.rs`.
- Test: `src/service/tasks/tests.rs`.

- [ ] **Step 1: Delete the service tests** covering `claim_scheduled_task`,
  `stamp_scheduled_check`, and the two `UpdateTaskParams` builder cases in
  `params.rs`'s own test table (`src/service/tasks/params.rs:380`). Confirm red.

- [ ] **Step 2: `params.rs`** — remove the two double-`Option` fields from
  `UpdateTaskParams`, their builders, their entries in the "at least one field"
  check, and the two `CreateTaskParams` fields.

- [ ] **Step 3: `validators.rs`** — remove the two patch-application arms.

- [ ] **Step 4: `crud.rs`** — remove the two `CreateTaskRequest` fields, and
  delete `claim_scheduled_task` and `stamp_scheduled_check` entirely.

- [ ] **Step 5: `dispatch.rs`** — remove the `DispatchClaim::TakeScheduled`
  variant and its match arm. The `DispatchClaim` enum keeps its remaining
  variants; the dispatch seam's sequence is otherwise untouched.

- [ ] **Step 6:** remove the mirrored methods from `TaskServiceApi`
  (`src/service/api.rs`) and its test double.

---

## Task 5: Dispatch layer

**Files:**
- Modify: `src/dispatch/worktree.rs`, `src/dispatch/agents.rs`,
  `src/dispatch/prompts.rs`, `src/dispatch/mod.rs`, `src/git.rs`.
- Test: `src/dispatch/tests.rs`.

- [ ] **Step 1: Delete the dispatch tests** for `BaseRef::Pinned`,
  `pipeline_agent`, and the pipeline prompt. Confirm red.

- [ ] **Step 2: `worktree.rs`** — delete the `BaseRef::Pinned` variant. Each of
  the five `match` sites collapses back to the `Branch` / `PrHead` pair:
  - `BaseRef::branch_name` (line ~314)
  - the start-point selection (line ~382)
  - the create-worktree arm (line ~401)
  - the reuse arm (line ~441)
  - the arg-push at line ~474 — this one is *only* reachable for `Pinned`, so
    the whole `Some(BaseRef::Pinned(..))` arm goes.

- [ ] **Step 3: `agents.rs`** — delete `pipeline_agent`, its dispatcher arm
  (line ~501), and simplify the `base_ref` match (line ~280) back to the
  two-way `pr_branch` decision. The `pr_branch` binding at line ~267 currently
  matches on a 3-tuple including `task.pinned_branch`; restore it to the
  2-tuple form.

- [ ] **Step 4: `prompts.rs`** — delete the pipeline-tick prompt builder and its
  `pinned_branch` parameter.

- [ ] **Step 5: `src/git.rs`** — delete `remote_branch_sha`. Its doc comment
  names `SchedulerRunner` as its only caller, and that is accurate — confirm
  with a grep before deleting rather than trusting the comment.

---

## Task 6: Delete the scheduler module

**Files:**
- Delete: `src/scheduler/mod.rs`, `src/scheduler/tests.rs`.
- Modify: `src/lib.rs`, `src/runtime/mod.rs`, `src/runtime/editor.rs`.

- [ ] **Step 1:** `rm -r src/scheduler`, remove `pub mod scheduler;` from
  `src/lib.rs:22`.

- [ ] **Step 2: `src/runtime/mod.rs`** — remove the `scheduler_runner` field
  (line ~319), its construction (line ~518-537), and its `.start()` call
  (line ~753). The `FeedRunner` beside it is untouched.

- [ ] **Step 3: `src/runtime/editor.rs:732`** — remove the
  `scheduler_runner: None,` test-fixture line.

- [ ] **Step 4:** `cargo build` — the runtime should compile with no scheduler
  reference left.

---

## Task 7: MCP surface

**Files:**
- Modify: `src/mcp/handlers/dispatch.rs`, `src/mcp/handlers/tasks/mod.rs`,
  `src/mcp/handlers/tasks/crud.rs`, `src/mcp/handlers/types.rs`.
- Test: `src/mcp/handlers/tests/tasks/crud.rs`.

- [ ] **Step 1: Delete the MCP handler tests** asserting that `create_task` /
  `update_task` accept the two fields, and any `get_task` formatting assertion
  mentioning them. Confirm red.

- [ ] **Step 2: `dispatch.rs:191-198`** — remove the two `create_task` schema
  properties.

- [ ] **Step 3: `tasks/mod.rs:154-161`** — remove the two `optional` entries
  from the generated `update_task` boundary, and the two fields from the parsed
  create struct (line ~208).

- [ ] **Step 4: `tasks/crud.rs:132-133`** — remove the two params passed into
  `CreateTaskParams`.

- [ ] **Step 5:** confirm no MCP tool description anywhere still mentions
  scheduling: `grep -rn "schedul" src/mcp/`.

---

## Task 8: TUI and editor surfaces

**Files:**
- Modify: `src/tui/types.rs`, `src/tui/messages/input.rs`,
  `src/tui/update/forms.rs`, `src/tui/update/system.rs`,
  `src/tui/ui/input_form.rs`, `src/tui/ui/kanban/mod.rs`,
  `src/tui/ui/kanban/cards.rs`, `src/editor.rs`, `src/runtime/editor.rs`,
  `src/runtime/tasks.rs`.
- Test: `src/tui/tests/input_handlers.rs`, `src/tui/tests/snapshots.rs`,
  `src/editor.rs`'s own test module.

- [ ] **Step 1: Delete the TUI and editor tests first** — the schedule-gate /
  interval / pinned-branch input-handler cases, the two card-badge tests in
  `src/tui/ui/kanban/cards.rs:851-885`, and the eight
  `task_editor_*schedule*` / `*pinned*` tests in `src/editor.rs:1301-1450`.
  Keep every `epic_editor_feed_interval_*` test — those cover the surviving
  epic surface. Confirm red.

- [ ] **Step 2: Creation form.** Remove `InputMode::InputScheduleGate`,
  `InputScheduleInterval`, `InputPinnedBranch` (`src/tui/types.rs:295-302`),
  the three `InputMessage` variants and their dispatch arms
  (`src/tui/messages/input.rs`), the three handlers and the two prompt
  constants (`src/tui/update/forms.rs`), the three line-renderers
  (`src/tui/ui/input_form.rs:381-430`) and their three `match` arms
  (`src/tui/ui/kanban/mod.rs:530-536`). Re-point whatever step previously
  advanced into `InputScheduleGate` (`forms.rs:184`) at the submit that
  followed the tail — read the current step chain rather than assuming.

- [ ] **Step 3: Draft and edit payloads.** Remove the two fields from the task
  draft (`src/tui/types.rs:352-359`) and from the post-edit payload
  (`src/tui/types.rs:624-628`), plus the two writes in
  `src/tui/update/system.rs:37-38` and the two in `src/runtime/tasks.rs:90-91`.

- [ ] **Step 4: Card badge.** Remove the `schedule_interval_secs` parameter from
  `render_card_indicator` (`src/tui/ui/kanban/cards.rs:285`), the badge
  construction (line ~351) and the call site (line ~552).

- [ ] **Step 5: `src/editor.rs`.** Remove `schedule_interval_secs` /
  `pinned_branch` from `TaskEditorFields` and `AppliedTaskEditorFields`, the two
  emitted sections (line ~246), the parse of `SCHEDULE_INTERVAL_SECS`
  (line ~457) and `PINNED_BRANCH` (line ~477), and the clearable-pinned-branch
  logic at line ~371. Keep `interval_parse_failure_message` and both
  `crate::models::parse_interval_secs` call paths that serve
  `FEED_INTERVAL_SECS`; reword its doc comment from "One wording for both
  sections" to name only the epic section.

- [ ] **Step 6: `src/runtime/editor.rs:337-383`** — remove the two destructured
  fields and the two `UpdateTaskParams` builder calls.

- [ ] **Step 7:** `cargo test` and re-accept any TUI snapshot whose creation
  form or card layout legitimately changed. Review each snapshot diff by eye —
  a snapshot that changes in a way this removal does not explain is a bug, not a
  re-accept.

---

## Task 9: Docs, spec alignment, and the epic's backlog

**Files:**
- Delete: `docs/superpowers/specs/2026-08-16-staging-pipeline-scheduled-agents-design.md`,
  `docs/plans/2026-08-16-scheduling-primitive.md`,
  `docs/plans/2026-08-16-tui-scheduled-tasks.md`,
  `docs/plans/2026-08-16-verify-command-tiering.md`,
  `docs/plans/2026-08-16-wrap-up-merge.md`.
- Modify: `docs/conventions.md`, `docs/module-map.md`.

- [ ] **Step 1:** delete the five epic documents above (the user asked for them
  to go, not to be archived).

- [ ] **Step 2:** remove the `scheduler` entry from `docs/module-map.md` and any
  scheduling example from `docs/conventions.md`.

- [ ] **Step 3:** `./scripts/check-doc-paths.sh` and
  `./scripts/check-doc-symbols.sh` — both will flag any citation left pointing
  at a deleted symbol or file. This is the check that catches a missed
  reference; do not skip it.

- [ ] **Step 4:** run the `allium:weed` skill over
  `docs/specs/{core,tasks,dispatch}.allium` to confirm spec and code now agree.

- [ ] **Step 5:** archive tasks #4205, #4230 and #4233 — all three are
  scheduling-specific follow-ups with nothing left to attach to. Leave #4234
  (`parse_section` empty-vs-unparseable) open: it is a general editor concern
  that outlives this epic. Leave #4206 open or closed as it stands — it never
  landed code.

---

## Task 10: Verify

- [ ] `cargo fmt`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test > /tmp/claude-1000/t.txt 2>&1; echo $?` — redirect, never pipe
  into `tail`/`grep`, or a failing suite reads as a pass.
- [ ] `./scripts/check-doc-paths.sh && ./scripts/check-doc-symbols.sh`
- [ ] `grep -rn "schedule_interval_secs\|pinned_branch\|last_processed_sha\|last_scheduled_check_at\|SchedulerRunner\|pipeline_agent" src/ tests/ docs/` — expect
  hits only in `src/db/migrations.rs` (v88's historical body and v90's drop) and
  this plan.
- [ ] `git log --oneline HEAD..main` — `main` moves during a session this long;
  merge and re-run the suite if it is non-empty.

---

## What actually happened

Three things the plan did not anticipate.

**1. `DROP COLUMN` re-resolves every trigger on the table.** Not only the ones
naming the dropped column — and a trigger body is never resolved when it is
*created*, so the error surfaces only at the next `ALTER TABLE`. v90 hit this on
two migration tests (`migration_v38_feed_epic_columns`,
`migration_v52_adds_verify_command_to_repo_paths`) whose stub schemas omit
`tasks.external_id` (v38) and `epics.parent_epic_id` (v34) — columns a real
database at those versions has, and which v72's feed-subtree triggers name. The
fixtures were completed rather than the migration weakened; the fixture comments
say why, since the next `ALTER TABLE tasks` migration would otherwise rediscover
this from scratch.

An intermediate version of v90 dropped and recreated those triggers to sidestep
the problem. That was wrong twice over: the triggers survive `DROP COLUMN`
untouched (verified directly), so it was unnecessary, and recreating them broke
three feed tests. `migration_v90_leaves_the_feed_subtree_triggers_intact` now
pins the real behaviour.

**2. `format_interval_secs` had to go.** Its only two callers were the card
badge and the creation form's schedule summary. `parse_interval_secs` and
`INTERVAL_EXAMPLES` survive on the epic feed-interval surface, but the formatter
had no caller left and `-D warnings` makes dead code a hard failure. Removed
along with its five tests; `core.allium`'s "How an interval is displayed back"
now says no surface humanises a cadence today.

**3. The epic's backlog tasks could not be archived.** MCP `update_task` accepts
only `backlog`/`running`/`review`, and no CLI subcommand archives a task — so
#4205, #4230 and #4233 were retitled `OBSOLETE (#4407 reverted this epic)` with
descriptions explaining what is gone. **They still need archiving from the TUI.**

#4233 left one live question behind, recorded in its description rather than
lost: `Epic.feed_interval_secs` has the same positivity gap
`schedule_interval_secs` had — the grammar enforces it for the typed surface,
the MCP integer path does not, and whether `FeedRunner` busy-loops on a
persisted `0` was never checked.

### Result

88 files, +284/−4981. Full suite green (22 targets, 4223 lib tests, tmux on
`PATH` so no target silently skipped), `cargo clippy --all-targets -D warnings`
clean, all four gate scripts pass, `allium check` reports no findings on the
three edited specs.
