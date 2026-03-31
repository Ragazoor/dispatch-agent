use std::time::Instant;

use crate::models::{EpicId, Task, TaskId, TaskStatus, TmuxWindow, WorktreePath};
use crate::tui::types::{ColumnItem, Command, InputMode, MoveDirection, TaskEdit};
use crate::tui::truncate_title;
use crate::tui::App;

impl App {
    pub(in crate::tui) fn handle_dispatch_task(&mut self, id: TaskId) -> Vec<Command> {
        if let Some(task) = self.find_task(id) {
            if task.status == TaskStatus::Backlog {
                return vec![Command::Dispatch { task: task.clone() }];
            }
        }
        vec![]
    }

    pub(in crate::tui) fn handle_brainstorm_task(&mut self, id: TaskId) -> Vec<Command> {
        if let Some(task) = self.find_task(id) {
            if task.status == TaskStatus::Backlog {
                return vec![Command::Brainstorm { task: task.clone() }];
            }
        }
        vec![]
    }

    pub(in crate::tui) fn handle_plan_task(&mut self, id: TaskId) -> Vec<Command> {
        if let Some(task) = self.find_task(id) {
            if task.status == TaskStatus::Backlog {
                return vec![Command::Plan { task: task.clone() }];
            }
        }
        vec![]
    }

    pub(in crate::tui) fn handle_dispatched(
        &mut self,
        id: TaskId,
        worktree: WorktreePath,
        tmux_window: TmuxWindow,
        switch_focus: bool,
    ) -> Vec<Command> {
        if let Some(task) = self.find_task_mut(id) {
            task.worktree = Some(worktree);
            task.tmux_window = Some(tmux_window.clone());
            task.status = TaskStatus::Running;
            let task_clone = task.clone();
            self.agents.last_output_change.insert(id, Instant::now());
            self.clamp_selection();
            let mut cmds = vec![Command::PersistTask(task_clone)];
            if switch_focus {
                cmds.push(Command::JumpToTmux { window: tmux_window });
            }
            cmds
        } else {
            vec![]
        }
    }

    pub(in crate::tui) fn handle_task_created(&mut self, task: Task) -> Vec<Command> {
        self.tasks.push(task);
        self.clamp_selection();
        vec![]
    }

    pub(in crate::tui) fn handle_delete_task(&mut self, id: TaskId) -> Vec<Command> {
        let cleanup = self.find_task_mut(id).and_then(Self::take_cleanup);
        self.clear_agent_tracking(id);
        self.tasks.retain(|t| t.id != id);
        self.clamp_selection();
        let archive_count = self.archived_tasks().len();
        if self.archive.selected_row >= archive_count && archive_count > 0 {
            self.archive.selected_row = archive_count - 1;
        }
        *self.archive.list_state.selected_mut() = Some(self.archive.selected_row);
        let mut cmds = Vec::new();
        if let Some(c) = cleanup {
            cmds.push(c);
        }
        cmds.push(Command::DeleteTask(id));
        cmds
    }

    pub(in crate::tui) fn handle_move_task(
        &mut self,
        id: TaskId,
        direction: MoveDirection,
    ) -> Vec<Command> {
        self.rebase_conflict_tasks.remove(&id);
        if let Some(task) = self.find_task_mut(id) {
            let new_status = match direction {
                MoveDirection::Forward => task.status.next(),
                MoveDirection::Backward => task.status.prev(),
            };
            if new_status == task.status {
                return vec![];
            }

            // Confirm before moving to Done
            if new_status == TaskStatus::Done {
                let title = truncate_title(&task.title, 30);
                self.input.mode = InputMode::ConfirmDone(id);
                self.set_status(format!("Move {title} to Done? (y/n)"));
                return vec![];
            }

            // Kill tmux window when moving backward, but keep worktree for resume
            let detach = if matches!(direction, MoveDirection::Backward) {
                Self::take_detach(task)
            } else {
                None
            };

            task.status = new_status;
            let task_clone = task.clone();
            self.clear_agent_tracking(id);
            self.clamp_selection();

            let mut cmds = Vec::new();
            if let Some(c) = detach {
                cmds.push(c);
            }
            cmds.push(Command::PersistTask(task_clone));
            cmds
        } else {
            vec![]
        }
    }

    pub(in crate::tui) fn handle_confirm_done(&mut self) -> Vec<Command> {
        let ids = if !self.pending_done_tasks.is_empty() {
            std::mem::take(&mut self.pending_done_tasks)
        } else {
            match self.input.mode {
                InputMode::ConfirmDone(id) => vec![id],
                _ => return vec![],
            }
        };
        self.input.mode = InputMode::Normal;
        self.clear_status();

        let mut cmds = Vec::new();
        for id in ids {
            if let Some(task) = self.find_task_mut(id) {
                if task.status != TaskStatus::Review {
                    continue;
                }
                let detach = Self::take_detach(task);
                task.status = TaskStatus::Done;
                let task_clone = task.clone();
                self.clear_agent_tracking(id);
                if let Some(c) = detach {
                    cmds.push(c);
                }
                cmds.push(Command::PersistTask(task_clone));
            }
        }
        self.selected_tasks.clear();
        self.clamp_selection();
        cmds
    }

    pub(in crate::tui) fn handle_cancel_done(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.clear_status();
        self.pending_done_tasks.clear();
        vec![]
    }

    pub(in crate::tui) fn handle_toggle_notifications(&mut self) -> Vec<Command> {
        self.notifications_enabled = !self.notifications_enabled;
        let label = if self.notifications_enabled {
            "Notifications enabled"
        } else {
            "Notifications disabled"
        };
        self.set_status(label.to_string());
        vec![Command::PersistSetting {
            key: "notifications_enabled".to_string(),
            value: self.notifications_enabled,
        }]
    }

    pub(in crate::tui) fn handle_resume_task(&mut self, id: TaskId) -> Vec<Command> {
        if let Some(task) = self.find_task(id) {
            if !matches!(task.status, TaskStatus::Running | TaskStatus::Review) {
                return vec![];
            }
            if task.worktree.is_some() && task.tmux_window.is_none() {
                vec![Command::Resume { task: task.clone() }]
            } else {
                vec![]
            }
        } else {
            vec![]
        }
    }

    pub(in crate::tui) fn handle_resumed(
        &mut self,
        id: TaskId,
        tmux_window: TmuxWindow,
    ) -> Vec<Command> {
        self.rebase_conflict_tasks.remove(&id);
        if let Some(task) = self.find_task_mut(id) {
            task.tmux_window = Some(tmux_window);
            task.status = TaskStatus::Running;
            let task_clone = task.clone();
            self.agents.last_output_change.insert(id, Instant::now());
            self.agents.stale_tasks.remove(&id);
            self.agents.crashed_tasks.remove(&id);
            self.clamp_selection();
            vec![Command::PersistTask(task_clone)]
        } else {
            vec![]
        }
    }

    pub(in crate::tui) fn handle_task_edited(&mut self, edit: TaskEdit) -> Vec<Command> {
        if let Some(t) = self.find_task_mut(edit.id) {
            t.title = edit.title;
            t.description = edit.description;
            t.repo_path = edit.repo_path;
            t.status = edit.status;
            t.plan = edit.plan;
            t.tag = edit.tag;
            t.updated_at = chrono::Utc::now();
        }
        self.clamp_selection();
        vec![]
    }

    pub(in crate::tui) fn handle_archive_task(&mut self, id: TaskId) -> Vec<Command> {
        if let Some(task) = self.find_task_mut(id) {
            if task.status == TaskStatus::Archived {
                return vec![];
            }
            let cleanup = Self::take_cleanup(task);
            task.status = TaskStatus::Archived;
            let task_clone = task.clone();
            self.clear_agent_tracking(id);
            self.clamp_selection();

            let mut cmds = Vec::new();
            if let Some(c) = cleanup {
                cmds.push(c);
            }
            cmds.push(Command::PersistTask(task_clone));
            cmds
        } else {
            vec![]
        }
    }

    pub(in crate::tui) fn handle_toggle_archive(&mut self) -> Vec<Command> {
        self.archive.visible = !self.archive.visible;
        if self.archive.visible {
            self.archive.selected_row = 0;
            *self.archive.list_state.selected_mut() = Some(0);
        }
        vec![]
    }

    pub(in crate::tui) fn handle_toggle_select(&mut self, id: TaskId) -> Vec<Command> {
        if self.selected_tasks.contains(&id) {
            self.selected_tasks.remove(&id);
        } else {
            self.selected_tasks.insert(id);
        }
        vec![]
    }

    pub(in crate::tui) fn handle_clear_selection(&mut self) -> Vec<Command> {
        self.selected_tasks.clear();
        self.selected_epics.clear();
        self.selection_mut().on_select_all = false;
        vec![]
    }

    pub(in crate::tui) fn handle_select_all_column(&mut self) -> Vec<Command> {
        let col = self.selection().column();
        let Some(status) = TaskStatus::from_column_index(col) else {
            return vec![];
        };
        let items = self.column_items_for_status(status);
        let mut task_ids = Vec::new();
        let mut epic_ids = Vec::new();
        for item in &items {
            match item {
                ColumnItem::Task(t) => task_ids.push(t.id),
                ColumnItem::Epic(e) => epic_ids.push(e.id),
            }
        }
        if task_ids.is_empty() && epic_ids.is_empty() {
            return vec![];
        }
        let all_tasks_selected = task_ids.iter().all(|id| self.selected_tasks.contains(id));
        let all_epics_selected = epic_ids.iter().all(|id| self.selected_epics.contains(id));
        if all_tasks_selected && all_epics_selected {
            for id in &task_ids {
                self.selected_tasks.remove(id);
            }
            for id in &epic_ids {
                self.selected_epics.remove(id);
            }
        } else {
            for id in task_ids {
                self.selected_tasks.insert(id);
            }
            for id in epic_ids {
                self.selected_epics.insert(id);
            }
        }
        vec![]
    }

    pub(in crate::tui) fn handle_toggle_select_epic(&mut self, id: EpicId) -> Vec<Command> {
        if self.selected_epics.contains(&id) {
            self.selected_epics.remove(&id);
        } else {
            self.selected_epics.insert(id);
        }
        vec![]
    }

    pub(in crate::tui) fn handle_batch_archive_epics(&mut self, ids: Vec<EpicId>) -> Vec<Command> {
        let mut cmds = Vec::new();
        for id in ids {
            cmds.extend(self.handle_archive_epic(id));
        }
        self.selected_epics.clear();
        self.selected_tasks.clear();
        cmds
    }

    pub(in crate::tui) fn handle_batch_move_tasks(
        &mut self,
        ids: Vec<TaskId>,
        direction: MoveDirection,
    ) -> Vec<Command> {
        if matches!(direction, MoveDirection::Forward) {
            let review_ids: Vec<TaskId> = ids
                .iter()
                .copied()
                .filter(|id| {
                    self.find_task(*id)
                        .is_some_and(|t| t.status == TaskStatus::Review)
                })
                .collect();

            if !review_ids.is_empty() {
                // Move non-Review tasks immediately
                let mut cmds = Vec::new();
                for id in &ids {
                    if !review_ids.contains(id) {
                        cmds.extend(self.handle_move_task(*id, direction.clone()));
                    }
                }
                // Enter confirmation for Review→Done tasks
                self.pending_done_tasks = review_ids;
                let count = self.pending_done_tasks.len();
                self.input.mode = InputMode::ConfirmDone(self.pending_done_tasks[0]);
                self.set_status(format!(
                    "Move {} {} to Done? (y/n)",
                    count,
                    if count == 1 { "task" } else { "tasks" }
                ));
                return cmds;
            }
        }

        let mut cmds = Vec::new();
        for id in ids {
            cmds.extend(self.handle_move_task(id, direction.clone()));
        }
        self.selected_tasks.clear();
        cmds
    }

    pub(in crate::tui) fn handle_batch_archive_tasks(
        &mut self,
        ids: Vec<TaskId>,
    ) -> Vec<Command> {
        let mut cmds = Vec::new();
        for id in ids {
            cmds.extend(self.handle_archive_task(id));
        }
        self.selected_tasks.clear();
        cmds
    }

    pub(in crate::tui) fn handle_detach_tmux(&mut self, ids: Vec<TaskId>) -> Vec<Command> {
        let detachable: Vec<TaskId> = ids.iter()
            .filter(|&&id| {
                self.find_task(id)
                    .is_some_and(|t| t.status == TaskStatus::Review && t.tmux_window.is_some())
            })
            .copied()
            .collect();

        if detachable.is_empty() {
            return vec![];
        }

        let count = detachable.len();
        let msg = if count == 1 {
            "Detach tmux panel? (y/n)".to_string()
        } else {
            format!("Detach {count} tmux panels? (y/n)")
        };
        self.input.mode = InputMode::ConfirmDetachTmux(detachable);
        self.set_status(msg);
        vec![]
    }

    pub(in crate::tui) fn handle_confirm_detach_tmux(&mut self) -> Vec<Command> {
        let InputMode::ConfirmDetachTmux(ref ids) = self.input.mode else {
            return vec![];
        };
        let ids = ids.clone();
        self.input.mode = InputMode::Normal;
        self.clear_status();

        let mut cmds = Vec::new();
        for id in ids {
            self.clear_agent_tracking(id);
            if let Some(task) = self.find_task_mut(id) {
                if let Some(window) = task.tmux_window.take() {
                    cmds.push(Command::KillTmuxWindow { window });
                }
                let task_clone = task.clone();
                cmds.push(Command::PersistTask(task_clone));
            }
        }
        cmds
    }
}
