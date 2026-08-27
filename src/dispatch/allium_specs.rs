//! Does the task's repository keep Allium specs?
//!
//! One directory read, run once per dispatch, that decides which design step
//! the prompt names — see `DesignStepMatchesTheReposSpecs` in
//! `docs/specs/dispatch.allium`. A repo that keeps specs gets the spec-first
//! sequence (`allium:elicit` → `allium:tend` → `allium:propagate` → implement →
//! `allium:weed`); a repo that keeps none gets `superpowers:brainstorming`
//! instead, and no `allium_instruction` in its trailing block.

use std::path::Path;

use crate::models::expand_tilde;

/// Relative path, from the repo root, where Allium specs live.
const SPEC_DIR: &str = "docs/specs";

/// File extension that marks an Allium spec.
const SPEC_EXT: &str = "allium";

/// True when `repo_path` holds at least one `docs/specs/*.allium` file.
///
/// Read against the parent repo rather than the freshly provisioned worktree:
/// the two are checkouts of the same repository, and the parent path is the one
/// the task actually carries.
///
/// Every failure to *see* such a file answers `false` — a missing `docs/specs`
/// directory, an empty one, one holding no `.allium` file, and one that cannot
/// be read all take the same branch. There is deliberately no error arm, so the
/// prompt an agent receives never depends on which way a read failed.
pub(super) fn repo_has_allium_specs(repo_path: &str) -> bool {
    // An empty path would join to the bare relative `docs/specs`, answering for
    // dispatch's own working directory rather than the task's repo. Dispatch
    // refuses to launch a task with no repo_path at all, so `false` here is the
    // answer for a path that cannot be checked, not a reachable prompt shape.
    if repo_path.is_empty() {
        return false;
    }
    let dir = Path::new(&expand_tilde(repo_path)).join(SPEC_DIR);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return false;
    };
    entries.flatten().any(|e| {
        let path = e.path();
        // `is_file` follows symlinks, so a linked-in spec still counts, while a
        // directory that merely happens to be named `*.allium` does not.
        path.is_file()
            && path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case(SPEC_EXT))
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// Creates `<root>/docs/specs` and returns it.
    fn spec_dir(root: &Path) -> std::path::PathBuf {
        let dir = root.join(SPEC_DIR);
        fs::create_dir_all(&dir).expect("create docs/specs");
        dir
    }

    /// The subject under test, taking the path the way a `Task` carries it.
    fn has_specs(repo: &Path) -> bool {
        repo_has_allium_specs(repo.to_str().expect("utf8 path"))
    }

    #[test]
    fn true_when_docs_specs_holds_an_allium_file() {
        let repo = tempdir().expect("tempdir");
        let dir = spec_dir(repo.path());
        fs::write(dir.join("dispatch.allium"), "-- allium: 3\n").expect("write spec");
        assert!(has_specs(repo.path()));
    }

    #[test]
    fn false_when_docs_specs_is_missing() {
        let repo = tempdir().expect("tempdir");
        assert!(!has_specs(repo.path()));
    }

    #[test]
    fn false_when_docs_specs_is_empty() {
        let repo = tempdir().expect("tempdir");
        spec_dir(repo.path());
        assert!(!has_specs(repo.path()));
    }

    #[test]
    fn false_when_docs_specs_holds_no_allium_file() {
        let repo = tempdir().expect("tempdir");
        let dir = spec_dir(repo.path());
        fs::write(dir.join("README.md"), "not a spec").expect("write file");
        fs::write(dir.join("notes.txt"), "not a spec either").expect("write file");
        assert!(!has_specs(repo.path()));
    }

    /// A spec nested one level deeper is not `docs/specs/*.allium`, and a
    /// directory merely *named* `something.allium` is not a spec file either —
    /// neither flips the answer.
    #[test]
    fn false_when_the_only_allium_entries_are_not_spec_files() {
        let repo = tempdir().expect("tempdir");
        let dir = spec_dir(repo.path());
        fs::create_dir(dir.join("archive")).expect("create subdir");
        fs::write(dir.join("archive/old.allium"), "-- allium: 3\n").expect("write nested spec");
        fs::create_dir(dir.join("named.allium")).expect("create allium-named dir");
        assert!(!has_specs(repo.path()));
    }

    #[test]
    fn false_when_the_repo_path_does_not_exist() {
        let repo = tempdir().expect("tempdir");
        let missing = repo.path().join("no-such-repo");
        assert!(!has_specs(&missing));
    }

    #[test]
    fn false_when_the_repo_path_is_a_file_not_a_directory() {
        let repo = tempdir().expect("tempdir");
        let file = repo.path().join("a-file");
        fs::write(&file, "not a repo").expect("write file");
        assert!(!has_specs(&file));
    }

    #[test]
    fn false_when_the_repo_path_is_empty() {
        assert!(!repo_has_allium_specs(""));
    }

    /// This repository keeps its own specs in `docs/specs`, so a dispatch
    /// against it must take the spec-first branch. Guards the constant pair
    /// against a rename that would silently downgrade every dispatch here.
    #[test]
    fn true_for_this_repository() {
        assert!(repo_has_allium_specs(env!("CARGO_MANIFEST_DIR")));
    }
}
