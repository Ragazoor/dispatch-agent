# 4382 — Closed PRs stay in review, don't move to Done

## Problem

`PollPrStatus` currently treats a closed-without-merge PR exactly like a
merged one: `PrClosed` fires and the task is moved straight to `done`
(`src/tui/update/pr.rs::handle_pr_terminal`, mirrored in
`docs/specs/pr-workflow.allium`'s `PrClosed` rule). The task wants this
changed: a closed PR should NOT move the task to done. It should stay in
review (or running, if it somehow gets there — in practice `PollPrStatus`
only ever polls `status = review` tasks) and the sub-status label should
change to something like "PR closed", so the user notices and decides what
to do (reopen the PR, archive the task, etc.).

## Design

Add a new `SubStatus::PrClosed` variant, valid only for `TaskStatus::Review`.
`PrClosed` (the runtime event) now sets `task.sub_status = pr_closed` instead
of transitioning `task.status` to `done`. No tmux/worktree teardown, no
detach — the agent session (if any) is untouched, since the task isn't
finishing.

Idempotency: `PollPrStatus` fires every tick while the PR stays closed and
`task.status` stays `review`. Guard on `sub_status != pr_closed` so we don't
re-persist/re-notify every tick, matching the existing guard pattern in the
open-PR review-decision branch.

Conflict interaction: unlike the open-PR branch (which never overwrites
`sub_status = conflict`), `PrClosed` overrides `conflict` unconditionally —
same rationale the current terminal rule documents: a closed PR is a
stronger, more definitive GitHub signal than a local rebase conflict, and
the user needs to see it.

Column placement: `pr_closed` joins the existing "PR Created" visual column
(alongside `awaiting_review` and `conflict`) rather than a new column — it's
still "a PR exists, something needs a look," just a different reason. A new
priority tier is inserted right after `conflict` (most urgent) so a closed
PR surfaces near the top of the Review swimlane. This shifts every priority
tier at/after `crashed` up by one, including `DETACHED_AWAITING_REVIEW_PRIORITY`
in `src/tui/mod.rs`.

DB: `sub_status` per `(status)` is enforced by a CHECK constraint on the
`tasks` table (rebuilt each time a valid pair is added — see migration v86
for the most recent, `stale_shell`, using generic pragma-based column
introspection). New migration v89 adds `pr_closed` to the `review` branch of
that CHECK, following the same generic-rebuild pattern.

## Steps (TDD: spec → tests → code)

1. **Spec first** (`allium:tend`): update `docs/specs/core.allium` (add
   `pr_closed` to the `SubStatus` enum + the valid-combinations comment) and
   `docs/specs/pr-workflow.allium` (`PollPrStatus` guidance, rewrite the
   `PrClosed` rule to update sub_status instead of transitioning to done).
2. **Model tests first**, then implement:
   - `src/models/tasks.rs`: `SubStatus::PrClosed` — `is_valid_for`,
     `default_for` (unaffected), `header_label` ("pr closed"), a new
     priority tier, `define_str_enum!` mapping (`"pr_closed"`), added to
     `ALL` but not `MCP_ADVERTISED` (system-derived, like `stale_shell`).
   - `src/models/columns.rs`: add `PrClosed` to the "PR Created"
     `VisualColumn`.
3. **DB migration test first**, then implement:
   - `src/db/tests/tasks.rs`: a `(review, pr_closed)` patch persists
     (mirrors `task_sub_status_persists`).
   - `src/db/migrations.rs`: migration v89, registered in `MIGRATIONS`.
4. **TUI handler tests first**, then implement:
   - `src/tui/tests/dispatch.rs`: replace `pr_closed_moves_to_done_and_detaches`
     with a test asserting the task stays in `review`, `sub_status` becomes
     `pr_closed`, and `tmux_window`/`worktree` are untouched. Add an
     idempotency test (second `PrMessage::Closed` on an already-`pr_closed`
     task emits no commands). Keep `pr_closed_status_message_says_closed_not_merged`,
     `pr_closed_no_notification_when_disabled`, `pr_closed_ignores_non_review_task`
     (still correct, minor message wording only).
   - `src/tui/update/pr.rs`: `handle_pr_closed` becomes its own function
     (no longer routed through `handle_pr_terminal`, which stays merged-only).
5. Update `src/tui/mod.rs`'s `DETACHED_AWAITING_REVIEW_PRIORITY` for the
   shifted priority tiers, and its doc comment.
6. Run `cargo test`, then `allium:weed` to confirm spec/code alignment.

## Out of scope

- `PrMerged` behavior (unaffected — still moves to done).
- Any new `CardIndicator` for `pr_closed` — existing review sub-statuses
  (`changes_requested`, `approved`) don't get bespoke card badges either;
  the generic `ReviewPr` indicator plus the column header label is the
  existing pattern.
