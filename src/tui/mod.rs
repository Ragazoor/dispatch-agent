pub mod input;
pub mod types;
pub mod ui;

pub use types::*;

use std::time::{Duration, Instant};

use crate::models::{Task, TaskId, TaskStatus};

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

pub struct App {
    pub(in crate::tui) tasks: Vec<Task>,
    pub(in crate::tui) selected_column: usize,
    pub(in crate::tui) selected_row: [usize; TaskStatus::COLUMN_COUNT],
    pub(in crate::tui) detail_visible: bool,
    pub(in crate::tui) status_message: Option<String>,
    pub(in crate::tui) error_popup: Option<String>,
    pub(in crate::tui) repo_paths: Vec<String>,
    pub(in crate::tui) should_quit: bool,
    pub(in crate::tui) input: InputState,
    pub(in crate::tui) agents: AgentTracking,
    pub(in crate::tui) archive: ArchiveState,
}

impl App {
    pub fn new(tasks: Vec<Task>, inactivity_timeout: Duration) -> Self {
        App {
            tasks,
            selected_column: 0,
            selected_row: [0; TaskStatus::COLUMN_COUNT],
            detail_visible: false,
            status_message: None,
            error_popup: None,
            repo_paths: Vec::new(),
            should_quit: false,
            input: InputState::default(),
            agents: AgentTracking::new(inactivity_timeout),
            archive: ArchiveState::default(),
        }
    }

    // Read-only accessors for code outside the tui module
    pub fn tasks(&self) -> &[Task] { &self.tasks }
    pub fn should_quit(&self) -> bool { self.should_quit }
    pub fn selected_column(&self) -> usize { self.selected_column }
    pub fn selected_row(&self) -> &[usize; TaskStatus::COLUMN_COUNT] { &self.selected_row }
    pub fn mode(&self) -> &InputMode { &self.input.mode }
    pub fn input_buffer(&self) -> &str { &self.input.buffer }
    pub fn detail_visible(&self) -> bool { self.detail_visible }
    pub fn tmux_outputs(&self) -> &std::collections::HashMap<TaskId, String> { &self.agents.tmux_outputs }
    pub fn status_message(&self) -> Option<&str> { self.status_message.as_deref() }
    pub fn error_popup(&self) -> Option<&str> { self.error_popup.as_deref() }
    pub fn repo_paths(&self) -> &[String] { &self.repo_paths }
    pub fn task_draft(&self) -> Option<&TaskDraft> { self.input.task_draft.as_ref() }
    pub fn stale_tasks(&self) -> &std::collections::HashSet<TaskId> { &self.agents.stale_tasks }
    pub fn crashed_tasks(&self) -> &std::collections::HashSet<TaskId> { &self.agents.crashed_tasks }
    pub fn inactivity_timeout(&self) -> Duration { self.agents.inactivity_timeout }
    pub fn show_archived(&self) -> bool { self.archive.visible }
    pub fn selected_archive_row(&self) -> usize { self.archive.selected_row }

    /// Return all tasks for a given status, ordered as they appear in self.tasks.
    pub fn tasks_by_status(&self, status: TaskStatus) -> Vec<&Task> {
        self.tasks.iter().filter(|t| t.status == status).collect()
    }

    /// Return all archived tasks, ordered as they appear in self.tasks.
    pub fn archived_tasks(&self) -> Vec<&Task> {
        self.tasks.iter().filter(|t| t.status == TaskStatus::Archived).collect()
    }

    /// Return the currently selected task (in the focused column), if any.
    pub fn selected_task(&self) -> Option<&Task> {
        let status = TaskStatus::from_column_index(self.selected_column)?;
        let col_tasks = self.tasks_by_status(status);
        let row = self.selected_row[self.selected_column];
        col_tasks.get(row).copied()
    }

    /// Clamp all selected_row values to be within bounds for each column.
    pub fn clamp_selection(&mut self) {
        for col in 0..TaskStatus::COLUMN_COUNT {
            if let Some(status) = TaskStatus::from_column_index(col) {
                let count = self.tasks_by_status(status).len();
                if count == 0 {
                    self.selected_row[col] = 0;
                } else if self.selected_row[col] >= count {
                    self.selected_row[col] = count - 1;
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

    /// Process a message and return a list of side-effect commands.
    pub fn update(&mut self, msg: Message) -> Vec<Command> {
        match msg {
            Message::Tick => self.handle_tick(),
            Message::Quit => self.handle_quit(),
            Message::NavigateColumn(delta) => self.handle_navigate_column(delta),
            Message::NavigateRow(delta) => self.handle_navigate_row(delta),
            Message::MoveTask { id, direction } => self.handle_move_task(id, direction),
            Message::DispatchTask(id) => self.handle_dispatch_task(id),
            Message::BrainstormTask(id) => self.handle_brainstorm_task(id),
            Message::Dispatched { id, worktree, tmux_window, switch_focus } =>
                self.handle_dispatched(id, worktree, tmux_window, switch_focus),
            Message::TaskCreated { task } => self.handle_task_created(task),
            Message::DeleteTask(id) => self.handle_delete_task(id),
            Message::ToggleDetail => self.handle_toggle_detail(),
            Message::TmuxOutput { id, output } => self.handle_tmux_output(id, output),
            Message::WindowGone(id) => self.handle_window_gone(id),
            Message::RefreshTasks(tasks) => self.handle_refresh_tasks(tasks),
            Message::ResumeTask(id) => self.handle_resume_task(id),
            Message::Resumed { id, tmux_window } => self.handle_resumed(id, tmux_window),
            Message::Error(msg) => self.handle_error(msg),
            Message::TaskEdited(edit) =>
                self.handle_task_edited(edit),
            Message::RepoPathsUpdated(paths) => self.handle_repo_paths_updated(paths),
            Message::QuickDispatch { repo_path } => self.handle_quick_dispatch(repo_path),
            Message::StaleAgent(id) => self.handle_stale_agent(id),
            Message::AgentCrashed(id) => self.handle_agent_crashed(id),
            Message::KillAndRetry(id) => self.handle_kill_and_retry(id),
            Message::RetryResume(id) => self.handle_retry_resume(id),
            Message::RetryFresh(id) => self.handle_retry_fresh(id),
            Message::ArchiveTask(id) => self.handle_archive_task(id),
            Message::ToggleArchive => self.handle_toggle_archive(),
            Message::DismissError => self.handle_dismiss_error(),
            Message::StartNewTask => self.handle_start_new_task(),
            Message::CancelInput => self.handle_cancel_input(),
            Message::ConfirmDeleteStart => self.handle_confirm_delete_start(),
            Message::ConfirmDeleteYes => self.handle_confirm_delete_yes(),
            Message::CancelDelete => self.handle_cancel_delete(),
            Message::SubmitTitle(value) => self.handle_submit_title(value),
            Message::SubmitDescription(value) => self.handle_submit_description(value),
            Message::SubmitRepoPath(value) => self.handle_submit_repo_path(value),
            Message::InputChar(c) => self.handle_input_char(c),
            Message::InputBackspace => self.handle_input_backspace(),
            Message::StartQuickDispatchSelection => self.handle_start_quick_dispatch_selection(),
            Message::SelectQuickDispatchRepo(idx) => self.handle_select_quick_dispatch_repo(idx),
            Message::CancelRetry => self.handle_cancel_retry(),
            Message::StatusInfo(msg) => self.handle_status_info(msg),
        }
    }

    // -----------------------------------------------------------------------
    // Per-message handlers
    // -----------------------------------------------------------------------

    fn handle_quit(&mut self) -> Vec<Command> {
        self.should_quit = true;
        vec![]
    }

    fn handle_navigate_column(&mut self, delta: isize) -> Vec<Command> {
        let new_col = (self.selected_column as isize + delta)
            .clamp(0, (TaskStatus::COLUMN_COUNT - 1) as isize) as usize;
        self.selected_column = new_col;
        self.clamp_selection();
        vec![]
    }

    fn handle_navigate_row(&mut self, delta: isize) -> Vec<Command> {
        let col = self.selected_column;
        if let Some(status) = TaskStatus::from_column_index(col) {
            let count = self.tasks_by_status(status).len();
            if count > 0 {
                let new_row = (self.selected_row[col] as isize + delta)
                    .clamp(0, count as isize - 1) as usize;
                self.selected_row[col] = new_row;
            }
        }
        vec![]
    }

    fn handle_move_task(&mut self, id: TaskId, direction: MoveDirection) -> Vec<Command> {
        if let Some(task) = self.find_task_mut(id) {
            let new_status = match direction {
                MoveDirection::Forward => task.status.next(),
                MoveDirection::Backward => task.status.prev(),
            };
            if new_status == task.status {
                // No movement possible (at boundary)
                return vec![];
            }

            // Clean up worktree/tmux when moving backward from a dispatched state,
            // or when moving forward to Done.
            let needs_cleanup = matches!(direction, MoveDirection::Backward)
                || new_status == TaskStatus::Done;
            let cleanup = if needs_cleanup {
                match task.worktree.take() {
                    Some(wt) => Some(Command::Cleanup {
                        repo_path: task.repo_path.clone(),
                        worktree: wt,
                        tmux_window: task.tmux_window.take(),
                    }),
                    None => {
                        task.tmux_window.take(); // clear even if no worktree
                        None
                    },
                }
            } else {
                None
            };

            task.status = new_status;
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

    fn handle_dispatch_task(&mut self, id: TaskId) -> Vec<Command> {
        if let Some(task) = self.find_task(id) {
            if task.status == TaskStatus::Ready {
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

    fn handle_dispatched(&mut self, id: TaskId, worktree: String, tmux_window: String, switch_focus: bool) -> Vec<Command> {
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

    fn handle_task_created(&mut self, task: Task) -> Vec<Command> {
        self.tasks.push(task);
        self.clamp_selection();
        vec![]
    }

    fn handle_delete_task(&mut self, id: TaskId) -> Vec<Command> {
        let cleanup = self.find_task_mut(id).and_then(|task| {
            let wt = task.worktree.take()?;
            Some(Command::Cleanup {
                repo_path: task.repo_path.clone(),
                worktree: wt,
                tmux_window: task.tmux_window.take(),
            })
        });
        self.clear_agent_tracking(id);
        self.tasks.retain(|t| t.id != id);
        self.clamp_selection();
        let archive_count = self.archived_tasks().len();
        if self.archive.selected_row >= archive_count && archive_count > 0 {
            self.archive.selected_row = archive_count - 1;
        }
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

    fn handle_tmux_output(&mut self, id: TaskId, output: String) -> Vec<Command> {
        let changed = self.agents.tmux_outputs.get(&id) != Some(&output);
        if changed {
            self.agents.last_output_change.insert(id, Instant::now());
            // If task was previously stale but output changed, clear stale flag
            self.agents.stale_tasks.remove(&id);
        }
        self.agents.tmux_outputs.insert(id, output);
        vec![]
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
        // Merge DB state into in-memory state, preserving tmux_outputs
        self.tasks = new_tasks;
        self.clamp_selection();
        vec![]
    }

    fn handle_tick(&mut self) -> Vec<Command> {
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
            .filter(|t| !self.agents.stale_tasks.contains(&t.id))
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

        cmds.push(Command::RefreshFromDb);
        cmds
    }

    fn handle_stale_agent(&mut self, id: TaskId) -> Vec<Command> {
        self.agents.stale_tasks.insert(id);
        if let Some(task) = self.find_task(id) {
            let elapsed = self.agents.last_output_change
                .get(&id)
                .map(|t| t.elapsed().as_secs() / 60)
                .unwrap_or(0);
            self.status_message = Some(format!(
                "Task {} inactive for {}m - press d to retry",
                task.id, elapsed
            ));
        }
        vec![]
    }

    fn handle_agent_crashed(&mut self, id: TaskId) -> Vec<Command> {
        self.agents.crashed_tasks.insert(id);
        if let Some(task) = self.find_task(id) {
            self.status_message = Some(format!(
                "Task {} agent crashed - press d to retry", task.id
            ));
        }
        vec![]
    }

    fn handle_resume_task(&mut self, id: TaskId) -> Vec<Command> {
        if let Some(task) = self.find_task(id) {
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
            t.updated_at = chrono::Utc::now();
        }
        self.clamp_selection();
        vec![]
    }

    fn handle_repo_paths_updated(&mut self, paths: Vec<String>) -> Vec<Command> {
        self.repo_paths = paths;
        vec![]
    }

    fn handle_quick_dispatch(&mut self, repo_path: String) -> Vec<Command> {
        vec![Command::QuickDispatch(TaskDraft {
            title: "Quick task".to_string(),
            description: String::new(),
            repo_path,
        })]
    }

    fn handle_kill_and_retry(&mut self, id: TaskId) -> Vec<Command> {
        self.input.mode = InputMode::ConfirmRetry(id);
        let label = if self.agents.crashed_tasks.contains(&id) {
            "crashed"
        } else {
            "stale"
        };
        self.status_message = Some(format!(
            "Agent {} - [r] Resume  [f] Fresh start  [Esc] Cancel", label
        ));
        vec![]
    }

    fn handle_retry_resume(&mut self, id: TaskId) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.status_message = None;
        self.clear_agent_tracking(id);

        if let Some(task) = self.find_task_mut(id) {
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
        self.status_message = None;
        self.clear_agent_tracking(id);

        if let Some(task) = self.find_task_mut(id) {
            let worktree = task.worktree.take();
            let tmux_window = task.tmux_window.take();
            task.status = TaskStatus::Ready;
            let task_clone = task.clone();

            let mut cmds = Vec::new();
            if let Some(wt) = worktree {
                cmds.push(Command::Cleanup {
                    repo_path: task_clone.repo_path.clone(),
                    worktree: wt,
                    tmux_window,
                });
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
            let cleanup = match task.worktree.take() {
                Some(wt) => Some(Command::Cleanup {
                    repo_path: task.repo_path.clone(),
                    worktree: wt,
                    tmux_window: task.tmux_window.take(),
                }),
                None => {
                    task.tmux_window.take();
                    None
                }
            };
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
        }
        vec![]
    }

    fn handle_dismiss_error(&mut self) -> Vec<Command> {
        self.error_popup = None;
        vec![]
    }

    fn handle_start_new_task(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::InputTitle;
        self.input.buffer.clear();
        self.input.task_draft = None;
        self.status_message = Some("Enter title: ".to_string());
        vec![]
    }

    fn handle_cancel_input(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.input.buffer.clear();
        self.input.task_draft = None;
        self.status_message = None;
        vec![]
    }

    fn handle_confirm_delete_start(&mut self) -> Vec<Command> {
        if self.selected_task().is_some() {
            self.input.mode = InputMode::ConfirmDelete;
            self.status_message = Some("Delete task? (y/n)".to_string());
        }
        vec![]
    }

    fn handle_confirm_delete_yes(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.status_message = None;
        if let Some(task) = self.selected_task() {
            let id = task.id;
            self.update(Message::DeleteTask(id))
        } else {
            vec![]
        }
    }

    fn handle_cancel_delete(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.status_message = None;
        vec![]
    }

    fn handle_submit_title(&mut self, value: String) -> Vec<Command> {
        self.input.buffer.clear();
        if value.is_empty() {
            self.input.mode = InputMode::Normal;
            self.input.task_draft = None;
            self.status_message = None;
        } else {
            self.input.task_draft = Some(TaskDraft {
                title: value,
                description: String::new(),
                repo_path: String::new(),
            });
            self.input.mode = InputMode::InputDescription;
            self.status_message = Some("Enter description: ".to_string());
        }
        vec![]
    }

    fn handle_submit_description(&mut self, value: String) -> Vec<Command> {
        self.input.buffer.clear();
        if let Some(ref mut draft) = self.input.task_draft {
            draft.description = value;
        }
        self.input.mode = InputMode::InputRepoPath;
        self.status_message = Some("Enter repo path: ".to_string());
        vec![]
    }

    fn handle_submit_repo_path(&mut self, value: String) -> Vec<Command> {
        self.input.buffer.clear();
        let repo_path = if value.is_empty() {
            if let Some(first) = self.repo_paths.first() {
                first.clone()
            } else {
                self.status_message = Some("Repo path required (no saved paths available)".to_string());
                return vec![];
            }
        } else {
            value
        };
        self.finish_task_creation(repo_path)
    }

    fn handle_input_char(&mut self, c: char) -> Vec<Command> {
        // In repo path mode with empty buffer, 1-9 selects a saved path
        if self.input.mode == InputMode::InputRepoPath
            && self.input.buffer.is_empty()
            && c.is_ascii_digit()
            && c != '0'
        {
            let idx = (c as usize) - ('1' as usize);
            if idx < self.repo_paths.len() {
                let repo_path = self.repo_paths[idx].clone();
                return self.finish_task_creation(repo_path);
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
        self.status_message = Some("Select repo path (1-9) or Esc to cancel".to_string());
        vec![]
    }

    fn handle_select_quick_dispatch_repo(&mut self, idx: usize) -> Vec<Command> {
        if idx < self.repo_paths.len() {
            let repo_path = self.repo_paths[idx].clone();
            self.input.mode = InputMode::Normal;
            self.status_message = None;
            self.update(Message::QuickDispatch { repo_path })
        } else {
            vec![]
        }
    }

    fn handle_cancel_retry(&mut self) -> Vec<Command> {
        self.input.mode = InputMode::Normal;
        self.status_message = None;
        vec![]
    }

    fn handle_status_info(&mut self, msg: String) -> Vec<Command> {
        self.status_message = Some(msg);
        vec![]
    }

    fn finish_task_creation(&mut self, repo_path: String) -> Vec<Command> {
        let mut draft = self.input.task_draft.take().unwrap_or_default();
        draft.repo_path = repo_path.clone();
        self.input.mode = InputMode::Normal;
        self.status_message = None;
        vec![
            Command::InsertTask(draft),
            Command::SaveRepoPath(repo_path),
        ]
    }
}

#[cfg(test)]
mod tests;
