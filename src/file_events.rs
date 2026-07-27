//! Per-task file-events JSONL log — the capture half of the agent file tree
//! panel (see `docs/specs/agent-tree.allium`'s `CaptureFileEvent` rule).
//!
//! Invoked from the `dispatch hook-file-event` CLI subcommand, which the
//! `PostToolUse` half of the shell hook calls for tracked tools
//! (Read/Write/Edit/NotebookEdit). Deliberately independent of
//! `models::HookEventKind` / `record_hook_event` — this module never touches
//! `TaskService` or the database, it only appends to a file.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::io::AsyncWriteExt;

pub const FILE_EVENTS_SUBDIR: &str = "file-events";

/// Path to a task's file-events JSONL log: `<data_dir>/file-events/<task_id>.jsonl`.
/// The one place this layout is defined — both the writer (`append_file_event`)
/// and the `dispatch agent-tree` reader (`src/cli/agent_tree.rs`) call this
/// rather than re-deriving the join themselves.
pub fn file_events_path(data_dir: &Path, task_id: i64) -> PathBuf {
    data_dir
        .join(FILE_EVENTS_SUBDIR)
        .join(format!("{task_id}.jsonl"))
}

const SCHEMA_VERSION: &str = "1.0.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileOperation {
    Read,
    Modified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum TrackedFileTool {
    Read,
    Write,
    Edit,
    NotebookEdit,
}

impl TrackedFileTool {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "Read" => Some(Self::Read),
            "Write" => Some(Self::Write),
            "Edit" => Some(Self::Edit),
            "NotebookEdit" => Some(Self::NotebookEdit),
            _ => None,
        }
    }

    fn operation(self) -> FileOperation {
        match self {
            Self::Read => FileOperation::Read,
            Self::Write | Self::Edit | Self::NotebookEdit => FileOperation::Modified,
        }
    }
}

#[derive(Debug, Serialize)]
struct FileEvent {
    schema_version: &'static str,
    timestamp: DateTime<Utc>,
    task_id: String,
    tool: TrackedFileTool,
    path: String,
    operation: FileOperation,
}

/// Append one [`FileEvent`] line to `<data_dir>/file-events/<task_id>.jsonl`.
///
/// Soft-fails on anything that isn't a clean, tracked, non-empty tool call: an
/// empty `path` or an unrecognised `tool` is silently skipped (no line
/// written, no error), matching this repo's soft-fail-decoding convention. I/O
/// or serialization failures are logged at `warn` level and otherwise
/// swallowed — a dropped event must never disturb the agent's tool call.
pub async fn append_file_event(data_dir: &Path, task_id: i64, tool: &str, path: &str) {
    if path.is_empty() {
        return;
    }
    let Some(tool) = TrackedFileTool::parse(tool) else {
        return;
    };

    let dir = data_dir.join(FILE_EVENTS_SUBDIR);
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        tracing::warn!(error = ?e, path = %dir.display(), "failed to create file-events dir");
        return;
    }
    let file_path = file_events_path(data_dir, task_id);
    let mut file = match tokio::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&file_path)
        .await
    {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(error = ?e, path = %file_path.display(), "failed to open file-events file");
            return;
        }
    };

    let entry = FileEvent {
        schema_version: SCHEMA_VERSION,
        timestamp: Utc::now(),
        task_id: task_id.to_string(),
        operation: tool.operation(),
        tool,
        path: path.to_string(),
    };
    match serde_json::to_string(&entry) {
        Ok(mut line) => {
            line.push('\n');
            // tokio::fs::File buffers writes internally and only issues them to
            // the OS once flushed — without this, the line can still be
            // in-flight when a reader (or this same short-lived process
            // exiting) observes the file.
            let result = match file.write_all(line.as_bytes()).await {
                Ok(()) => file.flush().await,
                Err(e) => Err(e),
            };
            if let Err(e) = result {
                tracing::warn!(error = ?e, path = %file_path.display(), "failed to write file event");
            }
        }
        Err(e) => {
            tracing::warn!(error = ?e, "failed to serialize file event");
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::Value;
    use tempfile::tempdir;

    fn events_file(dir: &Path, task_id: i64) -> std::path::PathBuf {
        dir.join(FILE_EVENTS_SUBDIR)
            .join(format!("{task_id}.jsonl"))
    }

    #[tokio::test]
    async fn append_creates_file_with_valid_json_line() {
        let dir = tempdir().unwrap();
        append_file_event(dir.path(), 42, "Read", "/tmp/foo.rs").await;

        let content = tokio::fs::read_to_string(events_file(dir.path(), 42))
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(parsed["schema_version"], "1.0.0");
        assert_eq!(parsed["task_id"], "42");
        assert_eq!(parsed["tool"], "read");
        assert_eq!(parsed["path"], "/tmp/foo.rs");
        assert_eq!(parsed["operation"], "read");
        assert!(parsed["timestamp"].is_string());
    }

    #[tokio::test]
    async fn write_edit_notebook_edit_all_produce_modified() {
        let dir = tempdir().unwrap();
        for (tool, task_id) in [("Write", 1), ("Edit", 2), ("NotebookEdit", 3)] {
            append_file_event(dir.path(), task_id, tool, "/tmp/x").await;
            let content = tokio::fs::read_to_string(events_file(dir.path(), task_id))
                .await
                .unwrap();
            let parsed: Value = serde_json::from_str(content.trim()).unwrap();
            assert_eq!(parsed["operation"], "modified", "tool={tool}");
        }
    }

    #[tokio::test]
    async fn notebook_edit_tool_serializes_as_snake_case() {
        let dir = tempdir().unwrap();
        append_file_event(dir.path(), 7, "NotebookEdit", "/tmp/nb.ipynb").await;
        let content = tokio::fs::read_to_string(events_file(dir.path(), 7))
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(parsed["tool"], "notebook_edit");
    }

    #[tokio::test]
    async fn append_adds_second_line() {
        let dir = tempdir().unwrap();
        append_file_event(dir.path(), 9, "Read", "/tmp/a").await;
        append_file_event(dir.path(), 9, "Write", "/tmp/b").await;

        let content = tokio::fs::read_to_string(events_file(dir.path(), 9))
            .await
            .unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        let first: Value = serde_json::from_str(lines[0]).unwrap();
        let second: Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(first["path"], "/tmp/a");
        assert_eq!(second["path"], "/tmp/b");
    }

    #[tokio::test]
    async fn append_auto_creates_file_events_dir() {
        let dir = tempdir().unwrap();
        append_file_event(dir.path(), 5, "Read", "/tmp/a").await;
        assert!(dir.path().join(FILE_EVENTS_SUBDIR).is_dir());
    }

    #[tokio::test]
    async fn different_tasks_get_separate_files() {
        let dir = tempdir().unwrap();
        append_file_event(dir.path(), 1, "Read", "/tmp/a").await;
        append_file_event(dir.path(), 2, "Read", "/tmp/b").await;
        assert!(events_file(dir.path(), 1).exists());
        assert!(events_file(dir.path(), 2).exists());
    }

    #[tokio::test]
    async fn empty_path_is_skipped() {
        let dir = tempdir().unwrap();
        append_file_event(dir.path(), 11, "Read", "").await;
        assert!(
            !events_file(dir.path(), 11).exists(),
            "an empty path must not create an events file"
        );
    }

    #[tokio::test]
    async fn unrecognized_tool_is_skipped() {
        let dir = tempdir().unwrap();
        append_file_event(dir.path(), 12, "Bash", "/tmp/a").await;
        append_file_event(dir.path(), 12, "Frobnicate", "/tmp/a").await;
        assert!(
            !events_file(dir.path(), 12).exists(),
            "an unrecognised tool must not create an events file"
        );
    }

    #[tokio::test]
    async fn fields_round_trip_correctly() {
        let dir = tempdir().unwrap();
        append_file_event(dir.path(), 42, "Edit", "/tmp/exact/path.rs").await;
        let content = tokio::fs::read_to_string(events_file(dir.path(), 42))
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(parsed["task_id"], "42");
        assert_eq!(parsed["path"], "/tmp/exact/path.rs");
        assert_eq!(parsed["tool"], "edit");
        assert_eq!(parsed["operation"], "modified");
        let ts_str = parsed["timestamp"].as_str().unwrap();
        chrono::DateTime::parse_from_rfc3339(ts_str).unwrap();
    }
}
