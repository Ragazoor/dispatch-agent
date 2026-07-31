//! The `dispatch statusline` decorator (see docs/specs/dispatch.allium:
//! TokenBudgetIndicator).
//!
//! Wired as the `statusLine` command of every dispatch-spawned Claude session.
//! Records the payload's `rate_limits` to a snapshot file, then runs the user's
//! previous statusLine command and prints its output verbatim.
//!
//! Two hard constraints:
//!
//! 1. **Never fail.** This runs on Claude Code's 300 ms statusLine debounce. Any
//!    error must still exit 0, or the user's status line breaks.
//! 2. **Never open the database.** At several invocations per second per session,
//!    across every agent, database work here would be pure waste. This module has
//!    no `Database` import and must keep it that way.

use crate::models::budget::BudgetSnapshot;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// Wall-clock budget for the chained command. If it has not closed its stdout
/// (i.e. exited or otherwise finished producing output) within this time, it
/// is killed and the chain yields an empty string — consistent with this
/// module's "any failure -> blank status line" philosophy. The real chained
/// command runs several `git -C` invocations, which can block on a lock, NFS,
/// or a network remote, so this bound is load-bearing, not decorative.
const CHAIN_TIMEOUT: Duration = Duration::from_secs(2);

/// Parse the payload and atomically publish a snapshot. Returns whether a
/// snapshot was written. Never panics; every failure is a silent `false`.
pub fn record_snapshot(stdin: &str, snapshot_path: &Path, now: i64) -> bool {
    let Ok(payload) = serde_json::from_str::<serde_json::Value>(stdin) else {
        return false;
    };
    let Some(snapshot) = BudgetSnapshot::from_status_payload(&payload, now) else {
        return false;
    };
    let Ok(text) = serde_json::to_string(&snapshot) else {
        return false;
    };
    write_atomically(snapshot_path, &text)
}

/// Publish `text` at `path` via a **uniquely named** temp file in the same
/// directory, then rename.
///
/// The unique name is load-bearing: every Claude session writes this same path
/// concurrently. With a fixed temp name, writer A could rename bytes that writer
/// B had truncated and partially written, publishing a torn value attributed to
/// the wrong writer. With a unique temp file, each writer only ever renames its
/// own complete bytes, so "last rename wins" is true — and since all writers
/// report the same account-global value, that is the correct outcome.
fn write_atomically(path: &Path, text: &str) -> bool {
    let Some(dir) = path.parent() else {
        return false;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let Ok(mut file) = tempfile::NamedTempFile::new_in(dir) else {
        return false;
    };
    if file.write_all(text.as_bytes()).is_err() {
        return false;
    }
    if file.flush().is_err() {
        return false;
    }
    file.persist(path).is_ok()
}

/// Run the chained command with `stdin` on its stdin, returning its stdout.
/// Any failure — including a timeout — yields an empty string, a blank status
/// line rather than a broken one.
///
/// Two hazards this guards against:
///
/// 1. **A never-exiting child hangs forever.** The chained command (in
///    practice, several `git -C` invocations) can block on a lock, NFS, or a
///    network remote. `timeout` bounds the wait; on expiry the child is
///    killed rather than awaited indefinitely.
/// 2. **Bidirectional-pipe deadlock.** Writing all of `stdin` before draining
///    the child's stdout deadlocks once the payload exceeds the pipe buffer
///    (~64 KiB on Linux) for a command that echoes as it reads (`cat`, `tee`,
///    `jq .`): the child blocks writing to its full, undrained stdout while
///    the parent blocks writing more to the child's full, undrained stdin.
///    Writing stdin from a separate thread lets the parent drain stdout
///    concurrently, so neither side can fill its pipe buffer and stall.
fn run_chain(chain: &str, stdin: &str, timeout: Duration) -> String {
    let spawned = Command::new("sh")
        .arg("-c")
        .arg(chain)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn();
    let Ok(mut child) = spawned else {
        return String::new();
    };

    if let Some(mut pipe) = child.stdin.take() {
        let payload = stdin.to_string();
        // Runs on its own thread so the parent is free to drain stdout at
        // the same time. The pipe is moved in and drops at the end of this
        // closure, closing the child's stdin so it sees EOF. A child that
        // never reads stdin at all is fine — `write_all` erroring is ignored.
        let _ = std::thread::spawn(move || {
            let _ = pipe.write_all(payload.as_bytes());
        });
    }

    let (tx, rx) = mpsc::channel();
    match child.stdout.take() {
        Some(mut out) => {
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = out.read_to_end(&mut buf);
                let _ = tx.send(buf);
            });
        }
        None => {
            let _ = tx.send(Vec::new());
        }
    }

    match rx.recv_timeout(timeout) {
        Ok(buf) => {
            let _ = child.wait();
            String::from_utf8_lossy(&buf).into_owned()
        }
        Err(_) => {
            // Timed out: the child never closed stdout within budget. Kill
            // and reap it so it does not linger as an orphan/zombie.
            let _ = child.kill();
            let _ = child.wait();
            String::new()
        }
    }
}

/// Decorator core: record, then chain. Always returns exit code 0.
pub fn run(stdin: &str, snapshot_path: &Path, chain: Option<&str>, now: i64) -> i32 {
    let out = run_capturing(stdin, snapshot_path, chain, now);
    print!("{out}");
    0
}

/// `run` without the side effect of printing, so tests can assert the output.
pub fn run_capturing(stdin: &str, snapshot_path: &Path, chain: Option<&str>, now: i64) -> String {
    record_snapshot(stdin, snapshot_path, now);
    match chain {
        Some(cmd) if !cmd.trim().is_empty() => run_chain(cmd, stdin, CHAIN_TIMEOUT),
        _ => String::new(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::models::budget::BudgetSnapshot;

    const PAYLOAD: &str = r#"{"rate_limits":{"five_hour":{"used_percentage":23.5,"resets_at":100},"seven_day":{"used_percentage":41.0,"resets_at":200}}}"#;

    fn read_snapshot(path: &Path) -> BudgetSnapshot {
        let text = std::fs::read_to_string(path).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    #[test]
    fn writes_snapshot_from_payload() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rate-limits.json");
        assert!(record_snapshot(PAYLOAD, &path, 42));
        let snap = read_snapshot(&path);
        assert_eq!(snap.five_hour.unwrap().used_percentage, 23.5);
        assert_eq!(snap.captured_at, 42);
    }

    #[test]
    fn creates_missing_parent_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp
            .path()
            .join("nested")
            .join("deeper")
            .join("rate-limits.json");
        assert!(record_snapshot(PAYLOAD, &path, 1));
        assert!(path.exists());
    }

    #[test]
    fn no_rate_limits_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rate-limits.json");
        assert!(!record_snapshot(
            r#"{"model":{"display_name":"Opus"}}"#,
            &path,
            1
        ));
        assert!(!path.exists());
    }

    #[test]
    fn malformed_stdin_writes_nothing_and_does_not_panic() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rate-limits.json");
        assert!(!record_snapshot("not json at all {{{", &path, 1));
        assert!(!path.exists());
    }

    #[test]
    fn empty_stdin_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rate-limits.json");
        assert!(!record_snapshot("", &path, 1));
        assert!(!path.exists());
    }

    #[test]
    fn overwrites_previous_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rate-limits.json");
        record_snapshot(PAYLOAD, &path, 1);
        let newer = r#"{"rate_limits":{"five_hour":{"used_percentage":99.0,"resets_at":100}}}"#;
        record_snapshot(newer, &path, 2);
        let snap = read_snapshot(&path);
        assert_eq!(snap.five_hour.unwrap().used_percentage, 99.0);
        assert_eq!(snap.captured_at, 2);
    }

    #[test]
    fn leaves_no_temp_files_behind() {
        // A fixed temp name would let concurrent writers publish each other's
        // partial bytes; a unique temp file must also not accumulate.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rate-limits.json");
        for i in 0..5 {
            record_snapshot(PAYLOAD, &path, i);
        }
        let entries: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(
            entries.len(),
            1,
            "expected only the snapshot, got {entries:?}"
        );
    }

    #[test]
    fn run_returns_zero_without_chain() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rate-limits.json");
        assert_eq!(run(PAYLOAD, &path, None, 1), 0);
    }

    #[test]
    fn run_returns_zero_when_snapshot_path_unwritable() {
        // /proc is not writable; the decorator must still succeed.
        let path = Path::new("/proc/definitely/not/writable/rate-limits.json");
        assert_eq!(run(PAYLOAD, path, None, 1), 0);
    }

    #[test]
    fn run_returns_zero_when_chained_command_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rate-limits.json");
        assert_eq!(run(PAYLOAD, &path, Some("exit 3"), 1), 0);
    }

    #[test]
    fn run_returns_zero_when_chained_command_does_not_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rate-limits.json");
        assert_eq!(
            run(PAYLOAD, &path, Some("definitely-not-a-real-binary-xyz"), 1),
            0
        );
    }

    #[test]
    fn chained_command_receives_stdin_and_its_stdout_is_returned() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rate-limits.json");
        let out = run_capturing(PAYLOAD, &path, Some("cat"), 1);
        assert_eq!(
            out, PAYLOAD,
            "chained command must receive the payload verbatim"
        );
    }

    #[test]
    fn snapshot_is_written_even_when_chain_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rate-limits.json");
        run(PAYLOAD, &path, Some("exit 1"), 7);
        assert_eq!(read_snapshot(&path).captured_at, 7);
    }

    #[test]
    fn chain_that_never_exits_times_out_and_returns_empty() {
        // Exercises run_chain directly with an injected short timeout so the
        // test does not wait out CHAIN_TIMEOUT's production value of 2s. A
        // chain that never exits (blocked on a lock, NFS, a network remote,
        // or here, `sleep`) must not hang the caller.
        let out = run_chain("sleep 30", PAYLOAD, Duration::from_millis(100));
        assert_eq!(
            out, "",
            "a hung chain command must yield empty output, not hang"
        );
    }

    #[test]
    fn chain_that_echoes_a_large_payload_does_not_deadlock() {
        // Comfortably larger than the ~64 KiB Linux pipe buffer. Padded as a
        // JSON string value so the payload still parses if something were to
        // feed it through record_snapshot, even though this test only
        // exercises run_chain's stdin/stdout plumbing.
        let padding = "x".repeat(200_000);
        let payload = format!(
            r#"{{"rate_limits":{{"five_hour":{{"used_percentage":1.0,"resets_at":1}}}},"padding":"{padding}"}}"#
        );
        let out = run_chain("cat", &payload, Duration::from_secs(5));
        assert_eq!(
            out, payload,
            "a large payload must round-trip through an echoing chain without deadlocking"
        );
    }
}
