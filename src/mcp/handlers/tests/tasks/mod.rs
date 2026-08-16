#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;

mod crud;
mod dispatch;
mod verify;
mod watch;
mod wrap_up;

// =======================================================================
// Epic tool tests
// =======================================================================

#[tokio::test]
async fn create_epic_minimal() {
    let state = test_state().await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_epic",
            "arguments": { "title": "My Epic" }
        })),
    )
    .await;
    let text = extract_response_text(&resp);
    assert!(text.contains("Epic"));
    assert!(text.contains("created"));

    let epics = state.db.list_epics().await.unwrap();
    assert_eq!(epics.len(), 1);
    assert_eq!(epics[0].title, "My Epic");
}

#[tokio::test]
async fn create_epic_with_all_fields() {
    let state = test_state().await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_epic",
            "arguments": {
                "title": "Full Epic",
                "description": "Epic desc"
            }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "{:?}", resp.error);

    let epics = state.db.list_epics().await.unwrap();
    assert_eq!(epics[0].description, "Epic desc");
}

#[tokio::test]
async fn create_epic_missing_title() {
    let state = test_state().await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_epic",
            "arguments": {}
        })),
    )
    .await;
    assert_error(&resp, "Invalid arguments");
}

#[tokio::test]
async fn get_epic_found() {
    let state = test_state().await;
    let epic = state
        .db_write()
        .create_epic("Get Me", "desc", None)
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "get_epic",
            "arguments": { "epic_id": epic.id.0 }
        })),
    )
    .await;
    let text = extract_response_text(&resp);
    assert!(text.contains("Get Me"));
    assert!(text.contains("desc"));
}

#[tokio::test]
async fn get_epic_not_found() {
    let state = test_state().await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "get_epic",
            "arguments": { "epic_id": 9999 }
        })),
    )
    .await;
    assert_error(&resp, "not found");
}

#[tokio::test]
async fn get_epic_shows_subtask_summary() {
    let state = test_state().await;
    let epic = state
        .db_write()
        .create_epic("With Tasks", "", None)
        .await
        .unwrap();
    let t1 = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Sub 1",
            description: "",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Done,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
            schedule_interval_secs: None,
            pinned_branch: None,
        })
        .await
        .unwrap();
    let t2 = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Sub 2",
            description: "",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
            schedule_interval_secs: None,
            pinned_branch: None,
        })
        .await
        .unwrap();
    state
        .db_write()
        .set_task_epic_id(t1, Some(epic.id))
        .await
        .unwrap();
    state
        .db_write()
        .set_task_epic_id(t2, Some(epic.id))
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "get_epic",
            "arguments": { "epic_id": epic.id.0 }
        })),
    )
    .await;
    let text = extract_response_text(&resp);
    assert!(
        text.contains("1/2 done"),
        "expected subtask summary, got: {text}"
    );
}

#[tokio::test]
async fn get_epic_accepts_string_id() {
    let state = test_state().await;
    let epic = state
        .db_write()
        .create_epic("String ID", "", None)
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "get_epic",
            "arguments": { "epic_id": epic.id.0.to_string() }
        })),
    )
    .await;
    assert!(
        resp.error.is_none(),
        "should accept string epic_id: {:?}",
        resp.error
    );
    let text = extract_response_text(&resp);
    assert!(text.contains("String ID"));
}

#[tokio::test]
async fn list_epics_empty() {
    let state = test_state().await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_epics", "arguments": {} })),
    )
    .await;
    let text = extract_response_text(&resp);
    assert!(text.contains("No epics found"));
}

#[tokio::test]
async fn list_epics_with_items() {
    let state = test_state().await;
    state
        .db_write()
        .create_epic("Epic A", "desc a", None)
        .await
        .unwrap();
    state
        .db_write()
        .create_epic("Epic B", "desc b", None)
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_epics", "arguments": {} })),
    )
    .await;
    let text = extract_response_text(&resp);
    assert!(text.contains("Epic A"));
    assert!(text.contains("Epic B"));
}

#[tokio::test]
async fn list_epics_shows_subtask_counts() {
    let state = test_state().await;
    let epic = state
        .db_write()
        .create_epic("Tracked", "", None)
        .await
        .unwrap();
    let t1 = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Done",
            description: "",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Done,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
            schedule_interval_secs: None,
            pinned_branch: None,
        })
        .await
        .unwrap();
    let t2 = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Pending",
            description: "",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
            schedule_interval_secs: None,
            pinned_branch: None,
        })
        .await
        .unwrap();
    state
        .db_write()
        .set_task_epic_id(t1, Some(epic.id))
        .await
        .unwrap();
    state
        .db_write()
        .set_task_epic_id(t2, Some(epic.id))
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_epics", "arguments": {} })),
    )
    .await;
    let text = extract_response_text(&resp);
    assert!(
        text.contains("1/2 done"),
        "expected subtask counts, got: {text}"
    );
}

#[tokio::test]
async fn update_epic_title() {
    let state = test_state().await;
    let epic = state
        .db_write()
        .create_epic("Old Title", "", None)
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_epic",
            "arguments": { "epic_id": epic.id.0, "title": "New Title" }
        })),
    )
    .await;
    let text = extract_response_text(&resp);
    assert!(text.contains("updated"));
    assert!(text.contains("title"));

    let updated = state.db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(updated.title, "New Title");
}

#[tokio::test]
async fn update_epic_mark_done() {
    let state = test_state().await;
    let epic = state
        .db_write()
        .create_epic("To Finish", "", None)
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_epic",
            "arguments": { "epic_id": epic.id.0, "status": "done" }
        })),
    )
    .await;
    let text = extract_response_text(&resp);
    assert!(
        text.contains("status"),
        "response should mention status field: {text}"
    );

    let updated = state.db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(updated.status, crate::models::TaskStatus::Done);
}

#[tokio::test]
async fn update_epic_multiple_fields() {
    let state = test_state().await;
    let epic = state
        .db_write()
        .create_epic("Old", "old desc", None)
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_epic",
            "arguments": {
                "epic_id": epic.id.0,
                "title": "New",
                "description": "new desc"
            }
        })),
    )
    .await;
    assert!(resp.error.is_none());

    let updated = state.db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(updated.title, "New");
    assert_eq!(updated.description, "new desc");
}

#[tokio::test]
async fn update_epic_accepts_string_id() {
    let state = test_state().await;
    let epic = state
        .db_write()
        .create_epic("Str Epic", "", None)
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_epic",
            "arguments": { "epic_id": epic.id.0.to_string(), "title": "Updated" }
        })),
    )
    .await;
    assert!(
        resp.error.is_none(),
        "should accept string epic_id: {:?}",
        resp.error
    );
}

#[tokio::test]
async fn update_epic_plan() {
    let state = test_state().await;
    let epic = state
        .db_write()
        .create_epic("Planned Epic", "", None)
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_epic",
            "arguments": { "epic_id": epic.id.0, "plan_path": "docs/plans/epic-plan.md" }
        })),
    )
    .await;
    let text = extract_response_text(&resp);
    assert!(
        text.contains("plan"),
        "response should mention plan: {text}"
    );

    let updated = state.db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(
        updated.plan_path.as_deref(),
        Some("docs/plans/epic-plan.md")
    );
}
#[tokio::test]
async fn update_epic_no_fields_errors() {
    let state = test_state().await;
    let epic = state
        .db_write()
        .create_epic("Test", "", None)
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_epic",
            "arguments": { "epic_id": epic.id.0 }
        })),
    )
    .await;
    assert_error(&resp, "At least one");
}

#[tokio::test]
async fn update_epic_feed_command_set() {
    let state = test_state().await;
    let epic = state
        .db_write()
        .create_epic("Feed Epic", "", None)
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_epic",
            "arguments": { "epic_id": epic.id.0, "feed_command": "echo []" }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "{:?}", resp.error);

    let updated = state.db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(updated.feed_command.as_deref(), Some("echo []"));
}

#[tokio::test]
async fn update_epic_feed_command_clear() {
    let state = test_state().await;
    let epic = state
        .db_write()
        .create_epic("Feed Epic", "", None)
        .await
        .unwrap();
    state
        .db_write()
        .patch_epic(
            epic.id,
            &crate::db::EpicPatch::default().feed_command(Some("old cmd")),
        )
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_epic",
            "arguments": { "epic_id": epic.id.0, "feed_command": null }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "{:?}", resp.error);

    let updated = state.db.get_epic(epic.id).await.unwrap().unwrap();
    assert!(
        updated.feed_command.is_none(),
        "feed_command should be cleared"
    );
}

#[tokio::test]
async fn update_epic_feed_command_absent_preserves_existing() {
    let state = test_state().await;
    let epic = state
        .db_write()
        .create_epic("Feed Epic", "", None)
        .await
        .unwrap();
    state
        .db_write()
        .patch_epic(
            epic.id,
            &crate::db::EpicPatch::default().feed_command(Some("keep me")),
        )
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_epic",
            "arguments": { "epic_id": epic.id.0, "title": "Updated Title" }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "{:?}", resp.error);

    let updated = state.db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(updated.feed_command.as_deref(), Some("keep me"));
}

#[tokio::test]
async fn update_epic_feed_interval_secs_set() {
    let state = test_state().await;
    let epic = state
        .db_write()
        .create_epic("Feed Epic", "", None)
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_epic",
            "arguments": { "epic_id": epic.id.0, "feed_interval_secs": 60 }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "{:?}", resp.error);

    let updated = state.db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(updated.feed_interval_secs, Some(60));
}

#[tokio::test]
async fn update_epic_feed_interval_secs_clear() {
    let state = test_state().await;
    let epic = state
        .db_write()
        .create_epic("Feed Epic", "", None)
        .await
        .unwrap();
    state
        .db_write()
        .patch_epic(
            epic.id,
            &crate::db::EpicPatch::default().feed_interval_secs(Some(120)),
        )
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_epic",
            "arguments": { "epic_id": epic.id.0, "feed_interval_secs": null }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "{:?}", resp.error);

    let updated = state.db.get_epic(epic.id).await.unwrap().unwrap();
    assert!(
        updated.feed_interval_secs.is_none(),
        "feed_interval_secs should be cleared"
    );
}

#[tokio::test]
async fn get_epic_shows_feed_command() {
    let state = test_state().await;
    let epic = state
        .db_write()
        .create_epic("Feed Epic", "", None)
        .await
        .unwrap();
    state
        .db_write()
        .patch_epic(
            epic.id,
            &crate::db::EpicPatch::default()
                .feed_command(Some("./scripts/feed.sh"))
                .feed_interval_secs(Some(300)),
        )
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "get_epic",
            "arguments": { "epic_id": epic.id.0 }
        })),
    )
    .await;
    let text = extract_response_text(&resp);
    assert!(
        text.contains("./scripts/feed.sh"),
        "get_epic should show feed_command: {text}"
    );
    assert!(
        text.contains("300"),
        "get_epic should show feed_interval_secs: {text}"
    );
}

// ---------------------------------------------------------------------------
// Step 6: MCP sub-epic creation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mcp_create_sub_epic() {
    let state = test_state().await;

    // Create parent epic first
    let parent = state
        .db_write()
        .create_epic("Parent Epic", "desc", None)
        .await
        .unwrap();

    // Create sub-epic via MCP with parent_epic_id
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_epic",
            "arguments": {
                "title": "Sub Epic",
                "description": "child",
                "parent_epic_id": parent.id.0
            }
        })),
    )
    .await;

    assert!(
        resp.error.is_none(),
        "expected success, got: {:?}",
        resp.error
    );

    // Verify the sub-epic has the correct parent
    let epics = state.db.list_epics().await.unwrap();
    let sub = epics
        .iter()
        .find(|e| e.title == "Sub Epic")
        .expect("sub epic should be created");
    assert_eq!(
        sub.parent_epic_id,
        Some(parent.id),
        "sub epic should have parent_epic_id set"
    );
}

#[tokio::test]
async fn create_epic_tool_schema_includes_parent_epic_id() {
    let state = test_state().await;
    let resp = call(&state, "tools/list", None).await;
    let tools = resp.result.as_ref().unwrap()["tools"].as_array().unwrap();
    let create_epic = tools
        .iter()
        .find(|t| t["name"] == "create_epic")
        .expect("create_epic not in tool list");
    let props = &create_epic["inputSchema"]["properties"];
    assert!(
        props.get("parent_epic_id").is_some(),
        "create_epic schema is missing parent_epic_id property"
    );
}

// ---------------------------------------------------------------------------
// Learning tool tests
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
            schedule_interval_secs: None,
            pinned_branch: None,
        })
        .await
        .unwrap()
}

async fn create_approved_learning(
    state: &Arc<McpState>,
    summary: &str,
    scope: crate::models::LearningScope,
    scope_ref: Option<&str>,
    tags: &[&str],
) -> crate::models::LearningId {
    let tag_strings: Vec<String> = tags.iter().map(|s| s.to_string()).collect();
    // Store a stub embedding so the RAG pipeline can score this learning.
    // EmbeddingService::new_test() returns vec![0.1; 384], which yields
    // cosine similarity = 1.0 against any query embedded by the same stub.
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
            schedule_interval_secs: None,
            pinned_branch: None,
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
    // tag_filter is a soft boost in the RAG pipeline, not a hard filter.
    // Both learnings score identically (same stub embedding) but the
    // rust-tagged one gets a small boost. Both should appear.
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
    assert!(text.contains("Rust tips"), "expected rust-tagged learning");
    // tag_filter is a soft boost — untagged/differently-tagged entries are NOT excluded
    assert!(
        text.contains("Testing tips"),
        "tag_filter is soft: non-matching entries should still appear"
    );
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

// rate_learning handler behaviour is covered in
// src/mcp/handlers/tests/learnings.rs (helped/wrong/retrieval-guard/bad-verdict).
