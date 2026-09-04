#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Real-tmux integration test for the two-tier mechanism that keeps new panes in
//! their agent's worktree: `split-window -c` for dispatch's own splits, and the
//! `after-split-window` correction hook for splits dispatch did not make
//! (`ensure_split_hook` / `set_window_dispatch_dir`).
//!
//! Every pane here runs a marked `cat >> log`, so this file observes three
//! things a mock cannot: the **directory** a pane resolves, whether a pane's
//! process was **restarted**, and whether anything was **typed** into it. The
//! last of those is a standing requirement, not a detail —
//! split-pane.allium's `SplitDirectoryIsNeverKeystrokes` invariant exists
//! because the previous mechanism typed `cd <worktree>` at whatever occupied the
//! pane, and the agent-tree companion pane exits on `q`.
//!
//! See `ensure_split_hook` in src/tmux.rs for why the hook needs an explicit
//! `-t #{pane_id}` target, and why the mechanism exists at all. Task #3781's
//! defect was purely a matter of tmux's own targeting semantics, and its mock
//! test asserted the broken hook string verbatim and stayed green throughout.
//!
//! Its sibling `tests/tmux_lifecycle.rs` covers the complementary question,
//! *topology*, on windows production creates. Why a real server is needed at all,
//! plus the shared rig: tests/tmux_harness/mod.rs.

mod tmux_harness;

use std::path::{Path, PathBuf};

use dispatch_tui::models::test_tmux_window as win;
use dispatch_tui::tmux;

use tmux_harness::{
    canonical, capture_cmd_marked, poll_until, start_count, tmux_available_or_skip, typed_input,
    TmuxServer,
};

/// The agent window under test, named the way dispatch names them (`task-<id>`).
const AGENT_WINDOW: &str = "task-42";
/// The board TUI window — the pane that must never be disturbed by the hook.
const BOARD_WINDOW: &str = "board";
/// Companion-pane width. Production's `AGENT_TREE_PANE_PERCENT` is private to
/// `src/dispatch/agents.rs`; the exact percentage is irrelevant to what these
/// tests assert.
const PANE_PERCENT: u8 = 30;

struct Fixture {
    // Declared before `dir` on purpose: fields drop in declaration order, so the
    // server (and its `cat` processes) dies before the temp dir holding their
    // capture files is unlinked.
    server: TmuxServer,
    /// Root for the worktree and capture files. Held for the fixture's lifetime
    /// because the pane processes write into it.
    dir: tempfile::TempDir,
    board_log: PathBuf,
    agent_log: PathBuf,
    worktree: PathBuf,
}

/// Build the fixture, or skip when tmux is unavailable. Folding the guard into
/// construction means a later test cannot forget it and hard-fail on a developer
/// machine without tmux. In CI the guard fails instead of skipping — see
/// `tmux_available_or_skip`.
fn setup_or_skip() -> Option<Fixture> {
    setup_named_or_skip("42-some-task")
}

/// [`setup_or_skip`] with the worktree directory's name under the test's control,
/// so a test can pin how the mechanism handles an awkward path.
fn setup_named_or_skip(worktree_name: &str) -> Option<Fixture> {
    if !tmux_available_or_skip() {
        return None;
    }
    Some(setup(worktree_name))
}

/// Build the topology dispatch actually produces: a focused board window plus a
/// background agent window carrying `@dispatch_dir`, with the production hook
/// installed. Mirrors `resume_agent` / `dispatch_with_prompt` in
/// src/dispatch/agents.rs, which call `set_window_dispatch_dir` then
/// `ensure_split_hook` before splitting the agent window for the companion pane.
fn setup(worktree_name: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let worktree = dir.path().join("worktrees").join(worktree_name);
    std::fs::create_dir_all(&worktree).unwrap();

    let board_log = dir.path().join("board.log");
    let agent_log = dir.path().join("agent.log");

    let server = TmuxServer::start();

    // Window 1: the board TUI. Created first, so it is the active window —
    // exactly the state during a dispatch or resume triggered from the board.
    server.tmux_ok(&[
        "new-session",
        "-d",
        "-s",
        "t",
        "-n",
        BOARD_WINDOW,
        "--",
        "sh",
        "-c",
        &capture_cmd_marked(&board_log),
    ]);

    // Window 2: the agent window, in the background.
    server.tmux_ok(&[
        "new-window",
        "-d",
        "-n",
        AGENT_WINDOW,
        "--",
        "sh",
        "-c",
        &capture_cmd_marked(&agent_log),
    ]);

    let runner = server.runner();
    tmux::set_window_dispatch_dir(&win(AGENT_WINDOW), worktree.to_str().unwrap(), &runner).unwrap();
    tmux::ensure_split_hook(&runner).unwrap();

    // Both panes must be up before any test asserts on what did *not* reach
    // them, or "the board received nothing" would pass against a pane that had
    // not started yet.
    await_started(&board_log);
    await_started(&agent_log);

    Fixture {
        server,
        dir,
        board_log,
        agent_log,
        worktree,
    }
}

/// Block until the pane writing to `log` has recorded its start marker.
///
/// A pane's id is returned by `split-window` before its process has run, so
/// every assertion about a pane's log — including the negative ones — needs this
/// first. Without it a test asserting "nothing was typed here" passes against a
/// pane that has not opened the file yet.
fn await_started(log: &Path) {
    assert!(
        poll_until(|| start_count(log) >= 1),
        "pane never started; expected a start marker in {}",
        log.display()
    );
}

impl Fixture {
    fn log(&self, name: &str) -> PathBuf {
        self.dir.path().join(format!("{name}.log"))
    }

    /// Split `window` the way dispatch does — through the production helper,
    /// naming the worktree as the new pane's start directory. Returns the new
    /// pane's id.
    fn dispatch_split(&self, window: &str, log: &Path) -> String {
        tmux::split_window_horizontal_running(
            window,
            PANE_PERCENT,
            &["sh", "-c", &capture_cmd_marked(log)],
            Some(self.worktree.to_str().unwrap()),
            &self.server.runner(),
        )
        .expect("split-window")
    }

    /// Split `window` the way a *user* does — a plain `split-window` with no
    /// start directory, so tmux picks the cwd itself and the new pane lands
    /// outside the worktree. This is the case the correction hook exists for.
    ///
    /// A raw tmux call rather than a nested client pressing a real key binding:
    /// what the hook keys off is `#{pane_start_path}`, and every split that
    /// names no `-c` produces the same one, whoever asked for it.
    fn user_split(&self, window: &str, log: &Path) -> String {
        self.server.tmux_stdout(&[
            "split-window",
            "-d",
            "-t",
            window,
            "-P",
            "-F",
            "#{pane_id}",
            "--",
            "sh",
            "-c",
            &capture_cmd_marked(log),
        ])
    }

    /// Assert that `pane` ends up in the worktree, waiting for it to get there.
    /// The hook fires through `run-shell -b`, so the correction is not visible
    /// when the split call returns.
    ///
    /// Doubles as the happens-before anchor every negative assertion in this file
    /// needs: once a correction has demonstrably landed, anything the hook
    /// misapplied has already happened too.
    fn await_corrected(&self, pane: &str) {
        let want = canonical(self.worktree.to_str().unwrap());
        assert!(
            poll_until(|| canonical(&self.server.pane_cwd(pane)) == want),
            "pane {pane} never reached the worktree; cwd is {:?}, want {want:?}",
            self.server.pane_cwd(pane)
        );
    }

    /// Remove the correction hook, so a test can observe what a split produced
    /// *before* tier 2 has a chance to change it.
    ///
    /// Without this, a test of tier 1 is not sensitive to tier 1: drop the
    /// `-c` and the hook quietly moves the pane to the same place, a moment
    /// later. Measured — the assertions below passed against a
    /// `split_window_horizontal_running` that had stopped sending `-c` at all.
    fn disable_correction_hook(&self) {
        self.server
            .tmux_ok(&["set-hook", "-u", "after-split-window"]);
    }

    /// Evaluate the production correction guard against `pane`, as tmux itself
    /// does when the hook fires. `"1"` means "this pane needs correcting".
    fn guard(&self, pane: &str) -> String {
        self.server.tmux_stdout(&[
            "display-message",
            "-p",
            "-t",
            pane,
            tmux::SPLIT_NEEDS_CORRECTION,
        ])
    }

    fn assert_cwd_is_worktree(&self, pane: &str) {
        assert_eq!(
            canonical(&self.server.pane_cwd(pane)),
            canonical(self.worktree.to_str().unwrap()),
            "pane {pane} should sit in the task's worktree"
        );
    }
}

/// Tier 2: a split dispatch did not make lands wherever tmux chose, and the hook
/// corrects it into the worktree.
#[test]
fn user_split_without_a_start_dir_is_corrected_into_the_worktree() {
    let Some(fx) = setup_or_skip() else { return };
    let log = fx.log("user_pane");

    let pane = fx.user_split(AGENT_WINDOW, &log);

    fx.await_corrected(&pane);
}

/// Tier 1: dispatch's own split is correct at creation, with the correction hook
/// taken out of the picture so this can only pass on the strength of `-c`.
#[test]
fn dispatch_split_starts_in_the_worktree_without_any_correction() {
    let Some(fx) = setup_or_skip() else { return };
    fx.disable_correction_hook();
    let dispatch_log = fx.log("dispatch_pane");

    let dispatch_pane = fx.dispatch_split(AGENT_WINDOW, &dispatch_log);
    await_started(&dispatch_log);

    fx.assert_cwd_is_worktree(&dispatch_pane);
}

/// …and the hook must therefore skip it: `respawn-pane` would restart the
/// `dispatch agent-tree` process the pane was created to run.
///
/// Asserted by evaluating the production guard itself against the pane, rather
/// than by watching for a restart. A restart is *not* reliably observable from
/// inside the pane: `respawn-pane -k` can kill the first process before it writes
/// anything, leaving a log indistinguishable from one that was never respawned.
/// Measured — an earlier version of this test counted start markers and passed
/// against a hook whose guard had been deleted.
///
/// This is the negative direction only. The positive one — that a pane tmux
/// placed outside the worktree *is* selected and corrected — is
/// `user_split_without_a_start_dir_is_corrected_into_the_worktree`, so a guard
/// stuck at false cannot satisfy both.
#[test]
fn correction_guard_skips_a_pane_that_started_in_the_worktree() {
    let Some(fx) = setup_or_skip() else { return };
    // The guard reads `#{pane_start_path}`, which a correction rewrites — so with
    // the hook live this would race its own subject.
    fx.disable_correction_hook();

    let dispatch_pane = fx.dispatch_split(AGENT_WINDOW, &fx.log("dispatch_pane"));

    assert_eq!(
        fx.guard(&dispatch_pane),
        "0",
        "a pane created with -c already sits in the worktree; correcting it \
         would restart the companion process it is running"
    );
}

/// Correcting a pane clears the condition that selected it, so the mechanism
/// converges instead of chasing the same pane. `respawn-pane` rewrites
/// `#{pane_start_path}`, which is the value the guard reads.
#[test]
fn a_corrected_pane_is_no_longer_selected_for_correction() {
    let Some(fx) = setup_or_skip() else { return };

    let pane = fx.user_split(AGENT_WINDOW, &fx.log("user_pane"));
    fx.await_corrected(&pane);

    assert_eq!(
        fx.guard(&pane),
        "0",
        "a pane already moved into the worktree must not be selected again"
    );
}

/// The invariant the previous mechanism violated: nothing is ever typed at a
/// pane to place it in its worktree — not the board, not the agent's own Claude
/// pane, and not the new pane either, whose occupant may read `q` as "quit".
#[test]
fn split_correction_never_types_into_any_pane() {
    let Some(fx) = setup_or_skip() else { return };
    let dispatch_log = fx.log("dispatch_pane");
    let user_log = fx.log("user_pane");

    fx.dispatch_split(AGENT_WINDOW, &dispatch_log);
    let user_pane = fx.user_split(AGENT_WINDOW, &user_log);
    fx.await_corrected(&user_pane);
    // Both panes up, so each log below is one a keystroke could have reached.
    await_started(&dispatch_log);
    await_started(&user_log);

    for (what, log) in [
        ("the board TUI pane", &fx.board_log),
        ("the agent's own Claude pane", &fx.agent_log),
        ("dispatch's own companion pane", &dispatch_log),
        ("the newly split pane", &user_log),
    ] {
        assert_eq!(
            typed_input(log),
            "",
            "{what} received synthesised keystrokes; the worktree must be set \
             through tmux, never typed"
        );
    }
}

/// A window without `@dispatch_dir` (the board, an editor window) must not
/// trigger the hook at all — the `if-shell -F` guard covers this, and it is the
/// property that keeps non-agent windows unaffected.
#[test]
fn split_hook_is_inert_for_windows_without_a_dispatch_dir() {
    let Some(fx) = setup_or_skip() else { return };

    // Split the *board* window, which carries no @dispatch_dir.
    let plain_log = fx.log("plain_pane");
    let plain_pane = fx.user_split(BOARD_WINDOW, &plain_log);
    await_started(&plain_log);

    // Anchor on a split that *does* fire the hook, so this is not merely a race
    // that happens to observe "nothing yet".
    let anchor_log = fx.log("anchor_pane");
    let anchor = fx.user_split(AGENT_WINDOW, &anchor_log);
    fx.await_corrected(&anchor);

    // Sound without watching for a restart: a respawn would have moved this
    // pane into the worktree, so its absence there is the observation.
    assert_ne!(
        canonical(&fx.server.pane_cwd(&plain_pane)),
        canonical(fx.worktree.to_str().unwrap()),
        "a window without @dispatch_dir must not be pulled into some other \
         task's worktree"
    );
    assert_eq!(
        fx.guard(&plain_pane),
        "0",
        "a window without @dispatch_dir must not be selected for correction"
    );
}

/// `window_dispatch_dir` is how the toggle and resync paths recover a start
/// directory from a bare window name. It resolves the name through
/// `window_target` and asks `show-options` about the resulting *pane* id — a
/// tmux targeting question a mock cannot answer, and the same shape that made
/// `set-option -w` need `window_target` in the first place.
#[test]
fn window_dispatch_dir_reads_back_what_was_written() {
    let Some(fx) = setup_or_skip() else { return };
    let runner = fx.server.runner();

    assert_eq!(
        tmux::window_dispatch_dir(&win(AGENT_WINDOW), &runner).unwrap(),
        Some(fx.worktree.to_str().unwrap().to_string()),
    );
    assert_eq!(
        tmux::window_dispatch_dir(&win(BOARD_WINDOW), &runner).unwrap(),
        None,
        "a window that was never given a worktree must report none"
    );
}

/// The hook interpolates `@dispatch_dir` into a nested tmux command string. Drop
/// the innermost quoting and a worktree path containing a space resolves to
/// `$HOME` instead — silently, since `respawn-pane` still succeeds.
#[test]
fn split_correction_handles_a_worktree_path_containing_a_space() {
    let Some(fx) = setup_named_or_skip("42-some task") else {
        return;
    };
    let log = fx.log("user_pane");

    let pane = fx.user_split(AGENT_WINDOW, &log);

    fx.await_corrected(&pane);
}
