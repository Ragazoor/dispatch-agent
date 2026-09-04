#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;

// --- Epic CRUD ---

#[tokio::test]
async fn create_repo_group_sub_epic_is_idempotent_and_unarchives() {
    let db = crate::db::Database::open_in_memory().await.unwrap();
    let root = db.create_epic("root", "", None).await.unwrap();

    let a1 = db
        .create_repo_group_sub_epic(root.id, "repo-a")
        .await
        .unwrap();
    let a2 = db
        .create_repo_group_sub_epic(root.id, "repo-a")
        .await
        .unwrap();
    assert_eq!(a1, a2, "same (parent,title) must return the same sub-epic");

    let sub = db.get_epic(a1).await.unwrap().unwrap();
    assert_eq!(sub.origin, crate::models::EpicOrigin::RepoGroup);
    assert_eq!(sub.parent_epic_id, Some(root.id));

    // Archive it, then re-request: must unarchive and reuse, not create a duplicate.
    db.patch_epic(
        a1,
        &crate::db::EpicPatch::new().status(crate::models::TaskStatus::Archived),
    )
    .await
    .unwrap();
    let a3 = db
        .create_repo_group_sub_epic(root.id, "repo-a")
        .await
        .unwrap();
    assert_eq!(a3, a1, "archived RepoGroup sub-epic must be reused");
    let reused = db.get_epic(a3).await.unwrap().unwrap();
    assert_ne!(
        reused.status,
        crate::models::TaskStatus::Archived,
        "must be unarchived"
    );

    let subs = db.list_sub_epics(root.id).await.unwrap();
    assert_eq!(subs.len(), 1, "no duplicate sub-epics created");
}

#[tokio::test]
async fn create_managed_role_epic_is_idempotent() {
    let db = crate::db::Database::open_in_memory().await.unwrap();

    let a1 = db
        .create_managed_role_epic(
            "PR Reviews",
            None,
            crate::models::FeedRole::ReviewsParent,
            Some("/scripts/fetch-reviews.sh"),
            Some(300),
        )
        .await
        .unwrap();
    let a2 = db
        .create_managed_role_epic(
            "PR Reviews",
            None,
            crate::models::FeedRole::ReviewsParent,
            Some("/scripts/fetch-reviews.sh"),
            Some(300),
        )
        .await
        .unwrap();
    assert_eq!(
        a1, a2,
        "a lost race on (parent, feed_role) must be a no-op, not a duplicate"
    );

    let epic = db.get_epic(a1).await.unwrap().unwrap();
    assert_eq!(epic.feed_role, crate::models::FeedRole::ReviewsParent);
    assert_eq!(
        epic.feed_command.as_deref(),
        Some("/scripts/fetch-reviews.sh")
    );
    assert_eq!(epic.feed_interval_secs, Some(300));

    let all = db.list_epics().await.unwrap();
    assert_eq!(
        all.iter()
            .filter(|e| e.feed_role == crate::models::FeedRole::ReviewsParent)
            .count(),
        1,
        "no orphaned untagged epic left behind by the lost race"
    );
}

#[tokio::test]
async fn create_epic_defaults_feed_role_none() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    assert_eq!(epic.feed_role, crate::models::FeedRole::None);
}

#[tokio::test]
async fn create_epic_defaults_origin_manual_and_patch_sets_repo_group() {
    let db = crate::db::Database::open_in_memory().await.unwrap();
    let root = db.create_epic("root", "", None).await.unwrap();
    assert_eq!(root.origin, crate::models::EpicOrigin::Manual);

    let patch = crate::db::EpicPatch::new().origin(crate::models::EpicOrigin::RepoGroup);
    db.patch_epic(root.id, &patch).await.unwrap();
    let reloaded = db.get_epic(root.id).await.unwrap().unwrap();
    assert_eq!(reloaded.origin, crate::models::EpicOrigin::RepoGroup);
}

#[tokio::test]
async fn create_and_get_epic() {
    let db = in_memory_db().await;
    let epic = db
        .create_epic("Auth Rewrite", "Rewrite auth", None)
        .await
        .unwrap();
    assert_eq!(epic.title, "Auth Rewrite");
    assert_eq!(epic.description, "Rewrite auth");
    assert_eq!(epic.status, TaskStatus::Backlog);

    let fetched = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(fetched.id, epic.id);
    assert_eq!(fetched.title, "Auth Rewrite");
}

#[tokio::test]
async fn list_epics() {
    let db = in_memory_db().await;
    db.create_epic("Epic A", "desc", None).await.unwrap();
    db.create_epic("Epic B", "desc", None).await.unwrap();
    let epics = db.list_epics().await.unwrap();
    assert_eq!(epics.len(), 2);
}

#[tokio::test]
async fn get_epic_nonexistent() {
    let db = in_memory_db().await;
    assert!(db.get_epic(EpicId(999)).await.unwrap().is_none());
}

#[tokio::test]
async fn delete_epic_cascades_subtasks() {
    let db = in_memory_db().await;
    let epic = db.create_epic("Epic", "desc", None).await.unwrap();
    db.create_task(CreateTaskRequest {
        title: "Sub 1",
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
        phoenix: false,
    })
    .await
    .unwrap();
    let sub_id = db
        .create_task(CreateTaskRequest {
            title: "Sub 2",
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
            phoenix: false,
        })
        .await
        .unwrap();

    // Link sub 2 to epic
    db.set_task_epic_id(sub_id, Some(epic.id)).await.unwrap();

    db.delete_epic(epic.id).await.unwrap();

    // Epic should be gone
    assert!(db.get_epic(epic.id).await.unwrap().is_none());
    // Sub 2 (linked to epic) should be deleted
    assert!(db.get_task(sub_id).await.unwrap().is_none());
    // Sub 1 (not linked) should still exist
    assert_eq!(db.list_all().await.unwrap().len(), 1);
}

#[tokio::test]
async fn delete_epic_with_sub_epics_succeeds() {
    let db = in_memory_db().await;
    let parent = db.create_epic("Parent", "", None).await.unwrap();
    let child = db.create_epic("Child", "", Some(parent.id)).await.unwrap();
    let task_id = db
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
            wrap_up_mode: None,
            auto_run_plan: false,
            phoenix: false,
        })
        .await
        .unwrap();
    db.set_task_epic_id(task_id, Some(child.id)).await.unwrap();

    db.delete_epic(parent.id)
        .await
        .expect("delete_epic with sub-epics should succeed");

    assert!(db.get_epic(parent.id).await.unwrap().is_none());
    assert!(db.get_epic(child.id).await.unwrap().is_none());
    assert!(db.get_task(task_id).await.unwrap().is_none());
}

#[tokio::test]
async fn delete_epic_multi_level_sub_epics() {
    let db = in_memory_db().await;
    let root = db.create_epic("Root", "", None).await.unwrap();
    let child = db.create_epic("Child", "", Some(root.id)).await.unwrap();
    let grandchild = db
        .create_epic("Grandchild", "", Some(child.id))
        .await
        .unwrap();

    db.delete_epic(root.id)
        .await
        .expect("deep delete should succeed");

    assert!(db.get_epic(root.id).await.unwrap().is_none());
    assert!(db.get_epic(child.id).await.unwrap().is_none());
    assert!(db.get_epic(grandchild.id).await.unwrap().is_none());
    assert_eq!(db.list_epics().await.unwrap().len(), 0);
}

#[tokio::test]
async fn epic_has_status_field() {
    let db = Database::open_in_memory().await.unwrap();
    let epic = db.create_epic("Test", "Desc", None).await.unwrap();
    assert_eq!(epic.status, TaskStatus::Backlog);
}

#[tokio::test]
async fn patch_epic_status() {
    let db = Database::open_in_memory().await.unwrap();
    let epic = db.create_epic("Test", "Desc", None).await.unwrap();
    db.patch_epic(epic.id, &EpicPatch::new().status(TaskStatus::Running))
        .await
        .unwrap();
    let epic = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(epic.status, TaskStatus::Running);
}

#[tokio::test]
async fn patch_epic_feed_role_persists() {
    let db = in_memory_db().await;
    let e = db.create_epic("E", "", None).await.unwrap();
    db.patch_epic(
        e.id,
        &EpicPatch::new().feed_role(crate::models::FeedRole::ReviewsParent),
    )
    .await
    .unwrap();
    let got = db.get_epic(e.id).await.unwrap().unwrap();
    assert_eq!(got.feed_role, crate::models::FeedRole::ReviewsParent);
}

#[tokio::test]
async fn patch_epic_title() {
    let db = in_memory_db().await;
    let epic = db.create_epic("Old Title", "desc", None).await.unwrap();

    db.patch_epic(epic.id, &EpicPatch::new().title("New Title"))
        .await
        .unwrap();
    let updated = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(updated.title, "New Title");
    assert_eq!(updated.description, "desc"); // unchanged
}

#[tokio::test]
async fn task_epic_id_roundtrip() {
    let db = in_memory_db().await;
    let epic = db.create_epic("Epic", "desc", None).await.unwrap();
    let task_id = db
        .create_task(CreateTaskRequest {
            title: "Task",
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
            phoenix: false,
        })
        .await
        .unwrap();

    db.set_task_epic_id(task_id, Some(epic.id)).await.unwrap();
    let task = db.get_task(task_id).await.unwrap().unwrap();
    assert_eq!(task.epic_id, Some(epic.id));

    db.set_task_epic_id(task_id, None).await.unwrap();
    let task = db.get_task(task_id).await.unwrap().unwrap();
    assert!(task.epic_id.is_none());
}

#[tokio::test]
async fn list_tasks_for_epic() {
    let db = in_memory_db().await;
    let epic = db.create_epic("Epic", "desc", None).await.unwrap();
    let id1 = db
        .create_task(CreateTaskRequest {
            title: "Sub A",
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
            phoenix: false,
        })
        .await
        .unwrap();
    let _id2 = db
        .create_task(CreateTaskRequest {
            title: "Standalone",
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
            phoenix: false,
        })
        .await
        .unwrap();

    db.set_task_epic_id(id1, Some(epic.id)).await.unwrap();

    let subtasks = db.list_tasks_for_epic(epic.id).await.unwrap();
    assert_eq!(subtasks.len(), 1);
    assert_eq!(subtasks[0].title, "Sub A");
}

#[tokio::test]
async fn patch_epic_plan() {
    let db = in_memory_db().await;
    let epic = db.create_epic("Epic", "desc", None).await.unwrap();
    assert!(epic.plan_path.is_none());

    db.patch_epic(epic.id, &EpicPatch::new().plan_path(Some("docs/plan.md")))
        .await
        .unwrap();
    let updated = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(updated.plan_path.as_deref(), Some("docs/plan.md"));
}

#[tokio::test]
async fn patch_epic_clear_plan() {
    let db = in_memory_db().await;
    let epic = db.create_epic("Epic", "desc", None).await.unwrap();

    db.patch_epic(epic.id, &EpicPatch::new().plan_path(Some("docs/plan.md")))
        .await
        .unwrap();
    db.patch_epic(epic.id, &EpicPatch::new().plan_path(None))
        .await
        .unwrap();
    let updated = db.get_epic(epic.id).await.unwrap().unwrap();
    assert!(updated.plan_path.is_none());
}

#[tokio::test]
async fn recalculate_epic_status_running_task_leaves_epic_in_backlog() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    assert_eq!(epic.status, TaskStatus::Backlog);

    let task = create_task_returning(&db, "T1", "", "/repo", None, TaskStatus::Backlog)
        .await
        .unwrap();
    db.set_task_epic_id(task.id, Some(epic.id)).await.unwrap();
    db.patch_task(task.id, &TaskPatch::new().status(TaskStatus::Running))
        .await
        .unwrap();

    db.recalculate_epic_status(epic.id).await.unwrap();
    let epic = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(epic.status, TaskStatus::Backlog);
}

#[tokio::test]
async fn recalculate_epic_status_running_task_leaves_review_epic_unchanged() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    db.patch_epic(epic.id, &EpicPatch::new().status(TaskStatus::Review))
        .await
        .unwrap();

    let task = create_task_returning(&db, "T1", "", "/repo", None, TaskStatus::Backlog)
        .await
        .unwrap();
    db.set_task_epic_id(task.id, Some(epic.id)).await.unwrap();
    db.patch_task(task.id, &TaskPatch::new().status(TaskStatus::Running))
        .await
        .unwrap();

    db.recalculate_epic_status(epic.id).await.unwrap();
    let epic = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(epic.status, TaskStatus::Review); // manual placement preserved
}

#[tokio::test]
async fn recalculate_epic_status_backlog_task_leaves_review_epic_unchanged() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    db.patch_epic(epic.id, &EpicPatch::new().status(TaskStatus::Review))
        .await
        .unwrap();

    let task = create_task_returning(&db, "T1", "", "/repo", None, TaskStatus::Backlog)
        .await
        .unwrap();
    db.set_task_epic_id(task.id, Some(epic.id)).await.unwrap();

    db.recalculate_epic_status(epic.id).await.unwrap();
    let epic = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(epic.status, TaskStatus::Review); // manual placement preserved
}

#[tokio::test]
async fn recalculate_epic_status_review_epic_stays_when_one_task_done_one_running() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();

    let t1 = create_task_returning(&db, "T1", "", "/repo", None, TaskStatus::Backlog)
        .await
        .unwrap();
    db.set_task_epic_id(t1.id, Some(epic.id)).await.unwrap();
    db.patch_task(t1.id, &TaskPatch::new().status(TaskStatus::Running))
        .await
        .unwrap();

    let t2 = create_task_returning(&db, "T2", "", "/repo", None, TaskStatus::Backlog)
        .await
        .unwrap();
    db.set_task_epic_id(t2.id, Some(epic.id)).await.unwrap();
    db.patch_task(t2.id, &TaskPatch::new().status(TaskStatus::Done))
        .await
        .unwrap();

    db.patch_epic(epic.id, &EpicPatch::new().status(TaskStatus::Review))
        .await
        .unwrap();

    db.recalculate_epic_status(epic.id).await.unwrap();
    let epic = db.get_epic(epic.id).await.unwrap().unwrap();
    // t1 still running, t2 done — not all done, not done-regression → no change
    assert_eq!(epic.status, TaskStatus::Review);
}

#[tokio::test]
async fn recalculate_epic_status_all_done() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();

    let t1 = create_task_returning(&db, "T1", "", "/repo", None, TaskStatus::Backlog)
        .await
        .unwrap();
    let t2 = create_task_returning(&db, "T2", "", "/repo", None, TaskStatus::Backlog)
        .await
        .unwrap();
    db.set_task_epic_id(t1.id, Some(epic.id)).await.unwrap();
    db.set_task_epic_id(t2.id, Some(epic.id)).await.unwrap();
    db.patch_task(t1.id, &TaskPatch::new().status(TaskStatus::Done))
        .await
        .unwrap();
    db.patch_task(t2.id, &TaskPatch::new().status(TaskStatus::Done))
        .await
        .unwrap();

    db.recalculate_epic_status(epic.id).await.unwrap();
    let epic = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(epic.status, TaskStatus::Done);
}

#[tokio::test]
async fn recalculate_epic_status_all_done_sets_sort_order() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let t1 = create_task_returning(&db, "T1", "", "/repo", None, TaskStatus::Backlog)
        .await
        .unwrap();
    db.set_task_epic_id(t1.id, Some(epic.id)).await.unwrap();
    db.patch_task(t1.id, &TaskPatch::new().status(TaskStatus::Done))
        .await
        .unwrap();

    let before = chrono::Utc::now().timestamp_millis();
    db.recalculate_epic_status(epic.id).await.unwrap();
    let after = chrono::Utc::now().timestamp_millis();

    let epic = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(epic.status, TaskStatus::Done);
    let sort_order = epic
        .sort_order
        .expect("sort_order should be set on entering Done");
    assert!(
        (-after..=-before).contains(&sort_order),
        "sort_order {sort_order} should be -now_millis, within [{}, {}]",
        -after,
        -before
    );
}

#[tokio::test]
async fn recalculate_epic_status_done_regression_clears_sort_order() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    db.patch_epic(
        epic.id,
        &EpicPatch::new()
            .status(TaskStatus::Done)
            .sort_order(Some(-1)),
    )
    .await
    .unwrap();

    let task = create_task_returning(&db, "T1", "", "/repo", None, TaskStatus::Backlog)
        .await
        .unwrap();
    db.set_task_epic_id(task.id, Some(epic.id)).await.unwrap();
    db.patch_task(task.id, &TaskPatch::new().status(TaskStatus::Running))
        .await
        .unwrap();

    db.recalculate_epic_status(epic.id).await.unwrap();

    let epic = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(epic.status, TaskStatus::Backlog);
    assert_eq!(epic.sort_order, None);
}

#[tokio::test]
async fn recalculate_epic_status_already_done_noop_leaves_sort_order_untouched() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let task = create_task_returning(&db, "T1", "", "/repo", None, TaskStatus::Done)
        .await
        .unwrap();
    db.set_task_epic_id(task.id, Some(epic.id)).await.unwrap();
    db.patch_epic(
        epic.id,
        &EpicPatch::new()
            .status(TaskStatus::Done)
            .sort_order(Some(-42)),
    )
    .await
    .unwrap();

    db.recalculate_epic_status(epic.id).await.unwrap();

    let epic = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(epic.status, TaskStatus::Done);
    assert_eq!(
        epic.sort_order,
        Some(-42),
        "a no-op recalculation (already Done, still all-done) must not touch sort_order"
    );
}

#[tokio::test]
async fn recalculate_epic_status_mixed_review_done_leaves_epic_in_backlog() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();

    let t1 = create_task_returning(&db, "T1", "", "/repo", None, TaskStatus::Backlog)
        .await
        .unwrap();
    let t2 = create_task_returning(&db, "T2", "", "/repo", None, TaskStatus::Backlog)
        .await
        .unwrap();
    db.set_task_epic_id(t1.id, Some(epic.id)).await.unwrap();
    db.set_task_epic_id(t2.id, Some(epic.id)).await.unwrap();
    db.patch_task(t1.id, &TaskPatch::new().status(TaskStatus::Review))
        .await
        .unwrap();
    db.patch_task(t2.id, &TaskPatch::new().status(TaskStatus::Done))
        .await
        .unwrap();

    db.recalculate_epic_status(epic.id).await.unwrap();
    let epic = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(epic.status, TaskStatus::Backlog); // not all done → no auto-advance
}

#[tokio::test]
async fn recalculate_epic_status_mixed_review_running_leaves_epic_in_backlog() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();

    let t1 = create_task_returning(&db, "T1", "", "/repo", None, TaskStatus::Backlog)
        .await
        .unwrap();
    let t2 = create_task_returning(&db, "T2", "", "/repo", None, TaskStatus::Backlog)
        .await
        .unwrap();
    let t3 = create_task_returning(&db, "T3", "", "/repo", None, TaskStatus::Backlog)
        .await
        .unwrap();
    db.set_task_epic_id(t1.id, Some(epic.id)).await.unwrap();
    db.set_task_epic_id(t2.id, Some(epic.id)).await.unwrap();
    db.set_task_epic_id(t3.id, Some(epic.id)).await.unwrap();
    db.patch_task(t1.id, &TaskPatch::new().status(TaskStatus::Review))
        .await
        .unwrap();
    db.patch_task(t2.id, &TaskPatch::new().status(TaskStatus::Review))
        .await
        .unwrap();
    db.patch_task(t3.id, &TaskPatch::new().status(TaskStatus::Running))
        .await
        .unwrap();

    db.recalculate_epic_status(epic.id).await.unwrap();
    let epic = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(epic.status, TaskStatus::Backlog);
}

#[tokio::test]
async fn update_task_to_review_leaves_epic_in_backlog() {
    use crate::service::{TaskService, UpdateTaskParams};

    let db = std::sync::Arc::new(in_memory_db().await);
    let epic = db.create_epic("E", "", None).await.unwrap();
    let task = create_task_returning(&db, "T1", "", "/repo", None, TaskStatus::Running)
        .await
        .unwrap();
    db.set_task_epic_id(task.id, Some(epic.id)).await.unwrap();

    let svc = TaskService::new(db.clone(), crate::process::MockProcessRunner::unused());
    svc.update_task(UpdateTaskParams::for_task(task.id).status(TaskStatus::Review))
        .await
        .unwrap();

    let epic = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(epic.status, TaskStatus::Backlog);
}

#[tokio::test]
async fn update_task_to_running_leaves_epic_in_backlog() {
    use crate::service::{TaskService, UpdateTaskParams};

    let db = std::sync::Arc::new(in_memory_db().await);
    let epic = db.create_epic("E", "", None).await.unwrap();
    let task = create_task_returning(&db, "T1", "", "/repo", None, TaskStatus::Backlog)
        .await
        .unwrap();
    db.set_task_epic_id(task.id, Some(epic.id)).await.unwrap();

    let svc = TaskService::new(db.clone(), crate::process::MockProcessRunner::unused());
    svc.update_task(UpdateTaskParams::for_task(task.id).status(TaskStatus::Running))
        .await
        .unwrap();

    let epic = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(epic.status, TaskStatus::Backlog);
}

#[tokio::test]
async fn epic_stays_backlog_when_review_task_completes() {
    use crate::service::{TaskService, UpdateTaskParams};

    let db = std::sync::Arc::new(in_memory_db().await);
    let epic = db.create_epic("E", "", None).await.unwrap();

    let t1 = create_task_returning(&db, "T1", "", "/repo", None, TaskStatus::Running)
        .await
        .unwrap();
    let t2 = create_task_returning(&db, "T2", "", "/repo", None, TaskStatus::Review)
        .await
        .unwrap();
    db.set_task_epic_id(t1.id, Some(epic.id)).await.unwrap();
    db.set_task_epic_id(t2.id, Some(epic.id)).await.unwrap();
    db.recalculate_epic_status(epic.id).await.unwrap();
    assert_eq!(
        db.get_epic(epic.id).await.unwrap().unwrap().status,
        TaskStatus::Backlog // running+review tasks → no auto-advance
    );

    let svc = TaskService::new(db.clone(), crate::process::MockProcessRunner::unused());
    svc.update_task(UpdateTaskParams::for_task(t2.id).status(TaskStatus::Done))
        .await
        .unwrap();

    let epic = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(epic.status, TaskStatus::Backlog); // t1 still running → not all done
}

#[tokio::test]
async fn update_task_with_substatus_keeps_task_running_and_epic_in_backlog() {
    use crate::service::{TaskService, UpdateTaskParams};

    let db = std::sync::Arc::new(in_memory_db().await);
    let epic = db.create_epic("E", "", None).await.unwrap();
    let task = create_task_returning(&db, "T1", "", "/repo", None, TaskStatus::Running)
        .await
        .unwrap();
    db.set_task_epic_id(task.id, Some(epic.id)).await.unwrap();
    db.recalculate_epic_status(epic.id).await.unwrap();

    let svc = TaskService::new(db.clone(), crate::process::MockProcessRunner::unused());
    svc.update_task(
        UpdateTaskParams::for_task(task.id)
            .status(TaskStatus::Running)
            .sub_status(SubStatus::NeedsInput),
    )
    .await
    .unwrap();

    let epic = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(epic.status, TaskStatus::Backlog); // running task → epic stays in backlog

    let task = db.get_task(task.id).await.unwrap().unwrap();
    assert_eq!(task.sub_status, SubStatus::NeedsInput);
}

// ---------------------------------------------------------------------------
// Query coverage: patch_epic edge cases
// ---------------------------------------------------------------------------

#[tokio::test]
async fn patch_epic_nonexistent_errors() {
    let db = in_memory_db().await;
    let result = db
        .patch_epic(EpicId(9999), &EpicPatch::new().title("x"))
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn patch_epic_no_changes_is_noop() {
    let db = in_memory_db().await;
    let epic = db.create_epic("Title", "desc", None).await.unwrap();
    // Empty patch — has_changes() is false, so this should succeed without touching DB
    db.patch_epic(epic.id, &EpicPatch::new()).await.unwrap();
    let fetched = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(fetched.title, "Title");
}

#[tokio::test]
async fn patch_epic_sort_order() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "desc", None).await.unwrap();
    assert!(epic.sort_order.is_none());

    db.patch_epic(epic.id, &EpicPatch::new().sort_order(Some(42)))
        .await
        .unwrap();
    let updated = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(updated.sort_order, Some(42));

    // Clear sort_order
    db.patch_epic(epic.id, &EpicPatch::new().sort_order(None))
        .await
        .unwrap();
    let cleared = db.get_epic(epic.id).await.unwrap().unwrap();
    assert!(cleared.sort_order.is_none());
}

#[tokio::test]
async fn delete_epic_nonexistent_errors() {
    let db = in_memory_db().await;
    let result = db.delete_epic(EpicId(9999)).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn recalculate_epic_status_ignores_archived_subtasks() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();

    let t1 = create_task_returning(&db, "T1", "", "/repo", None, TaskStatus::Backlog)
        .await
        .unwrap();
    let t2 = create_task_returning(&db, "T2", "", "/repo", None, TaskStatus::Backlog)
        .await
        .unwrap();
    db.set_task_epic_id(t1.id, Some(epic.id)).await.unwrap();
    db.set_task_epic_id(t2.id, Some(epic.id)).await.unwrap();

    // t1 done, t2 archived — only non-archived counted, so all done → Done
    db.patch_task(t1.id, &TaskPatch::new().status(TaskStatus::Done))
        .await
        .unwrap();
    db.patch_task(t2.id, &TaskPatch::new().status(TaskStatus::Archived))
        .await
        .unwrap();

    db.recalculate_epic_status(epic.id).await.unwrap();
    let epic = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(epic.status, TaskStatus::Done);
}

#[tokio::test]
async fn recalculate_epic_status_done_regresses_to_backlog_when_running_task_added() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    db.patch_epic(epic.id, &EpicPatch::new().status(TaskStatus::Done))
        .await
        .unwrap();

    let task = create_task_returning(&db, "T1", "", "/repo", None, TaskStatus::Backlog)
        .await
        .unwrap();
    db.set_task_epic_id(task.id, Some(epic.id)).await.unwrap();
    db.patch_task(task.id, &TaskPatch::new().status(TaskStatus::Running))
        .await
        .unwrap();

    db.recalculate_epic_status(epic.id).await.unwrap();
    let epic = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(epic.status, TaskStatus::Backlog);
}

#[tokio::test]
async fn recalculate_epic_status_no_active_children_leaves_status_unchanged() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    // Manually push to running to test that recalc with no tasks doesn't reset it
    db.patch_epic(epic.id, &EpicPatch::new().status(TaskStatus::Running))
        .await
        .unwrap();

    db.recalculate_epic_status(epic.id).await.unwrap();
    let epic = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(epic.status, TaskStatus::Running);
}

#[tokio::test]
async fn recalculate_epic_status_done_epic_stays_done_when_all_tasks_archived() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();

    let task = create_task_returning(&db, "T1", "", "/repo", None, TaskStatus::Backlog)
        .await
        .unwrap();
    db.set_task_epic_id(task.id, Some(epic.id)).await.unwrap();
    db.patch_task(task.id, &TaskPatch::new().status(TaskStatus::Done))
        .await
        .unwrap();
    db.recalculate_epic_status(epic.id).await.unwrap();
    assert_eq!(
        db.get_epic(epic.id).await.unwrap().unwrap().status,
        TaskStatus::Done
    );

    // Archive the task — epic has no active children now
    db.patch_task(task.id, &TaskPatch::new().status(TaskStatus::Archived))
        .await
        .unwrap();
    db.recalculate_epic_status(epic.id).await.unwrap();
    let epic = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(epic.status, TaskStatus::Done);
}

#[tokio::test]
async fn recalculate_epic_status_leaves_archived_epic_unchanged_even_when_children_all_done() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    db.patch_epic(epic.id, &EpicPatch::new().status(TaskStatus::Archived))
        .await
        .unwrap();

    // A non-archived, all-done child attached to an archived epic must not
    // flip the epic's status to done — archived is terminal.
    let task = create_task_returning(&db, "T1", "", "/repo", None, TaskStatus::Backlog)
        .await
        .unwrap();
    db.set_task_epic_id(task.id, Some(epic.id)).await.unwrap();
    db.patch_task(task.id, &TaskPatch::new().status(TaskStatus::Done))
        .await
        .unwrap();

    db.recalculate_epic_status(epic.id).await.unwrap();
    let epic = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(epic.status, TaskStatus::Archived);
}

#[tokio::test]
async fn recalculate_epic_status_all_done_stays_done_when_already_done() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let task = create_task_returning(&db, "T1", "", "/repo", None, TaskStatus::Done)
        .await
        .unwrap();
    db.set_task_epic_id(task.id, Some(epic.id)).await.unwrap();
    db.patch_epic(epic.id, &EpicPatch::new().status(TaskStatus::Done))
        .await
        .unwrap();

    db.recalculate_epic_status(epic.id).await.unwrap();
    let epic = db.get_epic(epic.id).await.unwrap().unwrap();
    assert_eq!(epic.status, TaskStatus::Done);
}

#[tokio::test]
async fn recalculate_epic_status_nonexistent_is_noop() {
    let db = in_memory_db().await;
    // Should not error for nonexistent epic
    db.recalculate_epic_status(EpicId(9999)).await.unwrap();
}

#[tokio::test]
async fn patch_epic_auto_dispatch_persists() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "desc", None).await.unwrap();
    assert!(!epic.auto_dispatch); // default false

    db.patch_epic(epic.id, &EpicPatch::new().auto_dispatch(false))
        .await
        .unwrap();
    let updated = db.get_epic(epic.id).await.unwrap().unwrap();
    assert!(!updated.auto_dispatch);

    db.patch_epic(epic.id, &EpicPatch::new().auto_dispatch(true))
        .await
        .unwrap();
    let re_enabled = db.get_epic(epic.id).await.unwrap().unwrap();
    assert!(re_enabled.auto_dispatch);
}

#[tokio::test]
async fn patch_epic_feed_append_only_defaults_false() {
    let db = Database::open_in_memory().await.unwrap();
    let epic = db.create_epic("Test", "", None).await.unwrap();
    assert!(
        !epic.feed_append_only,
        "mirroring is the default; an epic opts in to append-only"
    );
}

#[tokio::test]
async fn patch_epic_feed_append_only_persists() {
    let db = Database::open_in_memory().await.unwrap();
    let epic = db.create_epic("Test", "", None).await.unwrap();
    db.patch_epic(epic.id, &EpicPatch::new().feed_append_only(true))
        .await
        .unwrap();
    let updated = db.get_epic(epic.id).await.unwrap().unwrap();
    assert!(updated.feed_append_only);
    db.patch_epic(epic.id, &EpicPatch::new().feed_append_only(false))
        .await
        .unwrap();
    let re_disabled = db.get_epic(epic.id).await.unwrap().unwrap();
    assert!(!re_disabled.feed_append_only);
}

#[tokio::test]
async fn patch_epic_group_by_repo_defaults_false() {
    let db = Database::open_in_memory().await.unwrap();
    let epic = db.create_epic("Test", "", None).await.unwrap();
    assert!(!epic.group_by_repo);
}

#[tokio::test]
async fn patch_epic_group_by_repo_persists() {
    let db = Database::open_in_memory().await.unwrap();
    let epic = db.create_epic("Test", "", None).await.unwrap();
    db.patch_epic(epic.id, &EpicPatch::new().group_by_repo(true))
        .await
        .unwrap();
    let updated = db.get_epic(epic.id).await.unwrap().unwrap();
    assert!(updated.group_by_repo);
    db.patch_epic(epic.id, &EpicPatch::new().group_by_repo(false))
        .await
        .unwrap();
    let re_disabled = db.get_epic(epic.id).await.unwrap().unwrap();
    assert!(!re_disabled.group_by_repo);
}

#[tokio::test]
async fn list_all_tasks_with_epic_id_returns_only_tasks_with_epic() {
    let db = in_memory_db().await;
    let epic1_id = db.create_epic("E1", "", None).await.unwrap().id;
    let epic2_id = db.create_epic("E2", "", None).await.unwrap().id;

    let t1 = db
        .create_task(CreateTaskRequest {
            title: "Task1",
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
    let t2 = db
        .create_task(CreateTaskRequest {
            title: "Task2",
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
    let _t3 = db
        .create_task(CreateTaskRequest {
            title: "Orphan",
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

    db.set_task_epic_id(t1, Some(epic1_id)).await.unwrap();
    db.set_task_epic_id(t2, Some(epic2_id)).await.unwrap();

    let tasks = db.list_all_tasks_with_epic_id().await.unwrap();
    assert_eq!(tasks.len(), 2);
    assert!(tasks.iter().any(|t| t.id == t1));
    assert!(tasks.iter().any(|t| t.id == t2));
}

// ---------------------------------------------------------------------------
// Epic-in-epic (nested epics)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sub_epic_has_parent_id() {
    let db = in_memory_db().await;
    let parent = db.create_epic("Parent", "desc", None).await.unwrap();
    let child = db
        .create_epic("Child", "desc", Some(parent.id))
        .await
        .unwrap();
    assert_eq!(child.parent_epic_id, Some(parent.id));

    let fetched = db.get_epic(child.id).await.unwrap().unwrap();
    assert_eq!(fetched.parent_epic_id, Some(parent.id));
}

#[tokio::test]
async fn root_epic_has_no_parent() {
    let db = in_memory_db().await;
    let root = db.create_epic("Root", "desc", None).await.unwrap();
    assert_eq!(root.parent_epic_id, None);
}

#[tokio::test]
async fn list_root_epics_excludes_sub_epics() {
    let db = in_memory_db().await;
    let parent = db.create_epic("Parent", "desc", None).await.unwrap();
    db.create_epic("Child", "desc", Some(parent.id))
        .await
        .unwrap();

    let roots = db.list_root_epics().await.unwrap();
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].id, parent.id);
}

#[tokio::test]
async fn list_sub_epics_returns_children() {
    let db = in_memory_db().await;
    let parent = db.create_epic("Parent", "desc", None).await.unwrap();
    let child1 = db
        .create_epic("Child 1", "desc", Some(parent.id))
        .await
        .unwrap();
    let child2 = db
        .create_epic("Child 2", "desc", Some(parent.id))
        .await
        .unwrap();
    // unrelated root epic — must not appear
    db.create_epic("Other", "desc", None).await.unwrap();

    let children = db.list_sub_epics(parent.id).await.unwrap();
    assert_eq!(children.len(), 2);
    let ids: Vec<_> = children.iter().map(|e| e.id).collect();
    assert!(ids.contains(&child1.id));
    assert!(ids.contains(&child2.id));
}

#[tokio::test]
async fn recalculate_parent_status_from_sub_epic() {
    let db = in_memory_db().await;
    let parent = db.create_epic("Parent", "desc", None).await.unwrap();
    let child = db
        .create_epic("Child", "desc", Some(parent.id))
        .await
        .unwrap();

    // Add a task to the sub-epic and move it to running
    let task_id = db
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
            phoenix: false,
        })
        .await
        .unwrap();
    db.set_task_epic_id(task_id, Some(child.id)).await.unwrap();
    db.patch_task(
        task_id,
        &TaskPatch::new()
            .status(TaskStatus::Running)
            .sub_status(crate::models::SubStatus::Active),
    )
    .await
    .unwrap();

    // Recalculating the sub-epic should also propagate up to the parent.
    // Running tasks do not auto-advance epics — both stay in backlog.
    db.recalculate_epic_status(child.id).await.unwrap();

    let updated_child = db.get_epic(child.id).await.unwrap().unwrap();
    assert_eq!(updated_child.status, TaskStatus::Backlog);

    let updated_parent = db.get_epic(parent.id).await.unwrap().unwrap();
    assert_eq!(updated_parent.status, TaskStatus::Backlog);
}

#[tokio::test]
async fn recalculate_epic_status_terminates_on_cycle() {
    // Manually create a cycle at the DB level by bypassing FK checks,
    // then verify recalculate_epic_status returns Ok(()) rather than hanging.
    let db = in_memory_db().await;
    db.db_call(|conn| {
        conn.execute_batch("PRAGMA foreign_keys = OFF;")
            .map_err(anyhow::Error::from)
    })
    .await
    .unwrap();
    let a = db.create_epic("A", "", None).await.unwrap();
    let b = db.create_epic("B", "", Some(a.id)).await.unwrap();
    // Point a's parent back to b → a→b→a cycle
    let (a_id, b_id) = (a.id.0, b.id.0);
    db.db_call(move |conn| {
        conn.execute(
            "UPDATE epics SET parent_epic_id = ?1 WHERE id = ?2",
            rusqlite::params![b_id, a_id],
        )?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        Ok(())
    })
    .await
    .unwrap();
    // Must return without spinning. The timeout is the actual assertion: a
    // regressed cycle guard makes `recalculate_epic_status` loop forever, and a
    // bare `.await` would hang the test (or, on stack overflow, abort the whole
    // test binary and take every other test with it) instead of failing.
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        db.recalculate_epic_status(a.id),
    )
    .await
    .expect("recalculate_epic_status must terminate on an epic cycle, not spin");
    assert!(
        result.is_ok(),
        "expected Ok on a cyclic epic graph, got {result:?}"
    );
}

#[tokio::test]
async fn self_referential_epic_is_rejected() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "", None).await.unwrap();
    let eid = epic.id.0;
    let rejected = db
        .db_call(move |conn| {
            let result = conn.execute(
                "UPDATE epics SET parent_epic_id = id WHERE id = ?1",
                rusqlite::params![eid],
            );
            Ok(result.is_err())
        })
        .await
        .unwrap();
    assert!(rejected, "self-link should be rejected by CHECK constraint");
}

#[tokio::test]
async fn epic_patch_default_has_no_changes() {
    assert!(!EpicPatch::default().has_changes());
}

#[tokio::test]
async fn epic_patch_each_setter_marks_has_changes() {
    assert!(EpicPatch::new().title("t").has_changes());
    assert!(EpicPatch::new().description("d").has_changes());
    assert!(EpicPatch::new().status(TaskStatus::Running).has_changes());
    assert!(EpicPatch::new().plan_path(Some("p")).has_changes());
    assert!(EpicPatch::new().plan_path(None).has_changes());
    assert!(EpicPatch::new().sort_order(Some(1)).has_changes());
    assert!(EpicPatch::new().sort_order(None).has_changes());
    assert!(EpicPatch::new().auto_dispatch(true).has_changes());
    assert!(EpicPatch::new().group_by_repo(true).has_changes());
    assert!(EpicPatch::new().feed_append_only(true).has_changes());
    assert!(EpicPatch::new().feed_command(Some("cmd")).has_changes());
    assert!(EpicPatch::new().feed_command(None).has_changes());
    assert!(EpicPatch::new().feed_interval_secs(Some(60)).has_changes());
    assert!(EpicPatch::new().feed_interval_secs(None).has_changes());
}

#[tokio::test]
async fn row_to_epic_auto_dispatch_defaults_to_false() {
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "D", None).await.unwrap();
    assert!(!epic.auto_dispatch, "auto_dispatch should default to false");
}

#[tokio::test]
async fn get_epic_errors_on_corrupt_auto_dispatch_type() {
    // Regression: row.get::<_, bool>("auto_dispatch").unwrap_or(true) silently
    // returned true when the column held a non-boolean value. Now uses `?` so
    // schema drift surfaces immediately.
    let db = in_memory_db().await;
    let epic = db.create_epic("E", "D", None).await.unwrap();
    let epic_id = epic.id.0;
    db.db_call(move |conn| {
        conn.execute(
            "UPDATE epics SET auto_dispatch = 'not-a-bool' WHERE id = ?1",
            rusqlite::params![epic_id],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let result = db.get_epic(epic.id).await;
    assert!(
        result.is_err(),
        "expected Err when auto_dispatch holds a non-boolean value, got {:?}",
        result
    );
}

// ---------------------------------------------------------------------------
// Decode-failure policy (docs/conventions.md): bulk epic reads skip
// undecodable rows; `get_epic` fails loudly.
// ---------------------------------------------------------------------------

/// Plant an epic row with an unrecognised `status` alongside a healthy one.
async fn db_with_undecodable_epic_row() -> (Database, EpicId) {
    let db = in_memory_db().await;
    let good = db.create_epic("healthy", "", None).await.unwrap();
    write_corrupt_row(
        &db,
        "INSERT INTO epics (id, title, description, status, created_at, updated_at)
         VALUES (9101, 'corrupt', '', 'not_a_status',
                 '2026-01-01 00:00:00', '2026-01-01 00:00:00');",
    )
    .await;
    (db, good.id)
}

#[tokio::test]
async fn list_epics_skips_row_with_unrecognised_status() {
    let (db, good_id) = db_with_undecodable_epic_row().await;
    let epics = db
        .list_epics()
        .await
        .expect("one undecodable epic row must not fail the whole load");
    assert_eq!(
        epics.iter().map(|e| e.id).collect::<Vec<_>>(),
        vec![good_id]
    );
}

#[tokio::test]
async fn list_root_epics_skips_row_with_unrecognised_status() {
    let (db, good_id) = db_with_undecodable_epic_row().await;
    let epics = db.list_root_epics().await.unwrap();
    assert_eq!(
        epics.iter().map(|e| e.id).collect::<Vec<_>>(),
        vec![good_id]
    );
}

#[tokio::test]
async fn get_epic_errors_on_unrecognised_status() {
    let (db, _) = db_with_undecodable_epic_row().await;
    let result = db.get_epic(EpicId(9101)).await;
    assert!(
        result.is_err(),
        "a single-entity read must fail loudly, got {result:?}"
    );
}
