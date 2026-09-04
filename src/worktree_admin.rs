//! Where a linked worktree's git administrative directory is, and why two
//! unrelated subsystems both want to know.
//!
//! A file put in `<repo>/.git/worktrees/<name>/` has three properties nothing
//! else in a worktree has, and dispatch depends on all three:
//!
//!   * **Git never reports it.** It cannot appear as an untracked file, so an
//!     agent cannot stage or commit it, and it cannot show up in the
//!     agent-tree pane that is itself built from `git status`-shaped queries.
//!   * **`git worktree remove` deletes it** along with the worktree, so the
//!     teardown that already runs cleans it up: no fourth teardown step, and
//!     no new failure mode (`CallerIdentityConfigGoesWithTheWorktree` in
//!     docs/specs/tasks.allium).
//!   * **It is per worktree**, so two agents cannot read each other's.
//!
//! Two callers rely on that today: `dispatch::caller_identity` writes the
//! per-task MCP configuration there, and `agent_tree_open_set` writes the set
//! of files whose diffs the companion pane has open. The placement rule is the
//! same for both, so it is stated once, here.

use std::path::{Path, PathBuf};

/// The linked worktree's git administrative directory, read from the `.git`
/// pointer file git itself writes there.
///
/// A linked worktree's `.git` is a FILE holding `gitdir: <path>`; a main
/// checkout's is a directory. Reading the pointer rather than assembling
/// `<repo>/.git/worktrees/<name>` by hand costs no subprocess and stays correct
/// where the assembled guess would not — a repo that is itself a linked
/// worktree, a relocated admin directory, or a name git had to disambiguate.
///
/// The pointer may be RELATIVE — `git worktree add --relative-paths`, or
/// `worktree.useRelativePaths`, writes `gitdir: ../../.git/worktrees/<name>`.
/// It is relative to the worktree, not to this process, which has its own
/// unrelated working directory; joining it onto the worktree is what keeps the
/// file from being written somewhere else entirely. An absolute pointer needs
/// no arm of its own — `Path::join` discards the base for an absolute argument.
///
/// `None` for anything that is not a linked worktree, which is also the answer
/// for a `MockProcessRunner` dispatch that never really ran `git worktree add`.
pub fn worktree_admin_dir(worktree_path: &str) -> Option<PathBuf> {
    let pointer = std::fs::read_to_string(Path::new(worktree_path).join(".git")).ok()?;
    let dir = pointer.trim().strip_prefix("gitdir:")?.trim();
    if dir.is_empty() {
        return None;
    }
    Some(Path::new(worktree_path).join(dir))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
pub(crate) mod tests {
    use super::*;

    /// A linked worktree on disk: a `.git` POINTER FILE naming an admin
    /// directory that exists. Returns the worktree path and its admin
    /// directory.
    ///
    /// `pub(crate)` and living here rather than in a consumer's test module,
    /// because every test of this placement rule — this module's own, and
    /// `caller_identity`'s and `agent_tree_open_set`'s over the files they put
    /// there — needs the same on-disk shape. Two encodings of what git writes
    /// would be two things to keep in step with git.
    pub(crate) fn make_linked_worktree(base: &Path, slug: &str) -> (String, PathBuf) {
        // The real layout: the worktree under `<repo>/.worktrees/<name>` and its
        // admin directory under `<repo>/.git/worktrees/<name>`. The two-levels-up
        // relationship is load-bearing — `git worktree add --relative-paths`
        // writes `gitdir: ../../.git/worktrees/<name>`, and a fixture with a
        // different shape would resolve that to nothing.
        let worktree = base.join(".worktrees").join(slug);
        let admin = base.join(".git").join("worktrees").join(slug);
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", admin.display()),
        )
        .unwrap();
        (worktree.to_string_lossy().into_owned(), admin)
    }

    #[test]
    fn admin_dir_is_read_from_the_git_pointer_file() {
        let dir = tempfile::tempdir().unwrap();
        let (worktree, admin) = make_linked_worktree(dir.path(), "42-fix-bug");
        assert_eq!(worktree_admin_dir(&worktree), Some(admin));
    }

    #[test]
    fn a_relative_git_pointer_resolves_against_the_worktree() {
        // `git worktree add --relative-paths` (and worktree.useRelativePaths)
        // writes `gitdir: ../../.git/worktrees/<name>`. Taken literally that
        // resolves against THIS process's working directory, which is not the
        // worktree — the config would be written somewhere else entirely, or
        // not at all.
        let dir = tempfile::tempdir().unwrap();
        let (worktree, admin) = make_linked_worktree(dir.path(), "42-fix-bug");
        std::fs::write(
            Path::new(&worktree).join(".git"),
            "gitdir: ../../.git/worktrees/42-fix-bug\n",
        )
        .unwrap();

        let resolved = worktree_admin_dir(&worktree).unwrap();

        assert_eq!(
            resolved.canonicalize().unwrap(),
            admin.canonicalize().unwrap()
        );
    }

    #[test]
    fn a_main_checkout_has_no_admin_dir() {
        // `.git` is a directory there, not a pointer file.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        assert!(worktree_admin_dir(dir.path().to_str().unwrap()).is_none());
    }

    #[test]
    fn a_directory_with_no_git_at_all_has_no_admin_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert!(worktree_admin_dir(dir.path().to_str().unwrap()).is_none());
    }

    #[test]
    fn a_git_file_that_is_not_a_gitdir_pointer_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".git"), "not a pointer\n").unwrap();
        assert!(worktree_admin_dir(dir.path().to_str().unwrap()).is_none());
    }

    #[test]
    fn a_gitdir_pointer_naming_nothing_is_rejected() {
        // Otherwise the empty path joins to the worktree itself, and the config
        // lands where `git status` can see it.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".git"), "gitdir:   \n").unwrap();
        assert!(worktree_admin_dir(dir.path().to_str().unwrap()).is_none());
    }

    // -- write_agent_mcp_config --
}
