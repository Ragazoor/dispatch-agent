//! Opening the agent-tree companion pane's selected file in the user's editor
//! (see `docs/specs/agent-tree.allium`'s `OpenSelectedAgentTreeFile` surface
//! action and the `OpenAgentTreeFileInEditor` / `ReplaceAgentTreeEditorFile`
//! rules).
//!
//! Sibling of `src/agent_tree.rs`, which owns tree *building*. This module owns
//! the effect: which tmux pane the editor runs in. *Which* editor is
//! `crate::editor::editor_from_env` — shared with the board's pop-out task and
//! epic editor, so one `$EDITOR` means one thing everywhere
//! (docs/specs/core.allium: `editor_fallback`).

use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::process::ProcessRunner;
use crate::tmux::{self, PANE_ROLE_EDITOR, PANE_ROLE_OPTION};

/// Height of the editor pane as a percentage of the agent window — matches
/// `agent_tree_editor_pane_percent` in docs/specs/agent-tree.allium.
pub const EDITOR_PANE_PERCENT: u8 = 60;

/// The pane this process is running in, from `$TMUX_PANE`.
///
/// tmux exports it into every pane it creates, so it is present whenever the
/// renderer runs where it is meant to. Its absence means the renderer was started
/// outside tmux, which is a real (if unusual) way to run it — hence an error the
/// caller can show, not a panic.
pub fn current_pane_from_env() -> Result<String> {
    std::env::var("TMUX_PANE")
        .ok()
        .filter(|p| !p.is_empty())
        .context("not running inside tmux ($TMUX_PANE is unset)")
}

/// Show `relative` (a path below `root`) in this agent window's editor pane:
/// split one below the tree at [`EDITOR_PANE_PERCENT`] if the window has none
/// yet, otherwise replace what the existing one is running.
///
/// `my_pane` is the calling renderer's own pane id, used both as the split target
/// and to identify the window to look in — every pane involved is in the agent's
/// own window.
///
/// Focus does not move (see [`tmux::split_window_full_below_running`]), so the
/// user can keep browsing; each subsequent call swaps the file shown below.
/// Replacing kills whatever the pane was running, an editor with unsaved changes
/// included: accepted at design time in exchange for a layout that does not
/// subdivide on every open (docs/specs/agent-tree.allium:
/// `ReplaceAgentTreeEditorFile`).
pub fn open_in_editor(
    root: &Path,
    relative: &Path,
    my_pane: &str,
    editor: &[String],
    runner: &dyn ProcessRunner,
) -> Result<()> {
    if editor.is_empty() {
        bail!("no editor to run");
    }
    // A `..` in a selection path would address a file outside the worktree,
    // which no rendered node can name — the tree is built from path segments —
    // but the pane is deliberately rooted at the worktree and this keeps it so.
    //
    // The same guard the tree itself is built through
    // (`agent_tree::relative_components`), deliberately: two hand-rolled copies
    // of "is this path safely below the root" are two things that can drift
    // apart on which components they accept.
    if crate::agent_tree::relative_components(relative).is_none() {
        bail!("{}: not a path inside the worktree", relative.display());
    }

    let absolute = root.join(relative);
    // A Deleted node is refused before it ever reaches here (see `handle_key`'s
    // RefuseToOpenDeletedAgentTreeFile arm), so what this catches is the
    // remaining window: a file that vanished between the last git query and the
    // keypress. Checked before splitting so the failure is a message rather than
    // an editor sitting on an empty buffer.
    if !absolute.is_file() {
        bail!("{}: no longer exists", relative.display());
    }
    let absolute = absolute.to_string_lossy().into_owned();
    let root = root.to_string_lossy().into_owned();

    let mut command: Vec<&str> = editor.iter().map(String::as_str).collect();
    command.push(&absolute);

    // A failed lookup must not degrade to "no editor pane": that would split a
    // fresh pane on every press until the window ran out of room.
    // Value-matched, not presence-matched: the tree pane this was called from
    // carries the same option under a different role, and respawning *it* with an
    // editor would take the tree away.
    let existing =
        tmux::pane_ids_with_option_value(my_pane, PANE_ROLE_OPTION, PANE_ROLE_EDITOR, runner)
            .context("failed to look for an existing editor pane")?;

    if let Some(pane) = existing.first() {
        return tmux::respawn_pane_running(pane, &root, &command, runner);
    }

    let pane = tmux::split_window_full_below_running(
        my_pane,
        EDITOR_PANE_PERCENT,
        &root,
        &command,
        runner,
    )?;
    // The pane is open and showing the file; the marker only matters to the
    // *next* open, so a failure here is logged rather than reported as a failed
    // open. Worst case the next press splits a second pane.
    if let Err(e) = tmux::set_pane_option(&pane, PANE_ROLE_OPTION, PANE_ROLE_EDITOR, runner) {
        tracing::warn!(%pane, error = %e, "failed to mark the editor pane");
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    // --- open_in_editor ---

    use crate::process::MockProcessRunner;

    /// A real worktree with real files in it: `open_in_editor` checks the file
    /// exists before splitting, so a `tempfile` root is not optional.
    struct Fixture {
        dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
            std::fs::write(dir.path().join("src/lib.rs"), "fn main() {}").expect("write");
            Self { dir }
        }

        fn root(&self) -> &Path {
            self.dir.path()
        }

        fn abs(&self, relative: &str) -> String {
            self.root().join(relative).to_string_lossy().into_owned()
        }
    }

    fn editor() -> Vec<String> {
        vec!["vim".to_string()]
    }

    #[test]
    fn first_open_splits_a_pane_and_marks_it() {
        let fx = Fixture::new();
        let mock = MockProcessRunner::new(vec![
            // pane_ids_with_option_value: no editor pane yet
            MockProcessRunner::ok_with_stdout(b"%1 agent_tree\n%2 \n"),
            // split-window
            MockProcessRunner::ok_with_stdout(b"%7\n"),
            // set-option
            MockProcessRunner::ok(),
        ]);

        open_in_editor(fx.root(), Path::new("src/lib.rs"), "%1", &editor(), &mock).unwrap();

        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 3, "calls: {calls:?}");
        assert_eq!(calls[1].1[0], "split-window");
        assert_eq!(
            calls[1].1.last().unwrap(),
            &fx.abs("src/lib.rs"),
            "the editor must be given the absolute path"
        );
        assert_eq!(
            calls[2].1,
            vec![
                "set-option",
                "-p",
                "-t",
                "%7",
                PANE_ROLE_OPTION,
                PANE_ROLE_EDITOR
            ]
        );
    }

    #[test]
    fn the_editor_pane_starts_in_the_worktree() {
        let fx = Fixture::new();
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"%1 agent_tree\n"),
            MockProcessRunner::ok_with_stdout(b"%7\n"),
            MockProcessRunner::ok(),
        ]);

        open_in_editor(fx.root(), Path::new("src/lib.rs"), "%1", &editor(), &mock).unwrap();

        let args = &mock.recorded_calls()[1].1;
        let cwd = args.iter().position(|a| a == "-c").unwrap() + 1;
        assert_eq!(args[cwd], fx.root().to_string_lossy());
    }

    #[test]
    fn a_second_open_respawns_the_existing_pane_instead_of_splitting() {
        let fx = Fixture::new();
        let mock = MockProcessRunner::new(vec![
            // pane_ids_with_option_value: %7 is already the editor pane
            MockProcessRunner::ok_with_stdout(b"%1 agent_tree\n%7 editor\n"),
            // respawn-pane
            MockProcessRunner::ok(),
        ]);

        open_in_editor(fx.root(), Path::new("src/lib.rs"), "%1", &editor(), &mock).unwrap();

        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 2, "calls: {calls:?}");
        assert_eq!(calls[1].1[0], "respawn-pane");
        assert!(
            calls[1].1.contains(&"%7".to_string()),
            "must target the marked pane; calls: {calls:?}"
        );
        assert!(
            !calls.iter().any(|(_, args)| args[0] == "split-window"),
            "a second open must not add a pane; calls: {calls:?}"
        );
    }

    /// The tree pane the renderer is running in carries the *same* pane option
    /// under a different role, so a presence-matched lookup would find it and
    /// respawn the tree with an editor in it — taking the tree away and leaving
    /// the window with no way back to it.
    #[test]
    fn the_tree_panes_own_role_is_not_mistaken_for_an_editor_pane() {
        let fx = Fixture::new();
        let mock = MockProcessRunner::new(vec![
            // Only the calling pane is marked, and it is the tree.
            MockProcessRunner::ok_with_stdout(b"%1 agent_tree\n"),
            MockProcessRunner::ok_with_stdout(b"%7\n"), // split-window
            MockProcessRunner::ok(),                    // set-option
        ]);

        open_in_editor(fx.root(), Path::new("src/lib.rs"), "%1", &editor(), &mock).unwrap();

        let calls = mock.recorded_calls();
        assert_eq!(calls[1].1[0], "split-window", "calls: {calls:?}");
        assert!(
            !calls.iter().any(|(_, args)| args[0] == "respawn-pane"),
            "must not respawn the tree pane; calls: {calls:?}"
        );
    }

    /// Multi-word editors reach tmux as separate argv elements — a single
    /// "vim -p /path" string would be looked up as a binary of that name.
    #[test]
    fn a_multi_word_editor_stays_separate_argv_elements() {
        let fx = Fixture::new();
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"%1 agent_tree\n"),
            MockProcessRunner::ok_with_stdout(b"%7\n"),
            MockProcessRunner::ok(),
        ]);

        open_in_editor(
            fx.root(),
            Path::new("src/lib.rs"),
            "%1",
            &["vim".to_string(), "-p".to_string()],
            &mock,
        )
        .unwrap();

        let args = &mock.recorded_calls()[1].1;
        let sep = args.iter().position(|a| a == "--").unwrap();
        assert_eq!(args[sep + 1], "vim");
        assert_eq!(args[sep + 2], "-p");
        assert_eq!(args[sep + 3], fx.abs("src/lib.rs"));
    }

    #[test]
    fn a_missing_file_is_an_error_and_runs_no_tmux_command() {
        let fx = Fixture::new();
        let mock = MockProcessRunner::new(vec![]);

        let err = open_in_editor(fx.root(), Path::new("src/gone.rs"), "%1", &editor(), &mock)
            .unwrap_err();

        assert!(
            err.to_string().contains("src/gone.rs"),
            "the message must name the file: {err}"
        );
        assert!(mock.recorded_calls().is_empty());
    }

    /// A selection path must not be able to address anything outside the
    /// worktree. Tree nodes are built from path segments so this is not
    /// reachable today, but the check is one line and the alternative is
    /// trusting that forever.
    #[test]
    fn a_path_escaping_the_worktree_is_rejected() {
        let fx = Fixture::new();
        let mock = MockProcessRunner::new(vec![]);

        assert!(open_in_editor(
            fx.root(),
            Path::new("../outside.rs"),
            "%1",
            &editor(),
            &mock
        )
        .is_err());
        assert!(mock.recorded_calls().is_empty());
    }

    #[test]
    fn a_failing_split_is_an_error() {
        let fx = Fixture::new();
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"%1 agent_tree\n"),
            MockProcessRunner::fail("no space for a new pane"),
        ]);

        assert!(
            open_in_editor(fx.root(), Path::new("src/lib.rs"), "%1", &editor(), &mock).is_err()
        );
    }

    /// The marker is a convenience for the *next* open, not the point of this
    /// one: the pane is already open and showing the file, so failing the
    /// operation would misreport what happened.
    #[test]
    fn a_failing_marker_write_does_not_fail_the_open() {
        let fx = Fixture::new();
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"%1 agent_tree\n"),
            MockProcessRunner::ok_with_stdout(b"%7\n"),
            MockProcessRunner::fail("bad option"),
        ]);

        assert!(open_in_editor(fx.root(), Path::new("src/lib.rs"), "%1", &editor(), &mock).is_ok());
    }

    /// A failed lookup must not be read as "no editor pane" — that would split a
    /// second pane on every press.
    #[test]
    fn a_failing_pane_lookup_is_an_error() {
        let fx = Fixture::new();
        let mock = MockProcessRunner::new(vec![MockProcessRunner::fail("no window")]);

        assert!(
            open_in_editor(fx.root(), Path::new("src/lib.rs"), "%1", &editor(), &mock).is_err()
        );
    }

    #[test]
    fn an_empty_editor_is_an_error() {
        let fx = Fixture::new();
        let mock = MockProcessRunner::new(vec![]);

        assert!(open_in_editor(fx.root(), Path::new("src/lib.rs"), "%1", &[], &mock).is_err());
        assert!(mock.recorded_calls().is_empty());
    }
}
