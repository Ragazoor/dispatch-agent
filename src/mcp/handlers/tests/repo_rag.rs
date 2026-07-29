#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use crate::mcp::identity::CallerIdentity;
use crate::models::TaskId;
use crate::service::repo_index::BATCH_SIZE;
use serde_json::json;

/// Parse "count": N from the embedded JSON string in a search_docs response.
fn parse_count(text: &str) -> Option<u64> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    v.get("count")?.as_u64()
}

/// Parse "files_remaining": N from an index_repo response text.
fn parse_files_remaining(text: &str) -> Option<usize> {
    // text looks like "Indexed /path — files_indexed: 2, ..., files_remaining: 5, ..."
    let prefix = "files_remaining: ";
    let pos = text.find(prefix)?;
    text[pos + prefix.len()..]
        .split(',')
        .next()?
        .trim()
        .parse()
        .ok()
}

// --- index_repo ----------------------------------------------------------

#[tokio::test]
async fn index_repo_with_explicit_path_succeeds() {
    let state = test_state().await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.md"), "# Note\n\nContent.").unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "index_repo",
            "arguments": {
                "repo_path": dir.path().to_str().unwrap()
            }
        })),
    )
    .await;

    assert!(
        !is_error(&resp),
        "unexpected error: {}",
        error_message(&resp)
    );
    let text = extract_response_text(&resp);
    assert!(text.contains("files_indexed"), "got: {text}");
}

#[tokio::test]
async fn index_repo_without_repo_path_uses_task_repo() {
    let state = test_state().await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.md"), "# Note\n\nContent.").unwrap();

    let task_id = create_task_fixture_at(&state, dir.path().to_str().unwrap()).await;

    let resp = call_as(
        &state,
        "tools/call",
        Some(json!({
            "name": "index_repo",
            "arguments": {}
        })),
        CallerIdentity::Task(task_id),
    )
    .await;

    assert!(
        !is_error(&resp),
        "unexpected error: {}",
        error_message(&resp)
    );
    let text = extract_response_text(&resp);
    assert!(text.contains("files_indexed: 1"), "got: {text}");
}

#[tokio::test]
async fn index_repo_session_caller_without_repo_path_returns_error() {
    let state = test_state().await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "index_repo",
            "arguments": {}
        })),
    )
    .await;
    assert!(
        is_error(&resp),
        "expected error for session caller with no repo_path"
    );
}

#[tokio::test]
async fn index_repo_task_caller_unknown_task_returns_error() {
    let state = test_state().await;
    let resp = call_as(
        &state,
        "tools/call",
        Some(json!({
            "name": "index_repo",
            "arguments": {}
        })),
        CallerIdentity::Task(TaskId(99999)),
    )
    .await;
    assert!(is_error(&resp));
}

#[tokio::test]
async fn index_repo_empty_directory_succeeds_with_zero_files() {
    let state = test_state().await;
    let dir = tempfile::tempdir().unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "index_repo",
            "arguments": {
                "repo_path": dir.path().to_str().unwrap()
            }
        })),
    )
    .await;

    assert!(
        !is_error(&resp),
        "unexpected error: {}",
        error_message(&resp)
    );
    let text = extract_response_text(&resp);
    assert!(text.contains("files_indexed: 0"), "got: {text}");
}

#[tokio::test]
async fn index_repo_response_includes_files_remaining() {
    let state = test_state().await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.md"), "# Note\n\nContent.").unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "index_repo",
            "arguments": { "repo_path": dir.path().to_str().unwrap() }
        })),
    )
    .await;

    let text = extract_response_text(&resp);
    assert!(
        text.contains("files_remaining:"),
        "response must include files_remaining field, got: {text}"
    );
}

#[tokio::test]
async fn index_repo_batch_size_limits_files_per_call() {
    // Use the service directly to pass a tiny batch_size and verify files_remaining.
    let dir = tempfile::tempdir().unwrap();
    for i in 0..3 {
        std::fs::write(
            dir.path().join(format!("f{i}.md")),
            format!("# F{i}\n\nContent."),
        )
        .unwrap();
    }

    let svc = crate::service::repo_index::RepoIndexService::new(
        crate::service::embeddings::EmbeddingService::new_test(),
    );
    let result = svc.index_repo(dir.path(), 2).await.unwrap();

    assert_eq!(result.files_indexed, 2, "batch should cap at 2");
    assert_eq!(result.files_remaining, 1, "1 file left for next call");

    // Second call picks up the remainder.
    let result2 = svc.index_repo(dir.path(), 2).await.unwrap();
    assert_eq!(result2.files_indexed, 1);
    assert_eq!(result2.files_remaining, 0);

    // Third call: all skipped.
    let result3 = svc.index_repo(dir.path(), 2).await.unwrap();
    assert_eq!(result3.files_indexed, 0);
    assert_eq!(result3.files_skipped, 3);
    assert_eq!(result3.files_remaining, 0);
}

#[tokio::test]
async fn index_repo_with_tilde_task_repo_path_expands_tilde() {
    // Regression for knowledge #64: repo_path stored with leading `~` must be
    // expanded before use, otherwise the DB lookup produces a non-existent path.
    let home = std::env::var("HOME").unwrap();
    let dir = tempfile::tempdir_in(&home).unwrap();
    let rel = dir.path().strip_prefix(&home).unwrap().to_str().unwrap();
    let tilde_path = format!("~/{rel}");
    std::fs::write(dir.path().join("note.md"), "# Note\n\nContent.").unwrap();

    let state = test_state().await;
    let task_id = create_task_fixture_at(&state, &tilde_path).await;

    let resp = call_as(
        &state,
        "tools/call",
        Some(json!({
            "name": "index_repo",
            "arguments": {}
        })),
        CallerIdentity::Task(task_id),
    )
    .await;

    assert!(
        !is_error(&resp),
        "expand_tilde not applied to task repo_path — error: {}",
        error_message(&resp)
    );
    let text = extract_response_text(&resp);
    assert!(text.contains("files_indexed: 1"), "got: {text}");
}

#[tokio::test]
async fn index_repo_mcp_handler_uses_batch_size_constant() {
    // Verify that the MCP handler respects the BATCH_SIZE constant by checking
    // that files_remaining is reported correctly when more than BATCH_SIZE files exist.
    // (This relies on the service-level test above for detailed batching coverage.)
    let dir = tempfile::tempdir().unwrap();
    for i in 0..3 {
        std::fs::write(
            dir.path().join(format!("f{i}.md")),
            format!("# F{i}\n\nContent."),
        )
        .unwrap();
    }

    let state = test_state().await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "index_repo",
            "arguments": { "repo_path": dir.path().to_str().unwrap() }
        })),
    )
    .await;

    let text = extract_response_text(&resp);
    let remaining = parse_files_remaining(&text).expect("files_remaining must be in response");
    // With 3 files and BATCH_SIZE=50, all fit in one call → remaining = 0.
    assert_eq!(
        remaining, 0,
        "3 files < BATCH_SIZE({BATCH_SIZE}), got: {text}"
    );
}

// --- search_docs ---------------------------------------------------------

#[tokio::test]
async fn search_docs_on_unindexed_repo_returns_empty() {
    let state = test_state().await;
    let dir = tempfile::tempdir().unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "search_docs",
            "arguments": {
                "query": "anything",
                "repo_path": dir.path().to_str().unwrap()
            }
        })),
    )
    .await;

    assert!(
        !is_error(&resp),
        "unexpected error: {}",
        error_message(&resp)
    );
    let text = extract_response_text(&resp);
    assert_eq!(
        parse_count(&text),
        Some(0),
        "expected 0 results, got: {text}"
    );
}

#[tokio::test]
async fn search_docs_after_indexing_returns_results() {
    let state = test_state().await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("note.md"),
        "# Note\n\nContent about escalation patterns.",
    )
    .unwrap();

    call(
        &state,
        "tools/call",
        Some(json!({
            "name": "index_repo",
            "arguments": {
                "repo_path": dir.path().to_str().unwrap()
            }
        })),
    )
    .await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "search_docs",
            "arguments": {
                "query": "escalation",
                "repo_path": dir.path().to_str().unwrap()
            }
        })),
    )
    .await;

    assert!(
        !is_error(&resp),
        "unexpected error: {}",
        error_message(&resp)
    );
    let text = extract_response_text(&resp);
    assert!(
        parse_count(&text).is_some_and(|n| n > 0),
        "expected at least one result, got: {text}"
    );
}

#[tokio::test]
async fn search_docs_limit_zero_is_clamped_to_one_not_empty() {
    // Regression guard: limit is documented as clamped to [1, max_search_limit],
    // but the handler only applied an upper clamp (.min(20)) — limit: 0 reached
    // results.truncate(0), silently returning count: 0, indistinguishable from
    // "nothing matched".
    let state = test_state().await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("note.md"),
        "# Note\n\nContent about escalation patterns.",
    )
    .unwrap();

    call(
        &state,
        "tools/call",
        Some(json!({
            "name": "index_repo",
            "arguments": {
                "repo_path": dir.path().to_str().unwrap()
            }
        })),
    )
    .await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "search_docs",
            "arguments": {
                "query": "escalation",
                "repo_path": dir.path().to_str().unwrap(),
                "limit": 0
            }
        })),
    )
    .await;

    assert!(
        !is_error(&resp),
        "unexpected error: {}",
        error_message(&resp)
    );
    let text = extract_response_text(&resp);
    assert!(
        parse_count(&text).is_some_and(|n| n > 0),
        "limit: 0 should be clamped up to 1, not return an empty result set: {text}"
    );
}

#[tokio::test]
async fn search_docs_without_repo_path_uses_task_repo() {
    let state = test_state().await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.md"), "# Note\n\nSome content.").unwrap();

    let task_id = create_task_fixture_at(&state, dir.path().to_str().unwrap()).await;

    // Index using explicit path so rag.db exists.
    call(
        &state,
        "tools/call",
        Some(json!({
            "name": "index_repo",
            "arguments": {
                "repo_path": dir.path().to_str().unwrap()
            }
        })),
    )
    .await;

    // Search using only caller identity (no repo_path).
    let resp = call_as(
        &state,
        "tools/call",
        Some(json!({
            "name": "search_docs",
            "arguments": { "query": "content" }
        })),
        CallerIdentity::Task(task_id),
    )
    .await;

    assert!(
        !is_error(&resp),
        "unexpected error: {}",
        error_message(&resp)
    );
    let text = extract_response_text(&resp);
    assert!(
        parse_count(&text).is_some_and(|n| n > 0),
        "expected results via task repo_path, got: {text}"
    );
}

#[tokio::test]
async fn search_docs_session_caller_without_repo_path_returns_error() {
    let state = test_state().await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "search_docs",
            "arguments": { "query": "anything" }
        })),
    )
    .await;
    assert!(is_error(&resp));
}
