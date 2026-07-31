# Design: always-visible token budget indicator

Task: #3821
Date: 2026-07-31
Revision: 2 (after adversarial review + independent fact-check; see "Review corrections")

## Problem

To learn how much of the current subscription budget is left, the user opens the
main session's tmux window and runs `/usage`. That is a navigation round-trip to
read two numbers. The board should show them continuously, in the chrome, with no
popup and no keypress.

## What `/usage` actually reports

Two rolling subscription windows — a 5-hour session window and a 7-day weekly
window — each as a percentage consumed plus a reset time. Those two windows are
the "budget left" the user cares about. Per-message token counts and
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

The emit site corroborates the optionality: `(I.five_hour||I.seven_day)&&{rate_limits:I}`
— the key is omitted wholesale when neither window exists.

Every other candidate was checked and ruled out:

| Candidate | Verdict |
|---|---|
| `claude usage` subcommand / CLI flag | Does not exist. Subcommands are agents, auth, auto-mode, doctor, gateway, install, mcp, plugin, project, setup-token, ultrareview, update. `/usage` is interactive-only. |
| Cache file under `~/.claude` | No quota state on disk. `policy-limits.json` holds enterprise restriction flags only. |
| Environment variable | No *quota* var. (`CLAUDE_CODE_RATE_LIMIT_TIER` does exist, but carries the plan tier from the auth snapshot, not usage.) |
| `anthropic-ratelimit-unified-*` response headers | The header family exists (15 variants). They reach a third party only *transformed*, as the statusLine `rate_limits` object — which is this design's source. No path to the raw headers was found. |
| OpenTelemetry export | Full metric set enumerated from the binary; `cost.usage` and `token.usage` are present, nothing quota- or window-shaped. |
| Transcript JSONL (`~/.claude/projects/**/*.jsonl`) | Empirically checked: 5,048 distinct key paths across 508 transcripts, zero `rate_limits`/`five_hour`/`seven_day`/`resets_at` keys. Carries `message.usage.{input,output,cache_*}_tokens` only. |

Two consequences shape the design:

1. **Rate limits are account-global.** Any one session is as good a reporter as
   any other, so the store is a single latest-wins snapshot, not per-task rows.
2. **The payload is absent for non-subscription auth** (API key, Bedrock, Vertex,
   Foundry) and absent until the first API response of a session. The indicator
   must therefore have a well-defined *hidden* state, not a zero state.

### Accepted risk: this is not a stable contract

`rate_limits` is not a documented API, a CLI flag, or an env var. It is a payload
field observed in one point release. Anthropic can rename or restructure it
without notice. Combined with the decorator's unconditional exit 0 (below), a
schema change on upgrade degrades to *the indicator silently stays hidden* —
indistinguishable from "user is on API-key auth". That is an acceptable failure
mode for a chrome affordance, but it is a permanent maintenance risk and is
accepted knowingly, not overlooked.

There is deliberately **no automatic detection** of this. An earlier revision put
a check in `dispatch doctor`; that was dropped because the doctor CLI is being
retired (task #3832), and because a manual check only helps someone who already
suspects a problem — which is precisely what a silent hide denies them. The
accepted position: the badge's absence on a board the user looks at constantly is
the signal, and re-running `dispatch setup` is the remedy. Documented in
`docs/reference.md` rather than enforced in code.

## Architecture

### 1. `dispatch statusline` — a decorating CLI subcommand

statusLine is settings-only; it is **not** a plugin capability. Proven three ways
in the binary: the `statusLine` schema
(`{type:"command",command,padding,refreshInterval}`) lives in the *settings*
schema; every read is from a settings source (`us()?.statusLine`,
`Pr("policySettings")?.statusLine`); and the plugin component enumerations are
`["commands","agents","output-styles","skills","workflows","routines"]` and
`["skills","agents","hooks","mcp","lsp","output-style","channel"]` — `statusLine`
is in neither. So the reporter must be wired through a settings file.

`dispatch statusline --snapshot <path> [--chain <command>]` is a transparent
decorator:

1. Read the payload JSON from stdin.
2. If `.rate_limits` is present, write the snapshot to `<path>` atomically.
3. If `--chain` is given, run that command with the same stdin bytes and print its
   stdout verbatim, bounded by a hard wall-clock timeout (2s in production); a
   chain that does not finish within budget is killed and yields empty output.
4. **Exit 0 unconditionally.** A failure anywhere in 1–3 must never blank or break
   the user's status line.

With no `--chain`, step 3 is skipped and nothing is printed — exactly what a user
with no statusLine configured had before.

Note the invocation cadence: the statusLine call site is debounced at 300 ms
(`Iee(()=>{D()},300)`), not throttled, and the schema additionally offers
`refreshInterval`. So this subcommand runs several times a second per session and
must stay cheap — it must not open the database. Its only work is one small read
and one small write.

### 2. Install site: `--settings` injection at a fixed path

`src/setup/mod.rs:455` records a deliberate invariant: `~/.claude/settings.json`
is user-owned and setup does not touch it, guarded by the regression test
`setup_does_not_write_settings_json` (`src/setup/mod.rs:739`). This design keeps
that invariant intact.

**Injection wins over the user's global config — verified, not assumed.** The
binary contains the precedence string `"Settings precedence is user < project <
local < flag < policy"`, the source list
`["userSettings","projectSettings","localSettings","flagSettings","policySettings"]`
described in-schema as "Ordered low-to-high priority — later entries override
earlier ones", and a resolver that walks that list backwards so the last defining
source wins. `--settings` is `flagSettings` and therefore outranks the user's
`userSettings`. The injected statusLine wins.

**The settings file lives at a fixed literal path**, dispatch-owned:

```
~/.claude/dispatch-statusline.json
```

It deliberately does **not** live inside the plugin directory. `install_plugin_in`
calls `remove_stale_files`, which deletes any file under the plugin dir that is not
in the `include_dir!` embedded set (`src/setup/plugins.rs:107-111`) — a generated
file there would be destroyed on the next `dispatch setup`. The plugin dir is a
wholesale mirror of embedded content; generated state does not belong in it.

Nor does this touch `settings.json`: the `src/setup/mod.rs:455` invariant is about
that specific user-owned file, and setup already creates other dispatch-owned
things under `~/.claude` (the plugin dir) and elsewhere (the tmux conf).

`DISPATCH_PLUGIN_DIR` (`src/dispatch/prompts.rs:12`) becomes:

```rust
pub(super) const DISPATCH_PLUGIN_DIR: &str =
    "--plugin-dir ~/.claude/plugins/local/dispatch \
     --settings ~/.claude/dispatch-statusline.json";
```

This is what keeps the change genuinely small, and it is a direct correction to
revision 1 (see "Review corrections" R1/R2). The constant stays a compile-time
`const` containing only fixed literals with no spaces and no shell
metacharacters, so:

- No runtime value has to reach it. A `const` cannot hold one, and making it a
  function would force a `data_dir` parameter through `dispatch_agent`,
  `research_agent`, `quick_dispatch_agent`, `resume_agent` and
  `create_main_session`, their callers in `src/mcp/handlers/tasks/dispatch.rs`,
  `src/runtime/tasks.rs`, `src/runtime/split.rs`, `src/runtime/commands.rs`, and
  roughly 30 call sites in `src/dispatch/tests.rs`.
- No shell-quoting hazard. The constant is interpolated into strings that a
  pane's shell parses (`src/dispatch/agents.rs:184`, `:345`, `:389`, sent
  literally by `tmux::send_keys`). Interpolating an unquoted runtime path there
  would break on any `HOME`/`XDG_DATA_HOME` containing a space. Fixed literals
  cannot. The `~` continues to expand because the pane's shell parses the line.

The *runtime* paths live inside that JSON file instead, where `dispatch setup`
writes them and can quote them properly:

```json
{
  "statusLine": {
    "type": "command",
    "command": "dispatch statusline --snapshot '<data_dir>/rate-limits.json' --chain 'claude-statusline'"
  }
}
```

`run_setup_in` already receives `data_dir` (`src/setup/mod.rs:236-242`), so
nothing new has to be threaded. Both interpolated values are single-quoted with
embedded single quotes escaped, since a statusLine command is itself run through a
shell.

Setup discovers the `--chain` value by *reading* (never writing)
`statusLine.command` from the user's settings, via the already-injectable
`SetupPaths.claude_dir` seam (`src/setup/mod.rs:213-231`) — which is what makes the
new setup tests hermetic, the same seam `setup_does_not_write_settings_json`
already uses.

**Recursion guard.** If the discovered command already contains a `dispatch
statusline` invocation, setup writes the file with **no `--chain` at all** and
prints a warning naming the situation. It does not chain to itself, and it does
not silently skip writing the file — the reporter still runs, and the statusLine
renders empty, which is the honest outcome for a self-referential config. This is
the observable behaviour the regression test asserts.

**Chain drift.** The `--chain` target is baked in at setup time. If the user later
changes their own `statusLine.command`, the chain keeps invoking the stale one and
step 4 hides the discrepancy. The remedy is re-running `dispatch setup`, which
rewrites the chain target from the current config; setup is idempotent, so this is
safe to repeat. There is no automatic detection — this is a known, accepted
limitation, documented rather than solved.

Coverage is every session dispatch spawns — the main session, every agent, every
resume. Ad-hoc `claude` sessions the user launches themselves do not report; the
main session alone keeps the snapshot fresh whenever the board is in use.

### 3. Store: a file, not the database

`<data_dir>/rate-limits.json`:

```json
{
  "five_hour":  { "used_percentage": 23.5, "resets_at": 1738425600 },
  "seven_day":  { "used_percentage": 41.2, "resets_at": 1738857600 },
  "captured_at": 1738421000
}
```

Either window may be absent, mirroring the source payload.

A file rather than a table, for two reasons. **Performance**: the writer runs
several times a second per session, and with N agents plus the main session that
is N+1 processes writing concurrently; routing that through the single serialized
writer connection described in the `db_call`/`db_call_read` model
(`docs/conventions.md`) would be pure waste, and the subcommand must not pay
database-open cost at all. **No invariant**: the datum is one global latest-wins
value with no history and no cross-entity relationship, so the schema and the
service mutation boundary buy it nothing.

**Concurrent-writer safety.** Every session writes the *same* path, so the atomic
write must use a **unique** temp file — `tempfile::NamedTempFile::new_in(data_dir)`
followed by `persist`, never a fixed `rate-limits.json.tmp`. With a fixed temp
name, two writers can interleave (A truncates and starts writing, B truncates the
same path, A renames B's partial bytes) and the survivor is not necessarily the
renamer's own data. With unique temp names, each writer renames only bytes it
wrote and "latest rename wins" is genuinely true — which is the correct and benign
outcome here, since all writers report the same account-global value.

This differs from the `<data_dir>/file-events/<task_id>.jsonl` sidecar
(`src/agent_tree.rs:62`): that convention is *per-task*, so it never has two
processes on one path and offers no precedent for concurrent writers. The sidecar
comparison justifies only the file's *location*, not its concurrency story.

### 4. TUI: mirror the main-session poll pattern

The existing `main_session_alive` badge is the template, followed step for step:

| Piece | Existing (main session) | New (budget) |
|---|---|---|
| Tick constant | `MAIN_SESSION_POLL_TICKS` (`src/tui/mod.rs:41`) | `BUDGET_POLL_TICKS = 5` (10 s) |
| `App` state | `main_session_alive`, `ticks_since_main_session_poll` | `budget: Option<BudgetSnapshot>`, `ticks_since_budget_poll` |
| Tick sub-step | `tick_main_session_poll` (`src/tui/update/agent.rs:373`) | `tick_budget_poll` |
| Command | `MainSessionCommand::CheckLiveness` | `BudgetCommand::Refresh` |
| Executor | `exec_check_main_session_liveness` (`src/runtime/split.rs:90`) | `exec_refresh_budget`, `spawn_blocking` |
| Message | `MainSessionMessage::LivenessChanged` | `BudgetMessage::Updated` |
| Handler | `handle_main_session_liveness` (`src/tui/update/main_session.rs:39`) | `handle_budget_updated` |
| Render | `main_session_badge` (`src/tui/ui/kanban/status_bar.rs:54`) | span in `render_top_indicators` |

**Naming: "budget", never "usage".** The repo already has an unrelated `usage`
subsystem — `usage_events` (migration v59), `UsageCategory`, `query_usage` — which
counts keypresses and MCP tool calls, and whose spec says outright that those are
"feature-usage counters … **NOT token counts**"
(`docs/specs/mcp-task-tools.allium:423`). Naming the new types `Usage*` would
collide head-on. Everything introduced here is named `Budget*`.

Two constraints carried over from that pattern, both load-bearing:

- The file read happens in `spawn_blocking`, never on the event loop.
  `docs/conventions.md:324`: "No `std::fs` inside async handlers."
- `handle_budget_updated` sets `self.dirty = true` **only when the value changed**.
  This state is invisible to the discriminant-based dirty detector in
  `handle_key`, so an unchanged poll must not force a redraw.

### 5. Render site, states, and overflow

Rendered by `render_top_indicators` (`src/tui/ui/shared.rs:153`) — the top
indicator row, right-aligned (`:197`), prepended so it sits left of the bell.

The Normal-mode status bar was rejected as the site: its composed hint text
measures exactly 204 columns for a selected running task with a tmux window
(recomputed from `push_hint_spans`, `src/tui/ui/shared.rs:236-245`, over the 19
`action_hints` for `TaskStatus::Running`), before the `[flat]`, `[active]`,
`[/query]` and `● main` badges — and it renders as a single non-wrapping
left-aligned `Paragraph` (`status_bar.rs:24`), so anything added there is silently
clipped off-screen.

**The top row must not repeat that mistake.** It has no truncation logic today,
and it is not always near-empty: in epic view it already carries `auto dispatch
[U]` / `manual dispatch [U]`, an optional feed-role label, and `group:on/off [R]`;
and the `[N/M repos]` filter badge (`:179-187`) is **not** epic-gated, so it can
appear in board view too. The budget span therefore degrades in a defined order
when the row's spans would exceed the available width:

1. Drop the `·<countdown>` suffixes (saves ~14 cols).
2. Drop the `7d` window, keeping `5h`.
3. Drop the budget indicator entirely.

Existing badges are never sacrificed to make room for it — the budget readout is
the newest and least critical occupant of that row. This ordering is specced and
snapshot-tested at narrow widths, not left to chance.

Format at full width, ~28 columns:

```
5h 23% ·2h14m  7d 41% ·4d  🔔 [N]
```

States:

- **Colour by threshold**, each window independently: green below 50%, yellow
  50–80%, red above 80%.
- **Hidden** when the snapshot file is absent or holds neither window — the
  correct steady state for API-key and cloud-provider auth. The indicator must
  never imply 0% used.
- **Per-window omission** when one window is present and the other is not.
- **Stale**: when `captured_at` is older than `BUDGET_STALE_AFTER = 10 minutes`,
  the whole indicator dims and gains an age suffix (`… (17m old)`). A budget
  number that silently stopped updating is worse than one visibly marked old. The
  threshold is injected for tests rather than compared against the wall clock, per
  `docs/conventions.md:347,372` and `./scripts/check-no-test-sleep.sh`.
- **Reset already passed**: when `resets_at` is in the past — clock skew, or a
  snapshot straddling a window rollover — render `·now` rather than a negative
  countdown. Never emit `·-2m`.
- **Percentage out of range**: clamp to 0–100 for both colour selection and
  display. A negative or >100 value renders at the clamped bound rather than
  producing nonsense.
- **Missing data dir**: the writer creates it if absent; the reader treats any
  read error as the hidden state.
- **Passive**: display-only, never selectable, focusable, or a navigation target.

## Spec changes

- New `TokenBudgetIndicator` surface in `docs/specs/dispatch.allium`, which
  already owns `DispatchingFeedback` (`:818`) and `MainSessionIndicator` (`:871`).
  One `@guarantee` per state above — including the degradation order, the
  reset-in-the-past rule, and the clamp — plus one for "derived from a live read
  off the event loop, never persisted in the DB" and one for "refreshed every
  `config.budget_poll_interval`; an unchanged refresh does not force a redraw".
- `budget_poll_interval: Duration = 10.seconds` and
  `budget_stale_after: Duration = 10.minutes` in the `core.allium` config block,
  beside `pr_poll_interval` (`:529`) and `main_session_poll_interval` (`:530`).

Spec first, then tests, then code.

## Cleanup: remove the dead `task-usage-hook`

`plugin/hooks/scripts/task-usage-hook` is registered as a `Stop` hook
(`plugin/hooks/hooks.json:23-34`, the entry itself at `:29-32`) and still runs on
every agent stop. It sums per-model tokens out of the transcript JSONL against a
hard-coded, now-stale pricing table (`:31-33`), then POSTs a `tools/call` for
**`report_usage`** to `http://localhost:${PORT}/mcp` (`:69`) with only a
`Content-Type` header (`:70`), best-effort behind `|| true` (`:92`). That tool does
not exist (`grep -rn report_usage src/` → zero matches) and its `task_usage` table
was dropped in migration v56 (`src/db/migrations.rs:1423-1426`, registered at
`:114`). It has been silently discarding its work.

Remove:

- `plugin/hooks/scripts/task-usage-hook` (the script).
- Its registration at `plugin/hooks/hooks.json:29-32`.
- **`src/setup/hooks.rs:22-28`** — `fn usage_hook_script()`, which does
  `PLUGIN_DIR.get_file("hooks/scripts/task-usage-hook").expect("task-usage-hook
  must be embedded")` — **and its test at `:36-38`**. Deleting the script without
  this makes that `expect` panic and `cargo test` fail. Revision 1 missed this
  entirely; it is the single most important correction in this document.
- The stale mention at `src/setup/hooks.rs:130`.
- The `required`-array entry at `src/setup/plugins.rs:478`.
- `docs/reference.md:201` — "Reports token usage per task".
- `docs/specs/epics.allium:271` — the stale `task_usage` reference.

Note there is **no per-file embedding to remove**: `src/setup/plugins.rs:15` is
`static PLUGIN_DIR: Dir = include_dir!("$CARGO_MANIFEST_DIR/plugin")`, and the
comment at `:13-14` states that any file under `plugin/` is picked up
automatically. Revision 1 wrongly listed an embedding registration at
`plugins.rs:33,477`; `:33` is unrelated (`is_executable`).

Reviving the hook as per-task token accounting is a separate feature from
budget-remaining and is explicitly out of scope.

## Testing

| Behaviour | Where |
|---|---|
| Parses `rate_limits`, writes snapshot atomically | `src/cli/` inline tests |
| Absent / partial `rate_limits` → no write, or partial snapshot | same |
| `--chain` passes stdin through and echoes stdout verbatim | same |
| Malformed stdin, unwritable dir, failing chained command → exit 0 | same |
| Unique temp name: two concurrent writers never publish foreign bytes | `src/cli/` inline test driving two writers |
| Does not open the database | assert no DB file is created by the subcommand |
| Setup writes `statusline.json` with quoted snapshot + discovered chain | `src/setup/` inline tests, via the `claude_dir` seam |
| Recursion guard: self-referential command → file written with no `--chain` | `src/setup/` inline tests |
| Setup still does not write `~/.claude/settings.json` | existing `setup_does_not_write_settings_json` must keep passing |
| Spawn sites carry `--settings` | `src/dispatch/tests.rs`, prompt snapshots |
| `tick_budget_poll` emits `Refresh` on the Nth tick | `src/tui/tests/` |
| Unchanged snapshot does not set `dirty`; changed one does | `src/tui/tests/` |
| Thresholds, hidden, per-window omission, stale, `·now`, clamp | `src/tui/tests/snapshots/` |
| Degradation order at narrow widths, in board AND epic view | `src/tui/tests/snapshots/` |

Prompt snapshots under `src/dispatch/snapshots/` need re-accepting for the
`--settings` flag, with `.snap.new` files cleaned up afterwards.

## Review corrections

Revision 1 was reviewed adversarially and independently fact-checked. Both
load-bearing claims survived: the `rate_limits` schema at offset 472914 is exact,
and `--settings` provably outranks `~/.claude/settings.json`. The 204-column
figure was recomputed and is arithmetically exact. Corrections applied:

| # | Revision 1 said | Corrected to |
|---|---|---|
| R1 | "one-constant change" | False — a runtime path cannot live in a `const`, and threading `data_dir` would touch 5 functions, 4 modules and ~30 tests. Restructured: the settings file sits at a fixed literal path, runtime paths live *inside* it. |
| R2 | appended a runtime path to the spawn constant | Would break on any path containing a space, since the constant is parsed by a pane's shell. Constant now holds only fixed literals. |
| R3 | "a statusLine invocation cannot satisfy `CallerIdentity`" | **Factually wrong.** `X-Caller-Kind: session` yields `CallerIdentity::Session` with no task id (`src/mcp/identity.rs:31`). The file-vs-DB conclusion stands on performance and absence-of-invariant grounds only. |
| R4 | cleanup list | Missed `src/setup/hooks.rs:22-28,36-38`, whose `expect` would panic `cargo test`. Added, along with `:130` and the correct `plugins.rs:478`. |
| R5 | "remove the embedding at `plugins.rs:33,477`" | No such embedding exists; the plugin dir is included wholesale via `include_dir!`. Claim removed. |
| R6 | atomic write unspecified | Fixed temp names allow interleaved writers to publish foreign bytes. Now requires a unique temp file. |
| R7 | top row is "nearly empty" | Not epic-gated for the repo-filter badge, and the row has no truncation logic. Added an explicit degradation order, snapshot-tested. |
| R8 | source-availability table stated absolutely | Softened where evidence was indirect: raw rate-limit headers (could not prove a negative), transcript-format instability (no primary source — claim dropped), and `CLAUDE_CODE_RATE_LIMIT_TIER` (exists, but carries tier not quota). |
| R9 | "grouped with apiKeyHelper … therefore not a plugin capability" | That grouping is a settings-trust helper and proves nothing about plugins. Replaced with the real evidence: the settings-schema location and the two plugin component enumerations. |
| R10 | no undefined-state handling | Added `resets_at` in the past, percentage clamping, and missing-data-dir. |
| R11 | recursion guard behaviour unstated | Now specified as observable behaviour: write the file with no `--chain`, warn. |
| R12 | chain drift unaddressed | Documented as a known limitation, repaired by re-running `dispatch setup`. A `doctor` check was considered and rejected — the doctor CLI is being retired (task #3832). |
| R13 | line citations | Fixed: `dispatch.allium:818` (was 826), `hooks.json:23-34`/`:29-32` (was 27-36), `tui/mod.rs:41` (was 37), `update/agent.rs:373` (was 371), `runtime/split.rs:90` (was 86). |
| R14 | "~300 ms throttle" | It is a debounce, and `refreshInterval` can add periodic re-runs. Reinforces the "must not open the DB" constraint, now explicit. |
| — | stability of the source | Added an explicit accepted-risk section; revision 1 presented a binary-string discovery as though it were a contract. |
| R15 | settings file placed in the plugin dir (revision 2, pre-plan) | Found while writing the implementation plan: `remove_stale_files` (`src/setup/plugins.rs:107-111`) deletes any non-embedded file under the plugin dir, so it would be destroyed on the next `dispatch setup`. Moved to `~/.claude/dispatch-statusline.json`. |
