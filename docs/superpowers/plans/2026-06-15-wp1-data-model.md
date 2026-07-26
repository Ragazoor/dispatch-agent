# WP1 — Data model & migrations (feed_role + FeedItem.signals)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans. Steps use `- [ ]`.
> Follow the repo's TDD rule (CLAUDE.md): test first, then minimal code. One git command per Bash call.

**Goal:** Add the `feed_role` epic column (+ partial unique index) and a typed `FeedItem.signals` field, with soft-fail deserialization. No behavior change yet — this is the foundation WP2/WP3/WP5 build on.

**Spec:** `docs/superpowers/specs/2026-06-15-pr-review-feed-routing-design.md` §2, §4.

**Interface this WP exposes (used by later WPs — keep these names exact):**
- `enum FeedRole { None, ReviewsParent, MyReviews, TeamReviews, Bots, Cve }` (serde kebab-case; `none` default).
- `Epic.feed_role: FeedRole` field.
- `EpicPatch::feed_role(FeedRole)` builder.
- `enum Signal { DirectRequest, TeamRequest, Reviewed, Commented, AuthorBot, AuthorMe }` (serde kebab-case).
- `FeedItem.signals: Vec<Signal>` (`#[serde(default)]`).

**`group_by_repo` is NOT touched.**

---

### Task 1: `FeedRole` enum

**Files:**
- Modify: `src/models/epics.rs` (add enum near `Epic`)

- [ ] **Step 1 — failing test.** In `src/models/epics.rs` test module add:
```rust
#[test]
fn feed_role_roundtrips_kebab_case() {
    assert_eq!(serde_json::to_string(&FeedRole::MyReviews).unwrap(), "\"my-reviews\"");
    assert_eq!(serde_json::from_str::<FeedRole>("\"reviews-parent\"").unwrap(), FeedRole::ReviewsParent);
    assert_eq!(FeedRole::default(), FeedRole::None);
}
```
- [ ] **Step 2 — run, expect fail** (`FeedRole` undefined): `cargo test -p dispatch feed_role_roundtrips`
- [ ] **Step 3 — implement.** Add:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FeedRole {
    #[default]
    None,
    ReviewsParent,
    MyReviews,
    TeamReviews,
    Bots,
    Cve,
}
```
Add `as_str`/`from_str` (or rely on serde) following the pattern used by `TaskTag`/`TaskStatus` in `src/models/tasks.rs` — check how those serialize to/from the SQLite TEXT column and mirror it (the DB stores the kebab string).
- [ ] **Step 4 — run, expect pass.**
- [ ] **Step 5 — commit:** `feat(models): add FeedRole enum`

### Task 2: `Epic.feed_role` field + row mapping

**Files:**
- Modify: `src/models/epics.rs:15` (struct + the 3 default constructors at ~218/521/562)
- Modify: `src/db/queries/mod.rs:113` (row→Epic mapping)
- Modify: `src/db/queries/epics.rs` (the SELECT column lists at lines ~50/69/88/261 — add `feed_role`)

- [ ] **Step 1 — failing test.** In `src/db/tests/epics.rs`:
```rust
#[tokio::test]
async fn create_epic_defaults_feed_role_none() {
    let db = Database::open_in_memory().await.unwrap();
    let epic = db.create_epic("E", "", None).await.unwrap();
    assert_eq!(epic.feed_role, crate::models::FeedRole::None);
}
```
- [ ] **Step 2 — run, expect fail** (`feed_role` not a field).
- [ ] **Step 3 — implement.** Add `pub feed_role: FeedRole,` to `Epic`; set `FeedRole::None` in every struct literal flagged by the compiler (the `group_by_repo: false` neighbours are the map). Add `feed_role` to each SELECT column list in `src/db/queries/epics.rs` and map it in `src/db/queries/mod.rs:113` with `feed_role: row.get::<_, String>("feed_role")?.parse().unwrap_or_default()` (mirror how `TaskTag` is read — soft-fail to default on an unknown string).
- [ ] **Step 4 — run full epic tests:** `cargo test -p dispatch db::tests::epics`
- [ ] **Step 5 — commit:** `feat(models): Epic.feed_role field + row mapping`

### Task 3: Migration v64 — column + partial unique index

**Files:**
- Modify: `src/db/migrations.rs` (new `migrate_v64_add_epic_feed_role`, register in the array near line 112; use `migrate_v54_add_group_by_repo` at line 1230 as the template)
- Test: `src/db/tests/migrations.rs`

- [ ] **Step 1 — failing test.** In `src/db/tests/migrations.rs`, assert the column and index exist after migration (follow the existing column-exists test pattern in that file, e.g. the v54 group_by_repo test referenced at line 2682):
```rust
#[tokio::test]
async fn v64_adds_feed_role_column_and_unique_index() {
    let db = Database::open_in_memory().await.unwrap();
    // column present, defaults to 'none'
    let epic = db.create_epic("E", "", None).await.unwrap();
    assert_eq!(epic.feed_role, crate::models::FeedRole::None);
    // duplicate (parent, role) rejected
    let parent = db.create_epic("P", "", None).await.unwrap();
    db.create_epic("a", "", Some(parent.id)).await.unwrap(); // then set role via patch (Task 4)
    // (assert the unique index name exists via PRAGMA index_list — see existing index tests)
}
```
- [ ] **Step 2 — run, expect fail.**
- [ ] **Step 3 — implement:**
```rust
fn migrate_v64_add_epic_feed_role(conn: &Connection) -> Result<()> {
    if !column_exists(conn, "epics", "feed_role") {
        conn.execute_batch(
            "ALTER TABLE epics ADD COLUMN feed_role TEXT NOT NULL DEFAULT 'none';",
        )
        .context("Failed to add feed_role column to epics (migration v64)")?;
    }
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_epics_parent_feed_role
             ON epics(parent_epic_id, feed_role)
             WHERE feed_role <> 'none';",
    )
    .context("Failed to add feed_role unique index (migration v64)")?;
    Ok(())
}
```
Register `(64, migrate_v64_add_epic_feed_role)` in the migrations array.
- [ ] **Step 4 — run:** `cargo test -p dispatch db::tests::migrations`
- [ ] **Step 5 — commit:** `feat(db): migration v64 — epics.feed_role + unique index`

### Task 4: `EpicPatch::feed_role`

**Files:**
- Modify: `src/db/mod.rs:114` (`patch_struct!` — add `plain feed_role: FeedRole,`)
- Modify: `src/db/queries/epics.rs:133` area (add `set_field!(sets, values, patch.feed_role, "feed_role")`)

- [ ] **Step 1 — failing test.** In `src/db/tests/epics.rs`:
```rust
#[tokio::test]
async fn patch_epic_feed_role_persists() {
    let db = Database::open_in_memory().await.unwrap();
    let e = db.create_epic("E", "", None).await.unwrap();
    db.patch_epic(e.id, &EpicPatch::new().feed_role(crate::models::FeedRole::ReviewsParent)).await.unwrap();
    let got = db.get_epic(e.id).await.unwrap().unwrap();
    assert_eq!(got.feed_role, crate::models::FeedRole::ReviewsParent);
}
```
- [ ] **Step 2 — run, expect fail.**
- [ ] **Step 3 — implement.** Add the `patch_struct!` line and the `set_field!` call. The patch writes `feed_role.as_str()` (TEXT). Match the surrounding `group_by_repo` handling exactly.
- [ ] **Step 4 — run.** `cargo test -p dispatch db::tests::epics`
- [ ] **Step 5 — commit:** `feat(db): EpicPatch.feed_role`

### Task 5: `Signal` enum + `FeedItem.signals`

**Files:**
- Modify: `src/models/tasks.rs:440` (`FeedItem`) — add enum + field
- Test: `src/feed/parse.rs` test module (or `src/models/tasks.rs` tests)

- [ ] **Step 1 — failing tests.**
```rust
#[test]
fn signal_deserializes_kebab_case() {
    let s: Vec<Signal> = serde_json::from_str(r#"["direct-request","author-bot"]"#).unwrap();
    assert_eq!(s, vec![Signal::DirectRequest, Signal::AuthorBot]);
}
#[test]
fn feed_item_signals_default_empty_and_unknown_skipped() {
    // missing field -> empty
    let item: FeedItem = serde_json::from_str(
        r#"{"external_id":"x","title":"t","description":"","status":"backlog","tag":"pr-review"}"#).unwrap();
    assert!(item.signals.is_empty());
    // unknown signal value is dropped, not fatal
    let item2: FeedItem = serde_json::from_str(
        r#"{"external_id":"x","title":"t","description":"","status":"backlog","tag":"pr-review","signals":["reviewed","bogus"]}"#).unwrap();
    assert_eq!(item2.signals, vec![Signal::Reviewed]);
}
```
- [ ] **Step 2 — run, expect fail.**
- [ ] **Step 3 — implement.** Add:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Signal {
    DirectRequest, TeamRequest, Reviewed, Commented, AuthorBot, AuthorMe,
}
```
On `FeedItem` add `#[serde(default, deserialize_with = "deserialize_lenient_signals")] pub signals: Vec<Signal>,` where the helper deserializes into `Vec<serde_json::Value>` (or `Vec<Option<Signal>>` via `#[serde(untagged)]`) and filters out unrecognised entries, logging each dropped value with `tracing::warn!`. See the soft-fail-decoding section of `docs/conventions.md` for the canonical lenient-decode helper pattern; reuse it rather than inventing a new one.
- [ ] **Step 4 — run.** `cargo test -p dispatch feed::parse` and the new tests.
- [ ] **Step 5 — commit:** `feat(models): typed FeedItem.signals with lenient decode`

---

## Done when
- `cargo test && ./scripts/check-doc-paths.sh` passes.
- `feed_role` defaults to `none`; duplicate `(parent, role)` is rejected; `signals` defaults empty and drops unknown values with a warning.
- No change to `group_by_repo` behavior (existing feed tests still green).
