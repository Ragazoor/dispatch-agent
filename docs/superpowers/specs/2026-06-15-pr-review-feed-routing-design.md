# PR review feed routing — design

**Task:** 2105 — "PR bug" (reviewed/collaborated PRs disappear from their epic).
**Date:** 2026-06-15
**Status:** Draft for review — revised after adversarial review (B1–B3, H1–H4,
M1–M5 folded in).

> **Considered and rejected — minimal fix.** Adversarial review proposed a much
> smaller change: add `reviewed-by:@me`/`commenter:@me` to the existing script
> and make `upsert_feed_tasks` never delete non-`Backlog` tasks. This was
> consciously rejected (twice) by the task owner: it does not deliver the
> deterministic my/team/bot routing or the cross-epic *true move* that are
> explicit goals, and "never delete non-Backlog" creates zombie tasks for
> closed-unmerged PRs. The redesign below is the chosen scope.

## Problem

Review tasks vanish from their epic once you act on the PR. The feed
(`fetch-reviews.sh`) queries `gh search prs review-requested:@me`; GitHub clears
the review-request the instant you submit a review, so the PR drops out of the
search. The feed is source-of-truth — `upsert_feed_tasks` / `sync_grouped_feed`
DELETE any task whose `external_id` is absent from the latest emission
(`src/db/queries/tasks.rs:484`). So: review → PR leaves the search → next feed
cycle deletes the task → it disappears.

The current model also makes the my/team split fragile: `My Review` and `Team
Review` are **separate epics, each with its own `feed_command`**, reconciled
independently. Task identity is `(epic_id, external_id)`, so a PR transitioning
between epics is **delete+recreate**, not a move — losing status, worktree, and
agent session. And `team`'s set-difference exclusion (`review-requested:@me`
minus `user-review-requested:@me`) breaks exactly when you engage, because the
direct-request marker it subtracts on is the marker that clears.

## Goals

- A PR survives your entire engagement (requested → reviewed → collaborating)
  and only leaves the board when **merged or closed**.
- Deterministic, leak-free routing into **My Reviews / Team Reviews / Bots**.
- A bucket change is a **true move** (`move_task_to_epic`), preserving the
  task's status, worktree, and agent session.
- Better UX: configure **two scripts** (reviews + CVE); the TUI creates and
  manages the epics.

## Non-goals

- CVE/security feed unification — CVE advisories are not PRs; the CVE feed stays
  a separate script feeding a separate managed epic.
- Changing the **generic** feed mechanism (`flat` and `group_by_repo` modes)
  for power users. Those stay dumb and untouched (see Philosophy).

## Philosophy shift (flagged)

`feeds.allium` currently states: *"the runtime never embeds upstream-specific
knowledge — feed scripts are user-owned executables."* This redesign
**deliberately departs** from that for the review use case: PR-routing policy
moves into the runtime so it is centralized, testable, and leak-free.

To contain the departure, routing is a **new, named path** triggered by
`feed_role = reviews_parent` and implemented in its own reconciler. The generic
flat / `group_by_repo` feed paths remain dumb and upstream-agnostic; only the
role router carries review knowledge. `feeds.allium` will be updated (via
`allium:tend`) to document this opinionated path as an explicit exception to the
dumb-runtime principle.

## Architecture

```
                fetch-reviews.sh (single emission, signals on each item)
                                  │
                                  ▼
   parent "Reviews" epic (feed_command, feed_role = reviews_parent)
                                  │  route(signals) -> FeedRole
              ┌───────────────────┼───────────────────┐
              ▼                   ▼                   ▼
        My Reviews          Team Reviews            Bots
        (feed_role)         (feed_role)         (feed_role)

   fetch-cve.sh ───▶ CVE epic (feed_role = cve)   [separate, unchanged model]
```

### 1. Single reviews emission

`fetch-reviews.sh` is rewritten to emit **one deduped FeedItem list** covering
every relevant PR, dropping the `my`/`team`/`all` scope arg (routing handles
that now). It runs the union of:

- `review-requested:@me` (direct + team requests)
- `user-review-requested:@me` (direct only — distinguishes direct vs team)
- `reviewed-by:@me`
- `commenter:@me -author:@me`

…all `--state=open`, scoped to `ORGS`, drafts excluded. **Renovate/Dependabot
are no longer excluded**; bot-authored PRs flow in tagged `author-bot`. This
**folds `fetch-dependabot.sh` into the reviews emission**.

**Dedup must merge signals, not pick one object.** A PR can match several
queries (e.g. both `review-requested` and `reviewed-by`); each query-pass emits
the same PR with that pass's signal. The dedup therefore groups by URL and
**unions the signal arrays** — `group_by(.url) | map(.[0] + {signals: (map(.signals[]) | unique)})`
— not `unique_by(.url)` (which would drop all but one object and lose signals).

**Known limitation — search lag.** GitHub's `search` API is eventually
consistent; a just-reviewed PR can still match `review-requested:@me` for a few
minutes. Routing is correct once signals settle, but a role move may lag the
real-world action by a poll cycle or two. Acceptable; documented so it is not
mistaken for a bug.

### 2. `FeedItem.signals`

New optional field on `FeedItem`, a **typed enum set** (not free-form strings):

```rust
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Signal {
    DirectRequest, TeamRequest, Reviewed, Commented, AuthorBot, AuthorMe,
}

#[serde(default)]
pub signals: Vec<Signal>,
```

The reviews script emits e.g. `["direct-request","reviewed"]` (those that
apply). Signals are **transient** — consumed during reconciliation routing,
**not** persisted on the task row, so no task-table migration. The field is
generic (other feeds leave it empty); only the `route_by_role` router interprets
it.

**Unknown signals soft-fail.** An unrecognised string (e.g. a script typo) is
logged and skipped during deserialization — it does *not* fail the whole feed
parse (consistent with the soft-fail-decoding convention in
`docs/conventions.md`). The known set stays compile-time exhaustive so the
router's `match` is total. A typo therefore degrades one PR's routing (it loses
that signal) rather than silently routing the wrong way on a near-miss like
`author_bot` vs `author-bot`.

### 3. Routing function (pure Rust, unit-tested)

```rust
fn route(signals: &[Signal]) -> FeedRole
```

Precedence (engagement wins — see Resolved decisions):

1. engaged (`reviewed` OR `commented`) AND NOT `author-me` → **MyReviews**
2. `author-bot` → **Bots**
3. `direct-request` → **MyReviews**
4. `team-request` → **TeamReviews**
5. fallback → **MyReviews** (and `tracing::warn!` — a PR with an empty/unknown
   signal set indicates a feed-script bug; route it somewhere visible but log it)

This is deterministic per-PR: every PR routes to exactly one role, so there is
no leak and no duplication. The my/team set-difference is gone entirely.

### 4. Data model

- **Single new epic column `feed_role`** (enum:
  `reviews_parent | my_reviews | team_reviews | bots | cve | none`, default
  `none`). It carries *both* the role identity of the managed sub-epics *and*
  the "this is the routing parent" marker (`reviews_parent`). Role identity is
  stable across renames (the display title is user-editable; the role is not). A
  **partial unique index** on `(parent_epic_id, feed_role) WHERE feed_role IS NOT
  NULL` prevents duplicate role sub-epics (guards the multi-instance startup
  race — see §6).
- **`group_by_repo` is left untouched.** Grounding revealed it threads through
  ~30 call sites (DB/models/MCP/service/runtime/TUI toggle + header/snapshots);
  collapsing it into a mode enum would be a large refactor for no behavioral
  gain. The `FeedRunner` selects the routing path purely from `feed_role`: an
  epic with `feed_role = reviews_parent` routes via `run_role_routed_feed_sync`;
  every other epic keeps today's `flat`/`group_by_repo` behavior unchanged.
- `FeedItem.signals` as above.

### 5. Routing reconciliation (the core new behavior)

⚠️ This is **new infrastructure**, not a reuse of `upsert_feed_tasks`. That
function's delete pass is **per-epic** (`DELETE … WHERE epic_id = ?1 AND
external_id NOT IN (keep)`, `src/db/queries/tasks.rs:484`), so calling it
per-role would delete a task that was just *moved* into a different role (it is
absent from the losing role's keep-set). The router needs a **subtree-scoped**
reconciler with its own entry point and its own delete query.

**Separate entry point (H2).** Add `run_role_routed_feed_sync(parent_id, items)`
called by the `FeedRunner` when the parent's `feed_role = reviews_parent`. Do
**not** add a branch to `run_feed_sync` — its existing `group_by_repo` bool path
operates on the epic's *own* emission and stays untouched; the role router
orchestrates a subtree and is a different abstraction level.

Algorithm — one parent emission, **global `external_id` identity** across the
role sub-epics:

1. Load existing subtree feed tasks (all role sub-epics), indexed by
   `external_id`.
2. For each emitted PR: `route(signals) → role → target sub-epic`.
   - **Exists in subtree, same role:** update fields in place (`patch_task`:
     title, description, tag, labels, sort_order).
   - **Exists in subtree, different role:** `set_task_epic_id(task, target)`
     **then** `patch_task(fields)`. This is the move. `set_task_epic_id`
     (`src/db/queries/epics.rs:192`) touches only `epic_id`/`updated_at`, so
     **`status`, `sub_status`, `worktree`, `tmux_window`, `sort_order` are
     preserved** — an in-flight dispatched review agent keeps its session.
   - **Not in subtree:** insert into the target sub-epic.
3. **Subtree-scoped delete** — a new query
   `delete_stale_subtree_feed_tasks(parent_id, keep_ids)`:
   `DELETE FROM tasks WHERE epic_id IN (SELECT id FROM epics WHERE
   parent_epic_id = ?1) AND external_id IS NOT NULL AND external_id NOT IN
   (json_each(?2))`. Runs **once** over the whole subtree with the union of all
   emitted ids, so moved tasks survive. Manual tasks (`external_id IS NULL`) are
   preserved, as today.
4. Recalculate epic statuses up the tree (learning #121: reconcile the whole
   subtree, not just present roles).

Steps 1–4 run as **one non-interleaved unit** per parent tick.

**Concurrency (B3).** Role sub-epics must **never** carry a `feed_command`
(enforced at managed-epic creation in §6), so the `FeedRunner` tick loop
(`src/feed/mod.rs:139`, which spawns one task per epic *with* a feed_command)
never schedules an independent reconcile against a sub-epic. Only the parent is
polled. A regression test runs two back-to-back zero-interval ticks and asserts
no task is lost to a move/delete interleave.

Because the whole emission is reconciled together, "present but different role"
= move and "absent" = remove are **unambiguous** — the property that the old
independent-per-epic feeds could not provide.

### 6. Config + managed epics

- Config gains two script paths: **reviews script** and **CVE script** (plus
  their intervals). The reviews `feed_command` lives on the **parent** Reviews
  epic; the role sub-epics carry **no** `feed_command` (see §5 concurrency).
- On config set / startup, the TUI **ensures** the managed epics exist
  (idempotent, matched by `feed_role`): the parent `Reviews` epic with the role
  sub-epics, and the `CVE` epic. Creation uses `INSERT … ON CONFLICT DO NOTHING`
  against the `(parent_epic_id, feed_role)` unique index, so two dispatch
  instances racing on startup (M4) cannot create duplicates.
- **Archived managed epic (H3):** identity is by `feed_role`, not title, so a
  user rename is preserved. If the user has **archived** a managed sub-epic, the
  ensure step does **not** resurrect it — it logs and leaves it archived (a
  recreated-empty epic would be confusing). Re-enabling is an explicit user
  action.

### 7. Migration / coexistence

Existing hand-wired review/dependabot feed epics keep working (the generic feed
mechanism is untouched). Managed feeds are **opt-in** via config; when
configured, the TUI creates the managed epics and the user removes their old
hand-wired review/dependabot epics manually. **No auto-deletion** (no data-loss
risk). A detect-and-convert helper is explicitly out of scope for now.

**Transitional duplication (M5).** Until the user removes an old hand-wired
Dependabot/review epic, the same PR appears both there (its own `feed_command`)
and in the managed `Bots`/`My Reviews` sub-epic. The setup flow and
`docs/reference.md` must state plainly: **remove the old review/dependabot feed
epics when enabling the managed reviews feed.**

## Edge cases

- **Reopened PR (M1).** A closed-unmerged PR is deleted (absent from the
  emission); if reopened later it reappears and is inserted **fresh** — prior
  status/worktree are gone. Accepted for review tasks; documented, not fixed.
- **No-signal PR (M2).** Routed to My Reviews via the fallback rule **with a
  `tracing::warn!`** so a misbehaving feed script is debuggable (see §3).

## Testing strategy (TDD)

Tests precede implementation for each unit:

- **Routing function** — table-driven unit tests over every signal combination
  and the precedence ties (esp. engaged-bot).
- **Routing reconciliation** — in `src/feed/` / `src/db/`: move on role change
  with an **in-flight task** (`worktree`/`tmux_window`/`status` set) and assert
  those survive (B2); insert on first sight; delete on merge/close via the
  subtree-scoped query; a moved task is **not** deleted by the same cycle (B1);
  manual-task preservation; multi-role single emission; two back-to-back
  zero-interval ticks lose nothing (B3 concurrency).
- **`fetch-reviews.sh`** — shell test with a stub `gh` on PATH asserting: a
  reviewed-only PR is emitted; a bot PR carries `author-bot`; an authored PR is
  excluded from `commenter`; dedup by URL across queries. Validated with
  `cargo run -- verify-feed`.
- **Config + managed epics** — idempotent creation; rename survives; TUI
  snapshots where rendering changes.

Verification gate: `cargo test && ./scripts/check-doc-paths.sh`.

## Resolved decisions

1. **Engaged bot PR → My Reviews.** Engagement wins: a bot-authored PR you've
   reviewed/commented on collects in My Reviews. Bots holds only untouched bot
   PRs. (Routing rule 1 above.)
2. **Migration → opt-in + manual cleanup.** No auto-deletion or
   detect-and-convert; see Migration / coexistence.
3. **`signals` → typed `Vec<Signal>`** (kebab-case enum) on `FeedItem`,
   transient (not persisted), unknown values soft-fail-skipped. Chosen over
   free-form `Vec<String>` after adversarial review (H1): a typed set makes the
   router's `match` exhaustive and prevents near-miss strings from misrouting.

## Work package decomposition

Each WP is independently testable and lands behind passing tests.

- **WP1 — Data model & migrations.** Single `feed_role` column (+ partial unique
  index on `(parent_epic_id, feed_role)`); wire it through the `Epic` struct,
  row mapping, and `EpicPatch`. `group_by_repo` untouched. Typed `FeedItem.signals`
  with soft-fail deserialization. DB migration + model + parse tests.
- **WP2 — Routing function.** Pure `route(&[Signal]) -> FeedRole`, exhaustive
  `match` + table-driven unit tests (incl. engaged-bot tie, empty-signal
  fallback). No I/O.
- **WP3 — Routing reconciliation.** New `run_role_routed_feed_sync` entry point
  (separate from `run_feed_sync`) + new `delete_stale_subtree_feed_tasks` DB
  query + the move (`set_task_epic_id` then `patch_task`) sequence. Tests per the
  reconciliation list above (B1/B2/B3 coverage).
- **WP4 — Reviews script rewrite.** Single emission with typed signals;
  **signal-merging** dedup by URL (group_by, not unique_by); fold Dependabot;
  stop excluding Renovate; drop scope arg. Shell test (stub `gh`) covering the
  signal-merge + verify-feed.
- **WP5 — Config + managed epics.** Two-script config surface; idempotent
  managed-epic creation in the TUI. Tests + snapshots.
- **WP6 — Spec + docs.** Update `feeds.allium` (and `epics.allium`/`core.allium`
  as needed) via `allium:tend`; `allium:weed` to confirm alignment; document
  migration in `docs/reference.md`.
