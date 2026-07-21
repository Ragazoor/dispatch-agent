//! Epic-related message handlers: lifecycle, wrap-up, batch ops, creation flow.

use std::collections::HashSet;

use crate::dispatch;
use crate::models::{descendant_epic_ids, DispatchMode, Epic, EpicId, Task, TaskId, TaskStatus};

use super::super::types::*;
use super::super::{truncate_title, App, TITLE_DISPLAY_LENGTH};

impl App {
    pub(in crate::tui) fn handle_start_epic_wrap_up(&mut self, epic_id: EpicId) -> Vec<Command> {
        let review_count = self
            .board
            .tasks
            .iter()
            .filter(|t| {
                t.epic_id == Some(epic_id) && t.status == TaskStatus::Review && t.worktree.is_some()
            })
            .count();

        if review_count == 0 {
            return self.update(Message::System(
                crate::tui::messages::SystemMessage::StatusInfo(
                    "No review tasks to wrap up".to_string(),
                ),
            ));
        }

        self.input.mode = InputMode::ConfirmEpicWrapUp(epic_id);
        self.set_status(format!(
            "Wrap up {} review task{}: [r] rebase all  [Esc] cancel  (PR: agent /wrap-up skill per task)",
            review_count,
            if review_count == 1 { "" } else { "s" },
        ));
        vec![]
    }

    pub(in crate::tui) fn handle_epic_wrap_up(&mut self) -> Vec<Command> {
        let epic_id = match self.input.mode {
            InputMode::ConfirmEpicWrapUp(id) => id,
            _ => return vec![],
        };
        self.input.mode = InputMode::Normal;

        let mut review_tasks: Vec<&Task> = self
            .board
            .tasks
            .iter()
            .filter(|t| {
                t.epic_id == Some(epic_id) && t.status == TaskStatus::Review && t.worktree.is_some()
            })
            .collect();
        review_tasks.sort_by_key(|t| t.sort_order.unwrap_or(t.id.0));

        let task_ids: Vec<TaskId> = review_tasks.iter().map(|t| t.id).collect();

        if task_ids.is_empty() {
            return vec![];
        }

        self.merge_queue = Some(MergeQueue {
            epic_id,
            task_ids,
            completed: 0,
            current: None,
            failed: None,
        });

        self.advance_merge_queue()
    }

    pub(in crate::tui) fn advance_merge_queue(&mut self) -> Vec<Command> {
        loop {
            let (total, next_idx, next_id) = match &self.merge_queue {
                Some(q) if q.completed < q.task_ids.len() => {
                    (q.task_ids.len(), q.completed, q.task_ids[q.completed])
                }
                Some(q) => {
                    let total = q.task_ids.len();
                    self.merge_queue = None;
                    self.set_status(format!("Epic merge complete: {total}/{total} done"));
                    return vec![];
                }
                None => return vec![],
            };

            // Validate the task is still eligible
            let task_data = match self.find_task(next_id) {
                Some(t) if t.status == TaskStatus::Review => match t.worktree {
                    Some(ref worktree) => {
                        let worktree = worktree.clone();
                        let branch = dispatch::branch_from_worktree(&worktree);
                        let repo_path = t.repo_path.clone();
                        let base_branch = t.base_branch.clone();
                        let tmux_window = t.tmux_window.clone();
                        branch.map(|b| (worktree, b, repo_path, base_branch, tmux_window))
                    }
                    None => None,
                },
                _ => None,
            };

            let Some((worktree, branch, repo_path, base_branch, tmux_window)) = task_data else {
                // Skip this task — no longer eligible
                if let Some(q) = &mut self.merge_queue {
                    q.completed += 1;
                }
                continue;
            };

            if let Some(q) = &mut self.merge_queue {
                q.current = Some(next_id);
            }

            self.set_status(format!(
                "Epic merge: {next_idx}/{total} done \u{2014} processing #{}",
                next_id
            ));

            return vec![Command::Task(crate::tui::commands::TaskCommand::Finish {
                id: next_id,
                repo_path,
                branch,
                base_branch,
                worktree,
                tmux_window,
            })];
        }
    }

    pub(in crate::tui) fn handle_cancel_epic_wrap_up(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.clear_status();
        vec![]
    }

    pub(in crate::tui) fn handle_cancel_merge_queue(&mut self) -> Vec<Command> {
        self.merge_queue = None;
        self.set_status("Merge queue cancelled".to_string());
        vec![]
    }

    // -----------------------------------------------------------------------
    // Epic handlers
    // -----------------------------------------------------------------------

    pub(in crate::tui) fn handle_dispatch_epic(&mut self, id: EpicId) -> Vec<Command> {
        let Some(epic) = self.board.epics.iter().find(|e| e.id == id) else {
            return vec![];
        };
        let status = epic.status;

        if status != TaskStatus::Backlog {
            self.set_status("No backlog tasks in epic".to_string());
            return vec![];
        }

        if epic.plan_path.is_some() {
            // Epic has a plan — dispatch the next backlog subtask sorted by sort_order
            let mut backlog_subtasks: Vec<&Task> = self
                .board
                .tasks
                .iter()
                .filter(|t| {
                    t.epic_id == Some(id)
                        && t.status == TaskStatus::Backlog
                        && !self.dispatching.contains_key(&t.id)
                })
                .collect();
            backlog_subtasks.sort_by_key(|t| (t.sort_order.unwrap_or(t.id.0), t.id.0));

            let next = backlog_subtasks
                .first()
                .map(|t| (t.id, DispatchMode::for_task(t), (*t).clone()));
            drop(backlog_subtasks);

            match next {
                Some((task_id, mode, task)) => {
                    self.mark_dispatching(task_id);
                    vec![Command::Task(
                        crate::tui::commands::TaskCommand::DispatchAgent { task, mode },
                    )]
                }
                None => {
                    self.set_status("No backlog subtasks in epic".to_string());
                    vec![]
                }
            }
        } else {
            self.set_status("Cannot dispatch an epic".to_string());
            vec![]
        }
    }

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
                crate::tui::commands::EditorCommand::PopOut(EditKind::EpicEdit(epic.clone())),
            )]
        } else {
            vec![]
        }
    }

    pub(in crate::tui) fn handle_epic_edited(&mut self, epic: Epic) -> Vec<Command> {
        if let Some(e) = self.board.epics.iter_mut().find(|e| e.id == epic.id) {
            e.title = epic.title;
            e.description = epic.description;
            e.updated_at = chrono::Utc::now();
        }
        vec![]
    }

    pub(in crate::tui) fn handle_delete_epic(&mut self, id: EpicId) -> Vec<Command> {
        let mut cmds = Vec::new();
        // Clean up worktrees/tmux for subtasks before deleting
        let subtask_ids: Vec<TaskId> = self
            .board
            .tasks
            .iter()
            .filter(|t| t.epic_id == Some(id))
            .map(|t| t.id)
            .collect();
        for task_id in subtask_ids {
            if let Some(task) = self.find_task_mut(task_id) {
                let cleanup = Self::take_cleanup(task);
                if let Some(c) = cleanup {
                    cmds.push(c);
                }
                self.clear_agent_tracking(task_id);
            }
        }
        self.board.epics.retain(|e| e.id != id);
        self.board.tasks.retain(|t| t.epic_id != Some(id));
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
                            task.clone(),
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
