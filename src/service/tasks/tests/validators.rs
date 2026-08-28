use super::*;
use crate::models::test_tmux_window;

// --- FieldUpdate ---

#[tokio::test]
async fn field_update_set_has_value() {
    let fu: FieldUpdate = FieldUpdate::Set("hello".to_string());
    assert!(matches!(fu, FieldUpdate::Set(ref s) if s == "hello"));
}

#[tokio::test]
async fn field_update_clear_is_clear() {
    let fu: FieldUpdate = FieldUpdate::Clear;
    assert!(matches!(fu, FieldUpdate::Clear));
}

#[tokio::test]
async fn update_task_worktree_set_persists() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc
        .create_task(CreateTaskParams {
            title: "t".into(),
            description: "d".into(),
            repo_path: "/repo".to_string(),
            plan_path: None,
            epic_id: None,
            sort_order: None,
            tag: None,
            base_branch: None,
            wrap_up_mode: None,
            auto_run_plan: false,
            phoenix: false,
        })
        .await
        .unwrap();
    svc.update_task(
        UpdateTaskParams::for_task(id)
            .status(TaskStatus::Running)
            .worktree(FieldUpdate::Set("/wt".to_string()))
            .tmux_window(crate::service::TmuxWindowUpdate::Set(test_tmux_window(
                "win",
            ))),
    )
    .await
    .unwrap();
    let task = db.get_task(TaskId(id.0)).await.unwrap().unwrap();
    assert_eq!(task.worktree.as_deref(), Some("/wt"));
    assert_eq!(task.tmux_window.as_ref().map(|w| w.as_str()), Some("win"));
}

#[tokio::test]
async fn update_task_worktree_clear_sets_null() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc
        .create_task(CreateTaskParams {
            title: "t".into(),
            description: "d".into(),
            repo_path: "/repo".to_string(),
            plan_path: None,
            epic_id: None,
            sort_order: None,
            tag: None,
            base_branch: None,
            wrap_up_mode: None,
            auto_run_plan: false,
            phoenix: false,
        })
        .await
        .unwrap();
    // First set a value
    svc.update_task(
        UpdateTaskParams::for_task(id)
            .status(TaskStatus::Running)
            .worktree(FieldUpdate::Set("/wt".to_string()))
            .tmux_window(crate::service::TmuxWindowUpdate::Set(test_tmux_window(
                "win",
            ))),
    )
    .await
    .unwrap();
    // Then clear it
    svc.update_task(
        UpdateTaskParams::for_task(id)
            .worktree(FieldUpdate::Clear)
            .tmux_window(crate::service::TmuxWindowUpdate::Clear),
    )
    .await
    .unwrap();
    let task = db.get_task(TaskId(id.0)).await.unwrap().unwrap();
    assert_eq!(task.worktree, None);
    assert_eq!(task.tmux_window, None);
}

#[tokio::test]
async fn update_task_pr_url_set_and_clear() {
    let db = test_db().await;
    let svc = task_svc(&db);
    let id = svc
        .create_task(CreateTaskParams {
            title: "t".into(),
            description: "d".into(),
            repo_path: "/repo".to_string(),
            plan_path: None,
            epic_id: None,
            sort_order: None,
            tag: None,
            base_branch: None,
            wrap_up_mode: None,
            auto_run_plan: false,
            phoenix: false,
        })
        .await
        .unwrap();
    // Set PR URL
    svc.update_task(
        UpdateTaskParams::for_task(id).url(crate::service::UrlUpdate::Set(
            crate::models::TaskUrl::new(
                "https://github.com/org/repo/pull/1",
                crate::models::UrlType::Pr,
            ),
        )),
    )
    .await
    .unwrap();
    let task = db.get_task(TaskId(id.0)).await.unwrap().unwrap();
    assert_eq!(
        task.url.as_ref().map(|u| u.url.as_str()),
        Some("https://github.com/org/repo/pull/1")
    );
    // Clear PR URL
    svc.update_task(UpdateTaskParams::for_task(id).url(crate::service::UrlUpdate::Clear))
        .await
        .unwrap();
    let task = db.get_task(TaskId(id.0)).await.unwrap().unwrap();
    assert_eq!(task.url, None);
}
