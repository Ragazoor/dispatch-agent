//! Move-task-to-epic tree picker handlers (the `m` key on a task card).
//!
//! Mirrors the epic reparent flow in [`super::epics`] but targets a task: it
//! sets `task.epic_id` (or clears it for the "— no parent —" sentinel) rather
//! than reparenting an epic.

use crate::models::{EpicId, TaskId};

use super::super::types::*;
use super::super::{truncate_title, App, MoveTaskPickerState, TITLE_DISPLAY_LENGTH};

impl App {
    pub(in crate::tui) fn handle_start_move_to_epic(&mut self, task_id: TaskId) -> Vec<Command> {
        let mut tree_state = tui_tree_widget::TreeState::default();
        tree_state.select_first();
        let eligible = self.move_task_target_epics();
        let items = crate::tui::ui::build_reparent_tree(&eligible);
        self.interaction.move_task_picker = Some(MoveTaskPickerState {
            task_id,
            tree_state: std::cell::RefCell::new(tree_state),
            items,
        });
        self.input.mode = InputMode::MoveTaskToEpic(task_id);
        vec![]
    }

    pub(in crate::tui) fn handle_move_to_epic_navigate(&mut self, nav: TreeNav) -> Vec<Command> {
        if let Some(picker) = &self.interaction.move_task_picker {
            crate::tui::types::apply_tree_nav(&mut picker.tree_state.borrow_mut(), nav);
            self.dirty = true;
        }
        vec![]
    }

    pub(in crate::tui) fn handle_move_to_epic_confirm(&mut self) -> Vec<Command> {
        let task_id = match self.input.mode {
            InputMode::MoveTaskToEpic(id) => id,
            _ => return vec![],
        };

        let selected_id: Option<String> = self
            .interaction
            .move_task_picker
            .as_ref()
            .and_then(|p| p.tree_state.borrow().selected().last().cloned());

        let new_epic: Option<EpicId> = match selected_id.as_deref() {
            Some(s) if s != crate::tui::types::REPARENT_NO_PARENT_SENTINEL => s
                .strip_prefix("epic:")
                .and_then(|n| n.parse::<i64>().ok())
                .map(EpicId),
            _ => None,
        };

        let moving_title = self
            .board
            .tasks
            .iter()
            .find(|t| t.id == task_id)
            .map(|t| truncate_title(&t.title, TITLE_DISPLAY_LENGTH))
            .unwrap_or_default();

        let msg = match new_epic {
            None => format!("Detach {moving_title} from its epic? [y/n]"),
            Some(eid) => {
                let epic_label = self
                    .board
                    .epics
                    .iter()
                    .find(|e| e.id == eid)
                    .map(|e| truncate_title(&e.title, TITLE_DISPLAY_LENGTH))
                    .unwrap_or_else(|| format!("\"epic #{}\"", eid.0));
                format!("Move {moving_title} to {epic_label}? [y/n]")
            }
        };

        self.input.mode = InputMode::ConfirmMoveTaskToEpic { task_id, new_epic };
        self.set_status(msg);
        vec![]
    }

    fn clear_move_task_state(&mut self) {
        self.input.mode = InputMode::Normal;
        self.interaction.move_task_picker = None;
        self.clear_status();
    }

    pub(in crate::tui) fn handle_move_to_epic_execute(&mut self) -> Vec<Command> {
        let (task_id, new_epic) = match self.input.mode {
            InputMode::ConfirmMoveTaskToEpic { task_id, new_epic } => (task_id, new_epic),
            _ => return vec![],
        };
        self.clear_move_task_state();
        vec![Command::Task(
            crate::tui::commands::TaskCommand::MoveToEpic {
                id: task_id,
                new_epic,
            },
        )]
    }

    /// Cancel the move flow entirely (Esc/q from the confirm prompt), returning
    /// to Normal mode and clearing the picker.
    pub(in crate::tui) fn handle_move_to_epic_cancel_all(&mut self) -> Vec<Command> {
        self.clear_move_task_state();
        vec![]
    }

    pub(in crate::tui) fn handle_move_to_epic_cancel(&mut self) -> Vec<Command> {
        match self.input.mode {
            InputMode::ConfirmMoveTaskToEpic { task_id, .. } => {
                self.input.mode = InputMode::MoveTaskToEpic(task_id);
                self.clear_status();
            }
            InputMode::MoveTaskToEpic(_) => {
                self.input.mode = InputMode::Normal;
                self.interaction.move_task_picker = None;
            }
            _ => {}
        }
        vec![]
    }
}
