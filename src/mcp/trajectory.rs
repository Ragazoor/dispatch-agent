use std::path::Path;

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncWrite, AsyncWriteExt};

pub const TRAJECTORIES_SUBDIR: &str = "trajectories";

#[derive(Debug, Serialize)]
pub struct TrajectoryEntry {
    pub timestamp: DateTime<Utc>,
    pub task_id: i64,
    pub method: String,
    pub args: Value,
    pub result: Value,
    pub duration_ms: u64,
}

const SCHEMA_VERSION: &str = "1.0.0";

pub async fn append_entry(data_dir: &Path, entry: &TrajectoryEntry) {
    let trajectories_dir = data_dir.join(TRAJECTORIES_SUBDIR);
    if let Err(e) = tokio::fs::create_dir_all(&trajectories_dir).await {
        tracing::warn!(error = ?e, path = %trajectories_dir.display(), "failed to create trajectories dir");
        return;
    }
    let path = trajectories_dir.join(format!("{}.jsonl", entry.task_id));
    let file = match tokio::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)
        .await
    {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(error = ?e, path = %path.display(), "failed to open trajectory file");
            return;
        }
    };
    #[derive(Serialize)]
    struct WithVersion<'a> {
        schema_version: &'static str,
        #[serde(flatten)]
        entry: &'a TrajectoryEntry,
    }
    let payload = WithVersion {
        schema_version: SCHEMA_VERSION,
        entry,
    };
    match serde_json::to_string(&payload) {
        Ok(mut line) => {
            line.push('\n');
            if let Err(e) = write_and_flush(file, line.as_bytes()).await {
                tracing::warn!(error = ?e, path = %path.display(), "failed to write or flush trajectory entry");
            }
        }
        Err(e) => {
            tracing::warn!(error = ?e, "failed to serialize trajectory entry");
        }
    }
}

/// Writes `line` and flushes before returning. `tokio::fs::File::write_all`
/// completing does not mean the bytes reached the OS — the write can still
/// be sitting in an internal buffer, and `File`'s `Drop` only best-effort
/// (un-awaited) flushes it, racing with anything that reads the file next.
/// An explicit, awaited `flush()` closes that window.
async fn write_and_flush<W: AsyncWrite + Unpin>(mut writer: W, line: &[u8]) -> std::io::Result<()> {
    writer.write_all(line).await?;
    writer.flush().await
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::{json, Value};
    use tempfile::tempdir;

    fn make_entry(method: &str) -> TrajectoryEntry {
        TrajectoryEntry {
            timestamp: Utc::now(),
            task_id: 42,
            method: method.to_string(),
            args: json!({"task_id": 42}),
            result: json!({"content": [{"type": "text", "text": "ok"}]}),
            duration_ms: 10,
        }
    }

    #[tokio::test]
    async fn append_creates_file_with_valid_json_line() {
        let dir = tempdir().unwrap();
        let entry = make_entry("update_task");
        append_entry(dir.path(), &entry).await;
        let content =
            tokio::fs::read_to_string(dir.path().join(TRAJECTORIES_SUBDIR).join("42.jsonl"))
                .await
                .unwrap();
        assert!(!content.is_empty());
        let parsed: Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(parsed["schema_version"], "1.0.0");
        assert_eq!(parsed["task_id"], 42);
        assert_eq!(parsed["method"], "update_task");
        assert_eq!(parsed["duration_ms"], 10);
    }

    #[tokio::test]
    async fn append_adds_second_line() {
        let dir = tempdir().unwrap();
        append_entry(dir.path(), &make_entry("get_task")).await;
        append_entry(dir.path(), &make_entry("list_tasks")).await;
        let content =
            tokio::fs::read_to_string(dir.path().join(TRAJECTORIES_SUBDIR).join("42.jsonl"))
                .await
                .unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        let _: Value = serde_json::from_str(lines[0]).unwrap();
        let _: Value = serde_json::from_str(lines[1]).unwrap();
    }

    // `write_all` completing is not proof the data is durable — only
    // `flush` is. This writer distinguishes the two so the test can assert
    // on the property that actually matters, deterministically, instead of
    // racing real OS thread scheduling (which is what made the original
    // regression only reproduce under full-suite load).
    struct MockWriter {
        flushed: std::sync::Arc<std::sync::atomic::AtomicBool>,
        flush_error: Option<&'static str>,
    }

    impl AsyncWrite for MockWriter {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            src: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::task::Poll::Ready(Ok(src.len()))
        }

        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            self.flushed
                .store(true, std::sync::atomic::Ordering::SeqCst);
            match self.flush_error {
                Some(msg) => std::task::Poll::Ready(Err(std::io::Error::other(msg))),
                None => std::task::Poll::Ready(Ok(())),
            }
        }

        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn write_and_flush_flushes_before_returning() {
        let flushed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let writer = MockWriter {
            flushed: flushed.clone(),
            flush_error: None,
        };
        write_and_flush(writer, b"{}\n").await.unwrap();
        assert!(
            flushed.load(std::sync::atomic::Ordering::SeqCst),
            "append_entry must flush before returning, or a buffered write can be lost \
             when the file is dropped and reopened elsewhere"
        );
    }

    #[tokio::test]
    async fn write_and_flush_propagates_flush_error_even_though_write_succeeded() {
        let writer = MockWriter {
            flushed: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            flush_error: Some("flush failed"),
        };
        let err = write_and_flush(writer, b"{}\n").await.unwrap_err();
        assert_eq!(err.to_string(), "flush failed");
    }

    #[tokio::test]
    async fn append_auto_creates_trajectories_dir() {
        let dir = tempdir().unwrap();
        let entry = TrajectoryEntry {
            task_id: 7,
            ..make_entry("get_task")
        };
        append_entry(dir.path(), &entry).await;
        assert!(dir
            .path()
            .join(TRAJECTORIES_SUBDIR)
            .join("7.jsonl")
            .exists());
    }

    #[tokio::test]
    async fn different_tasks_get_separate_files() {
        let dir = tempdir().unwrap();
        let e1 = TrajectoryEntry {
            task_id: 1,
            ..make_entry("get_task")
        };
        let e2 = TrajectoryEntry {
            task_id: 2,
            ..make_entry("list_tasks")
        };
        append_entry(dir.path(), &e1).await;
        append_entry(dir.path(), &e2).await;
        assert!(dir
            .path()
            .join(TRAJECTORIES_SUBDIR)
            .join("1.jsonl")
            .exists());
        assert!(dir
            .path()
            .join(TRAJECTORIES_SUBDIR)
            .join("2.jsonl")
            .exists());
    }

    #[tokio::test]
    async fn fields_round_trip_correctly() {
        let dir = tempdir().unwrap();
        let entry = make_entry("get_task");
        let expected_ts = entry.timestamp;
        append_entry(dir.path(), &entry).await;
        let content =
            tokio::fs::read_to_string(dir.path().join(TRAJECTORIES_SUBDIR).join("42.jsonl"))
                .await
                .unwrap();
        let parsed: Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(parsed["schema_version"], SCHEMA_VERSION);
        assert_eq!(parsed["task_id"], 42);
        assert_eq!(parsed["method"], "get_task");
        assert_eq!(parsed["args"], json!({"task_id": 42}));
        assert_eq!(parsed["duration_ms"], 10);
        let ts_str = parsed["timestamp"].as_str().unwrap();
        let parsed_ts = chrono::DateTime::parse_from_rfc3339(ts_str).unwrap();
        assert_eq!(
            parsed_ts.timestamp_nanos_opt(),
            expected_ts.timestamp_nanos_opt()
        );
    }
}
