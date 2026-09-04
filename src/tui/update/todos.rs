//! Personal TODO overlay handlers.

use crate::models::{Todo, TodoId};
use crate::tui::commands::TodoCommand;
use crate::tui::types::{ColumnAnchor, Command, InputMode, ViewMode};
use crate::tui::{App, PendingAction};

/// Returns `max(sort_order) + 1` for all todos under `parent` (None = root level).
fn next_sort_order_for(parent: Option<TodoId>, todos: &[Todo]) -> i64 {
    todos
        .iter()
        .filter(|t| t.parent_id == parent)
        .map(|t| t.sort_order)
        .max()
        .map_or(0, |m| m + 1)
}

/// Sort todos for display: hierarchical order — each root item is followed immediately
/// by its children (open children first by sort_order, done children last), then the
/// next root item.  Orphaned children (whose parent has been deleted) are appended at
/// the end so they remain visible rather than silently disappearing.
fn sort_todos(todos: &mut Vec<Todo>) {
    use std::collections::HashMap;
    let all = std::mem::take(todos);
    let mut roots: Vec<Todo> = Vec::new();
    let mut children_map: HashMap<TodoId, Vec<Todo>> = HashMap::new();
    for t in all {
        if let Some(pid) = t.parent_id {
            children_map.entry(pid).or_default().push(t);
        } else {
            roots.push(t);
        }
    }
    roots.sort_by_key(|t| (t.done, t.sort_order, t.id.0));
    for kids in children_map.values_mut() {
        kids.sort_by_key(|t| (t.done, t.sort_order, t.id.0));
    }
    todos.reserve(roots.len() + children_map.values().map(|v| v.len()).sum::<usize>());
    for root in roots {
        let root_id = root.id;
        todos.push(root);
        if let Some(mut kids) = children_map.remove(&root_id) {
            todos.append(&mut kids);
        }
    }
    // Orphaned children (e.g. parent deleted without reload) appended at end
    let mut orphans: Vec<Todo> = children_map.into_values().flatten().collect();
    orphans.sort_by_key(|t| (t.done, t.sort_order, t.id.0));
    todos.append(&mut orphans);
}

impl App {
    /// Kick off loading todos for the overlay.
    pub(in crate::tui) fn handle_open_todos(&mut self) -> Vec<Command> {
        vec![Command::Todo(TodoCommand::Load)]
    }

    /// Update the cached open-todo badge count (pushed from a background reload).
    pub(in crate::tui) fn handle_todo_count_updated(&mut self, count: i64) -> Vec<Command> {
        self.board.todo_open_count = count;
        vec![]
    }

    pub(in crate::tui) fn handle_show_todos(&mut self, mut todos: Vec<Todo>) -> Vec<Command> {
        sort_todos(&mut todos);
        // When the overlay is already open (e.g. after creating a todo with reopen=true),
        // preserve the real pre-Todos `previous` instead of nesting Todos inside Todos.
        // Nesting would cause effective_view_mode() to return Todos, hitting unreachable!().
        let previous = match std::mem::take(&mut self.board.view_mode) {
            ViewMode::Todos { previous, .. } => previous,
            other => Box::new(other),
        };
        self.board.view_mode = ViewMode::Todos {
            todos,
            selected: 0,
            previous,
        };
        self.refresh_todo_count_from_view();
        vec![]
    }

    pub(in crate::tui) fn handle_close_todos(&mut self) -> Vec<Command> {
        // Take the view out (so nothing is borrowed) before reassigning.
        // `&self.board.view_mode` + assign-inside is E0506.
        if let ViewMode::Todos { previous, .. } = std::mem::take(&mut self.board.view_mode) {
            self.board.view_mode = *previous;
        }
        vec![]
    }

    /// Recompute the board footer's open-todo count from the in-memory list.
    /// Called after every in-view mutation so the board count never goes stale
    /// when the user returns to it (no DB round-trip needed).
    pub(in crate::tui) fn refresh_todo_count_from_view(&mut self) {
        if let ViewMode::Todos { todos, .. } = &self.board.view_mode {
            self.board.todo_open_count = todos.iter().filter(|t| !t.done).count() as i64;
        }
    }

    pub(in crate::tui) fn handle_todo_move_selection(&mut self, delta: isize) -> Vec<Command> {
        if let ViewMode::Todos {
            todos, selected, ..
        } = &mut self.board.view_mode
        {
            if todos.is_empty() {
                return vec![];
            }
            let max = todos.len() - 1;
            let next = (*selected as isize + delta).clamp(0, max as isize) as usize;
            *selected = next;
        }
        vec![]
    }

    pub(in crate::tui) fn handle_todo_add(&mut self) -> Vec<Command> {
        self.interaction.pending = PendingAction::None;
        self.input.clear_buffer();
        self.input.mode = InputMode::TodoTitle;
        vec![]
    }

    pub(in crate::tui) fn handle_todo_quick_add(
        &mut self,
        title: String,
        linked: Option<crate::models::TodoLink>,
    ) -> Vec<Command> {
        self.input.set_buffer(title);
        self.interaction.pending = linked.map_or(PendingAction::None, PendingAction::TodoLink);
        self.input.mode = InputMode::TodoQuickAdd;
        vec![]
    }

    pub(in crate::tui) fn handle_todo_edit(&mut self, id: TodoId) -> Vec<Command> {
        if let ViewMode::Todos { todos, .. } = &self.board.view_mode {
            if let Some(t) = todos.iter().find(|t| t.id == id) {
                self.input.set_buffer(t.title.clone());
                self.input.mode = InputMode::TodoTitle;
                self.interaction.pending = PendingAction::TodoEdit(id);
            }
        }
        vec![]
    }

    pub(in crate::tui) fn handle_todo_submit_title(&mut self, title: String) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.input.clear_buffer();
        let title = title.trim().to_string();
        if title.is_empty() {
            self.interaction.pending = PendingAction::None;
            return vec![];
        }
        if let PendingAction::TodoEdit(id) = std::mem::take(&mut self.interaction.pending) {
            // Edit: apply optimistically to the in-memory Vec, then persist.
            if let ViewMode::Todos { todos, .. } = &mut self.board.view_mode {
                if let Some(t) = todos.iter_mut().find(|t| t.id == id) {
                    t.title = title.clone();
                }
            }
            return vec![Command::Todo(TodoCommand::Update {
                id,
                update: crate::service::TodoUpdate {
                    title: Some(title),
                    ..Default::default()
                },
            })];
        }
        // Add: the new id is unknown until the DB insert, so reload the view after create.
        vec![Command::Todo(TodoCommand::Create {
            title,
            linked: None,
            reopen: true,
        })]
    }

    pub(in crate::tui) fn handle_todo_submit_quick_add(&mut self, title: String) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.input.clear_buffer();
        let title = title.trim().to_string();
        if title.is_empty() {
            self.interaction.pending = PendingAction::None;
            return vec![];
        }
        let linked = match std::mem::take(&mut self.interaction.pending) {
            PendingAction::TodoLink(link) => Some(link),
            _ => None,
        };
        // Stays on the board; only refreshes the open-count.
        vec![Command::Todo(TodoCommand::Create {
            title,
            linked,
            reopen: false,
        })]
    }

    pub(in crate::tui) fn handle_todo_toggle(&mut self, id: TodoId) -> Vec<Command> {
        let mut new_done = None;
        if let ViewMode::Todos {
            todos, selected, ..
        } = &mut self.board.view_mode
        {
            if let Some(t) = todos.iter_mut().find(|t| t.id == id) {
                t.done = !t.done;
                new_done = Some(t.done);
            }
            sort_todos(todos);
            *selected = (*selected).min(todos.len().saturating_sub(1));
        }
        self.refresh_todo_count_from_view();
        match new_done {
            Some(done) => vec![Command::Todo(TodoCommand::Update {
                id,
                update: crate::service::TodoUpdate {
                    done: Some(done),
                    ..Default::default()
                },
            })],
            None => vec![],
        }
    }

    pub(in crate::tui) fn handle_todo_reorder(&mut self, delta: isize) -> Vec<Command> {
        let mut cmds = vec![];
        if let ViewMode::Todos {
            todos, selected, ..
        } = &mut self.board.view_mode
        {
            let i = *selected;
            if i >= todos.len() {
                return vec![];
            }
            let moved_id = todos[i].id;
            let current_parent = todos[i].parent_id;

            // Find the nearest sibling (same parent_id) in direction of `delta`.
            // Relies on sort_todos placing each root's children contiguously after
            // it — the parent_id scan over the display list is equivalent to a
            // data-space sibling search only because of that ordering invariant.
            let sibling_idx = if delta > 0 {
                todos[(i + 1)..]
                    .iter()
                    .position(|t| t.parent_id == current_parent)
                    .map(|pos| i + 1 + pos)
            } else {
                todos[..i]
                    .iter()
                    .rposition(|t| t.parent_id == current_parent)
            };
            let Some(j) = sibling_idx else {
                return vec![];
            };

            // Swap sort_orders, then re-sort to maintain hierarchical display order.
            let (so_i, so_j) = (todos[i].sort_order, todos[j].sort_order);
            todos[i].sort_order = so_j;
            todos[j].sort_order = so_i;
            cmds.push(Command::Todo(TodoCommand::Update {
                id: todos[i].id,
                update: crate::service::TodoUpdate {
                    sort_order: Some(todos[i].sort_order),
                    ..Default::default()
                },
            }));
            cmds.push(Command::Todo(TodoCommand::Update {
                id: todos[j].id,
                update: crate::service::TodoUpdate {
                    sort_order: Some(todos[j].sort_order),
                    ..Default::default()
                },
            }));
            sort_todos(todos);
            // Re-anchor selection to the moved item.
            *selected = todos.iter().position(|t| t.id == moved_id).unwrap_or(0);
        }
        cmds
    }

    pub(in crate::tui) fn handle_todo_nest(&mut self, id: TodoId) -> Vec<Command> {
        let ViewMode::Todos {
            todos, selected, ..
        } = &mut self.board.view_mode
        else {
            return vec![];
        };
        let Some(display_idx) = todos.iter().position(|t| t.id == id) else {
            return vec![];
        };
        // No-op if already a child (depth limit = 1).
        if todos[display_idx].parent_id.is_some() {
            return vec![];
        }
        // Walk backwards in display order to find the nearest root item above.
        // Relies on sort_todos placing each root's children immediately after it,
        // so the first parent_id=None item scanning backwards is the correct parent.
        let parent_id = todos[..display_idx]
            .iter()
            .rev()
            .find(|t| t.parent_id.is_none())
            .map(|t| t.id);
        let Some(parent_id) = parent_id else {
            return vec![];
        };
        // Place after the last existing sibling.
        let new_sort_order = next_sort_order_for(Some(parent_id), todos);
        // Update in memory using display_idx directly (already found above).
        todos[display_idx].parent_id = Some(parent_id);
        todos[display_idx].sort_order = new_sort_order;
        sort_todos(todos);
        *selected = todos.iter().position(|t| t.id == id).unwrap_or(0);
        vec![Command::Todo(TodoCommand::Update {
            id,
            update: crate::service::TodoUpdate {
                parent_id: Some(Some(parent_id)),
                sort_order: Some(new_sort_order),
                ..Default::default()
            },
        })]
    }

    pub(in crate::tui) fn handle_todo_unnest(&mut self, id: TodoId) -> Vec<Command> {
        let ViewMode::Todos {
            todos, selected, ..
        } = &mut self.board.view_mode
        else {
            return vec![];
        };
        let Some(display_idx) = todos.iter().position(|t| t.id == id) else {
            return vec![];
        };
        // No-op if already a root.
        if todos[display_idx].parent_id.is_none() {
            return vec![];
        }
        // Append after the last root item.
        let new_sort_order = next_sort_order_for(None, todos);
        // Update in memory using display_idx directly (already found above).
        todos[display_idx].parent_id = None;
        todos[display_idx].sort_order = new_sort_order;
        sort_todos(todos);
        *selected = todos.iter().position(|t| t.id == id).unwrap_or(0);
        vec![Command::Todo(TodoCommand::Update {
            id,
            update: crate::service::TodoUpdate {
                parent_id: Some(None),
                sort_order: Some(new_sort_order),
                ..Default::default()
            },
        })]
    }

    pub(in crate::tui) fn handle_todo_clear_done(&mut self) -> Vec<Command> {
        if let ViewMode::Todos {
            todos, selected, ..
        } = &mut self.board.view_mode
        {
            todos.retain(|t| !t.done);
            *selected = (*selected).min(todos.len().saturating_sub(1));
        }
        self.refresh_todo_count_from_view();
        vec![Command::Todo(TodoCommand::ClearDone)]
    }

    pub(in crate::tui) fn handle_todo_delete(&mut self, id: TodoId) -> Vec<Command> {
        if let ViewMode::Todos {
            todos, selected, ..
        } = &mut self.board.view_mode
        {
            todos.retain(|t| t.id != id);
            *selected = (*selected).min(todos.len().saturating_sub(1));
        }
        self.refresh_todo_count_from_view();
        vec![Command::Todo(TodoCommand::Delete(id))]
    }

    pub(in crate::tui) fn handle_todo_link_to_task(&mut self, todo_id: TodoId) -> Vec<Command> {
        self.handle_close_todos();
        self.input.mode = InputMode::LinkTodoToTask(todo_id);
        self.set_status_sticky(
            "Navigate to a task or epic and press Enter to link — Esc to cancel".to_string(),
        );
        vec![]
    }

    pub(in crate::tui) fn handle_todo_jump_to_linked(
        &mut self,
        link: crate::models::TodoLink,
    ) -> Vec<Command> {
        // A task the board folds inside its epic card can only be reached by
        // entering that epic. Which tasks those are is the board view's own
        // rule, so ask it rather than re-deriving it from the flatten flag.
        let epic_id = match link {
            crate::models::TodoLink::Task(task_id) => self
                .find_task(task_id)
                .filter(|t| !self.shown_on_main_board(t))
                .and_then(|t| t.epic_id),
            _ => None,
        };

        let anchor = match link {
            crate::models::TodoLink::Task(id) => ColumnAnchor::Task(id),
            crate::models::TodoLink::Epic(id) => ColumnAnchor::Epic(id),
        };

        self.handle_close_todos();

        // If the task lives inside an epic, navigate into it so the task is visible.
        if let Some(eid) = epic_id {
            self.handle_enter_epic(eid);
        }

        // Set the anchor on the now-active selection (Board or Epic).
        self.selection_mut().anchor = Some(anchor);
        self.sync_board_selection();
        vec![]
    }
}
