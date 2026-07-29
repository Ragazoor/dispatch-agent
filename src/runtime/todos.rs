use super::*;

impl TuiRuntime {
    pub(super) async fn exec_load_todos(&self, app: &mut App) {
        match self.todo_svc.list_todos().await {
            Ok(todos) => {
                app.update(Message::Todo(crate::tui::messages::TodoMessage::Show(
                    todos,
                )));
            }
            Err(e) => tracing::warn!("failed to load todos: {e}"),
        }
    }

    pub(super) async fn exec_create_todo(
        &self,
        app: &mut App,
        title: String,
        linked: Option<crate::models::TodoLink>,
        reopen: bool,
    ) {
        if let Err(e) = self.todo_svc.create_todo(title, linked).await {
            tracing::warn!("create todo failed: {e}");
            return;
        }
        if reopen {
            self.exec_load_todos(app).await;
        } else {
            self.exec_load_todo_count(app).await;
        }
    }

    pub(super) async fn exec_load_todo_count(&self, app: &mut App) {
        if let Ok(todos) = self.todo_svc.list_todos().await {
            let open = todos.iter().filter(|t| !t.done).count() as i64;
            app.update(Message::Todo(
                crate::tui::messages::TodoMessage::CountUpdated(open),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::db::Database;
    use crate::service::todos::TodoUpdate;
    use crate::tui::ViewMode;

    /// Thin wrapper over `runtime::tests::make_runtime` (the canonical fixture,
    /// which shares one `Database` between every service) with the runner and
    /// message channel these tests never look at. `unused()` panics on the first
    /// shell-out, so an accidental one fails loudly.
    async fn make_runtime(db: std::sync::Arc<Database>) -> TuiRuntime {
        let (tx, _rx) = mpsc::unbounded_channel();
        super::super::tests::make_runtime(db, tx, crate::process::MockProcessRunner::unused()).await
    }

    #[tokio::test]
    async fn todo_open_is_two_phase_load_then_show() {
        let db = super::super::tests::test_db().await;
        // Insert a todo via the service
        let todo_svc = crate::service::TodoService::new(db.clone());
        todo_svc
            .create_todo("Write tests".to_string(), None)
            .await
            .unwrap();

        let rt = make_runtime(db.clone()).await;
        let mut app = App::new(vec![]);

        // Simulate the two-phase open: Open -> Load command -> exec_load_todos -> Show
        // Phase 1: Open emits Load command (tested in tui::tests::todos::open_returns_load_command)
        // Phase 2: exec_load_todos directly applies Show to app
        rt.exec_load_todos(&mut app).await;

        // After exec_load_todos, app should be in Todos view mode with the loaded item
        assert!(
            matches!(app.view_mode(), ViewMode::Todos { todos, .. } if todos.len() == 1),
            "expected Todos view with 1 todo, got {:?}",
            app.view_mode()
        );
    }

    #[tokio::test]
    async fn q_restores_previous_view() {
        // This test verifies the Close message restores the prior view (Board).
        // The input routing (q -> Close) is tested via handle_key_todos at the TUI layer.
        let db = super::super::tests::test_db().await;
        let rt = make_runtime(db.clone()).await;
        let mut app = App::new(vec![]);

        // Open the todos view
        rt.exec_load_todos(&mut app).await;
        assert!(
            matches!(app.view_mode(), ViewMode::Todos { .. }),
            "expected Todos view after load"
        );

        // Close should restore the board view
        app.update(Message::Todo(crate::tui::messages::TodoMessage::Close));
        assert!(
            matches!(app.view_mode(), ViewMode::Board(_)),
            "expected Board view after close, got {:?}",
            app.view_mode()
        );
    }

    #[tokio::test]
    async fn exec_create_todo_persists_to_db() {
        let db = super::super::tests::test_db().await;
        let rt = make_runtime(db.clone()).await;
        let mut app = App::new(vec![]);

        rt.exec_create_todo(&mut app, "buy milk".to_string(), None, false)
            .await;

        let todos = rt.todo_svc.list_todos().await.unwrap();
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0].title, "buy milk");
    }

    #[tokio::test]
    async fn exec_create_todo_reopen_false_updates_count() {
        let db = super::super::tests::test_db().await;
        let rt = make_runtime(db.clone()).await;
        let mut app = App::new(vec![]);

        rt.exec_create_todo(&mut app, "task one".to_string(), None, false)
            .await;

        // reopen=false: exec_load_todo_count was called, which emits CountUpdated
        assert_eq!(
            app.todo_open_count(),
            1,
            "open count should be 1 after creating one undone todo"
        );
        // Should still be in Board view (not Todos view) because reopen=false
        assert!(
            matches!(app.view_mode(), ViewMode::Board(_)),
            "expected Board view (reopen=false), got {:?}",
            app.view_mode()
        );
    }

    #[tokio::test]
    async fn exec_create_todo_reopen_true_opens_todos_view() {
        let db = super::super::tests::test_db().await;
        let rt = make_runtime(db.clone()).await;
        let mut app = App::new(vec![]);

        rt.exec_create_todo(&mut app, "task two".to_string(), None, true)
            .await;

        // reopen=true: exec_load_todos was called, which emits Show -> Todos view
        assert!(
            matches!(app.view_mode(), ViewMode::Todos { todos, .. } if todos.len() == 1),
            "expected Todos view with 1 item (reopen=true), got {:?}",
            app.view_mode()
        );
    }

    #[tokio::test]
    async fn exec_update_todo_persists_to_db() {
        let db = super::super::tests::test_db().await;
        let rt = make_runtime(db.clone()).await;
        let mut app = App::new(vec![]);

        rt.exec_create_todo(&mut app, "original title".to_string(), None, false)
            .await;
        let todos = rt.todo_svc.list_todos().await.unwrap();
        let id = todos[0].id;

        // Call the dispatch path directly via the update_todo service
        rt.todo_svc
            .update_todo(
                id,
                TodoUpdate {
                    done: Some(true),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let updated = rt.todo_svc.list_todos().await.unwrap();
        assert!(updated[0].done, "todo should be marked done after update");
    }

    #[tokio::test]
    async fn exec_delete_todo_removes_from_db() {
        let db = super::super::tests::test_db().await;
        let rt = make_runtime(db.clone()).await;
        let mut app = App::new(vec![]);

        rt.exec_create_todo(&mut app, "to delete".to_string(), None, false)
            .await;
        let todos = rt.todo_svc.list_todos().await.unwrap();
        let id = todos[0].id;

        rt.todo_svc.delete_todo(id).await.unwrap();

        let remaining = rt.todo_svc.list_todos().await.unwrap();
        assert!(remaining.is_empty(), "todo should be gone after delete");
    }

    #[tokio::test]
    async fn exec_clear_done_removes_done_from_db() {
        let db = super::super::tests::test_db().await;
        let rt = make_runtime(db.clone()).await;
        let mut app = App::new(vec![]);

        rt.exec_create_todo(&mut app, "keep me".to_string(), None, false)
            .await;
        rt.exec_create_todo(&mut app, "done item".to_string(), None, false)
            .await;
        let todos = rt.todo_svc.list_todos().await.unwrap();
        let done_id = todos[1].id;

        rt.todo_svc
            .update_todo(
                done_id,
                TodoUpdate {
                    done: Some(true),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        rt.todo_svc.clear_done().await.unwrap();

        let remaining = rt.todo_svc.list_todos().await.unwrap();
        assert_eq!(remaining.len(), 1, "only the open todo should remain");
        assert_eq!(remaining[0].title, "keep me");
    }

    #[tokio::test]
    async fn exec_load_todo_count_emits_count_updated() {
        let db = super::super::tests::test_db().await;
        let rt = make_runtime(db.clone()).await;
        let mut app = App::new(vec![]);

        rt.exec_create_todo(&mut app, "one".to_string(), None, false)
            .await;
        rt.exec_create_todo(&mut app, "two".to_string(), None, false)
            .await;
        // Mark one as done
        let todos = rt.todo_svc.list_todos().await.unwrap();
        rt.todo_svc
            .update_todo(
                todos[0].id,
                TodoUpdate {
                    done: Some(true),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        rt.exec_load_todo_count(&mut app).await;

        assert_eq!(
            app.todo_open_count(),
            1,
            "only the undone todo should count"
        );
    }
}
