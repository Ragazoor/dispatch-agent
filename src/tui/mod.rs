pub mod input;
pub mod types;
pub mod ui;
mod handlers;

pub use types::*;

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::models::{Epic, EpicId, Task, TaskId, TaskStatus, TaskUsage, epic_status};

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

pub struct App {
    pub(in crate::tui) tasks: Vec<Task>,
    pub(in crate::tui) epics: Vec<Epic>,
    pub(in crate::tui) view_mode: ViewMode,
    pub(in crate::tui) detail_visible: bool,
    pub(in crate::tui) status_message: Option<String>,
    pub(in crate::tui) status_message_set_at: Option<Instant>,
    pub(in crate::tui) error_popup: Option<String>,
    pub(in crate::tui) repo_paths: Vec<String>,
    pub(in crate::tui) should_quit: bool,
    pub(in crate::tui) input: InputState,
    pub(in crate::tui) agents: AgentTracking,
    pub(in crate::tui) archive: ArchiveState,
    pub(in crate::tui) selected_tasks: HashSet<TaskId>,
    pub(in crate::tui) selected_epics: HashSet<EpicId>,
    pub(in crate::tui) rebase_conflict_tasks: HashSet<TaskId>,
    pub(in crate::tui) pending_done_tasks: Vec<TaskId>,
    pub(in crate::tui) notifications_enabled: bool,
    pub(in crate::tui) repo_filter: HashSet<String>,
    pub(in crate::tui) filter_presets: Vec<(String, HashSet<String>)>,
    pub(in crate::tui) review_prs: Vec<crate::models::ReviewPr>,
    pub(in crate::tui) review_board_loading: bool,
    pub(in crate::tui) last_review_fetch: Option<Instant>,
    pub(in crate::tui) usage: HashMap<TaskId, TaskUsage>,
    pub(in crate::tui) merge_queue: Option<MergeQueue>,
}

/// Format a title for display in confirmation prompts, truncating if longer than `max_len` chars.
pub(in crate::tui) fn truncate_title(title: &str, max_len: usize) -> String {
    if title.chars().count() <= max_len {
        format!("\"{title}\"")
    } else {
        let truncated: String = title.chars().take(max_len.saturating_sub(3)).collect();
        format!("\"{truncated}...\"")
    }
}

impl App {
    pub fn new(tasks: Vec<Task>, inactivity_timeout: Duration) -> Self {
        App {
            tasks,
            epics: Vec::new(),
            view_mode: ViewMode::default(),
            detail_visible: false,
            status_message: None,
            status_message_set_at: None,
            error_popup: None,
            repo_paths: Vec::new(),
            should_quit: false,
            input: InputState::default(),
            agents: AgentTracking::new(inactivity_timeout),
            archive: ArchiveState::default(),
            selected_tasks: HashSet::new(),
            selected_epics: HashSet::new(),
            rebase_conflict_tasks: HashSet::new(),
            pending_done_tasks: Vec::new(),
            notifications_enabled: true,
            repo_filter: HashSet::new(),
            filter_presets: Vec::new(),
            review_prs: Vec::new(),
            review_board_loading: false,
            last_review_fetch: None,
            usage: HashMap::new(),
            merge_queue: None,
        }
    }

    /// Get the current selection state (from whichever view mode is active).
    pub fn selection(&self) -> &BoardSelection {
        match &self.view_mode {
            ViewMode::Board(sel) => sel,
            ViewMode::Epic { selection, .. } => selection,
            ViewMode::ReviewBoard { saved_board, .. } => saved_board,
        }
    }

    /// Get mutable access to the current selection state.
    pub(in crate::tui) fn selection_mut(&mut self) -> &mut BoardSelection {
        match &mut self.view_mode {
            ViewMode::Board(sel) => sel,
            ViewMode::Epic { selection, .. } => selection,
            ViewMode::ReviewBoard { saved_board, .. } => saved_board,
        }
    }

    // Read-only accessors for code outside the tui module
    pub fn tasks(&self) -> &[Task] { &self.tasks }
    pub fn should_quit(&self) -> bool { self.should_quit }
    pub fn selected_column(&self) -> usize { self.selection().column() }
    pub fn selected_row(&self) -> &[usize; TaskStatus::COLUMN_COUNT] { &self.selection().selected_row }
    pub fn view_mode(&self) -> &ViewMode { &self.view_mode }
    pub fn epics(&self) -> &[Epic] { &self.epics }
    pub fn mode(&self) -> &InputMode { &self.input.mode }
    pub fn input_buffer(&self) -> &str { &self.input.buffer }
    pub fn detail_visible(&self) -> bool { self.detail_visible }
    pub fn tmux_outputs(&self) -> &std::collections::HashMap<TaskId, String> { &self.agents.tmux_outputs }
    pub fn status_message(&self) -> Option<&str> { self.status_message.as_deref() }
    pub fn error_popup(&self) -> Option<&str> { self.error_popup.as_deref() }
    pub fn repo_paths(&self) -> &[String] { &self.repo_paths }
    pub fn task_draft(&self) -> Option<&TaskDraft> { self.input.task_draft.as_ref() }
    pub fn stale_tasks(&self) -> &HashSet<TaskId> { &self.agents.stale_tasks }
    pub fn crashed_tasks(&self) -> &HashSet<TaskId> { &self.agents.crashed_tasks }
    pub fn inactivity_timeout(&self) -> Duration { self.agents.inactivity_timeout }
    pub fn show_archived(&self) -> bool { self.archive.visible }
    pub fn selected_archive_row(&self) -> usize { self.archive.selected_row }
    pub fn selected_tasks(&self) -> &HashSet<TaskId> { &self.selected_tasks }
    pub fn selected_epics(&self) -> &HashSet<EpicId> { &self.selected_epics }
    pub fn on_select_all(&self) -> bool { self.selection().on_select_all }
    pub fn has_selection(&self) -> bool { !self.selected_tasks.is_empty() || !self.selected_epics.is_empty() }
    pub fn rebase_conflict_tasks(&self) -> &HashSet<TaskId> { &self.rebase_conflict_tasks }
    pub fn merge_queue(&self) -> Option<&MergeQueue> { self.merge_queue.as_ref() }
    pub fn notifications_enabled(&self) -> bool { self.notifications_enabled }
    pub fn repo_filter(&self) -> &HashSet<String> { &self.repo_filter }
    pub fn filter_presets(&self) -> &[(String, HashSet<String>)] { &self.filter_presets }
    pub fn review_prs(&self) -> &[crate::models::ReviewPr] { &self.review_prs }
    pub fn review_board_loading(&self) -> bool { self.review_board_loading }

    /// Get the review board selection state, if currently in review board mode.
    pub fn review_selection(&self) -> Option<&ReviewBoardSelection> {
        match &self.view_mode {
            ViewMode::ReviewBoard { selection, .. } => Some(selection),
            _ => None,
        }
    }

    pub(in crate::tui) fn review_selection_mut(&mut self) -> Option<&mut ReviewBoardSelection> {
        match &mut self.view_mode {
            ViewMode::ReviewBoard { selection, .. } => Some(selection),
            _ => None,
        }
    }

    pub fn set_notifications_enabled(&mut self, enabled: bool) {
        self.notifications_enabled = enabled;
    }

    pub fn set_review_prs(&mut self, prs: Vec<crate::models::ReviewPr>) {
        self.review_prs = prs;
    }

    pub fn set_repo_filter(&mut self, filter: HashSet<String>) {
        self.repo_filter = filter;
        self.clamp_selection();
    }

    /// Set a transient status message with auto-clear timestamp.
    pub(in crate::tui) fn set_status(&mut self, msg: String) {
        self.status_message = Some(msg);
        self.status_message_set_at = Some(Instant::now());
    }

    /// Clear the status message and its timestamp.
    pub(in crate::tui) fn clear_status(&mut self) {
        self.status_message = None;
        self.status_message_set_at = None;
    }

    /// Return tasks visible in the current view.
    /// Board view: standalone tasks only (epic_id is None).
    /// Epic view: only subtasks of the active epic.
    pub fn tasks_for_current_view(&self) -> Vec<&Task> {
        let repo_match = |t: &&Task| {
            self.repo_filter.is_empty() || self.repo_filter.contains(t.repo_path.as_ref())
        };
        match &self.view_mode {
            ViewMode::Board(_) => {
                self.tasks.iter().filter(|t| t.epic_id.is_none() && t.status != TaskStatus::Archived).filter(repo_match).collect()
            }
            ViewMode::Epic { epic_id, .. } => {
                self.tasks.iter().filter(|t| t.epic_id == Some(*epic_id) && t.status != TaskStatus::Archived).filter(repo_match).collect()
            }
            ViewMode::ReviewBoard { .. } => vec![],
        }
    }

    /// Return tasks for a given status in the current view.
    pub fn tasks_by_status(&self, status: TaskStatus) -> Vec<&Task> {
        self.tasks_for_current_view()
            .into_iter()
            .filter(|t| t.status == status)
            .collect()
    }

    /// Return all archived tasks, ordered as they appear in self.tasks.
    pub fn archived_tasks(&self) -> Vec<&Task> {
        self.tasks.iter()
            .filter(|t| t.status == TaskStatus::Archived)
            .filter(|t| self.repo_filter.is_empty() || self.repo_filter.contains(t.repo_path.as_ref()))
            .collect()
    }

    /// Build a list of items (tasks + epics) for a column in the current view.
    /// In board view, epics are included (positioned by derived status).
    /// In epic view, only subtasks are included (no epic cards).
    pub fn column_items_for_status(&self, status: TaskStatus) -> Vec<ColumnItem<'_>> {
        let tasks = self.tasks_by_status(status);
        let mut items: Vec<ColumnItem<'_>> = tasks.into_iter().map(ColumnItem::Task).collect();

        if matches!(self.view_mode, ViewMode::Board(_)) {
            for epic in &self.epics {
                if !self.repo_filter.is_empty() && !self.repo_filter.contains(epic.repo_path.as_ref()) {
                    continue;
                }
                if epic_status(epic, &self.subtask_statuses(epic.id)) == status {
                    items.push(ColumnItem::Epic(epic));
                }
            }
        }

        items.sort_by_key(|item| {
            let (sort_order, id) = match item {
                ColumnItem::Task(t) => (t.sort_order, t.id.0),
                ColumnItem::Epic(e) => (e.sort_order, e.id.0),
            };
            (sort_order.unwrap_or(id), id)
        });

        items
    }

    /// Get the statuses of all subtasks belonging to an epic.
    fn subtask_statuses(&self, epic_id: EpicId) -> Vec<TaskStatus> {
        self.tasks
            .iter()
            .filter(|t| t.epic_id == Some(epic_id) && t.status != TaskStatus::Archived)
            .map(|t| t.status)
            .collect()
    }

    /// Return the item (task or epic) currently under the cursor.
    pub fn selected_column_item(&self) -> Option<ColumnItem<'_>> {
        if self.selection().on_select_all {
            return None;
        }
        let col = self.selection().column();
        let status = TaskStatus::from_column_index(col)?;
        let items = self.column_items_for_status(status);
        let row = self.selection().row(col);
        items.into_iter().nth(row)
    }

    /// Return the currently selected task (if the cursor is on a task), or None
    /// if the cursor is on an epic or the column is empty.
    pub fn selected_task(&self) -> Option<&Task> {
        match self.selected_column_item() {
            Some(ColumnItem::Task(task)) => Some(task),
            _ => None,
        }
    }

    /// Clamp all selected_row values to be within bounds for each column.
    pub fn clamp_selection(&mut self) {
        for col in 0..TaskStatus::COLUMN_COUNT {
            if let Some(status) = TaskStatus::from_column_index(col) {
                let count = self.column_items_for_status(status).len();
                let sel = self.selection_mut();
                if count == 0 {
                    sel.set_row(col, 0);
                } else if sel.row(col) >= count {
                    sel.set_row(col, count - 1);
                }
            }
        }
    }

    fn find_task(&self, id: TaskId) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == id)
    }

    fn find_task_mut(&mut self, id: TaskId) -> Option<&mut Task> {
        self.tasks.iter_mut().find(|t| t.id == id)
    }

    /// Remove all in-memory agent tracking state for a task.
    fn clear_agent_tracking(&mut self, id: TaskId) {
        self.agents.clear(id);
    }

    /// Take worktree/tmux fields from a task and build a Cleanup command.
    /// Returns `None` if the task has no worktree (still clears tmux_window).
    fn take_cleanup(task: &mut Task) -> Option<Command> {
        match task.worktree.take() {
            Some(wt) => Some(Command::Cleanup {
                id: task.id,
                repo_path: task.repo_path.clone(),
                worktree: wt,
                tmux_window: task.tmux_window.take(),
            }),
            None => {
                task.tmux_window.take();
                None
            }
        }
    }

    /// Take the tmux_window from a task and build a KillTmuxWindow command.
    /// Leaves the worktree intact so the task can be resumed later.
    fn take_detach(task: &mut Task) -> Option<Command> {
        task.tmux_window.take().map(|window| Command::KillTmuxWindow { window })
    }

    /// Process a message and return a list of side-effect commands.
    pub fn update(&mut self, msg: Message) -> Vec<Command> {
        match msg {
            Message::Tick => self.handle_tick(),
            Message::Quit => self.handle_quit(),
            Message::NavigateColumn(delta) => self.handle_navigate_column(delta),
            Message::NavigateRow(delta) => self.handle_navigate_row(delta),
            Message::MoveTask { id, direction } => self.handle_move_task(id, direction),
            Message::ReorderItem(dir) => self.handle_reorder_item(dir),
            Message::DispatchTask(id) => self.handle_dispatch_task(id),
            Message::BrainstormTask(id) => self.handle_brainstorm_task(id),
            Message::PlanTask(id) => self.handle_plan_task(id),
            Message::Dispatched { id, worktree, tmux_window, switch_focus } =>
                self.handle_dispatched(id, worktree, tmux_window, switch_focus),
            Message::TaskCreated { task } => self.handle_task_created(task),
            Message::DeleteTask(id) => self.handle_delete_task(id),
            Message::ToggleDetail => self.handle_toggle_detail(),
            Message::TmuxOutput { id, output, activity_ts } => self.handle_tmux_output(id, output, activity_ts),
            Message::WindowGone(id) => self.handle_window_gone(id),
            Message::RefreshTasks(tasks) => self.handle_refresh_tasks(tasks),
            Message::ResumeTask(id) => self.handle_resume_task(id),
            Message::Resumed { id, tmux_window } => self.handle_resumed(id, tmux_window),
            Message::Error(msg) => self.handle_error(msg),
            Message::TaskEdited(edit) =>
                self.handle_task_edited(edit),
            Message::RepoPathsUpdated(paths) => self.handle_repo_paths_updated(paths),
            Message::QuickDispatch { repo_path, epic_id } => self.handle_quick_dispatch(repo_path, epic_id),
            Message::StaleAgent(id) => self.handle_stale_agent(id),
            Message::AgentCrashed(id) => self.handle_agent_crashed(id),
            Message::KillAndRetry(id) => self.handle_kill_and_retry(id),
            Message::RetryResume(id) => self.handle_retry_resume(id),
            Message::RetryFresh(id) => self.handle_retry_fresh(id),
            Message::ArchiveTask(id) => self.handle_archive_task(id),
            Message::ToggleArchive => self.handle_toggle_archive(),
            Message::ToggleSelect(id) => self.handle_toggle_select(id),
            Message::ToggleSelectEpic(id) => self.handle_toggle_select_epic(id),
            Message::ClearSelection => self.handle_clear_selection(),
            Message::SelectAllColumn => self.handle_select_all_column(),
            Message::BatchMoveTasks { ids, direction } => self.handle_batch_move_tasks(ids, direction),
            Message::BatchArchiveTasks(ids) => self.handle_batch_archive_tasks(ids),
            Message::BatchArchiveEpics(ids) => self.handle_batch_archive_epics(ids),
            Message::DetachTmux(id) => self.handle_detach_tmux(vec![id]),
            Message::BatchDetachTmux(ids) => self.handle_detach_tmux(ids),
            Message::ConfirmDetachTmux => self.handle_confirm_detach_tmux(),
            Message::DismissError => self.handle_dismiss_error(),
            Message::StartNewTask => self.handle_start_new_task(),
            Message::CancelInput => self.handle_cancel_input(),
            Message::ConfirmDeleteStart => self.handle_confirm_delete_start(),
            Message::ConfirmDeleteYes => self.handle_confirm_delete_yes(),
            Message::CancelDelete => self.handle_cancel_delete(),
            Message::SubmitTitle(value) => self.handle_submit_title(value),
            Message::SubmitDescription(value) => self.handle_submit_description(value),
            Message::SubmitRepoPath(value) => self.handle_submit_repo_path(value),
            Message::SubmitTag(tag) => self.handle_submit_tag(tag),
            Message::InputChar(c) => self.handle_input_char(c),
            Message::InputBackspace => self.handle_input_backspace(),
            Message::StartQuickDispatchSelection => self.handle_start_quick_dispatch_selection(),
            Message::SelectQuickDispatchRepo(idx) => self.handle_select_quick_dispatch_repo(idx),
            Message::CancelRetry => self.handle_cancel_retry(),
            Message::StatusInfo(msg) => self.handle_status_info(msg),
            Message::ToggleHelp => self.handle_toggle_help(),
            // Finish (rebase + cleanup)
            Message::FinishComplete(id) => self.handle_finish_complete(id),
            Message::FinishFailed { id, error, is_conflict } =>
                self.handle_finish_failed(id, error, is_conflict),
            // Done confirmation (no cleanup, just status change)
            Message::ConfirmDone => self.handle_confirm_done(),
            Message::CancelDone => self.handle_cancel_done(),
            Message::ToggleNotifications => self.handle_toggle_notifications(),
            // Epic messages
            Message::DispatchEpic(id) => self.handle_dispatch_epic(id),
            Message::EnterEpic(epic_id) => self.handle_enter_epic(epic_id),
            Message::ExitEpic => self.handle_exit_epic(),
            Message::RefreshEpics(epics) => self.handle_refresh_epics(epics),
            Message::RefreshUsage(usage) => self.handle_refresh_usage(usage),
            Message::EpicCreated(epic) => self.handle_epic_created(epic),
            Message::EditEpic(id) => self.handle_edit_epic(id),
            Message::EpicEdited(epic) => self.handle_epic_edited(epic),
            Message::DeleteEpic(id) => self.handle_delete_epic(id),
            Message::ConfirmDeleteEpic => self.handle_confirm_delete_epic(),
            Message::MarkEpicDone(id) => self.handle_mark_epic_done(id),
            Message::MarkEpicUndone(id) => self.handle_mark_epic_undone(id),
            Message::ConfirmEpicDone => self.handle_confirm_epic_done(),
            Message::CancelEpicDone => self.handle_cancel_epic_done(),
            Message::ArchiveEpic(id) => self.handle_archive_epic(id),
            Message::ConfirmArchiveEpic => self.handle_confirm_archive_epic(),
            Message::StartNewEpic => self.handle_start_new_epic(),
            Message::SubmitEpicTitle(v) => self.handle_submit_epic_title(v),
            Message::SubmitEpicDescription(v) => self.handle_submit_epic_description(v),
            Message::SubmitEpicRepoPath(v) => self.handle_submit_epic_repo_path(v),
            // PR flow
            Message::PrCreated { id, pr_url } => self.handle_pr_created(id, pr_url),
            Message::PrFailed { id, error } => self.handle_pr_failed(id, error),
            Message::PrMerged(id) => self.handle_pr_merged(id),
            // Repo filter
            Message::StartRepoFilter => self.handle_start_repo_filter(),
            Message::CloseRepoFilter => self.handle_close_repo_filter(),
            Message::ToggleRepoFilter(path) => self.handle_toggle_repo_filter(path),
            Message::ToggleAllRepoFilter => self.handle_toggle_all_repo_filter(),
            // Wrap up
            Message::StartWrapUp(id) => self.handle_start_wrap_up(id),
            Message::WrapUpRebase => self.handle_wrap_up_rebase(),
            Message::WrapUpPr => self.handle_wrap_up_pr(),
            Message::CancelWrapUp => self.handle_cancel_wrap_up(),
            // Epic batch wrap-up
            Message::StartEpicWrapUp(id) => self.handle_start_epic_wrap_up(id),
            Message::EpicWrapUpRebase => self.handle_epic_wrap_up(MergeAction::Rebase),
            Message::EpicWrapUpPr => self.handle_epic_wrap_up(MergeAction::Pr),
            Message::CancelEpicWrapUp => self.handle_cancel_epic_wrap_up(),
            Message::CancelMergeQueue => self.handle_cancel_merge_queue(),
            // Review board
            Message::SwitchToReviewBoard => self.handle_switch_to_review_board(),
            Message::SwitchToTaskBoard => self.handle_switch_to_task_board(),
            Message::ReviewPrsLoaded(prs) => self.handle_review_prs_loaded(prs),
            Message::ReviewPrsFetchFailed(err) => self.handle_review_prs_fetch_failed(err),
            Message::OpenInBrowser { url } => vec![Command::OpenInBrowser { url }],
            Message::RefreshReviewPrs => {
                self.review_board_loading = true;
                vec![Command::FetchReviewPrs]
            }
            // Filter presets
            Message::StartSavePreset => self.handle_start_save_preset(),
            Message::SaveFilterPreset(name) => self.handle_save_filter_preset(name),
            Message::LoadFilterPreset(name) => self.handle_load_filter_preset(name),
            Message::StartDeletePreset => self.handle_start_delete_preset(),
            Message::DeleteFilterPreset(name) => self.handle_delete_filter_preset(name),
            Message::CancelPresetInput => self.handle_cancel_preset_input(),
            Message::FilterPresetsLoaded(presets) => self.handle_filter_presets_loaded(presets),
        }
    }

    /// Get the currently selected ReviewPr, if in review board mode.
    pub fn selected_review_pr(&self) -> Option<&crate::models::ReviewPr> {
        let sel = self.review_selection()?;
        let col = sel.column();
        let row = sel.row(col);
        let decision = crate::models::ReviewDecision::from_column_index(col)?;
        self.review_prs
            .iter()
            .filter(|pr| pr.review_decision == decision)
            .nth(row)
    }

    pub(in crate::tui) fn navigate_review_row(&mut self, delta: isize) {
        let (col, count) = match self.review_selection() {
            Some(sel) => {
                let col = sel.selected_column;
                let count = self.review_prs.iter()
                    .filter(|pr| pr.review_decision.column_index() == col)
                    .count();
                (col, count)
            }
            None => return,
        };
        if count == 0 {
            return;
        }
        if let Some(sel) = self.review_selection_mut() {
            let current = sel.selected_row[col] as isize;
            let new = (current + delta).clamp(0, (count - 1) as isize) as usize;
            sel.selected_row[col] = new;
        }
    }

    /// Get PRs for a specific review decision column.
    pub fn review_prs_by_decision(&self, decision: crate::models::ReviewDecision) -> Vec<&crate::models::ReviewPr> {
        self.review_prs.iter()
            .filter(|pr| pr.review_decision == decision)
            .collect()
    }

}

#[cfg(test)]
mod tests;
