use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::{Duration, Instant};

use super::*;
use crate::models::{TaskId, TaskStatus};

fn make_key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn make_task(id: i64, status: TaskStatus) -> Task {
    let now = chrono::Utc::now();
    Task {
        id: TaskId(id),
        title: format!("Task {id}"),
        description: String::new(),
        repo_path: String::from("/repo"),
        status,
        worktree: None,
        tmux_window: None,
        plan: None,
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
    ], Duration::from_secs(300))
}

#[test]
fn tasks_by_status_filters() {
    let app = make_app();
    let backlog = app.tasks_by_status(TaskStatus::Backlog);
    assert_eq!(backlog.len(), 2);
    assert_eq!(backlog[0].id, TaskId(1));
    assert_eq!(backlog[1].id, TaskId(2));

    let ready = app.tasks_by_status(TaskStatus::Ready);
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].id, TaskId(3));

    let review = app.tasks_by_status(TaskStatus::Review);
    assert_eq!(review.len(), 0);
}

#[test]
fn move_task_forward() {
    let mut app = make_app();
    // Task 1 is in Backlog; move it forward -> Ready
    let cmds = app.update(Message::MoveTask {
        id: TaskId(1),
        direction: MoveDirection::Forward,
    });
    assert_eq!(app.tasks.iter().find(|t| t.id == TaskId(1)).unwrap().status, TaskStatus::Ready);
    // Should produce a PersistTask command
    assert!(matches!(cmds[0], Command::PersistTask(_)));
}

#[test]
fn move_task_backward_at_start_is_noop() {
    let mut app = make_app();
    // Task 1 is in Backlog; prev() stays Backlog
    let cmds = app.update(Message::MoveTask {
        id: TaskId(1),
        direction: MoveDirection::Backward,
    });
    assert_eq!(app.tasks.iter().find(|t| t.id == TaskId(1)).unwrap().status, TaskStatus::Backlog);
    assert!(cmds.is_empty());
}

#[test]
fn dispatch_only_ready_tasks() {
    let mut app = make_app();

    // Task 3 is Ready — should dispatch
    let cmds = app.update(Message::DispatchTask(TaskId(3)));
    assert!(matches!(cmds[0], Command::Dispatch { .. }));

    // Task 1 is Backlog — should not dispatch
    let cmds = app.update(Message::DispatchTask(TaskId(1)));
    assert!(cmds.is_empty());

    // Task 5 is Done — should not dispatch
    let cmds = app.update(Message::DispatchTask(TaskId(5)));
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
    let mut app = App::new(vec![task4], Duration::from_secs(300));
    let cmds = app.update(Message::Tick);
    // Should have CaptureTmux + RefreshFromDb
    assert_eq!(cmds.len(), 2);
    assert!(matches!(&cmds[0], Command::CaptureTmux { id: TaskId(4), window } if window == "main:task-4"));
    assert!(matches!(&cmds[1], Command::RefreshFromDb));
}

#[test]
fn tick_captures_review_task_with_live_window() {
    let mut task = make_task(5, TaskStatus::Review);
    task.tmux_window = Some("task-5".to_string());
    let mut app = App::new(vec![task], Duration::from_secs(300));

    let cmds = app.update(Message::Tick);

    assert!(cmds.iter().any(|c| matches!(c, Command::CaptureTmux { id: TaskId(5), .. })));
}

#[test]
fn task_created_adds_to_list() {
    let now = chrono::Utc::now();
    let task = Task {
        id: TaskId(42),
        title: "New Task".to_string(),
        description: "desc".to_string(),
        repo_path: "/repo".to_string(),
        status: TaskStatus::Backlog,
        worktree: None,
        tmux_window: None,
        plan: None,
        created_at: now,
        updated_at: now,
    };
    let mut app = App::new(vec![], Duration::from_secs(300));
    let cmds = app.update(Message::TaskCreated { task });
    assert_eq!(app.tasks.len(), 1);
    assert_eq!(app.tasks[0].id, TaskId(42));
    assert_eq!(app.tasks[0].status, TaskStatus::Backlog);
    assert!(cmds.is_empty());
}

#[test]
fn delete_task_removes_and_returns_command() {
    let mut app = make_app();
    let cmds = app.update(Message::DeleteTask(TaskId(1)));
    assert!(app.tasks.iter().all(|t| t.id != TaskId(1)));
    assert!(matches!(cmds[0], Command::DeleteTask(TaskId(1))));
}

#[test]
fn error_sets_error_popup() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.update(Message::Error("Something went wrong".to_string()));
    assert_eq!(app.error_popup.as_deref(), Some("Something went wrong"));
}

#[test]
fn dispatch_from_running_is_noop() {
    let mut task = make_task(4, TaskStatus::Running);
    task.worktree = Some("/repo/.worktrees/4-task-4".to_string());
    task.tmux_window = Some("task-4".to_string());
    let mut app = App::new(vec![task], Duration::from_secs(300));
    let cmds = app.update(Message::DispatchTask(TaskId(4)));
    assert!(cmds.is_empty());
}

#[test]
fn dispatch_from_review_is_noop() {
    let mut task = make_task(5, TaskStatus::Review);
    task.worktree = Some("/repo/.worktrees/5-task-5".to_string());
    task.tmux_window = Some("task-5".to_string());
    let mut app = App::new(vec![task], Duration::from_secs(300));
    let cmds = app.update(Message::DispatchTask(TaskId(5)));
    assert!(cmds.is_empty());
}

#[test]
fn move_backward_from_running_emits_cleanup() {
    let mut task = make_task(4, TaskStatus::Running);
    task.worktree = Some("/repo/.worktrees/4-task-4".to_string());
    task.tmux_window = Some("task-4".to_string());
    let mut app = App::new(vec![task], Duration::from_secs(300));

    let cmds = app.update(Message::MoveTask {
        id: TaskId(4),
        direction: MoveDirection::Backward,
    });

    // Should emit Cleanup then PersistTask
    assert_eq!(cmds.len(), 2);
    assert!(matches!(&cmds[0], Command::Cleanup { .. }));
    assert!(matches!(&cmds[1], Command::PersistTask(_)));

    // In-memory task should have cleared dispatch fields
    let task = app.tasks.iter().find(|t| t.id == TaskId(4)).unwrap();
    assert_eq!(task.status, TaskStatus::Ready);
    assert!(task.worktree.is_none());
    assert!(task.tmux_window.is_none());
}

#[test]
fn move_backward_without_dispatch_fields_no_cleanup() {
    let mut app = make_app();
    // Task 3 is Ready, no dispatch fields
    let cmds = app.update(Message::MoveTask {
        id: TaskId(3),
        direction: MoveDirection::Backward,
    });
    assert_eq!(cmds.len(), 1);
    assert!(matches!(cmds[0], Command::PersistTask(_)));
}

#[test]
fn repo_path_empty_uses_saved_path() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.repo_paths = vec!["/saved/repo".to_string()];

    app.mode = InputMode::InputRepoPath;
    app.task_draft = Some(TaskDraft { title: "Test".to_string(), description: "desc".to_string(), ..Default::default() });
    app.input_buffer.clear();

    let key = make_key(KeyCode::Enter);
    let cmds = app.handle_key(key);

    assert_eq!(app.mode, InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(c, Command::InsertTask(ref d) if d.repo_path == "/saved/repo")));
}

#[test]
fn repo_path_empty_no_saved_stays_in_mode() {

    let mut app = App::new(vec![], Duration::from_secs(300));
    app.repo_paths = vec![]; // no saved paths

    app.mode = InputMode::InputRepoPath;
    app.task_draft = Some(TaskDraft { title: "Test".to_string(), description: "desc".to_string(), ..Default::default() });
    app.input_buffer.clear();

    let key = make_key(KeyCode::Enter);
    let _cmds = app.handle_key(key);

    // Should stay in InputRepoPath mode
    assert_eq!(app.mode, InputMode::InputRepoPath);
    assert!(app.status_message.is_some());
    assert_eq!(app.tasks.len(), 0); // no task created
}

#[test]
fn repo_path_nonempty_used_as_is() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.repo_paths = vec!["/saved/repo".to_string()];

    app.mode = InputMode::InputRepoPath;
    app.task_draft = Some(TaskDraft { title: "Test".to_string(), description: "desc".to_string(), ..Default::default() });
    app.input_buffer = "/custom/path".to_string();

    let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    let cmds = app.handle_key(key);

    assert_eq!(app.mode, InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(c, Command::InsertTask(ref d) if d.repo_path == "/custom/path")));
    assert_eq!(app.tasks.len(), 0); // task not added until TaskCreated
}

#[test]
fn task_edited_updates_fields() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], Duration::from_secs(300));
    app.update(Message::TaskEdited {
        id: TaskId(1),
        title: "New".into(),
        description: "Desc".into(),
        repo_path: "/new".into(),
        status: TaskStatus::Ready,
        plan: Some("docs/plan.md".into()),
    });
    assert_eq!(app.tasks[0].title, "New");
    assert_eq!(app.tasks[0].description, "Desc");
    assert_eq!(app.tasks[0].repo_path, "/new");
    assert_eq!(app.tasks[0].status, TaskStatus::Ready);
    assert_eq!(app.tasks[0].plan.as_deref(), Some("docs/plan.md"));
}

#[test]
fn repo_paths_updated_replaces_paths() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.update(Message::RepoPathsUpdated(vec!["/a".into(), "/b".into()]));
    assert_eq!(app.repo_paths, vec!["/a", "/b"]);
}

#[test]
fn window_gone_on_running_task_marks_crashed_compat() {
    // Running task losing its window now marks it as crashed (not clears window)
    let mut task = make_task(4, TaskStatus::Running);
    task.worktree = Some("/repo/.worktrees/4-task-4".to_string());
    task.tmux_window = Some("task-4".to_string());
    let mut app = App::new(vec![task], Duration::from_secs(300));

    let cmds = app.update(Message::WindowGone(TaskId(4)));

    // Task should stay Running
    let task = app.tasks.iter().find(|t| t.id == TaskId(4)).unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    // tmux_window should NOT be cleared for crashed Running tasks
    assert!(task.tmux_window.is_some());
    // worktree should be preserved
    assert!(task.worktree.is_some());
    // Should be marked crashed, not emit PersistTask
    assert!(app.crashed_tasks.contains(&TaskId(4)));
    assert!(cmds.is_empty());
}

#[test]
fn move_forward_to_done_emits_cleanup() {
    let mut task = make_task(5, TaskStatus::Review);
    task.worktree = Some("/repo/.worktrees/5-task-5".to_string());
    task.tmux_window = None; // session closed, but worktree remains
    let mut app = App::new(vec![task], Duration::from_secs(300));

    let cmds = app.update(Message::MoveTask {
        id: TaskId(5),
        direction: MoveDirection::Forward,
    });

    let task = app.tasks.iter().find(|t| t.id == TaskId(5)).unwrap();
    assert_eq!(task.status, TaskStatus::Done);
    assert!(task.worktree.is_none());
    // Should have Cleanup + PersistTask
    assert_eq!(cmds.len(), 2);
    assert!(matches!(&cmds[0], Command::Cleanup { tmux_window: None, .. }));
    assert!(matches!(&cmds[1], Command::PersistTask(_)));
}

#[test]
fn move_forward_to_done_with_live_window_emits_cleanup() {
    let mut task = make_task(5, TaskStatus::Review);
    task.worktree = Some("/repo/.worktrees/5-task-5".to_string());
    task.tmux_window = Some("task-5".to_string());
    let mut app = App::new(vec![task], Duration::from_secs(300));

    let cmds = app.update(Message::MoveTask {
        id: TaskId(5),
        direction: MoveDirection::Forward,
    });

    assert_eq!(cmds.len(), 2);
    assert!(matches!(&cmds[0], Command::Cleanup { tmux_window: Some(_), .. }));
    assert!(matches!(&cmds[1], Command::PersistTask(_)));
}

#[test]
fn d_key_on_ready_dispatches() {

    let mut app = App::new(vec![make_task(3, TaskStatus::Ready)], Duration::from_secs(300));
    app.selected_column = 1; // Ready column
    let cmds = app.handle_key(make_key(KeyCode::Char('d')));
    assert!(matches!(&cmds[0], Command::Dispatch { .. }));
}

#[test]
fn d_key_on_running_with_window_shows_warning() {

    let mut task = make_task(4, TaskStatus::Running);
    task.tmux_window = Some("task-4".to_string());
    task.worktree = Some("/repo/.worktrees/4-task-4".to_string());
    let mut app = App::new(vec![task], Duration::from_secs(300));
    app.selected_column = 2; // Running column
    let cmds = app.handle_key(make_key(KeyCode::Char('d')));
    assert!(cmds.is_empty());
    assert!(app.status_message.as_deref().unwrap().contains("already running"));
}

#[test]
fn d_key_on_running_no_window_resumes() {

    let mut task = make_task(4, TaskStatus::Running);
    task.worktree = Some("/repo/.worktrees/4-task-4".to_string());
    task.tmux_window = None;
    let mut app = App::new(vec![task], Duration::from_secs(300));
    app.selected_column = 2; // Running column
    let cmds = app.handle_key(make_key(KeyCode::Char('d')));
    assert!(matches!(&cmds[0], Command::Resume { .. }));
}

#[test]
fn d_key_on_backlog_brainstorms() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], Duration::from_secs(300));
    app.selected_column = 0; // Backlog column
    let cmds = app.handle_key(make_key(KeyCode::Char('d')));
    assert_eq!(cmds.len(), 1);
    assert!(matches!(&cmds[0], Command::Brainstorm { task } if task.id == TaskId(1)));
}

#[test]
fn d_key_on_done_shows_warning() {

    let mut app = App::new(vec![make_task(1, TaskStatus::Done)], Duration::from_secs(300));
    app.selected_column = 4; // Done column
    let cmds = app.handle_key(make_key(KeyCode::Char('d')));
    assert!(cmds.is_empty());
    assert!(app.status_message.is_some());
}

#[test]
fn d_key_on_running_no_worktree_no_window_shows_warning() {

    let mut task = make_task(4, TaskStatus::Running);
    task.worktree = None;
    task.tmux_window = None;
    let mut app = App::new(vec![task], Duration::from_secs(300));
    app.selected_column = 2; // Running column
    let cmds = app.handle_key(make_key(KeyCode::Char('d')));
    assert!(cmds.is_empty());
    assert!(app
        .status_message
        .as_deref()
        .unwrap()
        .contains("No worktree"));
}

#[test]
fn g_key_with_live_window_jumps() {

    let mut task = make_task(4, TaskStatus::Running);
    task.tmux_window = Some("task-4".to_string());
    let mut app = App::new(vec![task], Duration::from_secs(300));
    app.selected_column = 2; // Running column
    let cmds = app.handle_key(make_key(KeyCode::Char('g')));
    assert!(matches!(&cmds[0], Command::JumpToTmux { window } if window == "task-4"));
}

#[test]
fn brainstorm_only_backlog_tasks() {
    let mut app = make_app();

    // Task 1 is Backlog — should brainstorm
    let cmds = app.update(Message::BrainstormTask(TaskId(1)));
    assert_eq!(cmds.len(), 1);
    assert!(matches!(&cmds[0], Command::Brainstorm { task } if task.id == TaskId(1)));

    // Task 3 is Ready — should not brainstorm
    let cmds = app.update(Message::BrainstormTask(TaskId(3)));
    assert!(cmds.is_empty());

    // Task 5 is Done — should not brainstorm
    let cmds = app.update(Message::BrainstormTask(TaskId(5)));
    assert!(cmds.is_empty());
}

#[test]
fn g_key_without_window_shows_message() {

    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], Duration::from_secs(300));
    app.selected_column = 0;
    let cmds = app.handle_key(make_key(KeyCode::Char('g')));
    assert!(cmds.is_empty());
    assert!(app.status_message.as_deref().unwrap().contains("No active session"));
}

// --- Task creation key flow ---

#[test]
fn n_key_enters_title_mode() {
    let mut app = make_app();
    let cmds = app.handle_key(make_key(KeyCode::Char('n')));
    assert!(cmds.is_empty());
    assert_eq!(app.mode, InputMode::InputTitle);
    assert!(app.input_buffer.is_empty());
    assert!(app.task_draft.is_none());
    assert_eq!(app.status_message.as_deref(), Some("Enter title: "));
}

#[test]
fn typing_appends_to_input_buffer() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.mode = InputMode::InputTitle;
    app.handle_key(make_key(KeyCode::Char('H')));
    app.handle_key(make_key(KeyCode::Char('i')));
    assert_eq!(app.input_buffer, "Hi");
}

#[test]
fn backspace_pops_from_input_buffer() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.mode = InputMode::InputTitle;
    app.input_buffer = "abc".to_string();
    app.handle_key(make_key(KeyCode::Backspace));
    assert_eq!(app.input_buffer, "ab");
}

#[test]
fn backspace_on_empty_buffer_is_noop() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.mode = InputMode::InputTitle;
    app.input_buffer.clear();
    app.handle_key(make_key(KeyCode::Backspace));
    assert!(app.input_buffer.is_empty());
    assert_eq!(app.mode, InputMode::InputTitle);
}

#[test]
fn enter_with_title_advances_to_description() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.mode = InputMode::InputTitle;
    app.input_buffer = "My Task".to_string();
    app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.mode, InputMode::InputDescription);
    assert!(app.input_buffer.is_empty());
    assert_eq!(app.task_draft.as_ref().unwrap().title, "My Task");
    assert_eq!(app.status_message.as_deref(), Some("Enter description: "));
}

#[test]
fn enter_with_empty_title_cancels() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.mode = InputMode::InputTitle;
    app.input_buffer.clear();
    app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.mode, InputMode::Normal);
    assert!(app.task_draft.is_none());
    assert!(app.status_message.is_none());
}

#[test]
fn enter_with_whitespace_only_title_cancels() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.mode = InputMode::InputTitle;
    app.input_buffer = "   ".to_string();
    app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.mode, InputMode::Normal);
    assert!(app.task_draft.is_none());
}

#[test]
fn enter_in_description_advances_to_repo_path() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.mode = InputMode::InputDescription;
    app.task_draft = Some(TaskDraft { title: "T".to_string(), description: String::new(), ..Default::default() });
    app.input_buffer = "some desc".to_string();
    app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.mode, InputMode::InputRepoPath);
    assert!(app.input_buffer.is_empty());
    assert_eq!(app.task_draft.as_ref().unwrap().description, "some desc");
    assert_eq!(app.status_message.as_deref(), Some("Enter repo path: "));
}

#[test]
fn number_key_in_repo_path_selects_saved_path() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.mode = InputMode::InputRepoPath;
    app.task_draft = Some(TaskDraft { title: "T".to_string(), description: "d".to_string(), ..Default::default() });
    app.input_buffer.clear();
    app.repo_paths = vec!["/repo1".to_string(), "/repo2".to_string()];
    let cmds = app.handle_key(make_key(KeyCode::Char('2')));
    assert_eq!(app.mode, InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(c, Command::InsertTask(ref d) if d.repo_path == "/repo2")));
}

#[test]
fn number_key_out_of_range_appends_to_buffer() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.mode = InputMode::InputRepoPath;
    app.task_draft = Some(TaskDraft { title: "T".to_string(), description: String::new(), ..Default::default() });
    app.input_buffer.clear();
    app.repo_paths = vec!["/repo1".to_string()]; // only 1 path
    app.handle_key(make_key(KeyCode::Char('5')));
    assert_eq!(app.input_buffer, "5");
    assert_eq!(app.mode, InputMode::InputRepoPath);
}

#[test]
fn number_key_with_nonempty_buffer_appends() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.mode = InputMode::InputRepoPath;
    app.task_draft = Some(TaskDraft { title: "T".to_string(), description: String::new(), ..Default::default() });
    app.input_buffer = "/my".to_string();
    app.repo_paths = vec!["/repo1".to_string()];
    app.handle_key(make_key(KeyCode::Char('1')));
    assert_eq!(app.input_buffer, "/my1");
}

#[test]
fn zero_key_in_repo_path_appends_to_buffer() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.mode = InputMode::InputRepoPath;
    app.task_draft = Some(TaskDraft { title: "T".to_string(), description: String::new(), ..Default::default() });
    app.input_buffer.clear();
    app.repo_paths = vec!["/repo".to_string()];
    app.handle_key(make_key(KeyCode::Char('0')));
    assert_eq!(app.input_buffer, "0");
}

#[test]
fn escape_from_title_mode_cancels() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.mode = InputMode::InputTitle;
    app.input_buffer = "partial".to_string();
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.mode, InputMode::Normal);
    assert!(app.input_buffer.is_empty());
    assert!(app.task_draft.is_none());
    assert!(app.status_message.is_none());
}

#[test]
fn escape_from_description_mode_cancels() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.mode = InputMode::InputDescription;
    app.task_draft = Some(TaskDraft { title: "T".to_string(), description: String::new(), ..Default::default() });
    app.input_buffer = "partial".to_string();
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.mode, InputMode::Normal);
    assert!(app.input_buffer.is_empty());
    assert!(app.task_draft.is_none());
    assert!(app.status_message.is_none());
}

#[test]
fn escape_from_repo_path_mode_cancels() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.mode = InputMode::InputRepoPath;
    app.task_draft = Some(TaskDraft { title: "T".to_string(), description: String::new(), ..Default::default() });
    app.input_buffer = "/partial".to_string();
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.mode, InputMode::Normal);
    assert!(app.input_buffer.is_empty());
    assert!(app.task_draft.is_none());
    assert!(app.status_message.is_none());
}

// --- Delete confirmation flow ---

#[test]
fn x_key_enters_confirm_delete_mode() {
    let mut app = make_app();
    app.selected_column = 0; // Backlog has tasks
    let cmds = app.handle_key(make_key(KeyCode::Char('x')));
    assert!(cmds.is_empty());
    assert_eq!(app.mode, InputMode::ConfirmDelete);
    assert_eq!(app.status_message.as_deref(), Some("Delete task? (y/n)"));
}

#[test]
fn y_confirms_deletion() {
    let mut app = make_app();
    app.selected_column = 0;
    app.handle_key(make_key(KeyCode::Char('x'))); // enter confirm mode
    let cmds = app.handle_key(make_key(KeyCode::Char('y')));
    assert_eq!(app.mode, InputMode::Normal);
    assert!(app.tasks.iter().all(|t| t.id != TaskId(1))); // task 1 deleted
    assert!(matches!(&cmds[0], Command::DeleteTask(TaskId(1))));
    assert!(app.status_message.is_none());
}

#[test]
fn uppercase_y_confirms_deletion() {
    let mut app = make_app();
    app.selected_column = 0;
    app.handle_key(make_key(KeyCode::Char('x')));
    let cmds = app.handle_key(make_key(KeyCode::Char('Y')));
    assert_eq!(app.mode, InputMode::Normal);
    assert!(app.tasks.iter().all(|t| t.id != TaskId(1)));
    assert!(matches!(&cmds[0], Command::DeleteTask(TaskId(1))));
}

#[test]
fn n_cancels_deletion() {
    let mut app = make_app();
    app.selected_column = 0;
    app.handle_key(make_key(KeyCode::Char('x')));
    let cmds = app.handle_key(make_key(KeyCode::Char('n')));
    assert_eq!(app.mode, InputMode::Normal);
    assert_eq!(app.tasks.len(), 5); // all tasks still present
    assert!(cmds.is_empty());
    assert!(app.status_message.is_none());
}

#[test]
fn escape_cancels_deletion() {
    let mut app = make_app();
    app.selected_column = 0;
    app.handle_key(make_key(KeyCode::Char('x')));
    let cmds = app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.mode, InputMode::Normal);
    assert_eq!(app.tasks.len(), 5);
    assert!(cmds.is_empty());
}

#[test]
fn x_key_on_empty_column_is_noop() {
    let mut app = make_app();
    app.selected_column = 3; // Review column is empty
    app.handle_key(make_key(KeyCode::Char('x')));
    assert_eq!(app.mode, InputMode::Normal); // did NOT enter ConfirmDelete
}

// --- Error popup dismissal ---

#[test]
fn any_key_clears_error_popup() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.error_popup = Some("boom".to_string());
    let cmds = app.handle_key(make_key(KeyCode::Char('a')));
    assert!(app.error_popup.is_none());
    assert!(cmds.is_empty());
}

// --- QuickDispatch ---

fn make_shift_key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::SHIFT)
}

#[test]
fn shift_d_with_one_repo_emits_quick_dispatch() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], Duration::from_secs(300));
    app.repo_paths = vec!["/repo".to_string()];
    let cmds = app.handle_key(make_shift_key(KeyCode::Char('D')));
    assert_eq!(cmds.len(), 1);
    assert!(matches!(&cmds[0], Command::QuickDispatch(ref d) if d.repo_path == "/repo"));
    assert_eq!(app.mode, InputMode::Normal);
}

#[test]
fn shift_d_with_no_repos_shows_error() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], Duration::from_secs(300));
    app.repo_paths = vec![];
    let cmds = app.handle_key(make_shift_key(KeyCode::Char('D')));
    assert!(cmds.is_empty());
    assert!(app.status_message.is_some());
    assert_eq!(app.mode, InputMode::Normal);
}

#[test]
fn shift_d_with_multiple_repos_enters_quick_dispatch_mode() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], Duration::from_secs(300));
    app.repo_paths = vec!["/repo1".to_string(), "/repo2".to_string()];
    let cmds = app.handle_key(make_shift_key(KeyCode::Char('D')));
    assert!(cmds.is_empty());
    assert_eq!(app.mode, InputMode::QuickDispatch);
}

#[test]
fn quick_dispatch_mode_number_selects_repo() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], Duration::from_secs(300));
    app.repo_paths = vec!["/repo1".to_string(), "/repo2".to_string()];
    app.mode = InputMode::QuickDispatch;
    let cmds = app.handle_key(make_key(KeyCode::Char('2')));
    assert_eq!(cmds.len(), 1);
    assert!(matches!(&cmds[0], Command::QuickDispatch(ref d) if d.repo_path == "/repo2"));
    assert_eq!(app.mode, InputMode::Normal);
}

#[test]
fn quick_dispatch_mode_esc_cancels() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], Duration::from_secs(300));
    app.repo_paths = vec!["/repo1".to_string(), "/repo2".to_string()];
    app.mode = InputMode::QuickDispatch;
    let cmds = app.handle_key(make_key(KeyCode::Esc));
    assert!(cmds.is_empty());
    assert_eq!(app.mode, InputMode::Normal);
}

#[test]
fn quick_dispatch_mode_invalid_number_is_noop() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], Duration::from_secs(300));
    app.repo_paths = vec!["/repo1".to_string()];
    app.mode = InputMode::QuickDispatch;
    let cmds = app.handle_key(make_key(KeyCode::Char('3')));
    assert!(cmds.is_empty());
    assert_eq!(app.mode, InputMode::QuickDispatch);
}

#[test]
fn quick_dispatch_message_emits_command() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    let cmds = app.update(Message::QuickDispatch { repo_path: "/my/repo".to_string() });
    assert_eq!(cmds.len(), 1);
    assert!(matches!(&cmds[0], Command::QuickDispatch(ref d)
        if d.title == "Quick task" && d.repo_path == "/my/repo"));
}

#[test]
fn error_popup_blocks_normal_key_handling() {
    let mut app = make_app();
    app.error_popup = Some("boom".to_string());
    app.handle_key(make_key(KeyCode::Char('q'))); // would normally quit
    assert!(app.error_popup.is_none());
    assert!(!app.should_quit); // quit was NOT processed
}

// --- Toggle detail ---

#[test]
fn toggle_detail_flips_visibility() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    assert!(!app.detail_visible);
    app.update(Message::ToggleDetail);
    assert!(app.detail_visible);
    app.update(Message::ToggleDetail);
    assert!(!app.detail_visible);
}

#[test]
fn stale_agent_detected_after_timeout() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Running),
    ], Duration::from_secs(300));
    app.tasks[0].tmux_window = Some("task-4".to_string());
    app.last_output_change.insert(TaskId(4), Instant::now() - Duration::from_secs(301));

    let cmds = app.update(Message::Tick);
    assert!(app.stale_tasks.contains(&TaskId(4)));
    assert!(cmds.iter().any(|c| matches!(c, Command::CaptureTmux { id: TaskId(4), .. })));
}

#[test]
fn window_gone_on_running_task_marks_crashed() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Running),
    ], Duration::from_secs(300));
    app.tasks[0].tmux_window = Some("task-4".to_string());

    let cmds = app.update(Message::WindowGone(TaskId(4)));
    assert!(app.crashed_tasks.contains(&TaskId(4)));
    // tmux_window should NOT be cleared for crashed Running tasks
    assert!(app.tasks[0].tmux_window.is_some());
    // Should NOT emit PersistTask
    assert!(cmds.is_empty());
}

#[test]
fn window_gone_on_review_task_clears_window() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Review),
    ], Duration::from_secs(300));
    app.tasks[0].tmux_window = Some("task-4".to_string());

    let cmds = app.update(Message::WindowGone(TaskId(4)));
    assert!(!app.crashed_tasks.contains(&TaskId(4)));
    assert!(app.tasks[0].tmux_window.is_none());
    assert!(matches!(&cmds[0], Command::PersistTask(_)));
}

#[test]
fn tmux_output_change_resets_staleness_timer() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Running),
    ], Duration::from_secs(300));
    app.tasks[0].tmux_window = Some("task-4".to_string());
    app.last_output_change.insert(TaskId(4), Instant::now() - Duration::from_secs(301));
    app.tmux_outputs.insert(TaskId(4), "old output".to_string());

    app.update(Message::TmuxOutput { id: TaskId(4), output: "new output".to_string() });
    let elapsed = app.last_output_change[&TaskId(4)].elapsed();
    assert!(elapsed < Duration::from_secs(1));
}

#[test]
fn tmux_output_same_does_not_reset_timer() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Running),
    ], Duration::from_secs(300));
    app.tasks[0].tmux_window = Some("task-4".to_string());
    let old_instant = Instant::now() - Duration::from_secs(200);
    app.last_output_change.insert(TaskId(4), old_instant);
    app.tmux_outputs.insert(TaskId(4), "same output".to_string());

    app.update(Message::TmuxOutput { id: TaskId(4), output: "same output".to_string() });
    let elapsed = app.last_output_change[&TaskId(4)].elapsed();
    assert!(elapsed >= Duration::from_secs(199));
}

#[test]
fn enter_key_toggles_detail() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    assert!(!app.detail_visible);
    app.handle_key(make_key(KeyCode::Enter));
    assert!(app.detail_visible);
}

// --- Async message handlers ---

#[test]
fn dispatched_sets_fields_and_transitions_to_running() {
    let mut app = App::new(vec![make_task(3, TaskStatus::Ready)], Duration::from_secs(300));
    let cmds = app.update(Message::Dispatched {
        id: TaskId(3),
        worktree: "/wt".to_string(),
        tmux_window: "win".to_string(),
        switch_focus: false,
    });
    let task = app.tasks.iter().find(|t| t.id == TaskId(3)).unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.worktree.as_deref(), Some("/wt"));
    assert_eq!(task.tmux_window.as_deref(), Some("win"));
    assert_eq!(cmds.len(), 1);
    assert!(matches!(&cmds[0], Command::PersistTask(_)));
}

#[test]
fn dispatched_with_switch_focus_emits_jump() {
    let mut app = App::new(vec![make_task(3, TaskStatus::Ready)], Duration::from_secs(300));
    let cmds = app.update(Message::Dispatched {
        id: TaskId(3),
        worktree: "/wt".to_string(),
        tmux_window: "win".to_string(),
        switch_focus: true,
    });
    assert_eq!(cmds.len(), 2);
    assert!(matches!(&cmds[0], Command::PersistTask(_)));
    assert!(matches!(&cmds[1], Command::JumpToTmux { window } if window == "win"));
}

#[test]
fn dispatched_unknown_id_is_noop() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Ready)], Duration::from_secs(300));
    let cmds = app.update(Message::Dispatched {
        id: TaskId(999),
        worktree: "/wt".to_string(),
        tmux_window: "win".to_string(),
        switch_focus: false,
    });
    assert!(cmds.is_empty());
    assert_eq!(app.tasks[0].status, TaskStatus::Ready);
}

#[test]
fn resumed_sets_tmux_window() {
    let mut task = make_task(4, TaskStatus::Running);
    task.worktree = Some("/wt".to_string());
    let mut app = App::new(vec![task], Duration::from_secs(300));
    let cmds = app.update(Message::Resumed {
        id: TaskId(4),
        tmux_window: "win-4".to_string(),
    });
    assert_eq!(app.tasks[0].tmux_window.as_deref(), Some("win-4"));
    assert_eq!(cmds.len(), 1);
    assert!(matches!(&cmds[0], Command::PersistTask(_)));
}

#[test]
fn resumed_unknown_id_is_noop() {
    let mut app = App::new(vec![make_task(4, TaskStatus::Running)], Duration::from_secs(300));
    let cmds = app.update(Message::Resumed {
        id: TaskId(999),
        tmux_window: "win".to_string(),
    });
    assert!(cmds.is_empty());
}

#[test]
fn resumed_sets_status_to_running() {
    let mut task = make_task(4, TaskStatus::Review);
    task.worktree = Some("/repo/.worktrees/4-task-4".to_string());
    task.tmux_window = None;
    let mut app = App::new(vec![task], Duration::from_secs(300));

    let cmds = app.update(Message::Resumed {
        id: TaskId(4),
        tmux_window: "task-4".to_string(),
    });

    let task = app.tasks.iter().find(|t| t.id == TaskId(4)).unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.tmux_window.as_deref(), Some("task-4"));
    assert_eq!(cmds.len(), 1);
    assert!(matches!(&cmds[0], Command::PersistTask(t) if t.status == TaskStatus::Running));
}

#[test]
fn tmux_output_stores_in_map() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Running)], Duration::from_secs(300));
    let cmds = app.update(Message::TmuxOutput {
        id: TaskId(1),
        output: "hello".to_string(),
    });
    assert_eq!(app.tmux_outputs.get(&TaskId(1)).unwrap(), "hello");
    assert!(cmds.is_empty());
}

#[test]
fn tmux_output_overwrites_previous() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Running)], Duration::from_secs(300));
    app.update(Message::TmuxOutput { id: TaskId(1), output: "first".to_string() });
    app.update(Message::TmuxOutput { id: TaskId(1), output: "second".to_string() });
    assert_eq!(app.tmux_outputs.get(&TaskId(1)).unwrap(), "second");
}

#[test]
fn refresh_tasks_replaces_and_clamps() {
    let mut app = make_app();
    app.selected_row[0] = 1; // row 1 of Backlog (has 2 items)
    app.update(Message::RefreshTasks(vec![make_task(10, TaskStatus::Backlog)]));
    assert_eq!(app.tasks.len(), 1);
    assert_eq!(app.tasks[0].id, TaskId(10));
    assert_eq!(app.selected_row[0], 0); // clamped from 1 to 0
}

#[test]
fn refresh_tasks_empty_clamps_all_rows_to_zero() {
    let mut app = make_app();
    app.selected_row[0] = 1;
    app.selected_row[1] = 1;
    app.update(Message::RefreshTasks(vec![]));
    assert!(app.tasks.is_empty());
    assert!(app.selected_row.iter().all(|&r| r == 0));
}

// --- Key actions on Review status ---

#[test]
fn d_key_on_review_with_window_shows_warning() {
    let mut task = make_task(5, TaskStatus::Review);
    task.tmux_window = Some("task-5".to_string());
    task.worktree = Some("/repo/.worktrees/5-task-5".to_string());
    let mut app = App::new(vec![task], Duration::from_secs(300));
    app.selected_column = 3; // Review column
    let cmds = app.handle_key(make_key(KeyCode::Char('d')));
    assert!(cmds.is_empty());
    assert!(app.status_message.as_deref().unwrap().contains("already running"));
}

#[test]
fn d_key_on_review_no_window_with_worktree_resumes() {
    let mut task = make_task(5, TaskStatus::Review);
    task.worktree = Some("/repo/.worktrees/5-task-5".to_string());
    task.tmux_window = None;
    let mut app = App::new(vec![task], Duration::from_secs(300));
    app.selected_column = 3; // Review column
    let cmds = app.handle_key(make_key(KeyCode::Char('d')));
    assert!(matches!(&cmds[0], Command::Resume { .. }));
}

#[test]
fn d_key_on_review_no_worktree_no_window_shows_warning() {
    let mut task = make_task(5, TaskStatus::Review);
    task.worktree = None;
    task.tmux_window = None;
    let mut app = App::new(vec![task], Duration::from_secs(300));
    app.selected_column = 3; // Review column
    let cmds = app.handle_key(make_key(KeyCode::Char('d')));
    assert!(cmds.is_empty());
    assert!(app.status_message.as_deref().unwrap().contains("No worktree"));
}

// --- Actions on empty columns ---

#[test]
fn d_key_on_empty_column_is_noop() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.selected_column = 0;
    let cmds = app.handle_key(make_key(KeyCode::Char('d')));
    assert!(cmds.is_empty());
}

#[test]
fn g_key_on_empty_column_is_noop() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.selected_column = 0;
    let cmds = app.handle_key(make_key(KeyCode::Char('g')));
    assert!(cmds.is_empty());
}

#[test]
fn m_key_on_empty_column_is_noop() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.selected_column = 0;
    let cmds = app.handle_key(make_key(KeyCode::Char('m')));
    assert!(cmds.is_empty());
}

#[test]
fn shift_m_key_on_empty_column_is_noop() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.selected_column = 0;
    let cmds = app.handle_key(make_key(KeyCode::Char('M')));
    assert!(cmds.is_empty());
}

#[test]
fn e_key_on_empty_column_is_noop() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.selected_column = 0;
    let cmds = app.handle_key(make_key(KeyCode::Char('e')));
    assert!(cmds.is_empty());
}

// --- action_hints ---

#[test]
fn action_hints_backlog_task() {
    let task = make_task(1, TaskStatus::Backlog);
    let hints = ui::action_hints(Some(&task));
    let text: String = hints.iter().map(|s| s.content.as_ref()).collect();
    assert!(text.contains("[d]"), "should have dispatch/brainstorm hint");
    assert!(text.contains("brainstorm"), "backlog dispatch means brainstorm");
    assert!(text.contains("[e]"), "should have edit hint");
    assert!(text.contains("[m]"), "should have move hint");
    assert!(!text.contains("[M]"), "backlog has no back movement");
    assert!(text.contains("[x]"), "should have delete hint");
    assert!(text.contains("[n]"), "should have new hint");
    assert!(text.contains("[q]"), "should have quit hint");
}

#[test]
fn action_hints_ready_task() {
    let task = make_task(3, TaskStatus::Ready);
    let hints = ui::action_hints(Some(&task));
    let text: String = hints.iter().map(|s| s.content.as_ref()).collect();
    assert!(text.contains("[d]"), "should have dispatch hint");
    assert!(text.contains("ispatch"), "ready dispatch means dispatch");
    assert!(text.contains("[M]"), "ready has back movement");
}

#[test]
fn action_hints_running_with_window() {
    let mut task = make_task(4, TaskStatus::Running);
    task.tmux_window = Some("win-4".to_string());
    let hints = ui::action_hints(Some(&task));
    let text: String = hints.iter().map(|s| s.content.as_ref()).collect();
    assert!(text.contains("[g]"), "should have go-to-session hint");
    assert!(!text.contains("[d]"), "should not have dispatch/resume when window exists");
}

#[test]
fn action_hints_running_with_worktree_no_window() {
    let mut task = make_task(4, TaskStatus::Running);
    task.worktree = Some("/tmp/wt".to_string());
    let hints = ui::action_hints(Some(&task));
    let text: String = hints.iter().map(|s| s.content.as_ref()).collect();
    assert!(text.contains("[d]"), "should have resume hint");
    assert!(text.contains("resume"), "d means resume here");
    assert!(!text.contains("[g]"), "no go-to-session without window");
}

#[test]
fn action_hints_running_no_worktree_no_window() {
    let task = make_task(4, TaskStatus::Running);
    let hints = ui::action_hints(Some(&task));
    let text: String = hints.iter().map(|s| s.content.as_ref()).collect();
    assert!(!text.contains("[d]"), "no dispatch/resume without worktree");
    assert!(!text.contains("[g]"), "no go-to-session without window");
    assert!(text.contains("[e]"), "still has edit");
}

#[test]
fn action_hints_review_with_window() {
    let mut task = make_task(6, TaskStatus::Review);
    task.tmux_window = Some("win-6".to_string());
    let hints = ui::action_hints(Some(&task));
    let text: String = hints.iter().map(|s| s.content.as_ref()).collect();
    assert!(text.contains("[g]"), "review with window shows go-to-session");
}

#[test]
fn action_hints_done_task() {
    let task = make_task(5, TaskStatus::Done);
    let hints = ui::action_hints(Some(&task));
    let text: String = hints.iter().map(|s| s.content.as_ref()).collect();
    assert!(text.contains("[e]"), "done has edit");
    assert!(text.contains("[M]"), "done has back");
    assert!(text.contains("[x]"), "done has delete");
    assert!(!text.contains("[m]ove"), "done has no forward move");
    assert!(!text.contains("[d]"), "done has no dispatch");
}

#[test]
fn action_hints_no_task() {
    let hints = ui::action_hints(None);
    let text: String = hints.iter().map(|s| s.content.as_ref()).collect();
    assert!(text.contains("[n]"), "no-task shows new");
    assert!(text.contains("[q]"), "no-task shows quit");
    assert!(!text.contains("[d]"), "no-task has no dispatch");
    assert!(!text.contains("[e]"), "no-task has no edit");
}

// --- Edit key ---

#[test]
fn e_key_emits_edit_task_in_editor() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], Duration::from_secs(300));
    app.selected_column = 0;
    let cmds = app.handle_key(make_key(KeyCode::Char('e')));
    assert_eq!(cmds.len(), 1);
    assert!(matches!(&cmds[0], Command::EditTaskInEditor(t) if t.id == TaskId(1)));
}

#[test]
fn new_app_has_empty_agent_tracking() {
    let app = App::new(vec![], Duration::from_secs(300));
    assert!(app.stale_tasks().is_empty());
    assert!(app.crashed_tasks().is_empty());
}

#[test]
fn kill_and_retry_enters_confirm_mode() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Running),
    ], Duration::from_secs(300));
    app.tasks[0].tmux_window = Some("task-4".to_string());
    app.stale_tasks.insert(TaskId(4));

    app.update(Message::KillAndRetry(TaskId(4)));
    assert!(matches!(app.mode, InputMode::ConfirmRetry(TaskId(4))));
}

#[test]
fn retry_resume_emits_kill_and_resume() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Running),
    ], Duration::from_secs(300));
    app.tasks[0].tmux_window = Some("task-4".to_string());
    app.tasks[0].worktree = Some("/repo/.worktrees/4-task-4".to_string());
    app.stale_tasks.insert(TaskId(4));
    app.crashed_tasks.insert(TaskId(4));
    app.mode = InputMode::ConfirmRetry(TaskId(4));

    let cmds = app.update(Message::RetryResume(TaskId(4)));

    assert!(!app.stale_tasks.contains(&TaskId(4)));
    assert!(!app.crashed_tasks.contains(&TaskId(4)));
    assert_eq!(app.mode, InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(c, Command::KillTmuxWindow { .. })));
    assert!(cmds.iter().any(|c| matches!(c, Command::Resume { .. })));
}

#[test]
fn retry_fresh_emits_cleanup_and_dispatch() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Running),
    ], Duration::from_secs(300));
    app.tasks[0].tmux_window = Some("task-4".to_string());
    app.tasks[0].worktree = Some("/repo/.worktrees/4-task-4".to_string());
    app.stale_tasks.insert(TaskId(4));
    app.mode = InputMode::ConfirmRetry(TaskId(4));

    let cmds = app.update(Message::RetryFresh(TaskId(4)));

    assert!(!app.stale_tasks.contains(&TaskId(4)));
    assert_eq!(app.mode, InputMode::Normal);
    assert_eq!(app.tasks[0].status, TaskStatus::Ready);
    assert!(cmds.iter().any(|c| matches!(c, Command::Cleanup { .. })));
    assert!(cmds.iter().any(|c| matches!(c, Command::Dispatch { .. })));
}

#[test]
fn d_key_on_stale_running_task_enters_retry_mode() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Running),
    ], Duration::from_secs(300));
    app.tasks[0].tmux_window = Some("task-4".to_string());
    app.stale_tasks.insert(TaskId(4));
    // Navigate to Running column (index 2)
    app.selected_column = 2;
    app.selected_row[2] = 0;

    app.handle_key(make_key(KeyCode::Char('d')));
    assert!(matches!(app.mode, InputMode::ConfirmRetry(TaskId(4))));
}

#[test]
fn d_key_on_crashed_running_task_enters_retry_mode() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Running),
    ], Duration::from_secs(300));
    app.tasks[0].tmux_window = Some("task-4".to_string());
    app.crashed_tasks.insert(TaskId(4));
    app.selected_column = 2;
    app.selected_row[2] = 0;

    app.handle_key(make_key(KeyCode::Char('d')));
    assert!(matches!(app.mode, InputMode::ConfirmRetry(TaskId(4))));
}

#[test]
fn confirm_retry_r_key_emits_resume() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Running),
    ], Duration::from_secs(300));
    app.tasks[0].tmux_window = Some("task-4".to_string());
    app.tasks[0].worktree = Some("/repo/.worktrees/4-task-4".to_string());
    app.mode = InputMode::ConfirmRetry(TaskId(4));

    let cmds = app.handle_key(make_key(KeyCode::Char('r')));
    assert_eq!(app.mode, InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(c, Command::Resume { .. })));
}

#[test]
fn confirm_retry_f_key_emits_fresh() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Running),
    ], Duration::from_secs(300));
    app.tasks[0].tmux_window = Some("task-4".to_string());
    app.tasks[0].worktree = Some("/repo/.worktrees/4-task-4".to_string());
    app.mode = InputMode::ConfirmRetry(TaskId(4));

    let cmds = app.handle_key(make_key(KeyCode::Char('f')));
    assert_eq!(app.mode, InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(c, Command::Dispatch { .. })));
}

#[test]
fn confirm_retry_esc_returns_to_normal() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Running),
    ], Duration::from_secs(300));
    app.mode = InputMode::ConfirmRetry(TaskId(4));

    let cmds = app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.mode, InputMode::Normal);
    assert!(cmds.is_empty());
}
