# WP5 — Config surface + managed epic provisioning

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans. TDD: test first.

**Goal:** Let the user configure two scripts (reviews + CVE) and have dispatch idempotently provision the managed epics: a `reviews_parent` epic carrying the reviews `feed_command` with `my_reviews`/`team_reviews`/`bots` sub-epics (no `feed_command`), and a `cve` epic carrying the CVE `feed_command`.

**Spec:** `docs/superpowers/specs/2026-06-15-pr-review-feed-routing-design.md` §6, §7.
**Depends on:** WP1 (`feed_role` + unique index), WP3 (`run_role_routed_feed_sync`).

**Before coding — locate the surfaces (use Grep):**
- The settings/config store: `grep -rn "settings" src/db/` and `src/config` / `src/cli/setup.rs` (the `setup` subcommand configures MCP). Find where existing scalar settings are read/written (the `repo_filter_mode` key migrated in `migrations.rs` v60 is an example of a settings key).
- Epic creation service: `EpicServiceApi` (mutation boundary — go through the service, not the DB, per CLAUDE.md). `create_epic` + `patch_epic(EpicPatch::new().feed_role(..).feed_command(..))`.

---

### Task 1: Config keys for the two scripts

**Files:** the settings module (located above) + its tests.

- [ ] **Step 1 — failing test.** Setting then getting `reviews_feed_command` (and `reviews_feed_interval_secs`, `cve_feed_command`, `cve_feed_interval_secs`) round-trips; unset returns `None`.
- [ ] **Step 2 — run, expect fail.**
- [ ] **Step 3 — implement** typed getters/setters over the settings store for those four keys (mirror an existing scalar setting's read/write).
- [ ] **Step 4 — run.** **Step 5 — commit:** `feat(config): reviews + cve feed-command settings`

### Task 2: Idempotent managed-epic provisioning

**Files:** Create `src/service/managed_feeds.rs` (or a focused module under `src/service/epics.rs`); test inline.

- [ ] **Step 1 — failing tests.**
  - `ensure_managed_epics_creates_tree`: from empty DB, after ensure, exactly one epic per `feed_role` in {reviews_parent, my_reviews, team_reviews, bots, cve}; the three review roles have `parent_epic_id == reviews_parent.id`; reviews_parent has the configured `feed_command`; the three role sub-epics have `feed_command == None`; cve has the CVE command.
  - `ensure_is_idempotent`: calling twice creates no duplicates (relies on the WP1 unique index + `ON CONFLICT DO NOTHING`).
  - `ensure_preserves_user_rename`: rename `my_reviews` epic's title, ensure again → no new epic, title kept.
  - `ensure_does_not_resurrect_archived`: archive the `bots` epic, ensure again → it stays archived (a `tracing::warn!` is logged), no empty duplicate created.
- [ ] **Step 2 — run, expect fail.**
- [ ] **Step 3 — implement** `ensure_managed_epics(reviews_cmd, reviews_interval, cve_cmd, cve_interval)`:
  - Look up existing epics by `feed_role` (add a `list_epics_by_feed_role` / filter helper).
  - For each required role: if an active (non-archived) epic with that role exists, update its `feed_command`/interval as needed; if an archived one exists, warn + skip; else create via the service and patch `feed_role` (+ `feed_command` only for `reviews_parent` and `cve`). Use the unique index for race-safety (`INSERT … ON CONFLICT(parent_epic_id, feed_role) DO NOTHING` semantics — if the DB layer lacks this, do a select-then-insert guarded by the index and treat the conflict error as "already exists").
- [ ] **Step 4 — run.** **Step 5 — commit:** `feat(service): idempotent managed-epic provisioning`

### Task 3: Call provisioning on startup / config change

**Files:** the runtime/TUI startup path (`grep -rn "FeedRunner::new\|run_tui\|fn run(" src/runtime src/cli`).

- [ ] **Step 1 — failing test.** A startup/integration test (under `tests/`) that, with the reviews+cve settings set, brings up the app state and asserts the managed epics exist and a reviews tick routes a stub emission into the correct sub-epics (combine with WP3's router).
- [ ] **Step 2 — run, expect fail.**
- [ ] **Step 3 — implement.** On startup (and when the settings change), call `ensure_managed_epics(...)` if the reviews/cve commands are configured. The reviews_parent's `feed_command` makes the existing `FeedRunner` poll it; WP3's branch routes it. Confirm role sub-epics never get a `feed_command` (so the runner never polls them independently — the B3 guard).
- [ ] **Step 4 — run.** **Step 5 — commit:** `feat(runtime): provision managed feeds on startup`

### Task 4: Setup UX + header rendering

**Files:** `src/cli/setup.rs` (or wherever feeds are configured); TUI epic header (`src/tui/ui/shared.rs:85` shows `group:on/off` for feed epics) — add a small indicator for managed/role epics if it clarifies; snapshot tests as needed.

- [ ] **Step 1 — failing snapshot/test** for any header change (render a `reviews_parent` epic; assert label).
- [ ] **Step 2 — implement** minimal surfacing (e.g. a `role:my-reviews` hint), or skip if not needed — keep scope tight.
- [ ] **Step 3 — accept snapshots** if intentional: `INSTA_UPDATE=always cargo test tui::tests::snapshots` then `rm src/tui/tests/snapshots/*.snap.new`.
- [ ] **Step 4 — commit:** `feat(tui): surface managed feed roles`

---

## Done when
- `cargo test && ./scripts/check-doc-paths.sh` passes.
- Configuring the two scripts provisions the epic tree idempotently; renames survive; archived managed epics are not resurrected; two startups create no duplicates.
- Role sub-epics carry no `feed_command`.
