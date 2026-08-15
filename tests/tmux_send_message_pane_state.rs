#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Real-tmux integration test for `notify::notify_tmux`'s pane-readiness probe
//! (task #4098). A mock test can only assert the argv `notify_tmux` hands
//! tmux — it cannot prove that a real `tmux capture-pane -p` snapshot of a
//! pane actually showing a modal (no "shift+tab to cycle" anywhere on
//! screen) suppresses `send-keys`, or that a pane showing Claude Code's idle
//! footer still receives it. See
//! `docs/superpowers/specs/2026-08-15-send-message-delivery-hardening-design.md`
//! for the reproduction this guards against, and
//! `tests/tmux_split_hook.rs` for the sibling "a mock pinned the vulnerable
//! behaviour" pattern this follows.
//!
//! Each pane here prints a fixed screen via `printf` (what `capture-pane -p`
//! sees) and then `exec cat >> <log>` (so any `send-keys` payload that
//! reaches the pane is captured, exactly like `tests/tmux_split_hook.rs`'s
//! keystroke capture) — no real `claude` process involved, so this needs no
//! network/API access in CI.

mod tmux_harness;

use std::path::PathBuf;

use dispatch_tui::notify::{self, DeliveryOutcome};

use tmux_harness::{tmux_available_or_skip, typed_input, TmuxServer};

const WINDOW: &str = "task-42";

/// A capture-pane snapshot representing Claude Code idle at its own chat
/// input — the "safe to inject keystrokes" case. Matches the marker
/// `notify::notify_tmux` looks for.
const READY_SCREEN: &str = "> \\nauto mode on (shift+tab to cycle) - 1 agent\\n";

/// A capture-pane snapshot representing a plan-mode/elicitation dialog — the
/// exact shape reproduced in the design doc. No "shift+tab to cycle"
/// anywhere.
const DIALOG_SCREEN: &str = "> 1. MIT\\n  2. Apache 2.0\\n\\nEnter to select - Esc to cancel\\n";

struct Fixture {
    server: TmuxServer,
    dir: tempfile::TempDir,
    log: PathBuf,
}

/// Start a tmux window whose pane prints `screen` (visible to
/// `capture-pane -p`) and then reads its stdin into a log file, so a test can
/// tell whether `send-keys` reached it. Skips (returns `None`) when tmux is
/// unavailable, matching the repo's real-tmux test convention.
fn setup_or_skip(screen: &str) -> Option<Fixture> {
    if !tmux_available_or_skip() {
        return None;
    }
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("typed.log");

    let server = TmuxServer::start();
    server.tmux_ok(&[
        "new-session",
        "-d",
        "-s",
        "t",
        "-n",
        WINDOW,
        "--",
        "sh",
        "-c",
        &format!("printf '{screen}'; exec cat >> {}", log.display()),
    ]);

    // Wait for the pane to actually render the screen before any test reads
    // it — `new-session` returns before the shell has run `printf`.
    assert!(
        tmux_harness::poll_until(|| {
            server
                .tmux_stdout(&["capture-pane", "-p", "-t", WINDOW])
                .contains("shift+tab to cycle")
                || server
                    .tmux_stdout(&["capture-pane", "-p", "-t", WINDOW])
                    .contains("Enter to select")
        }),
        "pane never rendered its fixture screen"
    );

    Some(Fixture { server, dir, log })
}

impl Fixture {
    fn worktree(&self) -> String {
        self.dir.path().to_str().unwrap().to_string()
    }
}

#[test]
fn notify_tmux_sends_keys_when_the_real_pane_shows_the_ready_footer() {
    let Some(fx) = setup_or_skip(READY_SCREEN) else {
        return;
    };
    let runner = fx.server.runner();
    let worktree = fx.worktree();

    let outcome = notify::notify_tmux(&runner, &worktree, WINDOW, "does-not-exist.md", "hello")
        .expect("notify_tmux should succeed against a live window");

    assert_eq!(outcome, DeliveryOutcome::Notified);
    assert!(
        tmux_harness::poll_until(|| typed_input(&fx.log) == "hello"),
        "expected 'hello' to reach the pane; got: {:?}",
        typed_input(&fx.log)
    );
}

#[test]
fn notify_tmux_withholds_keys_when_the_real_pane_shows_a_dialog() {
    let Some(fx) = setup_or_skip(DIALOG_SCREEN) else {
        return;
    };
    let runner = fx.server.runner();
    let worktree = fx.worktree();

    let outcome = notify::notify_tmux(&runner, &worktree, WINDOW, "does-not-exist.md", "hello")
        .expect("notify_tmux should succeed (queued, not nudged) against a live window");

    assert_eq!(outcome, DeliveryOutcome::QueuedNoNudge);
    // No wait needed: `notify_tmux` is synchronous and never issues the
    // `send-keys` subprocess call at all on this branch — by the time it has
    // returned, there is nothing still in flight that could land afterward.
    // This is the invariant the reproduction in the design doc violated.
    assert_eq!(
        typed_input(&fx.log),
        "",
        "no keystrokes must reach a pane showing a dialog"
    );
}
