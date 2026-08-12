# Feed stderr evidence — design

**Date**: 2026-08-12
**Task**: #3900

## Problem

Refreshing the "PR Reviews" feed epic left its My/Team/Bots sub-epics empty, with no
evidence anywhere of why.

The cause was outside dispatch: the active `gh` account had no access to the
`annotell`/`kognic-internal` orgs, so all four repo-scoped `gh search prs` queries in
`fetch-reviews.sh` failed and the three org-scoped ones returned nothing. The script
soft-fails each query to `[]` and exits 0, so it emitted a valid, empty array.

Dispatch discarded the only evidence. `exec_feed_command` uses `.output()`, which
captures stderr, and logs it **only** on a non-zero exit. A command that exits 0 while
writing four errors to stderr produces no log line at all. The status bar said
`Feed for 'PR Reviews': 0 task(s) synced`, `app.log` said nothing, and the four gh
errors were invisible.

## Scope

Evidence only. Two adjacent problems are deliberately **not** addressed here:

- **An empty emission is destructive.** `delete_stale_subtree` runs with
  `all_external_ids = []`, and `external_id NOT IN (SELECT value FROM json_each('[]'))`
  matches every feed task in the subtree (`src/db/queries/tasks.rs:558`). A silent-failure
  run does not merely fail to fill the sub-epics — it deletes what was already in them,
  via a raw `DELETE` with no worktree/tmux cleanup, orphaning any in-flight review
  agent's worktree on disk. Guarding against a suspicious empty emission is a separate
  behaviour change.
- **The two paths parse differently.** The manual path uses `serde_json::from_slice`
  while the tick uses `parse::parse_feed_items` (which warns on unknown tags). A real
  drift, but a behaviour change.

Both get follow-up tasks.

`feeds.allium:200` already states the script contract: FeedItem JSON on stdout,
"exit 0 on success, and emit any errors on stderr with non-zero exit."
`fetch-reviews.sh` violates it by swallowing per-query failure into `[]` and exiting 0.
Dispatch behaved correctly per spec; the script lied to it. This design makes dispatch
robust to that class of lie rather than trusting the contract.

## Design

### 1. One shared exec (`src/feed/exec.rs`)

```rust
pub(crate) struct FeedOutput {
    pub(crate) stdout: Vec<u8>,
    /// stderr the command wrote while still exiting 0 — trimmed, truncated.
    pub(crate) stderr: String,
}

pub(crate) async fn exec_feed_command(cmd, epic_id, epic_title)
    -> Result<FeedOutput, String>
```

- Spawn error or non-zero exit: logs WARN exactly as today, **and** returns
  `Err(message)` so the caller can act on it too.
- Exit 0 with non-empty stderr: logs
  `WARN … "command wrote to stderr on success"` with `epic_id`, `epic_title`, `stderr`,
  then returns `Ok`. The sync proceeds — this is a diagnostic, not a failure.
- Exit 0 with empty stderr: unchanged.

stderr is trimmed and truncated to 2000 characters. Today's four gh errors are ~1.3 KB;
an unbounded script could otherwise write megabytes into `app.log`.

Keeping the failure WARN inside `exec_feed_command` means `FeedJob::run`'s arm stays a
bare `Err(_) => return`, existing log output is unchanged, and the manual path gets both
a log line and a status-bar message.

### 2. Both callers use it

`exec_trigger_epic_feed` (`src/runtime/epics.rs:263`) currently carries its own private
copy of the spawn/exit/parse block, which is why a fix in `exec_feed_command` alone
would cover only the auto-poll path.

- `FeedJob::run` (`src/feed/mod.rs:84`): `Ok(out) => out.stdout`, `Err(_) => return`.
- `exec_trigger_epic_feed`: drops the private block, calls the shared function, maps
  `Err(e) => fail(e)` (unchanged `FeedMessage::Failed` behaviour).

One place owns the rule, so the two paths cannot drift on it again.

### 3. The status-bar hint

`FeedMessage::Refreshed` gains `wrote_stderr: bool`. `handle_feed_refreshed` appends
`" — command wrote to stderr (see app.log)"` when — and only when —
`count == 0 && wrote_stderr`:

```
Feed for 'PR Reviews': 0 task(s) synced — command wrote to stderr (see app.log)
```

Gating on `count == 0` keeps a script with chatty-but-harmless stderr from nagging on
every manual refresh. Auto-poll stays silent in the TUI, preserving
`feeds.allium`'s stance that only manual triggers speak in the status bar.

### 4. Spec

`feeds.allium` gains a rule beside `FeedCommandFailure`: a zero-exit feed command with
non-empty stderr logs a warning and syncs normally. `ManualFeedTrigger`'s status-bar
list gains the zero-items-plus-stderr line.

### 5. Tests (written first)

| Behaviour | Where |
|---|---|
| `exec_feed_command` captures stderr from a zero-exit command | inline in `src/feed/exec.rs` |
| stderr is truncated at the cap | inline in `src/feed/exec.rs` |
| non-zero exit returns `Err` carrying the stderr | inline in `src/feed/exec.rs` |
| hint appears at `count == 0 && wrote_stderr`, and not when `count > 0` or `!wrote_stderr` | `src/tui/tests/epics.rs`, beside the existing `Refreshed` tests |
| `exec_trigger_epic_feed` emits `wrote_stderr: true` for a command writing stderr and printing `[]` | `src/runtime/tests.rs`, beside `exec_trigger_epic_feed_zero_items` |

The log line itself is not asserted (no tracing capture in this suite); the returned
`FeedOutput.stderr` is what feeds both the log and the hint, so asserting it covers both.

## Notes

The comment at `scripts/fetch-reviews.sh:114` — "let gh's stderr flow to the feed log" —
is false today. This change makes it true, so the script needs no edit.
