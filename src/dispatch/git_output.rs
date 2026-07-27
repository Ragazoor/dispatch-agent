//! Named constants for the English-language substrings `finish.rs` and
//! `worktree.rs` pattern-match in git's stdout/stderr to detect outcomes git
//! does not otherwise expose in a locale- and version-stable way.
//!
//! Investigated whether a structural signal could replace these:
//! - `git rebase`'s exit code is **not** a reliable discriminator — a probe
//!   against git 2.54 showed both a real conflict and an unrelated failure
//!   (rebase refused due to a dirty worktree) exit with status 1.
//! - The presence of `.git/rebase-merge` (or `rebase-apply`) after a failed
//!   rebase *is* a reliable structural signal — it only exists once git has
//!   actually started rewriting commits — but reading it requires a
//!   filesystem check outside the `ProcessRunner` abstraction this module's
//!   callers are tested through (and, for a linked worktree, resolving the
//!   real per-worktree gitdir rather than `<worktree>/.git` directly). That's
//!   a bigger architectural change than this fix warrants, so the string
//!   match remains, centralised here instead of duplicated per call site.

/// `git rebase <branch>` (git >= 2.30) writes this to stdout when a patch
/// conflicts with the target.
const REBASE_CONFLICT_MARKER: &str = "CONFLICT";
/// `git rebase <branch>` (git >= 2.30) writes this to stderr for the same
/// conflict, alongside [`REBASE_CONFLICT_MARKER`].
const REBASE_COULD_NOT_APPLY: &str = "could not apply";
/// Alternate phrasing `git rebase` (git >= 2.30) uses for a conflict in
/// stderr on some code paths.
const REBASE_MERGE_CONFLICT: &str = "Merge conflict";

/// Whether a failed `git rebase <branch>`'s stdout/stderr indicates a
/// conflict, as opposed to some other rebase failure (e.g. a dirty
/// worktree, an invalid upstream). Combines all three markers so a future
/// git-version quirk in wording/stream only needs updating here, not at
/// every call site.
pub fn is_rebase_conflict(stdout: &str, stderr: &str) -> bool {
    stderr.contains(REBASE_CONFLICT_MARKER)
        || stdout.contains(REBASE_CONFLICT_MARKER)
        || stderr.contains(REBASE_COULD_NOT_APPLY)
        || stderr.contains(REBASE_MERGE_CONFLICT)
}

/// `git worktree remove <path>` (git >= 2.30) stderr substring when the path
/// is already not a registered worktree (manually removed or pruned) —
/// dispatch treats this as the desired end state rather than a failure.
pub const WORKTREE_ALREADY_REMOVED: &str = "is not a working tree";
