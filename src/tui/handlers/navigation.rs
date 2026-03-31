use crate::models::TaskStatus;
use crate::tui::types::{ColumnItem, Command, InputMode};
use crate::tui::App;

impl App {
    pub(in crate::tui) fn handle_quit(&mut self) -> Vec<Command> {
        self.should_quit = true;
        vec![]
    }

    pub(in crate::tui) fn handle_navigate_column(&mut self, delta: isize) -> Vec<Command> {
        let new_col = (self.selection().column() as isize + delta)
            .clamp(0, (TaskStatus::COLUMN_COUNT - 1) as isize) as usize;
        self.selection_mut().set_column(new_col);
        self.clamp_selection();
        vec![]
    }

    pub(in crate::tui) fn handle_navigate_row(&mut self, delta: isize) -> Vec<Command> {
        let col = self.selection().column();
        let Some(status) = TaskStatus::from_column_index(col) else {
            return vec![];
        };
        let count = self.column_items_for_status(status).len();

        if self.selection().on_select_all {
            // On the toggle row
            if delta > 0 && count > 0 {
                // Move down into task list
                self.selection_mut().on_select_all = false;
                self.selection_mut().set_row(col, 0);
            }
            // delta <= 0 or empty column: stay on toggle (already at top)
        } else if count > 0 {
            let current = self.selection().row(col);
            if current == 0 && delta < 0 {
                // Move up from first task to toggle row
                self.selection_mut().on_select_all = true;
            } else {
                let new_row = (current as isize + delta).clamp(0, count as isize - 1) as usize;
                self.selection_mut().set_row(col, new_row);
            }
        } else {
            // Empty column: move to toggle
            if delta < 0 {
                self.selection_mut().on_select_all = true;
            }
        }
        vec![]
    }

    pub(in crate::tui) fn handle_reorder_item(&mut self, direction: isize) -> Vec<Command> {
        let col = self.selection().column();
        let Some(status) = TaskStatus::from_column_index(col) else {
            return vec![];
        };
        let row = self.selection().row(col);
        let items = self.column_items_for_status(status);
        let target_row = row as isize + direction;
        if target_row < 0 || target_row >= items.len() as isize {
            return vec![];
        }
        let target_row = target_row as usize;

        // Get IDs and effective sort values
        let (a_task_id, a_epic_id, a_eff) = match &items[row] {
            ColumnItem::Task(t) => (Some(t.id), None, t.sort_order.unwrap_or(t.id.0)),
            ColumnItem::Epic(e) => (None, Some(e.id), e.sort_order.unwrap_or(e.id.0)),
        };
        let (b_task_id, b_epic_id, b_eff) = match &items[target_row] {
            ColumnItem::Task(t) => (Some(t.id), None, t.sort_order.unwrap_or(t.id.0)),
            ColumnItem::Epic(e) => (None, Some(e.id), e.sort_order.unwrap_or(e.id.0)),
        };

        // Swap effective values; offset if equal
        let (new_a, new_b) = if a_eff == b_eff {
            if direction > 0 { (a_eff + 1, b_eff) } else { (a_eff - 1, b_eff) }
        } else {
            (b_eff, a_eff)
        };

        // Drop the borrowed items before mutating
        drop(items);

        let mut cmds = vec![];

        if let Some(tid) = a_task_id {
            if let Some(t) = self.find_task_mut(tid) {
                t.sort_order = Some(new_a);
                cmds.push(Command::PersistTask(t.clone()));
            }
        }
        if let Some(eid) = a_epic_id {
            if let Some(e) = self.epics.iter_mut().find(|e2| e2.id == eid) {
                e.sort_order = Some(new_a);
                cmds.push(Command::PersistEpic { id: eid, done: None, sort_order: Some(new_a) });
            }
        }
        if let Some(tid) = b_task_id {
            if let Some(t) = self.find_task_mut(tid) {
                t.sort_order = Some(new_b);
                cmds.push(Command::PersistTask(t.clone()));
            }
        }
        if let Some(eid) = b_epic_id {
            if let Some(e) = self.epics.iter_mut().find(|e2| e2.id == eid) {
                e.sort_order = Some(new_b);
                cmds.push(Command::PersistEpic { id: eid, done: None, sort_order: Some(new_b) });
            }
        }

        // Cursor follows the moved item
        self.selection_mut().set_row(col, target_row);

        cmds
    }

    pub(in crate::tui) fn handle_toggle_detail(&mut self) -> Vec<Command> {
        self.detail_visible = !self.detail_visible;
        vec![]
    }

    pub(in crate::tui) fn handle_toggle_help(&mut self) -> Vec<Command> {
        if self.input.mode == InputMode::Help {
            self.input.mode = InputMode::Normal;
        } else {
            self.input.mode = InputMode::Help;
        }
        vec![]
    }
}
