# Token Budget Indicator Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show the Claude subscription 5-hour and 7-day budget windows continuously in the dispatch board's top indicator row, so the user never opens the main session just to run `/usage`.

**Architecture:** Claude Code's statusLine hook payload is the only programmatic source for the rate-limit windows. A new `dispatch statusline` subcommand acts as a transparent decorator: it records `rate_limits` to `<data_dir>/rate-limits.json` and then execs the user's previous statusLine command, printing its output verbatim. It is injected into every dispatch-spawned Claude session via `--settings`, which outranks the user's own settings. The TUI polls that snapshot file on a tick multiple and renders a badge.

**Tech Stack:** Rust 2021, ratatui, clap, serde_json, tempfile, insta (snapshots), tokio.

**Design doc:** `docs/superpowers/specs/2026-07-31-token-budget-indicator-design.md` — read it before starting. It records *why* each decision was made and which alternatives were refuted.

## Global Constraints

- **Naming: `Budget*`, never `Usage*`.** The repo has an unrelated `usage_events`/`query_usage` subsystem counting keypresses and MCP tool calls. `docs/specs/mcp-task-tools.allium:423` states those are "NOT token counts". Never name anything in this feature `Usage*`.
- **Spec first, then tests, then code.** Task 1 updates the Allium specs before any implementation.
- **TDD, strictly.** Write the failing test, run it, watch it fail for the right reason, then implement.
- **Inline test modules need `#![allow]`.** Every `mod tests` / `mod property_tests` must start with `#[allow(clippy::unwrap_used, clippy::expect_used)]` — the workspace `-D warnings` policy rejects bare `unwrap()`/`expect()` otherwise. Canonical pattern: `src/db/tests/mod.rs`.
- **No bare `unwrap()`/`expect()` in production code.** Clippy-warned outside tests, and the pre-push hook applies `-D warnings`. A plain `cargo build` will NOT catch this.
- **No `std::fs` inside async handlers** (`docs/conventions.md:324`). Use `tokio::task::spawn_blocking`.
- **No `tokio::time::sleep` in tests, no `std::thread::sleep` in test files** (`docs/conventions.md:347`, enforced by `./scripts/check-no-test-sleep.sh`). Inject thresholds instead.
- **Snapshot backend is 120×40.** Do not change it (`src/tui/tests/snapshots/`). Always `rm src/tui/tests/snapshots/*.snap.new` and `rm src/dispatch/snapshots/*.snap.new` after accepting.
- **Verification command** (run before declaring any task complete): `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`
- **Mutation boundary:** task/epic mutations go through `TaskServiceApi`/`EpicServiceApi`. This feature performs no task/epic mutations at all — it only reads a file. Do not add DB access anywhere in it.
- **Fixed literal path:** the injected settings file is `~/.claude/dispatch-statusline.json`. It must NOT go in the plugin dir — `remove_stale_files` (`src/setup/plugins.rs:107-111`) deletes any non-embedded file there.

## File Structure

| File | Responsibility |
|---|---|
| `docs/specs/dispatch.allium` (modify) | `TokenBudgetIndicator` surface — the specced behaviour |
| `docs/specs/core.allium` (modify) | `budget_poll_interval`, `budget_stale_after` config |
| `src/models/budget.rs` (create) | `BudgetSnapshot`, `BudgetWindow` — pure types, parse from payload, serde |
| `src/models/mod.rs` (modify) | `pub mod budget;` |
| `src/cli/statusline.rs` (create) | Pure decorator core: parse stdin → snapshot, atomic write, chain exec |
| `src/cli/mod.rs` (modify) | `pub mod statusline;` |
| `src/main.rs` (modify) | `Commands::Statusline` variant + dispatch arm |
| `src/setup/statusline.rs` (create) | Generate/write `dispatch-statusline.json`, discover chain target, recursion guard |
| `src/setup/mod.rs` (modify) | Call the writer from `run_setup_in`; add `statusline_path` to `SetupPaths` |
| `src/dispatch/prompts.rs` (modify) | `DISPATCH_PLUGIN_DIR` gains `--settings` |
| `src/tui/mod.rs` (modify) | `BUDGET_POLL_TICKS`, `BUDGET_STALE_AFTER`, `App.budget`, `App.ticks_since_budget_poll` |
| `src/tui/commands/budget.rs` (create) | `BudgetCommand::Refresh` |
| `src/tui/messages/budget.rs` (create) | `BudgetMessage::Updated` + `route` |
| `src/tui/update/budget.rs` (create) | `tick_budget_poll` caller target + `handle_budget_updated` |
| `src/tui/update/agent.rs` (modify) | `tick_budget_poll` sub-step, wired into `handle_tick` |
| `src/runtime/budget.rs` (create) | `exec_refresh_budget` — `spawn_blocking` file read |
| `src/runtime/commands.rs` (modify) | Dispatch `Command::Budget` |
| `src/tui/ui/budget.rs` (create) | `budget_spans()` — formatting, thresholds, degradation order |
| `src/tui/ui/shared.rs` (modify) | `render_top_indicators` prepends budget spans |
| Deletions | `plugin/hooks/scripts/task-usage-hook`, its `hooks.json` entry, `src/setup/hooks.rs:22-28,36-38,130`, `src/setup/plugins.rs:478` entry, `docs/reference.md:201`, `docs/specs/epics.allium:271` |

---

### Task 1: Spec the surface and config

**Files:**
- Modify: `docs/specs/dispatch.allium` (add surface after `MainSessionIndicator`, which ends near `:936`)
- Modify: `docs/specs/core.allium:530` (add two config entries after `main_session_poll_interval`)

**Interfaces:**
- Consumes: nothing.
- Produces: the `@guarantee` names that later tasks' tests cite in comments. Guarantee names: `WindowsRenderedWithPercentAndReset`, `ColourByThreshold`, `HiddenWhenNoSnapshot`, `PerWindowOmission`, `DimmedWhenStale`, `ResetInPastRendersNow`, `PercentageClamped`, `DegradesWhenRowTooNarrow`, `DerivedLiveNeverPersisted`, `RefreshedPeriodicallyNoRedrawWhenUnchanged`, `PassiveNotNavigable`.

- [ ] **Step 1: Read the template surface**

Read `docs/specs/dispatch.allium` lines 871-936 (`surface MainSessionIndicator`). Match its style exactly: `facing user:`, `let` bindings, `exposes:`, one `@guarantee` per behaviour, a closing `@guidance`.

- [ ] **Step 2: Add the config entries**

In `docs/specs/core.allium`, immediately after the `main_session_poll_interval` line (`:530`):

```
    budget_poll_interval: Duration = 10.seconds
    budget_stale_after: Duration = 10.minutes
```

- [ ] **Step 3: Add the surface**

Append after `MainSessionIndicator` in `docs/specs/dispatch.allium`:

```
surface TokenBudgetIndicator {
    facing user: User
    -- prose: passive readout of the Claude subscription rate-limit windows in
    -- the top indicator row. Sourced from the statusLine hook payload of every
    -- dispatch-spawned session, recorded to a snapshot file, polled by the TUI.
    -- NOT related to usage_events (see mcp-task-tools.allium: QueryUsageViaMcp) —
    -- those are feature-usage counters, these are subscription budget windows.
    let five_hour   = budget_window(FiveHour)
    let seven_day   = budget_window(SevenDay)
    let stale       = snapshot_age() > config.budget_stale_after
    let visible     = five_hour.present or seven_day.present
    exposes: five_hour  seven_day  stale  visible
    @guarantee WindowsRenderedWithPercentAndReset
        -- each present window renders as "<label> <pct>% ·<countdown>"
    @guarantee ColourByThreshold
        -- per window, independently: green below 50, yellow 50..80, red above 80
    @guarantee HiddenWhenNoSnapshot
        -- no snapshot, unreadable snapshot, or neither window present => nothing
        -- is rendered. Never imply 0% used: API-key and cloud-provider auth
        -- never emit rate_limits at all.
    @guarantee PerWindowOmission
        -- one window present and the other absent renders only the present one
    @guarantee DimmedWhenStale
        -- snapshot older than config.budget_stale_after dims the whole indicator
        -- and appends an age suffix
    @guarantee ResetInPastRendersNow
        -- a resets_at at or before now renders "·now", never a negative countdown
    @guarantee PercentageClamped
        -- used_percentage is clamped to 0..100 for both colour and display
    @guarantee DegradesWhenRowTooNarrow
        -- when the row's spans would exceed the width, drop in order: countdown
        -- suffixes, then the seven_day window, then the indicator entirely.
        -- Pre-existing badges in that row are never dropped to make room.
    @guarantee DerivedLiveNeverPersisted
        -- read from the snapshot file off the event loop; never stored in the
        -- database and never part of Board state
    @guarantee RefreshedPeriodicallyNoRedrawWhenUnchanged
        -- refreshed every config.budget_poll_interval, a fixed multiple of
        -- config.tick_interval; an unchanged refresh does not force a redraw
    @guarantee PassiveNotNavigable
        -- display-only; not selectable, focusable, or a navigation target
    @guidance -- rendered by render_top_indicators, prepended so it sits left of
              -- the bell; the status bar was rejected as a site because its
              -- hint text already overflows the terminal width and is clipped
}
```

- [ ] **Step 4: Validate the spec**

Run: `allium check docs/specs/dispatch.allium docs/specs/core.allium`
Expected: no errors. If `allium` is unavailable, skip — do not block on it.

- [ ] **Step 5: Verify doc paths still resolve**

Run: `./scripts/check-doc-paths.sh`
Expected: `check-doc-paths: all references resolve`

- [ ] **Step 6: Commit**

```bash
git add docs/specs/dispatch.allium docs/specs/core.allium
git commit -m "spec: add TokenBudgetIndicator surface and budget config"
```

---

### Task 2: `BudgetSnapshot` model

**Files:**
- Create: `src/models/budget.rs`
- Modify: `src/models/mod.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub struct BudgetWindow { pub used_percentage: f64, pub resets_at: i64 }`
  - `pub struct BudgetSnapshot { pub five_hour: Option<BudgetWindow>, pub seven_day: Option<BudgetWindow>, pub captured_at: i64 }`
  - `impl BudgetSnapshot { pub fn from_status_payload(payload: &serde_json::Value, captured_at: i64) -> Option<Self> }` — `None` when neither window is present.
  - `impl BudgetWindow { pub fn clamped_percentage(&self) -> f64 }` — clamps to `0.0..=100.0`.
  - Both derive `Serialize`/`Deserialize`/`Debug`/`Clone`/`PartialEq`.

- [ ] **Step 1: Write the failing tests**

Create `src/models/budget.rs` with only the test module and the `use super::*;`:

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_both_windows() {
        let payload = json!({
            "rate_limits": {
                "five_hour": { "used_percentage": 23.5, "resets_at": 1738425600_i64 },
                "seven_day": { "used_percentage": 41.2, "resets_at": 1738857600_i64 }
            }
        });
        let snap = BudgetSnapshot::from_status_payload(&payload, 1738421000).unwrap();
        assert_eq!(snap.five_hour.unwrap().used_percentage, 23.5);
        assert_eq!(snap.seven_day.unwrap().resets_at, 1738857600);
        assert_eq!(snap.captured_at, 1738421000);
    }

    #[test]
    fn parses_five_hour_only() {
        let payload = json!({
            "rate_limits": {
                "five_hour": { "used_percentage": 10.0, "resets_at": 1_i64 }
            }
        });
        let snap = BudgetSnapshot::from_status_payload(&payload, 0).unwrap();
        assert!(snap.five_hour.is_some());
        assert!(snap.seven_day.is_none());
    }

    #[test]
    fn absent_rate_limits_is_none() {
        // API-key and cloud-provider auth never emit rate_limits at all.
        let payload = json!({ "model": { "display_name": "Opus" } });
        assert!(BudgetSnapshot::from_status_payload(&payload, 0).is_none());
    }

    #[test]
    fn empty_rate_limits_is_none() {
        let payload = json!({ "rate_limits": {} });
        assert!(BudgetSnapshot::from_status_payload(&payload, 0).is_none());
    }

    #[test]
    fn window_missing_fields_is_skipped_not_defaulted() {
        // A window without used_percentage must not become 0% — that would
        // read as "plenty left" when we simply do not know.
        let payload = json!({
            "rate_limits": { "five_hour": { "resets_at": 5_i64 } }
        });
        assert!(BudgetSnapshot::from_status_payload(&payload, 0).is_none());
    }

    #[test]
    fn clamps_percentage_out_of_range() {
        let high = BudgetWindow { used_percentage: 137.0, resets_at: 0 };
        let low = BudgetWindow { used_percentage: -4.0, resets_at: 0 };
        assert_eq!(high.clamped_percentage(), 100.0);
        assert_eq!(low.clamped_percentage(), 0.0);
    }

    #[test]
    fn round_trips_through_json() {
        let snap = BudgetSnapshot {
            five_hour: Some(BudgetWindow { used_percentage: 1.5, resets_at: 2 }),
            seven_day: None,
            captured_at: 3,
        };
        let text = serde_json::to_string(&snap).unwrap();
        let back: BudgetSnapshot = serde_json::from_str(&text).unwrap();
        assert_eq!(snap, back);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Add `pub mod budget;` to `src/models/mod.rs` first, then run:
`cargo test models::budget`
Expected: FAIL — `cannot find struct BudgetSnapshot`.

- [ ] **Step 3: Implement**

Prepend to `src/models/budget.rs`:

```rust
//! Claude subscription rate-limit windows — the data behind the top-row budget
//! indicator (see docs/specs/dispatch.allium: TokenBudgetIndicator).
//!
//! Deliberately unrelated to `super::usage`: that module counts keybindings and
//! MCP tool calls. These are subscription budget windows.

use serde::{Deserialize, Serialize};

/// One rolling rate-limit window as reported by the statusLine hook payload.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BudgetWindow {
    pub used_percentage: f64,
    /// Unix epoch seconds at which this window resets.
    pub resets_at: i64,
}

impl BudgetWindow {
    /// Percentage constrained to 0..=100. The upstream field is documented as
    /// 0-100 but is not validated here, and a nonsense value must not produce
    /// nonsense colour or text.
    pub fn clamped_percentage(&self) -> f64 {
        self.used_percentage.clamp(0.0, 100.0)
    }

    fn from_json(value: &serde_json::Value) -> Option<Self> {
        Some(Self {
            used_percentage: value.get("used_percentage")?.as_f64()?,
            resets_at: value.get("resets_at")?.as_i64()?,
        })
    }
}

/// Latest-wins snapshot of the account-global budget windows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BudgetSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub five_hour: Option<BudgetWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seven_day: Option<BudgetWindow>,
    /// Unix epoch seconds at which this snapshot was captured.
    pub captured_at: i64,
}

impl BudgetSnapshot {
    /// Extract the budget windows from a statusLine hook payload.
    ///
    /// Returns `None` when `rate_limits` is absent or carries no usable window —
    /// the normal steady state for API-key and cloud-provider auth, and for a
    /// session that has not yet had an API response. A partially-specified
    /// window is dropped rather than defaulted, so an unknown percentage never
    /// renders as 0%.
    pub fn from_status_payload(payload: &serde_json::Value, captured_at: i64) -> Option<Self> {
        let limits = payload.get("rate_limits")?;
        let five_hour = limits.get("five_hour").and_then(BudgetWindow::from_json);
        let seven_day = limits.get("seven_day").and_then(BudgetWindow::from_json);
        if five_hour.is_none() && seven_day.is_none() {
            return None;
        }
        Some(Self {
            five_hour,
            seven_day,
            captured_at,
        })
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test models::budget`
Expected: 7 passed.

- [ ] **Step 5: Commit**

```bash
git add src/models/budget.rs src/models/mod.rs
git commit -m "feat(models): add BudgetSnapshot parsed from statusLine payload"
```

---

### Task 3: `dispatch statusline` decorator subcommand

**Files:**
- Create: `src/cli/statusline.rs`
- Modify: `src/cli/mod.rs`, `src/main.rs`

**Interfaces:**
- Consumes: `BudgetSnapshot::from_status_payload` (Task 2).
- Produces:
  - `pub fn record_snapshot(stdin: &str, snapshot_path: &Path, now: i64) -> bool` — writes atomically, returns whether it wrote.
  - `pub fn run(stdin: &str, snapshot_path: &Path, chain: Option<&str>, now: i64) -> i32` — full decorator; always returns 0.

Read `src/cli/caller_headers.rs` first — it is the model for this file: a pure core, no database, no async, exit 0 always.

- [ ] **Step 1: Write the failing tests**

Create `src/cli/statusline.rs` containing only:

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::models::budget::BudgetSnapshot;

    const PAYLOAD: &str = r#"{"rate_limits":{"five_hour":{"used_percentage":23.5,"resets_at":100},"seven_day":{"used_percentage":41.0,"resets_at":200}}}"#;

    fn read_snapshot(path: &Path) -> BudgetSnapshot {
        let text = std::fs::read_to_string(path).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    #[test]
    fn writes_snapshot_from_payload() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rate-limits.json");
        assert!(record_snapshot(PAYLOAD, &path, 42));
        let snap = read_snapshot(&path);
        assert_eq!(snap.five_hour.unwrap().used_percentage, 23.5);
        assert_eq!(snap.captured_at, 42);
    }

    #[test]
    fn creates_missing_parent_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested").join("deeper").join("rate-limits.json");
        assert!(record_snapshot(PAYLOAD, &path, 1));
        assert!(path.exists());
    }

    #[test]
    fn no_rate_limits_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rate-limits.json");
        assert!(!record_snapshot(r#"{"model":{"display_name":"Opus"}}"#, &path, 1));
        assert!(!path.exists());
    }

    #[test]
    fn malformed_stdin_writes_nothing_and_does_not_panic() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rate-limits.json");
        assert!(!record_snapshot("not json at all {{{", &path, 1));
        assert!(!path.exists());
    }

    #[test]
    fn empty_stdin_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rate-limits.json");
        assert!(!record_snapshot("", &path, 1));
        assert!(!path.exists());
    }

    #[test]
    fn overwrites_previous_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rate-limits.json");
        record_snapshot(PAYLOAD, &path, 1);
        let newer = r#"{"rate_limits":{"five_hour":{"used_percentage":99.0,"resets_at":100}}}"#;
        record_snapshot(newer, &path, 2);
        let snap = read_snapshot(&path);
        assert_eq!(snap.five_hour.unwrap().used_percentage, 99.0);
        assert_eq!(snap.captured_at, 2);
    }

    #[test]
    fn leaves_no_temp_files_behind() {
        // A fixed temp name would let concurrent writers publish each other's
        // partial bytes; a unique temp file must also not accumulate.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rate-limits.json");
        for i in 0..5 {
            record_snapshot(PAYLOAD, &path, i);
        }
        let entries: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(entries.len(), 1, "expected only the snapshot, got {entries:?}");
    }

    #[test]
    fn run_returns_zero_without_chain() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rate-limits.json");
        assert_eq!(run(PAYLOAD, &path, None, 1), 0);
    }

    #[test]
    fn run_returns_zero_when_snapshot_path_unwritable() {
        // /proc is not writable; the decorator must still succeed.
        let path = Path::new("/proc/definitely/not/writable/rate-limits.json");
        assert_eq!(run(PAYLOAD, path, None, 1), 0);
    }

    #[test]
    fn run_returns_zero_when_chained_command_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rate-limits.json");
        assert_eq!(run(PAYLOAD, &path, Some("exit 3"), 1), 0);
    }

    #[test]
    fn run_returns_zero_when_chained_command_does_not_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rate-limits.json");
        assert_eq!(run(PAYLOAD, &path, Some("definitely-not-a-real-binary-xyz"), 1), 0);
    }

    #[test]
    fn chained_command_receives_stdin_and_its_stdout_is_returned() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rate-limits.json");
        let out = run_capturing(PAYLOAD, &path, Some("cat"), 1);
        assert_eq!(out, PAYLOAD, "chained command must receive the payload verbatim");
    }

    #[test]
    fn snapshot_is_written_even_when_chain_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rate-limits.json");
        run(PAYLOAD, &path, Some("exit 1"), 7);
        assert_eq!(read_snapshot(&path).captured_at, 7);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Add `pub mod statusline;` to `src/cli/mod.rs`, then run:
`cargo test cli::statusline`
Expected: FAIL — `cannot find function record_snapshot`.

- [ ] **Step 3: Implement**

Prepend to `src/cli/statusline.rs`:

```rust
//! The `dispatch statusline` decorator (see docs/specs/dispatch.allium:
//! TokenBudgetIndicator).
//!
//! Wired as the `statusLine` command of every dispatch-spawned Claude session.
//! Records the payload's `rate_limits` to a snapshot file, then runs the user's
//! previous statusLine command and prints its output verbatim.
//!
//! Two hard constraints:
//!
//! 1. **Never fail.** This runs on Claude Code's 300 ms statusLine debounce. Any
//!    error must still exit 0, or the user's status line breaks.
//! 2. **Never open the database.** At several invocations per second per session,
//!    across every agent, database work here would be pure waste. This module has
//!    no `Database` import and must keep it that way.

use crate::models::budget::BudgetSnapshot;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Parse the payload and atomically publish a snapshot. Returns whether a
/// snapshot was written. Never panics; every failure is a silent `false`.
pub fn record_snapshot(stdin: &str, snapshot_path: &Path, now: i64) -> bool {
    let Ok(payload) = serde_json::from_str::<serde_json::Value>(stdin) else {
        return false;
    };
    let Some(snapshot) = BudgetSnapshot::from_status_payload(&payload, now) else {
        return false;
    };
    let Ok(text) = serde_json::to_string(&snapshot) else {
        return false;
    };
    write_atomically(snapshot_path, &text)
}

/// Publish `text` at `path` via a **uniquely named** temp file in the same
/// directory, then rename.
///
/// The unique name is load-bearing: every Claude session writes this same path
/// concurrently. With a fixed temp name, writer A could rename bytes that writer
/// B had truncated and partially written, publishing a torn value attributed to
/// the wrong writer. With a unique temp file, each writer only ever renames its
/// own complete bytes, so "last rename wins" is true — and since all writers
/// report the same account-global value, that is the correct outcome.
fn write_atomically(path: &Path, text: &str) -> bool {
    let Some(dir) = path.parent() else {
        return false;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let Ok(mut file) = tempfile::NamedTempFile::new_in(dir) else {
        return false;
    };
    if file.write_all(text.as_bytes()).is_err() {
        return false;
    }
    if file.flush().is_err() {
        return false;
    }
    file.persist(path).is_ok()
}

/// Run the chained command with `stdin` on its stdin, returning its stdout.
/// Any failure yields an empty string — a blank status line, never a broken one.
fn run_chain(chain: &str, stdin: &str) -> String {
    let spawned = Command::new("sh")
        .arg("-c")
        .arg(chain)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn();
    let Ok(mut child) = spawned else {
        return String::new();
    };
    if let Some(mut pipe) = child.stdin.take() {
        let _ = pipe.write_all(stdin.as_bytes());
        // Drop closes the pipe so the child sees EOF and can exit.
    }
    match child.wait_with_output() {
        Ok(out) => String::from_utf8_lossy(&out.stdout).into_owned(),
        Err(_) => String::new(),
    }
}

/// Decorator core: record, then chain. Always returns exit code 0.
pub fn run(stdin: &str, snapshot_path: &Path, chain: Option<&str>, now: i64) -> i32 {
    let out = run_capturing(stdin, snapshot_path, chain, now);
    print!("{out}");
    0
}

/// `run` without the side effect of printing, so tests can assert the output.
pub fn run_capturing(stdin: &str, snapshot_path: &Path, chain: Option<&str>, now: i64) -> String {
    record_snapshot(stdin, snapshot_path, now);
    match chain {
        Some(cmd) if !cmd.trim().is_empty() => run_chain(cmd, stdin),
        _ => String::new(),
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test cli::statusline`
Expected: 13 passed.

- [ ] **Step 5: Wire the subcommand into `main.rs`**

Add to the `Commands` enum in `src/main.rs` (after the `AgentTree` variant, which ends at `:116`):

```rust
    /// statusLine decorator for Claude Code: record the subscription
    /// rate-limit windows from the hook payload on stdin, then run the
    /// user's previous statusLine command and print its output verbatim.
    /// Always exits 0 — never breaks the user's status line. Opens no
    /// database (it runs several times a second per session).
    Statusline {
        /// Where to publish the snapshot JSON
        #[arg(long)]
        snapshot: String,
        /// The previous statusLine command to run and echo
        #[arg(long)]
        chain: Option<String>,
    },
```

Add the dispatch arm next to `Commands::AgentTree` (`:720`):

```rust
        Commands::Statusline { snapshot, chain } => {
            let mut stdin = String::new();
            let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut stdin);
            let now = chrono::Utc::now().timestamp();
            let code = dispatch_tui::cli::statusline::run(
                &stdin,
                std::path::Path::new(&snapshot),
                chain.as_deref(),
                now,
            );
            std::process::exit(code);
        }
```

- [ ] **Step 6: Verify it builds and behaves end to end**

Run: `cargo build`
Then a manual smoke test:

```bash
echo '{"rate_limits":{"five_hour":{"used_percentage":12.5,"resets_at":99}}}' \
  | ./target/debug/dispatch statusline --snapshot /tmp/rl-test.json --chain 'echo hello'
cat /tmp/rl-test.json
```

Expected: prints `hello`, and `/tmp/rl-test.json` contains `"used_percentage":12.5`. Then `rm /tmp/rl-test.json`.

- [ ] **Step 7: Confirm no database dependency crept in**

Run: `rtk grep -n "Database\|db::" src/cli/statusline.rs`
Expected: no matches.

- [ ] **Step 8: Commit**

```bash
git add src/cli/statusline.rs src/cli/mod.rs src/main.rs
git commit -m "feat(cli): add dispatch statusline decorator subcommand"
```

---

### Task 4: Setup writes the injected settings file

**Files:**
- Create: `src/setup/statusline.rs`
- Modify: `src/setup/mod.rs` (`SetupPaths` at `:213-231`, `run_setup_in` at `:256`)

**Interfaces:**
- Consumes: nothing from earlier tasks (it only generates a command string).
- Produces:
  - `pub(super) fn shell_quote(s: &str) -> String`
  - `pub(super) fn build_command(snapshot_path: &Path, chain: Option<&str>) -> String`
  - `pub(super) fn discover_chain(claude_dir: &Path) -> Option<String>`
  - `pub(super) fn write_settings_file(path: &Path, snapshot_path: &Path, chain: Option<&str>) -> Result<bool>` — returns whether content changed.

- [ ] **Step 1: Write the failing tests**

Create `src/setup/statusline.rs` with only:

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn quotes_plain_path() {
        assert_eq!(shell_quote("/home/a/b.json"), "'/home/a/b.json'");
    }

    #[test]
    fn quotes_path_with_spaces() {
        assert_eq!(shell_quote("/home/my dir/b.json"), "'/home/my dir/b.json'");
    }

    #[test]
    fn escapes_embedded_single_quote() {
        // A path containing a single quote must not terminate the quoting.
        assert_eq!(shell_quote("/home/o'brien/b"), r#"'/home/o'\''brien/b'"#);
    }

    #[test]
    fn builds_command_with_chain() {
        let cmd = build_command(Path::new("/d/rate-limits.json"), Some("claude-statusline"));
        assert_eq!(
            cmd,
            "dispatch statusline --snapshot '/d/rate-limits.json' --chain 'claude-statusline'"
        );
    }

    #[test]
    fn builds_command_without_chain() {
        let cmd = build_command(Path::new("/d/rate-limits.json"), None);
        assert_eq!(cmd, "dispatch statusline --snapshot '/d/rate-limits.json'");
    }

    #[test]
    fn discovers_existing_status_line_command() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("settings.json"),
            r#"{"statusLine":{"type":"command","command":"claude-statusline"}}"#,
        )
        .unwrap();
        assert_eq!(discover_chain(tmp.path()).as_deref(), Some("claude-statusline"));
    }

    #[test]
    fn discovers_none_when_no_settings_file() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(discover_chain(tmp.path()), None);
    }

    #[test]
    fn discovers_none_when_no_status_line_key() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("settings.json"), r#"{"permissions":{}}"#).unwrap();
        assert_eq!(discover_chain(tmp.path()), None);
    }

    #[test]
    fn discovers_none_when_settings_malformed() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("settings.json"), "{ not json").unwrap();
        assert_eq!(discover_chain(tmp.path()), None);
    }

    #[test]
    fn recursion_guard_refuses_to_chain_to_itself() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("settings.json"),
            r#"{"statusLine":{"type":"command","command":"dispatch statusline --snapshot /d/x.json"}}"#,
        )
        .unwrap();
        assert_eq!(
            discover_chain(tmp.path()),
            None,
            "must not chain to a dispatch statusline invocation"
        );
    }

    #[test]
    fn writes_valid_settings_json() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("dispatch-statusline.json");
        assert!(write_settings_file(&path, Path::new("/d/rl.json"), Some("cs")).unwrap());
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["statusLine"]["type"], "command");
        assert_eq!(
            v["statusLine"]["command"],
            "dispatch statusline --snapshot '/d/rl.json' --chain 'cs'"
        );
    }

    #[test]
    fn write_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("dispatch-statusline.json");
        assert!(write_settings_file(&path, Path::new("/d/rl.json"), Some("cs")).unwrap());
        assert!(
            !write_settings_file(&path, Path::new("/d/rl.json"), Some("cs")).unwrap(),
            "second identical write must report no change"
        );
    }

    #[test]
    fn write_reports_change_when_chain_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("dispatch-statusline.json");
        write_settings_file(&path, Path::new("/d/rl.json"), Some("old")).unwrap();
        assert!(write_settings_file(&path, Path::new("/d/rl.json"), Some("new")).unwrap());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Add `mod statusline;` to `src/setup/mod.rs`, then run:
`cargo test setup::statusline`
Expected: FAIL — `cannot find function shell_quote`.

- [ ] **Step 3: Implement**

Prepend to `src/setup/statusline.rs`:

```rust
//! Generates the dispatch-owned statusLine settings file that is injected into
//! every dispatch-spawned Claude session via `--settings`.
//!
//! The file lives at a **fixed literal path** (`~/.claude/dispatch-statusline.json`)
//! so the spawn constant in `src/dispatch/prompts.rs` stays a compile-time
//! `const` with no runtime path and no shell-quoting hazard. Runtime paths live
//! inside this file instead, where they can be quoted properly.
//!
//! Note it is NOT placed under the plugin dir: `remove_stale_files` deletes any
//! non-embedded file there. And it is NOT `~/.claude/settings.json`, which
//! `src/setup/mod.rs` deliberately never writes.

use anyhow::{Context, Result};
use serde_json::json;
use std::path::Path;

/// The fixed file name, under the resolved `~/.claude` directory.
pub(super) const SETTINGS_FILE_NAME: &str = "dispatch-statusline.json";

/// POSIX single-quoting: wrap in `'…'` and replace each embedded `'` with
/// `'\''`. The generated string is run through `sh -c`, so an unquoted path
/// containing a space would split into two arguments.
pub(super) fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Build the statusLine command string.
pub(super) fn build_command(snapshot_path: &Path, chain: Option<&str>) -> String {
    let mut cmd = format!(
        "dispatch statusline --snapshot {}",
        shell_quote(&snapshot_path.display().to_string())
    );
    if let Some(chain) = chain {
        cmd.push_str(&format!(" --chain {}", shell_quote(chain)));
    }
    cmd
}

/// Read the user's current `statusLine.command` so the decorator can chain to
/// it. Read-only — this never writes `settings.json`.
///
/// Returns `None` when there is nothing to chain, including the
/// **recursion-guard** case where the user's command is already a
/// `dispatch statusline` invocation. Chaining to ourselves would loop; the
/// honest outcome is an empty status line, with the reporter still running.
pub(super) fn discover_chain(claude_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(claude_dir.join("settings.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let command = value
        .get("statusLine")?
        .get("command")?
        .as_str()?
        .trim()
        .to_string();
    if command.is_empty() || command.contains("dispatch statusline") {
        return None;
    }
    Some(command)
}

/// Write the settings file. Returns whether the on-disk content changed, so
/// setup can report accurately and stay idempotent.
pub(super) fn write_settings_file(
    path: &Path,
    snapshot_path: &Path,
    chain: Option<&str>,
) -> Result<bool> {
    let content = serde_json::to_string_pretty(&json!({
        "statusLine": {
            "type": "command",
            "command": build_command(snapshot_path, chain),
        }
    }))
    .context("failed to serialize statusline settings")?;

    if let Ok(existing) = std::fs::read_to_string(path) {
        if existing.trim() == content.trim() {
            return Ok(false);
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(path, &content).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(true)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test setup::statusline`
Expected: 13 passed.

- [ ] **Step 5: Call it from `run_setup_in`**

Add to `SetupPaths` (`src/setup/mod.rs:213`):

```rust
    pub statusline_path: PathBuf,
```

In `SetupPaths::resolve` (`:222`), inside the `Ok(Self { … })`:

```rust
            statusline_path: claude_dir.join(statusline::SETTINGS_FILE_NAME),
```

Note `claude_dir` is moved into the struct — bind `let claude_dir = claude_dir()?;` first and build `statusline_path` from it before the struct literal, matching how `legacy_mcp_path` is already derived at `:224`.

In `run_setup_in`, next to the plugin-install reporting (`:287-289`):

```rust
    let chain = statusline::discover_chain(&paths.claude_dir);
    let snapshot_path = data_dir.join("rate-limits.json");
    match statusline::write_settings_file(&paths.statusline_path, &snapshot_path, chain.as_deref()) {
        Ok(true) => println!(
            "Status line: wrote {} (budget indicator){}",
            display_for(&paths.statusline_path),
            match &chain {
                Some(c) => format!(", chaining to `{c}`"),
                None => String::new(),
            }
        ),
        Ok(false) => println!("Status line: already configured"),
        Err(e) => eprintln!("Warning: failed to write statusline settings: {e}"),
    }
```

- [ ] **Step 6: Verify the settings.json invariant still holds**

Run: `cargo test setup::`
Expected: all pass, **including `setup_does_not_write_settings_json`**. If that test fails, you have written to the wrong file — the target is `dispatch-statusline.json`, never `settings.json`.

- [ ] **Step 7: Commit**

```bash
git add src/setup/statusline.rs src/setup/mod.rs
git commit -m "feat(setup): generate the injected statusLine settings file"
```

---

### Task 5: Inject `--settings` into spawned sessions

**Files:**
- Modify: `src/dispatch/prompts.rs:12`
- Accept: `src/dispatch/snapshots/*.snap`

**Interfaces:**
- Consumes: the fixed file name from Task 4 (as a literal — do not import, the const must stay a literal).
- Produces: nothing new.

- [ ] **Step 1: Write the failing test**

In `src/dispatch/tests.rs`, add:

```rust
#[test]
fn all_spawn_sites_inject_the_statusline_settings_file() {
    // Every dispatch-spawned session must report budget windows, so the
    // --settings overlay has to be on the agent, resume, and main-session
    // command lines alike. See docs/specs/dispatch.allium: TokenBudgetIndicator.
    assert!(
        crate::dispatch::prompts::DISPATCH_PLUGIN_DIR
            .contains("--settings ~/.claude/dispatch-statusline.json"),
        "spawn constant must inject the statusline settings overlay, got: {}",
        crate::dispatch::prompts::DISPATCH_PLUGIN_DIR
    );
}

#[test]
fn spawn_constant_contains_no_whitespace_hazard() {
    // The constant is interpolated into a shell command string sent through
    // tmux send_keys, so it must contain only fixed literal paths. A runtime
    // path here would break on any $HOME containing a space.
    for token in crate::dispatch::prompts::DISPATCH_PLUGIN_DIR.split_whitespace() {
        assert!(
            !token.contains('$'),
            "no runtime interpolation allowed in the spawn constant: {token}"
        );
    }
}
```

`DISPATCH_PLUGIN_DIR` is `pub(super)`; if the test module cannot see it, widen to `pub(crate)` rather than duplicating the literal.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib dispatch::tests::all_spawn_sites_inject`
Expected: FAIL — the constant lacks `--settings`.

- [ ] **Step 3: Implement**

Replace `src/dispatch/prompts.rs:12`:

```rust
/// Flags added to all Claude agent invocations. `--plugin-dir` so dispatched
/// agents discover the dispatch plugin's skills and commands (e.g. /wrap-up);
/// `--settings` so every session reports its subscription budget windows via
/// the `dispatch statusline` decorator (see docs/specs/dispatch.allium:
/// TokenBudgetIndicator).
///
/// Both paths are fixed literals on purpose. This string is interpolated into
/// shell command lines sent through `tmux send-keys`, so a runtime path could
/// break argument splitting on any `$HOME` containing a space; and a `const`
/// cannot hold a runtime value anyway. Runtime paths live inside the settings
/// file, written by `src/setup/statusline.rs`.
pub(super) const DISPATCH_PLUGIN_DIR: &str = "--plugin-dir ~/.claude/plugins/local/dispatch \
     --settings ~/.claude/dispatch-statusline.json";
```

Careful: a Rust line continuation `\` inside a string literal swallows the following whitespace, so write the second line with a leading space or use `concat!`. Verify the rendered value has exactly one space between the two flags.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib dispatch::tests`
Expected: the two new tests pass. Prompt snapshot tests may now FAIL — that is expected.

- [ ] **Step 5: Review and accept the prompt snapshots**

Run: `cargo insta review`
Inspect each diff: the only change should be the added `--settings` flag. Then:

```bash
INSTA_UPDATE=always cargo test dispatch::prompts_snapshots
rm -f src/dispatch/snapshots/*.snap.new
```

- [ ] **Step 6: Verify**

Run: `cargo test`
Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add src/dispatch/prompts.rs src/dispatch/tests.rs src/dispatch/snapshots/
git commit -m "feat(dispatch): inject statusline settings overlay into spawned sessions"
```

---

### Task 6: TUI plumbing — state, tick, command, message, executor

**Files:**
- Create: `src/tui/commands/budget.rs`, `src/tui/messages/budget.rs`, `src/tui/update/budget.rs`, `src/runtime/budget.rs`
- Modify: `src/tui/mod.rs`, `src/tui/commands/mod.rs`, `src/tui/messages/mod.rs`, `src/tui/update/mod.rs`, `src/tui/types.rs`, `src/tui/update/agent.rs`, `src/runtime/mod.rs`, `src/runtime/commands.rs`

**Interfaces:**
- Consumes: `BudgetSnapshot` (Task 2).
- Produces:
  - `BudgetCommand::Refresh`, wrapped as `Command::Budget`
  - `BudgetMessage::Updated(Option<BudgetSnapshot>)`, wrapped as `Message::Budget`
  - `App.budget: Option<BudgetSnapshot>`, `App.ticks_since_budget_poll: u64`
  - `App::handle_budget_updated(&mut self, Option<BudgetSnapshot>) -> Vec<Command>`
  - `crate::tui::BUDGET_POLL_TICKS: u64`, `crate::tui::BUDGET_STALE_AFTER: Duration`
  - `TuiRuntime::exec_refresh_budget(&self) -> JoinHandle<()>`

Read these four templates before writing anything: `src/tui/commands/main_session.rs`, `src/tui/messages/main_session.rs`, `src/tui/update/main_session.rs:36-48`, `src/runtime/split.rs:86-99`. Mirror them exactly.

- [ ] **Step 1: Write the failing tests**

Create `src/tui/tests/budget.rs` (and add `mod budget;` to `src/tui/tests/mod.rs`):

```rust
#[allow(clippy::unwrap_used, clippy::expect_used)]
use crate::models::budget::{BudgetSnapshot, BudgetWindow};
use crate::tui::commands::BudgetCommand;
use crate::tui::tests::helpers::test_app;
use crate::tui::types::Command;

fn snapshot(pct: f64) -> BudgetSnapshot {
    BudgetSnapshot {
        five_hour: Some(BudgetWindow { used_percentage: pct, resets_at: 0 }),
        seven_day: None,
        captured_at: 0,
    }
}

#[test]
fn tick_emits_refresh_on_the_nth_tick() {
    let mut app = test_app();
    for _ in 0..(crate::tui::BUDGET_POLL_TICKS - 1) {
        let cmds = app.handle_tick();
        assert!(
            !cmds.iter().any(|c| matches!(c, Command::Budget(BudgetCommand::Refresh))),
            "must not poll before the Nth tick"
        );
    }
    let cmds = app.handle_tick();
    assert!(
        cmds.iter().any(|c| matches!(c, Command::Budget(BudgetCommand::Refresh))),
        "must poll on the Nth tick"
    );
}

#[test]
fn changed_snapshot_marks_dirty() {
    let mut app = test_app();
    app.dirty = false;
    app.handle_budget_updated(Some(snapshot(10.0)));
    assert!(app.dirty, "a changed snapshot must force a redraw");
}

#[test]
fn unchanged_snapshot_does_not_mark_dirty() {
    // This state is invisible to the discriminant-based dirty detector in
    // handle_key, so the handler marks dirty itself — but only on change.
    let mut app = test_app();
    app.handle_budget_updated(Some(snapshot(10.0)));
    app.dirty = false;
    app.handle_budget_updated(Some(snapshot(10.0)));
    assert!(!app.dirty, "an identical refresh must not force a redraw");
}

#[test]
fn disappearing_snapshot_marks_dirty() {
    let mut app = test_app();
    app.handle_budget_updated(Some(snapshot(10.0)));
    app.dirty = false;
    app.handle_budget_updated(None);
    assert!(app.dirty);
}

#[test]
fn repeated_absent_snapshot_does_not_mark_dirty() {
    let mut app = test_app();
    app.handle_budget_updated(None);
    app.dirty = false;
    app.handle_budget_updated(None);
    assert!(!app.dirty);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test tui::tests::budget`
Expected: FAIL — `BudgetCommand` not found.

- [ ] **Step 3: Add the constants and `App` state**

In `src/tui/mod.rs`, after `MAIN_SESSION_POLL_TICKS` (`:41`):

```rust
/// Number of ticks between budget-snapshot reads. At `TICK_INTERVAL` (2s) this
/// is 10s — mirrors config.budget_poll_interval (see docs/specs/core.allium
/// config and dispatch.allium: TokenBudgetIndicator).
pub(in crate::tui) const BUDGET_POLL_TICKS: u64 = 5;

/// Age after which the budget indicator dims and shows its age. Mirrors
/// config.budget_stale_after.
pub(in crate::tui) const BUDGET_STALE_AFTER: Duration = Duration::from_secs(600);
```

After `ticks_since_main_session_poll` (`:245`):

```rust
    /// Latest budget snapshot read from `<data_dir>/rate-limits.json`. `None`
    /// when absent or unreadable — the steady state for non-subscription auth.
    /// Derived live, never persisted (dispatch.allium: TokenBudgetIndicator).
    pub(in crate::tui) budget: Option<crate::models::budget::BudgetSnapshot>,
    pub(in crate::tui) ticks_since_budget_poll: u64,
```

And in the constructor after `:450`:

```rust
            budget: None,
            ticks_since_budget_poll: 0,
```

- [ ] **Step 4: Add the command, message, handler, and executor**

`src/tui/commands/budget.rs`:

```rust
//! Budget-indicator side-effect commands.

/// Wrapped by [`crate::tui::types::Command::Budget`] for runtime dispatch.
#[derive(Debug, Clone)]
pub enum BudgetCommand {
    /// Read the budget snapshot file off the event loop and report the result
    /// via [`crate::tui::messages::BudgetMessage::Updated`]. Emitted by the tick
    /// loop every `BUDGET_POLL_TICKS`.
    Refresh,
}
```

`src/tui/messages/budget.rs`:

```rust
//! Budget-indicator messages.

use crate::models::budget::BudgetSnapshot;
use crate::tui::types::Command;
use crate::tui::App;

/// Wrapped by [`crate::tui::types::Message::Budget`] for dispatch.
#[derive(Debug, Clone)]
pub enum BudgetMessage {
    /// Result of a snapshot read. `None` when the file is absent or unreadable.
    Updated(Option<BudgetSnapshot>),
}

impl BudgetMessage {
    pub(in crate::tui) fn route(self, app: &mut App) -> Vec<Command> {
        match self {
            BudgetMessage::Updated(snapshot) => app.handle_budget_updated(snapshot),
        }
    }
}
```

`src/tui/update/budget.rs`:

```rust
//! Budget-indicator update handlers.

use crate::models::budget::BudgetSnapshot;
use crate::tui::types::Command;
use crate::tui::App;

impl App {
    /// Record the latest budget snapshot. Marks the board dirty only when the
    /// value changed, so a no-op refresh forces no redraw (see
    /// docs/specs/dispatch.allium: TokenBudgetIndicator).
    pub(in crate::tui) fn handle_budget_updated(
        &mut self,
        snapshot: Option<BudgetSnapshot>,
    ) -> Vec<Command> {
        if self.budget != snapshot {
            self.budget = snapshot;
            // Invisible to the discriminant-based dirty detector in handle_key,
            // so mark dirty directly — but only on a real change.
            self.dirty = true;
        }
        vec![]
    }
}
```

`src/runtime/budget.rs`:

```rust
//! Budget-snapshot refresh executor.

use crate::models::budget::BudgetSnapshot;
use crate::tui::types::Message;

impl super::TuiRuntime {
    /// Read the budget snapshot file off the event loop and report it via
    /// `BudgetMessage::Updated`. Drives the top-row budget indicator
    /// (docs/specs/dispatch.allium: TokenBudgetIndicator).
    ///
    /// `std::fs` is forbidden in async handlers (docs/conventions.md), hence
    /// `spawn_blocking`.
    pub(super) fn exec_refresh_budget(&self) -> tokio::task::JoinHandle<()> {
        let tx = self.msg_tx.clone();
        let path = self.budget_snapshot_path.clone();
        tokio::task::spawn_blocking(move || {
            let snapshot = std::fs::read_to_string(&path)
                .ok()
                .and_then(|text| serde_json::from_str::<BudgetSnapshot>(&text).ok());
            let _ = tx.send(Message::Budget(
                crate::tui::messages::BudgetMessage::Updated(snapshot),
            ));
        })
    }
}
```

Add `budget_snapshot_path: std::path::PathBuf` to `TuiRuntime` in `src/runtime/mod.rs`, initialised at construction as `db_path.parent().unwrap_or(Path::new(".")).join("rate-limits.json")` — use `unwrap_or`, not `unwrap()`. Register `mod budget;` there too.

Wire `Command::Budget` in `src/runtime/commands.rs` beside the `MainSessionCommand::CheckLiveness` arm (`:150-159`):

```rust
        Command::Budget(BudgetCommand::Refresh) => drop(rt.exec_refresh_budget()),
```

Add the `Command::Budget` / `Message::Budget` variants in `src/tui/types.rs`, the `Message::Budget(m) => m.route(app)` routing arm alongside the other message routes, and the `pub mod budget; pub use budget::*;` lines in `src/tui/commands/mod.rs`, `src/tui/messages/mod.rs`, `src/tui/update/mod.rs`.

- [ ] **Step 5: Add the tick sub-step**

In `src/tui/update/agent.rs`, after `tick_main_session_poll` (`:382`):

```rust
    /// Poll the budget snapshot file on a fixed multiple of the tick. Drives the
    /// top-row budget indicator. See docs/specs/dispatch.allium:
    /// TokenBudgetIndicator.
    fn tick_budget_poll(&mut self) -> Vec<Command> {
        self.ticks_since_budget_poll = self.ticks_since_budget_poll.saturating_add(1);
        if self.ticks_since_budget_poll >= crate::tui::BUDGET_POLL_TICKS {
            self.ticks_since_budget_poll = 0;
            return vec![Command::Budget(
                crate::tui::commands::BudgetCommand::Refresh,
            )];
        }
        vec![]
    }
```

And in `handle_tick` (`:161`), after the main-session line:

```rust
        cmds.extend(self.tick_budget_poll());
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test tui::tests::budget`
Expected: 5 passed.

- [ ] **Step 7: Full verification**

Run: `cargo fmt --check && cargo test && cargo clippy --all-targets -- -D warnings`
Expected: all clean. Clippy matters here — a bare `unwrap()` in the runtime path is a hard error under the pre-push hook.

- [ ] **Step 8: Commit**

```bash
git add src/tui src/runtime src/models
git commit -m "feat(tui): poll and store the budget snapshot"
```

---

### Task 7: Render the indicator

**Files:**
- Create: `src/tui/ui/budget.rs`
- Modify: `src/tui/ui/shared.rs:153-198`, `src/tui/ui/mod.rs`
- Test: `src/tui/tests/budget.rs`, `src/tui/tests/snapshots/`

**Interfaces:**
- Consumes: `App.budget`, `BUDGET_STALE_AFTER` (Task 6); `BudgetWindow::clamped_percentage` (Task 2).
- Produces:
  - `pub(in crate::tui::ui) fn budget_spans(snapshot: Option<&BudgetSnapshot>, now: i64, stale_after: Duration, width_budget: usize) -> Vec<Span<'static>>`
  - `fn format_countdown(seconds_remaining: i64) -> String`
  - `fn window_style(pct: f64) -> Style`

`now` and `stale_after` are parameters, not reads of the wall clock — that is what makes the states testable without sleeping (`docs/conventions.md:347,372`).

- [ ] **Step 1: Write the failing tests**

Append to `src/tui/tests/budget.rs`:

```rust
use crate::tui::ui::budget::budget_spans;
use ratatui::style::Color;
use std::time::Duration;

const STALE: Duration = Duration::from_secs(600);
const WIDE: usize = 200;

fn full(five: f64, seven: f64, captured_at: i64) -> BudgetSnapshot {
    BudgetSnapshot {
        five_hour: Some(BudgetWindow { used_percentage: five, resets_at: captured_at + 8040 }),
        seven_day: Some(BudgetWindow { used_percentage: seven, resets_at: captured_at + 345_600 }),
        captured_at,
    }
}

fn text_of(spans: &[ratatui::text::Span<'static>]) -> String {
    spans.iter().map(|s| s.content.as_ref()).collect()
}

#[test]
fn renders_both_windows_with_percent_and_countdown() {
    let snap = full(23.4, 41.2, 1000);
    let text = text_of(&budget_spans(Some(&snap), 1000, STALE, WIDE));
    assert!(text.contains("5h 23%"), "got {text:?}");
    assert!(text.contains("7d 41%"), "got {text:?}");
    assert!(text.contains("2h14m"), "got {text:?}");
    assert!(text.contains("4d"), "got {text:?}");
}

#[test]
fn no_snapshot_renders_nothing() {
    assert!(budget_spans(None, 0, STALE, WIDE).is_empty());
}

#[test]
fn omits_absent_window() {
    let snap = BudgetSnapshot {
        five_hour: Some(BudgetWindow { used_percentage: 5.0, resets_at: 60 }),
        seven_day: None,
        captured_at: 0,
    };
    let text = text_of(&budget_spans(Some(&snap), 0, STALE, WIDE));
    assert!(text.contains("5h"));
    assert!(!text.contains("7d"));
}

#[test]
fn colours_by_threshold() {
    let green = budget_spans(Some(&full(10.0, 10.0, 0)), 0, STALE, WIDE);
    let yellow = budget_spans(Some(&full(65.0, 65.0, 0)), 0, STALE, WIDE);
    let red = budget_spans(Some(&full(91.0, 91.0, 0)), 0, STALE, WIDE);
    assert!(green.iter().any(|s| s.style.fg == Some(Color::Green)));
    assert!(yellow.iter().any(|s| s.style.fg == Some(Color::Yellow)));
    assert!(red.iter().any(|s| s.style.fg == Some(Color::Red)));
}

#[test]
fn reset_in_the_past_renders_now_not_a_negative_countdown() {
    let snap = BudgetSnapshot {
        five_hour: Some(BudgetWindow { used_percentage: 5.0, resets_at: 100 }),
        seven_day: None,
        captured_at: 100,
    };
    let text = text_of(&budget_spans(Some(&snap), 500, STALE, WIDE));
    assert!(text.contains("now"), "got {text:?}");
    assert!(!text.contains('-'), "must never render a negative countdown: {text:?}");
}

#[test]
fn clamps_out_of_range_percentage() {
    let snap = BudgetSnapshot {
        five_hour: Some(BudgetWindow { used_percentage: 231.0, resets_at: 0 }),
        seven_day: Some(BudgetWindow { used_percentage: -9.0, resets_at: 0 }),
        captured_at: 0,
    };
    let text = text_of(&budget_spans(Some(&snap), 0, STALE, WIDE));
    assert!(text.contains("5h 100%"), "got {text:?}");
    assert!(text.contains("7d 0%"), "got {text:?}");
}

#[test]
fn stale_snapshot_is_dimmed_and_shows_age() {
    let snap = full(23.0, 41.0, 0);
    let text = text_of(&budget_spans(Some(&snap), 1_020, STALE, WIDE));
    assert!(text.contains("17m old"), "got {text:?}");
}

#[test]
fn fresh_snapshot_shows_no_age_suffix() {
    let snap = full(23.0, 41.0, 0);
    let text = text_of(&budget_spans(Some(&snap), 60, STALE, WIDE));
    assert!(!text.contains("old"), "got {text:?}");
}

#[test]
fn degrades_by_dropping_countdowns_first() {
    let snap = full(23.0, 41.0, 0);
    let text = text_of(&budget_spans(Some(&snap), 0, STALE, 18));
    assert!(text.contains("5h 23%") && text.contains("7d 41%"), "got {text:?}");
    assert!(!text.contains('·'), "countdowns must be dropped first: {text:?}");
}

#[test]
fn degrades_by_dropping_seven_day_next() {
    let snap = full(23.0, 41.0, 0);
    let text = text_of(&budget_spans(Some(&snap), 0, STALE, 10));
    assert!(text.contains("5h 23%"), "got {text:?}");
    assert!(!text.contains("7d"), "got {text:?}");
}

#[test]
fn degrades_to_nothing_when_hopeless() {
    let snap = full(23.0, 41.0, 0);
    assert!(budget_spans(Some(&snap), 0, STALE, 3).is_empty());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test tui::tests::budget`
Expected: FAIL — `budget_spans` not found.

- [ ] **Step 3: Implement**

Create `src/tui/ui/budget.rs`:

```rust
//! Top-row budget indicator rendering (docs/specs/dispatch.allium:
//! TokenBudgetIndicator).
//!
//! `now` and `stale_after` are parameters rather than wall-clock reads so every
//! state is testable without sleeping (docs/conventions.md: no sleeping in
//! tests).

use crate::models::budget::{BudgetSnapshot, BudgetWindow};
use crate::tui::ui::theme::MUTED;
use ratatui::style::{Color, Style};
use ratatui::text::Span;
use std::time::Duration;

/// Colour for a used-percentage: comfortable, tightening, nearly gone.
fn window_style(pct: f64) -> Style {
    let colour = if pct > 80.0 {
        Color::Red
    } else if pct >= 50.0 {
        Color::Yellow
    } else {
        Color::Green
    };
    Style::default().fg(colour)
}

/// Compact countdown. Never negative: a reset already in the past reads "now".
fn format_countdown(seconds_remaining: i64) -> String {
    if seconds_remaining <= 0 {
        return "now".to_string();
    }
    let days = seconds_remaining / 86_400;
    if days > 0 {
        return format!("{days}d");
    }
    let hours = seconds_remaining / 3_600;
    let minutes = (seconds_remaining % 3_600) / 60;
    if hours > 0 {
        format!("{hours}h{minutes:02}m")
    } else {
        format!("{minutes}m")
    }
}

fn window_text(label: &str, window: &BudgetWindow, now: i64, with_countdown: bool) -> String {
    let pct = window.clamped_percentage();
    if with_countdown {
        format!(
            "{label} {pct:.0}% ·{}",
            format_countdown(window.resets_at - now)
        )
    } else {
        format!("{label} {pct:.0}%")
    }
}

/// Build the indicator's spans, degrading to fit `width_budget`.
///
/// Degradation order, per the spec: drop the countdown suffixes, then the
/// seven-day window, then the indicator entirely. Pre-existing badges in the row
/// are never sacrificed to make room for this one — it is the newest and least
/// critical occupant.
pub(in crate::tui::ui) fn budget_spans(
    snapshot: Option<&BudgetSnapshot>,
    now: i64,
    stale_after: Duration,
    width_budget: usize,
) -> Vec<Span<'static>> {
    let Some(snapshot) = snapshot else {
        return Vec::new();
    };
    if snapshot.five_hour.is_none() && snapshot.seven_day.is_none() {
        return Vec::new();
    }

    let age = now.saturating_sub(snapshot.captured_at).max(0);
    let stale = age as u64 > stale_after.as_secs();
    let age_suffix = if stale {
        format!(" ({}m old)", age / 60)
    } else {
        String::new()
    };

    // Try each degradation level in order and take the first that fits.
    for (with_countdown, with_seven_day) in [(true, true), (false, true), (false, false)] {
        let mut spans: Vec<Span<'static>> = Vec::new();
        if let Some(w) = snapshot.five_hour.as_ref() {
            let style = if stale {
                Style::default().fg(MUTED)
            } else {
                window_style(w.clamped_percentage())
            };
            spans.push(Span::styled(window_text("5h", w, now, with_countdown), style));
        }
        if with_seven_day {
            if let Some(w) = snapshot.seven_day.as_ref() {
                if !spans.is_empty() {
                    spans.push(Span::raw("  "));
                }
                let style = if stale {
                    Style::default().fg(MUTED)
                } else {
                    window_style(w.clamped_percentage())
                };
                spans.push(Span::styled(window_text("7d", w, now, with_countdown), style));
            }
        }
        if spans.is_empty() {
            return Vec::new();
        }
        if !age_suffix.is_empty() {
            spans.push(Span::styled(
                age_suffix.clone(),
                Style::default().fg(MUTED),
            ));
        }
        spans.push(Span::raw("  "));

        let width: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        if width <= width_budget {
            return spans;
        }
    }
    Vec::new()
}
```

Check the actual name and import path of the muted colour in `src/tui/ui/` before compiling — `MUTED` is used in `shared.rs:161`; import it the same way that file does.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test tui::tests::budget`
Expected: all pass. If `renders_both_windows_with_percent_and_countdown` fails on the `2h14m` expectation, check the arithmetic: 8040 s = 2 h 14 m.

- [ ] **Step 5: Wire into `render_top_indicators`**

In `src/tui/ui/shared.rs`, at the start of `render_top_indicators` (`:154`), before the existing `parts` pushes, compute the budget spans and prepend them. Existing badges take priority, so budget gets whatever width is left:

```rust
    let mut parts: Vec<Span> = Vec::new();
    // Budget indicator is prepended so it sits left of everything else, and is
    // given only the width the existing badges leave over — it is never allowed
    // to push them off-screen (dispatch.allium: DegradesWhenRowTooNarrow).
    let existing_width = 0usize; // filled in below, after the other pushes
```

The simplest correct shape: build the existing `parts` first exactly as today, then compute `let used: usize = parts.iter().map(|s| s.content.chars().count()).sum();` and finally `let budget = crate::tui::ui::budget::budget_spans(app.budget.as_ref(), chrono::Utc::now().timestamp(), crate::tui::BUDGET_STALE_AFTER, area.width as usize - used.min(area.width as usize)); parts.splice(0..0, budget);` before constructing the `Line`.

Add `pub(in crate::tui::ui) mod budget;` to `src/tui/ui/mod.rs`.

- [ ] **Step 6: Add a rendering snapshot test**

In `src/tui/tests/snapshots/`, follow the existing pattern (see `src/tui/tests/tips_and_status.rs` for how a status/indicator render test is set up). Add one snapshot with a fresh both-windows snapshot and one with a stale snapshot. Then:

```bash
cargo insta review
rm -f src/tui/tests/snapshots/*.snap.new
```

- [ ] **Step 7: Verify**

Run: `cargo fmt --check && cargo test && cargo clippy --all-targets -- -D warnings`
Expected: all clean.

- [ ] **Step 8: Commit**

```bash
git add src/tui
git commit -m "feat(tui): render the budget indicator in the top row"
```

---

### Task 8: Remove the dead `task-usage-hook`

**Files:**
- Delete: `plugin/hooks/scripts/task-usage-hook`
- Modify: `plugin/hooks/hooks.json:23-34` (remove the entry at `:29-32`), `src/setup/hooks.rs:22-28,36-38,130`, `src/setup/plugins.rs:478`, `docs/reference.md:201`, `docs/specs/epics.allium:271`

**Interfaces:**
- Consumes: nothing.
- Produces: nothing.

Background: the hook sums per-model tokens from the transcript against a stale pricing table and POSTs `report_usage` to the MCP server. That tool does not exist (`grep -rn report_usage src/` → zero) and its `task_usage` table was dropped in migration v56. It has been silently discarding its work on every agent stop.

- [ ] **Step 1: Confirm the reference set before touching anything**

Run: `rtk grep -rn "usage_hook_script\|task-usage-hook\|report_usage" src/ plugin/ docs/`
Expected: matches in exactly the files listed above (plus `docs/plans/388-remove-cost-calculations.md`, a historical record — **leave that alone**). If you find a reference not listed here, add it to this task rather than skipping it.

- [ ] **Step 2: Delete the helper and its test first, and watch the build break**

Remove `fn usage_hook_script()` (`src/setup/hooks.rs:22-28`) and `fn usage_hook_script_is_valid_bash()` (`:36-38`). Update the stale comment at `:130` that references the hook.

This ordering is deliberate: `usage_hook_script()` does
`.expect("task-usage-hook must be embedded")`. Deleting the script while that
helper still exists panics `cargo test` — this is the step the original design
missed.

- [ ] **Step 3: Remove the registration and the script**

Remove the `task-usage-hook` object from the `Stop` array in `plugin/hooks/hooks.json` (`:29-32`). If it was the only entry, remove the now-empty `Stop` block too, and confirm the file is still valid JSON:

```bash
rm plugin/hooks/scripts/task-usage-hook
python3 -c "import json;json.load(open('plugin/hooks/hooks.json'))" && echo "valid json"
```

- [ ] **Step 4: Remove the `required`-array entry**

Delete the `"hooks/scripts/task-usage-hook",` line at `src/setup/plugins.rs:478`.

There is **no per-file embedding to remove**: `src/setup/plugins.rs:15` is `include_dir!` over the whole `plugin/` tree and picks files up automatically.

- [ ] **Step 5: Fix the docs**

Remove the `| `task-usage-hook` | Reports token usage per task |` row from `docs/reference.md:201`. In `docs/specs/epics.allium:271`, drop `task_usage` from the "from other tables (e.g. …)" list, leaving the remaining examples intact.

- [ ] **Step 6: Verify**

Run: `cargo test && ./scripts/check-doc-paths.sh && ./scripts/test-check-doc-paths.sh`
Expected: all pass. Then confirm nothing dangles:

```bash
rtk grep -rn "task-usage-hook\|report_usage" src/ plugin/ docs/reference.md docs/specs/
```
Expected: no matches.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "chore: remove the dead task-usage-hook"
```

---

### Task 9: Final verification and spec alignment

- [ ] **Step 1: Run the full verification command**

Run: `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`
Expected: all pass.

- [ ] **Step 2: Run the checks the pre-push hook adds**

```bash
cargo clippy --all-targets -- -D warnings
./scripts/check-no-test-sleep.sh
./scripts/test-check-doc-paths.sh
bash ./scripts/test-fetch-reviews.sh
```
Expected: all pass. Clippy is the one most likely to bite — a bare `unwrap()` anywhere in the new production code fails here but not in `cargo build`.

- [ ] **Step 3: Confirm no stray snapshot files**

```bash
ls src/tui/tests/snapshots/*.snap.new src/dispatch/snapshots/*.snap.new 2>/dev/null
```
Expected: no such files. A stray `.snap.new` gets silently mixed into an unrelated review pass later.

- [ ] **Step 4: Check spec/code alignment**

Use the `allium:weed` skill against `docs/specs/dispatch.allium` and `docs/specs/core.allium`. Resolve any divergence it reports for `TokenBudgetIndicator`.

- [ ] **Step 5: Manual end-to-end check**

```bash
cargo run -- setup
```
Confirm it reports writing the statusline file, then inspect it:
`cat ~/.claude/dispatch-statusline.json` — the `--chain` value should be the user's real previous command. Confirm `~/.claude/settings.json` is byte-identical to before (`git`-less check: compare a copy taken beforehand).

Then dispatch any task, let its agent produce one API response, and confirm `<data_dir>/rate-limits.json` appears and the board's top row shows the two windows.

- [ ] **Step 6: Commit any remaining changes**

```bash
git add -A
git commit -m "chore: final verification for the budget indicator"
```

---

## Self-Review Notes

**Spec coverage:** every design section maps to a task — data source and decorator → Task 3; install site and chain discovery/recursion guard → Task 4; injection → Task 5; store and concurrent-writer safety → Task 3 (unique temp file, with a test that no temp files accumulate); TUI poll pattern → Task 6; render site, all states, and degradation order → Task 7; cleanup → Task 8; specs → Task 1.

**Deliberately not covered: automatic detection of chain drift or a broken payload schema.** An earlier revision put these in `dispatch doctor`; that was dropped because the doctor CLI is being retired (task #3832). Drift is repaired by re-running `dispatch setup`, which rewrites the chain target from the user's current config — that is the whole remedy. Do not add doctor checks for this feature.

**Known soft spots for the implementer to watch:**

1. **Task 5, Step 3** — the Rust line-continuation `\` inside the string literal eats following whitespace. Verify the rendered constant has exactly one space between `--plugin-dir …dispatch` and `--settings`. A missing space silently produces a bogus flag.
2. **Task 7, Step 5** — the width arithmetic for prepending is the fiddliest part of the whole plan. `render_top_indicators` has no truncation logic today, and the right-aligned `Paragraph` will silently clip rather than error. Verify with an actual narrow-width snapshot test, not by reading the code.
3. **Task 6** — `budget_snapshot_path` on `TuiRuntime` must derive from the *same* `--db`-resolved data dir that `dispatch setup` used when writing `--snapshot` into the settings file. If the TUI runs with `--db /tmp/scratch.db` but setup ran against the default, the reader and writer point at different files and the indicator silently stays hidden. This is expected behaviour for a throwaway DB, but do not mistake it for a bug during manual testing.
4. **Task 4** — `discover_chain` and `build_command` stay `pub(super)` within `setup`. Nothing outside setup needs them now that the doctor checks are gone; resist widening their visibility without a caller.
