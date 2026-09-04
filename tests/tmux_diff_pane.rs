#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Real-tmux integration tests for the diff pane the agent-tree companion pane
//! opens (docs/specs/agent-tree.allium: `SplitAgentTreeDiffPane`,
//! `CloseAgentTreeDiffPaneWhenEmpty`, `KillAgentTreeDiffPaneWithItsTree`).
//!
//! What only a real server can show: how many panes a window ends up with, how
//! WIDE the new pane is (the one geometric difference from the editor pane this
//! replaced), which pane keeps focus, which cwd the process resolved, and which
//! panes the agent-tree toggle kills. `MockProcessRunner` can only pin the
//! command strings — see tests/tmux_harness/mod.rs and the mock-level tests in
//! `src/agent_tree_diff_pane.rs`.

mod tmux_harness;

use std::path::Path;

use dispatch_tui::agent_tree_diff_pane::reconcile_diff_pane;
use dispatch_tui::dispatch;
use dispatch_tui::models::test_tmux_window;
use dispatch_tui::tmux::{PANE_ROLE_AGENT_TREE, PANE_ROLE_DIFF, PANE_ROLE_OPTION};

use tmux_harness::{await_stub_line, stub_lines, tmux_available_or_skip, StubLine, TmuxServer};

const WINDOW: &str = "task-42";
const TASK_ID: i64 = 42;

/// The pane runs `dispatch agent-diff`, and the harness already stubs the
/// `dispatch` binary — so the process that starts records its own cwd and argv
/// and then holds its pane open, exactly as a real renderer would.
const DISPATCH_STUB: &str = "dispatch";

/// An agent window shaped like a live one: the agent's own pane (active) plus a
/// companion tree pane, and a worktree on disk.
struct Fixture {
    /// Declared before `dir`: fields drop in declaration order, so the server is
    /// killed (ending the stub processes holding the log open) before the
    /// temporary directory is unlinked.
    server: TmuxServer,
    dir: tempfile::TempDir,
    tree_pane: String,
    agent_pane: String,
}

fn setup_or_skip() -> Option<Fixture> {
    if !tmux_available_or_skip() {
        return None;
    }
    let server = TmuxServer::start();
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");

    let root = dir.path().to_string_lossy().into_owned();
    // Wide enough that the tree's 30% column is still several columns across
    // after the diff pane subdivides it, so the width assertions below are
    // measuring geometry rather than a tmux minimum.
    server.tmux_ok(&[
        "new-session",
        "-d",
        "-s",
        "t",
        "-n",
        WINDOW,
        "-c",
        &root,
        "-x",
        "200",
        "-y",
        "50",
    ]);
    let agent_pane = server.active_pane_id(WINDOW).expect("agent pane");

    // The companion pane through *production's own* spawn path, rather than a
    // hand-rolled split: the role marker every lookup here depends on is written
    // by that path, so a fixture that split and marked the pane itself would
    // assert against its own setup instead of against production.
    dispatch::toggle_agent_tree_pane(&test_tmux_window(WINDOW), &server.runner())
        .expect("split companion pane");
    let tree_pane = server
        .pane_ids(WINDOW)
        .into_iter()
        .find(|id| *id != agent_pane)
        .expect("companion pane should exist after the toggle");

    Some(Fixture {
        server,
        dir,
        tree_pane,
        agent_pane,
    })
}

impl Fixture {
    /// Reconcile the panes against `anything_open`, the way the tree does after
    /// every change to its open set.
    fn reconcile(&self, anything_open: bool) {
        reconcile_diff_pane(
            &self.tree_pane,
            Path::new("/data/tasks.db"),
            TASK_ID,
            self.dir.path(),
            anything_open,
            &self.server.runner(),
        )
        .expect("reconcile the diff pane");
    }

    fn diff_pane(&self) -> Option<String> {
        self.server
            .pane_ids(WINDOW)
            .into_iter()
            .find(|id| self.server.pane_option(id, PANE_ROLE_OPTION) == PANE_ROLE_DIFF)
    }

    /// Wait for the diff renderer to report having started. The pane starts
    /// asynchronously relative to `reconcile_diff_pane` returning, so this polls
    /// (deadline-bounded, never a fixed sleep).
    fn await_started(&self) -> StubLine {
        await_stub_line(&self.server, |line| {
            line.name == DISPATCH_STUB && line.args.contains("agent-diff")
        })
        .unwrap_or_else(|| {
            panic!(
                "the diff renderer never started; recorded: {:?}",
                stub_lines(&self.server)
            )
        })
    }

    fn pane_dimension(&self, target: &str, format: &str) -> u32 {
        self.server
            .tmux_stdout(&["display-message", "-p", "-t", target, format])
            .parse()
            .expect("dimension")
    }
}

/// The marker a real tmux server actually holds after production split the pane.
/// A mock can only show that `set-option` was *sent*; that the option is readable
/// back off the pane, by a later process, is what every lookup here depends on.
#[test]
fn the_companion_pane_carries_the_agent_tree_role() {
    let Some(fx) = setup_or_skip() else { return };

    assert_eq!(
        fx.server.pane_option(&fx.tree_pane, PANE_ROLE_OPTION),
        PANE_ROLE_AGENT_TREE,
    );
    assert_eq!(
        fx.server.pane_option(&fx.agent_pane, PANE_ROLE_OPTION),
        "",
        "the agent's own pane is not dispatch-created and must carry no role"
    );
}

#[test]
fn opening_the_first_diff_adds_one_marked_pane() {
    let Some(fx) = setup_or_skip() else { return };

    fx.reconcile(true);

    assert_eq!(
        fx.server.pane_count(WINDOW),
        3,
        "agent + tree + diff; panes: {:?}",
        fx.server.pane_ids(WINDOW)
    );
    assert!(
        fx.diff_pane().is_some(),
        "the new pane must carry {PANE_ROLE_OPTION} = {PANE_ROLE_DIFF}"
    );
}

/// `-d`: the user keeps browsing the tree after opening a diff. Reaching the
/// diff to scroll it is a deliberate second act, and tmux's own navigation.
#[test]
fn opening_a_diff_does_not_move_focus() {
    let Some(fx) = setup_or_skip() else { return };
    fx.server.tmux_ok(&["select-pane", "-t", &fx.tree_pane]);

    fx.reconcile(true);

    assert_eq!(
        fx.server.active_pane_id(WINDOW).as_deref(),
        Some(fx.tree_pane.as_str()),
        "focus must stay in the tree pane"
    );
}

/// The one geometric difference from the editor pane this replaced, and the
/// reason `-f` was dropped: the diff subdivides the tree's own narrow column, so
/// the agent's pane keeps every column it had. With `-f` the new pane would span
/// the window and take the agent's room instead.
#[test]
fn the_diff_pane_stays_inside_the_trees_column() {
    let Some(fx) = setup_or_skip() else { return };
    let agent_width_before = fx.pane_dimension(&fx.agent_pane, "#{pane_width}");

    fx.reconcile(true);

    let diff = fx.diff_pane().expect("diff pane");
    let window_width = fx.pane_dimension(WINDOW, "#{window_width}");
    assert!(
        fx.pane_dimension(&diff, "#{pane_width}") < window_width,
        "the diff pane must not span the window"
    );
    assert_eq!(
        fx.pane_dimension(&diff, "#{pane_width}"),
        fx.pane_dimension(&fx.tree_pane, "#{pane_width}"),
        "it must be exactly as wide as the tree it subdivides"
    );
    assert_eq!(
        fx.pane_dimension(&fx.agent_pane, "#{pane_width}"),
        agent_width_before,
        "the agent's own pane must be untouched by opening a diff"
    );
}

/// It takes the larger share of that column: the tree is a list of short path
/// segments, and reading the change is the point of opening it.
#[test]
fn the_diff_pane_takes_the_larger_share_of_the_column() {
    let Some(fx) = setup_or_skip() else { return };

    fx.reconcile(true);

    let diff = fx.diff_pane().expect("diff pane");
    assert!(
        fx.pane_dimension(&diff, "#{pane_height}")
            > fx.pane_dimension(&fx.tree_pane, "#{pane_height}"),
        "the diff must be taller than the tree above it"
    );
}

/// #231's failure mode, for this pane: `-c` is passed explicitly because the
/// `@dispatch_dir` split hook would *type* `cd <dir>` into the renderer. The
/// stub reports its own `$PWD` and argv, so this observes what the process
/// actually got — not merely what tmux was asked for.
#[test]
fn the_diff_renderer_runs_in_the_worktree_and_is_told_its_task() {
    let Some(fx) = setup_or_skip() else { return };

    fx.reconcile(true);

    let line = fx.await_started();
    let want = std::fs::canonicalize(fx.dir.path()).expect("canonicalize");
    let got = std::fs::canonicalize(&line.cwd).expect("canonicalize");
    assert_eq!(got, want, "line: {line:?}");
    assert!(
        line.args.contains("agent-diff 42"),
        "the renderer must be told which task to read; line: {line:?}"
    );
    assert!(
        line.args.contains("--db /data/tasks.db"),
        "and which database, so it reads the same open set; line: {line:?}"
    );
}

/// Splitting again on every toggle would subdivide the column until the window
/// ran out of room.
#[test]
fn opening_a_second_diff_reuses_the_pane_that_is_already_there() {
    let Some(fx) = setup_or_skip() else { return };
    fx.reconcile(true);
    let first = fx.diff_pane().expect("diff pane");

    fx.reconcile(true);

    assert_eq!(
        fx.server.pane_count(WINDOW),
        3,
        "a second open must not add a pane; panes: {:?}",
        fx.server.pane_ids(WINDOW)
    );
    assert_eq!(fx.diff_pane().as_deref(), Some(first.as_str()));
}

/// The pane's presence IS the answer to "is anything open", so closing the last
/// diff has to take it away — otherwise the user closes it a second time, by a
/// second mechanism, to get their rows back.
#[test]
fn closing_the_last_diff_removes_the_pane() {
    let Some(fx) = setup_or_skip() else { return };
    fx.reconcile(true);
    let diff = fx.diff_pane().expect("diff pane");

    fx.reconcile(false);

    assert!(!fx.server.pane_exists(&diff), "the diff pane must be gone");
    assert!(
        fx.server.pane_exists(&fx.tree_pane),
        "and it must not take the tree with it"
    );
    assert_eq!(fx.server.pane_count(WINDOW), 2);
}

/// The regression the role marker exists for: with focus in the tree pane, the
/// old single-inactive-pane lookup identified the *agent's* pane as the
/// companion and killed the user's claude session.
#[test]
fn the_toggle_kills_the_tree_pane_not_the_agents_when_the_tree_has_focus() {
    let Some(fx) = setup_or_skip() else { return };
    fx.server.tmux_ok(&["select-pane", "-t", &fx.tree_pane]);

    dispatch::toggle_agent_tree_pane(&test_tmux_window(WINDOW), &fx.server.runner())
        .expect("toggle");

    assert!(
        !fx.server.pane_exists(&fx.tree_pane),
        "the tree pane must be gone"
    );
    assert!(
        fx.server.pane_exists(&fx.agent_pane),
        "the agent's own pane must survive"
    );
}

/// Hiding the tree takes the diff pane with it. A diff pane outliving its tree
/// is orphaned: nothing drives its open set, nothing refreshes it, and the
/// toggle that would bring the tree back does not act on it.
#[test]
fn the_toggle_takes_the_diff_pane_with_the_tree() {
    let Some(fx) = setup_or_skip() else { return };
    fx.reconcile(true);
    let diff = fx.diff_pane().expect("diff pane");

    dispatch::toggle_agent_tree_pane(&test_tmux_window(WINDOW), &fx.server.runner())
        .expect("toggle");

    assert!(!fx.server.pane_exists(&fx.tree_pane), "tree pane must go");
    assert!(!fx.server.pane_exists(&diff), "diff pane must go with it");
    assert!(
        fx.server.pane_exists(&fx.agent_pane),
        "agent pane must stay"
    );
}

/// Pinning moves only the agent's own pane into the board window; every pane
/// dispatch added must be cleaned up, or it is orphaned in a window nothing owns.
#[test]
fn pinning_drains_both_the_tree_and_the_diff_pane() {
    let Some(fx) = setup_or_skip() else { return };
    fx.reconcile(true);
    let diff = fx.diff_pane().expect("diff pane");
    fx.server.tmux_ok(&["new-window", "-d", "-n", "board"]);
    let board_pane = fx.server.active_pane_id("board").expect("board pane");

    dispatch::join_task_window_into_pane(
        &test_tmux_window(WINDOW),
        &board_pane,
        &fx.server.runner(),
    )
    .expect("pin");

    assert!(
        !fx.server.pane_exists(&fx.tree_pane),
        "the tree pane must not be orphaned"
    );
    assert!(
        !fx.server.pane_exists(&diff),
        "the diff pane must not be orphaned"
    );
}
