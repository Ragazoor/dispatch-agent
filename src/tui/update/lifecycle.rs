//! Task lifecycle handlers: move, dispatch, create, delete, detail, flatten, done.

use crate::models::{DispatchMode, EpicId, SubStatus, Task, TaskId, TaskStatus};

use super::super::commands::CleanupFollowUp;
use super::super::types::*;
use super::super::{truncate_title, App, TITLE_DISPLAY_LENGTH};

impl App {
    pub(in crate::tui) fn handle_move_task(
        &mut self,
        id: TaskId,
        direction: MoveDirection,
    ) -> Vec<Command> {
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
                self.prompt_move_to_done(vec![id]);
                return vec![];
            }

            // Kill tmux window when moving backward, but keep worktree for resume
            let detach = if matches!(direction, MoveDirection::Backward) {
                Self::take_detach(task)
            } else {
                None
            };

            Self::set_local_status(task, new_status);

            // Seed last_pre_tool_use_at on any transition into Running so the
            // next ClassifyAgentActivity tick does not render "stale · 0m"
            // before the first PreToolUse hook fires. SeedActivity bypasses
            // the generic Persist so a later hook write is not clobbered by
            // an in-memory snapshot.
            let seed_at = (new_status == TaskStatus::Running).then(|| {
                let at = chrono::Utc::now();
                task.last_pre_tool_use_at = Some(at);
                at
            });

            let task_clone = task.clone();
            self.clear_agent_tracking(id);
            self.sync_board_selection();

            let mut cmds = Vec::new();
            if let Some(c) = detach {
                cmds.push(c);
            }
            cmds.push(Command::Task(crate::tui::commands::TaskCommand::Persist(
                task_clone,
            )));
            if let Some(at) = seed_at {
                cmds.push(Command::Task(
                    crate::tui::commands::TaskCommand::SeedActivity { id, at },
                ));
            }
            cmds
        } else {
            vec![]
        }
    }

    /// Open the "move to Done" confirmation for `ids`.
    ///
    /// The pending ids live in `select.pending_done` — the single source of
    /// truth that `handle_confirm_done` drains — so `InputMode::ConfirmDone`
    /// needs no payload. Shared by the single-task and batch paths. A no-op on
    /// an empty list: there is nothing to confirm, and entering the mode would
    /// strand the user in a prompt whose `y` does nothing.
    pub(in crate::tui) fn prompt_move_to_done(&mut self, ids: Vec<TaskId>) {
        if ids.is_empty() {
            return;
        }
        let status = match ids.as_slice() {
            [single] => {
                let title = self
                    .find_task(*single)
                    .map(|t| truncate_title(&t.title, TITLE_DISPLAY_LENGTH))
                    .unwrap_or_default();
                format!("Move {title} to Done? [y/n]")
            }
            _ => format!("Move {} tasks to Done? [y/n]", ids.len()),
        };
        self.select.pending_done = ids;
        self.input.mode = InputMode::ConfirmDone;
        self.set_status(status);
    }

    pub(in crate::tui) fn handle_confirm_done(&mut self) -> Vec<Command> {
        let ids = std::mem::take(&mut self.select.pending_done);
        if ids.is_empty() {
            return vec![];
        }
        self.input.mode = InputMode::Normal;
        self.clear_status();

        let mut cmds = Vec::new();
        for id in ids {
            if let Some(task) = self.find_task_mut(id) {
                // Stale-state guard: callers already filter out terminal
                // tasks, but a background refresh between the prompt and the
                // confirmation can move one under us.
                if matches!(task.status, TaskStatus::Done | TaskStatus::Archived) {
                    continue;
                }
                let detach = Self::take_detach(task);
                Self::set_local_status(task, TaskStatus::Done);
                let task_clone = task.clone();
                self.clear_agent_tracking(id);
                if let Some(c) = detach {
                    cmds.push(c);
                }
                cmds.push(Command::Task(crate::tui::commands::TaskCommand::Persist(
                    task_clone,
                )));
                cmds.extend(self.maybe_respawn_split_pane(id));
            }
        }
        self.select.tasks.clear();
        self.sync_board_selection();
        cmds
    }

    pub(in crate::tui) fn handle_cancel_done(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.clear_status();
        self.select.pending_done.clear();
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

    /// Start a dispatch from the board.
    ///
    /// The Backlog filter here is optimism, not the guard: it reads the board's
    /// snapshot, which a chain or an MCP `dispatch_task` can have invalidated
    /// already. The real guard is the atomic claim
    /// `TuiRuntime::exec_dispatch_agent` takes before provisioning
    /// (`DispatchClaimExclusive` in `docs/specs/dispatch.allium`) — this check
    /// just avoids queueing a command that is already known to be pointless, and
    /// keeps a Running card from sprouting a spinner on a keypress.
    pub(in crate::tui) fn handle_dispatch_task(
        &mut self,
        id: TaskId,
        mode: DispatchMode,
    ) -> Vec<Command> {
        if self.dispatching.contains_key(&id) {
            return vec![];
        }
        let task = self
            .find_task(id)
            .filter(|t| t.status == TaskStatus::Backlog)
            .cloned();
        if let Some(task) = task {
            self.mark_dispatching(id);
            return vec![Command::Task(
                crate::tui::commands::TaskCommand::DispatchAgent { task, mode },
            )];
        }
        vec![]
    }

    pub(in crate::tui) fn handle_trust_and_dispatch(
        &mut self,
        id: TaskId,
        mode: DispatchMode,
    ) -> Vec<Command> {
        if self.dispatching.contains_key(&id) {
            return vec![];
        }
        let task = self
            .find_task(id)
            .filter(|t| t.status == TaskStatus::Backlog)
            .cloned();
        if let Some(task) = task {
            self.mark_dispatching(id);
            return vec![Command::Task(
                crate::tui::commands::TaskCommand::TrustAndDispatch { task, mode },
            )];
        }
        vec![]
    }

    /// Result of `CheckTrustAndDispatch` finding the repo untrusted: enter the
    /// trust-confirmation prompt. Guarded like [`Self::handle_dispatch_task`]
    /// against the async gap between the check starting and its result
    /// arriving (task dispatched/deleted/moved in the meantime).
    pub(in crate::tui) fn handle_trust_check_untrusted(
        &mut self,
        task_id: TaskId,
        mode: DispatchMode,
        repo_path: String,
    ) -> Vec<Command> {
        if self.is_dispatching(task_id)
            || !self
                .find_task(task_id)
                .is_some_and(|t| t.status == TaskStatus::Backlog)
        {
            return vec![];
        }
        let expanded = crate::models::expand_tilde(&repo_path);
        self.input.mode = InputMode::ConfirmTrustRepo { task_id, mode };
        self.set_status(format!(
            "Repo '{expanded}' not trusted by Claude Code — trust it? [y/N]"
        ));
        vec![]
    }

    /// Result of `QuickDispatch`'s trust check finding the repo untrusted:
    /// enter the quick-dispatch trust-confirmation prompt. Mirrors
    /// `handle_trust_check_untrusted`, but keyed on the pending `TaskDraft`
    /// since no task exists yet.
    pub(in crate::tui) fn handle_trust_check_untrusted_for_quick_dispatch(
        &mut self,
        draft: TaskDraft,
        epic_id: Option<EpicId>,
    ) -> Vec<Command> {
        let expanded = crate::models::expand_tilde(&draft.repo_path);
        self.set_status(format!(
            "Repo '{expanded}' not trusted by Claude Code — trust it? [y/N]"
        ));
        self.input.mode = InputMode::ConfirmTrustRepoQuickDispatch { draft, epic_id };
        vec![]
    }

    pub(in crate::tui) fn handle_dispatched(
        &mut self,
        id: TaskId,
        worktree: String,
        tmux_window: String,
        switch_focus: bool,
    ) -> Vec<Command> {
        self.unmark_dispatching(id);
        if let Some(task) = self.find_task_mut(id) {
            task.worktree = Some(worktree);
            task.tmux_window = Some(tmux_window.clone());
            task.status = TaskStatus::Running;
            task.sub_status = SubStatus::default_for(TaskStatus::Running);
            // The status/sub_status/stamp are set on the board's copy only — the
            // pre-provisioning claim already wrote all three to the DB
            // (`DispatchTask` in docs/specs/dispatch.allium), so there is no
            // SeedActivity command here. Emitting one would re-write the stamp
            // the claim just set, and would overwrite a real hook stamp outright
            // if `Dispatched` handling ever moved behind slower work.
            task.last_pre_tool_use_at = Some(chrono::Utc::now());
            let task_clone = task.clone();
            let repo_path = task_clone.repo_path.clone();
            self.sync_board_selection();
            let mut cmds = vec![Command::Task(crate::tui::commands::TaskCommand::Persist(
                task_clone,
            ))];
            if switch_focus {
                cmds.push(Command::Task(
                    crate::tui::commands::TaskCommand::JumpToTmux {
                        window: tmux_window,
                    },
                ));
            }
            // RefreshRepoSyncStateAfterDispatch: provisioning the worktree already
            // fetched origin/<base> for this repository, so this is a local ref
            // read with no network cost.
            cmds.push(Self::refresh_repo_sync_command(repo_path));
            cmds
        } else {
            vec![]
        }
    }

    pub(in crate::tui) fn handle_task_created(&mut self, task: Task) -> Vec<Command> {
        self.board.tasks.push(task);
        self.sync_board_selection();
        vec![]
    }

    /// Permanent removal, GATED on the teardown: when the task owns live resources
    /// the row delete is the cleanup's follow-up, not a sibling command, so a
    /// failed `git worktree remove` leaves the row in place — still archived,
    /// still pointing at what is on disk, and retryable by deleting again
    /// (`WorktreeReleaseIsGated` in docs/specs/tasks.allium). A task owning a
    /// window but no worktree also goes through the cleanup (for the kill), and
    /// still gets deleted whatever that reports: the gate keys on the worktree, so
    /// with nothing to release there is nothing to withhold the delete for. Only a
    /// task owning neither resource is deleted immediately, with no teardown.
    ///
    /// The card leaves the board either way; a failed cleanup pulls it back with
    /// a `RefreshFromDb` (see `handle_cleanup_failed`).
    pub(in crate::tui) fn handle_delete_task(&mut self, id: TaskId) -> Vec<Command> {
        let cleanup = self
            .find_task_mut(id)
            .and_then(|t| Self::take_cleanup(t, CleanupFollowUp::DeleteRow));
        self.clear_agent_tracking(id);
        self.board.tasks.retain(|t| t.id != id);
        self.sync_board_selection();
        let archive_col = TaskStatus::COLUMN_COUNT + 1;
        let archive_count = self.archived_tasks().len();
        if archive_count > 0 && self.selection().row(archive_col) >= archive_count {
            self.selection_mut().set_row(archive_col, archive_count - 1);
        }
        *self.archive.list_state.selected_mut() = Some(self.selection().row(archive_col));
        match cleanup {
            // The cleanup owns the delete — see the doc comment above.
            Some(c) => vec![c],
            None => vec![Command::Task(crate::tui::commands::TaskCommand::Delete(id))],
        }
    }

    /// The teardown released the worktree, so its follow-up is now safe to
    /// apply. This is the only path that clears the column or drops the row for
    /// a task that owned a worktree.
    pub(in crate::tui) fn handle_cleanup_succeeded(
        &mut self,
        id: TaskId,
        follow_up: CleanupFollowUp,
    ) -> Vec<Command> {
        match follow_up {
            CleanupFollowUp::ClearPointer => {
                // The board already cleared these optimistically; keeping it in
                // step matters for the paths that did not (a cleanup issued for
                // a task the board has since refreshed).
                if let Some(task) = self.find_task_mut(id) {
                    task.worktree = None;
                    task.tmux_window = None;
                }
                vec![Command::Task(
                    crate::tui::commands::TaskCommand::ClearWorktreePointer(id),
                )]
            }
            CleanupFollowUp::DeleteRow => {
                vec![Command::Task(crate::tui::commands::TaskCommand::Delete(id))]
            }
            // The row went with the epic that owned it — writing to it now would
            // only produce a spurious "not found" error.
            CleanupFollowUp::Nothing => vec![],
        }
    }

    /// The teardown could not release the worktree. Nothing is cleared and
    /// nothing is deleted: the row still names the directory and the branch that
    /// are still on disk, which is what makes the failure retryable rather than
    /// an invisible orphan. The board is pulled back in step with that row,
    /// since it dropped the card (or its pointers) optimistically.
    pub(in crate::tui) fn handle_cleanup_failed(
        &mut self,
        id: TaskId,
        worktree: String,
        error: String,
    ) -> Vec<Command> {
        let mut cmds = self.handle_error(format!(
            "Cleanup failed for task {}: {worktree} was left on disk ({error}). \
             Delete the task again to retry.",
            id.0
        ));
        cmds.push(Command::Task(
            crate::tui::commands::TaskCommand::RefreshFromDb,
        ));
        cmds
    }

    pub(in crate::tui) fn handle_open_task_detail(&mut self, task_id: TaskId) -> Vec<Command> {
        let previous = Box::new(self.board.view_mode.clone());
        self.board.view_mode = ViewMode::TaskDetail {
            task_id,
            scroll: 0,
            zoomed: false,
            max_scroll: 0,
            previous,
        };
        vec![]
    }

    pub(in crate::tui) fn handle_close_task_detail(&mut self) -> Vec<Command> {
        if let ViewMode::TaskDetail { previous, .. } = std::mem::take(&mut self.board.view_mode) {
            self.board.view_mode = *previous;
        }
        vec![]
    }

    pub(in crate::tui) fn handle_toggle_flattened(&mut self) -> Vec<Command> {
        self.board.flattened = !self.board.flattened;
        // Column item counts change when toggling (epics hidden / shown, and
        // tasks from the subtree merged in / split out), so selection row
        // indices may be out of bounds. Sync to follow the anchor.
        self.sync_board_selection();
        self.reset_column_scroll();
        vec![]
    }
}
