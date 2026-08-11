#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Real-tmux integration tests for the editor pane the agent-tree companion pane
//! opens (docs/specs/agent-tree.allium: `OpenAgentTreeFileInEditor`,
//! `ReplaceAgentTreeEditorFile`).
//!
//! What only a real server can show: how many panes a window ends up with after
//! two opens, which pane keeps focus, which cwd the editor process resolved, and
//! which pane the agent-tree toggle kills. `MockProcessRunner` can only pin the
//! command strings — see tests/tmux_harness/mod.rs and the mock-level tests in
//! `src/agent_tree_editor.rs`.

mod tmux_harness;

use std::path::PathBuf;

use dispatch_tui::agent_tree_editor::{open_in_editor, EDITOR_PANE_OPTION};
use dispatch_tui::dispatch;
use dispatch_tui::process::ProcessRunner;
use dispatch_tui::tmux;

use tmux_harness::{await_stub_line, stub_lines, tmux_available_or_skip, StubLine, TmuxServer};

const WINDOW: &str = "task-42";
/// The stand-in for `$EDITOR`. A harness stub, so it records its own cwd and argv
/// and then holds its pane open — a real editor would want a tty, and a command
/// that exits would close the pane mid-assertion.
const EDITOR_STUB: &str = "fake-editor";

/// An agent window shaped like a live one: the agent's own pane (active) plus a
/// companion tree pane started with the stub `dispatch agent-tree 42`, and a
/// worktree on disk holding files to open.
struct Fixture {
    /// Declared before `dir`: fields drop in declaration order, so the server is
    /// killed (ending the stub processes holding the log open) before the
    /// temporary directory is unlinked.
    server: TmuxServer,
    dir: tempfile::TempDir,
    editor_bin: String,
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
    std::fs::write(dir.path().join("src/lib.rs"), "fn main() {}").expect("write lib.rs");
    std::fs::write(dir.path().join("README.md"), "# hi").expect("write README");

    let root = dir.path().to_string_lossy().into_owned();
    server.tmux_ok(&["new-session", "-d", "-s", "t", "-n", WINDOW, "-c", &root]);
    let agent_pane = server.active_pane_id(WINDOW).expect("agent pane");

    // The companion pane exactly as production spawns it: the stub dispatch
    // binary, `agent-tree`, the task id — so the start-command lookup under test
    // sees a production-shaped command line.
    let dispatch_bin = server.runner().agent_binaries().dispatch;
    let tree_pane = tmux::split_window_horizontal_running(
        WINDOW,
        30,
        &[&dispatch_bin, "agent-tree", "42"],
        &server.runner(),
    )
    .expect("split companion pane");

    let editor_bin = server.extra_stub(EDITOR_STUB);

    Some(Fixture {
        server,
        dir,
        editor_bin,
        tree_pane,
        agent_pane,
    })
}

impl Fixture {
    fn open(&self, relative: &str) {
        open_in_editor(
            self.dir.path(),
            &PathBuf::from(relative),
            &self.tree_pane,
            &[self.editor_bin.clone()],
            &self.server.runner(),
        )
        .expect("open in editor");
    }

    fn editor_pane(&self) -> Option<String> {
        self.server
            .pane_ids(WINDOW)
            .into_iter()
            .find(|id| self.server.pane_option(id, EDITOR_PANE_OPTION) == "1")
    }

    /// Wait for the editor stub to report having been handed `relative`. The pane
    /// starts asynchronously relative to `open_in_editor` returning, so this
    /// polls (deadline-bounded, never a fixed sleep).
    fn await_opened(&self, relative: &str) -> StubLine {
        await_stub_line(&self.server, |line| {
            line.name == EDITOR_STUB && line.args.contains(relative)
        })
        .unwrap_or_else(|| {
            panic!(
                "editor stub was never handed {relative}; recorded: {:?}",
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

#[test]
fn opening_a_file_adds_one_marked_pane() {
    let Some(fx) = setup_or_skip() else { return };

    fx.open("src/lib.rs");

    assert_eq!(
        fx.server.pane_count(WINDOW),
        3,
        "agent + tree + editor; panes: {:?}",
        fx.server.pane_ids(WINDOW)
    );
    assert!(
        fx.editor_pane().is_some(),
        "the new pane must carry {EDITOR_PANE_OPTION}"
    );
}

/// The point of `-d`: the user keeps browsing the tree after opening a file.
#[test]
fn opening_a_file_does_not_move_focus() {
    let Some(fx) = setup_or_skip() else { return };
    fx.server.tmux_ok(&["select-pane", "-t", &fx.tree_pane]);

    fx.open("src/lib.rs");

    assert_eq!(
        fx.server.active_pane_id(WINDOW).as_deref(),
        Some(fx.tree_pane.as_str()),
        "focus must stay in the tree pane"
    );
}

/// The point of `-f`: the editor pane spans the window rather than subdividing
/// the narrow tree pane it was split from.
#[test]
fn the_editor_pane_spans_the_window_width() {
    let Some(fx) = setup_or_skip() else { return };

    fx.open("src/lib.rs");

    let editor = fx.editor_pane().expect("editor pane");
    let window_width = fx.pane_dimension(WINDOW, "#{window_width}");
    assert_eq!(
        fx.pane_dimension(&editor, "#{pane_width}"),
        window_width,
        "the editor pane must span the whole window"
    );
    assert!(
        fx.pane_dimension(&editor, "#{pane_width}")
            > fx.pane_dimension(&fx.tree_pane, "#{pane_width}"),
        "and so be wider than the tree it was split from"
    );
}

/// #231's failure mode, for this pane: `-c` is passed explicitly because the
/// `@dispatch_dir` split hook would *type* `cd <dir>` into the editor.
#[test]
fn the_editor_runs_in_the_worktree_with_the_absolute_path() {
    let Some(fx) = setup_or_skip() else { return };

    fx.open("src/lib.rs");

    // The stub reports its own `$PWD` and argv, so this observes what the editor
    // process actually got — not merely what tmux was asked for.
    let line = fx.await_opened("src/lib.rs");
    let want = std::fs::canonicalize(fx.dir.path()).expect("canonicalize");
    let got = std::fs::canonicalize(&line.cwd).expect("canonicalize");
    assert_eq!(got, want, "line: {line:?}");
    assert!(
        line.args.starts_with('/'),
        "the editor must be handed an absolute path; line: {line:?}"
    );
}

#[test]
fn a_second_open_reuses_the_same_pane() {
    let Some(fx) = setup_or_skip() else { return };

    fx.open("src/lib.rs");
    fx.await_opened("src/lib.rs");
    let first = fx.editor_pane().expect("editor pane");

    fx.open("README.md");

    // The second file reaching the stub is what proves the respawn ran the editor
    // again rather than leaving the first file on screen.
    fx.await_opened("README.md");
    assert_eq!(
        fx.server.pane_count(WINDOW),
        3,
        "a second open must not add a pane; panes: {:?}",
        fx.server.pane_ids(WINDOW)
    );
    assert_eq!(
        fx.editor_pane().as_deref(),
        Some(first.as_str()),
        "respawn preserves the pane and its marker"
    );
}

/// The regression: with focus in the tree pane, the old single-inactive-pane
/// lookup identified the *agent's* pane as the companion and killed the user's
/// claude session.
#[test]
fn the_toggle_kills_the_tree_pane_not_the_agents_when_the_tree_has_focus() {
    let Some(fx) = setup_or_skip() else { return };
    fx.server.tmux_ok(&["select-pane", "-t", &fx.tree_pane]);

    dispatch::toggle_agent_tree_pane(WINDOW, &fx.server.runner()).expect("toggle");

    assert!(
        !fx.server.pane_exists(&fx.tree_pane),
        "the tree pane must be gone"
    );
    assert!(
        fx.server.pane_exists(&fx.agent_pane),
        "the agent's own pane must survive"
    );
}

#[test]
fn the_toggle_kills_the_tree_pane_with_an_editor_pane_open() {
    let Some(fx) = setup_or_skip() else { return };
    fx.open("src/lib.rs");
    let editor = fx.editor_pane().expect("editor pane");

    dispatch::toggle_agent_tree_pane(WINDOW, &fx.server.runner()).expect("toggle");

    assert!(!fx.server.pane_exists(&fx.tree_pane), "tree pane must go");
    assert!(fx.server.pane_exists(&editor), "editor pane must stay");
    assert!(
        fx.server.pane_exists(&fx.agent_pane),
        "agent pane must stay"
    );
}

/// Pinning moves only the agent's own pane into the board window; every pane
/// dispatch added must be cleaned up, or it is orphaned in a window nothing owns.
#[test]
fn pinning_drains_both_the_tree_and_the_editor_pane() {
    let Some(fx) = setup_or_skip() else { return };
    fx.open("src/lib.rs");
    let editor = fx.editor_pane().expect("editor pane");
    fx.server.tmux_ok(&["new-window", "-d", "-n", "board"]);
    let board_pane = fx.server.active_pane_id("board").expect("board pane");

    dispatch::join_task_window_into_pane(WINDOW, &board_pane, &fx.server.runner()).expect("pin");

    assert!(
        !fx.server.pane_exists(&fx.tree_pane),
        "the tree pane must not be orphaned"
    );
    assert!(
        !fx.server.pane_exists(&editor),
        "the editor pane must not be orphaned"
    );
}
