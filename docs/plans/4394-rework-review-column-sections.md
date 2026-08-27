# 4394 — Rework the Review column's section delimiters

## Problem

The Review column groups cards under section headers. Three things are wrong:

1. **The "awaiting merge" split uses the wrong signal.** It fires purely on
   `Task::is_detached()` (worktree present, tmux window gone) and never checks
   whether a PR exists. A task manually moved to Review with no PR at all, whose
   agent session has ended, renders under "awaiting merge" — there is nothing to
   merge.

2. **"changes requested" conflates two opposite situations.** The sub-status is
   set from GitHub's `reviewDecision`, so it covers both *someone requested
   changes on my PR* (I must act) and *I requested changes on someone else's PR*
   (I am waiting on them). Both land at the top of the column.

3. **The section order does not track relevance.** Approved PRs — ready to merge,
   a one-keystroke action — sort *below* awaiting-review PRs that need nothing
   from anyone.

## Decisions taken with the user

- **Both new sections are display-only, derived from the task row.** No new
  `SubStatus` variant, no migration, no MCP schema change. This extends the
  mechanism already in place (`display_column_priority` / `display_header_label`
  in `src/tui/mod.rs`); the bug was a wrong derivation, not the fact of deriving.
  Deriving also gives auto-un-parking for free — the moment the tmux window
  reappears the card leaves the parked section, with nothing to write back.

- **"parked"** applies when: `status = review` **and** `worktree != null` **and**
  `tmux_window = null` **and** the task carries no PR url. Sub-status is not part
  of the condition — with no PR there is nothing for any review decision to be
  about, so parked dominates.

- **"awaiting merge" is removed.** A detached task that *does* have a PR folds
  back into plain "awaiting review". Detach state stops mattering once a PR
  exists; only the review decision does.

- **"changes requested by me"** applies when `sub_status = changes_requested`
  **and** the task's tag is `pr-review` or `dependabot` (`TaskTag::is_review()`).
  Those tags mean by definition that the task is reviewing someone else's PR.

- **New section order, top to bottom:**

  | # | Section                    | Meaning                              |
  |---|----------------------------|--------------------------------------|
  | 1 | `conflict`                 | rebase/merge conflict (unchanged)    |
  | 2 | `changes requested`        | on my PR — I must fix                |
  | 3 | `approved`                 | ready to merge                       |
  | 4 | `awaiting review`          | nothing to do yet                    |
  | 5 | `parked`                   | no PR, agent session ended           |
  | 6 | `changes requested by me`  | waiting on the other author          |

  The change versus today is that `approved` moves *above* `awaiting review`,
  and two new buckets land at the bottom.

- **Collapsible sections are out of scope.** The user deferred them: get the
  ordering and the bucketing right first.

## Approach

`display_column_priority` and `display_header_label` currently take
`(SubStatus, bool)`. Every call site already has the whole `&Task` in hand (the
epic path calls `SubStatus::column_priority()` directly and is untouched), so
both functions change to take `&Task`. That lets the override consult `url` and
`tag` without threading three more booleans through six call sites.

The numeric priority slots in `src/models/tasks.rs` are renumbered in steps of
10. Today's slots are consecutive integers 0–6, leaving no room to insert
`approved` between `changes requested` (4) and the shared active slot (5).
Renumbering keeps every relative ordering that exists today intact and restores
the header comment's promise that the gaps leave room for display-only overrides.

## Steps

Each step is test-first: write the test, watch it fail, then implement.

### Step 1 — Renumber the priority slots and move `approved` above `awaiting review`

- **Test** (`src/models/tasks.rs`): rewrite
  `substatus_column_priority_matches_urgency_ordering` to assert *relative*
  ordering rather than exact integers — brittle exact values are what made this
  reorder awkward. Assert the full chain
  `Conflict < Crashed < Stale == StaleShell < NeedsInput < ChangesRequested <
  Approved < AwaitingReview == Active == None`.
- **Implement**: renumber the `PRIORITY_*` constants in steps of 10 and add
  `PRIORITY_APPROVED = 45` between `PRIORITY_CHANGES_REQUESTED` (40) and
  `PRIORITY_ACTIVE_SLOT` (50).
- Check `src/models/epics.rs`: `EpicSubstatus::column_priority` delegates to
  `SubStatus`, so it follows along. Its existing tests assert delegation, not
  literals, and should still pass.

### Step 2 — Derive the `parked` section

- **Test** (`src/tui/mod.rs::display_priority_tests`):
  - a review task with a worktree, no tmux window and **no** url gets the
    `"parked"` label and a priority above `AwaitingReview`;
  - the same task with a **PR** url gets `"awaiting review"` (this is the
    awaiting-merge removal);
  - the same task with a **non-PR** url (issue / security_alert) is still
    `"parked"`;
  - a review task with a live tmux window and no url is **not** parked;
  - a *running* detached task with no url is **not** parked;
  - a review task with no worktree at all (unprovisioned) is **not** parked.
- **Test** (`src/tui/tests/navigation.rs`): replace the three
  `awaiting_merge` render tests. A detached review task with no url renders a
  `"parked"` section header; a detached review task **with** a PR renders
  `"awaiting review"` and no `"parked"`.
- **Implement**: change both display functions to take `&Task`; add
  `PRIORITY_PARKED` and the `"parked"` label; update the six call sites in
  `src/tui/mod.rs` and `src/tui/ui/kanban/columns.rs`. Delete
  `is_detached_awaiting_review` and `DETACHED_AWAITING_REVIEW_PRIORITY`.

### Step 3 — Derive the `changes requested by me` section

- **Test** (`src/tui/mod.rs::display_priority_tests`):
  - `changes_requested` + `TaskTag::PrReview` → `"changes requested by me"`,
    priority below `parked`;
  - `changes_requested` + `TaskTag::Dependabot` → same;
  - `changes_requested` + `TaskTag::Feature` → plain `"changes requested"` at
    the model priority;
  - `changes_requested` + no tag → plain `"changes requested"`;
  - `approved` + `TaskTag::PrReview` → plain `"approved"` (the override is
    scoped to `changes_requested` only);
  - a parked task that is also `changes_requested` + `PrReview` renders
    `"parked"` — parked is checked first.
- **Test** (`src/tui/tests/navigation.rs`): a pr-review-tagged
  `changes_requested` task and a feature-tagged `changes_requested` task in the
  same column render two distinct section headers, with the pr-review one below.
- **Implement**: add the second override branch, ordered after `parked`.

### Step 4 — Spec and docs

- `docs/specs/core.allium`: update the `SubStatus.awaiting_review` comment (drop
  the awaiting-review/awaiting-merge split note), and the VisualColumn table's
  vcol 4–6 rows to describe the new section set and order. Add a note that
  `parked` and `changes requested by me` are presentation-only derivations, not
  `SubStatus` values.
- Run `allium:weed` to confirm spec and code agree.
- Check `src/tui/ui/kanban/popups/help.rs` and the snapshot tests for any
  mention of the old sections.

### Step 5 — Verify

- `cargo test --no-fail-fast > /tmp/claude-1000/t.txt 2>&1; echo $?` (the repo's
  verify command, run without a pipe so the exit code is real).
- `cargo fmt`, then `cargo clippy --all-targets -- -D warnings`.
- Review snapshot diffs with `cargo insta review` if any section-header
  snapshots shift.
- `git log --oneline HEAD..main` to check the base has not moved.

## Out of scope

- Collapsible / hideable sections (deferred by the user).
- Any change to how `sub_status` itself is computed from GitHub's
  `reviewDecision` (`PollPrStatus` in `docs/specs/pr-workflow.allium`).
- Fetching the PR author from GitHub to detect authorship — the existing
  `pr-review` / `dependabot` tag is the chosen signal.
