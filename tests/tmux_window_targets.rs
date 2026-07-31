#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Real-tmux integration tests for **exact** window-name targeting in the
//! `tmux::` helpers (task #3798).
//!
//! # The defect these guard against
//!
//! tmux resolves a `-t <window-name>` target by exact match *and then by
//! prefix*. Dispatch names windows `task-<id>`, so with ids in the thousands one
//! task's window name is a prefix of another's (`task-378` / `task-3782`). The
//! realistic trigger is an operation on a task whose window has died — resume,
//! kill, jump — while a longer-named sibling is alive: tmux happily resolves the
//! absent name to the live sibling. The consequences are the wrong task's Claude
//! session being typed into, and the wrong task's window being killed.
//!
//! See the `TmuxWindowTargetedExactly` invariant in docs/specs/dispatch.allium.
//!
//! # Why this needs a real tmux server
//!
//! `MockProcessRunner` records argv, so a mock test can assert *"we handed tmux
//! this string"* but never *"tmux did what we meant"* — and the defect is purely
//! a matter of tmux's own target-resolution semantics. Every mock test in
//! src/tmux.rs pinned the vulnerable `-t task-42` argv and stayed green. Worse,
//! the obvious fix (tmux's `=name` exact-match sigil) is *rejected* by
//! `send-keys` and `set-option -w`, and `display-message` accepts it while
//! printing nothing and exiting zero — none of which a mock can see either. So
//! the assertions below are about observed effect: which window received the
//! keystrokes, which window is still alive.
//!
//! No test anywhere had two windows whose names were prefixes of each other
//! before this file; that absence is why the bug shipped.
//!
//! # Isolation
//!
//! Private `-L` socket per test with drop-guard teardown, from the shared rig in
//! tests/tmux_harness/mod.rs. Its siblings are tests/tmux_lifecycle.rs
//! (topology: which windows and panes exist) and tests/tmux_split_hook.rs
//! (routing: which pane a keystroke reached). This file's question is narrower
//! and prior to both: whether a named target resolves to the window it names.

mod tmux_harness;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use dispatch_tui::tmux;

use tmux_harness::{
    capture_cmd, read_now, read_when_written, tmux_available_or_skip, SocketRunner, TmuxServer,
};

/// The board TUI window — always the session's first (and initially active)
/// window, matching the state during any board-initiated operation.
const BOARD: &str = "board";

struct Fixture {
    // Declared before `_dir` on purpose: fields drop in declaration order, so the
    // server (and its `cat` processes) dies before the temp dir holding their
    // capture files is unlinked.
    server: TmuxServer,
    /// Root for the capture files, held purely to keep them on disk for the
    /// fixture's lifetime — the pane processes write into it and `logs` carries
    /// the paths. Named `_dir` because it is an RAII guard, never read.
    _dir: tempfile::TempDir,
    logs: HashMap<String, PathBuf>,
}

/// Build a server whose windows are exactly `names` (in order, [`BOARD`] first),
/// each running a capture command — or skip when tmux is unavailable.
///
/// Folding the tmux guard into construction means a later test cannot forget it.
fn setup_or_skip(names: &[&str]) -> Option<Fixture> {
    if !tmux_available_or_skip() {
        return None;
    }
    Some(setup(names))
}

fn setup(names: &[&str]) -> Fixture {
    assert_eq!(
        names.first().copied(),
        Some(BOARD),
        "the board must be the session's first window, so it is the active one"
    );

    let dir = tempfile::tempdir().unwrap();
    let mut logs = HashMap::new();
    for (i, name) in names.iter().enumerate() {
        // Duplicate-name topologies are deliberate in one test, so index the
        // capture file rather than the name.
        logs.insert(format!("{i}:{name}"), dir.path().join(format!("{i}.log")));
    }

    let server = TmuxServer::start();
    for (i, name) in names.iter().enumerate() {
        let log = &logs[&format!("{i}:{name}")];
        let cmd = capture_cmd(log);
        if i == 0 {
            server.tmux_ok(&[
                "new-session",
                "-d",
                "-s",
                "t",
                "-n",
                name,
                "--",
                "sh",
                "-c",
                &cmd,
            ]);
        } else {
            // -d so the board stays active, as it is during a board-initiated
            // dispatch, kill or jump.
            server.tmux_ok(&["new-window", "-d", "-n", name, "--", "sh", "-c", &cmd]);
        }
    }

    Fixture {
        server,
        _dir: dir,
        logs,
    }
}

impl Fixture {
    fn runner(&self) -> SocketRunner {
        self.server.runner()
    }

    /// The capture file for the window created at position `index`.
    fn log(&self, index: usize, name: &str) -> &Path {
        &self.logs[&format!("{index}:{name}")]
    }

    /// Snapshot a capture file without waiting.
    fn read_now(&self, index: usize, name: &str) -> String {
        read_now(self.log(index, name))
    }

    /// Poll a capture file until it is non-empty or the deadline expires.
    fn read_when_written(&self, index: usize, name: &str) -> String {
        read_when_written(self.log(index, name))
    }

    /// Type `text` into the window at `index` through the production helper and
    /// block until it lands. Used as a **happens-before anchor**: once a later
    /// write to a window has demonstrably arrived, any earlier misrouted write to
    /// that same window has arrived too, so an absence can be asserted without
    /// sleeping out the full deadline.
    fn anchor_delivery(&self, index: usize, name: &str, text: &str) {
        tmux::send_keys(name, text, &self.runner()).expect("control send_keys should succeed");
        let got = self.read_when_written(index, name);
        assert!(
            got.contains(text),
            "control write to '{name}' never arrived, so absence assertions below \
             would be vacuous. got: {got:?}"
        );
    }

    fn window_names(&self) -> Vec<String> {
        self.server.window_names()
    }
}

// ---------------------------------------------------------------------------
// send_keys — the worst consequence: typing into another task's Claude session
// ---------------------------------------------------------------------------

#[test]
fn send_keys_to_absent_prefix_window_errors_and_types_nothing() {
    let Some(fx) = setup_or_skip(&[BOARD, "task-42"]) else {
        return;
    };

    let err = tmux::send_keys("task-4", "DANGER", &fx.runner())
        .expect_err("send_keys to an absent window must fail, not fall through to task-42");
    assert!(
        err.to_string().contains("task-4"),
        "error should name the window that was not found, got: {err}"
    );

    // Anchor, then assert the absence: had the prefix resolved, "DANGER" would
    // already be in task-42's capture file ahead of this control line.
    fx.anchor_delivery(1, "task-42", "CONTROL");
    let got = fx.read_now(1, "task-42");
    assert!(
        !got.contains("DANGER"),
        "task-42 runs another task's Claude session — a prefix-matched send-keys \
         types into it as user input. got: {got:?}"
    );
}

#[test]
fn send_keys_reaches_the_exactly_named_window_when_both_exist() {
    let Some(fx) = setup_or_skip(&[BOARD, "task-4", "task-42"]) else {
        return;
    };

    tmux::send_keys("task-4", "EXACT", &fx.runner()).expect("exact match must still work");

    let short = fx.read_when_written(1, "task-4");
    assert!(
        short.contains("EXACT"),
        "task-4 should receive its own keystrokes, got: {short:?}"
    );
    let long = fx.read_now(2, "task-42");
    assert!(
        long.trim().is_empty(),
        "task-42 must receive nothing, got: {long:?}"
    );
}

#[test]
fn send_keys_to_the_longer_name_is_unaffected_by_the_shorter_sibling() {
    let Some(fx) = setup_or_skip(&[BOARD, "task-4", "task-42"]) else {
        return;
    };

    tmux::send_keys("task-42", "LONG", &fx.runner()).expect("exact match must work");

    let long = fx.read_when_written(2, "task-42");
    assert!(long.contains("LONG"), "got: {long:?}");
    let short = fx.read_now(1, "task-4");
    assert!(
        short.trim().is_empty(),
        "task-4 must receive nothing, got: {short:?}"
    );
}

// ---------------------------------------------------------------------------
// kill_window — the other worst consequence: destroying another task's agent
// ---------------------------------------------------------------------------

#[test]
fn kill_window_on_absent_prefix_window_errors_and_spares_sibling() {
    let Some(fx) = setup_or_skip(&[BOARD, "keep-99"]) else {
        return;
    };

    let err = tmux::kill_window("keep-9", &fx.runner())
        .expect_err("kill_window on an absent window must fail, not kill keep-99");
    assert!(
        err.to_string().contains("keep-9"),
        "error should name the window that was not found, got: {err}"
    );

    assert!(
        fx.window_names().iter().any(|n| n == "keep-99"),
        "keep-99 belongs to a different task and must survive, windows: {:?}",
        fx.window_names()
    );
}

#[test]
fn kill_window_kills_only_the_exactly_named_window() {
    let Some(fx) = setup_or_skip(&[BOARD, "task-4", "task-42"]) else {
        return;
    };

    tmux::kill_window("task-4", &fx.runner()).expect("exact match must still work");

    let names = fx.window_names();
    assert!(
        !names.iter().any(|n| n == "task-4"),
        "task-4 should be gone, windows: {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "task-42"),
        "task-42 must survive, windows: {names:?}"
    );
}

// ---------------------------------------------------------------------------
// The remaining window-name targets
// ---------------------------------------------------------------------------

#[test]
fn select_window_on_absent_prefix_window_errors_and_keeps_focus() {
    let Some(fx) = setup_or_skip(&[BOARD, "task-42"]) else {
        return;
    };

    tmux::select_window("task-4", &fx.runner())
        .expect_err("select_window on an absent window must fail, not jump to task-42");

    assert_eq!(
        tmux::current_window_name(&fx.runner()).unwrap(),
        BOARD,
        "focus must not move to another task's window"
    );
}

/// `pane_id_for_window` is the silent-failure case: tmux's `=name` sigil makes
/// `display-message` print nothing and exit **zero**, so a wrong fix here yields
/// an empty pane ID that later targets whatever tmux's fallback picks.
#[test]
fn pane_id_for_window_on_absent_prefix_window_errors() {
    let Some(fx) = setup_or_skip(&[BOARD, "task-42"]) else {
        return;
    };

    let got = tmux::pane_id_for_window("task-4", &fx.runner());
    let err = got.expect_err("must fail rather than return task-42's pane ID");
    assert!(
        err.to_string().contains("task-4"),
        "error should name the window that was not found, got: {err}"
    );
}

#[test]
fn pane_id_for_window_returns_the_exactly_named_windows_pane() {
    let Some(fx) = setup_or_skip(&[BOARD, "task-4", "task-42"]) else {
        return;
    };

    let short = tmux::pane_id_for_window("task-4", &fx.runner()).unwrap();
    let long = tmux::pane_id_for_window("task-42", &fx.runner()).unwrap();

    assert!(short.starts_with('%'), "expected a pane ID, got: {short:?}");
    assert_ne!(
        short, long,
        "task-4 and task-42 are different windows and must resolve to different panes"
    );
    // And each ID really is the pane tmux itself calls that window's active one.
    assert_eq!(fx.server.active_pane_id("task-4"), Some(short));
    assert_eq!(fx.server.active_pane_id("task-42"), Some(long));
}

#[test]
fn inactive_pane_id_on_absent_prefix_window_errors() {
    let Some(fx) = setup_or_skip(&[BOARD, "task-42"]) else {
        return;
    };

    tmux::inactive_pane_id("task-4", &fx.runner())
        .expect_err("must fail rather than inspect task-42's panes");
}

#[test]
fn set_window_dispatch_dir_on_absent_prefix_window_errors_and_leaves_sibling_alone() {
    let Some(fx) = setup_or_skip(&[BOARD, "task-42"]) else {
        return;
    };

    tmux::set_window_dispatch_dir("task-4", "/tmp/wrong", &fx.runner())
        .expect_err("must fail rather than set @dispatch_dir on task-42");

    // A leaked @dispatch_dir on task-42 would make the split hook `cd` that
    // task's new panes into a different task's worktree.
    let leaked = fx.server.window_option("task-42", "@dispatch_dir");
    assert!(
        leaked.trim().is_empty(),
        "task-42 must not have acquired @dispatch_dir, got: {leaked:?}"
    );
}

#[test]
fn join_pane_on_absent_prefix_window_errors_and_spares_sibling() {
    let Some(fx) = setup_or_skip(&[BOARD, "task-42"]) else {
        return;
    };
    let board_pane = tmux::pane_id_for_window(BOARD, &fx.runner()).unwrap();

    tmux::join_pane("task-4", &board_pane, &fx.runner())
        .expect_err("must fail rather than pull task-42's pane into the board");

    assert!(
        fx.window_names().iter().any(|n| n == "task-42"),
        "task-42 must still be its own window, windows: {:?}",
        fx.window_names()
    );
}

#[test]
fn rename_window_on_absent_prefix_window_errors_and_spares_sibling() {
    let Some(fx) = setup_or_skip(&[BOARD, "task-42"]) else {
        return;
    };

    tmux::rename_window("task-4", "renamed", &fx.runner())
        .expect_err("must fail rather than rename task-42");

    let names = fx.window_names();
    assert!(
        names.iter().any(|n| n == "task-42") && !names.iter().any(|n| n == "renamed"),
        "task-42 must keep its name, windows: {names:?}"
    );
}

/// `rename_window` takes a *target* and a *new name* in adjacent arguments;
/// exact-match resolution must apply to the target only. Renaming `task-42` to
/// `task-4` is the shape that catches resolving the wrong argument: `task-4` does
/// not exist yet, so a resolver applied to the new name would fail.
#[test]
fn rename_window_resolves_the_target_not_the_new_name() {
    let Some(fx) = setup_or_skip(&[BOARD, "task-42"]) else {
        return;
    };

    tmux::rename_window("task-42", "task-4", &fx.runner()).expect("rename should succeed");

    let names = fx.window_names();
    assert!(
        names.iter().any(|n| n == "task-4") && !names.iter().any(|n| n == "task-42"),
        "task-42 should now be task-4, windows: {names:?}"
    );
}

/// `setup_tmux_for_tui` (src/runtime/mod.rs) renames by *pane ID*, precisely to
/// avoid an empty target resolving to whichever window is focused. Pane IDs are
/// already unambiguous and must pass through resolution untouched.
#[test]
fn rename_window_accepts_a_pane_id_target() {
    let Some(fx) = setup_or_skip(&[BOARD, "task-42"]) else {
        return;
    };
    let pane = tmux::pane_id_for_window("task-42", &fx.runner()).unwrap();

    tmux::rename_window(&pane, "renamed", &fx.runner()).expect("pane-ID target should work");

    let names = fx.window_names();
    assert!(
        names.iter().any(|n| n == "renamed") && !names.iter().any(|n| n == "task-42"),
        "the pane's window should have been renamed, windows: {names:?}"
    );
}

// ---------------------------------------------------------------------------
// Ambiguity
// ---------------------------------------------------------------------------

/// Two windows sharing a name is the other way a target can be wrong. tmux
/// already refuses this for `kill-window`/`select-window` ("can't find window")
/// but *silently picks one* for `set-option -w`, which is why
/// `set_window_dispatch_dir` grew a stderr sniff for "ambiguous". Resolution
/// makes the refusal uniform and the message the same everywhere.
#[test]
fn duplicate_window_names_are_refused_rather_than_resolved_arbitrarily() {
    let Some(fx) = setup_or_skip(&[BOARD, "dup", "dup"]) else {
        return;
    };

    let err = tmux::set_window_dispatch_dir("dup", "/tmp/whichever", &fx.runner())
        .expect_err("an ambiguous name must be refused");
    assert!(
        err.to_string().contains("multiple tmux windows"),
        "error should explain the ambiguity, got: {err}"
    );

    let err = tmux::kill_window("dup", &fx.runner()).expect_err("ambiguous kill must be refused");
    assert!(
        err.to_string().contains("multiple tmux windows"),
        "error should explain the ambiguity, got: {err}"
    );
    assert_eq!(
        fx.window_names().iter().filter(|n| *n == "dup").count(),
        2,
        "neither duplicate should have been killed"
    );
}
