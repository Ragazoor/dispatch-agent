use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer, style::{Color, Modifier}};
use std::time::{Duration, Instant};

use super::*;
use crate::models::{Epic, EpicId, TaskId, TaskStatus};

/// Check whether a rendered buffer contains the given text anywhere.
fn buffer_contains(buf: &Buffer, text: &str) -> bool {
    let area = buf.area();
    for y in area.top()..area.bottom() {
        let mut line = String::new();
        for x in area.left()..area.right() {
            line.push_str(buf[(x, y)].symbol());
        }
        if line.contains(text) {
            return true;
        }
    }
    false
}

/// Helper: render the app into a test terminal and return the buffer.
fn render_to_buffer(app: &App, width: u16, height: u16) -> Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| ui::render(f, app)).unwrap();
    terminal.backend().buffer().clone()
}

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
        epic_id: None,
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
    app.selection_mut().set_column(0);
    app.update(Message::NavigateColumn(-1));
    assert_eq!(app.selection().column(), 0); // can't go below 0

    app.selection_mut().set_column(4);
    app.update(Message::NavigateColumn(1));
    assert_eq!(app.selection().column(), 4); // can't go above 4
}

#[test]
fn navigate_row_clamps() {
    let mut app = make_app();
    // Backlog has 2 tasks (id 1, 2). Selected row starts at 0.
    app.selection_mut().set_column(0);
    app.update(Message::NavigateRow(-1));
    assert_eq!(app.selection().row(0), 0); // can't go below 0

    app.update(Message::NavigateRow(10));
    assert_eq!(app.selection().row(0), 1); // clamps to last item index
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
        epic_id: None,
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
fn delete_task_with_worktree_emits_cleanup() {
    let mut app = make_app();
    let task = app.find_task_mut(TaskId(4)).unwrap();
    task.worktree = Some("/repo/.worktrees/4-task".to_string());
    task.tmux_window = Some("task-4".to_string());

    let cmds = app.update(Message::DeleteTask(TaskId(4)));
    assert!(app.tasks.iter().all(|t| t.id != TaskId(4)));
    assert!(cmds.iter().any(|c| matches!(c, Command::Cleanup { .. })));
    assert!(cmds.iter().any(|c| matches!(c, Command::DeleteTask(TaskId(4)))));
}

#[test]
fn delete_task_without_worktree_no_cleanup() {
    let mut app = make_app();
    let cmds = app.update(Message::DeleteTask(TaskId(1)));
    assert!(!cmds.iter().any(|c| matches!(c, Command::Cleanup { .. })));
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

    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft { title: "Test".to_string(), description: "desc".to_string(), ..Default::default() });
    app.input.buffer.clear();

    let key = make_key(KeyCode::Enter);
    let cmds = app.handle_key(key);

    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(c, Command::InsertTask { ref draft, .. } if draft.repo_path == "/saved/repo")));
}

#[test]
fn repo_path_empty_no_saved_stays_in_mode() {

    let mut app = App::new(vec![], Duration::from_secs(300));
    app.repo_paths = vec![]; // no saved paths

    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft { title: "Test".to_string(), description: "desc".to_string(), ..Default::default() });
    app.input.buffer.clear();

    let key = make_key(KeyCode::Enter);
    let _cmds = app.handle_key(key);

    // Should stay in InputRepoPath mode
    assert_eq!(app.input.mode, InputMode::InputRepoPath);
    assert!(app.status_message.is_some());
    assert_eq!(app.tasks.len(), 0); // no task created
}

#[test]
fn repo_path_nonempty_used_as_is() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.repo_paths = vec!["/saved/repo".to_string()];

    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft { title: "Test".to_string(), description: "desc".to_string(), ..Default::default() });
    app.input.buffer = "/custom/path".to_string();

    let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    let cmds = app.handle_key(key);

    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(c, Command::InsertTask { ref draft, .. } if draft.repo_path == "/custom/path")));
    assert_eq!(app.tasks.len(), 0); // task not added until TaskCreated
}

#[test]
fn task_edited_updates_fields() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], Duration::from_secs(300));
    app.update(Message::TaskEdited(TaskEdit {
        id: TaskId(1),
        title: "New".into(),
        description: "Desc".into(),
        repo_path: "/new".into(),
        status: TaskStatus::Ready,
        plan: Some("docs/plan.md".into()),
    }));
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
fn move_forward_to_done_enters_confirm_mode() {
    let mut task = make_task(5, TaskStatus::Review);
    task.worktree = Some("/repo/.worktrees/5-task-5".to_string());
    task.tmux_window = None; // session closed, but worktree remains
    let mut app = App::new(vec![task], Duration::from_secs(300));

    let cmds = app.update(Message::MoveTask {
        id: TaskId(5),
        direction: MoveDirection::Forward,
    });

    // Should enter confirmation mode, not move immediately
    assert!(cmds.is_empty());
    assert!(matches!(app.input.mode, InputMode::ConfirmDone(TaskId(5))));
    let task = app.tasks.iter().find(|t| t.id == TaskId(5)).unwrap();
    assert_eq!(task.status, TaskStatus::Review);
    // Worktree preserved — not taken during confirmation
    assert!(task.worktree.is_some());
}

#[test]
fn move_forward_to_done_with_live_window_enters_confirm_mode() {
    let mut task = make_task(5, TaskStatus::Review);
    task.worktree = Some("/repo/.worktrees/5-task-5".to_string());
    task.tmux_window = Some("task-5".to_string());
    let mut app = App::new(vec![task], Duration::from_secs(300));

    let cmds = app.update(Message::MoveTask {
        id: TaskId(5),
        direction: MoveDirection::Forward,
    });

    // Should enter confirmation mode, not move immediately
    assert!(cmds.is_empty());
    assert!(matches!(app.input.mode, InputMode::ConfirmDone(TaskId(5))));
}

#[test]
fn d_key_on_ready_dispatches() {

    let mut app = App::new(vec![make_task(3, TaskStatus::Ready)], Duration::from_secs(300));
    app.selection_mut().set_column(1); // Ready column
    let cmds = app.handle_key(make_key(KeyCode::Char('d')));
    assert!(matches!(&cmds[0], Command::Dispatch { .. }));
}

#[test]
fn d_key_on_running_with_window_shows_warning() {

    let mut task = make_task(4, TaskStatus::Running);
    task.tmux_window = Some("task-4".to_string());
    task.worktree = Some("/repo/.worktrees/4-task-4".to_string());
    let mut app = App::new(vec![task], Duration::from_secs(300));
    app.selection_mut().set_column(2); // Running column
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
    app.selection_mut().set_column(2); // Running column
    let cmds = app.handle_key(make_key(KeyCode::Char('d')));
    assert!(matches!(&cmds[0], Command::Resume { .. }));
}

#[test]
fn d_key_on_backlog_brainstorms() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], Duration::from_secs(300));
    app.selection_mut().set_column(0); // Backlog column
    let cmds = app.handle_key(make_key(KeyCode::Char('d')));
    assert_eq!(cmds.len(), 1);
    assert!(matches!(&cmds[0], Command::Brainstorm { task } if task.id == TaskId(1)));
}

#[test]
fn d_key_on_done_shows_warning() {

    let mut app = App::new(vec![make_task(1, TaskStatus::Done)], Duration::from_secs(300));
    app.selection_mut().set_column(4); // Done column
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
    app.selection_mut().set_column(2); // Running column
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
    app.selection_mut().set_column(2); // Running column
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
    app.selection_mut().set_column(0);
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
    assert_eq!(app.input.mode, InputMode::InputTitle);
    assert!(app.input.buffer.is_empty());
    assert!(app.input.task_draft.is_none());
    assert_eq!(app.status_message.as_deref(), Some("Enter title: "));
}

#[test]
fn typing_appends_to_input_buffer() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.input.mode = InputMode::InputTitle;
    app.handle_key(make_key(KeyCode::Char('H')));
    app.handle_key(make_key(KeyCode::Char('i')));
    assert_eq!(app.input.buffer, "Hi");
}

#[test]
fn backspace_pops_from_input_buffer() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.input.mode = InputMode::InputTitle;
    app.input.buffer = "abc".to_string();
    app.handle_key(make_key(KeyCode::Backspace));
    assert_eq!(app.input.buffer, "ab");
}

#[test]
fn backspace_on_empty_buffer_is_noop() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.input.mode = InputMode::InputTitle;
    app.input.buffer.clear();
    app.handle_key(make_key(KeyCode::Backspace));
    assert!(app.input.buffer.is_empty());
    assert_eq!(app.input.mode, InputMode::InputTitle);
}

#[test]
fn enter_with_title_advances_to_description() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.input.mode = InputMode::InputTitle;
    app.input.buffer = "My Task".to_string();
    app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.input.mode, InputMode::InputDescription);
    assert!(app.input.buffer.is_empty());
    assert_eq!(app.input.task_draft.as_ref().unwrap().title, "My Task");
    assert_eq!(app.status_message.as_deref(), Some("Enter description: "));
}

#[test]
fn enter_with_empty_title_cancels() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.input.mode = InputMode::InputTitle;
    app.input.buffer.clear();
    app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.input.task_draft.is_none());
    assert!(app.status_message.is_none());
}

#[test]
fn enter_with_whitespace_only_title_cancels() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.input.mode = InputMode::InputTitle;
    app.input.buffer = "   ".to_string();
    app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.input.task_draft.is_none());
}

#[test]
fn enter_in_description_advances_to_repo_path() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.input.mode = InputMode::InputDescription;
    app.input.task_draft = Some(TaskDraft { title: "T".to_string(), description: String::new(), ..Default::default() });
    app.input.buffer = "some desc".to_string();
    app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.input.mode, InputMode::InputRepoPath);
    assert!(app.input.buffer.is_empty());
    assert_eq!(app.input.task_draft.as_ref().unwrap().description, "some desc");
    assert_eq!(app.status_message.as_deref(), Some("Enter repo path: "));
}

#[test]
fn number_key_in_repo_path_selects_saved_path() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft { title: "T".to_string(), description: "d".to_string(), ..Default::default() });
    app.input.buffer.clear();
    app.repo_paths = vec!["/repo1".to_string(), "/repo2".to_string()];
    let cmds = app.handle_key(make_key(KeyCode::Char('2')));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(c, Command::InsertTask { ref draft, .. } if draft.repo_path == "/repo2")));
}

#[test]
fn number_key_out_of_range_appends_to_buffer() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft { title: "T".to_string(), description: String::new(), ..Default::default() });
    app.input.buffer.clear();
    app.repo_paths = vec!["/repo1".to_string()]; // only 1 path
    app.handle_key(make_key(KeyCode::Char('5')));
    assert_eq!(app.input.buffer, "5");
    assert_eq!(app.input.mode, InputMode::InputRepoPath);
}

#[test]
fn number_key_with_nonempty_buffer_appends() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft { title: "T".to_string(), description: String::new(), ..Default::default() });
    app.input.buffer = "/my".to_string();
    app.repo_paths = vec!["/repo1".to_string()];
    app.handle_key(make_key(KeyCode::Char('1')));
    assert_eq!(app.input.buffer, "/my1");
}

#[test]
fn zero_key_in_repo_path_appends_to_buffer() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft { title: "T".to_string(), description: String::new(), ..Default::default() });
    app.input.buffer.clear();
    app.repo_paths = vec!["/repo".to_string()];
    app.handle_key(make_key(KeyCode::Char('0')));
    assert_eq!(app.input.buffer, "0");
}

#[test]
fn escape_from_title_mode_cancels() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.input.mode = InputMode::InputTitle;
    app.input.buffer = "partial".to_string();
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.input.buffer.is_empty());
    assert!(app.input.task_draft.is_none());
    assert!(app.status_message.is_none());
}

#[test]
fn escape_from_description_mode_cancels() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.input.mode = InputMode::InputDescription;
    app.input.task_draft = Some(TaskDraft { title: "T".to_string(), description: String::new(), ..Default::default() });
    app.input.buffer = "partial".to_string();
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.input.buffer.is_empty());
    assert!(app.input.task_draft.is_none());
    assert!(app.status_message.is_none());
}

#[test]
fn escape_from_repo_path_mode_cancels() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft { title: "T".to_string(), description: String::new(), ..Default::default() });
    app.input.buffer = "/partial".to_string();
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.input.buffer.is_empty());
    assert!(app.input.task_draft.is_none());
    assert!(app.status_message.is_none());
}

// --- Delete confirmation flow (via ConfirmDelete mode directly) ---

#[test]
fn confirm_delete_y_deletes_task() {
    let mut app = make_app();
    app.selection_mut().set_column(0);
    app.input.mode = InputMode::ConfirmDelete;
    let cmds = app.handle_key(make_key(KeyCode::Char('y')));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.tasks.iter().all(|t| t.id != TaskId(1))); // task 1 deleted
    assert!(matches!(&cmds[0], Command::DeleteTask(TaskId(1))));
    assert!(app.status_message.is_none());
}

#[test]
fn confirm_delete_uppercase_y_deletes_task() {
    let mut app = make_app();
    app.selection_mut().set_column(0);
    app.input.mode = InputMode::ConfirmDelete;
    let cmds = app.handle_key(make_key(KeyCode::Char('Y')));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.tasks.iter().all(|t| t.id != TaskId(1)));
    assert!(matches!(&cmds[0], Command::DeleteTask(TaskId(1))));
}

#[test]
fn confirm_delete_n_cancels() {
    let mut app = make_app();
    app.selection_mut().set_column(0);
    app.input.mode = InputMode::ConfirmDelete;
    let cmds = app.handle_key(make_key(KeyCode::Char('n')));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert_eq!(app.tasks.len(), 5);
    assert!(cmds.is_empty());
    assert!(app.status_message.is_none());
}

#[test]
fn confirm_delete_esc_cancels() {
    let mut app = make_app();
    app.selection_mut().set_column(0);
    app.input.mode = InputMode::ConfirmDelete;
    let cmds = app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert_eq!(app.tasks.len(), 5);
    assert!(cmds.is_empty());
}

// --- Archive confirmation flow (x key) ---

#[test]
fn x_key_enters_confirm_archive_mode() {
    let mut app = make_app();
    app.selection_mut().set_column(0); // Backlog has tasks
    let cmds = app.handle_key(make_key(KeyCode::Char('x')));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::ConfirmArchive);
    assert_eq!(app.status_message.as_deref(), Some("Archive task? (y/n)"));
}

#[test]
fn confirm_archive_y_emits_archive_task() {
    let mut app = make_app();
    app.selection_mut().set_column(0);
    app.handle_key(make_key(KeyCode::Char('x')));
    let _ = app.handle_key(make_key(KeyCode::Char('y')));
    assert_eq!(app.input.mode, InputMode::Normal);
    // Task 1 should now be Archived
    let task = app.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Archived);
}

#[test]
fn confirm_archive_n_cancels() {
    let mut app = make_app();
    app.selection_mut().set_column(0);
    app.handle_key(make_key(KeyCode::Char('x')));
    let _ = app.handle_key(make_key(KeyCode::Char('n')));
    assert_eq!(app.input.mode, InputMode::Normal);
    // Task 1 still in Backlog
    let task = app.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Backlog);
}

#[test]
fn x_key_on_empty_column_is_noop() {
    let mut app = make_app();
    app.selection_mut().set_column(3); // Review column is empty
    app.handle_key(make_key(KeyCode::Char('x')));
    assert_eq!(app.input.mode, InputMode::Normal); // did NOT enter ConfirmArchive
}

// --- H key toggles archive panel ---

#[test]
fn shift_h_toggles_archive() {
    let mut app = make_app();
    assert!(!app.archive.visible);
    app.handle_key(KeyEvent::new(KeyCode::Char('H'), KeyModifiers::SHIFT));
    assert!(app.archive.visible);
    app.handle_key(KeyEvent::new(KeyCode::Char('H'), KeyModifiers::SHIFT));
    assert!(!app.archive.visible);
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
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn shift_d_with_no_repos_shows_error() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], Duration::from_secs(300));
    app.repo_paths = vec![];
    let cmds = app.handle_key(make_shift_key(KeyCode::Char('D')));
    assert!(cmds.is_empty());
    assert!(app.status_message.is_some());
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn shift_d_with_multiple_repos_enters_quick_dispatch_mode() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], Duration::from_secs(300));
    app.repo_paths = vec!["/repo1".to_string(), "/repo2".to_string()];
    let cmds = app.handle_key(make_shift_key(KeyCode::Char('D')));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::QuickDispatch);
}

#[test]
fn quick_dispatch_mode_number_selects_repo() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], Duration::from_secs(300));
    app.repo_paths = vec!["/repo1".to_string(), "/repo2".to_string()];
    app.input.mode = InputMode::QuickDispatch;
    let cmds = app.handle_key(make_key(KeyCode::Char('2')));
    assert_eq!(cmds.len(), 1);
    assert!(matches!(&cmds[0], Command::QuickDispatch(ref d) if d.repo_path == "/repo2"));
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn quick_dispatch_mode_esc_cancels() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], Duration::from_secs(300));
    app.repo_paths = vec!["/repo1".to_string(), "/repo2".to_string()];
    app.input.mode = InputMode::QuickDispatch;
    let cmds = app.handle_key(make_key(KeyCode::Esc));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn quick_dispatch_mode_invalid_number_is_noop() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], Duration::from_secs(300));
    app.repo_paths = vec!["/repo1".to_string()];
    app.input.mode = InputMode::QuickDispatch;
    let cmds = app.handle_key(make_key(KeyCode::Char('3')));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::QuickDispatch);
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
    app.agents.last_output_change.insert(TaskId(4), Instant::now() - Duration::from_secs(301));

    let cmds = app.update(Message::Tick);
    assert!(app.agents.stale_tasks.contains(&TaskId(4)));
    assert!(cmds.iter().any(|c| matches!(c, Command::CaptureTmux { id: TaskId(4), .. })));
}

#[test]
fn window_gone_on_running_task_marks_crashed() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Running),
    ], Duration::from_secs(300));
    app.tasks[0].tmux_window = Some("task-4".to_string());

    let cmds = app.update(Message::WindowGone(TaskId(4)));
    assert!(app.agents.crashed_tasks.contains(&TaskId(4)));
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
    assert!(!app.agents.crashed_tasks.contains(&TaskId(4)));
    assert!(app.tasks[0].tmux_window.is_none());
    assert!(matches!(&cmds[0], Command::PersistTask(_)));
}

#[test]
fn tmux_output_change_resets_staleness_timer() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Running),
    ], Duration::from_secs(300));
    app.tasks[0].tmux_window = Some("task-4".to_string());
    app.agents.last_output_change.insert(TaskId(4), Instant::now() - Duration::from_secs(301));
    app.agents.tmux_outputs.insert(TaskId(4), "old output".to_string());

    app.update(Message::TmuxOutput { id: TaskId(4), output: "new output".to_string() });
    let elapsed = app.agents.last_output_change[&TaskId(4)].elapsed();
    assert!(elapsed < Duration::from_secs(1));
}

#[test]
fn tmux_output_same_does_not_reset_timer() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Running),
    ], Duration::from_secs(300));
    app.tasks[0].tmux_window = Some("task-4".to_string());
    let old_instant = Instant::now() - Duration::from_secs(200);
    app.agents.last_output_change.insert(TaskId(4), old_instant);
    app.agents.tmux_outputs.insert(TaskId(4), "same output".to_string());

    app.update(Message::TmuxOutput { id: TaskId(4), output: "same output".to_string() });
    let elapsed = app.agents.last_output_change[&TaskId(4)].elapsed();
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
    assert_eq!(app.agents.tmux_outputs.get(&TaskId(1)).unwrap(), "hello");
    assert!(cmds.is_empty());
}

#[test]
fn tmux_output_overwrites_previous() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Running)], Duration::from_secs(300));
    app.update(Message::TmuxOutput { id: TaskId(1), output: "first".to_string() });
    app.update(Message::TmuxOutput { id: TaskId(1), output: "second".to_string() });
    assert_eq!(app.agents.tmux_outputs.get(&TaskId(1)).unwrap(), "second");
}

#[test]
fn refresh_tasks_replaces_and_clamps() {
    let mut app = make_app();
    app.selection_mut().set_row(0, 1); // row 1 of Backlog (has 2 items)
    app.update(Message::RefreshTasks(vec![make_task(10, TaskStatus::Backlog)]));
    assert_eq!(app.tasks.len(), 1);
    assert_eq!(app.tasks[0].id, TaskId(10));
    assert_eq!(app.selection().row(0), 0); // clamped from 1 to 0
}

#[test]
fn refresh_tasks_empty_clamps_all_rows_to_zero() {
    let mut app = make_app();
    app.selection_mut().set_row(0, 1);
    app.selection_mut().set_row(1, 1);
    app.update(Message::RefreshTasks(vec![]));
    assert!(app.tasks.is_empty());
    assert!(app.selection().selected_row.iter().all(|&r| r == 0));
}

// --- Key actions on Review status ---

#[test]
fn d_key_on_review_with_window_shows_warning() {
    let mut task = make_task(5, TaskStatus::Review);
    task.tmux_window = Some("task-5".to_string());
    task.worktree = Some("/repo/.worktrees/5-task-5".to_string());
    let mut app = App::new(vec![task], Duration::from_secs(300));
    app.selection_mut().set_column(3); // Review column
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
    app.selection_mut().set_column(3); // Review column
    let cmds = app.handle_key(make_key(KeyCode::Char('d')));
    assert!(matches!(&cmds[0], Command::Resume { .. }));
}

#[test]
fn d_key_on_review_no_worktree_no_window_shows_warning() {
    let mut task = make_task(5, TaskStatus::Review);
    task.worktree = None;
    task.tmux_window = None;
    let mut app = App::new(vec![task], Duration::from_secs(300));
    app.selection_mut().set_column(3); // Review column
    let cmds = app.handle_key(make_key(KeyCode::Char('d')));
    assert!(cmds.is_empty());
    assert!(app.status_message.as_deref().unwrap().contains("No worktree"));
}

// --- Actions on empty columns ---

#[test]
fn d_key_on_empty_column_is_noop() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.selection_mut().set_column(0);
    let cmds = app.handle_key(make_key(KeyCode::Char('d')));
    assert!(cmds.is_empty());
}

#[test]
fn g_key_on_empty_column_is_noop() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.selection_mut().set_column(0);
    let cmds = app.handle_key(make_key(KeyCode::Char('g')));
    assert!(cmds.is_empty());
}

#[test]
fn m_key_on_empty_column_is_noop() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.selection_mut().set_column(0);
    let cmds = app.handle_key(make_key(KeyCode::Char('m')));
    assert!(cmds.is_empty());
}

#[test]
fn shift_m_key_on_empty_column_is_noop() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.selection_mut().set_column(0);
    let cmds = app.handle_key(make_key(KeyCode::Char('M')));
    assert!(cmds.is_empty());
}

#[test]
fn e_key_on_empty_column_is_noop() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.selection_mut().set_column(0);
    let cmds = app.handle_key(make_key(KeyCode::Char('e')));
    assert!(cmds.is_empty());
}

// --- action_hints ---

#[test]
fn action_hints_backlog_task() {
    let task = make_task(1, TaskStatus::Backlog);
    let hints = ui::action_hints(Some(&task), Color::Rgb(122, 162, 247));
    let keys: Vec<&str> = hints.iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
        .map(|s| s.content.as_ref())
        .collect();
    assert!(keys.contains(&"d"), "should have dispatch/brainstorm hint");
    assert!(keys.contains(&"e"), "should have edit hint");
    assert!(keys.contains(&"m"), "should have move hint");
    assert!(!keys.contains(&"M"), "backlog has no back movement");
    assert!(keys.contains(&"x"), "should have archive hint");
    assert!(keys.contains(&"n"), "should have new hint");
    assert!(keys.contains(&"q"), "should have quit hint");
    let text: String = hints.iter().map(|s| s.content.as_ref()).collect();
    assert!(text.contains("brainstorm"), "backlog dispatch means brainstorm");
}

#[test]
fn action_hints_ready_task() {
    let task = make_task(3, TaskStatus::Ready);
    let hints = ui::action_hints(Some(&task), Color::Rgb(122, 162, 247));
    let keys: Vec<&str> = hints.iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
        .map(|s| s.content.as_ref())
        .collect();
    assert!(keys.contains(&"d"), "should have dispatch hint");
    assert!(keys.contains(&"M"), "ready has back movement");
    let text: String = hints.iter().map(|s| s.content.as_ref()).collect();
    assert!(text.contains("dispatch"), "ready dispatch means dispatch");
}

#[test]
fn action_hints_running_with_window() {
    let mut task = make_task(4, TaskStatus::Running);
    task.tmux_window = Some("win-4".to_string());
    let hints = ui::action_hints(Some(&task), Color::Rgb(122, 162, 247));
    let keys: Vec<&str> = hints.iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
        .map(|s| s.content.as_ref())
        .collect();
    assert!(keys.contains(&"g"), "should have go-to-session hint");
    assert!(!keys.contains(&"d"), "should not have dispatch/resume when window exists");
}

#[test]
fn action_hints_running_with_worktree_no_window() {
    let mut task = make_task(4, TaskStatus::Running);
    task.worktree = Some("/tmp/wt".to_string());
    let hints = ui::action_hints(Some(&task), Color::Rgb(122, 162, 247));
    let keys: Vec<&str> = hints.iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
        .map(|s| s.content.as_ref())
        .collect();
    assert!(keys.contains(&"d"), "should have resume hint");
    assert!(!keys.contains(&"g"), "no go-to-session without window");
    let text: String = hints.iter().map(|s| s.content.as_ref()).collect();
    assert!(text.contains("resume"), "d means resume here");
}

#[test]
fn action_hints_running_no_worktree_no_window() {
    let task = make_task(4, TaskStatus::Running);
    let hints = ui::action_hints(Some(&task), Color::Rgb(122, 162, 247));
    let keys: Vec<&str> = hints.iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
        .map(|s| s.content.as_ref())
        .collect();
    assert!(!keys.contains(&"d"), "no dispatch/resume without worktree");
    assert!(!keys.contains(&"g"), "no go-to-session without window");
    assert!(keys.contains(&"e"), "still has edit");
}

#[test]
fn action_hints_review_with_window() {
    let mut task = make_task(6, TaskStatus::Review);
    task.tmux_window = Some("win-6".to_string());
    let hints = ui::action_hints(Some(&task), Color::Rgb(122, 162, 247));
    let keys: Vec<&str> = hints.iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
        .map(|s| s.content.as_ref())
        .collect();
    assert!(keys.contains(&"g"), "review with window shows go-to-session");
}

#[test]
fn action_hints_done_task() {
    let task = make_task(5, TaskStatus::Done);
    let hints = ui::action_hints(Some(&task), Color::Rgb(122, 162, 247));
    let keys: Vec<&str> = hints.iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
        .map(|s| s.content.as_ref())
        .collect();
    assert!(keys.contains(&"e"), "done has edit");
    assert!(keys.contains(&"M"), "done has back");
    assert!(keys.contains(&"x"), "done has archive");
    assert!(!keys.contains(&"m"), "done has no forward move");
    assert!(!keys.contains(&"d"), "done has no dispatch");
}

#[test]
fn action_hints_no_task() {
    let hints = ui::action_hints(None, Color::Rgb(122, 162, 247));
    let keys: Vec<&str> = hints.iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
        .map(|s| s.content.as_ref())
        .collect();
    assert!(keys.contains(&"n"), "no-task shows new");
    assert!(keys.contains(&"q"), "no-task shows quit");
    assert!(!keys.contains(&"d"), "no-task has no dispatch");
    assert!(!keys.contains(&"e"), "no-task has no edit");
}

// --- Edit key ---

#[test]
fn e_key_emits_edit_task_in_editor() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], Duration::from_secs(300));
    app.selection_mut().set_column(0);
    let cmds = app.handle_key(make_key(KeyCode::Char('e')));
    assert_eq!(cmds.len(), 1);
    assert!(matches!(&cmds[0], Command::EditTaskInEditor(t) if t.id == TaskId(1)));
}

#[test]
fn new_app_has_empty_agent_tracking() {
    let app = App::new(vec![], Duration::from_secs(300));
    assert!(app.agents.stale_tasks.is_empty());
    assert!(app.agents.crashed_tasks.is_empty());
}

#[test]
fn kill_and_retry_enters_confirm_mode() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Running),
    ], Duration::from_secs(300));
    app.tasks[0].tmux_window = Some("task-4".to_string());
    app.agents.stale_tasks.insert(TaskId(4));

    app.update(Message::KillAndRetry(TaskId(4)));
    assert!(matches!(app.input.mode, InputMode::ConfirmRetry(TaskId(4))));
}

#[test]
fn retry_resume_emits_kill_and_resume() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Running),
    ], Duration::from_secs(300));
    app.tasks[0].tmux_window = Some("task-4".to_string());
    app.tasks[0].worktree = Some("/repo/.worktrees/4-task-4".to_string());
    app.agents.stale_tasks.insert(TaskId(4));
    app.agents.crashed_tasks.insert(TaskId(4));
    app.input.mode = InputMode::ConfirmRetry(TaskId(4));

    let cmds = app.update(Message::RetryResume(TaskId(4)));

    assert!(!app.agents.stale_tasks.contains(&TaskId(4)));
    assert!(!app.agents.crashed_tasks.contains(&TaskId(4)));
    assert_eq!(app.input.mode, InputMode::Normal);
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
    app.agents.stale_tasks.insert(TaskId(4));
    app.input.mode = InputMode::ConfirmRetry(TaskId(4));

    let cmds = app.update(Message::RetryFresh(TaskId(4)));

    assert!(!app.agents.stale_tasks.contains(&TaskId(4)));
    assert_eq!(app.input.mode, InputMode::Normal);
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
    app.agents.stale_tasks.insert(TaskId(4));
    // Navigate to Running column (index 2)
    app.selection_mut().set_column(2);
    app.selection_mut().set_row(2, 0);

    app.handle_key(make_key(KeyCode::Char('d')));
    assert!(matches!(app.input.mode, InputMode::ConfirmRetry(TaskId(4))));
}

#[test]
fn d_key_on_crashed_running_task_enters_retry_mode() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Running),
    ], Duration::from_secs(300));
    app.tasks[0].tmux_window = Some("task-4".to_string());
    app.agents.crashed_tasks.insert(TaskId(4));
    app.selection_mut().set_column(2);
    app.selection_mut().set_row(2, 0);

    app.handle_key(make_key(KeyCode::Char('d')));
    assert!(matches!(app.input.mode, InputMode::ConfirmRetry(TaskId(4))));
}

#[test]
fn confirm_retry_r_key_emits_resume() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Running),
    ], Duration::from_secs(300));
    app.tasks[0].tmux_window = Some("task-4".to_string());
    app.tasks[0].worktree = Some("/repo/.worktrees/4-task-4".to_string());
    app.input.mode = InputMode::ConfirmRetry(TaskId(4));

    let cmds = app.handle_key(make_key(KeyCode::Char('r')));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(c, Command::Resume { .. })));
}

#[test]
fn confirm_retry_f_key_emits_fresh() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Running),
    ], Duration::from_secs(300));
    app.tasks[0].tmux_window = Some("task-4".to_string());
    app.tasks[0].worktree = Some("/repo/.worktrees/4-task-4".to_string());
    app.input.mode = InputMode::ConfirmRetry(TaskId(4));

    let cmds = app.handle_key(make_key(KeyCode::Char('f')));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(c, Command::Dispatch { .. })));
}

#[test]
fn confirm_retry_esc_returns_to_normal() {
    let mut app = App::new(vec![
        make_task(4, TaskStatus::Running),
    ], Duration::from_secs(300));
    app.input.mode = InputMode::ConfirmRetry(TaskId(4));

    let cmds = app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.is_empty());
}

// --- Message-level tests for new input routing handlers ---

#[test]
fn dismiss_error_clears_popup() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.error_popup = Some("boom".to_string());
    app.update(Message::DismissError);
    assert!(app.error_popup.is_none());
}

#[test]
fn start_new_task_enters_title_mode() {
    let mut app = make_app();
    app.update(Message::StartNewTask);
    assert_eq!(app.input.mode, InputMode::InputTitle);
    assert!(app.input.buffer.is_empty());
    assert!(app.input.task_draft.is_none());
    assert_eq!(app.status_message.as_deref(), Some("Enter title: "));
}

#[test]
fn cancel_input_returns_to_normal() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.input.mode = InputMode::InputTitle;
    app.input.buffer = "partial".to_string();
    app.input.task_draft = Some(TaskDraft::default());
    app.status_message = Some("Enter title: ".to_string());
    app.update(Message::CancelInput);
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.input.buffer.is_empty());
    assert!(app.input.task_draft.is_none());
    assert!(app.status_message.is_none());
}

#[test]
fn submit_title_with_text_advances_to_description() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.input.mode = InputMode::InputTitle;
    app.update(Message::SubmitTitle("My Task".to_string()));
    assert_eq!(app.input.mode, InputMode::InputDescription);
    assert_eq!(app.input.task_draft.as_ref().unwrap().title, "My Task");
    assert_eq!(app.status_message.as_deref(), Some("Enter description: "));
}

#[test]
fn submit_empty_title_cancels() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.input.mode = InputMode::InputTitle;
    app.update(Message::SubmitTitle(String::new()));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.input.task_draft.is_none());
}

#[test]
fn submit_description_advances_to_repo_path() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.input.mode = InputMode::InputDescription;
    app.input.task_draft = Some(TaskDraft { title: "T".to_string(), ..Default::default() });
    app.update(Message::SubmitDescription("my desc".to_string()));
    assert_eq!(app.input.mode, InputMode::InputRepoPath);
    assert_eq!(app.input.task_draft.as_ref().unwrap().description, "my desc");
}

#[test]
fn submit_repo_path_creates_task() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.input.mode = InputMode::InputRepoPath;
    app.input.task_draft = Some(TaskDraft { title: "T".to_string(), description: "D".to_string(), ..Default::default() });
    let cmds = app.update(Message::SubmitRepoPath("/my/repo".to_string()));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(c, Command::InsertTask { ref draft, .. } if draft.repo_path == "/my/repo")));
}

#[test]
fn input_char_appends_to_buffer() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.input.mode = InputMode::InputTitle;
    app.update(Message::InputChar('H'));
    app.update(Message::InputChar('i'));
    assert_eq!(app.input.buffer, "Hi");
}

#[test]
fn input_backspace_removes_last_char() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.input.buffer = "abc".to_string();
    app.update(Message::InputBackspace);
    assert_eq!(app.input.buffer, "ab");
}

#[test]
fn confirm_delete_start_enters_mode() {
    let mut app = make_app();
    app.update(Message::ConfirmDeleteStart);
    assert_eq!(app.input.mode, InputMode::ConfirmDelete);
    // make_app() selects column 0, row 0 = Task 1 (Backlog)
    assert_eq!(
        app.status_message.as_deref(),
        Some("Delete \"Task 1\" [backlog]? (y/n)")
    );
}

#[test]
fn cancel_delete_returns_to_normal() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.input.mode = InputMode::ConfirmDelete;
    app.status_message = Some("Delete \"Task 1\" [backlog]? (y/n)".to_string());
    app.update(Message::CancelDelete);
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.status_message.is_none());
}

#[test]
fn status_info_sets_message() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.update(Message::StatusInfo("hello".to_string()));
    assert_eq!(app.status_message.as_deref(), Some("hello"));
}

#[test]
fn start_quick_dispatch_selection_enters_mode() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.update(Message::StartQuickDispatchSelection);
    assert_eq!(app.input.mode, InputMode::QuickDispatch);
    assert!(app.status_message.as_deref().unwrap().contains("Select repo"));
}

#[test]
fn select_quick_dispatch_repo_dispatches() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.repo_paths = vec!["/repo1".to_string(), "/repo2".to_string()];
    let cmds = app.update(Message::SelectQuickDispatchRepo(1));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(c, Command::QuickDispatch(ref d) if d.repo_path == "/repo2")));
}

#[test]
fn select_quick_dispatch_repo_out_of_range_is_noop() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.repo_paths = vec!["/repo1".to_string()];
    app.input.mode = InputMode::QuickDispatch;
    let cmds = app.update(Message::SelectQuickDispatchRepo(5));
    assert!(cmds.is_empty());
    // Mode is not changed by the handler (stays as-is)
}

#[test]
fn cancel_retry_returns_to_normal() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.input.mode = InputMode::ConfirmRetry(TaskId(4));
    app.status_message = Some("Agent stale".to_string());
    app.update(Message::CancelRetry);
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.status_message.is_none());
}

// --- Archive ---

#[test]
fn archive_task_sets_status_and_emits_persist() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Done),
    ], Duration::from_secs(300));
    let cmds = app.update(Message::ArchiveTask(TaskId(1)));
    let task = app.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Archived);
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistTask(_))));
}

#[test]
fn archive_task_with_worktree_emits_cleanup() {
    let mut task = make_task(1, TaskStatus::Running);
    task.worktree = Some("/wt/1-test".to_string());
    task.tmux_window = Some("dev:1-test".to_string());
    let mut app = App::new(vec![task], Duration::from_secs(300));

    let cmds = app.update(Message::ArchiveTask(TaskId(1)));

    assert!(cmds.iter().any(|c| matches!(c, Command::Cleanup { .. })));
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistTask(_))));
    let task = app.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Archived);
    assert!(task.worktree.is_none());
    assert!(task.tmux_window.is_none());
}

#[test]
fn archive_task_without_worktree_no_cleanup() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
    ], Duration::from_secs(300));
    let cmds = app.update(Message::ArchiveTask(TaskId(1)));
    assert!(!cmds.iter().any(|c| matches!(c, Command::Cleanup { .. })));
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistTask(_))));
}

#[test]
fn archive_clears_agent_tracking() {
    let mut task = make_task(1, TaskStatus::Running);
    task.tmux_window = Some("dev:1-test".to_string());
    let mut app = App::new(vec![task], Duration::from_secs(300));
    app.agents.stale_tasks.insert(TaskId(1));
    app.agents.crashed_tasks.insert(TaskId(1));
    app.agents.tmux_outputs.insert(TaskId(1), "output".to_string());

    app.update(Message::ArchiveTask(TaskId(1)));

    assert!(!app.agents.stale_tasks.contains(&TaskId(1)));
    assert!(!app.agents.crashed_tasks.contains(&TaskId(1)));
    assert!(!app.agents.tmux_outputs.contains_key(&TaskId(1)));
}

// --- Archive panel key handling ---

#[test]
fn archive_panel_j_k_navigation() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Archived),
        make_task(2, TaskStatus::Archived),
        make_task(3, TaskStatus::Archived),
    ], Duration::from_secs(300));
    app.archive.visible = true;
    assert_eq!(app.archive.selected_row, 0);

    app.handle_key(make_key(KeyCode::Char('j')));
    assert_eq!(app.archive.selected_row, 1);

    app.handle_key(make_key(KeyCode::Char('j')));
    assert_eq!(app.archive.selected_row, 2);

    // Clamp at end
    app.handle_key(make_key(KeyCode::Char('j')));
    assert_eq!(app.archive.selected_row, 2);

    app.handle_key(make_key(KeyCode::Char('k')));
    assert_eq!(app.archive.selected_row, 1);
}

#[test]
fn archive_panel_h_closes() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Archived),
    ], Duration::from_secs(300));
    app.archive.visible = true;

    app.handle_key(KeyEvent::new(KeyCode::Char('H'), KeyModifiers::SHIFT));
    assert!(!app.archive.visible);
}

#[test]
fn archive_panel_x_enters_confirm_delete() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Archived),
    ], Duration::from_secs(300));
    app.archive.visible = true;

    app.handle_key(make_key(KeyCode::Char('x')));
    assert_eq!(app.input.mode, InputMode::ConfirmDelete);
    assert_eq!(
        app.status_message.as_deref(),
        Some("Delete \"Task 1\"? (y/n)")
    );
}

#[test]
fn archive_panel_confirm_delete_removes_task() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Archived),
    ], Duration::from_secs(300));
    app.archive.visible = true;

    app.handle_key(make_key(KeyCode::Char('x')));
    let cmds = app.handle_key(make_key(KeyCode::Char('y')));
    assert!(app.tasks.is_empty());
    assert!(cmds.iter().any(|c| matches!(c, Command::DeleteTask(TaskId(1)))));
}

#[test]
fn archived_tasks_not_in_kanban_columns() {
    let app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Archived),
    ], Duration::from_secs(300));

    for &status in TaskStatus::ALL {
        let tasks = app.tasks_by_status(status);
        for t in &tasks {
            assert_ne!(t.status, TaskStatus::Archived,
                "archived task should not appear in {} column", status.as_str());
        }
    }

    let archived = app.archived_tasks();
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].id, TaskId(2));
}

// --- End-to-end archive flow ---

#[test]
fn full_archive_flow() {
    // Create a running task with worktree
    let mut task = make_task(1, TaskStatus::Running);
    task.worktree = Some("/wt/1-test".to_string());
    task.tmux_window = Some("dev:1-test".to_string());
    let mut app = App::new(vec![task, make_task(2, TaskStatus::Backlog)], Duration::from_secs(300));

    // Navigate to Running column (column 2)
    app.handle_key(make_key(KeyCode::Right));
    app.handle_key(make_key(KeyCode::Right));

    // Press x to archive
    app.handle_key(make_key(KeyCode::Char('x')));
    assert_eq!(app.input.mode, InputMode::ConfirmArchive);

    // Confirm
    let cmds = app.handle_key(make_key(KeyCode::Char('y')));
    assert_eq!(app.input.mode, InputMode::Normal);

    // Task should be archived with cleanup
    let task = app.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Archived);
    assert!(task.worktree.is_none());
    assert!(cmds.iter().any(|c| matches!(c, Command::Cleanup { .. })));

    // Toggle archive panel
    app.handle_key(KeyEvent::new(KeyCode::Char('H'), KeyModifiers::SHIFT));
    assert!(app.archive.visible);

    // Should see 1 archived task
    assert_eq!(app.archived_tasks().len(), 1);

    // Hard delete from archive
    app.handle_key(make_key(KeyCode::Char('x')));
    assert_eq!(app.input.mode, InputMode::ConfirmDelete);

    let cmds = app.handle_key(make_key(KeyCode::Char('y')));
    assert!(cmds.iter().any(|c| matches!(c, Command::DeleteTask(TaskId(1)))));
    assert!(app.archived_tasks().is_empty());
}

// -----------------------------------------------------------------------
// Batch selection tests
// -----------------------------------------------------------------------

#[test]
fn space_toggles_task_selection() {
    let mut app = make_app();
    // Select task 1 in Backlog
    app.handle_key(make_key(KeyCode::Char(' ')));
    assert!(app.selected_tasks.contains(&TaskId(1)));

    // Toggle off
    app.handle_key(make_key(KeyCode::Char(' ')));
    assert!(!app.selected_tasks.contains(&TaskId(1)));
}

#[test]
fn space_on_empty_column_is_noop() {
    let mut app = make_app();
    // Navigate to Review column (empty)
    app.update(Message::NavigateColumn(3));
    app.handle_key(make_key(KeyCode::Char(' ')));
    assert!(app.selected_tasks.is_empty());
}

#[test]
fn esc_clears_selection() {
    let mut app = make_app();
    app.update(Message::ToggleSelect(TaskId(1)));
    app.update(Message::ToggleSelect(TaskId(2)));
    assert_eq!(app.selected_tasks.len(), 2);

    app.handle_key(make_key(KeyCode::Esc));
    assert!(app.selected_tasks.is_empty());
}

#[test]
fn esc_with_no_selection_is_noop() {
    let mut app = make_app();
    let cmds = app.handle_key(make_key(KeyCode::Esc));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn batch_move_forward_moves_all_selected() {
    let mut app = make_app();
    // Select both Backlog tasks
    app.update(Message::ToggleSelect(TaskId(1)));
    app.update(Message::ToggleSelect(TaskId(2)));

    // Press m to batch move forward
    let cmds = app.handle_key(make_key(KeyCode::Char('m')));

    // Both should now be Ready
    assert_eq!(app.find_task(TaskId(1)).unwrap().status, TaskStatus::Ready);
    assert_eq!(app.find_task(TaskId(2)).unwrap().status, TaskStatus::Ready);
    // Should have PersistTask commands
    let persist_count = cmds.iter().filter(|c| matches!(c, Command::PersistTask(_))).count();
    assert_eq!(persist_count, 2);
}

#[test]
fn batch_move_preserves_selection() {
    let mut app = make_app();
    app.update(Message::ToggleSelect(TaskId(1)));
    app.update(Message::ToggleSelect(TaskId(2)));

    app.handle_key(make_key(KeyCode::Char('m')));

    // Selection should persist after move
    assert_eq!(app.selected_tasks.len(), 2);
    assert!(app.selected_tasks.contains(&TaskId(1)));
    assert!(app.selected_tasks.contains(&TaskId(2)));
}

#[test]
fn batch_move_multiple_steps() {
    let mut app = make_app();
    app.update(Message::ToggleSelect(TaskId(1)));
    app.update(Message::ToggleSelect(TaskId(2)));

    // Move twice: Backlog -> Ready -> Running
    app.handle_key(make_key(KeyCode::Char('m')));
    app.handle_key(make_key(KeyCode::Char('m')));

    assert_eq!(app.find_task(TaskId(1)).unwrap().status, TaskStatus::Running);
    assert_eq!(app.find_task(TaskId(2)).unwrap().status, TaskStatus::Running);
}

#[test]
fn batch_move_backward() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Done),
        make_task(2, TaskStatus::Done),
        make_task(3, TaskStatus::Done),
    ], Duration::from_secs(300));

    app.update(Message::ToggleSelect(TaskId(1)));
    app.update(Message::ToggleSelect(TaskId(2)));

    app.handle_key(make_key(KeyCode::Char('M')));

    assert_eq!(app.find_task(TaskId(1)).unwrap().status, TaskStatus::Review);
    assert_eq!(app.find_task(TaskId(2)).unwrap().status, TaskStatus::Review);
    // Task 3 not selected, should remain Done
    assert_eq!(app.find_task(TaskId(3)).unwrap().status, TaskStatus::Done);
}

#[test]
fn batch_archive_archives_all_and_clears_selection() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Done),
        make_task(2, TaskStatus::Done),
        make_task(3, TaskStatus::Ready),
    ], Duration::from_secs(300));

    app.update(Message::ToggleSelect(TaskId(1)));
    app.update(Message::ToggleSelect(TaskId(2)));

    let cmds = app.update(Message::BatchArchiveTasks(vec![TaskId(1), TaskId(2)]));

    assert_eq!(app.find_task(TaskId(1)).unwrap().status, TaskStatus::Archived);
    assert_eq!(app.find_task(TaskId(2)).unwrap().status, TaskStatus::Archived);
    assert_eq!(app.find_task(TaskId(3)).unwrap().status, TaskStatus::Ready);
    // Selection should be cleared after archive
    assert!(app.selected_tasks.is_empty());
    // Should have PersistTask commands
    let persist_count = cmds.iter().filter(|c| matches!(c, Command::PersistTask(_))).count();
    assert_eq!(persist_count, 2);
}

#[test]
fn x_key_with_selection_shows_count_in_confirm() {
    let mut app = make_app();
    app.update(Message::ToggleSelect(TaskId(1)));
    app.update(Message::ToggleSelect(TaskId(2)));

    app.handle_key(make_key(KeyCode::Char('x')));
    assert_eq!(app.input.mode, InputMode::ConfirmArchive);
    assert_eq!(app.status_message.as_deref(), Some("Archive 2 tasks? (y/n)"));
}

#[test]
fn confirm_archive_with_selection_dispatches_batch() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Done),
        make_task(2, TaskStatus::Done),
    ], Duration::from_secs(300));

    app.update(Message::ToggleSelect(TaskId(1)));
    app.update(Message::ToggleSelect(TaskId(2)));
    app.input.mode = InputMode::ConfirmArchive;

    app.handle_key(make_key(KeyCode::Char('y')));

    assert_eq!(app.find_task(TaskId(1)).unwrap().status, TaskStatus::Archived);
    assert_eq!(app.find_task(TaskId(2)).unwrap().status, TaskStatus::Archived);
    assert!(app.selected_tasks.is_empty());
}

#[test]
fn single_task_operations_work_without_selection() {
    let mut app = make_app();
    assert!(app.selected_tasks.is_empty());

    // Single move should still work
    let cmds = app.handle_key(make_key(KeyCode::Char('m')));
    assert_eq!(app.find_task(TaskId(1)).unwrap().status, TaskStatus::Ready);
    assert!(!cmds.is_empty());
}

#[test]
fn refresh_tasks_prunes_stale_selections() {
    let mut app = make_app();
    app.update(Message::ToggleSelect(TaskId(1)));
    app.update(Message::ToggleSelect(TaskId(99))); // non-existent

    // Refresh with only task 1
    app.update(Message::RefreshTasks(vec![make_task(1, TaskStatus::Backlog)]));

    assert!(app.selected_tasks.contains(&TaskId(1)));
    assert!(!app.selected_tasks.contains(&TaskId(99)));
}

// ---------------------------------------------------------------------------
// Rendering tests
// ---------------------------------------------------------------------------

#[test]
fn render_empty_board_shows_all_column_headers() {
    let app = App::new(vec![], Duration::from_secs(300));
    let buf = render_to_buffer(&app, 100, 20);
    assert!(buffer_contains(&buf, "backlog"));
    assert!(buffer_contains(&buf, "ready"));
    assert!(buffer_contains(&buf, "running"));
    assert!(buffer_contains(&buf, "review"));
    assert!(buffer_contains(&buf, "done"));
}

#[test]
fn render_shows_task_titles_in_columns() {
    let tasks = vec![
        make_task(1, TaskStatus::Backlog),
        make_task(2, TaskStatus::Ready),
        make_task(3, TaskStatus::Running),
    ];
    let app = App::new(tasks, Duration::from_secs(300));
    let buf = render_to_buffer(&app, 120, 20);
    assert!(buffer_contains(&buf, "Task 1"));
    assert!(buffer_contains(&buf, "Task 2"));
    assert!(buffer_contains(&buf, "Task 3"));
}

#[test]
fn render_error_popup_shows_message() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.update(Message::Error("Something went wrong".to_string()));
    let buf = render_to_buffer(&app, 100, 20);
    assert!(buffer_contains(&buf, "Something went wrong"));
}

#[test]
fn render_status_bar_shows_keybindings() {
    let app = App::new(vec![], Duration::from_secs(300));
    let buf = render_to_buffer(&app, 100, 20);
    assert!(buffer_contains(&buf, "uit"));
}

#[test]
fn render_crashed_task_shows_label() {
    let mut task = make_task(1, TaskStatus::Running);
    task.tmux_window = Some("win-1".to_string());
    let mut app = App::new(vec![task], Duration::from_secs(300));
    app.agents.crashed_tasks.insert(TaskId(1));
    let buf = render_to_buffer(&app, 120, 20);
    assert!(buffer_contains(&buf, "crashed"));
}

#[test]
fn render_stale_task_shows_label() {
    let mut task = make_task(1, TaskStatus::Running);
    task.tmux_window = Some("win-1".to_string());
    let mut app = App::new(vec![task], Duration::from_secs(300));
    app.agents.stale_tasks.insert(TaskId(1));
    let buf = render_to_buffer(&app, 120, 20);
    assert!(buffer_contains(&buf, "stale"));
}

#[test]
fn render_does_not_panic_on_small_terminal() {
    let app = App::new(vec![make_task(1, TaskStatus::Backlog)], Duration::from_secs(300));
    // Very small terminal — should not panic
    let _ = render_to_buffer(&app, 20, 5);
}

#[test]
fn render_input_mode_shows_prompt() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.update(Message::StartNewTask);
    let buf = render_to_buffer(&app, 100, 20);
    assert!(buffer_contains(&buf, "Title"));
}

#[test]
fn truncate_respects_max_length() {
    assert_eq!(ui::truncate("short", 10), "short");
    assert_eq!(ui::truncate("hello world this is long", 10).chars().count(), 10);
    assert!(ui::truncate("hello world this is long", 10).ends_with('…'));
}

// ---------------------------------------------------------------------------
// Rendering tests — v2.0 cosmetic redesign
// ---------------------------------------------------------------------------

#[test]
fn render_v2_task_card_shows_stripe() {
    let app = App::new(vec![make_task(1, TaskStatus::Backlog)], Duration::from_secs(300));
    let buf = render_to_buffer(&app, 120, 20);
    assert!(buffer_contains(&buf, "\u{258e}"), "task card should have stripe character");
}

#[test]
fn render_v2_backlog_task_shows_status_icon() {
    let app = App::new(vec![make_task(1, TaskStatus::Backlog)], Duration::from_secs(300));
    let buf = render_to_buffer(&app, 120, 20);
    assert!(buffer_contains(&buf, "\u{25e6}"), "backlog task should show \u{25e6} icon");
}

#[test]
fn render_v2_running_task_shows_status_icon() {
    let mut task = make_task(1, TaskStatus::Running);
    task.tmux_window = Some("win-1".to_string());
    let app = App::new(vec![task], Duration::from_secs(300));
    let buf = render_to_buffer(&app, 120, 20);
    assert!(buffer_contains(&buf, "\u{25c9}"), "running task should show \u{25c9} icon");
}

#[test]
fn render_v2_focused_column_shows_arrow() {
    let app = App::new(vec![], Duration::from_secs(300));
    let buf = render_to_buffer(&app, 120, 20);
    // Default focus is on first column (Backlog), should show \u{25b8}
    assert!(buffer_contains(&buf, "\u{25b8}"), "focused column should show \u{25b8} indicator");
}

#[test]
fn render_v2_unfocused_columns_show_dot() {
    let app = App::new(vec![], Duration::from_secs(300));
    let buf = render_to_buffer(&app, 120, 20);
    // Unfocused columns should show \u{25e6}
    assert!(buffer_contains(&buf, "\u{25e6}"), "unfocused columns should show \u{25e6} indicator");
}

#[test]
fn render_v2_detail_panel_shows_inline_metadata() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], Duration::from_secs(300));
    app.update(Message::ToggleDetail);
    let buf = render_to_buffer(&app, 120, 20);
    // The compact detail panel shows "title \u{00b7} #id \u{00b7} status \u{00b7} repo" on one line
    // Check for the middle-dot separator which is new in v2
    assert!(buffer_contains(&buf, "\u{00b7}"), "detail panel should use \u{00b7} separator");
    assert!(buffer_contains(&buf, "#1"), "detail panel should show task ID with # prefix");
}

#[test]
fn render_v2_status_bar_no_brackets() {
    let app = App::new(vec![make_task(1, TaskStatus::Backlog)], Duration::from_secs(300));
    let buf = render_to_buffer(&app, 120, 20);
    let content: String = buf.content().iter().map(|cell| cell.symbol()).collect();
    // Old format had [n], [q] etc. New format should NOT have brackets
    assert!(!content.contains("[n]"), "status bar should not use bracket format");
    assert!(!content.contains("[q]"), "status bar should not use bracket format");
    // But should still contain the action words
    assert!(buffer_contains(&buf, "new"), "status bar should show 'new' hint");
    assert!(buffer_contains(&buf, "quit"), "status bar should show 'quit' hint");
}

#[test]
fn render_v2_done_task_shows_checkmark() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Done)], Duration::from_secs(300));
    // Navigate to Done column (column index 4)
    for _ in 0..4 {
        app.update(Message::NavigateColumn(1));
    }
    let buf = render_to_buffer(&app, 120, 20);
    assert!(buffer_contains(&buf, "\u{2713}"), "done task should show \u{2713} icon");
}

// ---------------------------------------------------------------------------
// Stress tests
// ---------------------------------------------------------------------------

#[test]
fn stress_large_task_list_navigation() {
    let tasks: Vec<_> = (1..=1000).map(|i| make_task(i, TaskStatus::Backlog)).collect();
    let mut app = App::new(tasks, Duration::from_secs(300));

    assert_eq!(app.tasks().len(), 1000);

    // Navigate through all rows
    for _ in 0..999 {
        app.update(Message::NavigateRow(1));
    }
    assert_eq!(app.selected_row()[0], 999);

    // Navigate back
    for _ in 0..999 {
        app.update(Message::NavigateRow(-1));
    }
    assert_eq!(app.selected_row()[0], 0);
}

#[test]
fn stress_large_task_list_rendering() {
    let mut tasks: Vec<_> = (1..=200).map(|i| make_task(i, TaskStatus::Backlog)).collect();
    // Spread tasks across all columns
    for (i, task) in tasks.iter_mut().enumerate() {
        task.status = match i % 5 {
            0 => TaskStatus::Backlog,
            1 => TaskStatus::Ready,
            2 => TaskStatus::Running,
            3 => TaskStatus::Review,
            _ => TaskStatus::Done,
        };
    }
    let app = App::new(tasks, Duration::from_secs(300));

    // Render at various sizes — must not panic
    for width in [40, 80, 120, 200] {
        for height in [10, 24, 50] {
            let _ = render_to_buffer(&app, width, height);
        }
    }
}

#[test]
fn stress_rapid_status_transitions() {
    let tasks = vec![make_task(1, TaskStatus::Backlog)];
    let mut app = App::new(tasks, Duration::from_secs(300));

    // Rapidly move task through all statuses and back.
    // Moving forward will stop at Review because Done requires confirmation.
    for _ in 0..100 {
        app.update(Message::MoveTask {
            id: TaskId(1),
            direction: MoveDirection::Forward,
        });
    }
    // Should be at Review (blocked by Done confirmation)
    assert_eq!(app.tasks()[0].status, TaskStatus::Review);
    assert!(matches!(app.input.mode, InputMode::ConfirmDone(TaskId(1))));

    // Confirm the Done transition
    app.update(Message::ConfirmDone);
    assert_eq!(app.tasks()[0].status, TaskStatus::Done);

    for _ in 0..100 {
        app.update(Message::MoveTask {
            id: TaskId(1),
            direction: MoveDirection::Backward,
        });
    }
    // Should be at Backlog (clamped)
    assert_eq!(app.tasks()[0].status, TaskStatus::Backlog);
}

#[test]
fn stress_db_with_many_tasks() {
    let db = crate::db::Database::open_in_memory().unwrap();
    use crate::db::TaskStore;
    for i in 0..500 {
        db.create_task(
            &format!("Task {i}"),
            "stress test",
            "/repo",
            None,
            TaskStatus::Backlog,
        )
        .unwrap();
    }
    let tasks = db.list_all().unwrap();
    assert_eq!(tasks.len(), 500);

    // Create app from DB tasks and verify navigation works
    let mut app = App::new(tasks, Duration::from_secs(300));
    for _ in 0..499 {
        app.update(Message::NavigateRow(1));
    }
    assert_eq!(app.selected_row()[0], 499);
}

// --- Epic helpers ---

fn make_epic(id: i64) -> Epic {
    let now = chrono::Utc::now();
    Epic {
        id: EpicId(id),
        title: format!("Epic {id}"),
        description: String::new(),
        plan: String::new(),
        repo_path: "/repo".to_string(),
        done: false,
        created_at: now,
        updated_at: now,
    }
}

// --- tasks_for_current_view ---

#[test]
fn tasks_for_current_view_board_excludes_epic_tasks() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    let standalone = make_task(1, TaskStatus::Backlog);
    let mut subtask = make_task(2, TaskStatus::Backlog);
    subtask.epic_id = Some(EpicId(10));
    app.tasks = vec![standalone, subtask];

    let visible = app.tasks_for_current_view();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].id, TaskId(1));
}

#[test]
fn tasks_for_current_view_epic_shows_only_subtasks() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    let standalone = make_task(1, TaskStatus::Backlog);
    let mut subtask = make_task(2, TaskStatus::Ready);
    subtask.epic_id = Some(EpicId(10));
    app.tasks = vec![standalone, subtask];

    app.view_mode = ViewMode::Epic {
        epic_id: EpicId(10),
        selection: BoardSelection::new(),
        saved_board: BoardSelection::new(),
    };

    let visible = app.tasks_for_current_view();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].id, TaskId(2));
}

// --- enter/exit epic ---

#[test]
fn enter_epic_switches_to_epic_view() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.epics = vec![make_epic(10)];
    app.selection_mut().set_column(2);

    app.update(Message::EnterEpic(EpicId(10)));

    match &app.view_mode {
        ViewMode::Epic { epic_id, saved_board, .. } => {
            assert_eq!(*epic_id, EpicId(10));
            assert_eq!(saved_board.column(), 2, "board selection should be preserved");
        }
        _ => panic!("Expected ViewMode::Epic"),
    }
}

#[test]
fn exit_epic_restores_board_selection() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.selection_mut().set_column(3);

    app.update(Message::EnterEpic(EpicId(10)));
    app.selection_mut().set_column(1);

    app.update(Message::ExitEpic);

    match &app.view_mode {
        ViewMode::Board(sel) => {
            assert_eq!(sel.column(), 3, "board selection should be restored");
        }
        _ => panic!("Expected ViewMode::Board"),
    }
}

#[test]
fn exit_epic_when_on_board_is_noop() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.update(Message::ExitEpic);
    assert!(matches!(app.view_mode, ViewMode::Board(_)));
}

// --- ColumnItem ---

#[test]
fn column_items_board_view_includes_epics() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
    ], Duration::from_secs(300));
    app.epics = vec![make_epic(10)]; // epic with no subtasks = Backlog

    let items = app.column_items_for_status(TaskStatus::Backlog);
    assert_eq!(items.len(), 2); // 1 task + 1 epic
    assert!(matches!(items[0], ColumnItem::Task(_)));
    assert!(matches!(items[1], ColumnItem::Epic(_)));
}

#[test]
fn column_items_epic_view_no_epics() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.view_mode = ViewMode::Epic {
        epic_id: EpicId(10),
        selection: BoardSelection::new(),
        saved_board: BoardSelection::new(),
    };
    app.epics = vec![make_epic(10)];

    let items = app.column_items_for_status(TaskStatus::Backlog);
    assert!(items.iter().all(|i| matches!(i, ColumnItem::Task(_))));
}

#[test]
fn selected_column_item_returns_epic() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
    ], Duration::from_secs(300));
    app.epics = vec![make_epic(10)];

    // Task is at row 0, Epic at row 1
    app.selection_mut().set_column(0);
    app.selection_mut().set_row(0, 1);

    match app.selected_column_item() {
        Some(ColumnItem::Epic(e)) => assert_eq!(e.id, EpicId(10)),
        other => panic!("Expected Epic, got {:?}", other),
    }
}

// --- Epic CRUD ---

#[test]
fn start_new_epic_sets_input_mode() {
    let mut app = make_app();
    app.update(Message::StartNewEpic);
    assert_eq!(*app.mode(), InputMode::InputEpicTitle);
}

#[test]
fn epic_created_adds_to_state() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    let epic = make_epic(1);
    app.update(Message::EpicCreated(epic));
    assert_eq!(app.epics().len(), 1);
}

#[test]
fn delete_epic_removes_from_state_and_tasks() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.epics = vec![make_epic(10)];
    let mut subtask = make_task(1, TaskStatus::Backlog);
    subtask.epic_id = Some(EpicId(10));
    app.tasks = vec![subtask, make_task(2, TaskStatus::Backlog)];

    let cmds = app.update(Message::DeleteEpic(EpicId(10)));
    assert!(app.epics.is_empty());
    assert_eq!(app.tasks.len(), 1);
    assert_eq!(app.tasks[0].id, TaskId(2));
    assert!(cmds.iter().any(|c| matches!(c, Command::DeleteEpic(id) if *id == EpicId(10))));
}

#[test]
fn mark_epic_done() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.epics = vec![make_epic(10)];
    let cmds = app.update(Message::MarkEpicDone(EpicId(10)));
    assert!(app.epics[0].done);
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistEpic { .. })));
}

// ---------------------------------------------------------------------------
// input.rs — Normal mode: Epic interactions
// ---------------------------------------------------------------------------

/// Helper: create an app with one task + one epic in Backlog, cursor on the epic.
fn make_app_with_epic_selected() -> App {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
    ], Duration::from_secs(300));
    app.epics = vec![make_epic(10)];
    // Task at row 0, Epic at row 1 in Backlog column
    app.selection_mut().set_column(0);
    app.selection_mut().set_row(0, 1);
    app
}

#[test]
fn m_key_on_epic_shows_status_info() {
    let mut app = make_app_with_epic_selected();
    let cmds = app.handle_key(make_key(KeyCode::Char('m')));
    assert!(cmds.is_empty());
    assert!(app.status_message.as_deref().unwrap().contains("derived from subtasks"));
}

#[test]
fn shift_m_key_on_epic_shows_status_info() {
    let mut app = make_app_with_epic_selected();
    let cmds = app.handle_key(make_key(KeyCode::Char('M')));
    assert!(cmds.is_empty());
    assert!(app.status_message.as_deref().unwrap().contains("derived from subtasks"));
}

#[test]
fn shift_e_key_starts_new_epic() {
    let mut app = make_app();
    let cmds = app.handle_key(make_key(KeyCode::Char('E')));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::InputEpicTitle);
}

#[test]
fn shift_v_key_on_epic_marks_done() {
    let mut app = make_app_with_epic_selected();
    let cmds = app.handle_key(make_key(KeyCode::Char('V')));
    assert!(app.epics[0].done);
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistEpic { .. })));
}

#[test]
fn shift_v_key_on_task_is_noop() {
    let mut app = make_app();
    app.selection_mut().set_column(0);
    let cmds = app.handle_key(make_key(KeyCode::Char('V')));
    assert!(cmds.is_empty());
}

#[test]
fn shift_v_key_on_empty_column_is_noop() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.selection_mut().set_column(0);
    let cmds = app.handle_key(make_key(KeyCode::Char('V')));
    assert!(cmds.is_empty());
}

#[test]
fn x_key_on_epic_enters_confirm_archive_epic() {
    let mut app = make_app_with_epic_selected();
    let cmds = app.handle_key(make_key(KeyCode::Char('x')));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::ConfirmArchiveEpic);
    assert!(app.status_message.as_deref().unwrap().contains("Archive epic"));
}

#[test]
fn enter_key_on_epic_enters_epic_view() {
    let mut app = make_app_with_epic_selected();
    app.handle_key(make_key(KeyCode::Enter));
    assert!(matches!(app.view_mode, ViewMode::Epic { epic_id: EpicId(10), .. }));
}

#[test]
fn e_key_in_epic_view_edits_epic() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.epics = vec![make_epic(10)];
    app.view_mode = ViewMode::Epic {
        epic_id: EpicId(10),
        selection: BoardSelection::new(),
        saved_board: BoardSelection::new(),
    };
    let cmds = app.handle_key(make_key(KeyCode::Char('e')));
    assert_eq!(cmds.len(), 1);
    assert!(matches!(&cmds[0], Command::EditEpicInEditor(e) if e.id == EpicId(10)));
}

#[test]
fn esc_in_epic_view_exits_to_board() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.view_mode = ViewMode::Epic {
        epic_id: EpicId(10),
        selection: BoardSelection::new(),
        saved_board: BoardSelection::new(),
    };
    app.handle_key(make_key(KeyCode::Esc));
    assert!(matches!(app.view_mode, ViewMode::Board(_)));
}

// ---------------------------------------------------------------------------
// input.rs — Normal mode: Arrow key variants
// ---------------------------------------------------------------------------

#[test]
fn left_arrow_navigates_column() {
    let mut app = make_app();
    app.selection_mut().set_column(2);
    app.handle_key(make_key(KeyCode::Left));
    assert_eq!(app.selection().column(), 1);
}

#[test]
fn right_arrow_navigates_column() {
    let mut app = make_app();
    app.selection_mut().set_column(1);
    app.handle_key(make_key(KeyCode::Right));
    assert_eq!(app.selection().column(), 2);
}

#[test]
fn down_arrow_navigates_row() {
    let mut app = make_app();
    app.selection_mut().set_column(0); // Backlog has 2 tasks
    app.handle_key(make_key(KeyCode::Down));
    assert_eq!(app.selection().row(0), 1);
}

#[test]
fn up_arrow_navigates_row() {
    let mut app = make_app();
    app.selection_mut().set_column(0);
    app.selection_mut().set_row(0, 1);
    app.handle_key(make_key(KeyCode::Up));
    assert_eq!(app.selection().row(0), 0);
}

// ---------------------------------------------------------------------------
// input.rs — handle_key_epic_text_input
// ---------------------------------------------------------------------------

#[test]
fn epic_title_esc_cancels() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.input.mode = InputMode::InputEpicTitle;
    app.input.buffer = "partial".to_string();
    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.input.buffer.is_empty());
}

#[test]
fn epic_title_enter_with_text_advances_to_description() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.input.mode = InputMode::InputEpicTitle;
    app.input.buffer = "My Epic".to_string();
    app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.input.mode, InputMode::InputEpicDescription);
    assert!(app.input.buffer.is_empty());
    assert_eq!(app.input.epic_draft.as_ref().unwrap().title, "My Epic");
}

#[test]
fn epic_title_enter_empty_cancels() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.input.mode = InputMode::InputEpicTitle;
    app.input.buffer.clear();
    app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn epic_description_enter_advances_to_repo_path() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.input.mode = InputMode::InputEpicDescription;
    app.input.epic_draft = Some(EpicDraft { title: "E".to_string(), ..Default::default() });
    app.input.buffer = "epic desc".to_string();
    app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.input.mode, InputMode::InputEpicRepoPath);
    assert!(app.input.buffer.is_empty());
    assert_eq!(app.input.epic_draft.as_ref().unwrap().description, "epic desc");
}

#[test]
fn epic_repo_path_enter_with_text_completes() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.input.mode = InputMode::InputEpicRepoPath;
    app.input.epic_draft = Some(EpicDraft { title: "E".to_string(), description: "D".to_string(), ..Default::default() });
    app.input.buffer = "/my/repo".to_string();
    let cmds = app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(c, Command::InsertEpic(ref d) if d.repo_path == "/my/repo")));
}

#[test]
fn epic_repo_path_enter_empty_uses_saved_path() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.repo_paths = vec!["/saved".to_string()];
    app.input.mode = InputMode::InputEpicRepoPath;
    app.input.epic_draft = Some(EpicDraft { title: "E".to_string(), description: "D".to_string(), ..Default::default() });
    app.input.buffer.clear();
    let cmds = app.handle_key(make_key(KeyCode::Enter));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.iter().any(|c| matches!(c, Command::InsertEpic(ref d) if d.repo_path == "/saved")));
}

#[test]
fn epic_repo_path_enter_empty_no_saved_stays() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.repo_paths = vec![];
    app.input.mode = InputMode::InputEpicRepoPath;
    app.input.epic_draft = Some(EpicDraft { title: "E".to_string(), description: "D".to_string(), ..Default::default() });
    app.input.buffer.clear();
    let _cmds = app.handle_key(make_key(KeyCode::Enter));
    // Should stay in repo path mode since there's no fallback
    assert!(app.status_message.is_some());
}

#[test]
fn epic_text_input_char_appends() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.input.mode = InputMode::InputEpicTitle;
    app.handle_key(make_key(KeyCode::Char('A')));
    app.handle_key(make_key(KeyCode::Char('b')));
    assert_eq!(app.input.buffer, "Ab");
}

#[test]
fn epic_text_input_backspace_removes() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.input.mode = InputMode::InputEpicTitle;
    app.input.buffer = "abc".to_string();
    app.handle_key(make_key(KeyCode::Backspace));
    assert_eq!(app.input.buffer, "ab");
}

#[test]
fn epic_text_input_unrecognized_key_is_noop() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.input.mode = InputMode::InputEpicTitle;
    app.input.buffer = "x".to_string();
    let cmds = app.handle_key(make_key(KeyCode::Tab));
    assert!(cmds.is_empty());
    assert_eq!(app.input.buffer, "x");
    assert_eq!(app.input.mode, InputMode::InputEpicTitle);
}

// ---------------------------------------------------------------------------
// input.rs — handle_key_confirm_delete_epic
// ---------------------------------------------------------------------------

fn make_app_confirm_delete_epic() -> App {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
    ], Duration::from_secs(300));
    app.epics = vec![make_epic(10)];
    app.selection_mut().set_column(0);
    app.selection_mut().set_row(0, 1); // cursor on epic
    app.input.mode = InputMode::ConfirmDeleteEpic;
    app.status_message = Some("Delete epic \"Epic 10\" and subtasks? (y/n)".to_string());
    app
}

#[test]
fn confirm_delete_epic_enters_mode_with_title() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
    ], Duration::from_secs(300));
    app.epics = vec![make_epic(10)];
    app.selection_mut().set_column(0);
    app.selection_mut().set_row(0, 1); // cursor on epic
    app.update(Message::ConfirmDeleteEpic);
    assert_eq!(app.input.mode, InputMode::ConfirmDeleteEpic);
    assert_eq!(
        app.status_message.as_deref(),
        Some("Delete epic \"Epic 10\" and subtasks? (y/n)")
    );
}

#[test]
fn confirm_delete_epic_y_deletes() {
    let mut app = make_app_confirm_delete_epic();
    let cmds = app.handle_key(make_key(KeyCode::Char('y')));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.status_message.is_none());
    assert!(app.epics.is_empty());
    assert!(cmds.iter().any(|c| matches!(c, Command::DeleteEpic(id) if *id == EpicId(10))));
}

#[test]
fn confirm_delete_epic_uppercase_y_deletes() {
    let mut app = make_app_confirm_delete_epic();
    let cmds = app.handle_key(make_key(KeyCode::Char('Y')));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.epics.is_empty());
    assert!(cmds.iter().any(|c| matches!(c, Command::DeleteEpic(id) if *id == EpicId(10))));
}

#[test]
fn confirm_delete_epic_other_key_cancels() {
    let mut app = make_app_confirm_delete_epic();
    let cmds = app.handle_key(make_key(KeyCode::Char('n')));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.status_message.is_none());
    assert_eq!(app.epics.len(), 1); // not deleted
    assert!(cmds.is_empty());
}

#[test]
fn confirm_delete_epic_no_epic_selected_is_noop() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], Duration::from_secs(300));
    app.selection_mut().set_column(0); // cursor on task, not epic
    app.input.mode = InputMode::ConfirmDeleteEpic;
    let cmds = app.handle_key(make_key(KeyCode::Char('y')));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.is_empty()); // no deletion happened
}

// ---------------------------------------------------------------------------
// input.rs — handle_key_confirm_archive_epic
// ---------------------------------------------------------------------------

fn make_app_confirm_archive_epic() -> App {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Backlog),
    ], Duration::from_secs(300));
    app.epics = vec![make_epic(10)];
    app.selection_mut().set_column(0);
    app.selection_mut().set_row(0, 1); // cursor on epic
    app.input.mode = InputMode::ConfirmArchiveEpic;
    app.status_message = Some("Archive epic and all subtasks? (y/n)".to_string());
    app
}

#[test]
fn confirm_archive_epic_y_archives() {
    let mut app = make_app_confirm_archive_epic();
    let cmds = app.handle_key(make_key(KeyCode::Char('y')));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.status_message.is_none());
    assert!(app.epics.is_empty()); // removed
    assert!(cmds.iter().any(|c| matches!(c, Command::DeleteEpic(id) if *id == EpicId(10))));
}

#[test]
fn confirm_archive_epic_uppercase_y_archives() {
    let mut app = make_app_confirm_archive_epic();
    let cmds = app.handle_key(make_key(KeyCode::Char('Y')));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.epics.is_empty());
    assert!(cmds.iter().any(|c| matches!(c, Command::DeleteEpic(id) if *id == EpicId(10))));
}

#[test]
fn confirm_archive_epic_other_key_cancels() {
    let mut app = make_app_confirm_archive_epic();
    let cmds = app.handle_key(make_key(KeyCode::Char('n')));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.status_message.is_none());
    assert_eq!(app.epics.len(), 1); // not removed
    assert!(cmds.is_empty());
}

#[test]
fn confirm_archive_epic_no_epic_selected_is_noop() {
    let mut app = App::new(vec![make_task(1, TaskStatus::Backlog)], Duration::from_secs(300));
    app.selection_mut().set_column(0);
    app.input.mode = InputMode::ConfirmArchiveEpic;
    let cmds = app.handle_key(make_key(KeyCode::Char('y')));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(cmds.is_empty());
}

// ---------------------------------------------------------------------------
// input.rs — Archive panel extras
// ---------------------------------------------------------------------------

#[test]
fn archive_panel_down_arrow_navigates() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Archived),
        make_task(2, TaskStatus::Archived),
    ], Duration::from_secs(300));
    app.archive.visible = true;
    assert_eq!(app.archive.selected_row, 0);
    app.handle_key(make_key(KeyCode::Down));
    assert_eq!(app.archive.selected_row, 1);
}

#[test]
fn archive_panel_up_arrow_navigates() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Archived),
        make_task(2, TaskStatus::Archived),
    ], Duration::from_secs(300));
    app.archive.visible = true;
    app.archive.selected_row = 1;
    app.handle_key(make_key(KeyCode::Up));
    assert_eq!(app.archive.selected_row, 0);
}

#[test]
fn archive_panel_esc_closes() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Archived),
    ], Duration::from_secs(300));
    app.archive.visible = true;
    app.handle_key(make_key(KeyCode::Esc));
    assert!(!app.archive.visible);
}

#[test]
fn archive_panel_e_edits_task() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Archived),
    ], Duration::from_secs(300));
    app.archive.visible = true;
    let cmds = app.handle_key(make_key(KeyCode::Char('e')));
    assert_eq!(cmds.len(), 1);
    assert!(matches!(&cmds[0], Command::EditTaskInEditor(t) if t.id == TaskId(1)));
}

#[test]
fn archive_panel_e_on_empty_is_noop() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.archive.visible = true;
    let cmds = app.handle_key(make_key(KeyCode::Char('e')));
    assert!(cmds.is_empty());
}

#[test]
fn archive_panel_x_on_empty_is_noop() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.archive.visible = true;
    app.handle_key(make_key(KeyCode::Char('x')));
    assert_eq!(app.input.mode, InputMode::Normal); // did not enter ConfirmDelete
}

#[test]
fn archive_panel_q_quits() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Archived),
    ], Duration::from_secs(300));
    app.archive.visible = true;
    app.handle_key(make_key(KeyCode::Char('q')));
    assert!(app.should_quit);
}

#[test]
fn archive_panel_unrecognized_key_is_noop() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Archived),
    ], Duration::from_secs(300));
    app.archive.visible = true;
    let cmds = app.handle_key(make_key(KeyCode::Char('z')));
    assert!(cmds.is_empty());
    assert!(app.archive.visible);
}

// ---------------------------------------------------------------------------
// input.rs — Confirm archive extras
// ---------------------------------------------------------------------------

#[test]
fn confirm_archive_uppercase_y_archives() {
    let mut app = make_app();
    app.selection_mut().set_column(0);
    app.input.mode = InputMode::ConfirmArchive;
    app.handle_key(make_key(KeyCode::Char('Y')));
    assert_eq!(app.input.mode, InputMode::Normal);
    let task = app.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Archived);
}

#[test]
fn confirm_archive_esc_cancels() {
    let mut app = make_app();
    app.selection_mut().set_column(0);
    app.input.mode = InputMode::ConfirmArchive;
    app.status_message = Some("Archive task? (y/n)".to_string());
    let cmds = app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.status_message.is_none());
    assert!(cmds.is_empty());
    let task = app.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Backlog); // unchanged
}

// ---------------------------------------------------------------------------
// input.rs — Quick dispatch extras
// ---------------------------------------------------------------------------

#[test]
fn quick_dispatch_zero_is_noop() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.repo_paths = vec!["/repo".to_string()];
    app.input.mode = InputMode::QuickDispatch;
    let cmds = app.handle_key(make_key(KeyCode::Char('0')));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::QuickDispatch);
}

#[test]
fn quick_dispatch_non_digit_is_noop() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.repo_paths = vec!["/repo".to_string()];
    app.input.mode = InputMode::QuickDispatch;
    let cmds = app.handle_key(make_key(KeyCode::Char('a')));
    assert!(cmds.is_empty());
    assert_eq!(app.input.mode, InputMode::QuickDispatch);
}

// ---------------------------------------------------------------------------
// input.rs — Other edge cases
// ---------------------------------------------------------------------------

#[test]
fn confirm_retry_unrecognized_key_is_noop() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.input.mode = InputMode::ConfirmRetry(TaskId(4));
    let cmds = app.handle_key(make_key(KeyCode::Char('x')));
    assert!(cmds.is_empty());
    assert!(matches!(app.input.mode, InputMode::ConfirmRetry(TaskId(4))));
}

#[test]
fn normal_mode_unrecognized_key_is_noop() {
    let mut app = make_app();
    let cmds = app.handle_key(make_key(KeyCode::Char('z')));
    assert!(cmds.is_empty());
    assert!(!app.should_quit);
}

#[test]
fn text_input_unrecognized_key_is_noop() {
    let mut app = App::new(vec![], Duration::from_secs(300));
    app.input.mode = InputMode::InputTitle;
    app.input.buffer = "x".to_string();
    let cmds = app.handle_key(make_key(KeyCode::Tab));
    assert!(cmds.is_empty());
    assert_eq!(app.input.buffer, "x");
    assert_eq!(app.input.mode, InputMode::InputTitle);
}

#[test]
fn d_key_on_archived_shows_warning() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Archived),
    ], Duration::from_secs(300));
    // Archived tasks don't appear in columns, but test dispatch routing directly
    app.selection_mut().set_column(0);
    let cmds = app.handle_key(make_key(KeyCode::Char('d')));
    // No task selected (archived tasks hidden from kanban) → noop
    assert!(cmds.is_empty());
}

#[test]
fn question_mark_toggles_help_mode() {
    let mut app = make_app();
    assert_eq!(app.input.mode, InputMode::Normal);

    app.handle_key(make_key(KeyCode::Char('?')));
    assert_eq!(app.input.mode, InputMode::Help);
}

#[test]
fn question_mark_dismisses_help() {
    let mut app = make_app();
    app.input.mode = InputMode::Help;

    app.handle_key(make_key(KeyCode::Char('?')));
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn esc_dismisses_help() {
    let mut app = make_app();
    app.input.mode = InputMode::Help;

    app.handle_key(make_key(KeyCode::Esc));
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn help_mode_ignores_other_keys() {
    let mut app = make_app();
    app.input.mode = InputMode::Help;

    app.handle_key(make_key(KeyCode::Char('q')));
    assert_eq!(app.input.mode, InputMode::Help);
    assert!(!app.should_quit);
}

#[test]
fn help_overlay_renders_when_active() {
    let mut app = make_app();
    app.input.mode = InputMode::Help;

    let buf = render_to_buffer(&app, 80, 30);
    assert!(buffer_contains(&buf, "Navigation"));
    assert!(buffer_contains(&buf, "Actions"));
    assert!(buffer_contains(&buf, "General"));
}

#[test]
fn help_overlay_hidden_in_normal_mode() {
    let app = make_app();
    let buf = render_to_buffer(&app, 80, 30);
    assert!(!buffer_contains(&buf, "Navigation"));
}

// ---------------------------------------------------------------------------
// Finish task tests
// ---------------------------------------------------------------------------

#[test]
fn finish_task_on_review_with_worktree_emits_command() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t.tmux_window = Some("task-1".to_string());
        t
    }], Duration::from_secs(300));
    app.update(Message::NavigateColumn(3));

    // FinishTask enters confirm mode
    app.update(Message::FinishTask(TaskId(1)));
    assert!(matches!(app.input.mode, InputMode::ConfirmFinish(TaskId(1))));

    // ConfirmFinish emits Command::Finish
    let cmds = app.update(Message::ConfirmFinish);
    assert!(
        cmds.iter().any(|c| matches!(c, Command::Finish { .. })),
        "Expected Command::Finish, got: {cmds:?}"
    );
}

#[test]
fn finish_task_on_non_review_is_noop() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Running);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t
    }], Duration::from_secs(300));

    let cmds = app.update(Message::FinishTask(TaskId(1)));
    assert!(cmds.is_empty(), "Should not produce commands for non-Review task");
}

#[test]
fn finish_task_without_worktree_is_noop() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Review),
    ], Duration::from_secs(300));

    let cmds = app.update(Message::FinishTask(TaskId(1)));
    assert!(cmds.is_empty(), "Should not produce commands without worktree");
}

#[test]
fn finish_task_shows_confirmation() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t
    }], Duration::from_secs(300));

    app.update(Message::FinishTask(TaskId(1)));
    assert!(matches!(app.input.mode, InputMode::ConfirmFinish(TaskId(1))));
    assert!(app.status_message.as_ref().unwrap().contains("merge"));
}

#[test]
fn confirm_finish_emits_finish_command() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t.tmux_window = Some("task-1".to_string());
        t
    }], Duration::from_secs(300));

    app.update(Message::FinishTask(TaskId(1)));
    let cmds = app.update(Message::ConfirmFinish);
    assert!(cmds.iter().any(|c| matches!(c, Command::Finish { .. })));
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn cancel_finish_returns_to_normal() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t
    }], Duration::from_secs(300));

    app.update(Message::FinishTask(TaskId(1)));
    app.update(Message::CancelFinish);
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.status_message.is_none());
}

#[test]
fn finish_complete_moves_to_done() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t.tmux_window = Some("task-1".to_string());
        t
    }], Duration::from_secs(300));

    let cmds = app.update(Message::FinishComplete(TaskId(1)));
    let task = app.tasks().iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Done);
    assert!(task.worktree.is_none());
    assert!(task.tmux_window.is_none());
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistTask(_))));
}

#[test]
fn finish_failed_with_conflict_sets_flag() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t
    }], Duration::from_secs(300));

    app.update(Message::FinishFailed {
        id: TaskId(1),
        error: "Merge conflict".to_string(),
        is_conflict: true,
    });
    assert!(app.merge_conflict_tasks().contains(&TaskId(1)));
    assert!(app.status_message.as_ref().unwrap().contains("Merge conflict"));
}

#[test]
fn finish_failed_without_conflict_does_not_set_flag() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t
    }], Duration::from_secs(300));

    app.update(Message::FinishFailed {
        id: TaskId(1),
        error: "Not on main".to_string(),
        is_conflict: false,
    });
    assert!(!app.merge_conflict_tasks().contains(&TaskId(1)));
}

#[test]
fn conflict_flag_clears_on_dispatch() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t
    }], Duration::from_secs(300));

    app.update(Message::FinishFailed {
        id: TaskId(1),
        error: "conflict".to_string(),
        is_conflict: true,
    });
    assert!(app.merge_conflict_tasks().contains(&TaskId(1)));

    app.update(Message::Resumed { id: TaskId(1), tmux_window: "task-1".to_string() });
    assert!(!app.merge_conflict_tasks().contains(&TaskId(1)));
}

#[test]
fn conflict_flag_clears_on_move_backward() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t
    }], Duration::from_secs(300));

    app.update(Message::FinishFailed {
        id: TaskId(1),
        error: "conflict".to_string(),
        is_conflict: true,
    });

    app.update(Message::MoveTask { id: TaskId(1), direction: MoveDirection::Backward });
    assert!(!app.merge_conflict_tasks().contains(&TaskId(1)));
}

#[test]
fn confirm_finish_clears_conflict_flag() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t.tmux_window = Some("task-1".to_string());
        t
    }], Duration::from_secs(300));

    app.update(Message::FinishFailed {
        id: TaskId(1),
        error: "conflict".to_string(),
        is_conflict: true,
    });

    app.update(Message::FinishTask(TaskId(1)));
    app.update(Message::ConfirmFinish);
    assert!(!app.merge_conflict_tasks().contains(&TaskId(1)));
}

#[test]
fn f_key_on_review_task_starts_finish() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t
    }], Duration::from_secs(300));
    app.update(Message::NavigateColumn(3));

    app.handle_key(make_key(KeyCode::Char('f')));
    assert!(matches!(app.input.mode, InputMode::ConfirmFinish(_)));
}

#[test]
fn f_key_on_non_review_task_is_noop() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Ready),
    ], Duration::from_secs(300));
    app.update(Message::NavigateColumn(1));

    app.handle_key(make_key(KeyCode::Char('f')));
    assert_eq!(app.input.mode, InputMode::Normal);
}

#[test]
fn confirm_finish_y_key_emits_command() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t.tmux_window = Some("task-1".to_string());
        t
    }], Duration::from_secs(300));
    app.update(Message::NavigateColumn(3));

    app.handle_key(make_key(KeyCode::Char('f')));
    let cmds = app.handle_key(make_key(KeyCode::Char('y')));
    assert!(cmds.iter().any(|c| matches!(c, Command::Finish { .. })));
}

#[test]
fn confirm_finish_n_key_cancels() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-task-1".to_string());
        t
    }], Duration::from_secs(300));
    app.update(Message::NavigateColumn(3));

    app.handle_key(make_key(KeyCode::Char('f')));
    app.handle_key(make_key(KeyCode::Char('n')));
    assert_eq!(app.input.mode, InputMode::Normal);
    assert!(app.status_message.is_none());
}

// --- truncate_title ---

#[test]
fn truncate_title_short() {
    assert_eq!(super::truncate_title("Fix bug", 30), "\"Fix bug\"");
}

#[test]
fn truncate_title_exact_limit() {
    let title = "a".repeat(30);
    assert_eq!(super::truncate_title(&title, 30), format!("\"{}\"", title));
}

#[test]
fn truncate_title_over_limit() {
    let title = "Refactor the authentication middleware system";
    assert_eq!(super::truncate_title(title, 30), "\"Refactor the authentication...\"");
}

#[test]
fn truncate_title_multibyte_chars() {
    // Multi-byte UTF-8 characters must not panic on truncation
    let title = "Fix the caf\u{00e9} rendering bug now";
    // 31 chars, should truncate at char boundary not byte boundary
    assert!(super::truncate_title(title, 10).ends_with("...\""));
}

#[test]
fn confirm_delete_start_running_with_worktree_shows_warning() {
    let mut task = make_task(4, TaskStatus::Running);
    task.worktree = Some("/wt/4-test".to_string());
    let mut app = App::new(vec![task], Duration::from_secs(300));
    // Task is in Running column (column 2), navigate there
    app.selection_mut().set_column(2);
    app.update(Message::ConfirmDeleteStart);
    assert_eq!(app.input.mode, InputMode::ConfirmDelete);
    assert_eq!(
        app.status_message.as_deref(),
        Some("Delete \"Task 4\" [running] (has worktree)? (y/n)")
    );
}

// ---------------------------------------------------------------------------
// Done confirmation tests
// ---------------------------------------------------------------------------

#[test]
fn move_review_to_done_enters_confirm_mode() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Review),
    ], Duration::from_secs(300));
    app.selection_mut().set_column(3); // Review column

    let cmds = app.handle_key(make_key(KeyCode::Char('m')));
    assert!(cmds.is_empty());
    assert!(matches!(app.input.mode, InputMode::ConfirmDone(TaskId(1))));
    assert!(app.status_message.as_deref().unwrap().contains("Done"));
}

#[test]
fn confirm_done_y_moves_task() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Review),
    ], Duration::from_secs(300));
    app.selection_mut().set_column(3);

    app.input.mode = InputMode::ConfirmDone(TaskId(1));
    let cmds = app.handle_key(make_key(KeyCode::Char('y')));
    assert_eq!(app.input.mode, InputMode::Normal);
    let task = app.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Done);
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistTask(_))));
}

#[test]
fn confirm_done_n_cancels() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Review),
    ], Duration::from_secs(300));
    app.selection_mut().set_column(3);

    app.input.mode = InputMode::ConfirmDone(TaskId(1));
    let cmds = app.handle_key(make_key(KeyCode::Char('n')));
    assert_eq!(app.input.mode, InputMode::Normal);
    let task = app.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Review);
    assert!(cmds.is_empty());
}

#[test]
fn move_ready_to_running_no_confirmation() {
    let mut app = App::new(vec![
        make_task(1, TaskStatus::Ready),
    ], Duration::from_secs(300));
    app.selection_mut().set_column(1); // Ready column

    let cmds = app.handle_key(make_key(KeyCode::Char('m')));
    assert_eq!(app.input.mode, InputMode::Normal);
    let task = app.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Running);
    assert!(cmds.iter().any(|c| matches!(c, Command::PersistTask(_))));
}

#[test]
fn confirm_done_does_not_cleanup_worktree() {
    let mut app = App::new(vec![{
        let mut t = make_task(1, TaskStatus::Review);
        t.worktree = Some("/repo/.worktrees/1-test".to_string());
        t.tmux_window = Some("task-1".to_string());
        t
    }], Duration::from_secs(300));
    app.selection_mut().set_column(3);

    // Enter confirm mode and confirm
    app.update(Message::MoveTask { id: TaskId(1), direction: MoveDirection::Forward });
    assert!(matches!(app.input.mode, InputMode::ConfirmDone(TaskId(1))));

    let cmds = app.update(Message::ConfirmDone);
    // No Cleanup command — worktree stays for archive to clean up later
    assert!(!cmds.iter().any(|c| matches!(c, Command::Cleanup { .. })));
    let task = app.tasks.iter().find(|t| t.id == TaskId(1)).unwrap();
    assert_eq!(task.status, TaskStatus::Done);
    // Worktree is preserved (not taken)
    assert!(task.worktree.is_some());
}
