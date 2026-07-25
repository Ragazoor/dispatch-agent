use anyhow::{Context, Result};
use rusqlite::params;

use crate::models::{EpicId, TaskId, Todo, TodoId, TodoLink};

use super::super::{CreateTodoRow, Database, TodoPatch};
use super::parse_datetime;

const TODO_COLUMNS: &str = "id, title, done, sort_order, created_at, task_id, epic_id, parent_id";

fn row_to_todo(row: &rusqlite::Row<'_>) -> rusqlite::Result<Todo> {
    let created_str: String = row.get(4)?;
    let task_id: Option<i64> = row.get(5)?;
    let epic_id: Option<i64> = row.get(6)?;
    let parent_id: Option<i64> = row.get(7)?;
    Ok(Todo {
        id: TodoId(row.get::<_, i64>(0)?),
        title: row.get(1)?,
        done: row.get(2)?,
        sort_order: row.get(3)?,
        created_at: parse_datetime(&created_str)?,
        linked: match (task_id, epic_id) {
            (Some(id), _) => Some(TodoLink::Task(TaskId(id))),
            (_, Some(id)) => Some(TodoLink::Epic(EpicId(id))),
            _ => None,
        },
        parent_id: parent_id.map(TodoId),
    })
}

#[async_trait::async_trait]
impl super::super::TodoStore for Database {
    async fn list_todos(&self) -> Result<Vec<Todo>> {
        self.db_call_read(move |conn| {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {TODO_COLUMNS} FROM todos ORDER BY sort_order ASC, id ASC"
                ))
                .context("Failed to prepare list_todos")?;
            let rows = stmt
                .query_map([], row_to_todo)
                .context("Failed to query todos")?
                .collect::<rusqlite::Result<Vec<_>>>()
                .context("Failed to collect todos")?;
            Ok(rows)
        })
        .await
    }

    async fn insert_todo(&self, row: CreateTodoRow<'_>) -> Result<TodoId> {
        let title = row.title.to_owned();
        let task_id = row.task_id;
        let epic_id = row.epic_id;
        self.db_call(move |conn| {
            conn.execute(
                "INSERT INTO todos (title, sort_order, task_id, epic_id)
                 VALUES (?1, COALESCE((SELECT MAX(sort_order) FROM todos), -1) + 1, ?2, ?3)",
                params![title, task_id, epic_id],
            )
            .context("Failed to insert todo")?;
            Ok(TodoId(conn.last_insert_rowid()))
        })
        .await
    }

    async fn patch_todo(&self, id: TodoId, patch: &TodoPatch<'_>) -> Result<()> {
        if !patch.has_changes() {
            return Ok(());
        }
        let title = patch.title.map(|s| s.to_owned());
        let done = patch.done;
        let sort_order = patch.sort_order;
        let task_id = patch.task_id;
        let epic_id = patch.epic_id;
        let parent_id = patch.parent_id;

        self.db_call(move |conn| {
            let mut sets: Vec<String> = Vec::new();
            let mut bind: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

            if let Some(t) = title {
                sets.push(format!("title = ?{}", bind.len() + 1));
                bind.push(Box::new(t));
            }
            if let Some(d) = done {
                sets.push(format!("done = ?{}", bind.len() + 1));
                bind.push(Box::new(d));
            }
            if let Some(s) = sort_order {
                sets.push(format!("sort_order = ?{}", bind.len() + 1));
                bind.push(Box::new(s));
            }
            if let Some(tid) = task_id {
                sets.push(format!("task_id = ?{}", bind.len() + 1));
                bind.push(Box::new(tid));
            }
            if let Some(eid) = epic_id {
                sets.push(format!("epic_id = ?{}", bind.len() + 1));
                bind.push(Box::new(eid));
            }
            if let Some(pid) = parent_id {
                sets.push(format!("parent_id = ?{}", bind.len() + 1));
                bind.push(Box::new(pid));
            }

            bind.push(Box::new(id.0));
            let sql = format!(
                "UPDATE todos SET {} WHERE id = ?{}",
                sets.join(", "),
                bind.len()
            );

            let params_refs: Vec<&dyn rusqlite::ToSql> = bind.iter().map(|b| b.as_ref()).collect();
            conn.execute(&sql, params_refs.as_slice())
                .context("Failed to patch todo")?;
            Ok(())
        })
        .await
    }

    async fn delete_todo(&self, id: TodoId) -> Result<()> {
        self.db_call(move |conn| {
            conn.execute("DELETE FROM todos WHERE id = ?1", params![id.0])
                .context("Failed to delete todo")?;
            Ok(())
        })
        .await
    }

    async fn delete_done_todos(&self) -> Result<()> {
        self.db_call(move |conn| {
            conn.execute("DELETE FROM todos WHERE done = 1", [])
                .context("Failed to delete done todos")?;
            Ok(())
        })
        .await
    }
}
