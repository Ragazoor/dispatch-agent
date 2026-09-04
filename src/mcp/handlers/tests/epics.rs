#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;

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

// `get_epic` must agree with `list_epics` on a `group_by_repo` epic's progress —
// both roll up descendant sub-epics rather than counting only direct subtasks.
#[tokio::test]
async fn get_epic_matches_list_epics_progress_for_grouped_epic() {
    let state = test_state().await;
    let root = state
        .db_write()
        .create_epic("Grouped Root", "", None)
        .await
        .unwrap();
    state
        .db_write()
        .patch_epic(root.id, &db::EpicPatch::new().group_by_repo(true))
        .await
        .unwrap();
    let sub = state
        .db_write()
        .create_repo_group_sub_epic(root.id, "alpha")
        .await
        .unwrap();
    state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "t",
            description: "",
            repo_path: "/x/alpha",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: Some(sub),
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
            phoenix: false,
        })
        .await
        .unwrap();

    let list_resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_epics", "arguments": {} })),
    )
    .await;
    let list_text = extract_response_text(&list_resp);
    let list_line = list_text
        .lines()
        .find(|l| l.contains("Grouped Root"))
        .expect("list_epics should show Grouped Root");
    assert!(
        list_line.contains("0/1 done"),
        "list_epics should aggregate the sub-epic's task: {list_line}"
    );

    let get_resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "get_epic",
            "arguments": { "epic_id": root.id.0 }
        })),
    )
    .await;
    let get_text = extract_response_text(&get_resp);
    assert!(
        get_text.contains("0/1 done"),
        "get_epic should match list_epics' rollup (0/1 done), got: {get_text}"
    );
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
            phoenix: false,
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
async fn list_epics_excludes_archived() {
    let state = test_state().await;
    state
        .db_write()
        .create_epic("Active Epic", "desc", None)
        .await
        .unwrap();
    let archived_epic = state
        .db_write()
        .create_epic("Archived Epic", "desc", None)
        .await
        .unwrap();
    state
        .db_write()
        .patch_epic(
            archived_epic.id,
            &db::EpicPatch::new().status(TaskStatus::Archived),
        )
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_epics", "arguments": {} })),
    )
    .await;
    let text = extract_response_text(&resp);
    assert!(text.contains("Active Epic"), "should show active epic");
    assert!(
        !text.contains("Archived Epic"),
        "should not show archived epic: {text}"
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

/// The MCP integer path is the one that had no lower bound at all: it was how a
/// `0` reached the column and made the feed runner respawn the command on every
/// poll tick. The floor now binds it like every other write path.
#[tokio::test]
async fn update_epic_sub_floor_feed_interval_secs_rejected() {
    for bad in [0, -5, crate::models::MIN_FEED_INTERVAL_SECS - 1] {
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
                "arguments": { "epic_id": epic.id.0, "feed_interval_secs": bad }
            })),
        )
        .await;
        let result = resp
            .result
            .as_ref()
            .expect("expected isError result, got no result");
        assert_eq!(
            result["isError"],
            json!(true),
            "feed_interval_secs = {bad} should be rejected, got: {resp:?}"
        );
        let text = result["content"][0]["text"].as_str().unwrap_or("");
        assert!(
            text.contains("feed_interval_secs"),
            "the error must name the field, got: {text}"
        );

        let after = state.db.get_epic(epic.id).await.unwrap().unwrap();
        assert_eq!(
            after.feed_interval_secs, None,
            "a rejected interval must not be persisted"
        );
    }
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
async fn update_epic_feed_append_only() {
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
            "arguments": { "epic_id": epic.id.0, "feed_append_only": true }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);

    let updated = state.db.get_epic(epic.id).await.unwrap().unwrap();
    assert!(updated.feed_append_only);
}

#[tokio::test]
async fn update_epic_group_by_repo() {
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
            "arguments": { "epic_id": epic.id.0, "group_by_repo": true }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);

    let updated = state.db.get_epic(epic.id).await.unwrap().unwrap();
    assert!(updated.group_by_repo);
}

// ---------------------------------------------------------------------------
// update_epic parent_epic_id tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_epic_parent_id_set() {
    let state = test_state().await;
    let parent = state
        .db_write()
        .create_epic("Parent", "", None)
        .await
        .unwrap();
    let child = state
        .db_write()
        .create_epic("Child", "", None)
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_epic",
            "arguments": { "epic_id": child.id.0, "parent_epic_id": parent.id.0 }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);

    let updated = state.db.get_epic(child.id).await.unwrap().unwrap();
    assert_eq!(updated.parent_epic_id, Some(parent.id));
}

#[tokio::test]
async fn update_epic_parent_id_clear() {
    let state = test_state().await;
    let parent = state
        .db_write()
        .create_epic("Parent", "", None)
        .await
        .unwrap();
    let child = state
        .db_write()
        .create_epic("Child", "", Some(parent.id))
        .await
        .unwrap();
    assert_eq!(child.parent_epic_id, Some(parent.id));

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_epic",
            "arguments": { "epic_id": child.id.0, "parent_epic_id": null }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);

    let updated = state.db.get_epic(child.id).await.unwrap().unwrap();
    assert!(
        updated.parent_epic_id.is_none(),
        "parent_epic_id should be cleared"
    );
}

#[tokio::test]
async fn update_epic_parent_id_absent_preserves_existing() {
    let state = test_state().await;
    let parent = state
        .db_write()
        .create_epic("Parent", "", None)
        .await
        .unwrap();
    let child = state
        .db_write()
        .create_epic("Child", "", Some(parent.id))
        .await
        .unwrap();

    // Update title only — parent_epic_id field absent
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_epic",
            "arguments": { "epic_id": child.id.0, "title": "New Title" }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);

    let updated = state.db.get_epic(child.id).await.unwrap().unwrap();
    assert_eq!(
        updated.parent_epic_id,
        Some(parent.id),
        "parent_epic_id unchanged"
    );
}

#[tokio::test]
async fn update_epic_parent_id_cycle_returns_error() {
    let state = test_state().await;
    let a = state.db_write().create_epic("A", "", None).await.unwrap();
    let b = state
        .db_write()
        .create_epic("B", "", Some(a.id))
        .await
        .unwrap();

    // A → B already; setting A.parent = B would create B → A cycle
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_epic",
            "arguments": { "epic_id": a.id.0, "parent_epic_id": b.id.0 }
        })),
    )
    .await;
    assert_error(&resp, "cycle");
}

#[tokio::test]
async fn update_epic_tool_schema_includes_parent_epic_id() {
    let state = test_state().await;
    let resp = call(&state, "tools/list", None).await;
    let tools = resp.result.as_ref().unwrap()["tools"].as_array().unwrap();
    let update_epic = tools
        .iter()
        .find(|t| t["name"] == "update_epic")
        .expect("update_epic not in tool list");
    let props = &update_epic["inputSchema"]["properties"];
    assert!(
        props.get("parent_epic_id").is_some(),
        "update_epic schema is missing parent_epic_id property"
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

// `Epic` has had no `repo_path` column since migration `v61_drop_epic_repo_path`,
// and `CreateEpicArgs`/`UpdateEpicArgs` have never carried the field — a schema
// that requires or accepts it makes every caller invent a value that is discarded.
#[tokio::test]
async fn create_epic_schema_excludes_repo_path() {
    let state = test_state().await;
    let resp = call(&state, "tools/list", None).await;
    let tools = resp.result.as_ref().unwrap()["tools"].as_array().unwrap();
    let create_epic = tools
        .iter()
        .find(|t| t["name"] == "create_epic")
        .expect("create_epic not in tool list");
    let schema = &create_epic["inputSchema"];
    assert!(
        schema["properties"].get("repo_path").is_none(),
        "create_epic schema should not declare repo_path — CreateEpicArgs has no such field"
    );
    let required = schema["required"].as_array().unwrap();
    assert!(
        !required.iter().any(|v| v == "repo_path"),
        "create_epic schema should not require repo_path"
    );
}

#[tokio::test]
async fn update_epic_schema_excludes_repo_path() {
    let state = test_state().await;
    let resp = call(&state, "tools/list", None).await;
    let tools = resp.result.as_ref().unwrap()["tools"].as_array().unwrap();
    let update_epic = tools
        .iter()
        .find(|t| t["name"] == "update_epic")
        .expect("update_epic not in tool list");
    assert!(
        update_epic["inputSchema"]["properties"]
            .get("repo_path")
            .is_none(),
        "update_epic schema should not declare repo_path — UpdateEpicArgs has no such field"
    );
}
