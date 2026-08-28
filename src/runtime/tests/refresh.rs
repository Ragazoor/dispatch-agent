use super::*;

/// Event-loop: spawn_refresh_from_db sends board data via msg_tx
mod spawn_refresh_from_db_via_msg_tx {
    use super::*;

    #[tokio::test]
    async fn spawn_refresh_from_db_sends_task_refresh_via_msg_tx() {
        let db = test_db().await;
        // Create a task so the refresh has something to send.
        db.create_task(crate::db::CreateTaskRequest {
            title: "test task",
            description: "desc",
            repo_path: "/repo",
            plan: None,
            status: crate::models::TaskStatus::Backlog,
            epic_id: None,
            sort_order: None,
            tag: None,
            base_branch: "main",
            wrap_up_mode: None,
            auto_run_plan: false,
            phoenix: false,
        })
        .await
        .unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
        let rt = make_runtime(db.clone(), tx, runner).await;

        rt.spawn_refresh_from_db().await.unwrap();

        // First message should be a task Refresh.
        let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(
                &msg,
                Message::Task(crate::tui::messages::TaskMessage::Refresh(tasks)) if !tasks.is_empty()
            ),
            "Expected Task::Refresh with tasks, got: {msg:?}"
        );
    }
}

mod spawn_refresh_task {
    use super::*;

    #[tokio::test]
    async fn spawn_refresh_task_sends_updated_task_via_msg_tx() {
        let db = test_db().await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
        let rt = make_runtime(db.clone(), tx, runner).await;
        let tasks = db.list_all().await.unwrap();
        let mut app = App::new(tasks);
        rt.exec_insert_task(
            &mut app,
            tui::TaskDraft {
                title: "Refresh Me".into(),
                description: "Desc".into(),
                repo_path: "/repo".into(),
                ..Default::default()
            },
            None,
        )
        .await;
        let id = app.tasks()[0].id;
        rt.db_write()
            .patch_task(
                id,
                &db::TaskPatch::new()
                    .status(models::TaskStatus::Running)
                    .sub_status(models::SubStatus::Active),
            )
            .await
            .unwrap();

        rt.spawn_refresh_task(id).await.unwrap();

        // Drain messages to find the Updated one.
        let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(
                &msg,
                Message::Task(crate::tui::messages::TaskMessage::Updated(t)) if t.status == models::TaskStatus::Running
            ),
            "Expected Updated with Running status, got: {msg:?}"
        );
    }

    #[tokio::test]
    async fn spawn_refresh_task_falls_back_when_task_gone() {
        let db = test_db().await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
        let rt = make_runtime(db.clone(), tx, runner).await;
        let tasks = db.list_all().await.unwrap();
        let mut app = App::new(tasks);
        rt.exec_insert_task(
            &mut app,
            tui::TaskDraft {
                title: "Gone Task".into(),
                description: "Desc".into(),
                repo_path: "/repo".into(),
                ..Default::default()
            },
            None,
        )
        .await;
        let id = app.tasks()[0].id;
        rt.db_write().delete_task(id).await.unwrap();

        rt.spawn_refresh_task(id).await.unwrap();

        // The fallback sends a Refresh message with an empty list.
        let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(
                &msg,
                Message::Task(crate::tui::messages::TaskMessage::Refresh(tasks)) if tasks.is_empty()
            ),
            "Expected empty Refresh fallback, got: {msg:?}"
        );
    }
}

mod spawn_refresh_epic {
    use super::*;

    #[tokio::test]
    async fn spawn_refresh_epic_sends_updated_epic_via_msg_tx() {
        let db = test_db().await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
        let rt = make_runtime(db.clone(), tx, runner).await;
        let epic = db.create_epic("Epic", "desc", None).await.unwrap();
        db.patch_epic(
            epic.id,
            &db::EpicPatch::new().status(models::TaskStatus::Running),
        )
        .await
        .unwrap();

        rt.spawn_refresh_epic(epic.id).await.unwrap();

        let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(
                &msg,
                Message::Epic(crate::tui::messages::EpicMessage::Updated(e)) if e.status == models::TaskStatus::Running
            ),
            "Expected Updated epic with Running status, got: {msg:?}"
        );
    }

    #[tokio::test]
    async fn spawn_refresh_epic_falls_back_when_epic_gone() {
        let db = test_db().await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
        let rt = make_runtime(db.clone(), tx, runner).await;
        let epic = db.create_epic("Gone Epic", "desc", None).await.unwrap();
        db.delete_epic(epic.id).await.unwrap();

        rt.spawn_refresh_epic(epic.id).await.unwrap();

        let msg = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
            .await
            .unwrap()
            .unwrap();
        // Fallback sends a full Refresh (tasks list, may be empty).
        assert!(
            matches!(
                msg,
                Message::Task(crate::tui::messages::TaskMessage::Refresh(_))
            ),
            "Expected Task::Refresh fallback, got: {msg:?}"
        );
    }

    #[tokio::test]
    async fn spawn_refresh_epic_also_sends_epic_tasks_via_msg_tx() {
        let db = test_db().await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let runner: Arc<dyn ProcessRunner> = Arc::new(MockProcessRunner::new(vec![]));
        let rt = make_runtime(db.clone(), tx, runner).await;
        let epic = db.create_epic("Feed Epic", "desc", None).await.unwrap();
        db.create_task(crate::db::CreateTaskRequest {
            title: "Feed Task",
            description: "from feed",
            repo_path: "/repo",
            plan: None,
            status: models::TaskStatus::Backlog,
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

        rt.spawn_refresh_epic(epic.id).await.unwrap();

        // First message: EpicMessage::Updated
        let msg1 = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(
                msg1,
                Message::Epic(crate::tui::messages::EpicMessage::Updated(_))
            ),
            "Expected Epic::Updated first, got: {msg1:?}"
        );
        // Second message: TaskMessage::Updated for the linked task
        let msg2 = tokio::time::timeout(TEST_TIMEOUT, rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(
                &msg2,
                Message::Task(crate::tui::messages::TaskMessage::Updated(t)) if t.title == "Feed Task"
            ),
            "Expected Task::Updated with 'Feed Task', got: {msg2:?}"
        );
    }
}
