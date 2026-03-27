pub mod input;
pub mod types;
pub mod ui;

pub use types::*;

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::models::{Task, TaskStatus};

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

pub struct App {
    pub(in crate::tui) tasks: Vec<Task>,
    pub(in crate::tui) selected_column: usize,
    pub(in crate::tui) selected_row: [usize; TaskStatus::COLUMN_COUNT],
    pub(in crate::tui) mode: InputMode,
    pub(in crate::tui) input_buffer: String,
    pub(in crate::tui) task_draft: Option<TaskDraft>,
    pub(in crate::tui) detail_visible: bool,
    pub(in crate::tui) tmux_outputs: HashMap<i64, String>,
    pub(in crate::tui) status_message: Option<String>,
    pub(in crate::tui) error_popup: Option<String>,
    pub(in crate::tui) repo_paths: Vec<String>,
    pub(in crate::tui) should_quit: bool,
    pub(in crate::tui) last_output_change: HashMap<i64, Instant>,
    pub(in crate::tui) stale_tasks: HashSet<i64>,
    pub(in crate::tui) crashed_tasks: HashSet<i64>,
    pub(in crate::tui) inactivity_timeout: Duration,
    pub(in crate::tui) show_archived: bool,
    pub(in crate::tui) selected_archive_row: usize,
}

impl App {
    pub fn new(tasks: Vec<Task>, inactivity_timeout: Duration) -> Self {
        App {
            tasks,
            selected_column: 0,
            selected_row: [0; TaskStatus::COLUMN_COUNT],
            mode: InputMode::Normal,
            input_buffer: String::new(),
            task_draft: None,
            detail_visible: false,
            tmux_outputs: HashMap::new(),
            status_message: None,
            error_popup: None,
            repo_paths: Vec::new(),
            should_quit: false,
            last_output_change: HashMap::new(),
            stale_tasks: HashSet::new(),
            crashed_tasks: HashSet::new(),
            inactivity_timeout,
            show_archived: false,
            selected_archive_row: 0,
        }
    }

    // Read-only accessors for code outside the tui module
    pub fn tasks(&self) -> &[Task] { &self.tasks }
    pub fn should_quit(&self) -> bool { self.should_quit }
    pub fn selected_column(&self) -> usize { self.selected_column }
    pub fn selected_row(&self) -> &[usize; TaskStatus::COLUMN_COUNT] { &self.selected_row }
    pub fn mode(&self) -> &InputMode { &self.mode }
    pub fn input_buffer(&self) -> &str { &self.input_buffer }
    pub fn detail_visible(&self) -> bool { self.detail_visible }
    pub fn tmux_outputs(&self) -> &HashMap<i64, String> { &self.tmux_outputs }
    pub fn status_message(&self) -> Option<&str> { self.status_message.as_deref() }
    pub fn error_popup(&self) -> Option<&str> { self.error_popup.as_deref() }
    pub fn repo_paths(&self) -> &[String] { &self.repo_paths }
    pub fn task_draft(&self) -> Option<&TaskDraft> { self.task_draft.as_ref() }
    pub fn stale_tasks(&self) -> &HashSet<i64> { &self.stale_tasks }
    pub fn crashed_tasks(&self) -> &HashSet<i64> { &self.crashed_tasks }
    pub fn inactivity_timeout(&self) -> Duration { self.inactivity_timeout }
    pub fn show_archived(&self) -> bool { self.show_archived }
    pub fn selected_archive_row(&self) -> usize { self.selected_archive_row }

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

    fn find_task(&self, id: i64) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == id)
    }

    fn find_task_mut(&mut self, id: i64) -> Option<&mut Task> {
        self.tasks.iter_mut().find(|t| t.id == id)
    }

    /// Remove all in-memory agent tracking state for a task.
    fn clear_agent_tracking(&mut self, id: i64) {
        self.last_output_change.remove(&id);
        self.stale_tasks.remove(&id);
        self.crashed_tasks.remove(&id);
        self.tmux_outputs.remove(&id);
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
            Message::TaskEdited { id, title, description, repo_path, status, plan } =>
                self.handle_task_edited(id, title, description, repo_path, status, plan),
            Message::RepoPathsUpdated(paths) => self.handle_repo_paths_updated(paths),
            Message::QuickDispatch { repo_path } => self.handle_quick_dispatch(repo_path),
            Message::StaleAgent(id) => self.handle_stale_agent(id),
            Message::AgentCrashed(id) => self.handle_agent_crashed(id),
            Message::KillAndRetry(id) => self.handle_kill_and_retry(id),
            Message::RetryResume(id) => self.handle_retry_resume(id),
            Message::RetryFresh(id) => self.handle_retry_fresh(id),
            Message::ArchiveTask(id) => self.handle_archive_task(id),
            Message::ToggleArchive => self.handle_toggle_archive(),
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

    fn handle_move_task(&mut self, id: i64, direction: MoveDirection) -> Vec<Command> {
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

    fn handle_dispatch_task(&mut self, id: i64) -> Vec<Command> {
        if let Some(task) = self.find_task(id) {
            if task.status == TaskStatus::Ready {
                return vec![Command::Dispatch { task: task.clone() }];
            }
        }
        vec![]
    }

    fn handle_brainstorm_task(&mut self, id: i64) -> Vec<Command> {
        if let Some(task) = self.find_task(id) {
            if task.status == TaskStatus::Backlog {
                return vec![Command::Brainstorm { task: task.clone() }];
            }
        }
        vec![]
    }

    fn handle_dispatched(&mut self, id: i64, worktree: String, tmux_window: String, switch_focus: bool) -> Vec<Command> {
        if let Some(task) = self.find_task_mut(id) {
            task.worktree = Some(worktree);
            task.tmux_window = Some(tmux_window.clone());
            task.status = TaskStatus::Running;
            let task_clone = task.clone();
            self.last_output_change.insert(id, Instant::now());
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

    fn handle_delete_task(&mut self, id: i64) -> Vec<Command> {
        self.tasks.retain(|t| t.id != id);
        self.clamp_selection();
        vec![Command::DeleteTask(id)]
    }

    fn handle_toggle_detail(&mut self) -> Vec<Command> {
        self.detail_visible = !self.detail_visible;
        vec![]
    }

    fn handle_tmux_output(&mut self, id: i64, output: String) -> Vec<Command> {
        let changed = self.tmux_outputs.get(&id) != Some(&output);
        if changed {
            self.last_output_change.insert(id, Instant::now());
            // If task was previously stale but output changed, clear stale flag
            self.stale_tasks.remove(&id);
        }
        self.tmux_outputs.insert(id, output);
        vec![]
    }

    fn handle_window_gone(&mut self, id: i64) -> Vec<Command> {
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
        let timeout = self.inactivity_timeout;
        let newly_stale: Vec<i64> = self
            .tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Running && t.tmux_window.is_some())
            .filter(|t| !self.stale_tasks.contains(&t.id))
            .filter(|t| {
                self.last_output_change
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

    fn handle_stale_agent(&mut self, id: i64) -> Vec<Command> {
        self.stale_tasks.insert(id);
        if let Some(task) = self.find_task(id) {
            let elapsed = self.last_output_change
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

    fn handle_agent_crashed(&mut self, id: i64) -> Vec<Command> {
        self.crashed_tasks.insert(id);
        if let Some(task) = self.find_task(id) {
            self.status_message = Some(format!(
                "Task {} agent crashed - press d to retry", task.id
            ));
        }
        vec![]
    }

    fn handle_resume_task(&mut self, id: i64) -> Vec<Command> {
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

    fn handle_resumed(&mut self, id: i64, tmux_window: String) -> Vec<Command> {
        if let Some(task) = self.find_task_mut(id) {
            task.tmux_window = Some(tmux_window);
            task.status = TaskStatus::Running;
            let task_clone = task.clone();
            self.last_output_change.insert(id, Instant::now());
            self.stale_tasks.remove(&id);
            self.crashed_tasks.remove(&id);
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

    fn handle_task_edited(&mut self, id: i64, title: String, description: String, repo_path: String, status: TaskStatus, plan: Option<String>) -> Vec<Command> {
        if let Some(t) = self.find_task_mut(id) {
            t.title = title;
            t.description = description;
            t.repo_path = repo_path;
            t.status = status;
            t.plan = plan;
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
        vec![Command::QuickDispatch {
            title: "Quick task".to_string(),
            description: String::new(),
            repo_path,
        }]
    }

    fn handle_kill_and_retry(&mut self, id: i64) -> Vec<Command> {
        self.mode = InputMode::ConfirmRetry(id);
        let label = if self.crashed_tasks.contains(&id) {
            "crashed"
        } else {
            "stale"
        };
        self.status_message = Some(format!(
            "Agent {} - [r] Resume  [f] Fresh start  [Esc] Cancel", label
        ));
        vec![]
    }

    fn handle_retry_resume(&mut self, id: i64) -> Vec<Command> {
        self.mode = InputMode::Normal;
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

    fn handle_retry_fresh(&mut self, id: i64) -> Vec<Command> {
        self.mode = InputMode::Normal;
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

    fn handle_archive_task(&mut self, id: i64) -> Vec<Command> {
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
        self.show_archived = !self.show_archived;
        if self.show_archived {
            self.selected_archive_row = 0;
        }
        vec![]
    }
}

#[cfg(test)]
mod tests;
