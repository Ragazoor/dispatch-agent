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
