#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;

fn todo(title: &str) -> crate::db::CreateTodoRow<'_> {
    crate::db::CreateTodoRow {
        title,
        task_id: None,
        epic_id: None,
    }
}

#[tokio::test]
async fn migration_v67_creates_todos_table() {
    let db = in_memory_db().await;

    // Verify the todos table exists
    let table_exists: bool = db
        .db_call(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='todos'",
                [],
                |row| {
                    let count: i64 = row.get(0)?;
                    Ok(count > 0)
                },
            )
            .map_err(anyhow::Error::from)
        })
        .await
        .unwrap();

    assert!(table_exists, "todos table should exist after migration v67");
}

#[tokio::test]
async fn insert_and_list_todos() {
    let db = in_memory_db().await;

    let id1 = db.insert_todo(todo("Buy milk")).await.unwrap();
    let id2 = db.insert_todo(todo("Write tests")).await.unwrap();

    let todos = db.list_todos().await.unwrap();
    assert_eq!(todos.len(), 2);
    assert_eq!(todos[0].id, id1);
    assert_eq!(todos[0].title, "Buy milk");
    assert!(!todos[0].done);
    assert_eq!(todos[1].id, id2);
    assert_eq!(todos[1].title, "Write tests");
}

#[tokio::test]
async fn sort_order_auto_increments() {
    let db = in_memory_db().await;

    db.insert_todo(todo("First")).await.unwrap();
    db.insert_todo(todo("Second")).await.unwrap();
    db.insert_todo(todo("Third")).await.unwrap();

    let todos = db.list_todos().await.unwrap();
    assert_eq!(todos.len(), 3);
    // Each new item gets sort_order = previous max + 1, starting at 0
    assert_eq!(todos[0].sort_order, 0);
    assert_eq!(todos[1].sort_order, 1);
    assert_eq!(todos[2].sort_order, 2);
}

#[tokio::test]
async fn patch_todo_title() {
    let db = in_memory_db().await;
    let id = db.insert_todo(todo("Old title")).await.unwrap();

    db.patch_todo(id, &TodoPatch::new().title("New title"))
        .await
        .unwrap();

    let todos = db.list_todos().await.unwrap();
    assert_eq!(todos[0].title, "New title");
}

#[tokio::test]
async fn patch_todo_done() {
    let db = in_memory_db().await;
    let id = db.insert_todo(todo("Task")).await.unwrap();

    db.patch_todo(id, &TodoPatch::new().done(true))
        .await
        .unwrap();

    let todos = db.list_todos().await.unwrap();
    assert!(todos[0].done);
}

#[tokio::test]
async fn patch_todo_sort_order() {
    let db = in_memory_db().await;
    let id1 = db.insert_todo(todo("First")).await.unwrap();
    let id2 = db.insert_todo(todo("Second")).await.unwrap();

    // Swap sort_orders
    db.patch_todo(id1, &TodoPatch::new().sort_order(10))
        .await
        .unwrap();
    db.patch_todo(id2, &TodoPatch::new().sort_order(0))
        .await
        .unwrap();

    let todos = db.list_todos().await.unwrap();
    // After swap, id2 has lower sort_order so comes first
    assert_eq!(todos[0].id, id2);
    assert_eq!(todos[1].id, id1);
}

#[tokio::test]
async fn patch_todo_no_changes_is_noop() {
    let db = in_memory_db().await;
    let id = db.insert_todo(todo("Unchanged")).await.unwrap();

    // patch with no fields set — should not error
    db.patch_todo(id, &TodoPatch::new()).await.unwrap();

    let todos = db.list_todos().await.unwrap();
    assert_eq!(todos[0].title, "Unchanged");
}

#[tokio::test]
async fn delete_todo() {
    let db = in_memory_db().await;
    let id = db.insert_todo(todo("To delete")).await.unwrap();
    db.insert_todo(todo("To keep")).await.unwrap();

    db.delete_todo(id).await.unwrap();

    let todos = db.list_todos().await.unwrap();
    assert_eq!(todos.len(), 1);
    assert_eq!(todos[0].title, "To keep");
}

#[tokio::test]
async fn delete_done_todos() {
    let db = in_memory_db().await;
    let id1 = db.insert_todo(todo("Done item")).await.unwrap();
    db.insert_todo(todo("Not done")).await.unwrap();
    db.patch_todo(id1, &TodoPatch::new().done(true))
        .await
        .unwrap();

    db.delete_done_todos().await.unwrap();

    let todos = db.list_todos().await.unwrap();
    assert_eq!(todos.len(), 1);
    assert_eq!(todos[0].title, "Not done");
}

#[tokio::test]
async fn list_todos_ordered_by_sort_order() {
    let db = in_memory_db().await;
    let id1 = db.insert_todo(todo("A")).await.unwrap();
    let id2 = db.insert_todo(todo("B")).await.unwrap();
    let id3 = db.insert_todo(todo("C")).await.unwrap();

    // Reorder: C first, A second, B third
    db.patch_todo(id3, &TodoPatch::new().sort_order(0))
        .await
        .unwrap();
    db.patch_todo(id1, &TodoPatch::new().sort_order(1))
        .await
        .unwrap();
    db.patch_todo(id2, &TodoPatch::new().sort_order(2))
        .await
        .unwrap();

    let todos = db.list_todos().await.unwrap();
    assert_eq!(todos[0].id, id3);
    assert_eq!(todos[1].id, id1);
    assert_eq!(todos[2].id, id2);
}

#[tokio::test]
async fn insert_todo_with_task_link_round_trips() {
    let db = in_memory_db().await;
    let task_id = db
        .create_task(crate::db::CreateTaskRequest {
            title: "linked task",
            description: "",
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

    let todo_id = db
        .insert_todo(crate::db::CreateTodoRow {
            title: "todo with task link",
            task_id: Some(task_id.0),
            epic_id: None,
        })
        .await
        .unwrap();

    let todos = db.list_todos().await.unwrap();
    let todo = todos.iter().find(|t| t.id == todo_id).unwrap();
    assert_eq!(
        todo.linked,
        Some(crate::models::TodoLink::Task(task_id)),
        "expected Task link, got {:?}",
        todo.linked
    );
}

#[tokio::test]
async fn insert_todo_with_epic_link_round_trips() {
    let db = in_memory_db().await;
    let epic = db.create_epic("linked epic", "", None).await.unwrap();

    let todo_id = db
        .insert_todo(crate::db::CreateTodoRow {
            title: "todo with epic link",
            task_id: None,
            epic_id: Some(epic.id.0),
        })
        .await
        .unwrap();

    let todos = db.list_todos().await.unwrap();
    let todo = todos.iter().find(|t| t.id == todo_id).unwrap();
    assert_eq!(
        todo.linked,
        Some(crate::models::TodoLink::Epic(epic.id)),
        "expected Epic link, got {:?}",
        todo.linked
    );
}

#[tokio::test]
async fn patch_todo_link_sets_and_clears() {
    let db = in_memory_db().await;
    let task_id = db
        .create_task(crate::db::CreateTaskRequest {
            title: "t",
            description: "",
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

    let todo_id = db
        .insert_todo(crate::db::CreateTodoRow {
            title: "unlinked",
            task_id: None,
            epic_id: None,
        })
        .await
        .unwrap();

    // Set link
    db.patch_todo(
        todo_id,
        &crate::db::TodoPatch::new().task_id(Some(task_id.0)),
    )
    .await
    .unwrap();
    let todos = db.list_todos().await.unwrap();
    assert_eq!(
        todos[0].linked,
        Some(crate::models::TodoLink::Task(task_id))
    );

    // Clear link
    db.patch_todo(todo_id, &crate::db::TodoPatch::new().task_id(None))
        .await
        .unwrap();
    let todos = db.list_todos().await.unwrap();
    assert_eq!(todos[0].linked, None, "expected no link after clear");
}

#[tokio::test]
async fn migration_v69_adds_parent_id_column() {
    let db = in_memory_db().await;
    let col_exists: bool = db
        .db_call(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('todos') WHERE name = 'parent_id'",
                [],
                |r| Ok(r.get::<_, i64>(0)? > 0),
            )
            .map_err(anyhow::Error::from)
        })
        .await
        .unwrap();
    assert!(
        col_exists,
        "parent_id column should exist after migration v69"
    );
}

#[tokio::test]
async fn patch_todo_parent_id_sets_and_clears() {
    let db = in_memory_db().await;
    let parent_id = db.insert_todo(todo("Parent")).await.unwrap();
    let child_id = db.insert_todo(todo("Child")).await.unwrap();

    // Set parent_id
    db.patch_todo(child_id, &TodoPatch::new().parent_id(Some(parent_id.0)))
        .await
        .unwrap();
    let todos = db.list_todos().await.unwrap();
    let child = todos.iter().find(|t| t.id == child_id).unwrap();
    assert_eq!(child.parent_id, Some(parent_id), "parent_id should be set");

    // Clear parent_id
    db.patch_todo(child_id, &TodoPatch::new().parent_id(None))
        .await
        .unwrap();
    let todos = db.list_todos().await.unwrap();
    let child = todos.iter().find(|t| t.id == child_id).unwrap();
    assert_eq!(child.parent_id, None, "parent_id should be cleared");
}

#[tokio::test]
async fn delete_parent_sets_child_parent_id_to_null() {
    let db = in_memory_db().await;
    let parent_id = db.insert_todo(todo("Parent")).await.unwrap();
    let child_id = db.insert_todo(todo("Child")).await.unwrap();

    // Manually set parent_id via patch (the real flow)
    db.patch_todo(child_id, &TodoPatch::new().parent_id(Some(parent_id.0)))
        .await
        .unwrap();

    // Delete the parent
    db.delete_todo(parent_id).await.unwrap();

    let todos = db.list_todos().await.unwrap();
    assert_eq!(todos.len(), 1);
    let child = &todos[0];
    assert_eq!(child.id, child_id);
    assert_eq!(
        child.parent_id, None,
        "ON DELETE SET NULL should clear parent_id"
    );
}
