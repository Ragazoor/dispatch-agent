# WP6 — Spec & docs alignment

> **For agentic workers:** REQUIRED SUB-SKILLS: `allium:tend` (edit specs), then `allium:weed` (verify alignment).

**Goal:** Bring `feeds.allium` (and `epics.allium`/`core.allium` as needed) in line with the implemented routing behavior, and document the migration for users. This WP runs **last**, after WP1–WP5 land, so the spec matches real code.

**Spec source:** `docs/superpowers/specs/2026-06-15-pr-review-feed-routing-design.md`.
**Depends on:** WP1–WP5 (spec must describe what was actually built).

---

### Task 1: Update `feeds.allium`

**Files:** `docs/specs/feeds.allium`

- [ ] **Step 1 — invoke `allium:tend`.** Update the spec to:
  - Add `signals: Set<Signal>` to the `FeedItem` value (transient; consumed by routing; not persisted); define the `Signal` enum (`direct-request | team-request | reviewed | commented | author-bot | author-me`); state unknown values soft-fail-skip.
  - Document the **`route_by_role` path**: an epic with `feed_role = reviews_parent` reconciles its role-sub-epic subtree from one emission; `route(signals) -> FeedRole`; global `external_id` identity → role changes are **moves** (status/worktree/session preserved); merged/closed → removed via the subtree-scoped delete.
  - Amend the **Scope** note: explicitly record the deliberate exception to the "runtime never embeds upstream-specific knowledge" principle for the role-routing path, with a pointer to this design doc.
- [ ] **Step 2 — `allium check`** passes (no syntax/validation errors).
- [ ] **Step 3 — commit:** `docs(spec): feeds.allium — signals + route_by_role path`

### Task 2: Update `epics.allium` / `core.allium`

**Files:** `docs/specs/epics.allium`, `docs/specs/core.allium`

- [ ] **Step 1 — invoke `allium:tend`.** Add the `FeedRole` enum to the domain model (`core.allium`) and the `Epic.feed_role` field + managed-epic provisioning rules (`epics.allium`): idempotent creation by role, rename-stable identity, archived-not-resurrected, role sub-epics carry no feed_command.
- [ ] **Step 2 — `allium check`** passes.
- [ ] **Step 3 — commit:** `docs(spec): FeedRole + managed epics`

### Task 3: Verify spec/code alignment

- [ ] **Step 1 — invoke `allium:weed`** across `feeds.allium`/`epics.allium`/`core.allium` vs the implementation. Resolve any divergence (fix spec or file a follow-up if code is wrong).
- [ ] **Step 2 — commit** any spec fixes: `docs(spec): weed feed-routing alignment`

### Task 4: User-facing migration docs

**Files:** `docs/reference.md` (configuration/feeds section)

- [ ] **Step 1 — document:** how to configure the two scripts; that enabling managed reviews means you should **remove old hand-wired review/dependabot feed epics** to avoid transitional duplication (M5); the signal vocabulary for anyone writing a custom reviews script; the known eventual-consistency lag of GitHub search (a move may lag a poll cycle).
- [ ] **Step 2 — run** `./scripts/check-doc-paths.sh` (validates doc links).
- [ ] **Step 3 — commit:** `docs: configure managed review/cve feeds + migration notes`

---

## Done when
- `allium check` clean on all touched specs; `allium:weed` reports alignment.
- `docs/reference.md` covers configuration, migration (remove old epics), signal vocabulary, and the search-lag caveat.
- `cargo test && ./scripts/check-doc-paths.sh` passes.
