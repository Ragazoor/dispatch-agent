# Feed stderr evidence Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a feed command's stderr visible when the command exits 0, so a script that silently fails into an empty emission leaves evidence in `app.log` and in the status bar.

**Architecture:** `src/feed/exec.rs::exec_feed_command` becomes the single exec used by both the auto-poll path (`FeedJob::run`) and the manual "r" path (`exec_trigger_epic_feed`, which today carries a private copy). It returns `Result<FeedOutput, String>` where `FeedOutput` carries stdout plus any stderr written on a zero exit, and it logs a WARN for that stderr itself. The manual path passes a `wrote_stderr` flag through `FeedMessage::Refreshed` so the status bar can add a hint when a refresh syncs 0 items.

**Tech Stack:** Rust 2021, tokio, tracing, ratatui TUI, Allium specs.

**Design doc:** `docs/superpowers/specs/2026-08-12-feed-stderr-evidence-design.md`

## Global Constraints

- Spec first, then tests, then code. `docs/specs/feeds.allium` is the source of truth and is updated in Task 1, before any Rust changes.
- TDD: every code task writes a failing test, runs it to confirm the failure, then implements.
- Inline test modules need `#![allow(clippy::unwrap_used, clippy::expect_used)]` at the top. The existing `mod tests` blocks in the files touched here already have it — do not add a second one.
- Tests must never sleep on the wall clock. `./scripts/check-no-test-sleep.sh` rejects `tokio::time::sleep` anywhere under `src/`. The runtime tests use `tokio::time::timeout(TEST_TIMEOUT, rx.recv())`, which is allowed and is the pattern to follow.
- stderr truncation cap: exactly **2000 characters** (`MAX_FEED_STDERR_CHARS`).
- Status-bar hint text, verbatim: `" — command wrote to stderr (see app.log)"` (note the em dash `—`, not a hyphen).
- The hint appears only when `count == 0 && wrote_stderr`. Never when `count > 0`, never when stderr was empty.
- Verification command for this repo: `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`.
- Do not cite `file:NN` line numbers in `docs/specs/feeds.allium`; use the `path::symbol` form (`src/feed/exec.rs::exec_feed_command`). `./scripts/check-doc-paths.sh` validates line citations and they rot immediately.

---

### Task 1: Spec the behaviour in `feeds.allium`

**Files:**
- Modify: `docs/specs/feeds.allium` (new rule after `FeedCommandFailure`; two edits inside `ManualFeedTrigger`)

**Interfaces:**
- Consumes: nothing.
- Produces: the rule name `FeedCommandStderrOnSuccess`, referenced by later tasks' commit messages and by the `ManualFeedTrigger` cross-reference.

- [ ] **Step 1: Add the new rule after `FeedCommandFailure`**

Find this exact text — the end of `FeedCommandFailure`'s `@guidance` block, its closing brace, and the comment that follows:

```
        -- A future improvement is surfacing repeated failures in the TUI
        -- as an epic-level health badge, but that is out of scope here.
}

-- FeedSync (conceptual): single dispatch point for feed upsert. Routing
```

Replace it with:

```
        -- A future improvement is surfacing repeated failures in the TUI
        -- as an epic-level health badge, but that is out of scope here.
}

rule FeedCommandStderrOnSuccess {
    when: FeedCommandCompleted(epic, stderr)

    -- A feed command that exits 0 while writing to stderr is NOT a
    -- failure: its stdout parsed, and the emission is synced normally.
    -- But it is the signature of a script that swallowed an internal
    -- error and emitted a degraded (often empty) array anyway, which
    -- FeedCommandFailure never sees. The stderr text is therefore logged
    -- at warn level so the evidence survives the run.
    --
    -- This is diagnostic only. The sync proceeds exactly as it would for
    -- a command that wrote nothing to stderr — in particular an empty
    -- emission still reconciles, and still removes tasks absent from it.
    --
    -- The logged text is trimmed and truncated to 2000 characters: a
    -- verbose script must not be able to write unbounded output into
    -- app.log on every poll.

    requires: exit_status == 0 AND stderr != ""

    ensures:
        log_warn(epic.id, epic.title, truncate(stderr, 2000))
        -- and then the emission syncs as normal (FeedSync)

    @guidance
        -- Implementation: src/feed/exec.rs::exec_feed_command logs the
        -- warning and returns the trimmed, truncated text on its
        -- FeedOutput so callers can surface it too. It is the SINGLE exec
        -- for both feed paths — src/feed/mod.rs::FeedJob (auto-poll) and
        -- src/runtime/epics.rs::exec_trigger_epic_feed (manual "r") both
        -- call it, so neither can regress to a private spawn that drops
        -- stderr. Only the manual path surfaces it in the TUI, via
        -- ManualFeedTrigger's zero-item hint.
}

-- FeedSync (conceptual): single dispatch point for feed upsert. Routing
```

- [ ] **Step 2: Add the hint to `ManualFeedTrigger`'s status-bar comment**

Find:

```
    --   - On success: "Feed for '<title>': N task(s) synced"
    --   - On failure: "Feed for '<title>' failed: <error>"
```

Replace with:

```
    --   - On success: "Feed for '<title>': N task(s) synced"
    --     When N = 0 AND the command wrote to stderr while exiting 0,
    --     this gains the suffix " — command wrote to stderr (see
    --     app.log)" (see FeedCommandStderrOnSuccess). The suffix is
    --     gated on N = 0 so a script that writes harmless progress
    --     chatter to stderr does not nag on every refresh.
    --   - On failure: "Feed for '<title>' failed: <error>"
```

- [ ] **Step 3: Add the hint to `ManualFeedTrigger`'s `ensures` block**

Find:

```
        -- Spawns epic.feed_command; on success:
        --   FeedSync(epic, items)
        --   status_bar = "Feed for '{epic.title}': {count} task(s) synced"
        -- on failure:
```

Replace with:

```
        -- Spawns epic.feed_command; on success:
        --   FeedSync(epic, items)
        --   status_bar = "Feed for '{epic.title}': {count} task(s) synced"
        --   plus " — command wrote to stderr (see app.log)" when
        --   count = 0 AND the command wrote to stderr
        -- on failure:
```

- [ ] **Step 4: Validate the spec**

Run: `allium check docs/specs/feeds.allium`
Expected: no errors.

Run: `./scripts/check-doc-paths.sh`
Expected: passes. (It validates every `src/…` path in the specs. `src/feed/exec.rs`, `src/feed/mod.rs`, and `src/runtime/epics.rs` all exist; `FeedOutput` does not exist yet but is not a path, and `./scripts/check-doc-symbols.sh` only rejects backticked snake_case identifiers — `FeedOutput` is neither backticked nor snake_case here.)

- [ ] **Step 5: Commit**

```bash
git add docs/specs/feeds.allium
git commit -m "docs(specs): spec FeedCommandStderrOnSuccess and the zero-item stderr hint"
```

---

### Task 2: `exec_feed_command` returns stdout + stderr

**Files:**
- Modify: `src/feed/exec.rs` (the `exec_feed_command` function and its inline `mod tests`)
- Modify: `src/feed/mod.rs:18` (re-export line) and `src/feed/mod.rs:84` (`FeedJob::run` call site)

**Interfaces:**
- Consumes: nothing from Task 1 (spec only).
- Produces, both re-exported from `crate::feed`:
  - `pub(crate) struct FeedOutput { pub(crate) stdout: Vec<u8>, pub(crate) stderr: String }`
  - `pub(crate) async fn exec_feed_command(cmd: &str, epic_id: i64, epic_title: &str) -> Result<FeedOutput, String>`

  `Err(String)` carries the human-readable failure text (spawn error message, or the failed command's stderr). `Ok(FeedOutput).stderr` is trimmed and truncated, and is empty when the command wrote nothing.

- [ ] **Step 1: Write the failing tests**

In `src/feed/exec.rs`, replace the four existing `exec_feed_command` tests (from `// --- exec_feed_command ---` through the end of `exec_feed_command_returns_empty_vec_on_zero_exit_with_no_output`) with these. The old tests assert the `Option` return that this task removes, so they are rewritten rather than kept alongside.

```rust
    // --- exec_feed_command ---

    #[tokio::test]
    async fn exec_feed_command_returns_stdout_on_success() {
        let out = exec_feed_command("printf 'hello'", 1, "test-epic")
            .await
            .expect("zero-exit command must succeed");
        assert_eq!(out.stdout, b"hello".to_vec());
        assert_eq!(out.stderr, "", "no stderr written, so none reported");
    }

    #[tokio::test]
    async fn exec_feed_command_returns_err_on_nonzero_exit() {
        let result = exec_feed_command("exit 1", 2, "test-epic").await;
        assert!(result.is_err(), "non-zero exit must be an error");
    }

    #[tokio::test]
    async fn exec_feed_command_err_carries_stderr_of_failed_command() {
        let err = exec_feed_command("echo 'error msg' >&2; exit 1", 3, "test-epic")
            .await
            .expect_err("non-zero exit must be an error");
        assert!(
            err.contains("error msg"),
            "error must carry the command's stderr, got: {err}"
        );
    }

    #[tokio::test]
    async fn exec_feed_command_returns_empty_stdout_on_zero_exit_with_no_output() {
        let out = exec_feed_command("true", 4, "test-epic")
            .await
            .expect("zero-exit command must succeed");
        assert!(out.stdout.is_empty(), "no stdout written");
        assert_eq!(out.stderr, "");
    }

    // The regression this whole change exists for: a command that fails
    // internally, writes the reason to stderr, and STILL exits 0 with a valid
    // (empty) JSON array. Previously the stderr was captured and discarded.
    #[tokio::test]
    async fn exec_feed_command_captures_stderr_written_on_zero_exit() {
        let out = exec_feed_command("echo 'Invalid search query' >&2; printf '[]'", 5, "test-epic")
            .await
            .expect("zero exit must still succeed");
        assert_eq!(out.stdout, b"[]".to_vec(), "stdout must be untouched");
        assert_eq!(
            out.stderr, "Invalid search query",
            "stderr written on a zero exit must be reported, trimmed"
        );
    }

    #[tokio::test]
    async fn exec_feed_command_truncates_long_stderr() {
        // 3000 'x' characters on stderr, valid empty array on stdout.
        let out = exec_feed_command(
            "printf '%3000s' '' | tr ' ' x >&2; printf '[]'",
            6,
            "test-epic",
        )
        .await
        .expect("zero exit must still succeed");
        assert_eq!(
            out.stderr.chars().count(),
            MAX_FEED_STDERR_CHARS,
            "stderr must be truncated to the cap"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test feed::exec::tests::exec_feed_command`
Expected: FAIL to compile — `MAX_FEED_STDERR_CHARS` not found, and `expect`/`expect_err` are not methods on `Option`.

- [ ] **Step 3: Implement `FeedOutput` and the new `exec_feed_command`**

In `src/feed/exec.rs`, replace the whole existing `exec_feed_command` function (and its doc comment) with:

```rust
/// Maximum number of characters of a feed command's stderr that are logged and
/// carried on [`FeedOutput`]. A verbose script must not be able to write
/// unbounded output into `app.log` on every poll.
pub(crate) const MAX_FEED_STDERR_CHARS: usize = 2000;

/// A successful feed command's output.
pub(crate) struct FeedOutput {
    /// Raw stdout bytes, to be parsed as a FeedItem JSON array.
    pub(crate) stdout: Vec<u8>,
    /// Anything the command wrote to stderr while still exiting 0 — trimmed
    /// and truncated to [`MAX_FEED_STDERR_CHARS`]. Empty when it wrote
    /// nothing. A non-empty value is the signature of a script that swallowed
    /// an internal error and emitted a degraded array anyway, so it is logged
    /// here and surfaced by the manual-refresh path (feeds.allium:
    /// FeedCommandStderrOnSuccess).
    pub(crate) stderr: String,
}

/// Execute the feed shell command.
///
/// On success returns stdout plus any stderr the command wrote while exiting 0,
/// logging a warning for that stderr — it is a diagnostic, not a failure, and
/// the caller syncs the emission as normal.
///
/// On spawn failure or non-zero exit, logs a warning (as before) and returns
/// `Err` with the failure text, so a caller that must surface the error to the
/// user — `exec_trigger_epic_feed`'s status bar — has it.
pub(crate) async fn exec_feed_command(
    cmd: &str,
    epic_id: i64,
    epic_title: &str,
) -> Result<FeedOutput, String> {
    let output = match tokio::process::Command::new("sh")
        .args(["-c", cmd])
        .output()
        .await
    {
        Ok(o) => o,
        Err(err) => {
            let msg = format!("{err:#}");
            tracing::warn!(
                epic_id,
                epic_title,
                "FeedRunner: failed to spawn command: {msg}"
            );
            return Err(msg);
        }
    };

    let stderr = truncate_stderr(&output.stderr);

    if !output.status.success() {
        tracing::warn!(
            epic_id,
            epic_title,
            "FeedRunner: command exited non-zero: {stderr}"
        );
        return Err(stderr);
    }

    if !stderr.is_empty() {
        tracing::warn!(
            epic_id,
            epic_title,
            "FeedRunner: command wrote to stderr but exited 0: {stderr}"
        );
    }

    Ok(FeedOutput {
        stdout: output.stdout,
        stderr,
    })
}

/// Decode stderr bytes lossily, trim surrounding whitespace, and cap the
/// length at [`MAX_FEED_STDERR_CHARS`]. Truncates on a character boundary, so
/// multi-byte output cannot panic here.
fn truncate_stderr(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim();
    trimmed.chars().take(MAX_FEED_STDERR_CHARS).collect()
}
```

- [ ] **Step 4: Update the re-export and the auto-poll call site**

In `src/feed/mod.rs`, change line 18 from:

```rust
pub(crate) use exec::resolve_base_branches;
```

to:

```rust
pub(crate) use exec::{exec_feed_command, resolve_base_branches, FeedOutput};
```

Then in `FeedJob::run` (`src/feed/mod.rs:84`), change:

```rust
        let Some(stdout) =
            exec::exec_feed_command(&self.cmd, self.epic.id.0, &self.epic.title).await
        else {
            return;
        };
```

to:

```rust
        // exec_feed_command has already logged the failure (and any
        // stderr-on-success); the auto-poll path adds nothing, per
        // feeds.allium FeedCommandFailure ("the TUI is NOT notified").
        let Ok(output) =
            exec::exec_feed_command(&self.cmd, self.epic.id.0, &self.epic.title).await
        else {
            return;
        };
        let stdout = output.stdout;
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test feed::`
Expected: PASS, including the six `exec_feed_command` tests.

- [ ] **Step 6: Check clippy, since `-D warnings` gates the push**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: no warnings. If `FeedOutput.stderr` is reported as never read, that is expected at this point in the plan — Task 3 adds the reader. Leave it; do not add `#[allow(dead_code)]`.

- [ ] **Step 7: Commit**

```bash
git add src/feed/exec.rs src/feed/mod.rs
git commit -m "feat(feed): log a feed command's stderr even when it exits 0"
```

---

### Task 3: Manual refresh uses the shared exec and hints in the status bar

**Files:**
- Modify: `src/runtime/epics.rs` (`exec_trigger_epic_feed`, the private spawn block at `src/runtime/epics.rs:263`)
- Modify: `src/tui/messages/feed.rs:16` (`FeedMessage::Refreshed`) and `:26` (its `route` arm)
- Modify: `src/tui/update/feeds.rs:34` (`handle_feed_refreshed`)
- Test: `src/tui/tests/epics.rs` (beside `feed_refreshed_sets_status_and_returns_refresh_from_db`)
- Test: `src/runtime/tests.rs` (beside `exec_trigger_epic_feed_zero_items`)

**Interfaces:**
- Consumes: `crate::feed::exec_feed_command` and `crate::feed::FeedOutput` from Task 2.
- Produces: `FeedMessage::Refreshed { epic_title: String, count: usize, wrote_stderr: bool }` and `App::handle_feed_refreshed(&mut self, epic_title: String, count: usize, wrote_stderr: bool) -> Vec<Command>`.

- [ ] **Step 1: Write the failing status-bar tests**

In `src/tui/tests/epics.rs`, add the two existing `Refreshed` constructions the new field, then add the new cases. First, in `feed_refreshed_sets_status_and_returns_refresh_from_db` and `feed_refreshed_zero_items_still_succeeds`, add `wrote_stderr: false,` after the `count` field. Then append these tests after `feed_refreshed_zero_items_still_succeeds`:

```rust
#[test]
fn feed_refreshed_zero_items_with_stderr_hints_at_the_log() {
    let mut app = App::new(vec![]);

    let cmds = app.update(Message::Feed(
        crate::tui::messages::FeedMessage::Refreshed {
            epic_title: "PR Reviews".to_string(),
            count: 0,
            wrote_stderr: true,
        },
    ));

    let status = app.status_message().unwrap_or("");
    assert!(
        status.contains("command wrote to stderr"),
        "a 0-item sync whose command wrote to stderr must point at the log, got: {status}"
    );
    assert!(
        status.contains("app.log"),
        "the hint must name app.log so the user knows where to look, got: {status}"
    );
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Task(crate::tui::commands::TaskCommand::RefreshFromDb)
        )),
        "the sync still succeeded, so it must still refresh"
    );
}

#[test]
fn feed_refreshed_zero_items_without_stderr_has_no_hint() {
    let mut app = App::new(vec![]);

    app.update(Message::Feed(
        crate::tui::messages::FeedMessage::Refreshed {
            epic_title: "Empty Feed".to_string(),
            count: 0,
            wrote_stderr: false,
        },
    ));

    let status = app.status_message().unwrap_or("");
    assert!(
        !status.contains("stderr"),
        "a genuinely empty feed is not an error and must not be flagged, got: {status}"
    );
}

// Gated on count == 0 deliberately: a script that writes harmless progress
// chatter to stderr must not nag on every successful refresh.
#[test]
fn feed_refreshed_with_items_and_stderr_has_no_hint() {
    let mut app = App::new(vec![]);

    app.update(Message::Feed(
        crate::tui::messages::FeedMessage::Refreshed {
            epic_title: "Chatty Feed".to_string(),
            count: 7,
            wrote_stderr: true,
        },
    ));

    let status = app.status_message().unwrap_or("");
    assert!(
        !status.contains("stderr"),
        "items synced, so stderr is not worth interrupting for, got: {status}"
    );
}
```

- [ ] **Step 2: Write the failing runtime test**

In `src/runtime/tests.rs`, add this after `exec_trigger_epic_feed_zero_items`:

```rust
// The exact shape of the PR Reviews failure: every internal query failed, the
// script reported why on stderr, and still exited 0 with an empty array.
#[tokio::test]
async fn exec_trigger_epic_feed_reports_stderr_written_on_zero_exit() {
    let db = test_db().await;
    let epic = db.create_epic("PR Reviews", "", None).await.unwrap();

    let (tx, mut rx) = mpsc::unbounded_channel();
    let rt = make_runtime(db, tx, Arc::new(MockProcessRunner::new(vec![]))).await;

    rt.exec_trigger_epic_feed(
        epic.id,
        "PR Reviews".to_string(),
        "echo 'Invalid search query' >&2; echo '[]'".to_string(),
        false,
    );

    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .expect("timed out")
        .expect("channel closed");
    assert!(
        matches!(
            msg,
            Message::Feed(crate::tui::messages::FeedMessage::Refreshed {
                count: 0,
                wrote_stderr: true,
                ..
            })
        ),
        "a zero-exit command that wrote to stderr must report it, got: {msg:?}"
    );
}

#[tokio::test]
async fn exec_trigger_epic_feed_quiet_command_reports_no_stderr() {
    let db = test_db().await;
    let epic = db.create_epic("Quiet Feed", "", None).await.unwrap();

    let (tx, mut rx) = mpsc::unbounded_channel();
    let rt = make_runtime(db, tx, Arc::new(MockProcessRunner::new(vec![]))).await;

    rt.exec_trigger_epic_feed(
        epic.id,
        "Quiet Feed".to_string(),
        "echo '[]'".to_string(),
        false,
    );

    let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
        .await
        .expect("timed out")
        .expect("channel closed");
    assert!(
        matches!(
            msg,
            Message::Feed(crate::tui::messages::FeedMessage::Refreshed {
                count: 0,
                wrote_stderr: false,
                ..
            })
        ),
        "a quiet command must not report stderr, got: {msg:?}"
    );
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test feed_refreshed && cargo test exec_trigger_epic_feed`
Expected: FAIL to compile — `FeedMessage::Refreshed` has no field `wrote_stderr`.

- [ ] **Step 4: Add the field to the message**

In `src/tui/messages/feed.rs`, change:

```rust
    /// Feed refresh succeeded.
    Refreshed { epic_title: String, count: usize },
```

to:

```rust
    /// Feed refresh succeeded. `wrote_stderr` is true when the feed command
    /// wrote to stderr while still exiting 0 — see feeds.allium
    /// FeedCommandStderrOnSuccess.
    Refreshed {
        epic_title: String,
        count: usize,
        wrote_stderr: bool,
    },
```

and in `route`, change:

```rust
            FeedMessage::Refreshed { epic_title, count } => {
                app.handle_feed_refreshed(epic_title, count)
            }
```

to:

```rust
            FeedMessage::Refreshed {
                epic_title,
                count,
                wrote_stderr,
            } => app.handle_feed_refreshed(epic_title, count, wrote_stderr),
```

- [ ] **Step 5: Add the hint in the handler**

In `src/tui/update/feeds.rs`, replace `handle_feed_refreshed` with:

```rust
    pub(in crate::tui) fn handle_feed_refreshed(
        &mut self,
        epic_title: String,
        count: usize,
        wrote_stderr: bool,
    ) -> Vec<Command> {
        // Only when nothing synced: a feed command that reported an error on
        // stderr and still exited 0 usually emitted a degraded array, and a
        // zero-item result is where that matters. Above zero, stderr is
        // chatter and the log line alone is enough.
        let hint = if count == 0 && wrote_stderr {
            " — command wrote to stderr (see app.log)"
        } else {
            ""
        };
        self.set_status(format!(
            "Feed for '{epic_title}': {count} task(s) synced{hint}"
        ));
        vec![Command::Task(
            crate::tui::commands::TaskCommand::RefreshFromDb,
        )]
    }
```

- [ ] **Step 6: Replace the manual path's private spawn block**

In `src/runtime/epics.rs`, inside `exec_trigger_epic_feed`'s `tokio::spawn`, replace this block:

```rust
            let output = match tokio::process::Command::new("sh")
                .args(["-c", &feed_command])
                .output()
                .await
            {
                Ok(o) => o,
                Err(e) => return fail(e.to_string()),
            };

            if !output.status.success() {
                return fail(String::from_utf8_lossy(&output.stderr).into_owned());
            }

            let items: Vec<models::FeedItem> = match serde_json::from_slice(&output.stdout) {
                Ok(i) => i,
                Err(e) => return fail(e.to_string()),
            };
```

with:

```rust
            // The SAME exec the auto-poll FeedRunner uses, so neither path can
            // drop a feed command's stderr again (feeds.allium:
            // FeedCommandStderrOnSuccess). It logs spawn/non-zero failures and
            // stderr-on-success itself; we add the status-bar surface.
            let output: crate::feed::FeedOutput =
                match crate::feed::exec_feed_command(&feed_command, epic_id.0, &epic_title).await {
                    Ok(o) => o,
                    Err(e) => return fail(e),
                };
            let wrote_stderr = !output.stderr.is_empty();

            let items: Vec<models::FeedItem> = match serde_json::from_slice(&output.stdout) {
                Ok(i) => i,
                Err(e) => return fail(e.to_string()),
            };
```

Then in the same function's success arm, change:

```rust
                    let _ = tx.send(Message::Feed(
                        crate::tui::messages::FeedMessage::Refreshed { epic_title, count },
                    ));
```

to:

```rust
                    let _ = tx.send(Message::Feed(
                        crate::tui::messages::FeedMessage::Refreshed {
                            epic_title,
                            count,
                            wrote_stderr,
                        },
                    ));
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test feed_refreshed && cargo test exec_trigger_epic_feed`
Expected: PASS — five `feed_refreshed*` tests (2 existing + 3 new) and eight `exec_trigger_epic_feed*` tests (6 existing + 2 new).

- [ ] **Step 8: Run the full verification**

Run: `cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh`
Expected: PASS. Then `cargo clippy --all-targets -- -D warnings`, expected clean — `FeedOutput.stderr` now has a reader.

If `cargo fmt --check` fails, run `cargo fmt` and re-check the diff before staging: a scoped `cargo fmt -- <files>` can still reformat unrelated files.

- [ ] **Step 9: Commit**

```bash
git add src/runtime/epics.rs src/tui/messages/feed.rs src/tui/update/feeds.rs src/tui/tests/epics.rs src/runtime/tests.rs
git commit -m "feat(feed): hint at stderr when a manual refresh syncs zero tasks"
```

---

## Out of scope (file as follow-up tasks at wrap-up)

Both were found while diagnosing this and are deliberately excluded — they are behaviour changes, not evidence:

1. **An empty emission wipes the subtree.** `delete_stale_subtree` runs with `all_external_ids = []`, and `external_id NOT IN (SELECT value FROM json_each('[]'))` matches every feed task in the subtree (`src/db/queries/tasks.rs:558`). A silent-failure run deletes what was already in My/Team/Bots, via a raw `DELETE` with no worktree/tmux cleanup, orphaning any in-flight review agent's worktree. Candidate guard: treat a zero-item emission whose command wrote to stderr as a `FeedCommandFailure` and skip the sync.
2. **The two paths parse differently.** `exec_trigger_epic_feed` uses `serde_json::from_slice` while `FeedJob::run` uses `parse::parse_feed_items`, which warns on unknown tags. Task 2 unified the exec; the parse is still duplicated.
