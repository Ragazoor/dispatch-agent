//! Epic-related message handlers: lifecycle, batch ops, creation flow.

use std::collections::HashSet;

use crate::models::{descendant_epic_ids, Epic, EpicId, TaskId, TaskStatus};

use super::super::types::*;
use super::super::{truncate_title, App, TITLE_DISPLAY_LENGTH};

impl App {
    // -----------------------------------------------------------------------
    // Epic handlers
    // -----------------------------------------------------------------------

    pub(in crate::tui) fn handle_enter_epic(&mut self, epic_id: EpicId) -> Vec<Command> {
        let parent = Box::new(self.board.view_mode.clone());
        self.board.view_mode = ViewMode::Epic {
            epic_id,
            selection: BoardSelection::new_for_epic(),
            parent,
        };
        self.invalidate_layout_cache();
        self.dirty = true;
        vec![]
    }

    pub(in crate::tui) fn handle_exit_epic(&mut self) -> Vec<Command> {
        if let ViewMode::Epic { parent, .. } = std::mem::take(&mut self.board.view_mode) {
            self.board.view_mode = *parent;
            self.invalidate_layout_cache();
            self.dirty = true;
        }
        vec![]
    }

    pub(in crate::tui) fn handle_refresh_epics(&mut self, epics: Vec<Epic>) -> Vec<Command> {
        self.board.epics = epics;
        let valid_ids: HashSet<EpicId> = self.board.epics.iter().map(|e| e.id).collect();
        self.select.epics.retain(|id| valid_ids.contains(id));
        self.invalidate_layout_cache();
        vec![]
    }

    /// Splice a single fresh epic into the in-memory list, replacing the row
    /// with a matching id or appending if it's newly-created.
    pub(in crate::tui) fn handle_epic_updated(&mut self, epic: Epic) -> Vec<Command> {
        if let Some(slot) = self.board.epics.iter_mut().find(|e| e.id == epic.id) {
            *slot = epic;
        } else {
            self.board.epics.push(epic);
        }
        self.invalidate_layout_cache();
        vec![]
    }

    pub(in crate::tui) fn handle_epic_created(&mut self, epic: Epic) -> Vec<Command> {
        self.board.epics.push(epic);
        self.invalidate_layout_cache();
        vec![]
    }

    pub(in crate::tui) fn handle_edit_epic(&mut self, id: EpicId) -> Vec<Command> {
        if let Some(epic) = self.board.epics.iter().find(|e| e.id == id) {
            vec![Command::Editor(
                crate::tui::commands::EditorCommand::PopOut(EditKind::EpicEdit(Box::new(
                    epic.clone(),
                ))),
            )]
        } else {
            vec![]
        }
    }

    pub(in crate::tui) fn handle_epic_edited(&mut self, epic: Epic) -> Vec<Command> {
        if let Some(slot) = self.board.epics.iter_mut().find(|e| e.id == epic.id) {
            *slot = epic;
        }
        vec![]
    }

    pub(in crate::tui) fn handle_delete_epic(&mut self, id: EpicId) -> Vec<Command> {
        let mut cmds = Vec::new();
        // The DB delete drops the whole subtree (`delete_epic_recursive` walks
        // parent_epic_id depth-first), so cleanup must cover the same subtree —
        // covering only direct children would delete a nested subtask's row
        // while leaving its worktree and tmux window with nothing referencing
        // them. Mirrors `handle_archive_epic`. See DeleteEpic in
        // docs/specs/epics.allium.
        let subtree = descendant_epic_ids(id, &self.board.epics);
        let in_subtree =
            |t: &crate::models::Task| t.epic_id.is_some_and(|eid| subtree.contains(&eid));
        let subtask_ids: Vec<TaskId> = self
            .board
            .tasks
            .iter()
            .filter(|t| in_subtree(t))
            .map(|t| t.id)
            .collect();
        for task_id in subtask_ids {
            if let Some(task) = self.find_task_mut(task_id) {
                // DeleteEpic is exempt from the pointer gate: the epic delete
                // drops every subtask row in one operation, so there is nothing
                // left that could hold a retryable pointer — and nothing to
                // write back on success either. The failure is still reported
                // and logged. See WorktreeReleaseIsGated in
                // docs/specs/tasks.allium.
                let cleanup =
                    Self::take_cleanup(task, crate::tui::commands::CleanupFollowUp::Nothing);
                if let Some(c) = cleanup {
                    cmds.push(c);
                }
                self.clear_agent_tracking(task_id);
            }
        }
        self.board.epics.retain(|e| !subtree.contains(&e.id));
        self.board.tasks.retain(|t| !in_subtree(t));
        // If we were viewing this epic, exit
        if matches!(&self.board.view_mode, ViewMode::Epic { epic_id, .. } if *epic_id == id) {
            self.handle_exit_epic();
        }
        self.sync_board_selection();
        cmds.push(Command::Epic(crate::tui::commands::EpicCommand::Delete(id)));
        cmds
    }

    pub(in crate::tui) fn handle_confirm_delete_epic(&mut self) -> Vec<Command> {
        if let Some(ColumnItem::Epic(epic)) = self.selected_column_item() {
            let title = truncate_title(&epic.title, TITLE_DISPLAY_LENGTH);
            self.input.mode = InputMode::ConfirmDeleteEpic;
            self.set_status(format!("Delete epic {title} and subtasks? [y/n]"));
        }
        vec![]
    }

    pub(in crate::tui) fn handle_move_epic_status(
        &mut self,
        id: EpicId,
        direction: MoveDirection,
    ) -> Vec<Command> {
        let Some(epic) = self.board.epics.iter_mut().find(|e| e.id == id) else {
            return vec![];
        };
        let new_status = match direction {
            MoveDirection::Forward => epic.status.next(),
            MoveDirection::Backward => epic.status.prev(),
        };
        if new_status == epic.status {
            return vec![];
        }
        epic.status = new_status;
        let mut cmds = vec![Command::Epic(crate::tui::commands::EpicCommand::Persist {
            id,
            status: Some(new_status),
            sort_order: None,
        })];

        // Moving to Done cleans up all subtask tmux windows
        if new_status == TaskStatus::Done {
            let subtask_ids: Vec<TaskId> = self
                .board
                .tasks
                .iter()
                .filter(|t| t.epic_id == Some(id) && t.tmux_window.is_some())
                .map(|t| t.id)
                .collect();
            for task_id in subtask_ids {
                if let Some(task) = self.find_task_mut(task_id) {
                    if let Some(window) = task.tmux_window.take() {
                        cmds.push(Command::Task(
                            crate::tui::commands::TaskCommand::KillTmuxWindow { window },
                        ));
                        cmds.push(Command::Task(crate::tui::commands::TaskCommand::Persist(
                            crate::tui::commands::PersistFields::from_task(task),
                        )));
                    }
                }
            }
        }
        self.sync_board_selection();
        cmds
    }

    pub(in crate::tui) fn handle_archive_epic(&mut self, id: EpicId) -> Vec<Command> {
        // Soft-archive: recursively transition the epic + all sub-epics + their
        // active subtasks to status = Archived. Nothing is deleted, so the
        // archive path doesn't exercise FK references from learnings.source_task_id
        // (which would block a hard delete).
        let mut cmds = Vec::new();

        let subtree = descendant_epic_ids(id, &self.board.epics);

        for epic_id in &subtree {
            let subtask_ids: Vec<TaskId> = self
                .board
                .tasks
                .iter()
                .filter(|t| t.epic_id == Some(*epic_id) && t.status != TaskStatus::Archived)
                .map(|t| t.id)
                .collect();
            for task_id in subtask_ids {
                cmds.extend(self.handle_archive_task(task_id));
            }

            if let Some(epic) = self.board.epics.iter_mut().find(|e| e.id == *epic_id) {
                if epic.status != TaskStatus::Archived {
                    epic.status = TaskStatus::Archived;
                    cmds.push(Command::Epic(crate::tui::commands::EpicCommand::Persist {
                        id: *epic_id,
                        status: Some(TaskStatus::Archived),
                        sort_order: None,
                    }));
                }
            }
        }

        if matches!(&self.board.view_mode, ViewMode::Epic { epic_id, .. } if *epic_id == id) {
            self.handle_exit_epic();
        }
        self.sync_board_selection();
        cmds
    }

    pub(in crate::tui) fn handle_confirm_archive_epic(&mut self) -> Vec<Command> {
        if let Some(ColumnItem::Epic(epic)) = self.selected_column_item() {
            let id = epic.id;
            let not_done_count = self
                .subtask_statuses(id)
                .iter()
                .filter(|s| **s != TaskStatus::Done)
                .count();
            if not_done_count > 0 {
                let noun = if not_done_count == 1 {
                    "subtask"
                } else {
                    "subtasks"
                };
                self.set_status(format!(
                    "Cannot archive epic: {} {} not done",
                    not_done_count, noun
                ));
                return vec![];
            }
            self.input.mode = InputMode::ConfirmArchiveEpic;
            self.set_status("Archive epic and all subtasks? [y/n]".to_string());
        }
        vec![]
    }

    pub(in crate::tui) fn handle_start_new_epic(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::InputEpicTitle;
        self.input.clear_buffer();
        let parent_epic_id = if let ViewMode::Epic { epic_id, .. } = self.board.view_mode {
            Some(epic_id)
        } else {
            None
        };
        self.input.epic_draft = Some(EpicDraft {
            parent_epic_id,
            ..Default::default()
        });
        self.set_status("Epic title: ".to_string());
        vec![]
    }

    pub(in crate::tui) fn handle_submit_epic_title(&mut self, value: String) -> Vec<Command> {
        self.input.clear_buffer();
        if value.is_empty() {
            self.input.mode = InputMode::Normal;
            self.clear_status();
            vec![]
        } else {
            let parent_epic_id = self
                .input
                .epic_draft
                .as_ref()
                .and_then(|d| d.parent_epic_id);
            self.input.epic_draft = Some(EpicDraft {
                title: value,
                description: String::new(),
                parent_epic_id,
            });
            self.input.mode = InputMode::InputEpicDescription;
            self.set_status("Opening editor for description...".to_string());
            vec![Command::Editor(
                crate::tui::commands::EditorCommand::PopOut(EditKind::Description {
                    is_epic: true,
                }),
            )]
        }
    }

    pub(in crate::tui) fn handle_submit_epic_description(&mut self, value: String) -> Vec<Command> {
        self.input.clear_buffer();
        if let Some(ref mut draft) = self.input.epic_draft {
            draft.description = value;
        }
        self.finish_epic_creation()
    }

    // -----------------------------------------------------------------------
    // Reparent epic handlers
    // -----------------------------------------------------------------------

    pub(in crate::tui) fn handle_start_reparent(&mut self, epic_id: EpicId) -> Vec<Command> {
        let mut tree_state = tui_tree_widget::TreeState::default();
        tree_state.select_first();
        let eligible = self.reparent_target_epics(epic_id);
        let items = crate::tui::ui::build_reparent_tree(&eligible);
        self.interaction.reparent_picker = Some(crate::tui::ReparentPickerState {
            epic_id,
            tree_state: std::cell::RefCell::new(tree_state),
            items,
        });
        self.input.mode = InputMode::ReparentEpic(epic_id);
        vec![]
    }

    pub(in crate::tui) fn handle_reparent_navigate(&mut self, nav: TreeNav) -> Vec<Command> {
        if let Some(picker) = &self.interaction.reparent_picker {
            crate::tui::types::apply_tree_nav(&mut picker.tree_state.borrow_mut(), nav);
            self.dirty = true;
        }
        vec![]
    }

    pub(in crate::tui) fn handle_reparent_confirm(&mut self) -> Vec<Command> {
        let epic_id = match self.input.mode {
            InputMode::ReparentEpic(id) => id,
            _ => return vec![],
        };

        let selected_id: Option<String> = self
            .interaction
            .reparent_picker
            .as_ref()
            .and_then(|p| p.tree_state.borrow().selected().last().cloned());

        let new_parent: Option<EpicId> = match selected_id.as_deref() {
            Some(s) if s != crate::tui::types::REPARENT_NO_PARENT_SENTINEL => s
                .strip_prefix("epic:")
                .and_then(|n| n.parse::<i64>().ok())
                .map(EpicId),
            _ => None,
        };

        let moving_title = self
            .board
            .epics
            .iter()
            .find(|e| e.id == epic_id)
            .map(|e| truncate_title(&e.title, TITLE_DISPLAY_LENGTH))
            .unwrap_or_default();

        let msg = match new_parent {
            None => format!("Make {moving_title} a root epic? [y/n]"),
            Some(pid) => {
                let parent_label = self
                    .board
                    .epics
                    .iter()
                    .find(|e| e.id == pid)
                    .map(|e| truncate_title(&e.title, TITLE_DISPLAY_LENGTH))
                    .unwrap_or_else(|| format!("\"epic #{}\"", pid.0));
                format!("Reparent {moving_title} under {parent_label}? [y/n]")
            }
        };

        self.input.mode = InputMode::ConfirmReparentEpic {
            epic_id,
            new_parent,
        };
        self.set_status(msg);
        vec![]
    }

    fn clear_reparent_state(&mut self) {
        self.input.mode = InputMode::Normal;
        self.interaction.reparent_picker = None;
        self.clear_status();
    }

    pub(in crate::tui) fn handle_reparent_execute(&mut self) -> Vec<Command> {
        let (epic_id, new_parent) = match self.input.mode {
            InputMode::ConfirmReparentEpic {
                epic_id,
                new_parent,
            } => (epic_id, new_parent),
            _ => return vec![],
        };
        self.clear_reparent_state();
        vec![Command::Epic(crate::tui::commands::EpicCommand::Reparent {
            id: epic_id,
            new_parent,
        })]
    }

    /// Cancel the reparent flow entirely (Esc/q from ConfirmReparentEpic),
    /// returning to Normal mode and clearing the picker.
    pub(in crate::tui) fn handle_reparent_cancel_all(&mut self) -> Vec<Command> {
        self.clear_reparent_state();
        vec![]
    }

    pub(in crate::tui) fn handle_reparent_cancel(&mut self) -> Vec<Command> {
        match self.input.mode {
            InputMode::ConfirmReparentEpic { epic_id, .. } => {
                self.input.mode = InputMode::ReparentEpic(epic_id);
                self.clear_status();
            }
            InputMode::ReparentEpic(_) => {
                self.input.mode = InputMode::Normal;
                self.interaction.reparent_picker = None;
            }
            _ => {}
        }
        vec![]
    }
}
