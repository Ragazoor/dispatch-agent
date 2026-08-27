# 4412 — Enforce a minimum `feed_interval_secs` at the service boundary

Date: 2026-08-27
Task: #4412

## Problem

`Epic.feed_interval_secs` has a strict rule on one write path and none on the
others.

- The epic editor's `FEED_INTERVAL_SECS` section parses through
  `src/models/interval.rs::parse_interval_secs`, which rejects `0` and negatives.
- `EpicService::update_epic` and `EpicService::create_epic` apply the integer
  straight through with no bound.
- `set_managed_feed_config` validates `>= 0` **in the MCP handler**, and its
  spec explicitly blesses `0` as "poll every tick". That value reaches
  `epic.feed_interval_secs` via `ensure_role_epic`, which patches the DB
  directly.

`FeedRunner` then does `Duration::from_secs(s as u64)`:

- `0` → always due, so the command respawns every 2s poll tick, forever.
- negative → `as u64` wraps to a near-infinite duration, so the feed goes
  permanently silent with no trace.

`docs/specs/core.allium` records this as a `KNOWN GAP` block under "Interval
literals". This task closes it.

## Decisions (elicited 2026-08-27)

1. **The floor is 60s, and it binds the *resolved* interval** — the value the
   runner actually uses, including the fallback for an unset epic. So
   `default_feed_interval` rises from 30s to 60s. One number means one thing
   everywhere; a blank field must not poll faster than the fastest value you
   are permitted to type.
2. **The floor applies to managed-feed intervals too.** The handler's `>= 0`
   check is replaced by a `>= min_feed_interval` check in the service, next to
   `update_epic`'s. The "0 = poll every tick" affordance is deleted.
3. **`FeedRunner` skips an epic whose stored interval is below the floor**, or
   negative, and warns on every tick. Chosen over clamping: a misconfigured
   feed should not run at all.
4. **A one-time migration clamps every sub-60 stored interval up to 60**,
   keeping the value explicit rather than nulling it.
5. **The editor grammar keeps `> 0` and only parses.** The service rejects a
   sub-floor value and the TUI surfaces the error. Positivity-and-floor is a
   domain invariant of the field; the grammar is a spelling rule for humans.
   Keeping them apart is the point of the fix.
6. **The floor is a config entry, `min_feed_interval`**, implemented as a
   `const` — exactly how `default_feed_interval` already works. There is no
   settings-table row behind either, so nothing can relax the floor at runtime.

### Consequences the decisions force

- **The settings rows need migrating too.** `reviews_feed_interval_secs` and
  `cve_feed_interval_secs` live in the settings table, not on `epics`.
  Decision 2 floors them, so decision 4 must clean them as well.
- **The negative wrap gets a real fix.** `s as u64` stops being load-bearing;
  use a checked conversion.
- **An optimistic-update bug becomes reachable.** `src/runtime/editor.rs`
  logs an `update_epic` failure and then still emits
  `EpicMessage::Edited(updated)` carrying the rejected value, so the board
  would show a number the DB refused. Nearly unreachable today; routine once
  the floor exists. Return early on `Err`.
- **Three feed tests use `feed_interval_secs(Some(0))`** to force "always due"
  without sleeping (`src/feed/mod.rs:1285`, `1334`, `1638`). Decision 3 breaks
  them. New lever: `last_run` is an in-memory `HashMap<EpicId, Instant>` on the
  runner and the tests are in the same module, so removing the epic's entry
  makes `elapsed` be `Duration::MAX` — always due, no sleep, no bad row.
- `src/setup/plugins.rs:250` seeds `Some(300)`, safely above the floor.

## Step 1 — Spec

`docs/specs/core.allium`

- config block: `default_feed_interval: Duration = 60.seconds`; add
  `min_feed_interval: Duration = 60.seconds` beside it, with the config
  invariant `default_feed_interval >= min_feed_interval`.
- "Interval literals": delete the `KNOWN GAP` block. Replace it with the
  settled rule, split into its two separate claims — the grammar is a
  human-surface spelling rule that only parses, and the floor is a universal
  domain invariant enforced in the epic service, inherited by every entry
  point.

`docs/specs/epics.allium`

- `CreateEpic` and `UpdateEpic`: `requires feed_interval_secs >= min_feed_interval`
  when set.
- `SetManagedFeedConfig`: replace `>= 0` with `>= min_feed_interval`; drop the
  "0 is allowed and means poll every tick" sentence.
- `EditEpic`: note that the section parses only, and a sub-floor value is
  refused by the service with the error surfaced in the TUI.

`docs/specs/feeds.allium`

- `FeedTick`: an epic whose stored `feed_interval_secs` is below
  `min_feed_interval` is skipped and warned about on every tick, not polled.

## Step 2 — Tests (must fail first)

- `src/service/epics.rs` — `update_epic` rejects `0`, `-5`, `59`; accepts `60`
  and explicit null. Same four for `create_epic`.
- `src/service/managed_feeds.rs` — `write_managed_feed_settings` rejects a
  sub-floor reviews or CVE interval.
- `src/mcp/handlers/tests/` — `set_managed_feed_config` returns a validation
  error for `0` (the case its spec used to bless).
- `src/feed/mod.rs` — an epic with a stored sub-floor interval is skipped, not
  polled; same for a negative. The unset-interval epic polls at 60s.
- `src/db/tests/migrations.rs` — v91 clamps `epics.feed_interval_secs` rows and
  both settings rows; leaves conforming values alone; is idempotent.
- `src/runtime/editor.rs` — a rejected epic edit emits no `Edited` message.
- Rework the three zero-interval feed tests onto the `last_run` lever.

## Step 3 — Code

| File | Change |
|---|---|
| `src/models/interval.rs` | Grammar unchanged. Doc comment points at the service for the floor. |
| `src/feed/mod.rs` | `DEFAULT_FEED_INTERVAL` 30s → 60s; add `MIN_FEED_INTERVAL_SECS`; skip + warn on sub-floor; checked conversion. |
| `src/service/epics.rs` | Validate in `create_epic` and `update_epic`. |
| `src/service/managed_feeds.rs` | Validate in `write_managed_feed_settings`. |
| `src/mcp/handlers/managed_feeds.rs` | Delete the handler's `>= 0` check. |
| `src/mcp/handlers/dispatch.rs` | Tool descriptions state the 60s minimum (lines 258, 444, 446). |
| `src/db/migrations.rs` | `v91_clamp_feed_intervals_to_minimum`. |
| `src/runtime/editor.rs` | Early return on `update_epic` error. |
| `docs/reference.md` | Default 30s → 60s; mention the floor (lines 211, 236-237). |

Where the two constants live: `MIN_FEED_INTERVAL_SECS` beside
`DEFAULT_FEED_INTERVAL` in `src/feed/mod.rs` only if the service can reach it
without a layering inversion; otherwise both move to `src/models/interval.rs`,
which already owns the interval domain facts. Keep them as two separate
constants that happen to be equal, not one derived from the other — a later
default of 120s must not move the floor.

## Step 4 — Verify

`cargo test` green, then `allium:weed` to confirm spec and code agree.
