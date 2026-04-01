pub mod input;
pub mod types;
pub mod ui;

pub use types::*;

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::dispatch;
use crate::models::{Epic, EpicId, SubStatus, Task, TaskId, TaskStatus, TaskUsage, VisualColumn, epic_status};

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
    pub fn is_stale(&self, id: TaskId) -> bool { self.find_task(id).is_some_and(|t| t.sub_status == SubStatus::Stale) }
    pub fn is_crashed(&self, id: TaskId) -> bool { self.find_task(id).is_some_and(|t| t.sub_status == SubStatus::Crashed) }
    pub fn inactivity_timeout(&self) -> Duration { self.agents.inactivity_timeout }
    pub fn show_archived(&self) -> bool { self.archive.visible }
    pub fn selected_archive_row(&self) -> usize { self.archive.selected_row }
    pub fn selected_tasks(&self) -> &HashSet<TaskId> { &self.selected_tasks }
    pub fn selected_epics(&self) -> &HashSet<EpicId> { &self.selected_epics }
    pub fn on_select_all(&self) -> bool { self.selection().on_select_all }
    pub fn has_selection(&self) -> bool { !self.selected_tasks.is_empty() || !self.selected_epics.is_empty() }

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
            self.repo_filter.is_empty() || self.repo_filter.contains(&t.repo_path)
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
            .filter(|t| self.repo_filter.is_empty() || self.repo_filter.contains(&t.repo_path))
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
                if !self.repo_filter.is_empty() && !self.repo_filter.contains(&epic.repo_path) {
                    continue;
                }
                if epic_status(epic, &self.subtask_statuses(epic.id)) == status {
                    items.push(ColumnItem::Epic(epic));
                }
            }
        }

        items.sort_by_key(|item| {
            match item {
                ColumnItem::Task(t) => {
                    (t.sub_status.column_priority(), t.sort_order.unwrap_or(t.id.0), t.id.0)
                }
                ColumnItem::Epic(e) => (9u8, e.sort_order.unwrap_or(e.id.0), e.id.0),
            }
        });

        items
    }

    /// Build a list of items (tasks + epics) for a visual column.
    /// Tasks are filtered by parent_status and sub_status matching the visual column.
    /// Epics appear in the first visual column of their parent status group.
    pub fn column_items_for_visual_column(&self, vcol_idx: usize) -> Vec<ColumnItem<'_>> {
        let vcol = &VisualColumn::ALL[vcol_idx];
        let tasks: Vec<&Task> = self.tasks_for_current_view()
            .into_iter()
            .filter(|t| t.status == vcol.parent_status && vcol.contains(t.sub_status))
            .collect();

        let mut items: Vec<ColumnItem<'_>> = tasks.into_iter().map(ColumnItem::Task).collect();

        // Include epics in board view — place them in the first visual column of their parent status
        if matches!(self.view_mode, ViewMode::Board(_))
            && vcol_idx == VisualColumn::parent_group_start(vcol.parent_status)
        {
            for epic in &self.epics {
                if !self.repo_filter.is_empty() && !self.repo_filter.contains(&epic.repo_path) {
                    continue;
                }
                if epic_status(epic, &self.subtask_statuses(epic.id)) == vcol.parent_status {
                    items.push(ColumnItem::Epic(epic));
                }
            }
        }

        items.sort_by_key(|item| {
            let (sort_order, id) = match item {
                ColumnItem::Task(t) => (t.sort_order, t.id.0),
                ColumnItem::Epic(e) => (e.sort_order, e.id.0),
            };
            (sort_order.unwrap_or(i64::MAX), id)
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
        for (col, &status) in TaskStatus::ALL.iter().enumerate() {
            let count = self.column_items_for_status(status).len();
            let sel = self.selection_mut();
            if count == 0 {
                sel.set_row(col, 0);
            } else if sel.row(col) >= count {
                sel.set_row(col, count - 1);
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
            Message::PrReviewState { id, review_decision } => self.handle_pr_review_state(id, review_decision),
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
            // Detach tmux panel
            Message::DetachTmux(id) => self.handle_detach_tmux(vec![id]),
            Message::BatchDetachTmux(ids) => self.handle_detach_tmux(ids),
            Message::ConfirmDetachTmux => self.handle_confirm_detach_tmux(),
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

    // -----------------------------------------------------------------------
    // Per-message handlers
    // -----------------------------------------------------------------------

    fn handle_detach_tmux(&mut self, ids: Vec<TaskId>) -> Vec<Command> {
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

    fn handle_confirm_detach_tmux(&mut self) -> Vec<Command> {
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
                // Reset sub_status when detaching (e.g. Stale/Crashed -> default)
                if task.sub_status == SubStatus::Stale || task.sub_status == SubStatus::Crashed {
                    task.sub_status = SubStatus::default_for(task.status);
                }
                let task_clone = task.clone();
                cmds.push(Command::PersistTask(task_clone));
            }
        }
        cmds
    }

    fn handle_quit(&mut self) -> Vec<Command> {
        self.should_quit = true;
        vec![]
    }

    fn handle_navigate_column(&mut self, delta: isize) -> Vec<Command> {
        let new_col = (self.selection().column() as isize + delta)
            .clamp(0, (TaskStatus::COLUMN_COUNT - 1) as isize) as usize;
        self.selection_mut().set_column(new_col);
        self.clamp_selection();
        vec![]
    }

    fn handle_navigate_row(&mut self, delta: isize) -> Vec<Command> {
        let col = self.selection().column();
        if col >= TaskStatus::COLUMN_COUNT {
            return vec![];
        }
        let status = match TaskStatus::from_column_index(col) {
            Some(s) => s,
            None => return vec![],
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

    fn handle_reorder_item(&mut self, direction: isize) -> Vec<Command> {
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

    fn handle_move_task(&mut self, id: TaskId, direction: MoveDirection) -> Vec<Command> {
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
            task.sub_status = SubStatus::default_for(new_status);
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

    fn handle_confirm_done(&mut self) -> Vec<Command> {
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

    fn handle_cancel_done(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.clear_status();
        self.pending_done_tasks.clear();
        vec![]
    }

    fn handle_toggle_notifications(&mut self) -> Vec<Command> {
        self.notifications_enabled = !self.notifications_enabled;
        let label = if self.notifications_enabled { "Notifications enabled" } else { "Notifications disabled" };
        self.set_status(label.to_string());
        vec![Command::PersistSetting {
            key: "notifications_enabled".to_string(),
            value: self.notifications_enabled,
        }]
    }

    fn handle_dispatch_task(&mut self, id: TaskId) -> Vec<Command> {
        if let Some(task) = self.find_task(id) {
            if task.status == TaskStatus::Backlog {
                return vec![Command::Dispatch { task: task.clone() }];
            }
        }
        vec![]
    }

    fn handle_brainstorm_task(&mut self, id: TaskId) -> Vec<Command> {
        if let Some(task) = self.find_task(id) {
            if task.status == TaskStatus::Backlog {
                return vec![Command::Brainstorm { task: task.clone() }];
            }
        }
        vec![]
    }

    fn handle_plan_task(&mut self, id: TaskId) -> Vec<Command> {
        if let Some(task) = self.find_task(id) {
            if task.status == TaskStatus::Backlog {
                return vec![Command::Plan { task: task.clone() }];
            }
        }
        vec![]
    }

    fn handle_dispatched(&mut self, id: TaskId, worktree: String, tmux_window: String, switch_focus: bool) -> Vec<Command> {
        if let Some(task) = self.find_task_mut(id) {
            task.worktree = Some(worktree);
            task.tmux_window = Some(tmux_window.clone());
            task.status = TaskStatus::Running;
            task.sub_status = SubStatus::default_for(TaskStatus::Running);
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

    fn handle_task_created(&mut self, task: Task) -> Vec<Command> {
        self.tasks.push(task);
        self.clamp_selection();
        vec![]
    }

    fn handle_delete_task(&mut self, id: TaskId) -> Vec<Command> {
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

    fn handle_toggle_detail(&mut self) -> Vec<Command> {
        self.detail_visible = !self.detail_visible;
        vec![]
    }

    fn handle_tmux_output(&mut self, id: TaskId, output: String, activity_ts: u64) -> Vec<Command> {
        let mut cmds = Vec::new();
        let activity_changed = self.agents.last_activity
            .get(&id)
            .is_none_or(|&prev| prev != activity_ts);
        if activity_changed {
            self.agents.last_output_change.insert(id, Instant::now());
            // Recovery: reset stale/crashed sub_status when activity resumes
            let needs_recovery = self.find_task(id)
                .is_some_and(|t| matches!(t.sub_status, SubStatus::Stale | SubStatus::Crashed));
            if needs_recovery {
                if let Some(task) = self.find_task_mut(id) {
                    task.sub_status = SubStatus::Active;
                }
                if let Some(task) = self.find_task(id) {
                    cmds.push(Command::PersistTask(task.clone()));
                }
            }
            self.agents.last_activity.insert(id, activity_ts);
        }
        self.agents.tmux_outputs.insert(id, output);
        cmds
    }

    fn handle_window_gone(&mut self, id: TaskId) -> Vec<Command> {
        if let Some(task) = self.find_task(id) {
            if task.status == TaskStatus::Running {
                // Running task lost its window — likely crashed
                return self.handle_agent_crashed(id);
            }
        }
        // Non-running task: existing behavior
        if let Some(task) = self.find_task_mut(id) {
            task.tmux_window = None;
            let task_clone = task.clone();
            vec![Command::PersistTask(task_clone)]
        } else {
            vec![]
        }
    }

    fn handle_refresh_tasks(&mut self, new_tasks: Vec<Task>) -> Vec<Command> {
        let mut cmds = Vec::new();

        for new_task in &new_tasks {
            if self.notifications_enabled {
                // Extract old state before any mutable borrows
                let old_task = self.find_task(new_task.id);
                let was_needs_input = old_task.is_some_and(|t| t.sub_status == SubStatus::NeedsInput);
                let was_review = old_task.is_some_and(|t| t.status == TaskStatus::Review);

                // Detect NeedsInput transition (running tasks only)
                // TODO: Task 5 will add proper notification dedup for sub_status
                if new_task.sub_status == SubStatus::NeedsInput && !was_needs_input
                    && new_task.status == TaskStatus::Running
                {
                    cmds.push(Command::SendNotification {
                        title: format!("Task #{}: {}", new_task.id.0, new_task.title),
                        body: "Agent needs your input".to_string(),
                        urgent: true,
                    });
                }

                // Detect review transition
                if new_task.status == TaskStatus::Review && !was_review
                    && !self.agents.notified_review.contains(&new_task.id)
                {
                    self.agents.notified_review.insert(new_task.id);
                    cmds.push(Command::SendNotification {
                        title: format!("Task #{}: {}", new_task.id.0, new_task.title),
                        body: "Ready for review".to_string(),
                        urgent: false,
                    });
                }
            }

            // Always clear notified state when task leaves the triggering state,
            // even when notifications are disabled. This prevents stale entries from
            // suppressing notifications after re-enabling.
            if new_task.status != TaskStatus::Review {
                self.agents.notified_review.remove(&new_task.id);
            }
        }

        // Merge DB state into in-memory state, preserving tmux_outputs
        // Prune selections for tasks that no longer exist
        let valid_ids: HashSet<TaskId> = new_tasks.iter().map(|t| t.id).collect();
        self.selected_tasks.retain(|id| valid_ids.contains(id));
        self.tasks = new_tasks;
        self.clamp_selection();
        cmds
    }

    fn handle_tick(&mut self) -> Vec<Command> {
        // Auto-clear transient status messages after 5 seconds (only in Normal mode)
        if self.input.mode == InputMode::Normal {
            if let Some(set_at) = self.status_message_set_at {
                if set_at.elapsed() > Duration::from_secs(5) {
                    self.clear_status();
                }
            }
        }

        let mut cmds: Vec<Command> = self
            .tasks
            .iter()
            .filter(|t| t.tmux_window.is_some())
            .filter_map(|t| {
                t.tmux_window.clone().map(|window| Command::CaptureTmux {
                    id: t.id,
                    window,
                })
            })
            .collect();

        // Check for stale agents
        let timeout = self.agents.inactivity_timeout;
        let newly_stale: Vec<TaskId> = self
            .tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Running && t.tmux_window.is_some())
            .filter(|t| t.sub_status != SubStatus::Stale && t.sub_status != SubStatus::Crashed)
            .filter(|t| {
                self.agents.last_output_change
                    .get(&t.id)
                    .is_some_and(|instant| instant.elapsed() > timeout)
            })
            .map(|t| t.id)
            .collect();

        for id in newly_stale {
            let stale_cmds = self.handle_stale_agent(id);
            cmds.extend(stale_cmds);
        }

        // Poll PR status for review tasks with open PRs
        let pr_poll_interval = Duration::from_secs(30);
        let pr_tasks: Vec<(TaskId, String)> = self
            .tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Review && t.pr_url.is_some())
            .filter(|t| {
                self.agents
                    .last_pr_poll
                    .get(&t.id)
                    .is_none_or(|last| last.elapsed() > pr_poll_interval)
            })
            .map(|t| (t.id, t.pr_url.clone().unwrap()))
            .collect();

        for (id, pr_url) in pr_tasks {
            self.agents.last_pr_poll.insert(id, Instant::now());
            cmds.push(Command::CheckPrStatus { id, pr_url });
        }

        // Refresh review board data if stale (> 60s), regardless of active tab
        let needs_fetch = self.last_review_fetch
            .map(|t| t.elapsed() > Duration::from_secs(60))
            .unwrap_or(true);
        if needs_fetch && !self.review_board_loading {
            self.review_board_loading = true;
            cmds.push(Command::FetchReviewPrs);
        }

        cmds.push(Command::RefreshFromDb);
        cmds
    }

    fn handle_stale_agent(&mut self, id: TaskId) -> Vec<Command> {
        // Only applies to Running tasks
        let dominated = match self.find_task(id) {
            Some(t) if t.status == TaskStatus::Running => {
                // Escalation only: don't downgrade Crashed to Stale
                t.sub_status == SubStatus::Crashed
            }
            _ => return vec![],
        };
        if dominated {
            return vec![];
        }

        let mut cmds = Vec::new();

        if let Some(task) = self.find_task_mut(id) {
            task.sub_status = SubStatus::Stale;
        }
        let elapsed = self.agents.last_output_change
            .get(&id)
            .map(|t| t.elapsed().as_secs() / 60)
            .unwrap_or(0);
        if let Some(task) = self.find_task(id) {
            cmds.push(Command::PersistTask(task.clone()));
        }
        self.set_status(format!(
            "Task {id} inactive for {elapsed}m - press d to retry",
        ));

        if self.notifications_enabled {
            if let Some(task) = self.find_task(id) {
                cmds.push(Command::SendNotification {
                    title: format!("Task #{}: {}", task.id.0, task.title),
                    body: format!("Agent inactive for {elapsed}m"),
                    urgent: false,
                });
            }
        }
        cmds
    }

    fn handle_agent_crashed(&mut self, id: TaskId) -> Vec<Command> {
        // Only applies to Running tasks
        if !self.find_task(id).is_some_and(|t| t.status == TaskStatus::Running) {
            return vec![];
        }

        let mut cmds = Vec::new();

        if let Some(task) = self.find_task_mut(id) {
            task.sub_status = SubStatus::Crashed;
        }
        if let Some(task) = self.find_task(id) {
            cmds.push(Command::PersistTask(task.clone()));
        }
        self.set_status(format!(
            "Task {id} agent crashed - press d to retry",
        ));

        if self.notifications_enabled {
            if let Some(task) = self.find_task(id) {
                cmds.push(Command::SendNotification {
                    title: format!("Task #{}: {}", task.id.0, task.title),
                    body: "Agent crashed".to_string(),
                    urgent: true,
                });
            }
        }
        cmds
    }

    fn handle_resume_task(&mut self, id: TaskId) -> Vec<Command> {
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

    fn handle_resumed(&mut self, id: TaskId, tmux_window: String) -> Vec<Command> {
        if let Some(task) = self.find_task_mut(id) {
            task.tmux_window = Some(tmux_window);
            task.status = TaskStatus::Running;
            task.sub_status = SubStatus::Active;
            let task_clone = task.clone();
            self.agents.last_output_change.insert(id, Instant::now());
            self.clamp_selection();
            vec![Command::PersistTask(task_clone)]
        } else {
            vec![]
        }
    }

    fn handle_error(&mut self, msg: String) -> Vec<Command> {
        self.error_popup = Some(msg);
        vec![]
    }

    fn handle_task_edited(&mut self, edit: TaskEdit) -> Vec<Command> {
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

    fn handle_repo_paths_updated(&mut self, paths: Vec<String>) -> Vec<Command> {
        self.repo_paths = paths;
        vec![]
    }

    fn handle_quick_dispatch(&mut self, repo_path: String, epic_id: Option<EpicId>) -> Vec<Command> {
        vec![Command::QuickDispatch {
            draft: TaskDraft {
                title: "Quick task".to_string(),
                description: String::new(),
                repo_path,
                tag: None,
            },
            epic_id,
        }]
    }

    fn handle_kill_and_retry(&mut self, id: TaskId) -> Vec<Command> {
        self.input.mode = InputMode::ConfirmRetry(id);
        let label = if self.find_task(id).is_some_and(|t| t.sub_status == SubStatus::Crashed) {
            "crashed"
        } else {
            "stale"
        };
        self.set_status(format!(
            "Agent {} - [r] Resume  [f] Fresh start  [Esc] Cancel", label
        ));
        vec![]
    }

    fn handle_retry_resume(&mut self, id: TaskId) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.clear_status();
        self.clear_agent_tracking(id);

        if let Some(task) = self.find_task_mut(id) {
            if task.status != TaskStatus::Running {
                return vec![];
            }
            if task.worktree.is_none() {
                self.set_status("Cannot resume: task has no worktree".to_string());
                return vec![];
            }
            task.sub_status = SubStatus::Active;
            let old_window = task.tmux_window.take();
            let task_clone = task.clone();

            let mut cmds = Vec::new();
            if let Some(window) = old_window {
                cmds.push(Command::KillTmuxWindow { window });
            }
            cmds.push(Command::Resume { task: task_clone });
            cmds
        } else {
            vec![]
        }
    }

    fn handle_retry_fresh(&mut self, id: TaskId) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.clear_status();
        self.clear_agent_tracking(id);

        if let Some(task) = self.find_task_mut(id) {
            if task.status != TaskStatus::Running {
                return vec![];
            }
            let cleanup = Self::take_cleanup(task);
            task.status = TaskStatus::Backlog;
            task.sub_status = SubStatus::None;
            let task_clone = task.clone();

            let mut cmds = Vec::new();
            if let Some(c) = cleanup {
                cmds.push(c);
            }
            cmds.push(Command::PersistTask(task_clone.clone()));
            cmds.push(Command::Dispatch { task: task_clone });
            cmds
        } else {
            vec![]
        }
    }

    fn handle_archive_task(&mut self, id: TaskId) -> Vec<Command> {
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

    fn handle_toggle_archive(&mut self) -> Vec<Command> {
        self.archive.visible = !self.archive.visible;
        if self.archive.visible {
            self.archive.selected_row = 0;
            *self.archive.list_state.selected_mut() = Some(0);
        }
        vec![]
    }

    fn handle_toggle_select(&mut self, id: TaskId) -> Vec<Command> {
        if self.selected_tasks.contains(&id) {
            self.selected_tasks.remove(&id);
        } else {
            self.selected_tasks.insert(id);
        }
        vec![]
    }

    fn handle_toggle_select_epic(&mut self, id: EpicId) -> Vec<Command> {
        if self.selected_epics.contains(&id) {
            self.selected_epics.remove(&id);
        } else {
            self.selected_epics.insert(id);
        }
        vec![]
    }

    fn handle_clear_selection(&mut self) -> Vec<Command> {
        self.selected_tasks.clear();
        self.selected_epics.clear();
        self.selection_mut().on_select_all = false;
        vec![]
    }

    fn handle_select_all_column(&mut self) -> Vec<Command> {
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

    fn handle_batch_archive_epics(&mut self, ids: Vec<EpicId>) -> Vec<Command> {
        let mut cmds = Vec::new();
        for id in ids {
            cmds.extend(self.handle_archive_epic(id));
        }
        self.selected_epics.clear();
        self.selected_tasks.clear();
        cmds
    }

    fn handle_batch_move_tasks(&mut self, ids: Vec<TaskId>, direction: MoveDirection) -> Vec<Command> {
        if matches!(direction, MoveDirection::Forward) {
            let review_ids: Vec<TaskId> = ids.iter().copied().filter(|id| {
                self.find_task(*id).is_some_and(|t| t.status == TaskStatus::Review)
            }).collect();

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

    fn handle_batch_archive_tasks(&mut self, ids: Vec<TaskId>) -> Vec<Command> {
        let mut cmds = Vec::new();
        for id in ids {
            cmds.extend(self.handle_archive_task(id));
        }
        self.selected_tasks.clear();
        cmds
    }

    fn handle_dismiss_error(&mut self) -> Vec<Command> {
        self.error_popup = None;
        vec![]
    }

    fn handle_start_new_task(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::InputTitle;
        self.input.buffer.clear();
        self.input.task_draft = None;
        self.set_status("Enter title: ".to_string());
        vec![]
    }

    fn handle_cancel_input(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.input.buffer.clear();
        self.input.task_draft = None;
        self.clear_status();
        vec![]
    }

    fn handle_confirm_delete_start(&mut self) -> Vec<Command> {
        if let Some(task) = self.selected_task() {
            let title = truncate_title(&task.title, 30);
            let status = task.status.as_str();
            let warning = if task.worktree.is_some() { " (has worktree)" } else { "" };
            self.input.mode = InputMode::ConfirmDelete;
            self.set_status(format!("Delete {title} [{status}]{warning}? (y/n)"));
        }
        vec![]
    }

    fn handle_confirm_delete_yes(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.clear_status();
        if let Some(task) = self.selected_task() {
            let id = task.id;
            self.handle_delete_task(id)
        } else {
            vec![]
        }
    }

    fn handle_cancel_delete(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.clear_status();
        vec![]
    }

    fn handle_submit_title(&mut self, value: String) -> Vec<Command> {
        self.input.buffer.clear();
        if value.is_empty() {
            self.input.mode = InputMode::Normal;
            self.input.task_draft = None;
            self.clear_status();
        } else {
            self.input.task_draft = Some(TaskDraft {
                title: value,
                description: String::new(),
                repo_path: String::new(),
                tag: None,
            });
            self.input.mode = InputMode::InputDescription;
            self.set_status("Enter description: ".to_string());
        }
        vec![]
    }

    fn handle_submit_description(&mut self, value: String) -> Vec<Command> {
        self.input.buffer.clear();
        if let Some(ref mut draft) = self.input.task_draft {
            draft.description = value;
        }
        self.input.mode = InputMode::InputRepoPath;
        self.set_status("Enter repo path: ".to_string());
        vec![]
    }

    fn handle_submit_repo_path(&mut self, value: String) -> Vec<Command> {
        self.input.buffer.clear();
        let repo_path = if value.is_empty() {
            if let Some(first) = self.repo_paths.first() {
                first.clone()
            } else {
                self.set_status("Repo path required (no saved paths available)".to_string());
                return vec![];
            }
        } else {
            value
        };
        if let Some(ref mut draft) = self.input.task_draft {
            draft.repo_path = repo_path;
        }
        self.input.mode = InputMode::InputTag;
        self.set_status("Tag: (b)ug (f)eature (c)hore (e)pic (Enter=none)".to_string());
        vec![]
    }

    fn handle_submit_tag(&mut self, tag: Option<String>) -> Vec<Command> {
        if let Some(ref mut draft) = self.input.task_draft {
            draft.tag = tag;
        }
        let repo_path = self.input.task_draft.as_ref()
            .map(|d| d.repo_path.clone())
            .unwrap_or_default();
        self.finish_task_creation(repo_path)
    }

    fn handle_input_char(&mut self, c: char) -> Vec<Command> {
        // In repo path mode with empty buffer, 1-9 selects a saved path
        if (self.input.mode == InputMode::InputRepoPath
            || self.input.mode == InputMode::InputEpicRepoPath)
            && self.input.buffer.is_empty()
            && c.is_ascii_digit()
            && c != '0'
        {
            let idx = (c as usize) - ('1' as usize);
            if idx < self.repo_paths.len() {
                let repo_path = self.repo_paths[idx].clone();
                if self.input.mode == InputMode::InputEpicRepoPath {
                    return self.finish_epic_creation(repo_path);
                }
                // For tasks, go through the tag selection step
                if let Some(ref mut draft) = self.input.task_draft {
                    draft.repo_path = repo_path;
                }
                self.input.mode = InputMode::InputTag;
                self.set_status("Tag: (b)ug (f)eature (c)hore (e)pic (Enter=none)".to_string());
                return vec![];
            }
        }
        self.input.buffer.push(c);
        vec![]
    }

    fn handle_input_backspace(&mut self) -> Vec<Command> {
        self.input.buffer.pop();
        vec![]
    }

    fn handle_start_quick_dispatch_selection(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::QuickDispatch;
        self.set_status("Select repo path (1-9) or Esc to cancel".to_string());
        vec![]
    }

    fn handle_select_quick_dispatch_repo(&mut self, idx: usize) -> Vec<Command> {
        if idx < self.repo_paths.len() {
            let repo_path = self.repo_paths[idx].clone();
            self.input.mode = InputMode::Normal;
            self.clear_status();
            self.handle_quick_dispatch(repo_path, None)
        } else {
            vec![]
        }
    }

    fn handle_cancel_retry(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.clear_status();
        vec![]
    }

    fn handle_status_info(&mut self, msg: String) -> Vec<Command> {
        self.set_status(msg);
        vec![]
    }

    fn handle_toggle_help(&mut self) -> Vec<Command> {
        if self.input.mode == InputMode::Help {
            self.input.mode = InputMode::Normal;
        } else {
            self.input.mode = InputMode::Help;
        }
        vec![]
    }

    fn finish_task_creation(&mut self, repo_path: String) -> Vec<Command> {
        let mut draft = self.input.task_draft.take().unwrap_or_default();
        draft.repo_path = repo_path.clone();
        self.input.mode = InputMode::Normal;
        self.clear_status();
        let epic_id = match &self.view_mode {
            ViewMode::Epic { epic_id, .. } => Some(*epic_id),
            _ => None,
        };
        vec![
            Command::InsertTask { draft, epic_id },
            Command::SaveRepoPath(repo_path),
        ]
    }

    // -----------------------------------------------------------------------
    // Finish handlers (rebase + cleanup)
    // -----------------------------------------------------------------------

    fn handle_finish_complete(&mut self, id: TaskId) -> Vec<Command> {
        let in_queue = self.merge_queue.as_ref().is_some_and(|q| q.current == Some(id));

        let mut cmds = if let Some(task) = self.find_task_mut(id) {
            task.tmux_window = None;
            task.status = TaskStatus::Done;
            task.sub_status = SubStatus::None;
            let task_clone = task.clone();
            self.clear_agent_tracking(id);
            self.clamp_selection();
            if !in_queue {
                self.set_status(format!("Task {} finished", id));
            }
            vec![Command::PersistTask(task_clone)]
        } else {
            vec![]
        };

        if in_queue {
            if let Some(q) = &mut self.merge_queue {
                q.completed += 1;
                q.current = None;
            }
            cmds.extend(self.advance_merge_queue());
        }

        cmds
    }

    fn handle_finish_failed(&mut self, id: TaskId, error: String, is_conflict: bool) -> Vec<Command> {
        let mut cmds = Vec::new();

        if is_conflict {
            if let Some(task) = self.find_task_mut(id) {
                task.sub_status = SubStatus::Conflict;
            }
            cmds.push(Command::PatchSubStatus { id, sub_status: SubStatus::Conflict });
        }

        if let Some(q) = &mut self.merge_queue {
            if q.current == Some(id) {
                q.current = None;
                q.failed = Some(id);
                let completed = q.completed;
                let total = q.task_ids.len();
                self.set_status(format!(
                    "Epic merge paused ({completed}/{total}): #{id} \u{2014} {error}"
                ));
                return cmds;
            }
        }

        self.set_status(error);
        cmds
    }

    // -----------------------------------------------------------------------
    // PR handlers
    // -----------------------------------------------------------------------

    fn handle_pr_created(&mut self, id: TaskId, pr_url: String) -> Vec<Command> {
        let in_queue = self.merge_queue.as_ref().is_some_and(|q| q.current == Some(id));

        let mut cmds = if let Some(task) = self.find_task_mut(id) {
            task.pr_url = Some(pr_url.clone());
            task.status = TaskStatus::Review;
            let task_clone = task.clone();
            if !in_queue {
                let pr_num = crate::models::pr_number_from_url(&pr_url);
                let label = pr_num.map_or("PR".to_string(), |n| format!("PR #{n}"));
                self.set_status(format!("{label} created: {pr_url}"));
            }
            vec![Command::PersistTask(task_clone)]
        } else {
            vec![]
        };

        if in_queue {
            if let Some(q) = &mut self.merge_queue {
                q.completed += 1;
                q.current = None;
            }
            cmds.extend(self.advance_merge_queue());
        }

        cmds
    }

    fn handle_pr_failed(&mut self, id: TaskId, error: String) -> Vec<Command> {
        if let Some(q) = &mut self.merge_queue {
            if q.current == Some(id) {
                q.current = None;
                q.failed = Some(id);
                let completed = q.completed;
                let total = q.task_ids.len();
                self.set_status(format!(
                    "Epic merge paused ({completed}/{total}): PR #{id} \u{2014} {error}"
                ));
                return vec![];
            }
        }

        self.set_status(error);
        vec![]
    }

    fn handle_pr_merged(&mut self, id: TaskId) -> Vec<Command> {
        let mut cmds = Vec::new();

        if let Some(task) = self.find_task_mut(id) {
            if task.status != TaskStatus::Review {
                return cmds;
            }

            let pr_label = task.pr_url.as_deref()
                .and_then(crate::models::pr_number_from_url)
                .map_or("PR".to_string(), |n| format!("PR #{n}"));
            let title = task.title.clone();

            // Detach: kill tmux window but preserve worktree
            if let Some(window) = task.tmux_window.take() {
                cmds.push(Command::KillTmuxWindow { window });
            }
            task.status = TaskStatus::Done;
            let task_clone = task.clone();

            self.clear_agent_tracking(id);
            self.clamp_selection();
            self.set_status(format!("{pr_label} merged \u{2014} task #{id} moved to Done"));

            cmds.push(Command::PersistTask(task_clone));

            if self.notifications_enabled {
                cmds.push(Command::SendNotification {
                    title: "PR merged".to_string(),
                    body: format!("{pr_label} merged: {title}"),
                    urgent: false,
                });
            }
        }

        cmds
    }

    fn handle_pr_review_state(&mut self, id: TaskId, review_decision: Option<dispatch::PrReviewDecision>) -> Vec<Command> {
        if let Some(task) = self.find_task_mut(id) {
            if task.status != TaskStatus::Review {
                return vec![];
            }
            let new_sub = match review_decision {
                Some(dispatch::PrReviewDecision::Approved) => SubStatus::Approved,
                Some(dispatch::PrReviewDecision::ChangesRequested) => SubStatus::ChangesRequested,
                _ => SubStatus::AwaitingReview,
            };
            if task.sub_status != new_sub {
                task.sub_status = new_sub;
                let task_clone = task.clone();
                return vec![Command::PersistTask(task_clone)];
            }
        }
        vec![]
    }

    // -----------------------------------------------------------------------
    // Wrap up handlers
    // -----------------------------------------------------------------------

    fn handle_start_wrap_up(&mut self, id: TaskId) -> Vec<Command> {
        let branch = match self.find_task(id) {
            Some(t) if dispatch::is_wrappable(t) => {
                match t.worktree.as_deref().and_then(dispatch::branch_from_worktree) {
                    Some(b) => b,
                    None => return vec![],
                }
            }
            _ => return vec![],
        };

        self.input.mode = InputMode::ConfirmWrapUp(id);
        self.set_status(format!(
            "Wrap up {}: (r) rebase onto main  (p) create PR  (Esc) cancel", branch
        ));
        vec![]
    }

    fn handle_wrap_up_rebase(&mut self) -> Vec<Command> {
        let id = match self.input.mode {
            InputMode::ConfirmWrapUp(id) => id,
            _ => return vec![],
        };
        self.input.mode = InputMode::Normal;
        self.set_status("Rebasing...".to_string());
        // Optimistically clear conflict substatus — FinishComplete will persist it.
        if let Some(task) = self.find_task_mut(id) {
            if task.sub_status == SubStatus::Conflict {
                task.sub_status = SubStatus::None;
            }
        }

        if let Some(task) = self.find_task(id) {
            let worktree = match &task.worktree {
                Some(wt) => wt.clone(),
                None => return vec![],
            };
            let branch = match dispatch::branch_from_worktree(&worktree) {
                Some(b) => b,
                None => return vec![],
            };
            vec![Command::Finish {
                id,
                repo_path: task.repo_path.clone(),
                branch,
                worktree,
                tmux_window: task.tmux_window.clone(),
            }]
        } else {
            vec![]
        }
    }

    fn handle_wrap_up_pr(&mut self) -> Vec<Command> {
        let id = match self.input.mode {
            InputMode::ConfirmWrapUp(id) => id,
            _ => return vec![],
        };
        self.input.mode = InputMode::Normal;
        self.set_status("Creating PR...".to_string());

        if let Some(task) = self.find_task(id) {
            let worktree = match &task.worktree {
                Some(wt) => wt.clone(),
                None => return vec![],
            };
            let branch = match dispatch::branch_from_worktree(&worktree) {
                Some(b) => b,
                None => return vec![],
            };
            vec![Command::CreatePr {
                id,
                repo_path: task.repo_path.clone(),
                branch,
                title: task.title.clone(),
                description: task.description.clone(),
            }]
        } else {
            vec![]
        }
    }

    fn handle_cancel_wrap_up(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.clear_status();
        vec![]
    }

    // -----------------------------------------------------------------------
    // Epic batch wrap-up handlers
    // -----------------------------------------------------------------------

    fn handle_start_epic_wrap_up(&mut self, epic_id: EpicId) -> Vec<Command> {
        let review_count = self.tasks.iter()
            .filter(|t| {
                t.epic_id == Some(epic_id)
                    && t.status == TaskStatus::Review
                    && t.worktree.is_some()
            })
            .count();

        if review_count == 0 {
            return self.update(Message::StatusInfo(
                "No review tasks to wrap up".to_string(),
            ));
        }

        self.input.mode = InputMode::ConfirmEpicWrapUp(epic_id);
        self.set_status(format!(
            "Wrap up {} review task{}: (r) rebase all  (p) PR all  (Esc) cancel",
            review_count,
            if review_count == 1 { "" } else { "s" },
        ));
        vec![]
    }

    fn handle_epic_wrap_up(&mut self, action: MergeAction) -> Vec<Command> {
        let epic_id = match self.input.mode {
            InputMode::ConfirmEpicWrapUp(id) => id,
            _ => return vec![],
        };
        self.input.mode = InputMode::Normal;

        let mut review_tasks: Vec<&Task> = self.tasks.iter()
            .filter(|t| {
                t.epic_id == Some(epic_id)
                    && t.status == TaskStatus::Review
                    && t.worktree.is_some()
            })
            .collect();
        review_tasks.sort_by_key(|t| t.sort_order.unwrap_or(t.id.0));

        let task_ids: Vec<TaskId> = review_tasks.iter().map(|t| t.id).collect();

        if task_ids.is_empty() {
            return vec![];
        }

        self.merge_queue = Some(MergeQueue {
            epic_id,
            action,
            task_ids,
            completed: 0,
            current: None,
            failed: None,
        });

        self.advance_merge_queue()
    }

    fn advance_merge_queue(&mut self) -> Vec<Command> {
        let (total, next_idx, action) = match &self.merge_queue {
            Some(q) => (q.task_ids.len(), q.completed, q.action.clone()),
            None => return vec![],
        };

        if next_idx >= total {
            self.merge_queue = None;
            self.set_status(format!("Epic merge complete: {total}/{total} done"));
            return vec![];
        }

        let next_id = self.merge_queue.as_ref().unwrap().task_ids[next_idx];

        // Validate the task is still eligible
        let task_data = match self.find_task(next_id) {
            Some(t) if t.status == TaskStatus::Review && t.worktree.is_some() => {
                let worktree = t.worktree.clone().unwrap();
                let branch = dispatch::branch_from_worktree(&worktree);
                let repo_path = t.repo_path.clone();
                let title = t.title.clone();
                let description = t.description.clone();
                let tmux_window = t.tmux_window.clone();
                branch.map(|b| (worktree, b, repo_path, title, description, tmux_window))
            }
            _ => None,
        };

        let Some((worktree, branch, repo_path, title, description, tmux_window)) = task_data else {
            // Skip this task — no longer eligible
            if let Some(q) = &mut self.merge_queue {
                q.completed += 1;
            }
            return self.advance_merge_queue();
        };

        if let Some(q) = &mut self.merge_queue {
            q.current = Some(next_id);
        }

        self.set_status(format!(
            "Epic merge: {next_idx}/{total} done \u{2014} processing #{}",
            next_id
        ));

        match action {
            MergeAction::Rebase => {
                vec![Command::Finish {
                    id: next_id,
                    repo_path,
                    branch,
                    worktree,
                    tmux_window,
                }]
            }
            MergeAction::Pr => vec![Command::CreatePr {
                id: next_id,
                repo_path,
                branch,
                title,
                description,
            }],
        }
    }

    fn handle_cancel_epic_wrap_up(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.clear_status();
        vec![]
    }

    fn handle_cancel_merge_queue(&mut self) -> Vec<Command> {
        self.merge_queue = None;
        self.set_status("Merge queue cancelled".to_string());
        vec![]
    }

    // -----------------------------------------------------------------------
    // Review board handlers
    // -----------------------------------------------------------------------

    fn handle_switch_to_review_board(&mut self) -> Vec<Command> {
        let saved_board = match &self.view_mode {
            ViewMode::Board(sel) => sel.clone(),
            ViewMode::Epic { saved_board, .. } => saved_board.clone(),
            ViewMode::ReviewBoard { saved_board, .. } => saved_board.clone(),
        };
        self.view_mode = ViewMode::ReviewBoard {
            selection: ReviewBoardSelection::new(),
            saved_board,
        };
        let needs_fetch = self.last_review_fetch
            .map(|t| t.elapsed() > Duration::from_secs(60))
            .unwrap_or(true);
        if needs_fetch && !self.review_board_loading {
            self.review_board_loading = true;
            vec![Command::FetchReviewPrs]
        } else {
            vec![]
        }
    }

    fn handle_switch_to_task_board(&mut self) -> Vec<Command> {
        if let ViewMode::ReviewBoard { saved_board, .. } = &self.view_mode {
            self.view_mode = ViewMode::Board(saved_board.clone());
        }
        vec![]
    }

    fn handle_review_prs_loaded(&mut self, prs: Vec<crate::models::ReviewPr>) -> Vec<Command> {
        let cmds = vec![Command::PersistReviewPrs(prs.clone())];
        self.review_prs = prs;
        self.review_board_loading = false;
        self.last_review_fetch = Some(Instant::now());
        self.clamp_review_selection();
        cmds
    }

    fn clamp_review_selection(&mut self) {
        let counts: [usize; 3] = std::array::from_fn(|col| {
            self.review_prs.iter()
                .filter(|pr| pr.review_decision.column_index() == col)
                .count()
        });
        if let Some(sel) = self.review_selection_mut() {
            for (col, &count) in counts.iter().enumerate() {
                if count == 0 {
                    sel.selected_row[col] = 0;
                } else if sel.selected_row[col] >= count {
                    sel.selected_row[col] = count - 1;
                }
            }
        }
    }

    fn handle_review_prs_fetch_failed(&mut self, error: String) -> Vec<Command> {
        self.review_board_loading = false;
        self.set_status(format!("Failed to fetch review PRs: {error}"));
        vec![]
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

    // -----------------------------------------------------------------------
    // Epic handlers
    // -----------------------------------------------------------------------

    fn handle_dispatch_epic(&mut self, id: EpicId) -> Vec<Command> {
        let Some(epic) = self.epics.iter().find(|e| e.id == id) else {
            return vec![];
        };
        let subtask_statuses = self.subtask_statuses(id);
        let status = crate::models::epic_status(epic, &subtask_statuses);

        if status != TaskStatus::Backlog {
            self.set_status("No backlog tasks in epic".to_string());
            return vec![];
        }

        if epic.plan.is_some() {
            // Epic has a plan — dispatch the next backlog subtask sorted by sort_order
            let mut backlog_subtasks: Vec<&Task> = self
                .tasks
                .iter()
                .filter(|t| t.epic_id == Some(id) && t.status == TaskStatus::Backlog)
                .collect();
            backlog_subtasks.sort_by_key(|t| (t.sort_order.unwrap_or(t.id.0), t.id.0));

            match backlog_subtasks.first() {
                Some(task) if task.plan.is_some() => {
                    vec![Command::Dispatch { task: (*task).clone() }]
                }
                Some(task) => {
                    match task.tag.as_deref() {
                        Some("epic") => vec![Command::Brainstorm { task: (*task).clone() }],
                        Some("feature") => vec![Command::Plan { task: (*task).clone() }],
                        _ => vec![Command::Dispatch { task: (*task).clone() }],
                    }
                }
                None => {
                    vec![Command::DispatchEpic { epic: epic.clone() }]
                }
            }
        } else {
            // No plan — spawn planning subtask
            vec![Command::DispatchEpic { epic: epic.clone() }]
        }
    }

    fn handle_enter_epic(&mut self, epic_id: EpicId) -> Vec<Command> {
        let saved_board = match &self.view_mode {
            ViewMode::Board(sel) => sel.clone(),
            ViewMode::Epic { saved_board, .. } => saved_board.clone(),
            ViewMode::ReviewBoard { saved_board, .. } => saved_board.clone(),
        };
        self.view_mode = ViewMode::Epic {
            epic_id,
            selection: BoardSelection::new(),
            saved_board,
        };
        self.detail_visible = false;
        vec![]
    }

    fn handle_exit_epic(&mut self) -> Vec<Command> {
        if let ViewMode::Epic { saved_board, .. } = &self.view_mode {
            self.view_mode = ViewMode::Board(saved_board.clone());
        }
        self.detail_visible = false;
        vec![]
    }

    fn handle_refresh_epics(&mut self, epics: Vec<Epic>) -> Vec<Command> {
        self.epics = epics;
        let valid_ids: HashSet<EpicId> = self.epics.iter().map(|e| e.id).collect();
        self.selected_epics.retain(|id| valid_ids.contains(id));
        vec![]
    }

    fn handle_refresh_usage(&mut self, usage: Vec<TaskUsage>) -> Vec<Command> {
        self.usage = usage.into_iter().map(|u| (u.task_id, u)).collect();
        vec![]
    }

    fn handle_epic_created(&mut self, epic: Epic) -> Vec<Command> {
        self.epics.push(epic);
        vec![]
    }

    fn handle_edit_epic(&mut self, id: EpicId) -> Vec<Command> {
        if let Some(epic) = self.epics.iter().find(|e| e.id == id) {
            vec![Command::EditEpicInEditor(epic.clone())]
        } else {
            vec![]
        }
    }

    fn handle_epic_edited(&mut self, epic: Epic) -> Vec<Command> {
        if let Some(e) = self.epics.iter_mut().find(|e| e.id == epic.id) {
            e.title = epic.title;
            e.description = epic.description;
            e.updated_at = chrono::Utc::now();
        }
        vec![]
    }

    fn handle_delete_epic(&mut self, id: EpicId) -> Vec<Command> {
        let mut cmds = Vec::new();
        // Clean up worktrees/tmux for subtasks before deleting
        let subtask_ids: Vec<TaskId> = self.tasks
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
        self.epics.retain(|e| e.id != id);
        self.tasks.retain(|t| t.epic_id != Some(id));
        // If we were viewing this epic, exit
        if matches!(&self.view_mode, ViewMode::Epic { epic_id, .. } if *epic_id == id) {
            self.handle_exit_epic();
        }
        self.clamp_selection();
        cmds.push(Command::DeleteEpic(id));
        cmds
    }

    fn handle_confirm_delete_epic(&mut self) -> Vec<Command> {
        if let Some(ColumnItem::Epic(epic)) = self.selected_column_item() {
            let title = truncate_title(&epic.title, 30);
            self.input.mode = InputMode::ConfirmDeleteEpic;
            self.set_status(format!("Delete epic {title} and subtasks? (y/n)"));
        }
        vec![]
    }

    fn handle_mark_epic_done(&mut self, id: EpicId) -> Vec<Command> {
        if let Some(epic) = self.epics.iter_mut().find(|e| e.id == id) {
            epic.done = true;
        }
        vec![Command::PersistEpic { id, done: Some(true), sort_order: None }]
    }

    fn handle_mark_epic_undone(&mut self, id: EpicId) -> Vec<Command> {
        if let Some(epic) = self.epics.iter_mut().find(|e| e.id == id) {
            epic.done = false;
        }
        vec![Command::PersistEpic { id, done: Some(false), sort_order: None }]
    }

    fn handle_confirm_epic_done(&mut self) -> Vec<Command> {
        let id = match self.input.mode {
            InputMode::ConfirmEpicDone(id) => id,
            _ => return vec![],
        };
        self.input.mode = InputMode::Normal;
        self.clear_status();
        self.handle_mark_epic_done(id)
    }

    fn handle_cancel_epic_done(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.clear_status();
        vec![]
    }

    fn handle_archive_epic(&mut self, id: EpicId) -> Vec<Command> {
        let mut cmds = Vec::new();
        let subtask_ids: Vec<TaskId> = self.tasks
            .iter()
            .filter(|t| t.epic_id == Some(id) && t.status != TaskStatus::Archived)
            .map(|t| t.id)
            .collect();
        for task_id in subtask_ids {
            cmds.extend(self.handle_archive_task(task_id));
        }
        self.epics.retain(|e| e.id != id);
        if matches!(&self.view_mode, ViewMode::Epic { epic_id, .. } if *epic_id == id) {
            self.handle_exit_epic();
        }
        self.clamp_selection();
        cmds.push(Command::DeleteEpic(id));
        cmds
    }

    fn handle_confirm_archive_epic(&mut self) -> Vec<Command> {
        if let Some(ColumnItem::Epic(epic)) = self.selected_column_item() {
            let id = epic.id;
            let not_done_count = self.subtask_statuses(id)
                .iter()
                .filter(|s| **s != TaskStatus::Done)
                .count();
            if not_done_count > 0 {
                let noun = if not_done_count == 1 { "subtask" } else { "subtasks" };
                self.set_status(format!(
                    "Cannot archive epic: {} {} not done", not_done_count, noun
                ));
                return vec![];
            }
            self.input.mode = InputMode::ConfirmArchiveEpic;
            self.set_status("Archive epic and all subtasks? (y/n)".to_string());
        }
        vec![]
    }

    fn handle_start_new_epic(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::InputEpicTitle;
        self.input.buffer.clear();
        self.input.epic_draft = None;
        self.set_status("Epic title: ".to_string());
        vec![]
    }

    fn handle_submit_epic_title(&mut self, value: String) -> Vec<Command> {
        self.input.buffer.clear();
        if value.is_empty() {
            self.input.mode = InputMode::Normal;
            self.clear_status();
        } else {
            self.input.epic_draft = Some(EpicDraft {
                title: value,
                description: String::new(),
                repo_path: String::new(),
            });
            self.input.mode = InputMode::InputEpicDescription;
            self.set_status("Epic description: ".to_string());
        }
        vec![]
    }

    fn handle_submit_epic_description(&mut self, value: String) -> Vec<Command> {
        self.input.buffer.clear();
        if let Some(ref mut draft) = self.input.epic_draft {
            draft.description = value;
        }
        self.input.mode = InputMode::InputEpicRepoPath;
        self.set_status("Epic repo path: ".to_string());
        vec![]
    }

    fn handle_submit_epic_repo_path(&mut self, value: String) -> Vec<Command> {
        self.input.buffer.clear();
        let repo_path = if value.is_empty() {
            if let Some(first) = self.repo_paths.first() {
                first.clone()
            } else {
                self.set_status("Repo path required".to_string());
                return vec![];
            }
        } else {
            value
        };

        self.finish_epic_creation(repo_path)
    }

    fn handle_start_repo_filter(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::RepoFilter;
        vec![]
    }

    fn handle_close_repo_filter(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.clamp_selection();
        let mut paths: Vec<_> = self.repo_filter.iter().cloned().collect();
        paths.sort();
        let value = paths.join("\n");
        vec![Command::PersistStringSetting {
            key: "repo_filter".to_string(),
            value,
        }]
    }

    fn handle_toggle_repo_filter(&mut self, path: String) -> Vec<Command> {
        if self.repo_filter.contains(&path) {
            self.repo_filter.remove(&path);
        } else {
            self.repo_filter.insert(path);
        }
        self.clamp_selection();
        vec![]
    }

    fn handle_toggle_all_repo_filter(&mut self) -> Vec<Command> {
        if self.repo_filter.len() == self.repo_paths.len() {
            self.repo_filter.clear();
        } else {
            self.repo_filter = self.repo_paths.iter().cloned().collect();
        }
        self.clamp_selection();
        vec![]
    }

    fn handle_start_save_preset(&mut self) -> Vec<Command> {
        self.input.buffer.clear();
        self.input.mode = InputMode::InputPresetName;
        vec![]
    }

    fn handle_save_filter_preset(&mut self, name: String) -> Vec<Command> {
        let name = name.trim().to_string();
        if name.is_empty() {
            self.input.mode = InputMode::RepoFilter;
            return vec![];
        }
        let repos: HashSet<String> = self.repo_filter.clone();
        // Update or insert in the presets list
        if let Some(existing) = self.filter_presets.iter_mut().find(|(n, _)| *n == name) {
            existing.1.clone_from(&repos);
        } else {
            self.filter_presets.push((name.clone(), repos));
            self.filter_presets.sort_by(|a, b| a.0.cmp(&b.0));
        }
        self.input.buffer.clear();
        self.input.mode = InputMode::RepoFilter;
        self.set_status(format!("Saved preset \"{name}\""));
        let mut paths: Vec<_> = self.repo_filter.iter().cloned().collect();
        paths.sort();
        vec![Command::PersistFilterPreset {
            name,
            repo_paths: paths.join("\n"),
        }]
    }

    fn handle_load_filter_preset(&mut self, name: String) -> Vec<Command> {
        if let Some((_, repos)) = self.filter_presets.iter().find(|(n, _)| *n == name) {
            // Intersect with known repo_paths to skip stale entries
            let known: HashSet<&String> = self.repo_paths.iter().collect();
            self.repo_filter = repos.iter().filter(|p| known.contains(p)).cloned().collect();
            self.clamp_selection();
            self.set_status(format!("Loaded preset \"{name}\""));
        }
        vec![]
    }

    fn handle_start_delete_preset(&mut self) -> Vec<Command> {
        if self.filter_presets.is_empty() {
            return vec![];
        }
        self.input.mode = InputMode::ConfirmDeletePreset;
        vec![]
    }

    fn handle_delete_filter_preset(&mut self, name: String) -> Vec<Command> {
        self.filter_presets.retain(|(n, _)| *n != name);
        self.input.mode = InputMode::RepoFilter;
        self.set_status(format!("Deleted preset \"{name}\""));
        vec![Command::DeleteFilterPreset(name)]
    }

    fn handle_cancel_preset_input(&mut self) -> Vec<Command> {
        self.input.buffer.clear();
        self.input.mode = InputMode::RepoFilter;
        vec![]
    }

    fn handle_filter_presets_loaded(&mut self, presets: Vec<(String, HashSet<String>)>) -> Vec<Command> {
        self.filter_presets = presets;
        vec![]
    }

    fn finish_epic_creation(&mut self, repo_path: String) -> Vec<Command> {
        let mut draft = self.input.epic_draft.take().unwrap_or_default();
        draft.repo_path = repo_path.clone();
        self.input.mode = InputMode::Normal;
        self.clear_status();
        vec![
            Command::InsertEpic(draft),
            Command::SaveRepoPath(repo_path),
        ]
    }
}

#[cfg(test)]
mod tests;
