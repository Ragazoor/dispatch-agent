//! Opening the agent-tree companion pane's selected file in the user's editor
//! (see `docs/specs/agent-tree.allium`'s `OpenSelectedAgentTreeFile` surface
//! action and the `OpenAgentTreeFileInEditor` / `ReplaceAgentTreeEditorFile`
//! rules).
//!
//! Sibling of `src/agent_tree.rs`, which owns tree *building*. This module owns
//! the effect: which editor to run, and which tmux pane it runs in.

use std::path::{Component, Path};

use anyhow::{bail, Context, Result};

use crate::process::ProcessRunner;
use crate::tmux;

/// Editor of last resort when neither `$VISUAL` nor `$EDITOR` names one —
/// `config.agent_tree_editor_fallback` in docs/specs/agent-tree.allium.
pub const EDITOR_FALLBACK: &str = "vi";

/// tmux pane option marking the pane this module opened, so the next open finds
/// it instead of splitting another. See `OneEditorPanePerAgentWindow` in
/// docs/specs/agent-tree.allium.
pub const EDITOR_PANE_OPTION: &str = "@dispatch_editor_pane";

/// Height of the editor pane as a percentage of the agent window — matches
/// `agent_tree_editor_pane_percent` in docs/specs/agent-tree.allium.
pub const EDITOR_PANE_PERCENT: u8 = 60;

/// Resolve the editor argv from environment *values*: `$VISUAL`, then
/// `$EDITOR`, then [`EDITOR_FALLBACK`]. Never returns an empty vector.
///
/// Takes the values as parameters rather than reading the process environment,
/// so the resolution order is testable without `std::env::set_var` — which is
/// `unsafe` in edition 2024 and races the test harness's threads either way.
/// [`editor_from_env`] is the one-line adapter that reads them.
///
/// A value is treated as unset when it is empty or all whitespace: `export
/// EDITOR=` is how a shell spells "no editor", and it would otherwise produce an
/// unrunnable empty argv. The value is split on whitespace into argv and
/// executed directly, never through a shell, so `EDITOR="nvim -p"` works and
/// nothing in it is expanded, globbed or word-split by anything but this
/// function.
pub fn resolve_editor(visual: Option<&str>, editor: Option<&str>) -> Vec<String> {
    for candidate in [visual, editor] {
        let Some(value) = candidate else { continue };
        let argv: Vec<String> = value.split_whitespace().map(str::to_string).collect();
        if !argv.is_empty() {
            return argv;
        }
    }
    vec![EDITOR_FALLBACK.to_string()]
}

/// [`resolve_editor`] against the real process environment.
pub fn editor_from_env() -> Vec<String> {
    let visual = std::env::var("VISUAL").ok();
    let editor = std::env::var("EDITOR").ok();
    resolve_editor(visual.as_deref(), editor.as_deref())
}

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
    if relative
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::RootDir))
    {
        bail!("{}: not a path inside the worktree", relative.display());
    }

    let absolute = root.join(relative);
    // The event log records touches, never deletions, so a node outlives a file
    // the agent removed. Checked before splitting so the failure is a message
    // rather than an editor sitting on an empty buffer.
    if !absolute.is_file() {
        bail!("{}: no longer exists", relative.display());
    }
    let absolute = absolute.to_string_lossy().into_owned();
    let root = root.to_string_lossy().into_owned();

    let mut command: Vec<&str> = editor.iter().map(String::as_str).collect();
    command.push(&absolute);

    // A failed lookup must not degrade to "no editor pane": that would split a
    // fresh pane on every press until the window ran out of room.
    let existing = tmux::pane_ids_with_option(my_pane, EDITOR_PANE_OPTION, runner)
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
    if let Err(e) = tmux::set_pane_option(&pane, EDITOR_PANE_OPTION, "1", runner) {
        tracing::warn!(%pane, error = %e, "failed to mark the editor pane");
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn visual_wins_over_editor() {
        assert_eq!(resolve_editor(Some("nvim"), Some("nano")), vec!["nvim"]);
    }

    #[test]
    fn editor_is_used_when_visual_is_unset() {
        assert_eq!(resolve_editor(None, Some("nano")), vec!["nano"]);
    }

    #[test]
    fn falls_back_to_vi_when_neither_is_set() {
        assert_eq!(resolve_editor(None, None), vec![EDITOR_FALLBACK]);
    }

    /// An exported-but-empty variable is how a shell spells "unset" in practice
    /// (`export EDITOR=`), and an empty argv would be unrunnable.
    #[test]
    fn an_empty_value_counts_as_unset() {
        assert_eq!(resolve_editor(Some(""), Some("nano")), vec!["nano"]);
        assert_eq!(resolve_editor(Some(""), Some("")), vec![EDITOR_FALLBACK]);
        assert_eq!(resolve_editor(Some("   "), None), vec![EDITOR_FALLBACK]);
    }

    /// The value is argv, not a shell command: it is split on whitespace and
    /// executed directly, so flags in `$EDITOR` work and nothing in it is
    /// shell-interpreted.
    #[test]
    fn a_multi_word_value_splits_into_argv() {
        assert_eq!(resolve_editor(Some("nvim -p"), None), vec!["nvim", "-p"]);
        assert_eq!(
            resolve_editor(Some("  code   -w  "), None),
            vec!["code", "-w"]
        );
    }

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
            // pane_ids_with_option: no editor pane yet
            MockProcessRunner::ok_with_stdout(b"%1 \n%2 \n"),
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
            vec!["set-option", "-p", "-t", "%7", EDITOR_PANE_OPTION, "1"]
        );
    }

    #[test]
    fn the_editor_pane_starts_in_the_worktree() {
        let fx = Fixture::new();
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"%1 \n"),
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
            // pane_ids_with_option: %7 is already the editor pane
            MockProcessRunner::ok_with_stdout(b"%1 \n%7 1\n"),
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

    /// Multi-word editors reach tmux as separate argv elements — a single
    /// "nvim -p /path" string would be looked up as a binary of that name.
    #[test]
    fn a_multi_word_editor_stays_separate_argv_elements() {
        let fx = Fixture::new();
        let mock = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"%1 \n"),
            MockProcessRunner::ok_with_stdout(b"%7\n"),
            MockProcessRunner::ok(),
        ]);

        open_in_editor(
            fx.root(),
            Path::new("src/lib.rs"),
            "%1",
            &["nvim".to_string(), "-p".to_string()],
            &mock,
        )
        .unwrap();

        let args = &mock.recorded_calls()[1].1;
        let sep = args.iter().position(|a| a == "--").unwrap();
        assert_eq!(args[sep + 1], "nvim");
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
            MockProcessRunner::ok_with_stdout(b"%1 \n"),
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
            MockProcessRunner::ok_with_stdout(b"%1 \n"),
            MockProcessRunner::ok_with_stdout(b"%7\n"),
            MockProcessRunner::fail("bad option"),
        ]);

        assert!(
            open_in_editor(fx.root(), Path::new("src/lib.rs"), "%1", &editor(), &mock).is_ok()
        );
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
