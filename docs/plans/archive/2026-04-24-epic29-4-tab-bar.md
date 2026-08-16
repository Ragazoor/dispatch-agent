# Task: Dynamic Tab Bar + Setup Seeding

## Context

Fourth phase. Makes the tab bar data-driven: feed epics (those with `feed_command IS
NOT NULL`) appear as tabs. The hardcoded "Reviews" and "Security" tabs are removed from
the tab bar rendering. `dispatch setup` pre-seeds the two built-in feed epics.

Depends on phase 1 (DB columns) and can be developed alongside phases 2 and 3.
The dedicated board views are NOT removed here — that is phase 5.

## Design

### Tab bar (src/tui/ui/shared.rs)

`render_tab_bar()` currently hardcodes three tabs: Tasks | Reviews | Security.

New behaviour:
1. Accept `feed_epics: &[Epic]` as an additional parameter (epics with
   `feed_command IS NOT NULL`, ordered by `sort_order`)
2. Render: `Tasks | {feed_epic.title} | ...` dynamically
3. Active tab highlight: check `ViewMode` — `Board(_)` or `Epic(_)` → highlight the
   matching tab by epic_id; `Board(_)` without an active feed epic → highlight Tasks

The `App` struct already holds all epics; filter for feed epics at render time.

### Tab switching (src/tui/input.rs + src/tui/mod.rs)

Current `Tab` key: Board → ReviewBoard → SecurityBoard → Board

New `Tab` key: Board → FeedEpic[0] → FeedEpic[1] → ... → Board

In the `Tab` key handler:
- Fetch `feed_epics` list (epics with feed_command, sorted)
- If currently in `ViewMode::Board`: switch to first feed epic (if any), else stay
- If currently in `ViewMode::Epic { epic_id }` and that epic is a feed epic: switch to
  next feed epic, or back to Board if it was the last one
- If in another ViewMode: no-op or delegate to existing handler

Message: `Message::SwitchToFeedEpic(EpicId)` → sets `ViewMode::Epic { epic_id, ... }`

Back navigation: `Esc` from a feed epic view returns to Board (same as existing epic
`Esc` behaviour — no change needed).

### Setup seeding (src/setup.rs)

`dispatch setup` is extended to create the two built-in feed epics if they don't
already exist (checked by querying epics with `feed_command = 'dispatch fetch-reviews'`
and `feed_command = 'dispatch fetch-security'`):

```rust
fn seed_feed_epics(db: &Database) -> Result<()> {
    let epics = db.list_epics()?;
    let has_reviews = epics.iter().any(|e| e.feed_command.as_deref()
        == Some("dispatch fetch-reviews"));
    let has_security = epics.iter().any(|e| e.feed_command.as_deref()
        == Some("dispatch fetch-security"));

    if !has_reviews {
        db.create_epic(CreateEpicParams {
            title: "Reviews".into(),
            feed_command: Some("dispatch fetch-reviews".into()),
            feed_interval_secs: Some(30),
            sort_order: Some(-2),
            ..Default::default()
        })?;
    }
    if !has_security {
        db.create_epic(CreateEpicParams {
            title: "Security".into(),
            feed_command: Some("dispatch fetch-security".into()),
            feed_interval_secs: Some(300),
            sort_order: Some(-1),
            ..Default::default()
        })?;
    }
    Ok(())
}
```

Negative `sort_order` ensures they sort before user epics (which default to `NULL`,
treated as the epic id).

## TDD Checklist

- [ ] Write test: `Tab` from `ViewMode::Board` → switches to first feed epic view
- [ ] Write test: `Tab` from last feed epic → returns to `ViewMode::Board`
- [ ] Write test: `Tab` from non-feed epic view → no-op
- [ ] Write test: no feed epics in DB → `Tab` is a no-op from Board
- [ ] Write snapshot test: tab bar renders feed epic titles dynamically
- [ ] Write test: `seed_feed_epics` creates both epics when DB is empty
- [ ] Write test: `seed_feed_epics` is idempotent (re-running does not duplicate epics)
- Then implement minimum code to pass

## Files

- `src/tui/ui/shared.rs` — dynamic tab bar
- `src/tui/input.rs` — Tab key handler
- `src/tui/mod.rs` — `Message::SwitchToFeedEpic` handler
- `src/tui/types.rs` — add `Message::SwitchToFeedEpic(EpicId)` if needed
- `src/setup.rs` — `seed_feed_epics()`
- `src/tui/tests/snapshots/` — update affected snapshots
