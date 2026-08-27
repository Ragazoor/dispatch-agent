#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use crate::dispatch::mock_sequence::DispatchScript;

// -- update_task boundary parity ---------------------------------------------
//
// These guard the `mcp_args!` generation itself (see `src/mcp/handlers/args.rs`
// for what it emits and why): that the schema is well-formed, that every name it
// advertises is one the parser accepts, and that every advertised field still
// reaches the service params.

fn update_task_schema() -> serde_json::Value {
    crate::mcp::handlers::tasks::update_task_schema()
}

/// The advertised property set is exactly the struct's field set. A schema
/// property the struct does not have would be a runtime `-32602` on the first
/// agent that believed the schema.
#[test]
fn update_task_schema_properties_match_the_args_struct() {
    let schema = update_task_schema();
    let mut advertised: Vec<&str> = schema["properties"]
        .as_object()
        .expect("properties must be an object")
        .keys()
        .map(String::as_str)
        .collect();
    advertised.sort_unstable();

    let mut declared: Vec<&str> = crate::mcp::handlers::tasks::UpdateTaskArgs::FIELD_NAMES.to_vec();
    declared.sort_unstable();

    assert_eq!(advertised, declared);
}

/// The JSON type a property really is, ignoring a nullable field's `"null"`
/// alternative. `"type"` is either a bare string or an array of them.
fn nominal_type(ty: &serde_json::Value) -> Option<&str> {
    match ty {
        serde_json::Value::String(s) => Some(s.as_str()),
        serde_json::Value::Array(members) => members
            .iter()
            .filter_map(serde_json::Value::as_str)
            .find(|m| *m != "null"),
        _ => None,
    }
}

/// Every advertised field name is one `deny_unknown_fields` accepts. This is
/// the leg that used to fail at runtime: a schema/struct name mismatch was a
/// `-32602 Invalid arguments` rather than a build or test failure.
#[test]
fn every_advertised_update_task_field_is_accepted_by_the_parser() {
    let schema = update_task_schema();
    let properties = schema["properties"].as_object().unwrap();

    // One representative valid value per JSON type the schema advertises. The
    // point is name acceptance, not value validation, so enums use their first
    // advertised member.
    for (name, spec) in properties {
        let value = match spec.get("enum").and_then(|e| e.as_array()) {
            Some(members) => members
                .iter()
                .find(|m| !m.is_null())
                .expect("an enum must advertise at least one non-null member")
                .clone(),
            // A nullable field advertises `["<type>", "null"]`. Clearing is
            // covered elsewhere; here the representative value is the
            // non-null half.
            None => match nominal_type(&spec["type"]) {
                Some("integer") => json!(1),
                Some("boolean") => json!(true),
                Some("string") => json!("x"),
                other => panic!("field {name} has unhandled schema type {other:?}"),
            },
        };
        let args = json!({ "task_id": 1, (name.as_str()): value });
        let parsed = serde_json::from_value::<crate::mcp::handlers::tasks::UpdateTaskArgs>(args);
        assert!(
            parsed.is_ok(),
            "schema advertises {name} but the args struct rejects it: {:?}",
            parsed.err()
        );
    }
}

/// `task_id` must stay the only required field — every other one is a no-op
/// when absent, so it is the only one the tool cannot work without.
/// (`tool_schemas_have_consistent_required_fields` in `tests/mod.rs` already
/// checks registry-wide that `required` names real properties.)
#[test]
fn update_task_schema_required_list_is_task_id_only() {
    assert_eq!(update_task_schema()["required"], json!(["task_id"]));
}

/// Every property carries a description — the schema is the only documentation
/// an agent sees for these fields.
#[test]
fn every_update_task_property_is_documented() {
    for (name, spec) in update_task_schema()["properties"].as_object().unwrap() {
        assert!(
            spec.get("description")
                .and_then(|d| d.as_str())
                .is_some_and(|d| !d.is_empty()),
            "field {name} has no description"
        );
    }
}

/// The mapping leg: sending every advertised field in one call must report
/// every corresponding params field as updated. A field present in the struct
/// and the schema but dropped on the way to `UpdateTaskParams` would be
/// silently ignored — the failure mode this whole boundary exists to prevent.
#[tokio::test]
async fn update_task_maps_every_advertised_field_to_params() {
    let state = test_state().await;
    let task_id = create_task_fixture(&state).await;
    let epic = state.db_write().create_epic("E", "", None).await.unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": {
                "task_id": task_id.0,
                "status": "running",
                "plan_path": "/plans/p.md",
                "title": "new title",
                "description": "new description",
                "repo_path": "/repo",
                "sort_order": 3,
                "url": "https://github.com/org/repo/pull/1",
                "url_type": "pr",
                "tag": "bug",
                "sub_status": "active",
                "epic_id": epic.id.0,
                "base_branch": "develop",
                "wrap_up_mode": "pr",
                "auto_run_plan": true,
            }
        })),
    )
    .await;

    let text = resp.result.as_ref().unwrap()["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();

    // Only the `[manual]` fields are exempt, and the exemption set is derived
    // from the macro rather than hand-listed — so declaring a new `[manual]`
    // field cannot quietly widen what this test forgives.
    let manual = crate::mcp::handlers::tasks::UpdateTaskArgs::manual_fields();
    for field in crate::mcp::handlers::tasks::UpdateTaskArgs::FIELD_NAMES {
        if manual.contains(field) {
            continue;
        }
        assert!(
            text.contains(field),
            "update_task did not report {field} as updated: {text}"
        );
    }

    // `url` is `[manual]` but the handler does map it, so assert it explicitly
    // rather than letting the exemption above cover for a regression there.
    // (`url_type` has no params field of its own — it folds into `url`.)
    assert!(
        text.contains("url"),
        "update_task did not report url as updated: {text}"
    );
}

// -- update_task tests -------------------------------------------------------

#[tokio::test]
async fn update_task_valid() {
    let state = test_state().await;
    let task_id = create_task_fixture(&state).await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "status": "running" }
        })),
    )
    .await;
    assert!(resp.result.is_some());
    assert!(resp.error.is_none());

    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(task.status, crate::models::TaskStatus::Running);
}

#[tokio::test]
async fn update_task_invalid_status() {
    let state = test_state().await;
    let task_id = create_task_fixture(&state).await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "status": "bogus" }
        })),
    )
    .await;
    assert_error(&resp, "unknown variant `bogus`");
}

#[tokio::test]
async fn update_task_rejects_done_status() {
    let state = test_state().await;
    let task_id = create_task_fixture(&state).await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "status": "done" }
        })),
    )
    .await;
    assert_error(&resp, "Cannot set status to done or archived via MCP");

    // Verify task status unchanged
    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert_ne!(task.status, crate::models::TaskStatus::Done);

    // Also verify archived is rejected
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "status": "archived" }
        })),
    )
    .await;
    assert_error(&resp, "Cannot set status to done or archived via MCP");
}

#[tokio::test]
async fn update_task_still_allows_other_statuses() {
    let state = test_state().await;
    let task_id = create_task_fixture(&state).await;

    for status in &["running", "review", "ready", "backlog"] {
        let resp = call(
            &state,
            "tools/call",
            Some(json!({
                "name": "update_task",
                "arguments": { "task_id": task_id.0, "status": status }
            })),
        )
        .await;
        assert!(
            resp.error.is_none(),
            "status={status} should be allowed, got: {:?}",
            resp.error
        );
    }
}

/// The done-status rejection above is one-directional: it blocks moving a task
/// INTO done, but not out of it. So every destination this tool accepts is
/// reachable from a done task, and each one hits the leaving-done sort_order
/// clear in `TaskService::update_task` — the completion-recency rank must not
/// survive into the destination column, where its large negative value would
/// pin the card to the top. The clear itself is covered at the service layer
/// (`update_task_leaving_done_clears_sort_order`); what this asserts is that
/// the MCP gate does not stand between a done task and that clear.
/// Spec: `UpdateTaskViaMcp` in docs/specs/mcp-task-tools.allium.
#[tokio::test]
async fn update_task_done_rejection_does_not_block_leaving_done() {
    let state = test_state().await;

    for status in &["backlog", "running", "review"] {
        let task_id = create_task_fixture(&state).await;

        // Seed a done task holding a rank. The value only has to be non-null
        // for the assertion below — its encoding is owned by
        // sort_order_for_status_transition, not by this test.
        state
            .db_write()
            .patch_task(
                task_id,
                &db::TaskPatch::new()
                    .status(TaskStatus::Done)
                    .sort_order(Some(-1_700_000_000_000)),
            )
            .await
            .unwrap();

        let resp = call(
            &state,
            "tools/call",
            Some(json!({
                "name": "update_task",
                "arguments": { "task_id": task_id.0, "status": status }
            })),
        )
        .await;
        assert!(!is_error(&resp), "status={status}: {:?}", resp);

        let task = state.db.get_task(task_id).await.unwrap().unwrap();
        assert_eq!(
            task.sort_order, None,
            "leaving done -> {status} must clear sort_order"
        );
    }
}

#[tokio::test]
async fn update_task_missing_args() {
    let state = test_state().await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "update_task", "arguments": {} })),
    )
    .await;
    assert!(is_error(&resp));
}

#[tokio::test]
async fn get_task_found() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "My Task",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: crate::models::TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "get_task",
            "arguments": { "task_id": task_id.0 }
        })),
    )
    .await;
    let result = resp.result.unwrap();
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("My Task"));
}

#[tokio::test]
async fn get_task_not_found() {
    let state = test_state().await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "get_task",
            "arguments": { "task_id": 9999 }
        })),
    )
    .await;
    assert!(is_error(&resp));
    assert!(error_message(&resp).contains("not found"));
}

#[tokio::test]
async fn create_task_minimal() {
    let state = test_state().await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_task",
            "arguments": {
                "title": "New Task",
                "repo_path": "/my/repo",
            }
        })),
    )
    .await;
    assert!(resp.error.is_none());
    let result = resp.result.unwrap();
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("created"));

    // Verify task was created in DB
    let tasks = state.db.list_all().await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].title, "New Task");
    assert_eq!(tasks[0].status, TaskStatus::Backlog);
    assert!(tasks[0].plan_path.is_none());
}

#[tokio::test]
async fn create_task_with_plan_stays_backlog() {
    let dir = tempfile::tempdir().unwrap();
    let plan_file = dir.path().join("plan.md");
    std::fs::write(&plan_file, "# Plan").unwrap();

    let state = test_state().await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_task",
            "arguments": {
                "title": "Planned Task",
                "repo_path": "/my/repo",
                "plan_path": plan_file.to_string_lossy(),
            }
        })),
    )
    .await;
    assert!(resp.error.is_none());

    let tasks = state.db.list_all().await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].status, TaskStatus::Backlog);
    let stored = tasks[0].plan_path.as_deref().unwrap();
    assert!(
        std::path::Path::new(stored).is_absolute(),
        "plan path should be absolute, got: {stored}"
    );
    assert_eq!(
        stored,
        std::fs::canonicalize(&plan_file).unwrap().to_string_lossy()
    );
}

#[tokio::test]
async fn create_task_with_description() {
    let state = test_state().await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_task",
            "arguments": {
                "title": "Described Task",
                "repo_path": "/repo",
                "description": "Some details",
            }
        })),
    )
    .await;
    assert!(resp.error.is_none());

    let tasks = state.db.list_all().await.unwrap();
    assert_eq!(tasks[0].description, "Some details");
}

#[tokio::test]
async fn create_task_missing_title() {
    let state = test_state().await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_task",
            "arguments": { "repo_path": "/repo" }
        })),
    )
    .await;
    assert!(is_error(&resp));
}

// An unknown argument name is a typo or a stale/removed field — without
// `deny_unknown_fields` it is silently dropped instead of surfacing as an error.
#[tokio::test]
async fn create_task_unknown_field_returns_error() {
    let state = test_state().await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_task",
            "arguments": { "title": "t", "repo_path": "/repo", "bogus_field": "x" }
        })),
    )
    .await;
    assert_error(&resp, "bogus_field");
}

// -- String task_id coercion (Claude Code sends integers as strings) ------

#[tokio::test]
async fn update_task_accepts_string_task_id() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Test",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: crate::models::TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0.to_string(), "status": "running" }
        })),
    )
    .await;
    assert!(
        resp.error.is_none(),
        "update_task should accept string task_id, got: {:?}",
        resp.error
    );

    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(task.status, crate::models::TaskStatus::Running);
}

// ---------------------------------------------------------------------------
// Mock service injection — demonstrates the TaskServiceApi seam
// ---------------------------------------------------------------------------

/// A minimal mock that satisfies `TaskServiceApi` without a database.
///
/// Only `list_tasks` is mocked; every other method inherits the panicking
/// default from `TaskServiceApiStub`, so an unexpected call fails loudly and
/// adding a method to the seam does not touch this file. See the emitter docs
/// in `src/service/api.rs`.
struct MockTaskService {
    tasks: Vec<crate::models::Task>,
}

#[async_trait::async_trait]
impl crate::service::TaskServiceApiStub for MockTaskService {
    async fn list_tasks(
        &self,
        _filter: crate::service::ListTasksFilter,
    ) -> Result<Vec<crate::models::Task>, crate::service::ServiceError> {
        Ok(self.tasks.clone())
    }
}

crate::task_service_api!(service_api_stub_bridge, MockTaskService);

fn mock_task(id: i64, title: &str) -> crate::models::Task {
    crate::models::Task {
        id: crate::models::TaskId(id),
        title: title.to_string(),
        description: "mock description".to_string(),
        repo_path: "/mock/repo".to_string(),
        status: crate::models::TaskStatus::Backlog,
        worktree: None,
        tmux_window: None,
        plan_path: None,
        epic_id: None,
        sub_status: crate::models::SubStatus::None,
        url: None,
        tag: None,
        sort_order: None,
        base_branch: "main".into(),
        external_id: None,
        labels: vec![],
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        last_pre_tool_use_at: None,
        last_notification_at: None,
        last_peer_message_sent_at: None,
        last_peer_message_received_at: None,
        wrap_up_mode: None,
        auto_run_plan: false,
        live_subagents: 0,
        stop_pending: false,
        live_shells: 0,
        oldest_live_shell_started_at: None,
    }
}

/// Constructs McpState with `task_svc` injected — the mock-service seam.
async fn state_with_mock_task_svc(
    task_svc: Arc<dyn crate::service::TaskServiceApi>,
) -> Arc<McpState> {
    test_state_with_overrides(
        Arc::new(MockProcessRunner::new(vec![])),
        None,
        Some(task_svc),
    )
    .await
    .0
}

/// `list_tasks` returns whatever the service layer provides, independently of
/// what is stored in the DB. This test proves the handler calls `task_svc`,
/// not a raw DB query — and that the seam is injectable in unit tests.
#[tokio::test]
async fn list_tasks_uses_service_not_db_directly() {
    let mock_svc = Arc::new(MockTaskService {
        tasks: vec![mock_task(101, "Alpha task"), mock_task(102, "Beta task")],
    });
    let state = state_with_mock_task_svc(mock_svc).await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_tasks", "arguments": {} })),
    )
    .await;

    assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
    let text = extract_response_text(&resp);
    // Both mock tasks appear in the response; neither was in the database.
    assert!(text.contains("Alpha task"), "expected mock task in: {text}");
    assert!(text.contains("Beta task"), "expected mock task in: {text}");
}

#[tokio::test]
async fn get_task_accepts_string_task_id() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "My Task",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: crate::models::TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "get_task",
            "arguments": { "task_id": task_id.0.to_string() }
        })),
    )
    .await;
    assert!(
        resp.error.is_none(),
        "get_task should accept string task_id, got: {:?}",
        resp.error
    );
    let result = resp.result.unwrap();
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("My Task"));
}

#[tokio::test]
async fn update_task_with_plan() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Test",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: crate::models::TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "status": "ready", "plan_path": "/path/to/plan.md" }
        })),
    )
    .await;
    assert!(resp.error.is_none());

    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(task.status, crate::models::TaskStatus::Backlog);
    assert_eq!(task.plan_path.as_deref(), Some("/path/to/plan.md"));
}

#[tokio::test]
async fn update_task_title_only() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Old",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: crate::models::TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "title": "New Title" }
        })),
    )
    .await;
    assert!(
        resp.error.is_none(),
        "should succeed with title only: {:?}",
        resp.error
    );

    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(task.title, "New Title");
    assert_eq!(task.status, crate::models::TaskStatus::Backlog); // unchanged
}

#[tokio::test]
async fn update_task_status_optional() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Test",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: crate::models::TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "title": "Renamed" }
        })),
    )
    .await;
    assert!(resp.error.is_none());

    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(task.title, "Renamed");
    assert_eq!(task.status, crate::models::TaskStatus::Backlog);
}

#[tokio::test]
async fn update_task_title_and_description() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Old",
            description: "old desc",
            repo_path: "/repo",
            plan: None,
            status: crate::models::TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "title": "New", "description": "new desc" }
        })),
    )
    .await;
    assert!(resp.error.is_none());

    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(task.title, "New");
    assert_eq!(task.description, "new desc");
}

#[tokio::test]
async fn update_task_repo_path() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Test",
            description: "desc",
            repo_path: "/old/repo",
            plan: None,
            status: crate::models::TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "repo_path": "/new/repo" }
        })),
    )
    .await;
    assert!(
        resp.error.is_none(),
        "should succeed with repo_path only: {:?}",
        resp.error
    );

    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(task.repo_path, "/new/repo");
    assert_eq!(task.status, crate::models::TaskStatus::Backlog); // unchanged
}

#[tokio::test]
async fn update_task_no_fields_errors() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Test",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: crate::models::TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0 }
        })),
    )
    .await;
    assert!(is_error(&resp), "should error with no fields to update");
}

#[tokio::test]
async fn patch_task_sets_multiple_fields() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Test",
            description: "Desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": {
                "task_id": task_id.0,
                "status": "ready",
                "title": "Updated Title"
            }
        })),
    )
    .await;
    assert!(resp.error.is_none());

    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Backlog);
    assert_eq!(task.title, "Updated Title");
}

#[tokio::test]
async fn update_task_without_plan_preserves_existing() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Test",
            description: "desc",
            repo_path: "/repo",
            plan: Some("/existing.md"),
            status: crate::models::TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "status": "ready" }
        })),
    )
    .await;
    assert!(resp.error.is_none());

    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(
        task.plan_path.as_deref(),
        Some("/existing.md"),
        "plan should be preserved when not provided"
    );
}

#[tokio::test]
async fn update_task_sets_pr_fields() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "PR test",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": {
                "task_id": task_id.0,
                "url": "https://github.com/org/repo/pull/99",
                "url_type": "pr"
            }
        })),
    )
    .await;
    assert!(
        resp.error.is_none(),
        "Expected success, got: {:?}",
        resp.error
    );

    let updated = state.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(
        updated.url.as_ref().map(|u| u.url.as_str()),
        Some("https://github.com/org/repo/pull/99")
    );
    assert_eq!(
        updated.url.as_ref().map(|u| u.url_type),
        Some(crate::models::UrlType::Pr)
    );
}

#[tokio::test]
async fn update_task_rejects_unknown_url_type() {
    let state = test_state().await;
    let task_id = create_task_fixture(&state).await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": {
                "task_id": task_id.0,
                "url": "https://x/y",
                "url_type": "bogus"
            }
        })),
    )
    .await;
    assert_error(&resp, "url_type");
}

/// `url_type` is parsed into its enum at the JSON-RPC boundary, like `status`,
/// `tag` and `sub_status` — so a bad literal is rejected on its own, not only
/// when a `url` happens to accompany it. Previously it was carried inward as a
/// `String` and only validated on the url-setting path, where a url-less call
/// with a typo'd `url_type` succeeded silently.
#[tokio::test]
async fn update_task_rejects_unknown_url_type_even_without_a_url() {
    let state = test_state().await;
    let task_id = create_task_fixture(&state).await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": {
                "task_id": task_id.0,
                "title": "t",
                "url_type": "bogus"
            }
        })),
    )
    .await;
    assert_error(&resp, "url_type");
}

#[tokio::test]
async fn update_task_url_without_type_is_rejected() {
    let state = test_state().await;
    let task_id = create_task_fixture(&state).await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": {
                "task_id": task_id.0,
                "url": "https://x/y"
            }
        })),
    )
    .await;
    assert_error(&resp, "url_type");
}

// -- wrap_up_mode tests -----------------------------------------------------

#[tokio::test]
async fn update_task_sets_wrap_up_mode() {
    let state = test_state().await;
    let task_id = create_task_fixture(&state).await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "wrap_up_mode": "rebase" }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "got error: {:?}", resp.error);

    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(task.wrap_up_mode, Some(crate::models::WrapUpMode::Rebase));
}

#[tokio::test]
async fn update_task_wrap_up_mode_all_variants() {
    use crate::models::WrapUpMode;
    let state = test_state().await;
    let task_id = create_task_fixture(&state).await;

    for (input, expected) in [
        ("rebase", WrapUpMode::Rebase),
        ("pr", WrapUpMode::Pr),
        ("done", WrapUpMode::Done),
    ] {
        let resp = call(
            &state,
            "tools/call",
            Some(json!({
                "name": "update_task",
                "arguments": { "task_id": task_id.0, "wrap_up_mode": input }
            })),
        )
        .await;
        assert!(
            resp.error.is_none(),
            "wrap_up_mode={input} should succeed, got: {:?}",
            resp.error
        );
        let task = state.db.get_task(task_id).await.unwrap().unwrap();
        assert_eq!(
            task.wrap_up_mode,
            Some(expected),
            "wrap_up_mode should be {expected:?} after setting to {input}"
        );
    }
}

#[tokio::test]
async fn update_task_clears_wrap_up_mode_with_null() {
    let state = test_state().await;
    let task_id = create_task_fixture(&state).await;

    // First set a mode
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "wrap_up_mode": "pr" }
        })),
    )
    .await;
    assert!(resp.error.is_none());

    // Now clear it with null
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "wrap_up_mode": null }
        })),
    )
    .await;
    assert!(
        resp.error.is_none(),
        "clearing wrap_up_mode with null should succeed: {:?}",
        resp.error
    );

    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert!(
        task.wrap_up_mode.is_none(),
        "wrap_up_mode should be cleared after null"
    );
}

#[tokio::test]
async fn update_task_rejects_invalid_wrap_up_mode() {
    let state = test_state().await;
    let task_id = create_task_fixture(&state).await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "wrap_up_mode": "teleport" }
        })),
    )
    .await;
    assert!(is_error(&resp), "invalid wrap_up_mode should error");
}

#[tokio::test]
async fn create_task_with_wrap_up_mode() {
    let state = test_state().await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_task",
            "arguments": {
                "title": "Task with mode",
                "repo_path": "/repo",
                "wrap_up_mode": "pr"
            }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "got error: {:?}", resp.error);

    let task_id = extract_created_task_id(&resp);
    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(task.wrap_up_mode, Some(crate::models::WrapUpMode::Pr));
}

#[tokio::test]
async fn create_task_with_auto_run_plan_true() {
    let state = test_state().await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_task",
            "arguments": {
                "title": "T",
                "repo_path": "/r",
                "auto_run_plan": true
            }
        })),
    )
    .await;
    assert!(!is_error(&resp));

    let tasks = state.db.list_all().await.unwrap();
    let task = tasks
        .iter()
        .find(|t| t.title == "T")
        .expect("task should exist");
    assert!(task.auto_run_plan);
}

#[tokio::test]
async fn update_task_sets_auto_run_plan() {
    let state = test_state().await;
    let task_id = create_task_fixture(&state).await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "auto_run_plan": true }
        })),
    )
    .await;
    assert!(!is_error(&resp));

    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert!(task.auto_run_plan);
}

#[tokio::test]
async fn get_task_shows_wrap_up_mode() {
    let state = test_state().await;
    let task_id = create_task_fixture(&state).await;

    // Set wrap_up_mode
    call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "wrap_up_mode": "rebase" }
        })),
    )
    .await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "get_task",
            "arguments": { "task_id": task_id.0 }
        })),
    )
    .await;
    let result = resp.result.unwrap();
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("rebase"),
        "get_task should show wrap_up_mode: {text}"
    );
}

// -- list_tasks tests -------------------------------------------------------

#[tokio::test]
async fn list_tasks_returns_all_when_no_filter() {
    let state = test_state().await;
    state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Task A",
            description: "desc a",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Task B",
            description: "desc b",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_tasks", "arguments": {} })),
    )
    .await;
    assert!(resp.error.is_none());
    let result = resp.result.unwrap();
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Task A"));
    assert!(text.contains("Task B"));
}

#[tokio::test]
async fn list_tasks_filters_by_single_status() {
    let state = test_state().await;
    state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Backlog Task",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Running Task",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_tasks", "arguments": { "status": "backlog" } })),
    )
    .await;
    assert!(resp.error.is_none());
    let result = resp.result.unwrap();
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Backlog Task"));
    assert!(!text.contains("Running Task"));
}

#[tokio::test]
async fn list_tasks_filters_by_multiple_statuses() {
    let state = test_state().await;
    state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Backlog Task",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Running Task",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Review Task",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Review,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_tasks", "arguments": { "status": ["backlog", "running"] } })),
    )
    .await;
    assert!(resp.error.is_none());
    let result = resp.result.unwrap();
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Backlog Task"));
    assert!(text.contains("Running Task"));
    assert!(!text.contains("Review Task"));
}

#[tokio::test]
async fn list_tasks_empty_result() {
    let state = test_state().await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_tasks", "arguments": { "status": "running" } })),
    )
    .await;
    assert!(resp.error.is_none());
    let result = resp.result.unwrap();
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("No tasks found"));
}

// =======================================================================
// Additional edge case tests
// =======================================================================

#[tokio::test]
async fn list_tasks_invalid_status_string() {
    let state = test_state().await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_tasks", "arguments": { "status": "bogus" } })),
    )
    .await;
    assert_error(&resp, "Unknown status");
}

#[tokio::test]
async fn list_tasks_invalid_status_in_array() {
    let state = test_state().await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_tasks", "arguments": { "status": ["backlog", "bogus"] } })),
    )
    .await;
    assert_error(&resp, "Unknown status: bogus");
}

#[tokio::test]
async fn list_tasks_status_as_number_errors() {
    let state = test_state().await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_tasks", "arguments": { "status": 42 } })),
    )
    .await;
    assert_error(&resp, "expected a status string");
}

#[tokio::test]
async fn create_task_with_epic_id() {
    let state = test_state().await;
    let epic = state
        .db_write()
        .create_epic("Parent Epic", "", None)
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_task",
            "arguments": {
                "title": "Epic Child",
                "repo_path": "/repo",
                "epic_id": epic.id.0,
            }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "{:?}", resp.error);

    let subtasks = state.db.list_tasks_for_epic(epic.id).await.unwrap();
    assert_eq!(subtasks.len(), 1);
    assert_eq!(subtasks[0].title, "Epic Child");
}

#[tokio::test]
async fn create_task_with_string_epic_id() {
    let state = test_state().await;
    let epic = state
        .db_write()
        .create_epic("Parent", "", None)
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_task",
            "arguments": {
                "title": "String Epic Child",
                "repo_path": "/repo",
                "epic_id": epic.id.0.to_string(),
            }
        })),
    )
    .await;
    assert!(
        resp.error.is_none(),
        "should accept string epic_id: {:?}",
        resp.error
    );

    let subtasks = state.db.list_tasks_for_epic(epic.id).await.unwrap();
    assert_eq!(subtasks.len(), 1);
}

// ---------------------------------------------------------------------------
// sub_status tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_task_sets_sub_status() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "T",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "sub_status": "needs_input" }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "expected success: {:?}", resp.error);
    let text = extract_response_text(&resp);
    assert!(
        text.contains("sub_status"),
        "response should mention sub_status: {text}"
    );

    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(task.sub_status, crate::models::SubStatus::NeedsInput);
}

#[tokio::test]
async fn update_task_rejects_invalid_sub_status_for_status() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "T",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "sub_status": "approved" }
        })),
    )
    .await;
    assert_error(&resp, "not valid for status");
}

#[tokio::test]
async fn update_task_rejects_bogus_sub_status() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "T",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "sub_status": "bogus" }
        })),
    )
    .await;
    assert_error(&resp, "unknown variant `bogus`");
}

#[tokio::test]
async fn update_task_sub_status_with_status_change() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "T",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    // Change status to review and set sub_status to approved in one call
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "status": "review", "sub_status": "approved" }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "expected success: {:?}", resp.error);

    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Review);
    assert_eq!(task.sub_status, crate::models::SubStatus::Approved);
}

#[tokio::test]
async fn update_task_status_running_with_needs_input() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "T",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    // Set status=running and sub_status=needs_input in one call.
    // Before the fix, status() auto-reset sub_status to Active, which could
    // overwrite the explicit needs_input depending on builder call order.
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "status": "running", "sub_status": "needs_input" }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "expected success: {:?}", resp.error);

    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.sub_status, crate::models::SubStatus::NeedsInput);
}

#[tokio::test]
async fn update_task_sub_status_invalid_for_new_status() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "T",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    // Change status to review but set sub_status to active (valid for running, not review)
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "status": "review", "sub_status": "active" }
        })),
    )
    .await;
    assert_error(&resp, "not valid for status");
}

#[tokio::test]
async fn list_tasks_shows_sub_status() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Listed Task",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    state
        .db_write()
        .patch_task(
            task_id,
            &db::TaskPatch::new().sub_status(crate::models::SubStatus::NeedsInput),
        )
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_tasks", "arguments": {} })),
    )
    .await;
    let text = extract_response_text(&resp);
    assert!(
        text.contains("running/needs_input"),
        "expected running/needs_input in list output, got: {text}"
    );
}

#[tokio::test]
async fn get_task_shows_sub_status() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Detail Task",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Review,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    state
        .db_write()
        .patch_task(
            task_id,
            &db::TaskPatch::new().sub_status(crate::models::SubStatus::ChangesRequested),
        )
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "get_task",
            "arguments": { "task_id": task_id.0 }
        })),
    )
    .await;
    let text = extract_response_text(&resp);
    assert!(
        text.contains("Sub-status: changes_requested"),
        "expected sub-status in detail, got: {text}"
    );
}

// =======================================================================
// update_task: additional validation and edge cases
// =======================================================================

#[tokio::test]
async fn update_task_nonexistent_task_returns_error() {
    let state = test_state().await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": 9999, "status": "running" }
        })),
    )
    .await;
    assert_error(&resp, "Task 9999 not found");
}

#[tokio::test]
async fn update_task_invalid_tag() {
    let state = test_state().await;
    let task_id = create_task_fixture(&state).await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "tag": "invalid_tag" }
        })),
    )
    .await;
    assert_error(&resp, "unknown variant `invalid_tag`");
}

#[tokio::test]
async fn update_task_valid_tag() {
    let state = test_state().await;
    let task_id = create_task_fixture(&state).await;

    for tag in &["bug", "feature", "chore"] {
        let resp = call(
            &state,
            "tools/call",
            Some(json!({
                "name": "update_task",
                "arguments": { "task_id": task_id.0, "tag": tag }
            })),
        )
        .await;
        assert!(
            resp.error.is_none(),
            "tag={tag} should be valid, got: {:?}",
            resp.error
        );
    }

    // Verify last tag persisted
    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(task.tag, Some(crate::models::TaskTag::Chore));
}

#[tokio::test]
async fn update_task_rejects_epic_tag() {
    let state = test_state().await;
    let task_id = create_task_fixture(&state).await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "tag": "epic" }
        })),
    )
    .await;
    assert!(
        is_error(&resp),
        "tag=epic should be rejected; the variant was removed"
    );
}

#[tokio::test]
async fn update_task_sets_epic_id() {
    let state = test_state().await;
    let epic = state
        .db_write()
        .create_epic("Parent", "", None)
        .await
        .unwrap();
    let task_id = create_task_fixture(&state).await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "epic_id": epic.id.0 }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "{:?}", resp.error);
    let text = extract_response_text(&resp);
    assert!(
        text.contains("epic_id"),
        "response should list epic_id: {text}"
    );

    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(task.epic_id, Some(epic.id));
}

#[tokio::test]
async fn update_task_sort_order() {
    let state = test_state().await;
    let task_id = create_task_fixture(&state).await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": { "task_id": task_id.0, "sort_order": 42 }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "{:?}", resp.error);

    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(task.sort_order, Some(42));
}

// =======================================================================
// create_task: additional validation and edge cases
// =======================================================================

#[tokio::test]
async fn create_task_invalid_tag() {
    let state = test_state().await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_task",
            "arguments": { "title": "Tagged", "repo_path": "/repo", "tag": "bogus" }
        })),
    )
    .await;
    assert_error(&resp, "unknown variant `bogus`");
}

#[tokio::test]
async fn create_task_valid_tag() {
    let state = test_state().await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_task",
            "arguments": { "title": "Bug Task", "repo_path": "/repo", "tag": "bug" }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "{:?}", resp.error);

    let tasks = state.db.list_all().await.unwrap();
    assert_eq!(tasks[0].tag, Some(crate::models::TaskTag::Bug));
}

#[tokio::test]
async fn create_task_with_sort_order() {
    let state = test_state().await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_task",
            "arguments": { "title": "Ordered Task", "repo_path": "/repo", "sort_order": 99 }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "{:?}", resp.error);

    let tasks = state.db.list_all().await.unwrap();
    assert_eq!(tasks[0].sort_order, Some(99));
}

#[tokio::test]
async fn create_task_with_nonexistent_epic() {
    let state = test_state().await;
    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_task",
            "arguments": { "title": "Orphan", "repo_path": "/repo", "epic_id": 9999 }
        })),
    )
    .await;
    // Should fail because the epic FK doesn't exist
    assert!(is_error(&resp), "should error with invalid epic_id");
}

// =======================================================================
// list_tasks: filtering edge cases
// =======================================================================

#[tokio::test]
async fn list_tasks_filters_by_epic_id() {
    let state = test_state().await;
    let epic = state
        .db_write()
        .create_epic("My Epic", "", None)
        .await
        .unwrap();
    let t1 = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Epic Task",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Standalone Task",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    state
        .db_write()
        .set_task_epic_id(t1, Some(epic.id))
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_tasks", "arguments": { "epic_id": epic.id.0 } })),
    )
    .await;
    assert!(resp.error.is_none());
    let text = extract_response_text(&resp);
    assert!(
        text.contains("Epic Task"),
        "should include task linked to epic"
    );
    assert!(
        !text.contains("Standalone Task"),
        "should exclude task not linked to epic"
    );
}

#[tokio::test]
async fn list_tasks_filters_by_status_and_epic_id() {
    let state = test_state().await;
    let epic = state
        .db_write()
        .create_epic("Combined Filter", "", None)
        .await
        .unwrap();
    let t1 = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Backlog Epic",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let t2 = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Running Epic",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
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
            "name": "list_tasks",
            "arguments": { "status": "backlog", "epic_id": epic.id.0 }
        })),
    )
    .await;
    assert!(resp.error.is_none());
    let text = extract_response_text(&resp);
    assert!(
        text.contains("Backlog Epic"),
        "should include backlog task in epic"
    );
    assert!(
        !text.contains("Running Epic"),
        "should exclude running task when filtering by backlog"
    );
}

#[tokio::test]
async fn list_tasks_epic_filter_no_match() {
    let state = test_state().await;
    state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "No Epic",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_tasks", "arguments": { "epic_id": 9999 } })),
    )
    .await;
    assert!(resp.error.is_none());
    let text = extract_response_text(&resp);
    assert!(text.contains("No tasks found"));
}

#[tokio::test]
async fn list_tasks_done_status_filter() {
    let state = test_state().await;
    state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Done Task",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Done,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Backlog Task",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_tasks", "arguments": { "status": "done" } })),
    )
    .await;
    assert!(resp.error.is_none());
    let text = extract_response_text(&resp);
    assert!(text.contains("Done Task"));
    assert!(!text.contains("Backlog Task"));
}

// =======================================================================
// wrap_up: verify DB state after successful operations
// =======================================================================

#[tokio::test]
async fn wrap_up_rebase_does_not_change_status() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let runner: Arc<dyn ProcessRunner> = DispatchScript::finish().no_remote().shared_runner();
    let state = Arc::new(McpState::new(
        McpDeps {
            db: db.clone(),
            runner,
            embedding_service: EmbeddingService::new_test(),
            data_dir: std::env::temp_dir(),
        },
        None,
    ));

    let task_id = db
        .create_task(CreateTaskRequest {
            title: "Rebase Done",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Review,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    db.patch_task(
        task_id,
        &db::TaskPatch::new().worktree(Some("/repo/.worktrees/1-rebase-done")),
    )
    .await
    .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "wrap_up",
            "arguments": { "task_id": task_id.0, "action": "rebase" }
        })),
    )
    .await;
    let text = extract_response_text(&resp);
    assert!(text.contains("wrap_up complete"));

    let task = db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Review,
        "wrap_up must not change status — exit_session owns the Done transition"
    );
}

#[tokio::test]
async fn wrap_up_rebase_does_not_recalculate_epic_status() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let runner: Arc<dyn ProcessRunner> = DispatchScript::finish().no_remote().shared_runner();
    let state = Arc::new(McpState::new(
        McpDeps {
            db: db.clone(),
            runner,
            embedding_service: EmbeddingService::new_test(),
            data_dir: std::env::temp_dir(),
        },
        None,
    ));

    let epic = db.create_epic("E", "", None).await.unwrap();
    let task_id = db
        .create_task(CreateTaskRequest {
            title: "Only Task",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Review,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    db.set_task_epic_id(task_id, Some(epic.id)).await.unwrap();
    db.patch_task(
        task_id,
        &db::TaskPatch::new().worktree(Some("/repo/.worktrees/1-only-task")),
    )
    .await
    .unwrap();
    db.recalculate_epic_status(epic.id).await.unwrap();
    let epic_status_before = db.get_epic(epic.id).await.unwrap().unwrap().status;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "wrap_up",
            "arguments": { "task_id": task_id.0, "action": "rebase" }
        })),
    )
    .await;
    assert!(resp.error.is_none(), "{:?}", resp.error);

    let epic_after = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(
        epic_after.status, epic_status_before,
        "wrap_up must not recalculate epic status — that runs at exit_session"
    );
}

#[tokio::test]
async fn wrap_up_accepts_string_task_id() {
    let db: Arc<dyn db::TaskStore> = Arc::new(Database::open_in_memory().await.unwrap());
    let runner: Arc<dyn ProcessRunner> = DispatchScript::finish().no_remote().shared_runner();
    let state = Arc::new(McpState::new(
        McpDeps {
            db: db.clone(),
            runner,
            embedding_service: EmbeddingService::new_test(),
            data_dir: std::env::temp_dir(),
        },
        None,
    ));

    let task_id = db
        .create_task(CreateTaskRequest {
            title: "T",
            description: "d",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Review,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    db.patch_task(
        task_id,
        &db::TaskPatch::new().worktree(Some("/repo/.worktrees/1-t")),
    )
    .await
    .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "wrap_up",
            "arguments": { "task_id": task_id.0.to_string(), "action": "rebase" }
        })),
    )
    .await;
    assert!(
        resp.error.is_none(),
        "wrap_up should accept string task_id: {:?}",
        resp.error
    );
}

// =======================================================================
// get_task: additional formatting checks
// =======================================================================

#[tokio::test]
async fn get_task_shows_all_fields() {
    let state = test_state().await;
    let epic = state
        .db_write()
        .create_epic("Parent Epic", "", None)
        .await
        .unwrap();
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Full Task",
            description: "detailed desc",
            repo_path: "/repo",
            plan: Some("/plan.md"),
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    state
        .db_write()
        .set_task_epic_id(task_id, Some(epic.id))
        .await
        .unwrap();
    let full_url = crate::models::TaskUrl::new(
        "https://github.com/org/repo/pull/5",
        crate::models::UrlType::Pr,
    );
    state
        .db_write()
        .patch_task(
            task_id,
            &db::TaskPatch::new()
                .worktree(Some("/repo/.worktrees/1-full"))
                .tmux_window(Some("task-1"))
                .url(Some(&full_url))
                .tag(Some(crate::models::TaskTag::Feature))
                .sort_order(Some(10)),
        )
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "get_task",
            "arguments": { "task_id": task_id.0 }
        })),
    )
    .await;
    let text = extract_response_text(&resp);
    assert!(text.contains("Full Task"), "should show title");
    assert!(text.contains("detailed desc"), "should show description");
    assert!(text.contains("/repo"), "should show repo path");
    assert!(text.contains("/plan.md"), "should show plan");
    assert!(text.contains("Parent Epic"), "should show epic title");
    assert!(
        text.contains("/repo/.worktrees/1-full"),
        "should show worktree"
    );
    assert!(text.contains("task-1"), "should show tmux window");
    assert!(text.contains("pull/5"), "should show PR URL");
    assert!(text.contains("feature"), "should show tag");
    assert!(text.contains("Sort order: 10"), "should show sort order");
}

#[tokio::test]
async fn get_task_without_epic_omits_epic_line() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Solo Task",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "get_task",
            "arguments": { "task_id": task_id.0 }
        })),
    )
    .await;
    let text = extract_response_text(&resp);
    assert!(
        !text.contains("Epic:"),
        "should not show Epic line for task without epic: {text}"
    );
}

// =======================================================================
// list_tasks: format verification
// =======================================================================

#[tokio::test]
async fn list_tasks_shows_tag_and_plan_indicators() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Tagged Planned",
            description: "desc",
            repo_path: "/repo",
            plan: Some("/plan.md"),
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    state
        .db_write()
        .patch_task(
            task_id,
            &db::TaskPatch::new().tag(Some(crate::models::TaskTag::Bug)),
        )
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_tasks", "arguments": {} })),
    )
    .await;
    let text = extract_response_text(&resp);
    // [plan] indicator replaced by | Goal: <goal text> when plan is readable;
    // when the plan file doesn't exist the description is shown as fallback.
    assert!(
        !text.contains("[plan]"),
        "old [plan] badge should no longer appear: {text}"
    );
    assert!(text.contains("[bug]"), "should show tag indicator: {text}");
}

#[tokio::test]
async fn list_tasks_shows_epic_indicator() {
    let state = test_state().await;
    let epic = state
        .db_write()
        .create_epic("Sprint 1", "", None)
        .await
        .unwrap();
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Epic Task",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    state
        .db_write()
        .set_task_epic_id(task_id, Some(epic.id))
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_tasks", "arguments": {} })),
    )
    .await;
    let text = extract_response_text(&resp);
    assert!(
        text.contains("Sprint 1"),
        "should show epic title in list: {text}"
    );
}

#[tokio::test]
async fn list_tasks_truncates_long_descriptions() {
    let state = test_state().await;
    let long_desc = "x".repeat(300);
    state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Long Desc",
            description: &long_desc,
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_tasks", "arguments": {} })),
    )
    .await;
    let text = extract_response_text(&resp);
    assert!(
        text.contains("..."),
        "should truncate long description: {text}"
    );
    assert!(
        text.len() < long_desc.len() + 100,
        "truncated output should be shorter than full description"
    );
}

#[tokio::test]
async fn list_tasks_excludes_archived_by_default() {
    let state = test_state().await;
    state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Active Task",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Archived Task",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Archived,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_tasks", "arguments": {} })),
    )
    .await;
    assert!(resp.error.is_none());
    let text = extract_response_text(&resp);
    assert!(text.contains("Active Task"), "should show active task");
    assert!(
        !text.contains("Archived Task"),
        "should not show archived task: {text}"
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

// ---------------------------------------------------------------------------
// base_branch: create_task and update_task MCP schema tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_task_with_base_branch_stores_it() {
    let state = test_state().await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_task",
            "arguments": {
                "title": "My Feature",
                "repo_path": "/repo",
                "base_branch": "develop",
            }
        })),
    )
    .await;

    assert!(resp.error.is_none(), "{:?}", resp.error);
    let tasks = state.db.list_all().await.unwrap();
    let task = tasks.iter().find(|t| t.title == "My Feature").unwrap();
    assert_eq!(task.base_branch, "develop");
}

#[tokio::test]
async fn create_task_without_base_branch_defaults_to_main() {
    let state = test_state().await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_task",
            "arguments": {
                "title": "Default Branch Task",
                "repo_path": "/repo",
            }
        })),
    )
    .await;

    assert!(resp.error.is_none(), "{:?}", resp.error);
    let tasks = state.db.list_all().await.unwrap();
    let task = tasks
        .iter()
        .find(|t| t.title == "Default Branch Task")
        .unwrap();
    assert_eq!(task.base_branch, "main");
}

#[tokio::test]
async fn update_task_with_base_branch_updates_it() {
    let state = test_state().await;

    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "T",
            description: "d",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": {
                "task_id": task_id.0,
                "base_branch": "release/2.0"
            }
        })),
    )
    .await;

    assert!(resp.error.is_none(), "{:?}", resp.error);
    let task = state.db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(task.base_branch, "release/2.0");
}

// `dispatch_next_returns_disabled_when_auto_dispatch_off` moved to
// tests/tasks/dispatch.rs as `exit_session_does_not_chain_when_auto_dispatch_off`
// — the auto_dispatch flag is now read by the session-close chain, not by a tool.

// -- list_tasks: header-based caller identity ---------------------------------

#[tokio::test]
async fn list_tasks_task_identity_scopes_to_epic_and_excludes_self() {
    let (state, db) = test_state_with_db().await;
    let eid = db.create_epic("e", "", None).await.unwrap().id;
    let me = db
        .create_task(CreateTaskRequest {
            title: "me",
            description: "",
            repo_path: "/r",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: Some(eid),
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let sibling = db
        .create_task(CreateTaskRequest {
            title: "sibling",
            description: "",
            repo_path: "/r",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: Some(eid),
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let _unrelated = db
        .create_task(CreateTaskRequest {
            title: "unrelated",
            description: "",
            repo_path: "/r",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call_as(
        &state,
        "tools/call",
        Some(json!({ "name": "list_tasks", "arguments": {} })),
        CallerIdentity::Task(me),
    )
    .await;

    let text = extract_response_text(&resp);
    // sibling is in scope (same epic); me is excluded (self); unrelated is out of scope.
    assert!(
        text.contains(&format!("[{}]", sibling.0)),
        "expected sibling in:\n{text}"
    );
    assert!(
        !text.contains(&format!("[{}]", me.0)),
        "self should be excluded:\n{text}"
    );
}

#[tokio::test]
async fn list_tasks_task_identity_scopes_to_project_when_no_epic() {
    let (state, db) = test_state_with_db().await;
    let me = db
        .create_task(CreateTaskRequest {
            title: "me",
            description: "",
            repo_path: "/r",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let sibling = db
        .create_task(CreateTaskRequest {
            title: "sib",
            description: "",
            repo_path: "/r",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call_as(
        &state,
        "tools/call",
        Some(json!({ "name": "list_tasks", "arguments": {} })),
        CallerIdentity::Task(me),
    )
    .await;
    let text = extract_response_text(&resp);
    assert!(
        text.contains(&format!("[{}]", sibling.0)),
        "expected sibling:\n{text}"
    );
    assert!(
        !text.contains(&format!("[{}]", me.0)),
        "self excluded:\n{text}"
    );
}

#[tokio::test]
async fn list_tasks_session_identity_sees_all_tasks() {
    let (state, db) = test_state_with_db().await;
    db.create_task(CreateTaskRequest {
        title: "t1",
        description: "",
        repo_path: "/r",
        plan: None,
        status: TaskStatus::Backlog,
        base_branch: "main",
        epic_id: None,
        sort_order: None,
        tag: None,
        wrap_up_mode: None,
        auto_run_plan: false,
    })
    .await
    .unwrap();
    db.create_task(CreateTaskRequest {
        title: "t2",
        description: "",
        repo_path: "/r",
        plan: None,
        status: TaskStatus::Backlog,
        base_branch: "main",
        epic_id: None,
        sort_order: None,
        tag: None,
        wrap_up_mode: None,
        auto_run_plan: false,
    })
    .await
    .unwrap();

    let resp = call_as(
        &state,
        "tools/call",
        Some(json!({ "name": "list_tasks", "arguments": {} })),
        CallerIdentity::Session,
    )
    .await;
    let text = extract_response_text(&resp);
    assert!(text.contains("t1"), "got:\n{text}");
    assert!(text.contains("t2"), "got:\n{text}");
}

#[tokio::test]
async fn list_tasks_repo_paths_filter() {
    let state = test_state().await;

    state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Repo A task",
            description: "",
            repo_path: "/repo/a",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Repo B task",
            description: "",
            repo_path: "/repo/b",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "list_tasks",
            "arguments": { "repo_paths": ["/repo/a"] }
        })),
    )
    .await;

    let text = extract_response_text(&resp);
    assert!(text.contains("Repo A task"));
    assert!(!text.contains("Repo B task"));
}

#[tokio::test]
async fn list_tasks_includes_pr_url_in_output() {
    let state = test_state().await;

    let task_id = create_task_fixture(&state).await;
    let url = crate::models::TaskUrl::new(
        "https://github.com/org/repo/pull/42",
        crate::models::UrlType::Pr,
    );
    state
        .db_write()
        .patch_task(task_id, &crate::db::TaskPatch::new().url(Some(&url)))
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_tasks", "arguments": {} })),
    )
    .await;

    let text = extract_response_text(&resp);
    assert!(
        text.contains("| PR #42: https://github.com/org/repo/pull/42"),
        "PR URL should appear in output; got: {text}"
    );
}

#[tokio::test]
async fn list_tasks_includes_plan_goal_in_output() {
    let state = test_state().await;

    let plan_path = std::env::temp_dir().join("dispatch_test_plan_345.md");
    std::fs::write(
        &plan_path,
        "# My Feature — Implementation Plan\n\n**Goal:** Implement the learning enrichment.\n",
    )
    .unwrap();
    let plan_path_str = plan_path.to_string_lossy().to_string();

    state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Feature task",
            description: "desc",
            repo_path: "/repo",
            plan: Some(&plan_path_str),
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_tasks", "arguments": {} })),
    )
    .await;

    let text = extract_response_text(&resp);
    assert!(
        text.contains("| Goal: Implement the learning enrichment."),
        "Plan goal should appear in output; got: {text}"
    );

    let _ = std::fs::remove_file(&plan_path);
}

#[tokio::test]
async fn list_tasks_falls_back_to_description_when_no_plan() {
    let state = test_state().await;

    state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "No Plan Task",
            description: "A task without a plan file",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_tasks", "arguments": {} })),
    )
    .await;

    let text = extract_response_text(&resp);
    assert!(
        text.contains("A task without a plan file"),
        "Description should appear as fallback; got: {text}"
    );
}

#[tokio::test]
async fn list_tasks_omits_pr_segment_when_no_pr_url() {
    let state = test_state().await;
    create_task_fixture(&state).await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "list_tasks", "arguments": {} })),
    )
    .await;

    let text = extract_response_text(&resp);
    assert!(
        !text.contains("| PR:"),
        "No PR segment should appear when pr_url is null; got: {text}"
    );
}

// -- update_task PR-finalisation nudge tests -------------------------------
//
// When the agent records a freshly-created PR via update_task (per the
// agent-driven /wrap-up flow), the response should append the same
// reflection nudge that the rebase wrap_up emits — i.e. when pr_url
// transitions from null to a value AND status is being set to review.

#[tokio::test]
async fn update_task_pr_finalisation_appends_reflection_nudge_by_default() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "PR finalise",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": {
                "task_id": task_id.0,
                "url": "https://github.com/org/repo/pull/7", "url_type": "pr",
                "status": "review"
            }
        })),
    )
    .await;

    let text = extract_response_text(&resp);
    assert!(
        text.contains("record_learning"),
        "nudge should appear when finalising a PR via update_task; got: {text}"
    );
}

#[tokio::test]
async fn update_task_pr_finalisation_omits_nudge_when_disabled() {
    let state = test_state().await;
    state
        .db
        .set_setting_bool("learning_reflection_enabled", false)
        .await
        .unwrap();
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "PR finalise disabled",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": {
                "task_id": task_id.0,
                "url": "https://github.com/org/repo/pull/7", "url_type": "pr",
                "status": "review"
            }
        })),
    )
    .await;

    let text = extract_response_text(&resp);
    assert!(
        !text.contains("record_learning"),
        "nudge must not appear when reflection disabled; got: {text}"
    );
}

#[tokio::test]
async fn update_task_pr_set_without_status_does_not_nudge() {
    // Agent setting only pr_url (no status transition) is not a wrap-up
    // finalisation — don't nudge. This preserves current update_task UX
    // for non-wrap-up callers tweaking the URL.
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "PR set no status",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": {
                "task_id": task_id.0,
                "url": "https://github.com/org/repo/pull/7", "url_type": "pr"
            }
        })),
    )
    .await;

    let text = extract_response_text(&resp);
    assert!(
        !text.contains("record_learning"),
        "nudge must not appear when status is not transitioning; got: {text}"
    );
}

#[tokio::test]
async fn update_task_status_review_without_pr_url_change_does_not_nudge() {
    // Re-confirming a task to review without setting a new pr_url is
    // not a wrap-up finalisation. No nudge.
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "Already in review",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": {
                "task_id": task_id.0,
                "status": "review"
            }
        })),
    )
    .await;

    let text = extract_response_text(&resp);
    assert!(
        !text.contains("record_learning"),
        "nudge must not appear without pr_url transition; got: {text}"
    );
}

#[tokio::test]
async fn update_task_pr_url_already_set_does_not_nudge_again() {
    // The nudge should fire only on the first null->set transition.
    // Subsequent updates to pr_url (e.g. correcting the URL) must not
    // re-nudge.
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "PR already set",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Review,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();
    let url = crate::models::TaskUrl::new(
        "https://github.com/org/repo/pull/1",
        crate::models::UrlType::Pr,
    );
    state
        .db_write()
        .patch_task(task_id, &db::TaskPatch::new().url(Some(&url)))
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({
            "name": "update_task",
            "arguments": {
                "task_id": task_id.0,
                "url": "https://github.com/org/repo/pull/2", "url_type": "pr",
                "status": "review"
            }
        })),
    )
    .await;

    let text = extract_response_text(&resp);
    assert!(
        !text.contains("record_learning"),
        "nudge must not fire when pr_url was already set; got: {text}"
    );
}

// -- create_task: header-based caller identity --------------------------------

fn extract_created_task_id(resp: &JsonRpcResponse) -> crate::models::TaskId {
    let result = resp.result.as_ref().expect("expected ok response");
    let text = result["content"][0]["text"].as_str().expect("text field");
    // "Task <id> created"
    let id_str = text
        .strip_prefix("Task ")
        .and_then(|s| s.strip_suffix(" created"))
        .expect("expected 'Task <id> created'");
    crate::models::TaskId(id_str.parse().expect("numeric id"))
}

#[tokio::test]
async fn create_task_task_identity_inherits_epic() {
    let (state, _db) = test_state_with_db().await;
    // Create parent task with an epic; child should inherit the epic.
    let parent_epic = state
        .db_write()
        .create_epic("parent epic", "", None)
        .await
        .unwrap();
    let parent = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "parent",
            description: "",
            repo_path: "/r",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: Some(parent_epic.id),
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call_as(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_task",
            "arguments": { "title": "child", "repo_path": "/r" }
        })),
        CallerIdentity::Task(parent),
    )
    .await;

    let new_id = extract_created_task_id(&resp);
    let t = state.db.get_task(new_id).await.unwrap().unwrap();
    assert_eq!(t.epic_id, Some(parent_epic.id));
}

#[tokio::test]
async fn create_task_explicit_null_epic_clears_inheritance() {
    let (state, db) = test_state_with_db().await;
    let parent_epic = db.create_epic("e", "", None).await.unwrap();
    let parent = db
        .create_task(CreateTaskRequest {
            title: "parent",
            description: "",
            repo_path: "/r",
            plan: None,
            status: TaskStatus::Running,
            base_branch: "main",
            epic_id: Some(parent_epic.id),
            sort_order: None,
            tag: None,
            wrap_up_mode: None,
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call_as(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_task",
            "arguments": { "title": "t", "repo_path": "/r", "epic_id": null }
        })),
        CallerIdentity::Task(parent),
    )
    .await;
    let new_id = extract_created_task_id(&resp);
    let t = db.get_task(new_id).await.unwrap().unwrap();
    assert_eq!(t.epic_id, None);
}

#[tokio::test]
async fn create_task_unknown_caller_identity_returns_error() {
    let state = test_state().await;
    let resp = call_as(
        &state,
        "tools/call",
        Some(json!({
            "name": "create_task",
            "arguments": { "title": "t", "repo_path": "/r" }
        })),
        CallerIdentity::Task(crate::models::TaskId(99999)),
    )
    .await;
    assert!(is_error(&resp));
    let msg = error_message(&resp);
    assert!(msg.to_lowercase().contains("caller"), "got {msg}");
}

#[tokio::test]
async fn get_task_shows_wrap_up_mode_when_set() {
    let state = test_state().await;
    let task_id = state
        .db_write()
        .create_task(CreateTaskRequest {
            title: "T",
            description: "",
            repo_path: "/repo",
            plan: None,
            status: TaskStatus::Backlog,
            base_branch: "main",
            epic_id: None,
            sort_order: None,
            tag: None,
            wrap_up_mode: Some(crate::models::WrapUpMode::Rebase),
            auto_run_plan: false,
        })
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "get_task", "arguments": { "task_id": task_id.0 } })),
    )
    .await;
    let text = extract_response_text(&resp);
    assert!(
        text.to_lowercase().contains("wrap-up") || text.to_lowercase().contains("wrap_up"),
        "expected wrap-up mode in output, got: {text}"
    );
    assert!(
        text.contains("rebase"),
        "expected rebase in output, got: {text}"
    );
}

#[tokio::test]
async fn get_task_shows_verify_command_when_configured() {
    let state = test_state().await;
    let task_id = create_task_fixture(&state).await;
    state
        .db_write()
        .set_verify_command("/repo", Some("cargo test"))
        .await
        .unwrap();

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "get_task", "arguments": { "task_id": task_id.0 } })),
    )
    .await;
    let text = extract_response_text(&resp);
    assert!(
        text.contains("Verify command: cargo test"),
        "expected 'Verify command: cargo test' in output, got: {text}"
    );
}

#[tokio::test]
async fn get_task_omits_verify_command_when_unconfigured() {
    let state = test_state().await;
    let task_id = create_task_fixture(&state).await;

    let resp = call(
        &state,
        "tools/call",
        Some(json!({ "name": "get_task", "arguments": { "task_id": task_id.0 } })),
    )
    .await;
    let text = extract_response_text(&resp);
    assert!(
        !text.contains("Verify command"),
        "expected no 'Verify command' line in output, got: {text}"
    );
}
