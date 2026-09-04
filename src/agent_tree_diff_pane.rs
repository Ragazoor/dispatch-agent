//! The diff pane's lifecycle as seen from the tree that owns it: split it when
//! something is open, kill it when nothing is (see
//! `docs/specs/agent-tree.allium`'s `SplitAgentTreeDiffPane` and
//! `CloseAgentTreeDiffPaneWhenEmpty`).
//!
//! Sibling of `src/agent_tree.rs`, which owns tree *building*, and of
//! `src/cli/agent_diff.rs`, which owns what the pane draws once it exists. This
//! module owns only the tmux effect, so the renderer's key handling can stay a
//! pure function of the keys pressed.
//!
//! Replaces the editor pane this feature removed, and inherits its pane-role
//! marker, its focus rule and its accepted marker gap — the differences are
//! that the new pane subdivides the tree's own column instead of spanning the
//! window, and that it is killed rather than reused when it is no longer
//! wanted.

use std::path::Path;

use anyhow::{Context, Result};

use crate::process::ProcessRunner;
use crate::tmux::{self, PANE_ROLE_DIFF, PANE_ROLE_OPTION};

/// Height of the diff pane as a percentage of the COMPANION PANE'S COLUMN, not
/// of the agent window — matches `config.agent_tree_diff_pane_percent` in
/// docs/specs/agent-tree.allium.
///
/// Deliberately the larger share of that column: the tree is a list of short
/// path segments and stays legible in a few rows, while reading the change is
/// the point of opening it.
pub const DIFF_PANE_PERCENT: u8 = 66;

/// The pane this process is running in, from `$TMUX_PANE`.
///
/// tmux exports it into every pane it creates, so it is present whenever the
/// renderer runs where it is meant to. Its absence means the renderer was
/// started outside tmux, which is a real (if unusual) way to run it — hence an
/// error the caller can show, not a panic.
pub fn current_pane_from_env() -> Result<String> {
    std::env::var("TMUX_PANE")
        .ok()
        .filter(|p| !p.is_empty())
        .context("not running inside tmux ($TMUX_PANE is unset)")
}

/// This agent window's diff pane, if it has one.
///
/// Value-matched, not presence-matched: the tree pane this is called from
/// carries the same option under a different role, and mistaking one for the
/// other would let a close take the tree away.
fn existing_diff_pane(my_pane: &str, runner: &dyn ProcessRunner) -> Result<Option<String>> {
    Ok(
        tmux::pane_ids_with_option_value(my_pane, PANE_ROLE_OPTION, PANE_ROLE_DIFF, runner)
            .context("failed to look for an existing diff pane")?
            .into_iter()
            .next(),
    )
}

/// Make the open set and the panes agree: split a diff pane when something is
/// open and there is none, kill it when nothing is open and there is one.
///
/// Called on every change to the open set, which is what makes a pane the user
/// killed themselves come back on their next toggle rather than leaving them
/// with a set of open files and nowhere to see them. Deliberately NOT called on
/// the refresh tick: a pane the user closed a second ago must stay closed until
/// they ask again.
///
/// `my_pane` is the calling renderer's own pane id, used both as the split
/// target and to identify the window to look in — every pane involved is in the
/// agent's own window.
pub fn reconcile_diff_pane(
    my_pane: &str,
    db_path: &Path,
    task_id: i64,
    worktree: &Path,
    anything_open: bool,
    runner: &dyn ProcessRunner,
) -> Result<()> {
    let existing = existing_diff_pane(my_pane, runner)?;

    match (anything_open, existing) {
        (true, None) => split_diff_pane(my_pane, db_path, task_id, worktree, runner),
        (false, Some(pane)) => tmux::kill_pane(&pane, runner),
        // Already agreeing: something is open and a pane is showing it, or
        // nothing is open and there is no pane. Both are the resting state.
        (true, Some(_)) | (false, None) => Ok(()),
    }
}

fn split_diff_pane(
    my_pane: &str,
    db_path: &Path,
    task_id: i64,
    worktree: &Path,
    runner: &dyn ProcessRunner,
) -> Result<()> {
    let db_arg = db_path.to_string_lossy().into_owned();
    let id_arg = task_id.to_string();
    let dispatch_bin = runner.agent_binaries().dispatch;
    let cwd = worktree.to_string_lossy().into_owned();

    // `--db` is named explicitly rather than left to the default. This process
    // was given a database path and the pane below it must read the SAME open
    // set and the same task; inheriting a default that happened to match would
    // be a coincidence, not a guarantee.
    let command = [
        dispatch_bin.as_str(),
        "--db",
        db_arg.as_str(),
        "agent-diff",
        id_arg.as_str(),
    ];

    let pane =
        tmux::split_window_below_running(my_pane, DIFF_PANE_PERCENT, &cwd, &command, runner)?;

    // The pane is open and rendering; the marker only matters to the NEXT
    // reconcile, so a failure here is logged rather than reported as a failed
    // open. Worst case a later toggle splits a second pane — the same accepted
    // gap the editor pane's marker had, and the reason
    // OneDiffPanePerAgentWindow is held by construction rather than enforced.
    if let Err(e) = tmux::set_pane_option(&pane, PANE_ROLE_OPTION, PANE_ROLE_DIFF, runner) {
        tracing::warn!(%pane, error = %e, "failed to mark the diff pane with its role");
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::process::MockProcessRunner;

    fn db() -> &'static Path {
        Path::new("/data/tasks.db")
    }

    fn worktree() -> &'static Path {
        Path::new("/work/wt")
    }

    /// No pane carries the diff role, then the split and the marker succeed.
    fn no_pane_then_split() -> MockProcessRunner {
        MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"%1 \n%2 agent_tree\n"),
            MockProcessRunner::ok_with_stdout(b"%9\n"),
            MockProcessRunner::ok(),
        ])
    }

    /// A diff pane already exists, then whatever the caller does with it.
    fn existing_pane(rest: Vec<anyhow::Result<std::process::Output>>) -> MockProcessRunner {
        let mut queued = vec![MockProcessRunner::ok_with_stdout(
            b"%1 \n%2 agent_tree\n%9 diff\n",
        )];
        queued.extend(rest);
        MockProcessRunner::new(queued)
    }

    #[test]
    fn opening_the_first_diff_splits_a_pane_below_the_tree() {
        let runner = no_pane_then_split();

        reconcile_diff_pane("%2", db(), 42, worktree(), true, &runner).unwrap();

        let calls = runner.flattened_calls();
        assert!(
            calls[1].contains("split-window") && calls[1].contains("-t %2"),
            "must split from the tree's own pane; got {calls:?}"
        );
    }

    /// The pane must read the SAME database as the tree, so it sees the same
    /// open set and the same task. Inheriting a default that happened to match
    /// would be a coincidence, not a guarantee.
    #[test]
    fn the_diff_pane_is_told_which_database_and_task_to_read() {
        let runner = no_pane_then_split();

        reconcile_diff_pane("%2", db(), 42, worktree(), true, &runner).unwrap();

        let split = &runner.flattened_calls()[1];
        assert!(split.contains("--db /data/tasks.db"), "got {split}");
        assert!(split.contains("agent-diff 42"), "got {split}");
    }

    #[test]
    fn the_new_pane_is_marked_with_the_diff_role() {
        let runner = no_pane_then_split();

        reconcile_diff_pane("%2", db(), 42, worktree(), true, &runner).unwrap();

        let mark = &runner.flattened_calls()[2];
        assert!(
            mark.contains("set-option") && mark.contains("%9") && mark.ends_with("diff"),
            "got {mark}"
        );
    }

    /// Splitting again on every toggle would subdivide the column until the
    /// window ran out of room.
    #[test]
    fn opening_a_second_diff_reuses_the_pane_that_is_already_there() {
        let runner = existing_pane(vec![]);

        reconcile_diff_pane("%2", db(), 42, worktree(), true, &runner).unwrap();

        assert_eq!(runner.flattened_calls().len(), 1, "lookup only");
    }

    #[test]
    fn closing_the_last_diff_kills_the_pane() {
        let runner = existing_pane(vec![MockProcessRunner::ok()]);

        reconcile_diff_pane("%2", db(), 42, worktree(), false, &runner).unwrap();

        assert_eq!(
            runner.flattened_calls()[1],
            "tmux kill-pane -t %9".to_string()
        );
    }

    /// The resting state, and by far the commonest call: nothing is open and
    /// there is no pane. It must cost one lookup and no tmux mutation.
    #[test]
    fn nothing_open_and_no_pane_changes_nothing() {
        let runner = MockProcessRunner::new(vec![MockProcessRunner::ok_with_stdout(
            b"%1 \n%2 agent_tree\n",
        )]);

        reconcile_diff_pane("%2", db(), 42, worktree(), false, &runner).unwrap();

        assert_eq!(runner.flattened_calls().len(), 1);
    }

    /// The role lookup is matched on its exact VALUE. The tree pane carries the
    /// same option under a different role, and mistaking one for the other
    /// would let a close take the tree away.
    #[test]
    fn the_trees_own_pane_is_never_mistaken_for_the_diff_pane() {
        let runner = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"%2 agent_tree\n"),
            MockProcessRunner::ok_with_stdout(b"%9\n"),
            MockProcessRunner::ok(),
        ]);

        reconcile_diff_pane("%2", db(), 42, worktree(), true, &runner).unwrap();

        let calls = runner.flattened_calls();
        assert!(
            !calls.iter().any(|c| c.contains("kill-pane")),
            "got {calls:?}"
        );
        assert!(calls[1].contains("split-window"), "got {calls:?}");
    }

    /// A failed lookup must not degrade to "there is no diff pane": that would
    /// split a fresh one on every toggle until the column ran out of room.
    #[test]
    fn a_failed_role_lookup_is_an_error_rather_than_a_second_pane() {
        let runner = MockProcessRunner::new(vec![MockProcessRunner::fail("no server running")]);

        let err = reconcile_diff_pane("%2", db(), 42, worktree(), true, &runner).unwrap_err();

        assert!(
            format!("{err:#}").contains("existing diff pane"),
            "got {err:#}"
        );
    }

    /// The pane is open and rendering; the marker only matters to the next
    /// reconcile, so failing to write it must not report a failed open.
    #[test]
    fn a_pane_that_opened_but_could_not_be_marked_still_counts_as_opened() {
        let runner = MockProcessRunner::new(vec![
            MockProcessRunner::ok_with_stdout(b"%2 agent_tree\n"),
            MockProcessRunner::ok_with_stdout(b"%9\n"),
            MockProcessRunner::fail("unknown option"),
        ]);

        assert!(reconcile_diff_pane("%2", db(), 42, worktree(), true, &runner).is_ok());
    }
}
