# Task: Feed Runner

## Context

Second phase of the "programmable epics" refactor. Implements the background polling
engine that runs feed commands and upserts their output into tasks. Depends on the
DB migration (phase 1) being merged first.

## Design

### FeedItem (src/feed.rs or src/models.rs)

Deserialised from feed command JSON output:

```rust
#[derive(Deserialize)]
pub struct FeedItem {
    pub external_id: String,
    pub title: String,
    pub description: Option<String>,
    pub url: Option<String>,
    pub status: FeedItemStatus,  // backlog | running | review | done
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FeedItemStatus { Backlog, Running, Review, Done }
```

### FeedRunner (src/feed.rs)

```rust
pub struct FeedRunner {
    db: Arc<dyn TaskStore>,
    notify: mpsc::UnboundedSender<McpEvent>,
    last_run: HashMap<EpicId, Instant>,
}
```

Public API:
- `FeedRunner::new(db, notify) -> Self`
- `async fn tick(&mut self)` — called from the runtime event loop on each tick:
  1. Query all epics with `feed_command IS NOT NULL` from DB
  2. For each: check if `elapsed >= feed_interval_secs` (default 30s if NULL)
  3. If due: spawn `tokio::process::Command::new("sh").arg("-c").arg(cmd)` with
     stdout piped
  4. Parse stdout as `Vec<FeedItem>` (serde_json)
  5. Call `db.upsert_feed_tasks(epic_id, &items)`
  6. Send `McpEvent::Refresh`
  7. Update `last_run[epic_id]`
- On command failure or JSON parse error: send a status message (not a panic);
  existing tasks are preserved

### Runtime wiring (src/runtime.rs)

In `TuiRuntime`:
- Add `feed_runner: FeedRunner` field
- In `run_event_loop()` tick handler: call `self.feed_runner.tick().await`

The feed runner runs on each 2-second tick but only launches commands when their
interval has elapsed, so the tick overhead is negligible (hash map lookup + Instant
comparison per epic).

### Error handling

Feed command errors are non-fatal:
- Command exits non-zero → log status message "Feed error: <epic title>: <stderr>"
- Stdout is not valid JSON → log status message, skip upsert
- DB upsert fails → propagate as `anyhow::Result` error, log

## TDD Checklist

- [ ] Write test: `FeedRunner::tick()` with a mock command that outputs valid JSON →
      tasks are upserted (use `Database::open_in_memory()` + a script that echoes JSON)
- [ ] Write test: command exits non-zero → existing tasks untouched, no panic
- [ ] Write test: stdout is malformed JSON → existing tasks untouched, no panic
- [ ] Write test: interval not yet elapsed → command not re-run (last_run respected)
- [ ] Write test: feed_command IS NULL on an epic → skipped silently
- Then implement minimum code to pass

## Files

- `src/feed.rs` — new file: `FeedItem`, `FeedItemStatus`, `FeedRunner`
- `src/lib.rs` — add `pub mod feed;`
- `src/runtime.rs` — add `feed_runner` field, wire `tick()` call
- `src/mcp/mod.rs` — no change needed (McpEvent::Refresh already exists)
