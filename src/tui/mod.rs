pub mod input;
pub mod types;
pub mod ui;

pub use types::*;

use std::collections::HashMap;

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
}

impl App {
    pub fn new(tasks: Vec<Task>) -> Self {
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

    /// Return all tasks for a given status, ordered as they appear in self.tasks.
    pub fn tasks_by_status(&self, status: TaskStatus) -> Vec<&Task> {
        self.tasks.iter().filter(|t| t.status == status).collect()
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
            Message::Dispatched { id, worktree, tmux_window } =>
                self.handle_dispatched(id, worktree, tmux_window),
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

    fn handle_dispatched(&mut self, id: i64, worktree: String, tmux_window: String) -> Vec<Command> {
        if let Some(task) = self.find_task_mut(id) {
            task.worktree = Some(worktree);
            task.tmux_window = Some(tmux_window);
            task.status = TaskStatus::Running;
            let task_clone = task.clone();
            self.clamp_selection();
            vec![Command::PersistTask(task_clone)]
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
        self.tmux_outputs.insert(id, output);
        vec![]
    }

    fn handle_window_gone(&mut self, id: i64) -> Vec<Command> {
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
        // Return CaptureTmux commands for Running tasks + a RefreshFromDb command
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
        cmds.push(Command::RefreshFromDb);
        cmds
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
}

#[cfg(test)]
mod tests;
