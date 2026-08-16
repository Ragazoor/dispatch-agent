# Design: Programmable Epics (Replace Review/Security Boards)

## Context

The dispatch TUI currently has two hardcoded board tabs — Review Board and Security
Board — each with bespoke data sources, column layouts, message types, and rendering
code. This creates a maintenance burden and limits extensibility.

The goal is to replace these specialised boards with a general "feed epic" mechanism:
an epic with a shell command that periodically runs to populate its tasks from external
data. Review and security boards are then reimplemented as pre-seeded feed epics using
built-in fetch subcommands.

## Concept: Feed Epic

A **feed epic** is a root epic with a `feed_command` string. On a configurable interval
the command runs, its JSON output is parsed, and the resulting items are upserted as
tasks inside the epic. The epic's standard kanban view (already built) renders these
tasks like any other.

```
dispatch fetch-reviews        # prints JSON task specs for open PRs
dispatch fetch-security       # prints JSON task specs for security alerts
```

Any user-defined shell command producing the same JSON schema also works, enabling
fully custom boards (CI dashboards, issue trackers, etc.) without touching dispatch source.

## Data Model Changes

### Epic table

Add two columns via a new DB migration:

| Column | Type | Default | Meaning |
|---|---|---|---|
| `feed_command` | `TEXT NULL` | `NULL` | Shell command to run; `NULL` means no feed |
| `feed_interval_secs` | `INTEGER NULL` | `NULL` | Poll interval; `NULL` inherits global default |

### Task table

Add one column:

| Column | Type | Default | Meaning |
|---|---|---|---|
| `external_id` | `TEXT NULL` | `NULL` | Stable key from feed output; enables upsert matching |

Add a unique index `(epic_id, external_id)` (where `external_id IS NOT NULL`) for
efficient upsert.

### Feed command output schema

The command writes a JSON array to stdout:

```json
[
  {
    "external_id": "pr:owner/repo#123",
    "title": "Review PR #123: fix auth bug",
    "description": "Optional additional detail",
    "url": "https://github.com/...",
    "status": "backlog"
  }
]
```

`status` is one of `backlog | running | review | done`. On first creation the status is
applied; on subsequent refreshes the status field in the feed output is **ignored** so
that user-driven workflow state is preserved.

## Built-in Fetch Subcommands

Two new CLI subcommands replace the current `gh`-backed fetch logic:

- `dispatch fetch-reviews` — wraps the existing `github::fetch_review_prs()` and
  `github::fetch_bot_prs()` logic, outputs JSON task specs
- `dispatch fetch-security` — wraps `github::fetch_security_alerts()`, outputs JSON
  task specs

These live in a new `src/feed/` module. The existing GitHub fetch functions in
`src/github.rs` are reused directly; only the output format changes.

**Mapping: ReviewWorkflowState → TaskStatus**

| ReviewWorkflowState | TaskStatus |
|---|---|
| `backlog` | `backlog` |
| `ongoing` | `running` |
| `action_required` | `review` |
| `done` | `done` |

**Mapping: SecurityWorkflowState → TaskStatus** (same pattern)

## Tab Bar Evolution

Current: `Board → Review → Security → Board` (Tab key)

New: `Board → <feed epics in sort_order> → Board`

Feed epics (those with `feed_command IS NOT NULL`) appear as extra tabs after the main
board, ordered by `sort_order`. The tab bar renders feed epic titles instead of
hardcoded "Reviews" / "Security" labels. Switching to a feed epic opens
`ViewMode::Epic { epic_id, ... }` for that epic.

This means the tab bar becomes data-driven: adding a new feed epic automatically adds a
new tab without any code change.

## Pre-seeded Feed Epics

`dispatch setup` (in `src/setup.rs`) is extended to check for and create the two
built-in feed epics if they don't already exist:

```
title:         "Reviews"
feed_command:  "dispatch fetch-reviews"
feed_interval: 30s
sort_order:    -2   (ensures they appear before user epics)

title:         "Security"
feed_command:  "dispatch fetch-security"
feed_interval: 300s
sort_order:    -1
```

These are regular epics; users can rename, delete, or reconfigure them.

## Feed Runner (src/feed.rs)

A new `FeedRunner` struct runs in the background (spawned from `runtime.rs`):

1. On startup: run all feed commands immediately.
2. On tick: check which feed epics are due (elapsed ≥ `feed_interval`).
3. For each due epic: spawn command as `tokio::process::Command`, capture stdout,
   parse JSON, upsert tasks via `db.upsert_feed_tasks(epic_id, items)`.
4. Send `McpEvent::Refresh` after each upsert.

Error handling: if the command fails or output is invalid JSON, log a status message;
do not clear existing tasks.

## What Gets Removed

After the feed epic mechanism is working and the two built-in fetch commands cover the
existing feature set:

- `ViewMode::ReviewBoard` and `ViewMode::SecurityBoard` variants in `types.rs`
- All `ReviewBoard*` and `SecurityBoard*` message/command variants
- `ReviewBoardState`, `SecurityBoardState`, `DependabotBoardState` from the `App` struct
- `src/tui/ui/review.rs` (~719 lines)
- `src/tui/ui/security.rs` (~704 lines)
- Review/security key handlers in `src/tui/input.rs` (~335 lines combined)
- All review/security message handling in `src/tui/mod.rs`
- `REVIEW_REFRESH_INTERVAL`, `SECURITY_POLL_INTERVAL` constants (replaced by per-epic config)
- `review-board.allium` and `security.allium` specs (content moves to new `feeds.allium`)

## What is NOT Changed (initially)

- The core Epic and Task data model beyond the new columns
- The standard epic kanban view rendering (already works for feed epics)
- MCP tools (epics/tasks work the same; feed is just a data source)
- Allium specs for tasks and core (unchanged)

## Implementation Phases

### Phase 1 — Data model & feed runner (no UI removal yet)
1. DB migration: add `feed_command`, `feed_interval_secs` to epics; `external_id` to tasks
2. Update `Epic` struct, `EpicPatch`, `UpdateEpicParams`
3. `db::upsert_feed_tasks()` method
4. `src/feed.rs` — `FeedRunner` with JSON parsing and upsert logic
5. Wire `FeedRunner` into `runtime.rs`
6. Tests: upsert semantics, status preservation on re-run, invalid output handling

### Phase 2 — Built-in fetch commands
1. `dispatch fetch-reviews` subcommand (reuses existing GitHub fetch logic)
2. `dispatch fetch-security` subcommand
3. Tests: JSON output schema validation

### Phase 3 — Tab bar and feed epic navigation
1. Tab bar: discover feed epics from DB, render as dynamic tabs
2. `Tab` key cycles through feed epics (opens `ViewMode::Epic`)
3. Setup: seed "Reviews" and "Security" feed epics
4. Tests: tab switching, feed epic creation

### Phase 4 — Remove dedicated boards
1. Remove `ViewMode::ReviewBoard`, `ViewMode::SecurityBoard`
2. Remove associated messages, commands, state structs
3. Remove `ui/review.rs`, `ui/security.rs`
4. Trim `input.rs` and `mod.rs`
5. Update `core.allium` config section (remove `review_refresh_interval`,
   `security_poll_interval`; add `default_feed_interval`)
6. Write `docs/specs/feeds.allium` covering the feed epic concept
7. Remove `review-board.allium`, `security.allium` (or mark deprecated)

## Open Questions / Risks

- **Feature parity for review actions**: The current review board supports approve,
  merge, dispatch-review-agent, and manual column moves per-PR. These actions are
  currently triggered from the custom review board view. In the feed epic view, these
  would need to be available as task-level actions. Some (dispatch agent, move status)
  already work on tasks; others (approve PR, merge) may need new task actions.
- **Agent continuity**: Review agents currently track by `PrRef`. After migration,
  they'd track by task (via `task.pr_url`). The handle lookup changes.
- **Dependabot sub-view**: The review board has a `Reviewer | Dependabot` mode toggle.
  In the feed model, these could be two separate feed epics or filtered views of one.
- **Sort/filter within a feed epic**: The current boards have repo filters and severity
  filters. The standard epic view doesn't have filtering yet; this may need to be added.

## Files to Create or Modify

| File | Action |
|---|---|
| `src/db/migrations.rs` | Add migration (epic feed columns + task external_id) |
| `src/db/mod.rs` | Add `upsert_feed_tasks()`, update `EpicPatch` |
| `src/db/queries.rs` | Implement upsert query |
| `src/models.rs` | Add `feed_command`, `feed_interval_secs` to `Epic`; `external_id` to `Task` |
| `src/service.rs` | Expose feed fields in `UpdateEpicParams` |
| `src/feed.rs` | New: `FeedRunner`, JSON schema, command execution |
| `src/main.rs` | Add `fetch-reviews` and `fetch-security` subcommands |
| `src/runtime.rs` | Wire `FeedRunner` |
| `src/setup.rs` | Seed built-in feed epics |
| `src/tui/ui/shared.rs` | Dynamic tab bar from DB |
| `src/tui/input.rs` | Update Tab key handler |
| `src/tui/mod.rs` | Remove review/security message handling (Phase 4) |
| `src/tui/types.rs` | Remove ReviewBoard/SecurityBoard ViewMode variants (Phase 4) |
| `docs/specs/feeds.allium` | New spec for feed epic concept |
| `docs/specs/core.allium` | Update config section |
