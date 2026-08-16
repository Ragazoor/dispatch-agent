# 3842 — Deferred findings from the budget-indicator whole-branch review

**Goal:** close findings 5–9 of the #3821 whole-branch review. Six small,
independent changes, all inside the statusline/budget-indicator feature: one
production robustness fix, four test changes (three added, one deleted, one
strengthened), one startup-behaviour change, one new Allium surface, one doc nit.

Nothing here changes the rendered indicator. The only production behaviour
changes are (A) the chain wait becomes bounded and (G) the badge appears at
startup instead of ~10 s in.

Order: spec first (item 1), then test-first per item, docs last.

---

## 1. Spec the decorator's contract — `StatusLineDecorator`

**Why:** `TokenBudgetIndicator` (`docs/specs/dispatch.allium:1123`) specs the
*readout*. The decorator — an always-on process in every Claude session, the
highest-risk component of the feature — has no spec surface at all, so
`allium:weed` cannot catch a regression in it.

**Change:** add `surface StatusLineDecorator { facing operator: User }` to
`docs/specs/dispatch.allium`, immediately after `TokenBudgetIndicator`, via the
`allium:tend` skill. It specs `dispatch statusline --snapshot <path>
[--chain <cmd>]` with these guarantees:

- `AlwaysExitsZero` — every failure path (malformed stdin, absent `rate_limits`,
  unwritable snapshot dir, missing or failing chained command, chain timeout)
  still exits 0 and prints at most an empty status line. Claude Code runs this on
  a 300 ms debounce; a non-zero exit breaks the user's status line.
- `NeverOpensTheDatabase` — several invocations per second per session, across
  every agent; database work here would be pure waste. No `Database` import, and
  running the subcommand creates no database file.
- `AtomicUniquePublish` — the snapshot is published by writing a *uniquely named*
  temp file in the destination directory and renaming it. Every session writes
  the same path concurrently; a fixed temp name would let one writer rename
  another's partial bytes. Concurrent writers therefore never publish foreign or
  torn bytes, no temp file is left behind, and last-rename-wins is correct
  because all writers report the same account-global value.
- `SnapshotIndependentOfChain` — the snapshot is recorded before the chain runs
  and is unaffected by the chain's outcome.
- `ChainOutputVerbatim` — with `--chain`, the payload is passed on the child's
  stdin and the child's stdout is reproduced verbatim, with no deadlock however
  large the payload (stdin is written from a separate thread while stdout is
  drained).
- `ChainBounded` — the chain is bounded by one wall-clock budget covering both
  its output *and* its exit. A chain that never produces output, and one that
  closes stdout but never exits, are both abandoned (killed and reaped) at the
  deadline, yielding an empty status line rather than a hung decorator.
- `NoChainToItself` — when the user's own `statusLine.command` is already a
  `dispatch statusline` invocation, nothing is chained (recursion guard); the
  reporter still runs and the status line is empty.

`@guidance` names `src/cli/statusline.rs` (decorator) and
`src/setup/statusline.rs` (the generated settings file the chain is discovered
and quoted in).

**Verify:** `allium check docs/specs/dispatch.allium`, then `allium:weed` over
the new surface against `src/cli/statusline.rs` / `src/setup/statusline.rs`.
`./scripts/check-doc-paths.sh` covers the `src/…` paths cited.

---

## 2. Bound `child.wait()` on the success path — `src/cli/statusline.rs`

**Why:** `CHAIN_TIMEOUT` bounds `rx.recv_timeout` — i.e. the child *closing
stdout*. Once stdout closes, `child.wait()` (line 129) has no bound. A chain that
closes stdout and keeps running (`exec 1>&-; …`, or a wrapper whose last stage
exits while a sibling holds the process group) blocks the decorator forever, at
several invocations per second in every session.

**Test first** (`mod tests` in `src/cli/statusline.rs`):

```rust
#[test]
fn chain_that_closes_stdout_but_keeps_running_does_not_hang() {
    // `exec 1>&-` closes stdout immediately, so the read thread finishes at
    // once and the old code fell through to an unbounded child.wait().
    let start = std::time::Instant::now();
    let out = run_chain("exec 1>&- ; sleep 30", PAYLOAD, Duration::from_millis(100));
    assert_eq!(out, "", "no output was produced before stdout closed");
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "the wait for the child to exit must be bounded, took {:?}",
        start.elapsed()
    );
}
```

Generous bound (5 s vs. the 30 s the bug costs) so it is not CI-timing-fragile;
no sleep in the test itself.

**Implementation:** turn `timeout` into a single deadline covering both phases.

```rust
/// Poll step for the bounded post-output wait. Short enough that the common
/// case (the child has already exited) costs one extra syscall, not a sleep.
const WAIT_POLL_STEP: Duration = Duration::from_millis(5);

fn run_chain(chain: &str, stdin: &str, timeout: Duration) -> String {
    // … unchanged spawn / stdin thread / stdout thread …
    let deadline = Instant::now() + timeout;
    match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(buf) => {
            reap_before(&mut child, deadline);
            String::from_utf8_lossy(&buf).into_owned()
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            String::new()
        }
    }
}

/// Wait for `child` to exit, but never past `deadline`: a chain that closes
/// stdout and keeps running must not hold the decorator open. On expiry the
/// child is killed and reaped so it does not linger as an orphan/zombie.
fn reap_before(child: &mut std::process::Child, deadline: Instant) {
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return,
            Ok(None) => {}
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return;
        }
        std::thread::sleep(WAIT_POLL_STEP);
    }
}
```

`std::thread::sleep` in production code is permitted (`check-no-test-sleep.sh`
only rejects it in test files); this call sits in the non-test part of the file.
Update the `CHAIN_TIMEOUT` and `run_chain` doc comments to say the budget covers
output *and* exit, and cite the new spec guarantee `ChainBounded`.

---

## 3. Concurrent-writer test — the unique-temp-name property

**Why:** the design's testing table promised "two concurrent writers never
publish foreign bytes". `leaves_no_temp_files_behind` writes *sequentially* five
times, so the property that makes the unique temp name load-bearing is untested.

**Test** (`mod tests` in `src/cli/statusline.rs`):

```rust
#[test]
fn concurrent_writers_never_publish_foreign_or_torn_bytes() {
    // Two writers with distinguishable payloads hammer the same path while a
    // reader reads it. Every successful read must parse and must equal one of
    // the two writers' complete values — never a blend, never a truncation.
    const ROUNDS: usize = 200;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("rate-limits.json");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));

    let writers: Vec<_> = [(11.0_f64, 1_i64), (99.0_f64, 2_i64)]
        .into_iter()
        .map(|(pct, now)| {
            let (path, barrier) = (path.clone(), barrier.clone());
            std::thread::spawn(move || {
                let payload = format!(
                    r#"{{"rate_limits":{{"five_hour":{{"used_percentage":{pct},"resets_at":7}}}}}}"#
                );
                barrier.wait();
                for _ in 0..ROUNDS {
                    assert!(record_snapshot(&payload, &path, now));
                }
            })
        })
        .collect();

    barrier.wait();
    let mut seen = 0;
    for _ in 0..ROUNDS * 4 {
        if let Ok(text) = std::fs::read_to_string(&path) {
            let snap: BudgetSnapshot = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("torn snapshot published: {e}: {text:?}"));
            let pct = snap.five_hour.unwrap().used_percentage;
            assert!(
                (pct, snap.captured_at) == (11.0, 1) || (pct, snap.captured_at) == (99.0, 2),
                "blended snapshot: pct {pct} with captured_at {}",
                snap.captured_at
            );
            seen += 1;
        }
    }
    for w in writers {
        w.join().unwrap();
    }
    assert!(seen > 0, "reader never observed a published snapshot");

    // Both writers' temp files must have been renamed, none left behind.
    let entries: Vec<_> = std::fs::read_dir(tmp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(entries.len(), 1, "expected only the snapshot, got {entries:?}");
}
```

Barrier-synchronised, no sleeps. The reader loop is bounded by iteration count,
not time, and a missing file (before the first rename) is tolerated — only a
*parsed* value is asserted on, so the test cannot flake on ordering, only on the
actual defect. `leaves_no_temp_files_behind` stays: it is the single-writer
statement of the same hygiene property.

---

## 4. No-DB assertion — `tests/cli.rs`

**Why:** the design called for asserting the subcommand creates no DB file. What
shipped was a plan-time `grep` (which can never come back clean, since the module
doc comment names `Database` — already recorded as a Task 3 minor). The real
guarantee is observable: run the subcommand and look for the file.

**Test** (new, in `tests/cli.rs`, alongside the other binary-invoking tests):

```rust
#[test]
fn statusline_creates_no_database_file() {
    // The decorator runs several times a second in every Claude session; it
    // must never touch the database. dispatch.allium: StatusLineDecorator
    // (@guarantee NeverOpensTheDatabase).
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("tasks.db");
    let snapshot = tmp.path().join("rate-limits.json");

    let mut child = binary()
        .args([
            "--db", db_path.to_str().unwrap(),
            "statusline", "--snapshot", snapshot.to_str().unwrap(),
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap()
        .write_all(br#"{"rate_limits":{"five_hour":{"used_percentage":5.0,"resets_at":9}}}"#)
        .unwrap();
    let status = child.wait().unwrap();

    assert!(status.success(), "the decorator must always exit 0");
    assert!(snapshot.exists(), "the snapshot must have been published");
    assert!(
        !db_path.exists(),
        "the statusline subcommand must not create a database file"
    );
}
```

A plain `#[test]` (not `#[tokio::test]`) — this one drives only the child process.
Uses `--db` with a path that does not exist so a `Database::open` anywhere on the
path would create it and fail the assertion.

---

## 5. Epic-view degradation test — `src/tui/tests/budget.rs`

**Why:** the design called for degradation tests "in board AND epic view"; only
board view shipped. Epic view is the wide case — `render_top_indicators`
(`src/tui/ui/shared.rs:153`) adds `auto dispatch [U]`, an optional `role:…`
label, and `group:on [R]` — so it is where the budget span is most likely to be
squeezed, and where a wrong `used_width` sum would clip a pre-existing badge.

**Test** (in `mod render_glue`), mirroring the three board-view cases but in one
epic-view test at the width where degradation is forced by the epic badges alone:

```rust
/// Epic view carries the widest occupancy of the top row: `auto dispatch [U]`,
/// a feed-role label, and `group:on [R]` on top of the repo-filter and bell
/// badges. The budget span must degrade to fit around them, and none of them
/// may be dropped or clipped.
#[test]
fn epic_view_badges_squeeze_the_budget_but_are_never_clipped() {
    let mut app = app_with_badges_and_budget();
    let mut epic = make_epic(1);
    epic.auto_dispatch = true;
    epic.group_by_repo = true;
    app.board.epics = vec![epic];
    app.board.view_mode = ViewMode::Epic { epic_id: EpicId(1), .. };
    app.invalidate_layout_cache();

    let row = top_row(&mut app, 80);
    assert!(row.contains("auto dispatch [U]"), "got {row:?}");
    assert!(row.contains("group:on [R]"), "got {row:?}");
    assert!(row.contains("[1/2 repos]"), "got {row:?}");
    assert!(row.trim_end().ends_with("[N]"), "bell must render intact: {row:?}");
    assert!(row.contains("5h 23%"), "the 5h window must survive: {row:?}");
    assert!(!row.contains('\u{00B7}'), "countdowns must be dropped to fit: {row:?}");
}
```

Plus a very-narrow epic-view case asserting the budget disappears entirely while
every epic badge still renders. The exact widths (80 / 40 above) are derived
during implementation from the measured badge widths, exactly as
`narrow_width_never_clips_the_bell_badge` documents its width=32 choice — the
committed test states the arithmetic in a comment rather than a bare number.
`ViewMode::Epic`'s remaining fields are filled per the existing epic-view tests
(`src/tui/tests/epics.rs:136`); `invalidate_layout_cache()` is called because the
test mutates `board.epics` directly (learning #201).

---

## 6. Make `all_spawn_sites_inject_the_statusline_settings_file` true — `src/dispatch/tests.rs`

**Why:** it asserts on the *constant*, not on any spawn site, so a new spawn site
that forgot to interpolate `DISPATCH_PLUGIN_DIR` would pass. `create_main_session`
has no `send_keys` assertion for either flag.

**Change:** rewrite it to drive all three spawn sites through
`MockProcessRunner` and assert the flags are in the payload actually sent to
`tmux send-keys`:

- `dispatch_agent` — same mock script as
  `dispatch_agent_opens_tmux_window_in_worktree_not_parent_repo`
  (`src/dispatch/tests.rs:2784`); assert the `send-keys -l` payload contains both
  `--plugin-dir ~/.claude/plugins/local/dispatch` and
  `--settings ~/.claude/dispatch-statusline.json`.
- `resume_agent` — mock new-window / set-option / set-hook / send-keys ×2 /
  split-window; same assertion.
- `create_main_session` — mock new-window / send-keys ×2; same assertion.

One test per site, each named for its site, plus a comment saying the trio is the
whole set of `DISPATCH_PLUGIN_DIR` interpolation sites (grep-checkable:
`DISPATCH_PLUGIN_DIR` appears in `src/dispatch/agents.rs` exactly three times as
an interpolation). The existing
`spawn_constant_has_exactly_one_space_between_flags` and
`spawn_constant_contains_no_whitespace_hazard` stay — they pin the constant's
shape, which is a different claim.
`create_main_session_sends_claude_with_plugin_dir` becomes redundant with the new
per-site test and is folded into it.

---

## 7. Delete `budget_stale_after_is_ten_minutes` — `src/tui/tests/budget.rs:76`

**Why:** it restates a literal against itself (`assert_eq!(BUDGET_STALE_AFTER,
Duration::from_secs(600))`), and its stated justification — dodging a
`dead_code` warning before the renderer landed — expired when
`render_top_indicators` started consuming the constant
(`src/tui/ui/shared.rs`).

**Change:** delete the test. Confirm the constant is still consumed (it is, by
`render_top_indicators`) so `cargo clippy --all-targets -- -D warnings` stays
clean.

---

## 8. Initial budget refresh at startup — `src/runtime/mod.rs`

**Why:** `tick_budget_poll` (`src/tui/update/agent.rs:394`) increments before
comparing, so the first `Refresh` fires ~10 s after startup while `App::new`
leaves `budget: None`. The badge is blank for the first 10 s of every session
even when a warm snapshot is already on disk.

**Test first** (`src/runtime/tests.rs`):

```rust
#[test]
fn startup_commands_refresh_the_budget_snapshot() {
    // App::new starts with budget: None and the first tick-driven poll is
    // BUDGET_POLL_TICKS away, so without this the badge is blank for the
    // first ~10s of every session even with a warm snapshot on disk.
    assert!(
        startup_commands()
            .iter()
            .any(|c| matches!(c, Command::Budget(BudgetCommand::Refresh))),
        "startup must prime the budget snapshot"
    );
}
```

**Implementation:** a free `fn startup_commands() -> Vec<Command>` in
`src/runtime/mod.rs` returning `vec![Command::Budget(BudgetCommand::Refresh)]`,
executed once in `run_loop` before the event loop:

```rust
execute_commands(app, startup_commands(), rt, terminal, key_rx).await?;
```

Placed after the `feed_runner.start()` block and before the `loop`, so the first
frame either already has the snapshot or gets a `dirty` redraw as soon as the
`spawn_blocking` read lands. A one-line list rather than an inline
`exec_refresh_budget()` call so the "what runs at startup" set is one
unit-testable value, and future startup priming has an obvious home.

Also update `TokenBudgetIndicator`'s
`RefreshedPeriodicallyNoRedrawWhenUnchanged` guarantee to say the snapshot is
read once at startup and then every `budget_poll_interval` (spec change, via
`allium:tend`, done together with item 1).

---

## 9. Doc nit — `CLAUDE.md:120`

**Why:** the External Dependencies entry describes the spawn as
`claude --plugin-dir ~/.claude/plugins/local/dispatch …`. The `--settings` flag
is now always present and load-bearing: `claude` refuses to start if the settings
file is missing, which is why `runtime::bootstrap` recreates it.

**Change:** name both flags and the recovery path:

> - **claude** — spawned inside the tmux window by `src/dispatch/agents.rs` as
>   `claude --plugin-dir ~/.claude/plugins/local/dispatch --settings ~/.claude/dispatch-statusline.json`.
>   Both flags are load-bearing: the plugin dir is installed by
>   `cargo run -- setup`, and `claude` refuses to start at all if the settings
>   file is absent — so `runtime::bootstrap` recreates it best-effort at TUI
>   startup (`src/setup/statusline.rs` generates it).

---

## Verification

Per-item, then the whole gate:

```
cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh
```

Plus, because the pre-push hook runs them and this change touches specs, docs and
a `#[cfg(test)]`-adjacent production file:

```
cargo clippy --all-targets -- -D warnings
./scripts/check-doc-symbols.sh
./scripts/check-no-test-sleep.sh
allium check docs/specs/dispatch.allium
```

No snapshot files change (no rendered output changes), so no
`cargo insta review` and no `.snap.new` cleanup is expected — if a snapshot does
move, that is a regression to investigate, not to accept.
