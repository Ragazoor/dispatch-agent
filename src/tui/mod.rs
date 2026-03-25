pub mod input;
pub mod types;
pub mod ui;

pub use types::*;

use std::collections::HashMap;

use crate::models::{Note, Task, TaskStatus};

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

pub struct App {
    pub tasks: Vec<Task>,
    pub selected_column: usize,
    pub selected_row: [usize; 5],
    pub mode: InputMode,
    pub input_buffer: String,
    pub detail_visible: bool,
    pub tmux_outputs: HashMap<i64, String>,
    pub notes: HashMap<i64, Vec<Note>>,
    pub status_message: Option<String>,
    pub error_popup: Option<String>,
    pub repo_paths: Vec<String>,
    pub should_quit: bool,
}

impl App {
    pub fn new(tasks: Vec<Task>) -> Self {
        App {
            tasks,
            selected_column: 0,
            selected_row: [0; 5],
            mode: InputMode::Normal,
            input_buffer: String::new(),
            detail_visible: false,
            tmux_outputs: HashMap::new(),
            notes: HashMap::new(),
            status_message: None,
            error_popup: None,
            repo_paths: Vec::new(),
            should_quit: false,
        }
    }

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
        for col in 0..5 {
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

    /// Process a message and return a list of side-effect commands.
    pub fn update(&mut self, msg: Message) -> Vec<Command> {
        match msg {
            Message::Quit => {
                self.should_quit = true;
                vec![]
            }

            Message::NavigateColumn(delta) => {
                let new_col = (self.selected_column as isize + delta)
                    .clamp(0, 4) as usize;
                self.selected_column = new_col;
                self.clamp_selection();
                vec![]
            }

            Message::NavigateRow(delta) => {
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

            Message::MoveTask { id, direction } => {
                if let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) {
                    let new_status = match direction {
                        MoveDirection::Forward => task.status.next(),
                        MoveDirection::Backward => task.status.prev(),
                    };
                    if new_status == task.status {
                        // No movement possible (at boundary)
                        return vec![];
                    }

                    // Clean up worktree/tmux when moving backward from a dispatched state
                    let cleanup = if matches!(direction, MoveDirection::Backward) {
                        match (task.worktree.take(), task.tmux_window.take()) {
                            (Some(wt), Some(tw)) => Some(Command::Cleanup {
                                repo_path: task.repo_path.clone(),
                                worktree: wt,
                                tmux_window: tw,
                            }),
                            _ => None,
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

            Message::DispatchTask(id) => {
                if let Some(task) = self.tasks.iter().find(|t| t.id == id) {
                    match task.status {
                        TaskStatus::Ready | TaskStatus::Running | TaskStatus::Review => {
                            return vec![Command::Dispatch { task: task.clone() }];
                        }
                        _ => {
                            self.status_message = Some(
                                "Move task to Ready before dispatching (press m)".to_string(),
                            );
                        }
                    }
                }
                vec![]
            }

            Message::Dispatched { id, worktree, tmux_window } => {
                if let Some(task) = self.tasks.iter_mut().find(|t| t.id == id) {
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

            Message::CreateTask { title, description, repo_path } => {
                let now = chrono::Utc::now();
                let save_path = Command::SaveRepoPath(repo_path.clone());
                let task = Task {
                    id: 0,
                    title,
                    description,
                    repo_path,
                    status: TaskStatus::Backlog,
                    worktree: None,
                    tmux_window: None,
                    created_at: now,
                    updated_at: now,
                };
                let task_clone = task.clone();
                self.tasks.push(task);
                self.clamp_selection();
                vec![Command::PersistTask(task_clone), save_path]
            }

            Message::DeleteTask(id) => {
                self.tasks.retain(|t| t.id != id);
                self.clamp_selection();
                vec![Command::DeleteTask(id)]
            }

            Message::ToggleDetail => {
                self.detail_visible = !self.detail_visible;
                vec![]
            }

            Message::TmuxOutput { id, output } => {
                self.tmux_outputs.insert(id, output);
                vec![]
            }

            Message::WindowGone(id) => {
                // Only auto-advance if the task is still Running
                if let Some(task) = self.tasks.iter().find(|t| t.id == id) {
                    if task.status == TaskStatus::Running {
                        return self.update(Message::MoveTask {
                            id,
                            direction: MoveDirection::Forward,
                        });
                    }
                }
                vec![]
            }

            Message::NotesLoaded { task_id, notes } => {
                self.notes.insert(task_id, notes);
                vec![]
            }

            Message::RefreshTasks(new_tasks) => {
                // Merge DB state into in-memory state, preserving tmux_outputs
                self.tasks = new_tasks;
                self.clamp_selection();
                vec![]
            }

            Message::Tick => {
                // Return CaptureTmux commands for Running tasks + a RefreshFromDb command
                let mut cmds: Vec<Command> = self
                    .tasks
                    .iter()
                    .filter(|t| t.status == TaskStatus::Running)
                    .filter_map(|t| {
                        t.tmux_window.clone().map(|window| Command::CaptureTmux {
                            id: t.id,
                            window,
                        })
                    })
                    .collect();
                cmds.push(Command::RefreshFromDb);
                if self.detail_visible {
                    if let Some(task) = self.selected_task() {
                        cmds.push(Command::LoadNotes(task.id));
                    }
                }
                cmds
            }

            Message::Error(msg) => {
                self.error_popup = Some(msg);
                vec![]
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::TaskStatus;

    fn make_task(id: i64, status: TaskStatus) -> Task {
        let now = chrono::Utc::now();
        Task {
            id,
            title: format!("Task {id}"),
            description: String::new(),
            repo_path: String::from("/repo"),
            status,
            worktree: None,
            tmux_window: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn make_app() -> App {
        App::new(vec![
            make_task(1, TaskStatus::Backlog),
            make_task(2, TaskStatus::Backlog),
            make_task(3, TaskStatus::Ready),
            make_task(4, TaskStatus::Running),
            make_task(5, TaskStatus::Done),
        ])
    }

    #[test]
    fn tasks_by_status_filters() {
        let app = make_app();
        let backlog = app.tasks_by_status(TaskStatus::Backlog);
        assert_eq!(backlog.len(), 2);
        assert_eq!(backlog[0].id, 1);
        assert_eq!(backlog[1].id, 2);

        let ready = app.tasks_by_status(TaskStatus::Ready);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, 3);

        let review = app.tasks_by_status(TaskStatus::Review);
        assert_eq!(review.len(), 0);
    }

    #[test]
    fn move_task_forward() {
        let mut app = make_app();
        // Task 1 is in Backlog; move it forward -> Ready
        let cmds = app.update(Message::MoveTask {
            id: 1,
            direction: MoveDirection::Forward,
        });
        assert_eq!(app.tasks.iter().find(|t| t.id == 1).unwrap().status, TaskStatus::Ready);
        // Should produce a PersistTask command
        assert!(matches!(cmds[0], Command::PersistTask(_)));
    }

    #[test]
    fn move_task_backward_at_start_is_noop() {
        let mut app = make_app();
        // Task 1 is in Backlog; prev() stays Backlog
        let cmds = app.update(Message::MoveTask {
            id: 1,
            direction: MoveDirection::Backward,
        });
        assert_eq!(app.tasks.iter().find(|t| t.id == 1).unwrap().status, TaskStatus::Backlog);
        assert!(cmds.is_empty());
    }

    #[test]
    fn dispatch_only_ready_tasks() {
        let mut app = make_app();

        // Task 3 is Ready — should dispatch
        let cmds = app.update(Message::DispatchTask(3));
        assert!(matches!(cmds[0], Command::Dispatch { .. }));

        // Task 1 is Backlog — should not dispatch
        let cmds = app.update(Message::DispatchTask(1));
        assert!(cmds.is_empty());

        // Task 5 is Done — should not dispatch
        let cmds = app.update(Message::DispatchTask(5));
        assert!(cmds.is_empty());
    }

    #[test]
    fn quit_sets_flag() {
        let mut app = make_app();
        assert!(!app.should_quit);
        app.update(Message::Quit);
        assert!(app.should_quit);
    }

    #[test]
    fn navigate_column_clamps() {
        let mut app = make_app();
        app.selected_column = 0;
        app.update(Message::NavigateColumn(-1));
        assert_eq!(app.selected_column, 0); // can't go below 0

        app.selected_column = 4;
        app.update(Message::NavigateColumn(1));
        assert_eq!(app.selected_column, 4); // can't go above 4
    }

    #[test]
    fn navigate_row_clamps() {
        let mut app = make_app();
        // Backlog has 2 tasks (id 1, 2). Selected row starts at 0.
        app.selected_column = 0;
        app.update(Message::NavigateRow(-1));
        assert_eq!(app.selected_row[0], 0); // can't go below 0

        app.update(Message::NavigateRow(10));
        assert_eq!(app.selected_row[0], 1); // clamps to last item index
    }

    #[test]
    fn tick_produces_capture_for_running_tasks_with_window() {
        let mut task4 = make_task(4, TaskStatus::Running);
        task4.tmux_window = Some("main:task-4".to_string());
        let mut app = App::new(vec![task4]);
        let cmds = app.update(Message::Tick);
        // Should have CaptureTmux + RefreshFromDb
        assert_eq!(cmds.len(), 2);
        assert!(matches!(&cmds[0], Command::CaptureTmux { id: 4, window } if window == "main:task-4"));
        assert!(matches!(&cmds[1], Command::RefreshFromDb));
    }

    #[test]
    fn create_task_adds_to_backlog_and_persists() {
        let mut app = App::new(vec![]);
        let cmds = app.update(Message::CreateTask {
            title: "New Task".to_string(),
            description: "desc".to_string(),
            repo_path: "/repo".to_string(),
        });
        assert_eq!(app.tasks.len(), 1);
        assert_eq!(app.tasks[0].status, TaskStatus::Backlog);
        assert!(matches!(cmds[0], Command::PersistTask(_)));
    }

    #[test]
    fn delete_task_removes_and_returns_command() {
        let mut app = make_app();
        let cmds = app.update(Message::DeleteTask(1));
        assert!(app.tasks.iter().all(|t| t.id != 1));
        assert!(matches!(cmds[0], Command::DeleteTask(1)));
    }

    #[test]
    fn error_sets_error_popup() {
        let mut app = App::new(vec![]);
        app.update(Message::Error("Something went wrong".to_string()));
        assert_eq!(app.error_popup.as_deref(), Some("Something went wrong"));
    }

    #[test]
    fn dispatch_from_running_redispatches() {
        let mut task = make_task(4, TaskStatus::Running);
        task.worktree = Some("/repo/.worktrees/4-task-4".to_string());
        task.tmux_window = Some("task-4".to_string());
        let mut app = App::new(vec![task]);
        let cmds = app.update(Message::DispatchTask(4));
        assert!(matches!(cmds[0], Command::Dispatch { .. }));
    }

    #[test]
    fn dispatch_from_review_redispatches() {
        let mut task = make_task(5, TaskStatus::Review);
        task.worktree = Some("/repo/.worktrees/5-task-5".to_string());
        task.tmux_window = Some("task-5".to_string());
        let mut app = App::new(vec![task]);
        let cmds = app.update(Message::DispatchTask(5));
        assert!(matches!(cmds[0], Command::Dispatch { .. }));
    }

    #[test]
    fn move_backward_from_running_emits_cleanup() {
        let mut task = make_task(4, TaskStatus::Running);
        task.worktree = Some("/repo/.worktrees/4-task-4".to_string());
        task.tmux_window = Some("task-4".to_string());
        let mut app = App::new(vec![task]);

        let cmds = app.update(Message::MoveTask {
            id: 4,
            direction: MoveDirection::Backward,
        });

        // Should emit Cleanup then PersistTask
        assert_eq!(cmds.len(), 2);
        assert!(matches!(&cmds[0], Command::Cleanup { .. }));
        assert!(matches!(&cmds[1], Command::PersistTask(_)));

        // In-memory task should have cleared dispatch fields
        let task = app.tasks.iter().find(|t| t.id == 4).unwrap();
        assert_eq!(task.status, TaskStatus::Ready);
        assert!(task.worktree.is_none());
        assert!(task.tmux_window.is_none());
    }

    #[test]
    fn move_backward_without_dispatch_fields_no_cleanup() {
        let mut app = make_app();
        // Task 3 is Ready, no dispatch fields
        let cmds = app.update(Message::MoveTask {
            id: 3,
            direction: MoveDirection::Backward,
        });
        assert_eq!(cmds.len(), 1);
        assert!(matches!(cmds[0], Command::PersistTask(_)));
    }

    #[test]
    fn repo_path_empty_uses_saved_path() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = App::new(vec![]);
        app.repo_paths = vec!["/saved/repo".to_string()];

        // Set up InputRepoPath mode manually
        app.mode = InputMode::InputRepoPath {
            title: "Test".to_string(),
            description: "desc".to_string(),
        };
        app.input_buffer.clear();

        // Press Enter with empty buffer
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let _cmds = app.handle_key(key);

        // Should have created a task with the saved repo path
        assert_eq!(app.mode, InputMode::Normal);
        assert_eq!(app.tasks.len(), 1);
        assert_eq!(app.tasks[0].repo_path, "/saved/repo");
    }

    #[test]
    fn repo_path_empty_no_saved_stays_in_mode() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = App::new(vec![]);
        app.repo_paths = vec![]; // no saved paths

        app.mode = InputMode::InputRepoPath {
            title: "Test".to_string(),
            description: "desc".to_string(),
        };
        app.input_buffer.clear();

        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let _cmds = app.handle_key(key);

        // Should stay in InputRepoPath mode
        assert!(matches!(app.mode, InputMode::InputRepoPath { .. }));
        assert!(app.status_message.is_some());
        assert_eq!(app.tasks.len(), 0); // no task created
    }

    #[test]
    fn repo_path_nonempty_used_as_is() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = App::new(vec![]);
        app.repo_paths = vec!["/saved/repo".to_string()];

        app.mode = InputMode::InputRepoPath {
            title: "Test".to_string(),
            description: "desc".to_string(),
        };
        app.input_buffer = "/custom/path".to_string();

        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let _cmds = app.handle_key(key);

        assert_eq!(app.mode, InputMode::Normal);
        assert_eq!(app.tasks.len(), 1);
        assert_eq!(app.tasks[0].repo_path, "/custom/path");
    }

    #[test]
    fn tick_emits_load_notes_when_detail_visible() {
        let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
        app.detail_visible = true;
        app.selected_column = 0;
        app.selected_row[0] = 0;

        let cmds = app.update(Message::Tick);
        assert!(cmds.iter().any(|c| matches!(c, Command::LoadNotes(1))));
    }

    #[test]
    fn tick_skips_load_notes_when_detail_hidden() {
        let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);
        app.detail_visible = false;

        let cmds = app.update(Message::Tick);
        assert!(!cmds.iter().any(|c| matches!(c, Command::LoadNotes(_))));
    }

    #[test]
    fn notes_loaded_stores_in_cache() {
        use crate::models::{Note, NoteSource};
        let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)]);

        let notes = vec![Note {
            id: 1,
            task_id: 1,
            content: "Agent progress".to_string(),
            source: NoteSource::Agent,
            created_at: chrono::Utc::now(),
        }];

        app.update(Message::NotesLoaded { task_id: 1, notes });
        let cached = app.notes.get(&1).unwrap();
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].content, "Agent progress");
    }
}
