# Design: always-visible token budget indicator

Task: #3821
Date: 2026-07-31

## Problem

To learn how much of the current subscription budget is left, the user opens the
main session's tmux window and runs `/usage`. That is a navigation round-trip to
read two numbers. The board should show them continuously, in the chrome, with no
popup and no keypress.

## What `/usage` actually reports

`/usage` shows two rolling subscription windows — a 5-hour session window and a
7-day weekly window — each as a percentage consumed plus a reset time. Those two
windows are the "budget left" the user cares about. Per-message token counts and
context-window occupancy are a different question and are out of scope.

## Data source

There is exactly one programmatic source for the rate-limit windows: the
**statusLine hook payload**.

Verified against the installed binary
(`/home/ragge/.local/share/claude/versions/2.1.220`, `strings` offset 472914),
which embeds the payload schema:

```
"rate_limits": {          // Optional: Claude.ai subscription usage limits.
                          // Only present for subscribers after first API response.
  "five_hour": {          // Optional: 5-hour session limit (may be absent)
    "used_percentage": number,   // 0-100
    "resets_at": number          // Unix epoch seconds
  },
  "seven_day": { ... }    // Optional: 7-day weekly limit (may be absent)
}
```

Every other candidate was checked and ruled out:

| Candidate | Verdict |
|---|---|
| `claude usage` subcommand / CLI flag | Does not exist. `/usage` is interactive-only. |
| Cache file under `~/.claude` | None written. No quota state on disk. |
| Environment variable | None. |
| `anthropic-ratelimit-unified-*` response headers | Consumed internally; never surfaced to a third party. |
| OpenTelemetry export | Emits `claude_code.cost.usage` and `claude_code.token.usage`; **no** quota or window metric. |
| Transcript JSONL (`~/.claude/projects/**/*.jsonl`) | Carries per-message `usage` token counts only — no rate-limit or window state. Format is explicitly internal and unstable. |

Two consequences shape the design:

1. **Rate limits are account-global.** Any one session is as good a reporter as
   any other, so the store is a single latest-wins snapshot, not per-task rows.
2. **The payload is absent for non-subscription auth** (API key, Bedrock, Vertex,
   Foundry) and absent until the first API response of a session. The indicator
   must therefore have a well-defined hidden state, not a zero state.

## Architecture

### 1. `dispatch statusline` — a decorating CLI subcommand

statusLine is a settings-only key. It is **not** a plugin capability: it is
grouped in the binary with `apiKeyHelper`, `processWrapper` and
`subagentStatusLine` as settings-sourced command keys, and the built-in
`statusline-setup` agent is instructed to edit `~/.claude/settings.json`. So the
reporter has to be wired through a settings file.

`dispatch statusline --chain <command>` behaves as a transparent decorator:

1. Read the payload JSON from stdin.
2. If `.rate_limits` is present, atomically write the snapshot to
   `<data_dir>/rate-limits.json`.
3. Run `<command>`, feeding it the same stdin bytes, and print its stdout
   verbatim.
4. **Exit 0 unconditionally.** A failure anywhere in 1–3 must never blank or
   break the user's status line.

With no `--chain`, step 3 is skipped and nothing is printed — which is exactly
what a user with no statusLine configured had before.

### 2. Install site: `--settings` injection, not global config

`src/setup/mod.rs:455` records a deliberate invariant: `~/.claude/settings.json`
is user-owned and setup does not touch it, guarded by the regression test
`setup_does_not_write_settings_json` (`src/setup/mod.rs:739`). This design keeps
that invariant.

Instead, `dispatch setup`:

- reads (read-only) the user's existing `statusLine.command` from
  `~/.claude/settings.json`, and
- writes a dispatch-owned fragment to `<data_dir>/statusline.json`:

```json
{
  "statusLine": {
    "type": "command",
    "command": "dispatch statusline --chain claude-statusline"
  }
}
```

`DISPATCH_PLUGIN_DIR` (`src/dispatch/prompts.rs:12`) gains
`--settings <data_dir>/statusline.json`. All three spawn sites already share that
constant — dispatched agents (`src/dispatch/agents.rs:184`), resume (`:345`), and
the main session (`:389`) — so this is a one-constant change. `--settings` is
documented as loading *additional* settings, so it layers over the user's global
config rather than replacing it.

Coverage is every session dispatch spawns: the main session, every agent, and
every resume. Ad-hoc `claude` sessions the user launches themselves do not
report. That is accepted: the main session alone keeps the snapshot fresh
whenever the board is in use.

Recursion guard: if the discovered command is already a `dispatch statusline`
invocation, it is not chained.

### 3. Store: a file, not the database

`<data_dir>/rate-limits.json`, written atomically (temp file + rename):

```json
{
  "five_hour":  { "used_percentage": 23.5, "resets_at": 1738425600 },
  "seven_day":  { "used_percentage": 41.2, "resets_at": 1738857600 },
  "captured_at": 1738421000
}
```

Either window may be absent, mirroring the source payload.

A file rather than a table, for three reasons. The statusLine runs on a ~300 ms
throttle, so a per-render HTTP round-trip plus a write through the single
serialized writer connection would be pure waste. A statusLine invocation
carries no task id, so it cannot satisfy `CallerIdentity` and cannot reach the
MCP server at all — this is precisely why the dead `task-usage-hook` (below)
could never have worked. And the datum is one global latest-wins value with no
history and no cross-entity invariant, so it gains nothing from the schema.
`<data_dir>/file-events/<task_id>.jsonl` already establishes the
sidecar-file-in-data-dir convention.

### 4. TUI: mirror the main-session poll pattern

The existing `main_session_alive` badge is the template, followed step for step:

| Piece | Existing (main session) | New (budget) |
|---|---|---|
| Tick constant | `MAIN_SESSION_POLL_TICKS` (`src/tui/mod.rs:37`) | `BUDGET_POLL_TICKS = 5` (10 s) |
| `App` state | `main_session_alive`, `ticks_since_main_session_poll` | `budget: Option<BudgetSnapshot>`, `ticks_since_budget_poll` |
| Tick sub-step | `tick_main_session_poll` (`src/tui/update/agent.rs:371`) | `tick_budget_poll` |
| Command | `MainSessionCommand::CheckLiveness` | `BudgetCommand::Refresh` |
| Executor | `exec_check_main_session_liveness` (`src/runtime/split.rs:86`) | `exec_refresh_budget`, `spawn_blocking` |
| Message | `MainSessionMessage::LivenessChanged` | `BudgetMessage::Updated` |
| Handler | `handle_main_session_liveness` (`src/tui/update/main_session.rs:39`) | `handle_budget_updated` |
| Render | `main_session_badge` (`src/tui/ui/kanban/status_bar.rs:54`) | span in `render_top_indicators` |

**Naming: "budget", never "usage".** The repo already has an unrelated `usage`
subsystem — `usage_events`, `UsageCategory`, `query_usage` — which counts
keypresses and MCP tool calls, and whose spec states outright that those are
"feature-usage counters … **NOT token counts**"
(`docs/specs/mcp-task-tools.allium:423`). Naming the new types `Usage*` would
collide with it head-on. Everything introduced here is named `Budget*`.

Two constraints carried over from that pattern, both load-bearing:

- The file read happens in `spawn_blocking`, never on the event loop —
  `docs/conventions.md` forbids `std::fs` in async paths.
- `handle_budget_updated` sets `self.dirty = true` **only when the value
  changed**. This state is invisible to the discriminant-based dirty detector in
  `handle_key`, so an unchanged poll must not force a redraw.

### 5. Render site and states

Rendered by `render_top_indicators` (`src/tui/ui/shared.rs:153`) — the top
indicator row, right-aligned, prepended so it sits left of the bell.

The Normal-mode status bar was rejected as the site: its composed hint text
already measures 204 columns for a selected running task, before the `[flat]`,
`[active]`, `[/query]` and `● main` badges, and it renders as a single non-wrapping
left-aligned `Paragraph` — so anything added there is silently clipped off-screen
on a normal terminal. The top row holds only the bell in board view and is empty
across nearly its whole width.

Format, ~28 columns:

```
5h 23% ·2h14m  7d 41% ·4d  🔔 [N]
```

States:

- **Colour by threshold** on each window independently: green below 50%, yellow
  50–80%, red above 80%.
- **Hidden** when `<data_dir>/rate-limits.json` is absent or holds neither
  window. This is the correct steady state for API-key and cloud-provider auth,
  which never emit `rate_limits` — the indicator must not imply 0% used.
- **Per-window omission** when one window is present and the other is not.
- **Stale**: when `captured_at` is older than `BUDGET_STALE_AFTER = 10 minutes`,
  the whole indicator dims and gains an age suffix (`5h 23% ·2h14m  7d 41% ·4d
  (17m old)`). A budget number that silently stopped updating is worse than one
  visibly marked old. The threshold is injected for tests rather than compared
  against the wall clock, per the no-test-sleep rule in `docs/conventions.md`.
- **Passive**: display-only, never selectable, focusable, or a navigation target.

## Spec changes

- New `TokenBudgetIndicator` surface in `docs/specs/dispatch.allium`, which
  already owns `MainSessionIndicator` (`:871`) and `DispatchingFeedback` (`:826`).
  One `@guarantee` per state above, plus one for "derived from a live read off the
  event loop, never persisted in the DB" and one for "refreshed every
  `config.usage_poll_interval`; an unchanged refresh does not force a redraw".
- `budget_poll_interval: Duration = 10.seconds` and
  `budget_stale_after: Duration = 10.minutes` in the `core.allium` config block,
  beside `main_session_poll_interval` (`:530`) and `pr_poll_interval` (`:529`).

Spec first, then tests, then code.

## Cleanup: remove the dead `task-usage-hook`

`plugin/hooks/scripts/task-usage-hook` is registered as a `Stop` hook
(`plugin/hooks/hooks.json:27-36`) and still runs on every agent stop. It sums
per-model tokens out of the transcript JSONL against a hard-coded and now-stale
pricing table, then POSTs a `tools/call` for **`report_usage`** to the MCP server.
That tool no longer exists (`grep -rn report_usage src/` → nothing), its
`task_usage` table was dropped in migration v56
(`src/db/migrations.rs:1423-1426`), and the POST carries no identity header so it
could not authorize even if the handler were restored. It has been silently
discarding its work.

Remove: the script, its `hooks.json` registration, its embedding in
`src/setup/plugins.rs:33,477`, the stale "Reports token usage per task" line at
`docs/reference.md:201`, and the stale `task_usage` reference at
`docs/specs/epics.allium:271`.

Reviving it as per-task token accounting is a separate feature from
budget-remaining and is explicitly not in scope here.

## Testing

| Behaviour | Where |
|---|---|
| `dispatch statusline` parses `rate_limits`, writes snapshot atomically | `src/cli/` inline tests |
| Absent / partial `rate_limits` → no write, or partial snapshot | same |
| `--chain` passes stdin through and echoes stdout verbatim | same |
| Malformed stdin, unwritable data dir, failing chained command → exit 0 | same |
| Recursion guard: existing `dispatch statusline` command not re-chained | `src/setup/` inline tests |
| Setup writes `<data_dir>/statusline.json` with discovered chain command | `src/setup/` inline tests |
| Setup still does not write `~/.claude/settings.json` | existing `setup_does_not_write_settings_json` must keep passing |
| Spawn sites carry `--settings` | `src/dispatch/tests.rs`, prompt snapshots |
| `tick_usage_poll` emits `Refresh` on the Nth tick | `src/tui/tests/` |
| Unchanged snapshot does not set `dirty`; changed one does | `src/tui/tests/` |
| Colour thresholds, hidden, per-window omission, stale states | `src/tui/tests/snapshots/` |

Prompt snapshots under `src/dispatch/snapshots/` will need re-accepting for the
`--settings` flag, with `.snap.new` files cleaned up afterwards.
