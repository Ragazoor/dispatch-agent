use std::sync::Arc;

use crate::db::{CreateTodoRow, TodoPatch, TodoStore};
use crate::models::{Todo, TodoId, TodoLink};

use super::ServiceError;

// ---------------------------------------------------------------------------
// TodoUpdate — service-layer mutation params
// ---------------------------------------------------------------------------

/// Partial update for a todo item. `None` fields are left unchanged.
/// For `linked`: `Some(None)` clears the link; `Some(Some(l))` sets it.
/// For `parent_id`: `Some(None)` = un-nest (set parent_id to NULL);
/// `Some(Some(id))` = nest under `id` (depth-checked: candidate parent must be a root item).
#[derive(Debug, Clone, Default)]
pub struct TodoUpdate {
    pub title: Option<String>,
    pub done: Option<bool>,
    pub sort_order: Option<i64>,
    pub linked: Option<Option<TodoLink>>,
    pub parent_id: Option<Option<TodoId>>,
}

// ---------------------------------------------------------------------------
// TodoService
// ---------------------------------------------------------------------------

pub struct TodoService {
    db: Arc<dyn TodoStore>,
}

impl TodoService {
    pub fn new(db: Arc<dyn TodoStore>) -> Self {
        Self { db }
    }

    pub async fn list_todos(&self) -> Result<Vec<Todo>, ServiceError> {
        self.db.list_todos().await.map_err(ServiceError::from)
    }

    pub async fn create_todo(
        &self,
        title: String,
        linked: Option<TodoLink>,
    ) -> Result<Todo, ServiceError> {
        if title.trim().is_empty() {
            return Err(ServiceError::Validation("title must not be empty".into()));
        }
        let (task_id, epic_id) = match linked {
            Some(TodoLink::Task(id)) => (Some(id.0), None),
            Some(TodoLink::Epic(id)) => (None, Some(id.0)),
            None => (None, None),
        };
        let id = self
            .db
            .insert_todo(CreateTodoRow {
                title: &title,
                task_id,
                epic_id,
            })
            .await
            .map_err(ServiceError::from)?;
        let todos = self.db.list_todos().await.map_err(ServiceError::from)?;
        todos
            .into_iter()
            .find(|t| t.id == id)
            .ok_or_else(|| ServiceError::NotFound(format!("todo {id:?} not found after insert")))
    }

    pub async fn update_todo(&self, id: TodoId, update: TodoUpdate) -> Result<(), ServiceError> {
        let mut patch = TodoPatch::new();
        if let Some(ref title) = update.title {
            patch = patch.title(title.as_str());
        }
        if let Some(done) = update.done {
            patch = patch.done(done);
        }
        if let Some(sort_order) = update.sort_order {
            patch = patch.sort_order(sort_order);
        }
        if let Some(linked_update) = update.linked {
            // Some(None) = clear both columns; Some(Some(link)) = set one, clear other
            let (task_id, epic_id) = match linked_update {
                None => (Some(None), Some(None)),
                Some(TodoLink::Task(tid)) => (Some(Some(tid.0)), Some(None)),
                Some(TodoLink::Epic(eid)) => (Some(None), Some(Some(eid.0))),
            };
            if let Some(t) = task_id {
                patch = patch.task_id(t);
            }
            if let Some(e) = epic_id {
                patch = patch.epic_id(e);
            }
        }
        if let Some(parent_update) = update.parent_id {
            match parent_update {
                None => {
                    patch = patch.parent_id(None);
                }
                Some(pid) => {
                    // Depth validation: the candidate parent must be a root item.
                    let todos = self.db.list_todos().await.map_err(ServiceError::from)?;
                    match todos.iter().find(|t| t.id == pid) {
                        None => {
                            return Err(ServiceError::NotFound(format!(
                                "parent todo {pid:?} not found"
                            )));
                        }
                        Some(p) if p.parent_id.is_some() => {
                            return Err(ServiceError::Validation(
                                "cannot nest a todo under another nested todo (depth limit is 1)"
                                    .into(),
                            ));
                        }
                        _ => {}
                    }
                    if todos.iter().any(|t| t.parent_id == Some(id)) {
                        return Err(ServiceError::Validation(
                            "cannot nest a todo that already has children (depth limit is 1)"
                                .into(),
                        ));
                    }
                    patch = patch.parent_id(Some(pid.0));
                }
            }
        }
        if patch.has_changes() {
            self.db
                .patch_todo(id, &patch)
                .await
                .map_err(ServiceError::from)?;
        }
        Ok(())
    }

    pub async fn delete_todo(&self, id: TodoId) -> Result<(), ServiceError> {
        self.db.delete_todo(id).await.map_err(ServiceError::from)
    }

    pub async fn clear_done(&self) -> Result<(), ServiceError> {
        self.db
            .delete_done_todos()
            .await
            .map_err(ServiceError::from)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::Arc;

    use crate::db::{Database, TaskCrud};
    use crate::models::TodoId;
    use crate::service::todos::{TodoService, TodoUpdate};

    async fn make_service() -> TodoService {
        let db = Database::open_in_memory().await.unwrap();
        TodoService::new(Arc::new(db))
    }

    #[tokio::test]
    async fn create_todo_returns_todo_with_correct_title() {
        let svc = make_service().await;
        let todo = svc.create_todo("Buy milk".into(), None).await.unwrap();
        assert_eq!(todo.title, "Buy milk");
        assert!(!todo.done);
    }

    #[tokio::test]
    async fn create_todo_empty_title_returns_validation_error() {
        let svc = make_service().await;
        let err = svc.create_todo("   ".into(), None).await.unwrap_err();
        assert!(
            matches!(err, crate::service::ServiceError::Validation(_)),
            "expected Validation error, got {err:?}"
        );
    }

    #[tokio::test]
    async fn list_todos_returns_all_created() {
        let svc = make_service().await;
        svc.create_todo("A".into(), None).await.unwrap();
        svc.create_todo("B".into(), None).await.unwrap();
        let todos = svc.list_todos().await.unwrap();
        assert_eq!(todos.len(), 2);
        assert_eq!(todos[0].title, "A");
        assert_eq!(todos[1].title, "B");
    }

    #[tokio::test]
    async fn update_todo_title() {
        let svc = make_service().await;
        let todo = svc.create_todo("Old title".into(), None).await.unwrap();
        svc.update_todo(
            todo.id,
            TodoUpdate {
                title: Some("New title".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let todos = svc.list_todos().await.unwrap();
        assert_eq!(todos[0].title, "New title");
    }

    #[tokio::test]
    async fn update_todo_done() {
        let svc = make_service().await;
        let todo = svc.create_todo("Check me".into(), None).await.unwrap();
        assert!(!todo.done);
        svc.update_todo(
            todo.id,
            TodoUpdate {
                done: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let todos = svc.list_todos().await.unwrap();
        assert!(todos[0].done);
    }

    #[tokio::test]
    async fn update_todo_noop_when_no_changes() {
        let svc = make_service().await;
        let todo = svc.create_todo("Stable".into(), None).await.unwrap();
        // Should not error even with empty update
        svc.update_todo(todo.id, TodoUpdate::default())
            .await
            .unwrap();
        let todos = svc.list_todos().await.unwrap();
        assert_eq!(todos[0].title, "Stable");
    }

    #[tokio::test]
    async fn delete_todo_removes_it() {
        let svc = make_service().await;
        let todo = svc.create_todo("Temporary".into(), None).await.unwrap();
        svc.delete_todo(todo.id).await.unwrap();
        let todos = svc.list_todos().await.unwrap();
        assert!(todos.is_empty());
    }

    #[tokio::test]
    async fn clear_done_removes_only_done_todos() {
        let svc = make_service().await;
        let a = svc.create_todo("Keep me".into(), None).await.unwrap();
        let b = svc.create_todo("Done item".into(), None).await.unwrap();
        svc.update_todo(
            b.id,
            TodoUpdate {
                done: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        svc.clear_done().await.unwrap();
        let todos = svc.list_todos().await.unwrap();
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0].id, a.id);
    }

    #[tokio::test]
    async fn delete_nonexistent_todo_does_not_error() {
        let svc = make_service().await;
        // SQLite DELETE on a non-existent row is a no-op — no error expected
        svc.delete_todo(TodoId(9999)).await.unwrap();
    }

    #[tokio::test]
    async fn create_todo_with_task_link() {
        let db = std::sync::Arc::new(Database::open_in_memory().await.unwrap());
        // Insert a task first
        let task_id = db
            .create_task(crate::db::CreateTaskRequest {
                title: "linked task",
                description: "",
                repo_path: "/repo",
                plan: None,
                status: crate::models::TaskStatus::Backlog,
                tag: None,
                base_branch: "main",
                epic_id: None,
                sort_order: None,
                wrap_up_mode: None,
                auto_run_plan: false,
            })
            .await
            .unwrap();
        let svc = TodoService::new(db);
        let todo = svc
            .create_todo(
                "Implement auth".into(),
                Some(crate::models::TodoLink::Task(task_id)),
            )
            .await
            .unwrap();
        assert_eq!(todo.linked, Some(crate::models::TodoLink::Task(task_id)));
    }

    #[tokio::test]
    async fn nest_todo_sets_parent_id() {
        let svc = make_service().await;
        let parent = svc.create_todo("Parent".into(), None).await.unwrap();
        let child = svc.create_todo("Child".into(), None).await.unwrap();

        svc.update_todo(
            child.id,
            TodoUpdate {
                parent_id: Some(Some(parent.id)),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let todos = svc.list_todos().await.unwrap();
        let updated = todos.iter().find(|t| t.id == child.id).unwrap();
        assert_eq!(updated.parent_id, Some(parent.id));
    }

    #[tokio::test]
    async fn unnest_todo_clears_parent_id() {
        let svc = make_service().await;
        let parent = svc.create_todo("Parent".into(), None).await.unwrap();
        let child = svc.create_todo("Child".into(), None).await.unwrap();

        // Nest first
        svc.update_todo(
            child.id,
            TodoUpdate {
                parent_id: Some(Some(parent.id)),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        // Then unnest
        svc.update_todo(
            child.id,
            TodoUpdate {
                parent_id: Some(None),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let todos = svc.list_todos().await.unwrap();
        let updated = todos.iter().find(|t| t.id == child.id).unwrap();
        assert_eq!(updated.parent_id, None);
    }

    #[tokio::test]
    async fn nesting_child_under_child_returns_validation_error() {
        let svc = make_service().await;
        let grandparent = svc.create_todo("Grandparent".into(), None).await.unwrap();
        let parent = svc.create_todo("Parent".into(), None).await.unwrap();
        let child = svc.create_todo("Child".into(), None).await.unwrap();

        // Make parent a child of grandparent
        svc.update_todo(
            parent.id,
            TodoUpdate {
                parent_id: Some(Some(grandparent.id)),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        // Try to nest child under parent (which is already nested) — must fail
        let result = svc
            .update_todo(
                child.id,
                TodoUpdate {
                    parent_id: Some(Some(parent.id)),
                    ..Default::default()
                },
            )
            .await;

        assert!(
            matches!(result, Err(crate::service::ServiceError::Validation(_))),
            "expected Validation error when nesting under a nested todo, got {result:?}"
        );
    }

    #[tokio::test]
    async fn nesting_a_todo_that_already_has_children_returns_validation_error() {
        // Regression guard: NestingDepthOne requires parent != null implies
        // children.count = 0. Root Z, root A with child A1; nesting A under Z
        // must be rejected — it would leave A.parent != null with A having a
        // child of its own (A1), violating the one-level-deep invariant even
        // though neither existing guard (candidate-parent-is-child,
        // selected-is-already-child) catches it.
        let svc = make_service().await;
        let z = svc.create_todo("Z".into(), None).await.unwrap();
        let a = svc.create_todo("A".into(), None).await.unwrap();
        let a1 = svc.create_todo("A1".into(), None).await.unwrap();
        svc.update_todo(
            a1.id,
            TodoUpdate {
                parent_id: Some(Some(a.id)),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let result = svc
            .update_todo(
                a.id,
                TodoUpdate {
                    parent_id: Some(Some(z.id)),
                    ..Default::default()
                },
            )
            .await;

        assert!(
            matches!(result, Err(crate::service::ServiceError::Validation(_))),
            "expected Validation error when nesting a todo that already has children, got {result:?}"
        );
    }

    #[tokio::test]
    async fn update_todo_link_and_unlink() {
        let db = std::sync::Arc::new(Database::open_in_memory().await.unwrap());
        let task_id = db
            .create_task(crate::db::CreateTaskRequest {
                title: "t",
                description: "",
                repo_path: "/repo",
                plan: None,
                status: crate::models::TaskStatus::Backlog,
                tag: None,
                base_branch: "main",
                epic_id: None,
                sort_order: None,
                wrap_up_mode: None,
                auto_run_plan: false,
            })
            .await
            .unwrap();
        let svc = TodoService::new(db);
        let todo = svc.create_todo("Unlinked".into(), None).await.unwrap();

        // Link
        svc.update_todo(
            todo.id,
            TodoUpdate {
                linked: Some(Some(crate::models::TodoLink::Task(task_id))),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let todos = svc.list_todos().await.unwrap();
        assert_eq!(
            todos[0].linked,
            Some(crate::models::TodoLink::Task(task_id))
        );

        // Unlink
        svc.update_todo(
            todos[0].id,
            TodoUpdate {
                linked: Some(None),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let todos = svc.list_todos().await.unwrap();
        assert_eq!(todos[0].linked, None);
    }
}
