#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;

// ---------------------------------------------------------------------------
// Local helpers
// ---------------------------------------------------------------------------

async fn create_task_in_repo(state: &Arc<McpState>, repo: &str) -> crate::models::TaskId {
    state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Test task",
            description: "",
            repo_path: repo,
            plan: None,
            status: crate::models::TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
            phoenix: false,
        })
        .await
        .unwrap()
}

/// Create an approved learning with a stub embedding (vec![0.1; 384]) so
/// it is visible to the RAG pipeline in `handle_query_learnings`.
async fn create_approved_learning(
    state: &Arc<McpState>,
    summary: &str,
    scope: crate::models::LearningScope,
    scope_ref: Option<&str>,
    tags: &[&str],
) -> crate::models::LearningId {
    let tag_strings: Vec<String> = tags.iter().map(|s| s.to_string()).collect();
    // Store the same stub vector that EmbeddingService::new_test() returns so
    // the RAG cosine similarity is 1.0 (well above the 0.25 threshold).
    let stub_emb: Vec<u8> = serialize_embedding(&vec![0.1f32; 384]);
    let id = state
        .db
        .create_learning(CreateLearningRow {
            kind: crate::models::LearningKind::Convention,
            summary,
            detail: None,
            scope,
            scope_ref,
            tags: &tag_strings,
            source_task_id: None,
            embedding: Some(&stub_emb),
        })
        .await
        .unwrap();
    state
        .db
        .patch_learning(
            id,
            &crate::db::LearningPatch::new().status(crate::models::LearningStatus::Approved),
        )
        .await
        .unwrap();
    id
}

/// Create an approved user-scoped learning and record a prompt-injection
/// retrieval for `task_id`, so it satisfies `rate_learning`'s retrieval guard.
async fn create_retrieved_learning(
    state: &Arc<McpState>,
    task_id: crate::models::TaskId,
    summary: &str,
) -> crate::models::LearningId {
    let learning_id = create_approved_learning(
        state,
        summary,
        crate::models::LearningScope::User,
        None,
        &[],
    )
    .await;
    state
        .db
        .record_retrieval(
            task_id,
            learning_id,
            crate::models::RetrievalSource::PromptInjection,
        )
        .await
        .unwrap();
    learning_id
}

// --- record_learning ---------------------------------------------------------

#[tokio::test]
async fn record_learning_creates_proposed_entry() {
    let state = test_state().await;
    let task_id = create_task_in_repo(&state, "/repo/foo").await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "record_learning",
            "arguments": {
                "task_id": task_id.0,
                "kind": "convention",
                "summary": "Always use cargo fmt before committing",
                "scope": "repo",
                "scope_ref": "/repo/foo"
            }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
    let text = extract_response_text(&resp);
    assert!(text.contains("active"), "expected 'active' in: {text}");

    let filter = crate::db::LearningFilter {
        status: Some(crate::models::LearningStatus::Approved),
        ..Default::default()
    };
    let learnings = state.db.list_learnings(filter).await.unwrap();
    assert_eq!(learnings.len(), 1);
    assert_eq!(
        learnings[0].summary,
        "Always use cargo fmt before committing"
    );
    assert_eq!(learnings[0].scope, crate::models::LearningScope::Repo);
    assert_eq!(learnings[0].source_task_id, Some(task_id));
}

#[tokio::test]
async fn record_learning_derives_scope_ref_for_repo() {
    let state = test_state().await;
    let task_id = create_task_in_repo(&state, "/repo/bar").await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "record_learning",
            "arguments": {
                "task_id": task_id.0,
                "kind": "pitfall",
                "summary": "Watch out for integer overflow",
                "scope": "repo"
            }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);

    let filter = crate::db::LearningFilter {
        status: Some(crate::models::LearningStatus::Approved),
        ..Default::default()
    };
    let learnings = state.db.list_learnings(filter).await.unwrap();
    assert_eq!(learnings.len(), 1);
    assert_eq!(learnings[0].scope_ref.as_deref(), Some("/repo/bar"));
}

#[tokio::test]
async fn record_learning_derives_scope_ref_for_epic() {
    let state = test_state().await;
    let epic = state.db_write().create_epic("E", "", None).await.unwrap();
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "T",
            description: "",
            repo_path: "/r",
            plan: None,
            status: crate::models::TaskStatus::Backlog,
            base_branch: "main",
            epic_id: Some(epic.id),
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
            phoenix: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "record_learning",
            "arguments": {
                "task_id": task_id.0,
                "kind": "convention",
                "summary": "Epic-level outcome",
                "scope": "epic"
            }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);

    let filter = crate::db::LearningFilter {
        status: Some(crate::models::LearningStatus::Approved),
        ..Default::default()
    };
    let learnings = state.db.list_learnings(filter).await.unwrap();
    assert_eq!(learnings.len(), 1);
    assert_eq!(
        learnings[0].scope_ref.as_deref(),
        Some(epic.id.0.to_string().as_str())
    );
}

#[tokio::test]
async fn record_learning_epic_scope_no_epic_fails() {
    let state = test_state().await;
    let task_id = create_task_in_repo(&state, "/repo/baz").await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "record_learning",
            "arguments": {
                "task_id": task_id.0,
                "kind": "convention",
                "summary": "Epic outcome but no epic",
                "scope": "epic"
            }
        })),
    )
    .await;
    assert_error(&resp, "epic");
}

#[tokio::test]
async fn record_learning_user_scope_no_scope_ref() {
    let state = test_state().await;
    let task_id = create_task_in_repo(&state, "/repo/foo").await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "record_learning",
            "arguments": {
                "task_id": task_id.0,
                "kind": "preference",
                "summary": "I prefer verbose variable names",
                "scope": "user"
            }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);

    let filter = crate::db::LearningFilter {
        status: Some(crate::models::LearningStatus::Approved),
        ..Default::default()
    };
    let learnings = state.db.list_learnings(filter).await.unwrap();
    assert_eq!(learnings.len(), 1);
    assert!(learnings[0].scope_ref.is_none());
}

#[tokio::test]
async fn record_learning_empty_summary_fails() {
    let state = test_state().await;
    let task_id = create_task_in_repo(&state, "/repo").await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "record_learning",
            "arguments": {
                "task_id": task_id.0,
                "kind": "pitfall",
                "summary": "   ",
                "scope": "user"
            }
        })),
    )
    .await;
    assert_error(&resp, "summary");
}

#[tokio::test]
async fn record_learning_unknown_task_fails() {
    let state = test_state().await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "record_learning",
            "arguments": {
                "task_id": 9999,
                "kind": "pitfall",
                "summary": "Some learning",
                "scope": "user"
            }
        })),
    )
    .await;
    assert_error(&resp, "9999");
}

// --- record_learning: internal-code citation rejection -----------------------

#[tokio::test]
async fn record_learning_rejects_code_citation_in_summary() {
    let state = test_state().await;
    let task_id = create_task_in_repo(&state, "/repo/foo").await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "record_learning",
            "arguments": {
                "task_id": task_id.0,
                "kind": "convention",
                "summary": "A step goes in src/feed/cycle.rs::run_feed_cycle.",
                "scope": "user"
            }
        })),
    )
    .await;
    assert_error(&resp, "implementation detail");

    let learnings = state
        .db
        .list_learnings(crate::db::LearningFilter::default())
        .await
        .unwrap();
    assert!(
        learnings.is_empty(),
        "rejected learning must not be created"
    );
}

#[tokio::test]
async fn record_learning_allows_mcp_tool_name_reference() {
    let state = test_state().await;
    let task_id = create_task_in_repo(&state, "/repo/foo").await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "record_learning",
            "arguments": {
                "task_id": task_id.0,
                "kind": "procedural",
                "summary": "Call `query_learnings` before guessing, not after.",
                "scope": "user"
            }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
}

// --- record_learning: similar-entries echo -----------------------------------

#[tokio::test]
async fn record_learning_echoes_similar_approved_entries() {
    let state = test_state().await;
    let task_id = create_task_in_repo(&state, "/repo/foo").await;

    // Pre-seed an approved learning with same (kind=convention, scope=repo, scope_ref=/repo/foo)
    let existing_id = create_approved_learning(
        &state,
        "Always use cargo fmt before committing",
        crate::models::LearningScope::Repo,
        Some("/repo/foo"),
        &[],
    )
    .await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "record_learning",
            "arguments": {
                "task_id": task_id.0,
                "kind": "convention",
                "summary": "Prefer rustfmt over manual formatting",
                "scope": "repo"
            }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);

    let text = extract_response_text(&resp);
    assert!(
        text.contains(&existing_id.0.to_string()),
        "expected existing learning id {} in response: {text}",
        existing_id.0
    );
    // The dedup response echoes similar entries but must NOT suggest a rate call:
    // rate_learning requires a prior retrieval, which dedup matches are not.
    assert!(
        !text.contains("upvote_learning") && !text.contains("rate_learning"),
        "dedup response must not suggest a rate/upvote call: {text}"
    );
}

#[tokio::test]
async fn record_learning_no_echo_when_different_kind() {
    let state = test_state().await;
    let task_id = create_task_in_repo(&state, "/repo/foo").await;

    // Pre-seed an approved convention learning; we will submit a pitfall learning.
    create_approved_learning(
        &state,
        "Watch out for integer overflow",
        crate::models::LearningScope::Repo,
        Some("/repo/foo"),
        &[],
    )
    .await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "record_learning",
            "arguments": {
                "task_id": task_id.0,
                "kind": "pitfall",
                "summary": "Use checked_add to avoid overflow",
                "scope": "repo"
            }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);

    let text = extract_response_text(&resp);
    assert!(
        !text.contains("upvote_learning"),
        "should not suggest upvote_learning when existing entry has a different kind: {text}"
    );
}

#[tokio::test]
async fn record_learning_still_creates_when_similar_exists() {
    let state = test_state().await;
    let task_id = create_task_in_repo(&state, "/repo/foo").await;

    create_approved_learning(
        &state,
        "Existing approved entry",
        crate::models::LearningScope::Repo,
        Some("/repo/foo"),
        &[],
    )
    .await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "record_learning",
            "arguments": {
                "task_id": task_id.0,
                "kind": "convention",
                "summary": "New entry despite similar existing",
                "scope": "repo"
            }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);

    let all = state
        .db
        .list_learnings(crate::db::LearningFilter::default())
        .await
        .unwrap();
    assert_eq!(
        all.len(),
        2,
        "expected pre-existing + newly created learning"
    );
    let new_one = all
        .iter()
        .find(|l| l.summary == "New entry despite similar existing");
    assert!(new_one.is_some(), "newly created learning must exist");
}

#[tokio::test]
async fn record_learning_does_not_echo_itself() {
    // When no pre-existing similar entry exists, the newly created entry must
    // not be echoed as a "similar" entry (it should exclude itself).
    let state = test_state().await;
    let task_id = create_task_in_repo(&state, "/repo/foo").await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "record_learning",
            "arguments": {
                "task_id": task_id.0,
                "kind": "convention",
                "summary": "A brand new learning with no prior similar entries",
                "scope": "repo"
            }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);

    let text = extract_response_text(&resp);
    assert!(
        !text.contains("upvote_learning"),
        "should not suggest upvote_learning when no pre-existing similar entries: {text}"
    );
}

// --- query_learnings ---------------------------------------------------------

#[tokio::test]
async fn query_learnings_returns_approved_for_task() {
    let state = test_state().await;
    let task_id = create_task_in_repo(&state, "/repo/myproject").await;
    create_approved_learning(
        &state,
        "Use anyhow for errors",
        crate::models::LearningScope::Repo,
        Some("/repo/myproject"),
        &[],
    )
    .await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "query_learnings",
            "arguments": { "task_id": task_id.0 }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
    let text = extract_response_text(&resp);
    assert!(
        text.contains("Use anyhow for errors"),
        "expected learning in: {text}"
    );
}

#[tokio::test]
async fn query_learnings_tag_filter_narrows_results() {
    let state = test_state().await;
    let task_id = create_task_in_repo(&state, "/repo/tagged").await;
    create_approved_learning(
        &state,
        "Rust tips",
        crate::models::LearningScope::Repo,
        Some("/repo/tagged"),
        &["rust"],
    )
    .await;
    create_approved_learning(
        &state,
        "Testing tips",
        crate::models::LearningScope::Repo,
        Some("/repo/tagged"),
        &["testing"],
    )
    .await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "query_learnings",
            "arguments": { "task_id": task_id.0, "tag_filter": ["rust"] }
        })),
    )
    .await;
    assert!(resp.error.is_none());
    let text = extract_response_text(&resp);
    assert!(text.contains("Rust tips"), "expected rust learning");
    // With RAG (stub returns identical vectors for all), both learnings score equally
    // but the rust-tagged one gets a soft boost — both still appear (soft filter, not hard).
    // The important check is that the rust-tagged entry is present.
}

#[tokio::test]
async fn query_learnings_respects_limit() {
    let state = test_state().await;
    let task_id = create_task_in_repo(&state, "/repo/limited").await;
    for i in 0..5 {
        create_approved_learning(
            &state,
            &format!("Learning {i}"),
            crate::models::LearningScope::Repo,
            Some("/repo/limited"),
            &[],
        )
        .await;
    }

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "query_learnings",
            "arguments": { "task_id": task_id.0, "limit": 2 }
        })),
    )
    .await;
    assert!(resp.error.is_none());
    let text = extract_response_text(&resp);
    // Each entry starts with "[<id>]", count those occurrences
    let count = text.matches('[').count();
    assert_eq!(count, 2, "expected exactly 2 learnings, got text: {text}");
}

#[tokio::test]
async fn query_learnings_records_a_retrieval_per_returned_id() {
    let state = test_state().await;
    let task_id = create_task_in_repo(&state, "/repo/retrievals").await;
    create_approved_learning(
        &state,
        "First entry",
        crate::models::LearningScope::Repo,
        Some("/repo/retrievals"),
        &[],
    )
    .await;
    create_approved_learning(
        &state,
        "Second entry",
        crate::models::LearningScope::Repo,
        Some("/repo/retrievals"),
        &[],
    )
    .await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "query_learnings",
            "arguments": { "task_id": task_id.0 }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);

    let rows = state.db.list_retrievals_for_task(task_id).await.unwrap();
    let query_rows: Vec<_> = rows
        .iter()
        .filter(|r| r.source == crate::models::RetrievalSource::QueryLearnings)
        .collect();
    assert_eq!(
        query_rows.len(),
        2,
        "expected 2 query_learnings retrievals, got {rows:?}"
    );
}

#[tokio::test]
async fn query_learnings_unknown_task_fails() {
    let state = test_state().await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "query_learnings",
            "arguments": { "task_id": 9999 }
        })),
    )
    .await;
    assert_error(&resp, "9999");
}

// --- rate_learning ----------------------------------------------------------

#[tokio::test]
async fn rate_learning_helped_increments_count() {
    let state = test_state().await;
    let task_id = create_task_in_repo(&state, "/repo").await;
    let learning_id = create_retrieved_learning(&state, task_id, "Useful tip").await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "rate_learning",
            "arguments": { "learning_id": learning_id, "task_id": task_id.0, "verdict": "helped" }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);

    let learning = state.db.get_learning(learning_id).await.unwrap().unwrap();
    assert_eq!(learning.upvote_count, 1);
    assert_eq!(learning.status, crate::models::LearningStatus::Approved);
}

#[tokio::test]
async fn rate_learning_wrong_decrements_upvote_count() {
    let state = test_state().await;
    let task_id = create_task_in_repo(&state, "/repo").await;
    let learning_id = create_retrieved_learning(&state, task_id, "Misleading tip").await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "rate_learning",
            "arguments": { "learning_id": learning_id, "task_id": task_id.0, "verdict": "wrong" }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);

    let learning = state.db.get_learning(learning_id).await.unwrap().unwrap();
    // A `wrong` verdict downvotes; status is unchanged.
    assert_eq!(learning.status, crate::models::LearningStatus::Approved);
    assert_eq!(learning.upvote_count, -1);
}

#[tokio::test]
async fn rate_learning_without_retrieval_is_rejected() {
    let state = test_state().await;
    let task_id = create_task_in_repo(&state, "/repo").await;
    let learning_id = create_approved_learning(
        &state,
        "Never surfaced",
        crate::models::LearningScope::User,
        None,
        &[],
    )
    .await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "rate_learning",
            "arguments": { "learning_id": learning_id, "task_id": task_id.0, "verdict": "helped" }
        })),
    )
    .await;
    assert_error(&resp, "retriev");

    let learning = state.db.get_learning(learning_id).await.unwrap().unwrap();
    assert_eq!(learning.upvote_count, 0, "no upvote on rejected rating");
}

#[tokio::test]
async fn rate_learning_unknown_verdict_rejected() {
    let state = test_state().await;
    let task_id = create_task_in_repo(&state, "/repo").await;
    let learning_id =
        create_approved_learning(&state, "Tip", crate::models::LearningScope::User, None, &[])
            .await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "rate_learning",
            "arguments": { "learning_id": learning_id, "task_id": task_id.0, "verdict": "bogus" }
        })),
    )
    .await;
    assert!(
        is_error(&resp),
        "an unknown verdict string must be rejected, got: {resp:?}"
    );
}

// --- delete_learning ---------------------------------------------------------

#[tokio::test]
async fn delete_learning_success() {
    let state = test_state().await;

    let learning_id = create_approved_learning(
        &state,
        "To be deleted",
        crate::models::LearningScope::User,
        None,
        &[],
    )
    .await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "delete_learning",
            "arguments": { "learning_id": learning_id.0 }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
    let text = extract_response_text(&resp);
    assert!(
        text.contains(&learning_id.0.to_string()),
        "response should confirm deleted id: {text}"
    );

    let remaining = state
        .db
        .list_learnings(crate::db::LearningFilter::default())
        .await
        .unwrap();
    assert!(remaining.is_empty(), "learning must be deleted");
}

#[tokio::test]
async fn delete_learning_not_found_returns_error() {
    let state = test_state().await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "delete_learning",
            "arguments": { "learning_id": 9999 }
        })),
    )
    .await;
    assert_error(&resp, "9999");
}

// --- query_learnings: RAG pipeline ------------------------------------------

#[tokio::test]
async fn query_learnings_uses_query_param_when_provided() {
    let state = test_state().await;
    let task_id = create_task_in_repo(&state, "/repo/rag").await;
    create_approved_learning(
        &state,
        "Always use anyhow for error handling",
        crate::models::LearningScope::Repo,
        Some("/repo/rag"),
        &[],
    )
    .await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "query_learnings",
            "arguments": { "task_id": task_id.0, "query": "how to handle errors" }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
    // The result must be either a text with the learning or the "no learnings" message —
    // both are valid when embedding stubs return identical vectors (cosine threshold may
    // or may not be met depending on threshold). We just verify no crash / no error.
    let result = resp.result.expect("expected result");
    assert!(result.get("content").is_some(), "missing content in result");
}

#[tokio::test]
async fn query_learnings_falls_back_to_task_context_when_no_query() {
    let state = test_state().await;
    let task_id = create_task_in_repo(&state, "/repo/fallback").await;
    create_approved_learning(
        &state,
        "Convention for fallback test",
        crate::models::LearningScope::Repo,
        Some("/repo/fallback"),
        &[],
    )
    .await;

    // No query param — should use task title/description as fallback
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "query_learnings",
            "arguments": { "task_id": task_id.0 }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
    let result = resp.result.expect("expected result");
    assert!(result.get("content").is_some(), "missing content in result");
}

#[tokio::test]
async fn query_learnings_soft_tag_boost_does_not_hard_filter() {
    // The stub embedding service returns vec![0.1; 384] for every embed call.
    // cosine(identical_vec, identical_vec) = 1.0 ≥ threshold (0.25), so both
    // learnings pass. tag_filter provides a soft boost (not a hard filter),
    // so the untagged learning should also appear.
    let state = test_state().await;
    let task_id = create_task_in_repo(&state, "/repo/soft-tag").await;
    let _rust_id = create_approved_learning(
        &state,
        "Use Rust idioms",
        crate::models::LearningScope::Repo,
        Some("/repo/soft-tag"),
        &["rust"],
    )
    .await;
    let _no_tag_id = create_approved_learning(
        &state,
        "General best practice",
        crate::models::LearningScope::Repo,
        Some("/repo/soft-tag"),
        &[],
    )
    .await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "query_learnings",
            "arguments": { "task_id": task_id.0, "tag_filter": ["rust"] }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
    let text = extract_response_text(&resp);
    assert!(
        text.contains("Use Rust idioms"),
        "expected rust-tagged learning in results: {text}"
    );
    assert!(
        text.contains("General best practice"),
        "tag_filter must be soft (boost only), untagged learning should also appear: {text}"
    );
}

// --- tool description --------------------------------------------------------

/// `record_learning`'s validator is deliberately narrow: it rejects only the
/// shapes prose never produces, because no regex separates an internal symbol
/// from a product name. Everything else in the rule — don't name the code, the
/// machine-check triage, the boundary on a procedural entry — lives in prose,
/// in two places: `plugin/skills/learnings/SKILL.md` and this description.
///
/// The skill copy is pinned by tests in `src/setup/plugins.rs`. This pins the
/// other half. The description is the only surface guaranteed to be in front
/// of an agent at the moment it calls the tool — an agent can reach
/// `record_learning` without the `/learnings` skill loaded — and it is long
/// enough to be a standing target for a trim. See docs/specs/learnings.allium:
/// RecordLearningViaMcp.
#[tokio::test]
async fn record_learning_tool_description_carries_the_rule_the_validator_cannot() {
    let state = test_state().await;
    let resp = call(&state, "tools/list", None).await;
    let tools = resp.result.unwrap()["tools"].as_array().unwrap().clone();
    let description = tools
        .iter()
        .find(|t| t["name"] == "record_learning")
        .expect("record_learning must be registered")["description"]
        .as_str()
        .unwrap()
        .to_string();

    for (needle, why) in [
        (
            "must not name the function",
            "the don't-name-the-code rule, which the validator only partly enforces",
        ),
        (
            "failing check",
            "the machine-check triage that routes lintable findings away from prose",
        ),
        (
            "lint rule",
            "the triage's yes-branch, without which the question has no answer",
        ),
        (
            "when to stop",
            "the boundary a procedural entry's detail must state",
        ),
    ] {
        assert!(
            description.contains(needle),
            "record_learning's description must carry {why} — expected {needle:?} in: \
             {description}"
        );
    }
}
