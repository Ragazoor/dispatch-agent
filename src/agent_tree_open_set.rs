//! The one-way channel between the agent-tree companion pane and the diff pane
//! beneath it: which files' diffs the user has opened.
//!
//! The two are separate processes in separate tmux panes, so the set has to
//! reach the second one somehow. It goes in a file in the worktree's git
//! administrative directory — see [`crate::worktree_admin`] for why there, and
//! `OpenSetIsNotWorktreeState` in `docs/specs/agent-tree.allium` for what that
//! placement guarantees the user.
//!
//! **The tree writes, the diff pane reads, and never the other way round.**
//! That is what keeps the tree the single place a toggle is decided even though
//! two processes can see the file, and it is what makes the tree's row markers
//! unable to disagree with the pane below them (`NodeDiffOpenMatchesOpenSet`).
//!
//! Nothing here is durable state. The tree empties the set when it exits, so a
//! file left behind by a killed renderer describes nothing, and every read
//! soft-fails to "nothing is open" rather than erroring — a view of a git
//! query must not fail to draw because a scratch file was unreadable.

use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::worktree_admin::worktree_admin_dir;

/// Basename of the open-set file inside the worktree's git admin directory.
const OPEN_SET_FILE: &str = "dispatch-agent-tree.json";

/// Where this worktree's open set lives, or `None` when the path is not a
/// linked worktree and so has no admin directory to put it in.
pub fn open_set_path(worktree_path: &str) -> Option<PathBuf> {
    Some(worktree_admin_dir(worktree_path)?.join(OPEN_SET_FILE))
}

/// Replace the recorded open set.
///
/// Written to a sibling temporary file and renamed into place, because the diff
/// pane reads this on its own timer and the two are not synchronised. A rename
/// is atomic within a directory on every platform this runs on, so a reader
/// either sees the whole previous set or the whole new one — never half a file,
/// which would decode as "nothing is open" and blank the pane for a tick.
pub fn write_open_set(worktree_path: &str, paths: &BTreeSet<PathBuf>) -> Result<()> {
    let path = open_set_path(worktree_path)
        .with_context(|| format!("{worktree_path} is not a linked worktree"))?;

    let encoded: Vec<String> = paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let body = serde_json::to_vec(&encoded).context("could not encode the open set")?;

    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, &body).with_context(|| format!("could not write {}", temp.display()))?;
    std::fs::rename(&temp, &path)
        .with_context(|| format!("could not replace {}", path.display()))?;
    Ok(())
}

/// Read the recorded open set, or an empty one.
///
/// Every failure reads as empty and none of them is an error: the file has not
/// been written yet, the worktree is not linked, the file is half-written or
/// holds something this does not understand. The diff pane's answer to all of
/// those is the same — show nothing — and making it an error would let a
/// scratch file take down a view whose real source of truth is git.
pub fn read_open_set(worktree_path: &str) -> BTreeSet<PathBuf> {
    let Some(path) = open_set_path(worktree_path) else {
        return BTreeSet::new();
    };
    let Ok(body) = std::fs::read(&path) else {
        return BTreeSet::new();
    };
    let Ok(paths) = serde_json::from_slice::<Vec<String>>(&body) else {
        tracing::warn!(
            path = %path.display(),
            "agent-tree open set is unreadable; treating it as empty"
        );
        return BTreeSet::new();
    };
    // A path that escapes the worktree cannot name a file the tree ever showed,
    // so it is dropped rather than trusted — the same fail-closed rule
    // `agent_tree::relative_components` applies to git's own output.
    paths
        .into_iter()
        .map(PathBuf::from)
        .filter(|p| crate::agent_tree::relative_components(p).is_some())
        .collect()
}

/// Forget everything, leaving the file behind as an empty set.
///
/// Called when the tree exits, so a diff pane that outlives it by a moment sees
/// an honest answer rather than a stale one. A worktree with no admin directory
/// has nothing to clear and reports success.
pub fn clear_open_set(worktree_path: &str) -> Result<()> {
    if open_set_path(worktree_path).is_none() {
        return Ok(());
    }
    write_open_set(worktree_path, &BTreeSet::new())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    // The one on-disk encoding of a linked worktree, shared with every other
    // test of this placement rule — see its own doc comment.
    use crate::worktree_admin::tests::make_linked_worktree;

    fn set_of(paths: &[&str]) -> BTreeSet<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn the_open_set_goes_in_the_worktrees_git_admin_directory() {
        let dir = tempfile::tempdir().unwrap();
        let (worktree, admin) = make_linked_worktree(dir.path(), "42-fix");

        assert_eq!(
            open_set_path(&worktree),
            Some(admin.join("dispatch-agent-tree.json"))
        );
    }

    /// The whole point of the placement: nothing under the admin directory is
    /// inside the working tree, so git cannot report it and the agent cannot
    /// commit it. See OpenSetIsNotWorktreeState in the spec.
    #[test]
    fn the_open_set_is_never_written_inside_the_working_tree() {
        let dir = tempfile::tempdir().unwrap();
        let (worktree, _) = make_linked_worktree(dir.path(), "42-fix");

        write_open_set(&worktree, &set_of(&["a.rs"])).unwrap();

        let path = open_set_path(&worktree).unwrap();
        assert!(
            !path.starts_with(&worktree),
            "{} must not be inside {worktree}",
            path.display()
        );
    }

    #[test]
    fn a_written_set_reads_back_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let (worktree, _) = make_linked_worktree(dir.path(), "42-fix");
        let paths = set_of(&["a.rs", "src/lib.rs", "docs/my notes.md"]);

        write_open_set(&worktree, &paths).unwrap();

        assert_eq!(read_open_set(&worktree), paths);
    }

    #[test]
    fn an_unwritten_set_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let (worktree, _) = make_linked_worktree(dir.path(), "42-fix");
        assert!(read_open_set(&worktree).is_empty());
    }

    /// The diff pane reads on its own timer while the tree writes on keypresses.
    /// A half-written or hand-mangled file must cost one blank pane, not a
    /// crash — soft-fail decoding, as everywhere else in this subsystem.
    #[test]
    fn an_unreadable_set_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let (worktree, _) = make_linked_worktree(dir.path(), "42-fix");
        let path = open_set_path(&worktree).unwrap();
        std::fs::write(&path, b"{ not json at all").unwrap();

        assert!(read_open_set(&worktree).is_empty());
    }

    /// A path that escapes the worktree names nothing the tree ever rendered,
    /// so it is dropped rather than handed to a git command.
    #[test]
    fn a_path_escaping_the_worktree_is_dropped_on_read() {
        let dir = tempfile::tempdir().unwrap();
        let (worktree, _) = make_linked_worktree(dir.path(), "42-fix");
        let path = open_set_path(&worktree).unwrap();
        std::fs::write(&path, br#"["../outside.rs","/etc/passwd","ok.rs"]"#).unwrap();

        assert_eq!(read_open_set(&worktree), set_of(&["ok.rs"]));
    }

    #[test]
    fn writing_replaces_rather_than_merges() {
        let dir = tempfile::tempdir().unwrap();
        let (worktree, _) = make_linked_worktree(dir.path(), "42-fix");

        write_open_set(&worktree, &set_of(&["a.rs", "b.rs"])).unwrap();
        write_open_set(&worktree, &set_of(&["c.rs"])).unwrap();

        assert_eq!(read_open_set(&worktree), set_of(&["c.rs"]));
    }

    #[test]
    fn clearing_leaves_an_empty_set_behind() {
        let dir = tempfile::tempdir().unwrap();
        let (worktree, _) = make_linked_worktree(dir.path(), "42-fix");
        write_open_set(&worktree, &set_of(&["a.rs"])).unwrap();

        clear_open_set(&worktree).unwrap();

        assert!(read_open_set(&worktree).is_empty());
    }

    /// A main checkout's `.git` is a directory, not a pointer file, so there is
    /// nowhere to put the set — and reading reports "nothing open" rather than
    /// failing, which is what keeps `dispatch agent-tree` usable outside a
    /// dispatched worktree.
    #[test]
    fn a_path_that_is_not_a_linked_worktree_has_nowhere_to_record() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("main-checkout");
        std::fs::create_dir_all(plain.join(".git")).unwrap();
        let plain = plain.to_string_lossy().into_owned();

        assert_eq!(open_set_path(&plain), None);
        assert!(read_open_set(&plain).is_empty());
        assert!(write_open_set(&plain, &set_of(&["a.rs"])).is_err());
        assert!(clear_open_set(&plain).is_ok());
    }
}
