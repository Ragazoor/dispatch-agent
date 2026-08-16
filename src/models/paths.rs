//! Path and repo-identity utilities shared across the domain model.
//!
//! The repo-grouping family lives here as one unit — both halves must agree on
//! [`UNKNOWN_REPO_GROUP`], so splitting them across a layer boundary would put
//! the shared fallback one module away from one of its users. Turning a
//! resolved repo name into a *local filesystem path* is a different job and
//! stays in the dispatch adapter (`resolve_repo_path`), which is the only
//! thing here that needs to know what is on disk.

/// Expand a leading `~` or `~/` to the user's home directory.
/// Returns the path unchanged if it doesn't start with `~` or `$HOME` is unset.
pub fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return format!("{}/{rest}", home.to_string_lossy());
        }
    } else if path == "~" {
        if let Some(home) = std::env::var_os("HOME") {
            return home.to_string_lossy().into_owned();
        }
    }
    path.to_string()
}

/// Grouping key for anything whose repo cannot be determined.
pub const UNKNOWN_REPO_GROUP: &str = "other";

/// Derive a repo-grouping key from a filesystem repo path: the final path
/// component (basename). Empty / root paths fall back to
/// [`UNKNOWN_REPO_GROUP`], matching the URL-based grouping used by feeds.
pub fn repo_name_from_path(repo_path: &str) -> String {
    std::path::Path::new(repo_path.trim_end_matches('/'))
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(UNKNOWN_REPO_GROUP)
        .to_string()
}

/// Extract the short repo name (e.g. `"my-repo"`) from a GitHub URL — the
/// URL-side twin of [`repo_name_from_path`], sharing its fallback.
///
/// Returns [`UNKNOWN_REPO_GROUP`] for non-GitHub URLs, empty strings, and malformed input.
pub fn repo_name_from_url(url: &str) -> String {
    extract_github_repo(url)
        .and_then(|s| s.split('/').next_back())
        .unwrap_or(UNKNOWN_REPO_GROUP)
        .to_string()
}

/// Extract `"org/repo"` from a GitHub URL.
///
/// Handles `https://github.com/org/repo`, `.../pull/N`, `.../issues/N`,
/// `.../tree/...`, and similar paths — any URL whose host is `github.com`.
/// Returns `None` for non-GitHub URLs, empty strings, and single-segment paths.
pub fn extract_github_repo(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("https://github.com/")?;
    let rest = rest.trim_end_matches('/');
    // Need at least two path segments: "org/repo[/...]"
    let slash = rest.find('/')?;
    let after_org = &rest[slash + 1..];
    if after_org.is_empty() {
        return None;
    }
    let end = after_org.find('/').unwrap_or(after_org.len());
    let repo = &after_org[..end];
    if repo.is_empty() {
        return None;
    }
    Some(&rest[..slash + 1 + end])
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    // --- repo_name_from_url tests ---

    #[test]
    fn repo_name_from_url_extracts_last_segment() {
        assert_eq!(
            repo_name_from_url("https://github.com/org/my-repo/pull/42"),
            "my-repo"
        );
        assert_eq!(
            repo_name_from_url("https://github.com/org/my-repo"),
            "my-repo"
        );
        assert_eq!(
            repo_name_from_url("https://github.com/org/another-repo/issues/1"),
            "another-repo"
        );
    }

    #[test]
    fn repo_name_from_url_returns_other_for_non_github() {
        assert_eq!(repo_name_from_url(""), "other");
        assert_eq!(
            repo_name_from_url("https://example.com/not-github"),
            "other"
        );
        assert_eq!(repo_name_from_url("https://gitlab.com/org/repo"), "other");
    }

    // --- extract_github_repo tests ---

    #[test]
    fn extract_github_repo_pr_url() {
        assert_eq!(
            extract_github_repo("https://github.com/org/repo/pull/42"),
            Some("org/repo"),
        );
    }

    #[test]
    fn extract_github_repo_issue_url() {
        assert_eq!(
            extract_github_repo("https://github.com/org/repo/issues/5"),
            Some("org/repo"),
        );
    }

    #[test]
    fn extract_github_repo_root_url() {
        assert_eq!(
            extract_github_repo("https://github.com/org/repo"),
            Some("org/repo"),
        );
    }

    #[test]
    fn extract_github_repo_root_url_with_trailing_slash() {
        assert_eq!(
            extract_github_repo("https://github.com/org/repo/"),
            Some("org/repo"),
        );
    }

    #[test]
    fn extract_github_repo_tree_url() {
        assert_eq!(
            extract_github_repo("https://github.com/org/repo/tree/main"),
            Some("org/repo"),
        );
    }

    #[test]
    fn extract_github_repo_non_github_url() {
        assert_eq!(
            extract_github_repo("https://jira.company.com/browse/PROJ-123"),
            None
        );
    }

    #[test]
    fn extract_github_repo_empty_string() {
        assert_eq!(extract_github_repo(""), None);
    }

    #[test]
    fn extract_github_repo_only_one_segment() {
        assert_eq!(extract_github_repo("https://github.com/org"), None);
    }

    #[test]
    fn extract_github_repo_malformed_url() {
        assert_eq!(extract_github_repo("not-a-url"), None);
    }

    #[test]
    fn repo_name_from_path_uses_basename() {
        assert_eq!(repo_name_from_path("/home/u/dispatch"), "dispatch");
        assert_eq!(repo_name_from_path("/home/u/dispatch/"), "dispatch");
        assert_eq!(repo_name_from_path(""), UNKNOWN_REPO_GROUP);
        assert_eq!(repo_name_from_path("/"), UNKNOWN_REPO_GROUP);
    }

    #[test]
    fn expand_tilde_with_path() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(
            expand_tilde("~/projects/foo"),
            format!("{home}/projects/foo")
        );
    }

    #[test]
    fn expand_tilde_bare() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(expand_tilde("~"), home);
    }

    #[test]
    fn expand_tilde_absolute_unchanged() {
        assert_eq!(expand_tilde("/home/user/foo"), "/home/user/foo");
    }
}
