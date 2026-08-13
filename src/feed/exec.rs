use std::collections::HashMap;

use crate::git::detect_default_branch;
use crate::process::ProcessRunner;

/// Resolve a base branch for each `repo_paths[i]`, caching by unique path so
/// `git symbolic-ref` is invoked at most once per distinct repo. Empty paths
/// (unresolved repos) get `"main"` without shelling out.
pub(crate) fn resolve_base_branches(
    repo_paths: &[String],
    runner: &dyn ProcessRunner,
) -> Vec<String> {
    let mut cache: HashMap<&str, String> = HashMap::new();
    repo_paths
        .iter()
        .map(|path| {
            cache
                .entry(path.as_str())
                .or_insert_with(|| {
                    if path.is_empty() {
                        "main".to_string()
                    } else {
                        detect_default_branch(path, runner)
                    }
                })
                .clone()
        })
        .collect()
}

/// A `ProcessRunner` that always fails — used in tests that only need the
/// `"main"` fallback and don't want to set up git subprocess stubs.
#[cfg(test)]
pub(super) struct AlwaysFailRunner;

#[cfg(test)]
impl ProcessRunner for AlwaysFailRunner {
    fn run(&self, _: &str, _: &[&str]) -> anyhow::Result<std::process::Output> {
        crate::process::MockProcessRunner::fail("not a git repo")
    }
}

/// Maximum number of characters of a feed command's stderr that are logged and
/// carried on [`FeedOutput`]. A verbose script must not be able to write
/// unbounded output into `app.log` on every poll.
pub(crate) const MAX_FEED_STDERR_CHARS: usize = 2000;

/// A successful feed command's output.
#[derive(Debug)]
pub(crate) struct FeedOutput {
    /// Raw stdout bytes, to be parsed as a FeedItem JSON array.
    pub(crate) stdout: Vec<u8>,
    /// Anything the command wrote to stderr while still exiting 0 — trimmed
    /// and truncated to [`MAX_FEED_STDERR_CHARS`]. Empty when it wrote
    /// nothing. A non-empty value is the signature of a script that swallowed
    /// an internal error and emitted a degraded array anyway, so it is logged
    /// here and surfaced by the manual-refresh path (feeds.allium:
    /// FeedCommandStderrOnSuccess).
    pub(crate) stderr: String,
}

/// Execute the feed shell command.
///
/// On success returns stdout plus any stderr the command wrote while exiting 0,
/// logging a warning for that stderr — it is a diagnostic, not a failure, and
/// the caller syncs the emission as normal.
///
/// On spawn failure or non-zero exit, logs a warning (as before) and returns
/// `Err` with the failure text, so a caller that must surface the error to the
/// user — `exec_trigger_epic_feed`'s status bar — has it.
pub(crate) async fn exec_feed_command(
    cmd: &str,
    epic_id: i64,
    epic_title: &str,
) -> Result<FeedOutput, String> {
    let output = match tokio::process::Command::new("sh")
        .args(["-c", cmd])
        .output()
        .await
    {
        Ok(o) => o,
        Err(err) => {
            let msg = format!("{err:#}");
            tracing::warn!(epic_id, epic_title, "feed: failed to spawn command: {msg}");
            return Err(msg);
        }
    };

    let stderr = truncate_stderr(&output.stderr);

    if !output.status.success() {
        tracing::warn!(
            epic_id,
            epic_title,
            "feed: command exited non-zero: {stderr}"
        );
        return Err(stderr);
    }

    if !stderr.is_empty() {
        tracing::warn!(
            epic_id,
            epic_title,
            "feed: command wrote to stderr but exited 0: {stderr}"
        );
    }

    Ok(FeedOutput {
        stdout: output.stdout,
        stderr,
    })
}

/// Classify a zero-exit emission that produced no items while writing to
/// stderr. That combination is the signature of a script which failed
/// internally and soft-failed to an empty array — syncing it would delete
/// every feed task in the epic's subtree.
///
/// Returns `Some(reason)` to suppress the sync, `None` to proceed.
///
/// A genuinely-empty clean run (`stderr` empty) returns `None` and reconciles
/// normally, so merged and closed PRs are still removed. A non-empty emission
/// returns `None` here regardless of stderr — it is never suppressed, only
/// downgraded to an additive sync by [`degraded_partial_emission`].
///
/// See feeds.allium: DegradedEmptyEmission.
pub(crate) fn degraded_empty_emission(item_count: usize, stderr: &str) -> Option<String> {
    if item_count == 0 && !stderr.is_empty() {
        Some(format!(
            "command emitted no items but wrote to stderr: {stderr}"
        ))
    } else {
        None
    }
}

/// Classify a zero-exit emission that produced items while ALSO writing to
/// stderr. That combination is a PARTIALLY degraded emission: part of what the
/// command was asked to fetch came back and part of it soft-failed, so the
/// emission is evidence of what exists but not of what does not.
///
/// Returns `Some(reason)` to downgrade the sync to [`SyncMode::Additive`],
/// `None` to reconcile as normal.
///
/// Deliberately the same shape as [`degraded_empty_emission`], and mutually
/// exclusive with it by the `item_count` arm — the empty case suppresses the
/// sync entirely, this one only withholds its removals.
///
/// See feeds.allium: DegradedNonEmptyEmission.
///
/// [`SyncMode::Additive`]: super::SyncMode::Additive
pub(crate) fn degraded_partial_emission(item_count: usize, stderr: &str) -> Option<String> {
    if item_count > 0 && !stderr.is_empty() {
        Some(format!(
            "command wrote to stderr, so its omissions are not trusted: {stderr}"
        ))
    } else {
        None
    }
}

/// Decode stderr bytes lossily, trim surrounding whitespace, and cap the
/// length at [`MAX_FEED_STDERR_CHARS`]. Truncates on a character boundary, so
/// multi-byte output cannot panic here.
fn truncate_stderr(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim();
    trimmed.chars().take(MAX_FEED_STDERR_CHARS).collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use super::*;
    use crate::process::{MockProcessRunner, ProcessRunner};

    struct FixedBranchRunner(std::collections::HashMap<String, String>);

    impl FixedBranchRunner {
        fn new(pairs: &[(&str, &str)]) -> Self {
            Self(
                pairs
                    .iter()
                    .map(|(p, b)| (p.to_string(), b.to_string()))
                    .collect(),
            )
        }
    }

    impl ProcessRunner for FixedBranchRunner {
        fn run(&self, _program: &str, args: &[&str]) -> anyhow::Result<std::process::Output> {
            let path = args.get(1).copied().unwrap_or("");
            match self.0.get(path) {
                Some(branch) => MockProcessRunner::ok_with_stdout(
                    format!("refs/remotes/origin/{branch}\n").as_bytes(),
                ),
                None => MockProcessRunner::fail("unknown repo"),
            }
        }
    }

    struct CountingRunner(Arc<AtomicUsize>);

    impl ProcessRunner for CountingRunner {
        fn run(&self, _: &str, _: &[&str]) -> anyhow::Result<std::process::Output> {
            self.0.fetch_add(1, Ordering::SeqCst);
            MockProcessRunner::ok_with_stdout(b"refs/remotes/origin/main\n")
        }
    }

    #[test]
    fn empty_path_resolves_to_main_without_calling_runner() {
        let paths = vec!["".to_string(), "".to_string()];
        let branches = resolve_base_branches(&paths, &AlwaysFailRunner);
        assert_eq!(branches, vec!["main", "main"]);
    }

    #[test]
    fn known_path_resolves_to_configured_branch() {
        let runner = FixedBranchRunner::new(&[("/repo/a", "develop")]);
        let paths = vec!["/repo/a".to_string()];
        let branches = resolve_base_branches(&paths, &runner);
        assert_eq!(branches, vec!["develop"]);
    }

    #[test]
    fn same_path_queried_only_once() {
        let counter = Arc::new(AtomicUsize::new(0));
        let runner = CountingRunner(counter.clone());
        let paths = vec![
            "/repo".to_string(),
            "/repo".to_string(),
            "/repo".to_string(),
        ];
        let _ = resolve_base_branches(&paths, &runner);
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "runner called more than once for the same path"
        );
    }

    #[test]
    fn unknown_path_falls_back_to_main() {
        let paths = vec!["/unknown/repo".to_string()];
        let branches = resolve_base_branches(&paths, &AlwaysFailRunner);
        assert_eq!(branches, vec!["main"]);
    }

    // --- exec_feed_command ---

    #[tokio::test]
    async fn exec_feed_command_returns_stdout_on_success() {
        let out = exec_feed_command("printf 'hello'", 1, "test-epic")
            .await
            .expect("zero-exit command must succeed");
        assert_eq!(out.stdout, b"hello".to_vec());
        assert_eq!(out.stderr, "", "no stderr written, so none reported");
    }

    #[tokio::test]
    async fn exec_feed_command_returns_err_on_nonzero_exit() {
        let result = exec_feed_command("exit 1", 2, "test-epic").await;
        assert!(result.is_err(), "non-zero exit must be an error");
    }

    #[tokio::test]
    async fn exec_feed_command_err_carries_stderr_of_failed_command() {
        let err = exec_feed_command("echo 'error msg' >&2; exit 1", 3, "test-epic")
            .await
            .expect_err("non-zero exit must be an error");
        assert!(
            err.contains("error msg"),
            "error must carry the command's stderr, got: {err}"
        );
    }

    #[tokio::test]
    async fn exec_feed_command_returns_empty_stdout_on_zero_exit_with_no_output() {
        let out = exec_feed_command("true", 4, "test-epic")
            .await
            .expect("zero-exit command must succeed");
        assert!(out.stdout.is_empty(), "no stdout written");
        assert_eq!(out.stderr, "");
    }

    // The regression this whole change exists for: a command that fails
    // internally, writes the reason to stderr, and STILL exits 0 with a valid
    // (empty) JSON array. Previously the stderr was captured and discarded.
    #[tokio::test]
    async fn exec_feed_command_captures_stderr_written_on_zero_exit() {
        let out = exec_feed_command(
            "echo 'Invalid search query' >&2; printf '[]'",
            5,
            "test-epic",
        )
        .await
        .expect("zero exit must still succeed");
        assert_eq!(out.stdout, b"[]".to_vec(), "stdout must be untouched");
        assert_eq!(
            out.stderr, "Invalid search query",
            "stderr written on a zero exit must be reported, trimmed"
        );
    }

    #[tokio::test]
    async fn exec_feed_command_truncates_long_stderr() {
        // 3000 'x' characters on stderr, valid empty array on stdout.
        let out = exec_feed_command(
            "printf '%3000s' '' | tr ' ' x >&2; printf '[]'",
            6,
            "test-epic",
        )
        .await
        .expect("zero exit must still succeed");
        assert_eq!(
            out.stderr.chars().count(),
            MAX_FEED_STDERR_CHARS,
            "stderr must be truncated to the cap"
        );
    }

    // truncate_stderr's doc comment promises a character-boundary-safe
    // truncation. 3000 ASCII bytes alone can't distinguish that from a naive
    // `&text[..2000]` byte slice, which would panic on multi-byte input. This
    // drives well over the cap with a repeated non-ASCII character so a
    // byte-slicing regression panics this test instead of shipping.
    #[tokio::test]
    async fn exec_feed_command_truncates_multi_byte_stderr_without_panicking() {
        // 'é' is 2 bytes in UTF-8; "yes é" repeats "é\n" (3 bytes), so
        // `head -c 9000` yields exactly 3000 'é' characters once newlines
        // are stripped — well over the 2000-character cap. A naive
        // `&text[..2000]` byte slice would land mid-character and panic.
        let out = exec_feed_command(
            "yes é 2>/dev/null | head -c 9000 | tr -d '\\n' >&2; printf '[]'",
            8,
            "test-epic",
        )
        .await
        .expect("zero exit must still succeed");
        assert_eq!(
            out.stderr.chars().count(),
            MAX_FEED_STDERR_CHARS,
            "multi-byte stderr must be truncated to exactly the character cap without panicking"
        );
    }

    #[tokio::test]
    async fn exec_feed_command_reports_stderr_of_exactly_the_cap_complete() {
        // Exactly MAX_FEED_STDERR_CHARS ASCII characters must come back
        // whole, not truncated by one.
        let out = exec_feed_command(
            &format!(
                "printf '%{n}s' '' | tr ' ' x >&2; printf '[]'",
                n = MAX_FEED_STDERR_CHARS
            ),
            9,
            "test-epic",
        )
        .await
        .expect("zero exit must still succeed");
        assert_eq!(
            out.stderr.chars().count(),
            MAX_FEED_STDERR_CHARS,
            "exact-cap stderr must be reported in full"
        );
        assert!(
            out.stderr.chars().all(|c| c == 'x'),
            "exact-cap stderr must be untruncated content"
        );
    }

    // --- degraded_empty_emission (feeds.allium: DegradedEmptyEmission) ---

    #[test]
    fn degraded_when_zero_items_and_stderr_present() {
        let reason = degraded_empty_emission(0, "Invalid search query")
            .expect("zero items plus stderr must be treated as a failure");
        assert!(
            reason.contains("Invalid search query"),
            "reason must carry the stderr so the status bar can show it, got: {reason}"
        );
    }

    #[test]
    fn not_degraded_when_zero_items_and_no_stderr() {
        // A genuinely-empty clean run must still reconcile — this is the
        // false-positive boundary the guard must never cross.
        assert!(degraded_empty_emission(0, "").is_none());
    }

    #[test]
    fn not_degraded_when_items_present_despite_stderr() {
        // A non-empty emission is never SUPPRESSED. It is downgraded to an
        // additive sync by degraded_partial_emission instead — this predicate
        // owns the zero-item arm only.
        assert!(degraded_empty_emission(3, "some warning").is_none());
    }

    // --- degraded_partial_emission (feeds.allium: DegradedNonEmptyEmission) ---

    #[test]
    fn degraded_partial_when_items_present_and_stderr_present() {
        let reason = degraded_partial_emission(3, "fetch-reviews: gh search prs failed")
            .expect("items alongside stderr must downgrade the sync to additive");
        assert!(
            reason.contains("fetch-reviews: gh search prs failed"),
            "reason must carry the stderr so the status bar can show it, got: {reason}"
        );
    }

    #[test]
    fn not_partially_degraded_when_no_stderr() {
        // A clean non-empty emission is fully trusted and still reconciles.
        assert!(degraded_partial_emission(3, "").is_none());
    }

    #[test]
    fn not_partially_degraded_when_zero_items() {
        // The zero-item arm belongs to degraded_empty_emission, which
        // suppresses the sync outright. The two predicates must never both
        // fire for one emission.
        assert!(degraded_partial_emission(0, "some warning").is_none());
        assert!(degraded_partial_emission(0, "").is_none());
    }

    #[test]
    fn the_two_degradation_predicates_are_mutually_exclusive() {
        for count in [0usize, 1, 7] {
            for stderr in ["", "boom"] {
                let empty = degraded_empty_emission(count, stderr).is_some();
                let partial = degraded_partial_emission(count, stderr).is_some();
                assert!(
                    !(empty && partial),
                    "both guards fired for count={count} stderr={stderr:?}"
                );
            }
        }
    }
}
